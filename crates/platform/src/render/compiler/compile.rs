//! Validation and translation of the versioned, read-only graphics shader profile.
//!
//! Source bytes and copied reflection are authoritative; no backend accepts a
//! client native shader binary that bypasses these checks. Binding groups follow
//! the stage split: vertex sampled pairs 0, vertex uniforms 1, fragment pairs 2,
//! fragment uniforms 3. Each sampled pair uses texture=2*slot, sampler=2*slot+1.

use neocogi_platform::render::abi::GpuShaderStage;
use neocogi_platform::render::shader::{
    Shader, ShaderCode, ShaderTarget, IoLocation, IoShape, Uniform, SampledTexture, ShaderScalar, ShaderInterpolation, ShaderSampling,
};
mod images;

use naga::{AddressSpace, Binding, Handle, ScalarKind, ShaderStage, Type, TypeInner};

use neocogi_platform::render::shader::{MAX_SHADER_BYTES, MAX_SHADER_ARTIFACT_BYTES};

/// Factory errors keep malformed programs, exhausted output quota and allocation failure distinct.
#[derive(Debug)]
pub(crate) enum CompileError {
    /// Invalid/unsupported source or compiler emission failure.
    Invalid(String),
    /// Admitted output cannot fit the hard profile or remaining domain-byte budget.
    Limit,
    /// Known output format is absent from this compiler build.
    Unavailable,
    /// Bounded output reservation failed without publishing a partial artifact.
    Allocation,
}
impl From<String> for CompileError {
    /// Preserve compiler diagnostics while keeping quota failures typed separately.
    fn from(value: String) -> Self {
        Self::Invalid(value)
    }
}
impl From<&str> for CompileError {
    /// Static profile diagnostics use the same invalid-program category.
    fn from(value: &str) -> Self {
        Self::Invalid(value.into())
    }
}
impl std::fmt::Display for CompileError {
    /// Pure Rust compilation exposes the same useful diagnostics without an ABI error-code wrapper.
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => output.write_str(message),
            Self::Limit => output.write_str("shader artifact byte budget exceeded"),
            Self::Unavailable => output.write_str("shader compiler output is not enabled in this build"),
            Self::Allocation => output.write_str("shader artifact allocation failed"),
        }
    }
}

/// Immutable renderer configuration; requests cannot change its admitted output format.
pub(crate) struct Compiler {
    /// Selected once by the renderer, before any source is compiled.
    target: ShaderTarget,
}

/// GLSL expansion cannot grow an unrestricted String before domain artifact admission.
#[cfg(feature = "shader-glsl")]
struct Output {
    /// Complete initialized prefix only; allocation growth is capped at the selected limit.
    text: String,
    /// Maximum selected GLSL bytes after accounting for the entry name.
    limit: usize,
    /// First writer failure survives Naga's formatting-error conversion.
    failure: Option<WriteFailure>,
}
/// Exact cause of a failed bounded formatting operation.
#[derive(Clone, Copy)]
#[cfg(feature = "shader-glsl")]
enum WriteFailure {
    /// Requested output would exceed the selected budget.
    Limit,
    /// A bounded reservation failed.
    Allocation,
}
#[cfg(feature = "shader-glsl")]
impl std::fmt::Write for Output {
    /// Check complete writes before reserving or appending, including large initializer expansion.
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        if self.failure.is_some() {
            return Err(std::fmt::Error);
        }
        let Some(required) = self.text.len().checked_add(value.len()).filter(|length| *length <= self.limit) else {
            self.failure = Some(WriteFailure::Limit);
            return Err(std::fmt::Error);
        };
        if required > self.text.capacity() {
            let capacity = required.max(self.text.capacity().saturating_mul(2)).min(self.limit);
            if self.text.try_reserve_exact(capacity - self.text.len()).is_err() {
                self.failure = Some(WriteFailure::Allocation);
                return Err(std::fmt::Error);
            }
        }
        self.text.push_str(value);
        Ok(())
    }
}

impl Compiler {
    /// Test convenience uses the portable WGSL target unless a test selects another explicitly.
    #[cfg(test)]
    pub fn compile(stage: GpuShaderStage, source: &str, entry: &str) -> Result<Shader, String> {
        Self::new(ShaderTarget::Wgsl)
            .and_then(|compiler| compiler.compile_bounded(stage, source, entry, MAX_SHADER_ARTIFACT_BYTES))
            .map_err(|error| error.to_string())
    }

