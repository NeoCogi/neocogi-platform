//! Shared native-test input: a procedural sampled triangle using the public graphics vocabulary.
use neocogi_com::{ComBox, ComPtr, NcCopyFromBufferOffset};
use neocogi_render_gpu::{
    GpuDevice,
    abi::*,
    source::{Bytes, PipelineDesc},
};

/// Record one full-root sampled contribution. The caller keeps its domain controller alive.
pub fn block<B: neocogi_render_gpu::native::NativeResources>(
    gpu: &GpuDevice<B>,
    resources: &ComPtr<IRenderResourcesAbi>,
    root: Rect,
    size: Size,
    pixels: &[u8],
) -> ComBox<IRenderViewBlockAbi> {
    block_initial(gpu, resources, root, size, Some(pixels))
}

/// Records the same real public shader/image path with explicitly optional initial contents.
pub fn block_initial<B: neocogi_render_gpu::native::NativeResources>(
    gpu: &GpuDevice<B>,
    resources: &ComPtr<IRenderResourcesAbi>,
    root: Rect,
    size: Size,
    pixels: Option<&[u8]>,
) -> ComBox<IRenderViewBlockAbi> {
    block_fragment(gpu, resources, root, size, pixels, "textureSample(image,image_sampler,v.uv)")
}

