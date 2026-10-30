//! Validation and translation of the versioned, read-only graphics shader profile.
//!
//! Source bytes and copied reflection are authoritative; no backend accepts a
//! client native shader binary that bypasses these checks. Binding groups follow
//! the stage split: vertex sampled pairs 0, vertex uniforms 1, fragment pairs 2,
//! fragment uniforms 3. Each sampled pair uses texture=2*slot, sampler=2*slot+1.

use crate::abi::GpuShaderStage;
use naga::{AddressSpace, Binding, Handle, ScalarKind, ShaderStage, Type, TypeInner};

/// Maximum UTF-8 source length admitted before parsing.
pub const MAX_SHADER_BYTES: usize = 1024 * 1024;

/// Scalar/vector shape compared across independent shader modules and vertex layouts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IoShape {
    /// Floating-point or unsigned integer scalar family.
    pub kind: ScalarKind,
    /// One for scalars, otherwise the vector width.
    pub components: u8,
}

/// One user location in an entry point's input/output interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IoLocation {
    /// Portable location, below sixteen.
    pub location: u32,
    /// Required scalar/vector interpretation.
    pub shape: IoShape,
    /// Cross-stage interpolation must agree even when scalar/vector shapes match.
    pub interpolation: Option<naga::Interpolation>,
    /// Cross-stage sampling qualifier; matching names alone do not establish linkage.
    pub sampling: Option<naga::Sampling>,
}

/// Uniform metadata copied out of compiler IR before publication.
#[derive(Clone, Debug)]
pub struct Uniform {
    /// Stage-local slot, below four.
    pub slot: u32,
    /// Exact WGSL uniform byte length; native end padding is separate.
    pub byte_len: u32,
    /// Reflected generated GLSL block name; never an application-facing convention.
    pub gl_block: String,
}

/// Combined texture/sampler binding in emitted GLSL.
#[derive(Clone, Debug)]
pub struct SampledTexture {
    /// Stage-local slot shared by the texture and its corresponding sampler.
    pub slot: u32,
    /// Reflected GLSL combined-sampler uniform name.
    pub gl_name: String,
}

/// Validated CPU artifacts; immutable snapshots may outlive the public shader owner.
#[derive(Debug)]
pub struct Shader {
    /// Selected graphics stage.
    pub stage: GpuShaderStage,
    /// Selected WGSL/SPIR-V entry point.
    pub entry: String,
    /// Original canonical source used by WGPU.
    pub wgsl: String,
    /// GL ES 300 / WebGL 2 source, including normalized clip-space conversion.
    pub glsl: String,
    /// Vulkan artifact with the same binding and clip conventions.
    pub spirv: Vec<u32>,
    /// Active uniform blocks in this entry point only.
    pub uniforms: Vec<Uniform>,
    /// Active texture/sampler pairs in this entry point only.
    pub textures: Vec<SampledTexture>,
    /// Location-based inputs, including vertex attributes or fragment varyings.
    pub inputs: Vec<IoLocation>,
    /// Location-based outputs; fragment output is restricted to color location zero.
    pub outputs: Vec<IoLocation>,
}

impl Shader {
    /// Parses, validates the complete module, reflects one entry and emits both native artifacts.
    /// Driver allocation/linking remains a later fallible preparation operation.
    pub fn compile(stage: GpuShaderStage, source: &str, entry: &str) -> Result<Self, String> {
        if source.is_empty() || source.len() > MAX_SHADER_BYTES || entry.is_empty() || entry.len() > 128 {
            return Err("shader source or entry length exceeds the graphics profile".into());
        }
        let native_stage = native_stage(stage)?;
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
            && (outputs.len() != 1 || outputs[0].location != 0 || outputs[0].shape != (IoShape { kind: ScalarKind::Float, components: 4 }))
        {
            return Err("fragment profile requires exactly one vec4<f32> color output at location 0".into());
        }
        let options = naga::back::glsl::Options {
            version: naga::back::glsl::Version::Embedded { version: 300, is_webgl: true },
            ..Default::default()
        };
        let pipeline = naga::back::glsl::PipelineOptions {
            shader_stage: native_stage,
            entry_point: entry.into(),
            multiview: None,
        };
        let mut glsl = String::new();
        let reflection = naga::back::glsl::Writer::new(&mut glsl, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default())
            .map_err(|error| error.to_string())?
            .write()
            .map_err(|error| error.to_string())?;
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
                        gl_block: reflection.uniforms.get(&handle).cloned().ok_or("missing emitted uniform reflection")?,
                    });
                }
                AddressSpace::Handle if binding.group == texture_group => {}
                _ => return Err("entry point uses a resource from another stage or an unsupported binding group".into()),
            }
        }
        for (gl_name, mapping) in &reflection.texture_mapping {
            let image = module.global_variables[mapping.texture].binding.ok_or("sampled texture lacks a binding")?;
            let sampler_handle = mapping.sampler.ok_or("graphics profile requires a paired sampler")?;
            let sampler = module.global_variables[sampler_handle].binding.ok_or("sampler lacks a binding")?;
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
        let spirv = naga::back::spv::write_vec(
            &module,
            &info,
            &Default::default(),
            Some(&naga::back::spv::PipelineOptions {
                shader_stage: native_stage,
                entry_point: entry.into(),
            }),
        )
        .map_err(|error| error.to_string())?;
        Ok(Self {
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

    /// Conservative owned-artifact bytes used before logical publication.
    pub fn byte_len(&self) -> usize {
        self.wgsl.len() + self.glsl.len() + self.spirv.len() * 4 + self.entry.len()
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
            if outputs.len() != 1 || outputs[0].location != 0 || outputs[0].shape != (IoShape { kind: ScalarKind::Float, components: 4 }) {
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
                shape: IoShape { kind: scalar.kind, components },
                interpolation: *interpolation,
                sampling: *sampling,
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