    /// Bind a compiler to one available output; reject unavailable configurations before compilation.
    pub(crate) fn new(target: ShaderTarget) -> Result<Self, CompileError> {
        if !matches!(target, ShaderTarget::Wgsl | ShaderTarget::SpirV | ShaderTarget::GlslEs300) {
            return Err("unknown shader target".into());
        }
        let enabled = match target {
            ShaderTarget::Wgsl => cfg!(any(test, feature = "shader-wgsl")),
            ShaderTarget::SpirV => cfg!(feature = "shader-spirv"),
            ShaderTarget::GlslEs300 => cfg!(feature = "shader-glsl"),
            _ => unreachable!(),
        };
        if !enabled {
            return Err(CompileError::Unavailable);
        }
        Ok(Self { target })
    }

    /// Validate the whole module and emit the renderer's fixed format, retaining no scratch IR.
    /// The output cap covers only the entry and emitted code.
    pub(crate) fn compile_bounded(&self, stage: GpuShaderStage, source: &str, entry: &str, budget: usize) -> Result<Shader, CompileError> {
        let target = self.target;
        if source.is_empty() || source.len() > MAX_SHADER_BYTES || entry.is_empty() || entry.len() > 128 {
            return Err("shader source or entry length exceeds the graphics profile".into());
        }
        let native_stage = native_stage(stage)?;
        let available = budget.min(MAX_SHADER_ARTIFACT_BYTES).checked_sub(entry.len()).ok_or(CompileError::Limit)?;
        let module = naga::front::wgsl::parse_str(source).map_err(|error| error.emit_to_string(source))?;
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .map_err(|error| error.emit_to_string(source))?;
        validate_module(&module, &info)?;
        let (entry_index, selected) = module
            .entry_points
            .iter()
            .enumerate()
            .find(|(_, point)| point.stage == native_stage && point.name == entry)
            .ok_or("requested graphics entry point does not exist")?;
        let mut inputs = Vec::new();
        for argument in &selected.function.arguments {
            collect_io(&module, argument.ty, argument.binding.as_ref(), &mut inputs)?;
        }
        let mut outputs = Vec::new();
        if let Some(result) = &selected.function.result {
            collect_io(&module, result.ty, result.binding.as_ref(), &mut outputs)?;
        }
        let usage = info.get_entry_point(entry_index);
        let mut uniforms = Vec::new();
        for (handle, variable) in module.global_variables.iter() {
            if usage[handle].is_empty() || variable.space != AddressSpace::Uniform {
                continue;
            }
            let binding = variable.binding.ok_or("admitted uniform lost its binding")?;
            uniforms.push(Uniform {
                slot: binding.binding,
                byte_len: uniform_block_len(&module, variable.ty)?,
                gl_block: None,
            });
        }
        let mut textures = Vec::new();
        for pair in &usage.sampling_set {
            let image = module.global_variables[pair.image].binding.ok_or("sampled texture lacks a binding")?;
            textures.push(SampledTexture { slot: image.binding / 2, gl_name: None });
        }
        uniforms.sort_by_key(|value| value.slot);
        textures.sort_by_key(|value| value.slot);
        let code = match target {
            ShaderTarget::Wgsl => {
                if source.len() > available {
                    return Err(CompileError::Limit);
                }
                ShaderCode::Wgsl(owned_text(source)?)
            }
            #[cfg(feature = "shader-spirv")]
            ShaderTarget::SpirV => {
                let words = emit_spirv(&module, &info, native_stage, entry)?;
                if words.len().checked_mul(4).ok_or(CompileError::Limit)? > available {
                    return Err(CompileError::Limit);
                }
                ShaderCode::SpirV(words)
            }
            #[cfg(feature = "shader-glsl")]
            ShaderTarget::GlslEs300 => {
                let words = emit_spirv(&module, &info, native_stage, entry)?;
                // Import guarded words to lower array and buffer checks before GLSL.
                // Intermediate words die here and are never charged or exported as shader code.
                ShaderCode::GlslEs300(
                    SelectedEntry {
                        module: &module,
                        info: &info,
                        entry_index,
                        stage,
                        native_stage,
                        entry,
                    }
                    .emit_glsl(&words, available, &mut uniforms, &mut textures)?,
                )
            }
            _ => return Err(CompileError::Unavailable),
        };
        Ok(Shader {
            stage,
            entry: owned_text(entry)?,
            code,
            inputs,
            outputs,
            uniforms,
            textures,
        })
    }
}

