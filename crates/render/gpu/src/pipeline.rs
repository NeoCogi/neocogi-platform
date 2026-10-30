//! Complete copied pipeline validation; no semantic material or vertex type is imposed.
use crate::{
    abi::*,
    resource::{self, Domain},
    shader::IoShape,
};
use neocogi_com::{ComBox, ComRef, NcOutputBuffer, NcStatus, NC_E_INVALID_ARG, NC_E_OUT_OF_RANGE};
use std::rc::Rc;

/// Reads bounded descriptor arrays before taking any resource gate, then captures shader snapshots.
pub(crate) fn create(
    domain: &Rc<Domain>,
    source: ComRef<'_, IGpuPipelineSourceAbi>,
    vertex: ComRef<'_, IGpuShaderAbi>,
    fragment: ComRef<'_, IGpuShaderAbi>,
) -> Result<ComBox<IGpuPipelineAbi>, NcStatus> {
    let _operation = domain.device.operation()?;
    domain.open()?;
    let info = source.info()?;
    validate_state(info)?;
    let owner = domain.account.resource()?;
    let byte_len = std::mem::size_of::<GpuPipelineInfo>()
        + info.vertex_buffer_count as usize * std::mem::size_of::<GpuVertexBufferLayout>()
        + info.attribute_count as usize * std::mem::size_of::<GpuVertexAttribute>()
        + info.binding_count as usize * std::mem::size_of::<GpuShaderBinding>();
    let charge = domain.account.bytes(byte_len)?;
    let mut layouts = NcOutputBuffer::new();
    let mut attributes = NcOutputBuffer::new();
    let mut bindings = NcOutputBuffer::new();
    source.vertex_buffers(layouts.output(0, info.vertex_buffer_count as usize))?;
    source.attributes(attributes.output(0, info.attribute_count as usize))?;
    source.bindings(bindings.output(0, info.binding_count as usize))?;
    if layouts.len() != info.vertex_buffer_count as usize || attributes.len() != info.attribute_count as usize || bindings.len() != info.binding_count as usize
    {
        return Err(NC_E_INVALID_ARG);
    }
    domain.open()?;
    let vertex_observer = resource::observe(&domain.device.inner.shaders, vertex.as_raw().object as usize)?;
    let fragment_observer = resource::observe(&domain.device.inner.shaders, fragment.as_raw().object as usize)?;
    let vertex = vertex_observer.with(|shader| {
        resource::same_domain(domain, &shader.domain)?;
        Ok::<_, NcStatus>(shader.current.clone())
    })??;
    let fragment = fragment_observer.with(|shader| {
        resource::same_domain(domain, &shader.domain)?;
        Ok::<_, NcStatus>(shader.current.clone())
    })??;
    if vertex.artifact.stage != GpuShaderStage::Vertex || fragment.artifact.stage != GpuShaderStage::Fragment {
        return Err(NC_E_INVALID_ARG);
    }
    let layouts = layouts.into_vec();
    let attributes = attributes.into_vec();
    let bindings = bindings.into_vec();
    for (index, layout) in layouts.iter().enumerate() {
        if layout.slot >= 8
            || layout.stride == 0
            || layout.stride > 2048
            || layout.stride % 4 != 0
            || !matches!(layout.step, GpuStepMode::Vertex | GpuStepMode::Instance)
            || layouts[..index].iter().any(|previous| previous.slot == layout.slot)
        {
            return Err(NC_E_INVALID_ARG);
        }
    }
    if attributes.len() != vertex.artifact.inputs.len() {
        return Err(NC_E_INVALID_ARG);
    }
    for (index, attribute) in attributes.iter().enumerate() {
        let layout = layouts.iter().find(|layout| layout.slot == attribute.slot).ok_or(NC_E_INVALID_ARG)?;
        let (size, align, shape) = vertex_format(attribute.format)?;
        if attribute.location >= 16
            || attribute.offset % align != 0
            || attribute.offset.checked_add(size).filter(|end| *end <= layout.stride).is_none()
            || attributes[..index].iter().any(|previous| previous.location == attribute.location)
            || !vertex
                .artifact
                .inputs
                .iter()
                .any(|input| input.location == attribute.location && input.shape == shape)
        {
            return Err(NC_E_INVALID_ARG);
        }
    }
    if layouts.iter().any(|layout| !attributes.iter().any(|attribute| attribute.slot == layout.slot)) {
        return Err(NC_E_INVALID_ARG);
    }
    // Cross-module handles are not comparable; compare the reflected value
    // shape at each location instead, leaving no driver-only interface mismatch.
    for input in &fragment.artifact.inputs {
        if !vertex.artifact.outputs.iter().any(|output| output == input) {
            return Err(NC_E_INVALID_ARG);
        }
    }
    let mut expected = Vec::new();
    for shader in [&vertex.artifact, &fragment.artifact] {
        for uniform in &shader.uniforms {
            expected.push(GpuShaderBinding {
                stage: shader.stage,
                slot: uniform.slot,
                kind: GpuBindingKind::Uniform,
                byte_len: uniform.byte_len,
            });
        }
        for texture in &shader.textures {
            expected.push(GpuShaderBinding {
                stage: shader.stage,
                slot: texture.slot,
                kind: GpuBindingKind::SampledTexture,
                byte_len: 0,
            });
        }
    }
    if bindings.len() != expected.len() {
        return Err(NC_E_INVALID_ARG);
    }
    for (index, binding) in bindings.iter().enumerate() {
        if bindings[..index]
            .iter()
            .any(|previous| previous.stage == binding.stage && previous.kind == binding.kind && previous.slot == binding.slot)
            || !expected.iter().any(|expected| {
                expected.stage == binding.stage && expected.slot == binding.slot && expected.kind == binding.kind && expected.byte_len == binding.byte_len
            })
        {
            return Err(NC_E_INVALID_ARG);
        }
    }
    let version = resource::PipelineVersion {
        id: domain.device.next()?,
        info,
        vertex_buffers: layouts,
        attributes,
        bindings,
        vertex,
        fragment,
        _charge: charge,
    };
    resource::publish_pipeline(domain, version, owner)
}

