//! Validation and translation of the versioned, read-only graphics shader profile.
//!
//! Source bytes and copied reflection are authoritative; no backend accepts a
//! client native shader binary that bypasses these checks. Binding groups follow
//! the stage split: vertex sampled pairs 0, vertex uniforms 1, fragment pairs 2,
//! fragment uniforms 3. Each sampled pair uses texture=2*slot, sampler=2*slot+1.

use neocogi_render_core::abi::GpuShaderStage;
use neocogi_shader_core::{Shader, IoLocation, IoShape, Uniform, SampledTexture, ShaderScalar, ShaderInterpolation, ShaderSampling};
use naga::{AddressSpace, Binding, Handle, ScalarKind, ShaderStage, Type, TypeInner};

/// Maximum UTF-8 source length admitted before parsing.
pub const MAX_SHADER_BYTES: usize = 1024 * 1024;
/// Combined owned WGSL, GLSL, SPIR-V and entry-name bytes in one portable artifact.
pub const MAX_SHADER_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;

/// Factory errors keep malformed programs, exhausted output quota and allocation failure distinct.
#[derive(Debug)]
pub(crate) enum CompileError {
    /// Invalid/unsupported source or compiler emission failure.
    Invalid(String),
    /// Admitted output cannot fit the hard profile or remaining domain-byte budget.
    Limit,
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
            Self::Allocation => output.write_str("shader artifact allocation failed"),
        }
    }
}

/// Private stateless translation engine; only the COM provider publishes outputs.
pub(crate) struct Compiler;