/// Owns bounded text fallibly before it can be published as code or an entry name.
fn owned_text(text: &str) -> Result<String, CompileError> {
    let mut owned = String::new();
    owned.try_reserve_exact(text.len()).map_err(|_| CompileError::Allocation)?;
    owned.push_str(text);
    Ok(owned)
}

/// Emits guarded Vulkan words, also used transiently by GL's bounds-check lowering.
#[cfg(any(feature = "shader-spirv", feature = "shader-glsl"))]
fn emit_spirv(module: &naga::Module, info: &naga::valid::ModuleInfo, native_stage: ShaderStage, entry: &str) -> Result<Vec<u32>, CompileError> {
    let spv_options = naga::back::spv::Options {
        flags: naga::back::spv::WriterFlags::ADJUST_COORDINATE_SPACE
            | naga::back::spv::WriterFlags::LABEL_VARYINGS
            | naga::back::spv::WriterFlags::CLAMP_FRAG_DEPTH,
        bounds_check_policies: bounds_checks(),
        ..Default::default()
    };
    let spirv = naga::back::spv::write_vec(
        module,
        info,
        &spv_options,
        Some(&naga::back::spv::PipelineOptions {
            shader_stage: native_stage,
            entry_point: entry.into(),
        }),
    )
    .map_err(|error| error.to_string())?;
    Ok(spirv)
}

/// Immutable view of the one validated entry selected for native GL emission.
#[cfg(feature = "shader-glsl")]
struct SelectedEntry<'a> {
    /// Original validated module; native names are recovered by its explicit bindings.
    module: &'a naga::Module,
    /// Validation and resource-usage information for that same module.
    info: &'a naga::valid::ModuleInfo,
    /// Selected entry's index in the original module.
    entry_index: usize,
    /// Portable stage used to authenticate stage-local native reflection.
    stage: GpuShaderStage,
    /// Corresponding Naga stage for its target-specific writer.
    native_stage: ShaderStage,
    /// Selected original entry name.
    entry: &'a str,
}
#[cfg(feature = "shader-glsl")]
impl SelectedEntry<'_> {
    /// Produces bounded ES300 and attaches names only to existing portable reflection slots.
    fn emit_glsl(&self, spirv: &[u32], limit: usize, uniforms: &mut [Uniform], textures: &mut [SampledTexture]) -> Result<String, CompileError> {
        let Self {
            module,
            info,
            entry_index,
            stage,
            native_stage,
            entry,
        } = *self;
        let mut gl_module = naga::front::spv::Frontend::new(
            spirv.iter().copied(),
            &naga::front::spv::Options {
                adjust_coordinate_space: true,
                ..Default::default()
            },
        )
        .parse()
        .map_err(|error| error.to_string())?;
        images::specialize_gl(&mut gl_module);
        let gl_info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&gl_module)
            .map_err(|error| error.to_string())?;
        let options = naga::back::glsl::Options {
            writer_flags: naga::back::glsl::WriterFlags::ADJUST_COORDINATE_SPACE,
            version: naga::back::glsl::Version::Embedded { version: 300, is_webgl: true },
            ..Default::default()
        };
        let pipeline = naga::back::glsl::PipelineOptions {
            shader_stage: native_stage,
            entry_point: entry.into(),
            multiview: None,
        };
        let mut output = Output {
            text: String::new(),
            limit,
            failure: None,
        };
        let emission = naga::back::glsl::Writer::new(
            &mut output,
            &gl_module,
            &gl_info,
            &options,
            &pipeline,
            naga::proc::BoundsCheckPolicies {
                // Coordinate/level checks are explicit branches imported from guarded
                // SPIR-V; another writer guard would require unavailable ES300 queries.
                image_load: naga::proc::BoundsCheckPolicy::Unchecked,
                ..bounds_checks()
            },
        )
        .and_then(|mut writer| writer.write());
        let reflection = emission.map_err(|error| match output.failure {
            Some(WriteFailure::Limit) => CompileError::Limit,
            Some(WriteFailure::Allocation) => CompileError::Allocation,
            None => CompileError::Invalid(error.to_string()),
        })?;
        let glsl = output.text;
        let usage = info.get_entry_point(entry_index);
        let (_, uniform_group) = groups(stage)?;
        for uniform in uniforms.iter_mut() {
            let (handle, variable) = module
                .global_variables
                .iter()
                .find(|(_, variable)| {
                    variable.space == AddressSpace::Uniform
                        && variable
                            .binding
                            .is_some_and(|binding| binding.binding == uniform.slot && binding.group == uniform_group)
                })
                .ok_or("selected uniform lost its binding")?;
            if usage[handle].is_empty() || variable.space != AddressSpace::Uniform {
                continue;
            }
            let binding = variable.binding.ok_or("admitted uniform lost its binding")?;
            uniform.gl_block = Some(
                gl_module
                    .global_variables
                    .iter()
                    .find(|(_, global)| global.binding == Some(binding) && global.space == AddressSpace::Uniform)
                    .and_then(|(handle, _)| reflection.uniforms.get(&handle))
                    .cloned()
                    .ok_or("missing emitted uniform reflection")?,
            );
        }
        for (gl_name, mapping) in &reflection.texture_mapping {
            let image = gl_module.global_variables[mapping.texture].binding.ok_or("sampled texture lacks a binding")?;
            // The emitter must preserve the admitted sampled pair; its name is selected-output data.
            mapping.sampler.ok_or("emitted texture reflection lost its paired sampler")?;
            let texture = textures
                .iter_mut()
                .find(|texture| texture.slot == image.binding / 2)
                .ok_or("emitted sampler is outside selected reflection")?;
            texture.gl_name = Some(gl_name.clone());
        }
        if textures.iter().any(|texture| texture.gl_name.is_none()) {
            return Err("selected texture lacks emitted native name".into());
        }
        Ok(glsl)
    }
}