/// Returns packed size/alignment and shader interpretation for an admitted attribute format.
pub(crate) fn vertex_format(format: GpuVertexFormat) -> Result<(u32, u32, IoShape), NcStatus> {
    let (components, size, align, kind) = match format {
        GpuVertexFormat::Float32 => (1, 4, 4, naga::ScalarKind::Float),
        GpuVertexFormat::Float32x2 => (2, 8, 4, naga::ScalarKind::Float),
        GpuVertexFormat::Float32x3 => (3, 12, 4, naga::ScalarKind::Float),
        GpuVertexFormat::Float32x4 => (4, 16, 4, naga::ScalarKind::Float),
        GpuVertexFormat::Uint32 => (1, 4, 4, naga::ScalarKind::Uint),
        GpuVertexFormat::Uint32x2 => (2, 8, 4, naga::ScalarKind::Uint),
        GpuVertexFormat::Uint32x3 => (3, 12, 4, naga::ScalarKind::Uint),
        GpuVertexFormat::Uint32x4 => (4, 16, 4, naga::ScalarKind::Uint),
        GpuVertexFormat::Unorm8x4 => (4, 4, 1, naga::ScalarKind::Float),
        _ => return Err(NC_E_INVALID_ARG),
    };
    Ok((size, align, IoShape { kind, components }))
}

/// Rejects every unknown open-enum/fixed-state value before dispatching native API mappings.
fn validate_state(info: GpuPipelineInfo) -> Result<(), NcStatus> {
    if info.vertex_buffer_count > 8 || info.attribute_count > 16 || info.binding_count > 16 {
        return Err(NC_E_OUT_OF_RANGE);
    }
    if info.color_write & !0xf != 0
        || !matches!(
            info.depth_compare,
            GpuCompare::Never
                | GpuCompare::Less
                | GpuCompare::Equal
                | GpuCompare::LessEqual
                | GpuCompare::Greater
                | GpuCompare::NotEqual
                | GpuCompare::GreaterEqual
                | GpuCompare::Always
        )
        || !matches!(info.cull, GpuCullMode::None | GpuCullMode::Front | GpuCullMode::Back)
        || !matches!(info.front_face, GpuFrontFace::Ccw | GpuFrontFace::Cw)
        || [info.blend.src_color, info.blend.dst_color, info.blend.src_alpha, info.blend.dst_alpha]
            .iter()
            .any(|factor| {
                !matches!(
                    *factor,
                    GpuBlendFactor::Zero
                        | GpuBlendFactor::One
                        | GpuBlendFactor::SrcColor
                        | GpuBlendFactor::OneMinusSrcColor
                        | GpuBlendFactor::DstColor
                        | GpuBlendFactor::OneMinusDstColor
                        | GpuBlendFactor::SrcAlpha
                        | GpuBlendFactor::OneMinusSrcAlpha
                        | GpuBlendFactor::DstAlpha
                        | GpuBlendFactor::OneMinusDstAlpha
                )
            })
        || [info.blend.color_op, info.blend.alpha_op].iter().any(|op| {
            !matches!(
                *op,
                GpuBlendOp::Add | GpuBlendOp::Subtract | GpuBlendOp::ReverseSubtract | GpuBlendOp::Min | GpuBlendOp::Max
            )
        })
    {
        return Err(NC_E_INVALID_ARG);
    }
    Ok(())
}