/// GLSL expansion cannot grow an unrestricted String before domain artifact admission.
struct Output {
    /// Complete initialized prefix only; allocation growth is capped at the selected limit.
    text: String,
    /// Remaining combined artifact budget after source, entry and SPIR-V bytes.
    limit: usize,
    /// First writer failure survives Naga's formatting-error conversion.
    failure: Option<WriteFailure>,
}
/// Exact cause of a failed bounded formatting operation.
#[derive(Clone, Copy)]
enum WriteFailure {
    /// Requested output would exceed the selected budget.
    Limit,
    /// A bounded reservation failed.
    Allocation,
}
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
    /// Parses, validates the complete module, reflects one entry and emits both native artifacts.
    /// Driver allocation/linking remains a later fallible preparation operation.
    #[cfg(test)]
    pub fn compile(stage: GpuShaderStage, source: &str, entry: &str) -> Result<Shader, String> {
        Self::compile_bounded(stage, source, entry, MAX_SHADER_ARTIFACT_BYTES).map_err(|error| error.to_string())
    }

    /// The factory supplies its actual remaining live-byte quota before generated output expands.
    pub(crate) fn compile_bounded(stage: GpuShaderStage, source: &str, entry: &str, budget: usize) -> Result<Shader, CompileError> {
        if source.is_empty() || source.len() > MAX_SHADER_BYTES || entry.is_empty() || entry.len() > 128 {
            return Err("shader source or entry length exceeds the graphics profile".into());
        }
        let native_stage = native_stage(stage)?;
        let fixed = source.len().checked_add(entry.len()).ok_or(CompileError::Limit)?;
        let available = budget.min(MAX_SHADER_ARTIFACT_BYTES).checked_sub(fixed).ok_or(CompileError::Limit)?;
        let module = naga::front::wgsl::parse_str(source).map_err(|error| error.emit_to_string(source))?;
        validate_module(&module)?;
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .map_err(|error| error.emit_to_string(source))?;
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
        if matches!(stage, GpuShaderStage::Fragment)
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
        let spv_options = naga::back::spv::Options {
            flags: naga::back::spv::WriterFlags::ADJUST_COORDINATE_SPACE
                | naga::back::spv::WriterFlags::LABEL_VARYINGS
                | naga::back::spv::WriterFlags::CLAMP_FRAG_DEPTH,
            bounds_check_policies: bounds_checks(),
            ..Default::default()
        };
        let spirv = naga::back::spv::write_vec(
            &module,
            &info,
            &spv_options,
            Some(&naga::back::spv::PipelineOptions {
                shader_stage: native_stage,
                entry_point: entry.into(),
            }),
        )
        .map_err(|error| error.to_string())?;
        let available = available
            .checked_sub(spirv.len().checked_mul(4).ok_or(CompileError::Limit)?)
            .ok_or(CompileError::Limit)?;
        // Naga's GLSL writer currently applies image-load policies but does not lower array/
        // buffer index policies. Reimport its guarded SPIR-V so those explicit branches become
        // ordinary IR before GLSL emission. The importer restores canonical vertex coordinates;
        // the final GLSL writer then performs its usual target conversion exactly once.
        let gl_module = naga::front::spv::Frontend::new(
            spirv.iter().copied(),
            &naga::front::spv::Options {
                adjust_coordinate_space: true,
                ..Default::default()
            },
        )
        .parse()
        .map_err(|error| error.to_string())?;
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
            limit: available,
            failure: None,
        };
        let emission =
            naga::back::glsl::Writer::new(&mut output, &gl_module, &gl_info, &options, &pipeline, bounds_checks()).and_then(|mut writer| writer.write());
        let reflection = emission.map_err(|error| match output.failure {
            Some(WriteFailure::Limit) => CompileError::Limit,
            Some(WriteFailure::Allocation) => CompileError::Allocation,
            None => CompileError::Invalid(error.to_string()),
        })?;
        let glsl = output.text;
        let mut uniforms = Vec::new();
        let mut textures = Vec::new();
        let usage = info.get_entry_point(entry_index);
        let (texture_group, uniform_group) = groups(stage)?;
        for (handle, variable) in module.global_variables.iter() {
            if usage[handle].is_empty() {
                continue;
            }
            let Some(binding) = variable.binding else {
                continue;
            };
            match variable.space {
                AddressSpace::Uniform if binding.group == uniform_group && binding.binding < 4 => {
                    let (_, byte_len) = uniform_layout(&module, variable.ty)?;
                    let actual_len = match module.types[variable.ty].inner {
                        TypeInner::Struct { span, .. } => span,
                        _ => return Err("uniform inputs must be named structures".into()),
                    };
                    // Native UBO allocation can pad the final struct, but member
                    // offsets/strides must already agree with std140.
                    if actual_len > byte_len || actual_len > 16 * 1024 {
                        return Err("uniform block exceeds the portable layout/size".into());
                    }
                    uniforms.push(Uniform {
                        slot: binding.binding,
                        byte_len: actual_len,
                        gl_block: gl_module
                            .global_variables
                            .iter()
                            .find(|(_, global)| global.binding == Some(binding) && global.space == AddressSpace::Uniform)
                            .and_then(|(handle, _)| reflection.uniforms.get(&handle))
                            .cloned()
                            .ok_or("missing emitted uniform reflection")?,
                    });
                }
                AddressSpace::Handle if binding.group == texture_group => {}
                _ => return Err("entry point uses a resource from another stage or an unsupported binding group".into()),
            }
        }
        for (gl_name, mapping) in &reflection.texture_mapping {
            let image = gl_module.global_variables[mapping.texture].binding.ok_or("sampled texture lacks a binding")?;
            let sampler_handle = mapping.sampler.ok_or("graphics profile requires a paired sampler")?;
            let sampler = gl_module.global_variables[sampler_handle].binding.ok_or("sampler lacks a binding")?;
            if image.group != texture_group
                || sampler.group != texture_group
                || image.binding >= 8
                || image.binding % 2 != 0
                || sampler.binding != image.binding + 1
            {
                return Err("sampled textures require stage-local consecutive texture/sampler pairs".into());
            }
            if textures.iter().any(|existing: &SampledTexture| existing.slot == image.binding / 2) {
                return Err("one texture slot cannot use multiple sampler combinations".into());
            }
            textures.push(SampledTexture {
                slot: image.binding / 2,
                gl_name: gl_name.clone(),
            });
        }
        uniforms.sort_by_key(|uniform| uniform.slot);
        textures.sort_by_key(|texture| texture.slot);
        Ok(Shader {
            stage,
            entry: entry.into(),
            wgsl: source.into(),
            glsl,
            spirv,
            uniforms,
            textures,
            inputs,
            outputs,
        })
    }
}

/// Naga is a translator and defaults to unchecked indexing. Clients do not gain authority
/// beyond their own read-only bindings or private computation through undefined array access.
/// Hardware robustness is deliberately not assumed by the direct GL/Vulkan adapters.
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
fn validate_module(module: &naga::Module) -> Result<(), String> {
    if module
        .entry_points
        .iter()
        .any(|entry| !matches!(entry.stage, ShaderStage::Vertex | ShaderStage::Fragment))
    {
        return Err("graphics profile rejects compute and other non-graphics entry points".into());
    }
    // WGPU receives the canonical module, not just the selected emitted entry.
    // Reject extra attachments in every fragment entry rather than letting an
    // unused entry hide a different resource/effect contract in that module.
    for entry in &module.entry_points {
        if entry.stage == ShaderStage::Fragment {
            let mut outputs = Vec::new();
            if let Some(result) = &entry.function.result {
                collect_io(module, result.ty, result.binding.as_ref(), &mut outputs)?;
            }
            if outputs.len() != 1
                || outputs[0].location != 0
                || outputs[0].shape
                    != (IoShape {
                        kind: ShaderScalar::Float32,
                        components: 4,
                    })
            {
                return Err("fragment profile requires exactly one vec4<f32> color output at location 0".into());
            }
        }
    }
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
                uniform_layout(module, variable.ty)?;
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