/// Naga is a translator and defaults to unchecked indexing. Clients do not gain authority
/// beyond their own read-only bindings or private computation through undefined array access.
/// Hardware robustness is deliberately not assumed by the direct GL/Vulkan adapters.
#[cfg(any(feature = "shader-spirv", feature = "shader-glsl"))]
fn bounds_checks() -> naga::proc::BoundsCheckPolicies {
    use naga::proc::BoundsCheckPolicy::ReadZeroSkipWrite;
    naga::proc::BoundsCheckPolicies {
        index: ReadZeroSkipWrite,
        buffer: ReadZeroSkipWrite,
        image_load: ReadZeroSkipWrite,
        binding_array: ReadZeroSkipWrite,
    }
}

/// Converts the copied ABI stage without relying on enum representation.
pub fn native_stage(stage: GpuShaderStage) -> Result<ShaderStage, String> {
    match stage {
        GpuShaderStage::Vertex => Ok(ShaderStage::Vertex),
        GpuShaderStage::Fragment => Ok(ShaderStage::Fragment),
        _ => Err("unknown graphics shader stage".into()),
    }
}

/// Returns sampled-pair and uniform groups for one portable stage.
pub fn groups(stage: GpuShaderStage) -> Result<(u32, u32), String> {
    match stage {
        GpuShaderStage::Vertex => Ok((0, 1)),
        GpuShaderStage::Fragment => Ok((2, 3)),
        _ => Err("unknown graphics shader stage".into()),
    }
}

