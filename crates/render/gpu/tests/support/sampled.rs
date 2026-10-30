//! Shared native-test input: a procedural sampled triangle using the public graphics vocabulary.
use neocogi_com::{ComBox, ComPtr, NcCopyFromBufferOffset};
use neocogi_render_gpu::{
    GpuDevice,
    abi::*,
    source::{Bytes, PipelineDesc},
};

/// Record one full-root sampled contribution. The caller keeps its domain controller alive.
pub fn block(gpu: &GpuDevice, resources: &ComPtr<IRenderResourcesAbi>, root: Rect, size: Size, pixels: &[u8]) -> ComBox<IRenderViewBlockAbi> {
    let code = Bytes::source(
        br#"
        @group(2) @binding(0) var image:texture_2d<f32>;
        @group(2) @binding(1) var image_sampler:sampler;
        struct V { @builtin(position) p:vec4<f32>, @location(0) uv:vec2<f32> }
        @vertex fn vs(@builtin(vertex_index) i:u32) -> V {
            let p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
            return V(vec4(p[i],0.0,1.0),p[i]*vec2(0.5,-0.5)+vec2(0.5));
        }
        @fragment fn fs(v:V) -> @location(0) vec4<f32> { return textureSample(image,image_sampler,v.uv); }
    "#
        .to_vec(),
    );
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, code.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, code.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let desc = PipelineDesc {
        bindings: vec![GpuShaderBinding {
            stage: GpuShaderStage::Fragment,
            slot: 0,
            kind: GpuBindingKind::SampledTexture,
            byte_len: 0,
        }],
        ..Default::default()
    }
    .source()
    .unwrap();
    let pipeline = resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let texture = resources
        .create_texture(
            GpuTextureDesc {
                size,
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            Bytes::source(pixels.to_vec()).as_com_ref(),
        )
        .unwrap();
    let sampler = resources.create_sampler(GpuSamplerDesc::default()).unwrap();
    let mut frame = gpu.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.bind_texture(GpuShaderStage::Fragment, 0, texture.as_com_ref(), sampler.as_com_ref())
        .unwrap();
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    block
}