/// Builds the same real resource path with an explicit fragment result.
fn block_fragment<B: neocogi_render_gpu::native::NativeResources>(
    gpu: &GpuDevice<B>,
    resources: &ComPtr<IRenderResourcesAbi>,
    root: Rect,
    size: Size,
    pixels: Option<&[u8]>,
    expression: &str,
) -> ComBox<IRenderViewBlockAbi> {
    let source = r#"
        @group(2) @binding(0) var image:texture_2d<f32>;
        @group(2) @binding(1) var image_sampler:sampler;
        struct V { @builtin(position) p:vec4<f32>, @location(0) uv:vec2<f32> }
        @vertex fn vs(@builtin(vertex_index) i:u32) -> V {
            let p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
            return V(vec4(p[i],0.0,1.0),p[i]*vec2(0.5,-0.5)+vec2(0.5));
        }
        @fragment fn fs(v:V) -> @location(0) vec4<f32> { return REPLACEMENT; }
    "#
    .replace("REPLACEMENT", expression);
    let code = Bytes::source(source.into_bytes());
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
            neocogi_render_core::source::initial(pixels.map(|pixels| Bytes::source(pixels.to_vec())))
                .borrow()
                .unwrap(),
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

/// Exact mip-zero read with one in-range result and adversarial signed/unsigned coordinates.
pub struct LoadCase {
    /// Coordinate scenario for diagnostics.
    pub name: &'static str,
    /// WGSL integer-vector coordinate expression.
    coordinate: &'static str,
    /// Whether portable WGPU may choose a clamped result instead of zero.
    pub outside: bool,
}
/// Tests both coordinate edges and very large signed/unsigned values through actual shaders.
pub fn load_cases() -> Vec<LoadCase> {
    [
        ("signed origin", "vec2<i32>(0)", false),
        ("unsigned origin", "vec2<u32>(0u)", false),
        ("negative x", "vec2<i32>(-1,0)", true),
        ("negative y", "vec2<i32>(0,-1)", true),
        ("upper x", "vec2<i32>(1,0)", true),
        ("upper y", "vec2<i32>(0,1)", true),
        ("maximum signed", "vec2<i32>(2147483647,0)", true),
        ("maximum unsigned", "vec2<u32>(4294967295u,0u)", true),
    ]
    .into_iter()
    .map(|(name, coordinate, outside)| LoadCase { name, coordinate, outside })
    .collect()
}
/// Keeps an actual sampled pair in the same entry; its tiny contribution cannot change RGBA8.
pub fn load_block<B: neocogi_render_gpu::native::NativeResources>(
    gpu: &GpuDevice<B>,
    resources: &ComPtr<IRenderResourcesAbi>,
    case: &LoadCase,
) -> ComBox<IRenderViewBlockAbi> {
    block_fragment(
        gpu,
        resources,
        Rect { x: 0, y: 0, width: 8, height: 8 },
        Size { width: 1, height: 1 },
        Some(&[191, 63, 127, 255]),
        &format!("textureLoad(image,{},0)+textureSample(image,image_sampler,v.uv)*0.000001", case.coordinate),
    )
}

/// Actual None-created vertex/index storage and its later initialized versions share one captured block.
/// Left reads zero vertex bytes, middle uses three zero indices (a degenerate primitive), right
/// consumes the updated red vertices and [0,1,2] indices. Earlier captures must remain unchanged.
pub fn zero_storage_block<B: neocogi_render_gpu::native::NativeResources>(
    gpu: &GpuDevice<B>,
    resources: &ComPtr<IRenderResourcesAbi>,
) -> ComBox<IRenderViewBlockAbi> {
    let source = Bytes::source(
        br#"
        struct V { @builtin(position) p:vec4<f32>, @location(0) color:vec4<f32> }
        @vertex fn vs(@builtin(vertex_index) i:u32,@location(0) color:vec4<f32>)->V {
            let p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
            return V(vec4(p[i],.5,1.),color);
        }
        @fragment fn fs(@location(0) color:vec4<f32>)->@location(0) vec4<f32>{return color;}
    "#
        .to_vec(),
    );
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let desc = PipelineDesc {
        vertex_buffers: vec![GpuVertexBufferLayout {
            slot: 0,
            stride: 16,
            step: GpuStepMode::Vertex,
        }],
        attributes: vec![GpuVertexAttribute {
            location: 0,
            slot: 0,
            offset: 0,
            format: GpuVertexFormat::Float32x4,
        }],
        ..Default::default()
    }
    .source()
    .unwrap();
    let pipeline = resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let none = neocogi_render_core::source::initial(None);
    let vertices = resources
        .create_buffer(
            GpuBufferDesc {
                role: GpuBufferRole::Vertex,
                byte_len: 48,
            },
            none.borrow().unwrap(),
        )
        .unwrap();
    let indices = resources
        .create_buffer(GpuBufferDesc { role: GpuBufferRole::Index, byte_len: 6 }, none.borrow().unwrap())
        .unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut frame = gpu.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.bind_vertex_buffer(0, vertices.as_com_ref(), 0).unwrap();
    view.bind_index_buffer(indices.as_com_ref(), GpuIndexFormat::U16, 0).unwrap();
    view.push_view(Rect { x: 0, y: 0, width: 2, height: 8 }).unwrap();
    view.draw(3, 1, 0).unwrap();
    view.pop().unwrap();
    view.push_view(Rect { x: 2, y: 0, width: 2, height: 8 }).unwrap();
    view.draw_indexed(3, 1, 0).unwrap();
    view.pop().unwrap();
    let red: Vec<_> = [1f32, 0., 0., 1.].into_iter().cycle().take(12).flat_map(f32::to_le_bytes).collect();
    resources.update_buffer(vertices.as_com_ref(), 0, Bytes::source(red).as_com_ref()).unwrap();
    resources
        .update_buffer(indices.as_com_ref(), 0, Bytes::source(vec![0, 0, 1, 0, 2, 0]).as_com_ref())
        .unwrap();
    view.push_view(Rect { x: 4, y: 0, width: 4, height: 8 }).unwrap();
    view.draw_indexed(3, 1, 0).unwrap();
    view.pop().unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    block
}

/// Exact native RGBA rows prove zero initialization and A/B capture, including the degenerate zero index draw.
pub fn assert_zero_storage_pixels(pixels: &[u8]) {
    assert_eq!(pixels.len(), 8 * 8 * 4);
    for row in pixels.as_chunks::<32>().0 {
        for (x, pixel) in row.as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(
                *pixel,
                if x < 2 {
                    [0; 4]
                } else if x < 4 {
                    [0, 255, 0, 255]
                } else {
                    [255, 0, 0, 255]
                },
                "column {x}"
            );
        }
    }
}