/// Rejects forbidden effects even in unused globals/entry points; native artifacts cannot bypass it.
fn validate_module(module: &naga::Module, info: &naga::valid::ModuleInfo) -> Result<(), String> {
    images::validate(module)?;
    for (_, ty) in module.types.iter() {
        if matches!(ty.inner, TypeInner::Atomic(_) | TypeInner::BindingArray { .. }) {
            return Err("graphics profile rejects atomic and bindless types".into());
        }
    }
    for (_, variable) in module.global_variables.iter() {
        match variable.space {
            AddressSpace::Private => {
                if variable.binding.is_some() {
                    return Err("private variables cannot carry resource bindings".into());
                }
            }
            AddressSpace::Uniform => {
                let binding = variable.binding.ok_or("uniform lacks explicit stage/slot binding")?;
                if !matches!(binding.group, 1 | 3) || binding.binding >= 4 {
                    return Err("uniform group/slot exceeds graphics profile".into());
                }
                uniform_block_len(module, variable.ty)?;
            }
            AddressSpace::Handle => {
                let binding = variable.binding.ok_or("sampled input lacks explicit binding")?;
                if !matches!(binding.group, 0 | 2) || binding.binding >= 8 {
                    return Err("sampled input group/slot exceeds graphics profile".into());
                }
                match module.types[variable.ty].inner {
                    TypeInner::Image {
                        dim: naga::ImageDimension::D2,
                        arrayed: false,
                        class: naga::ImageClass::Sampled { kind: ScalarKind::Float, multi: false },
                    } if binding.binding % 2 == 0 => {}
                    TypeInner::Sampler { comparison: false } if binding.binding % 2 == 1 => {}
                    _ => return Err("graphics profile admits only paired read-only 2D float textures and ordinary samplers".into()),
                }
            }
            _ => return Err("graphics profile rejects storage, workgroup, immediate and other side-effect resource spaces".into()),
        }
    }
    // WGPU receives the complete WGSL. Every entry must have the same portable contract,
    // even if a different entry is selected for native emission and exported reflection.
    for (index, entry) in module.entry_points.iter().enumerate() {
        let stage = match entry.stage {
            ShaderStage::Vertex => GpuShaderStage::Vertex,
            ShaderStage::Fragment => GpuShaderStage::Fragment,
            _ => return Err("graphics profile rejects compute and other non-graphics entry points".into()),
        };
        let (texture_group, uniform_group) = groups(stage)?;
        let mut inputs = Vec::new();
        for argument in &entry.function.arguments {
            collect_io(module, argument.ty, argument.binding.as_ref(), &mut inputs)?;
        }
        let mut outputs = Vec::new();
        if let Some(result) = &entry.function.result {
            collect_io(module, result.ty, result.binding.as_ref(), &mut outputs)?;
        }
        if entry.stage == ShaderStage::Fragment
            && (outputs.len() != 1
                || outputs[0].location != 0
                || outputs[0].shape
                    != (IoShape {
                        kind: ShaderScalar::Float32,
                        components: 4,
                    }))
        {
            return Err("fragment profile requires exactly one vec4<f32> color output at location 0".into());
        }
        let usage = info.get_entry_point(index);
        for (handle, variable) in module.global_variables.iter() {
            if usage[handle].is_empty() {
                continue;
            }
            match variable.space {
                AddressSpace::Private => {}
                AddressSpace::Uniform if variable.binding.is_some_and(|binding| binding.group == uniform_group) => {}
                AddressSpace::Handle if variable.binding.is_some_and(|binding| binding.group == texture_group) => {}
                _ => return Err("entry point uses a resource from another stage or an unsupported binding group".into()),
            }
        }
        let mut images = std::collections::BTreeSet::new();
        let mut slots = std::collections::BTreeSet::new();
        for pair in &usage.sampling_set {
            let image = module.global_variables[pair.image].binding.ok_or("sampled texture lacks a binding")?;
            let sampler = module.global_variables[pair.sampler].binding.ok_or("sampler lacks a binding")?;
            if image.group != texture_group
                || sampler.group != texture_group
                || image.binding >= 8
                || image.binding % 2 != 0
                || sampler.binding != image.binding + 1
            {
                return Err("sampled textures require stage-local consecutive texture/sampler pairs".into());
            }
            if !images.insert(pair.image) || !slots.insert(image.binding / 2) {
                return Err("one texture slot cannot use multiple sampler combinations".into());
            }
        }
        // Load/query use is absent from sampling_set. Preserve the paired-image profile
        // rather than letting an inactive load-only entry escape the former reflection check.
        for (handle, variable) in module.global_variables.iter() {
            if !usage[handle].is_empty() && matches!(module.types[variable.ty].inner, TypeInner::Image { .. }) && !images.contains(&handle) {
                return Err("graphics profile requires a paired sampler".into());
            }
        }
    }
    Ok(())
}

/// Reflects location-bound scalars/vectors recursively through entry-point structs.
fn collect_io(module: &naga::Module, ty: Handle<Type>, binding: Option<&Binding>, output: &mut Vec<IoLocation>) -> Result<(), String> {
    match binding {
        Some(Binding::Location { location, interpolation, sampling, .. }) => {
            if *location >= 16 || output.iter().any(|entry| entry.location == *location) {
                return Err("graphics location exceeds profile or is duplicated".into());
            }
            let (scalar, components) = match module.types[ty].inner {
                TypeInner::Scalar(scalar) => (scalar, 1),
                TypeInner::Vector { size, scalar } => (scalar, size as u8),
                _ => return Err("graphics locations require scalar/vector values".into()),
            };
            if scalar.width != 4 || !matches!(scalar.kind, ScalarKind::Float | ScalarKind::Uint) {
                return Err("graphics locations support f32 and u32 components only".into());
            }
            output.push(IoLocation {
                location: *location,
                shape: IoShape {
                    kind: if scalar.kind == ScalarKind::Float {
                        ShaderScalar::Float32
                    } else {
                        ShaderScalar::Uint32
                    },
                    components,
                },
                interpolation: match interpolation {
                    None => ShaderInterpolation::None,
                    Some(naga::Interpolation::Perspective) => ShaderInterpolation::Perspective,
                    Some(naga::Interpolation::Linear) => ShaderInterpolation::Linear,
                    Some(naga::Interpolation::Flat) => ShaderInterpolation::Flat,
                    Some(naga::Interpolation::PerVertex) => return Err("per-vertex interpolation is outside the portable graphics contract".into()),
                },
                sampling: match sampling {
                    None => ShaderSampling::None,
                    Some(naga::Sampling::Center) => ShaderSampling::Center,
                    Some(naga::Sampling::Centroid) => ShaderSampling::Centroid,
                    Some(naga::Sampling::Sample) => ShaderSampling::Sample,
                    Some(naga::Sampling::First) => ShaderSampling::First,
                    Some(naga::Sampling::Either) => ShaderSampling::Either,
                },
            });
        }
        Some(Binding::BuiltIn(_)) => {}
        None => match &module.types[ty].inner {
            TypeInner::Struct { members, .. } => {
                for member in members {
                    collect_io(module, member.ty, member.binding.as_ref(), output)?;
                }
            }
            _ => return Err("graphics input/output must have explicit locations or builtins".into()),
        },
    }
    output.sort_by_key(|entry| entry.location);
    Ok(())
}

/// Admits an entire named uniform block, including unused declarations, with the portable cap.
fn uniform_block_len(module: &naga::Module, ty: Handle<Type>) -> Result<u32, String> {
    let (_, padded) = uniform_layout(module, ty)?;
    let span = match module.types[ty].inner {
        TypeInner::Struct { span, .. } => span,
        _ => return Err("uniform inputs must be named structures".into()),
    };
    if span > padded || span > 16 * 1024 {
        return Err("uniform block exceeds the portable layout/size".into());
    }
    Ok(span)
}

/// Computes standard uniform alignment/size and verifies WGSL member offsets and strides.
/// Final block padding is permitted; differing matrix/array/member offsets are rejected.
fn uniform_layout(module: &naga::Module, ty: Handle<Type>) -> Result<(u32, u32), String> {
    let rounded = |value: u32, align: u32| {
        value
            .checked_add(align - 1)
            .map(|v| v / align * align)
            .ok_or("uniform layout overflow".to_string())
    };
    match &module.types[ty].inner {
        TypeInner::Scalar(scalar) if scalar.width == 4 && matches!(scalar.kind, ScalarKind::Float | ScalarKind::Uint | ScalarKind::Sint) => Ok((4, 4)),
        TypeInner::Vector { size, scalar } if scalar.width == 4 && matches!(scalar.kind, ScalarKind::Float | ScalarKind::Uint | ScalarKind::Sint) => {
            let count = *size as u32;
            Ok((if count == 2 { 8 } else { 16 }, count * 4))
        }
        TypeInner::Matrix { columns, rows, scalar } if scalar.width == 4 && matches!(rows, naga::VectorSize::Tri | naga::VectorSize::Quad) => {
            Ok((16, *columns as u32 * 16))
        }
        TypeInner::Array {
            base,
            size: naga::ArraySize::Constant(count),
            stride,
        } => {
            let (alignment, size) = uniform_layout(module, *base)?;
            let alignment = alignment.max(16);
            if *stride != rounded(size, alignment)? {
                return Err("uniform array stride differs between WGSL and std140".into());
            }
            Ok((alignment, stride.checked_mul(count.get()).ok_or("uniform array size overflow")?))
        }
        TypeInner::Struct { members, span } => {
            let (mut offset, mut alignment) = (0, 16);
            for member in members {
                let (member_align, size) = uniform_layout(module, member.ty)?;
                alignment = alignment.max(member_align);
                offset = rounded(offset, member_align)?;
                if member.offset != offset {
                    return Err("uniform member offset differs between WGSL and std140".into());
                }
                offset = offset.checked_add(size).ok_or("uniform struct size overflow")?;
            }
            let size = rounded(offset, alignment)?;
            if *span > size {
                return Err("uniform explicit padding differs from portable std140 layout".into());
            }
            Ok((alignment, size))
        }
        _ => Err("uniform type has no portable graphics layout".into()),
    }
}

#[cfg(test)]
mod tests;
