//! Headless native-adapter acceptance exercises real COM recording and WGPU validation/pixels.
use super::*;
use neocogi_render_gpu::abi::{Rect, Size};
use neocogi_com::{ComBox, ComPtr, NcCopyFromBufferOffset};
use neocogi_render_gpu::{
    GpuDevice,
    abi::*,
    source::{Bytes, PipelineDesc},
};

/// Obtains a real native adapter/device without requiring presentation or an SDL display.
fn device() -> (wgpu::Device, wgpu::Queue) {
    device_with_limits(wgpu::Limits::default())
}

/// A constrained buffer limit deterministically exercises the same invalid-buffer path as OOM.
fn device_with_limits(limits: wgpu::Limits) -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: limits,
        ..Default::default()
    }))
    .unwrap()
}

/// A client shader owns its color and depth semantics; the renderer only sees reflected bindings.
const SHADER: &str = r#"
struct Values { color:vec4<f32>, params:vec4<f32> }
@group(1) @binding(0) var<uniform> vertex_values:Values;
@group(3) @binding(0) var<uniform> fragment_values:Values;
@vertex fn vs(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>,3>(vec2(-1.,-1.), vec2(3.,-1.), vec2(-1.,3.));
    return vec4(p[i],vertex_values.params.x,1.);
}
@fragment fn fs() -> @location(0) vec4<f32> { return fragment_values.color; }
"#;

/// Publishes the explicit per-stage uniform layout through generated descriptor/source calls.
fn pipeline(resources: &ComPtr<IRenderResourcesAbi>, mask: u8) -> ComBox<IGpuPipelineAbi> {
    let source = Bytes::source(SHADER.as_bytes().to_vec());
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let description = PipelineDesc {
        state: GpuPipelineInfo {
            depth_write: true,
            depth_compare: GpuCompare::LessEqual,
            color_write: mask,
            ..Default::default()
        },
        bindings: [GpuShaderStage::Vertex, GpuShaderStage::Fragment]
            .map(|stage| GpuShaderBinding {
                stage,
                slot: 0,
                kind: GpuBindingKind::Uniform,
                byte_len: 32,
            })
            .to_vec(),
        ..Default::default()
    }
    .source()
    .unwrap();
    resources.create_pipeline(description.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap()
}

/// Explicit little-endian bytes, independent of semantic Rust structure layout.
fn values(view: &mut ComBox<IRenderViewAbi>, color: [f32; 4], depth: f32) {
    let bytes: Vec<_> = color.into_iter().chain([depth, 0., 0., 0.]).flat_map(f32::to_le_bytes).collect();
    for stage in [GpuShaderStage::Vertex, GpuShaderStage::Fragment] {
        view.push_uniforms(stage, 0, NcCopyFromBufferOffset::new(0, &bytes)).unwrap();
    }
}

/// Full-screen shader geometry stays confined by the host root, regardless of clip reset.
#[test]
#[ignore = "requires a native WGPU adapter, including a software adapter"]
fn regional_depth_clears_ordered_views_and_completion_keep_their_contracts() {
    let (native, queue) = device();
    let outer = native.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut executor = crate::generic::Executor::new(&native, &queue);
    let gpu = GpuDevice::new();
    let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    let pipeline = pipeline(&resources, 15);
    let hidden = self::pipeline(&resources, 0);
    let mut recording = gpu.broker().begin_frame().unwrap();
    let root = Rect { x: 4, y: 4, width: 56, height: 56 };
    let mut session = recording
        .begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    values(&mut view, [1., 0., 0., 1.], 0.25);
    view.draw(3, 1, 0).unwrap();
    view.push_clip(Rect { x: 28, y: 0, width: 28, height: 56 }).unwrap();
    view.clear_depth(1.).unwrap();
    values(&mut view, [0., 0., 1., 1.], 0.75);
    view.draw(3, 1, 0).unwrap();
    view.pop().unwrap();
    view.reset().unwrap();
    values(&mut view, [1., 1., 0., 1.], 0.9);
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let first = session.take_block().unwrap();
    let second_root = Rect { x: 20, y: 20, width: 24, height: 24 };
    let mut second = recording
        .begin_view(RenderViewRoot { client: second_root, target: second_root }, resources.as_com_ref())
        .unwrap();
    let mut second_view = second.view().unwrap();
    second_view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    values(&mut second_view, [0., 1., 0., 1.], 0.75);
    second_view.draw(3, 1, 0).unwrap();
    second_view.finish().unwrap();
    let second_owner = second.take_block().unwrap();
    let third_root = Rect { x: 1, y: 1, width: 2, height: 2 };
    let mut third = recording
        .begin_view(RenderViewRoot { client: third_root, target: third_root }, resources.as_com_ref())
        .unwrap();
    let mut third_view = third.view().unwrap();
    third_view.bind_pipeline(hidden.as_com_ref()).unwrap();
    values(&mut third_view, [1., 0., 1., 1.], 0.5);
    third_view.draw(3, 1, 0).unwrap();
    third_view.clear_color(0xffffffff).unwrap();
    third_view.finish().unwrap();
    let third_owner = third.take_block().unwrap();
    recording.finish().unwrap();
    let mut blocks = Vec::new();
    for owner in [&first, &second_owner, &third_owner] {
        let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
        blocks.push(executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, true, &HashMap::new()).unwrap());
    }
    drop((
        first,
        second_owner,
        third_owner,
        view,
        second_view,
        third_view,
        session,
        second,
        third,
        recording,
        pipeline,
        hidden,
    ));
    let charged = gpu.recording_usage(resources.as_com_ref()).unwrap().0;
    assert!(charged > 0);
    let target = native.create_texture(&wgpu::TextureDescriptor {
        label: Some("generic acceptance color"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = WgpuRenderer::create_depth_target(&native, 64, 64);
    let mut encoder = native.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("generic acceptance pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.2, g: 0.2, b: 0.2, a: 1. }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth.view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for block in &blocks {
            executor
                .record(&mut pass, block, Size { width: 64, height: 64 }, Size { width: 64, height: 64 })
                .unwrap();
        }
    }
    queue.submit(Some(encoder.finish()));
    executor.submitted(blocks);
    assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap(), (charged, 0));
    let pixels = texture_pixels(&native, &queue, &target, Dimensioni::new(64, 64));
    let pixel = |x: usize, y: usize| &pixels[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
    assert_eq!(pixel(8, 8), [255, 0, 0, 255]);
    assert_eq!(pixel(52, 8), [0, 0, 255, 255]);
    assert_eq!(pixel(24, 24), [0, 255, 0, 255]);
    assert_eq!(pixel(1, 1), [255; 4]);
    assert_eq!(pixel(0, 0), [51, 51, 51, 255]);
    // Readback synchronizes GPU work, but accounting waits for explicit completion observation.
    assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap().0, charged);
    native.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    executor.collect().unwrap();
    assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap(), (0, 0));
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
    assert!(pollster::block_on(outer.pop()).is_none());
}

/// Explicit vertex/index bytes, sampled-image updates and copied uniforms survive owner expiry.
#[test]
#[ignore = "requires a native WGPU adapter, including a software adapter"]
fn indexed_textures_and_uniform_versions_execute_without_public_owners() {
    for operation in [GpuBlendOp::Min, GpuBlendOp::Max] {
        for imported in [false, true] {
            indexed_texture_case(operation, imported);
        }
    }
}

/// Both unscaled min/max blend operations ignore factors, including factors WebGPU forbids verbatim.
fn indexed_texture_case(operation: GpuBlendOp, imported: bool) {
    let (native, queue) = device();
    let outer = native.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut executor = crate::generic::Executor::new(&native, &queue);
    let gpu = GpuDevice::new();
    let controller = if imported {
        gpu.create_ui_client(RenderClientLimits::default(), None)
    } else {
        gpu.create_client(RenderClientLimits::default())
    }
    .unwrap();
    let mut images = HashMap::new();
    let resources = controller.resources().unwrap();
    let source = Bytes::source(
        br#"
        struct Output { @builtin(position) pos:vec4<f32>, @location(0) uv:vec2<f32>, @location(1) color:vec4<f32> }
        @vertex fn vs(@location(0) p:vec2<f32>, @location(1) color:vec4<f32>) -> Output {
            var out:Output; out.pos=vec4(p,0.5,1.); out.uv=vec2(p.x*0.5+0.5,0.5-p.y*0.5); out.color=color; return out;
        }
        struct Values { tint:vec4<f32> }
        @group(2) @binding(0) var image:texture_2d<f32>;
        @group(2) @binding(1) var image_sampler:sampler;
        @group(3) @binding(0) var<uniform> values:Values;
        @fragment fn fs(input:Output) -> @location(0) vec4<f32> {
            return textureSample(image,image_sampler,input.uv)*input.color*values.tint;
        }
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
        state: GpuPipelineInfo {
            blend: GpuBlendState {
                enabled: true,
                color_op: operation,
                alpha_op: operation,
                src_color: GpuBlendFactor::Zero,
                dst_color: GpuBlendFactor::Zero,
                src_alpha: GpuBlendFactor::Zero,
                dst_alpha: GpuBlendFactor::Zero,
            },
            ..Default::default()
        },
        vertex_buffers: vec![GpuVertexBufferLayout {
            slot: 3,
            stride: 12,
            step: GpuStepMode::Vertex,
        }],
        attributes: vec![
            GpuVertexAttribute {
                location: 0,
                slot: 3,
                offset: 0,
                format: GpuVertexFormat::Float32x2,
            },
            GpuVertexAttribute {
                location: 1,
                slot: 3,
                offset: 8,
                format: GpuVertexFormat::Unorm8x4,
            },
        ],
        bindings: vec![
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 0,
                kind: GpuBindingKind::SampledTexture,
                byte_len: 0,
            },
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 0,
                kind: GpuBindingKind::Uniform,
                byte_len: 16,
            },
        ],
    }
    .source()
    .unwrap();
    let pipeline = resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let mut vertex_bytes = Vec::new();
    for [x, y] in [[-1.0f32, 1.], [1., 1.], [1., -1.], [-1., -1.]] {
        vertex_bytes.extend_from_slice(&x.to_le_bytes());
        vertex_bytes.extend_from_slice(&y.to_le_bytes());
        vertex_bytes.extend_from_slice(&[255; 4]);
    }
    let source = Bytes::source(vertex_bytes);
    let vertices = resources
        .create_buffer(
            GpuBufferDesc {
                role: GpuBufferRole::Vertex,
                byte_len: 48,
            },
            source.as_com_ref(),
        )
        .unwrap();
    let source = Bytes::source([0u16, 1, 2, 0, 2, 3].into_iter().flat_map(u16::to_le_bytes).collect());
    let indices = resources
        .create_buffer(GpuBufferDesc { role: GpuBufferRole::Index, byte_len: 12 }, source.as_com_ref())
        .unwrap();
    let source = Bytes::source(vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255]);
    let image = resources
        .create_texture(
            GpuTextureDesc {
                size: Size { width: 2, height: 2 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            source.as_com_ref(),
        )
        .unwrap();
    let image = if imported {
        import_native_image(&gpu, &resources, &native, &queue, &mut images, image)
    } else {
        image
    };
    let sampler = resources.create_sampler(GpuSamplerDesc::default()).unwrap();
    let mut recording = gpu.broker().begin_frame().unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut session = recording
        .begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.bind_vertex_buffer(3, vertices.as_com_ref(), 0).unwrap();
    view.bind_index_buffer(indices.as_com_ref(), GpuIndexFormat::U16, 0).unwrap();
    view.bind_texture(GpuShaderStage::Fragment, 0, image.as_com_ref(), sampler.as_com_ref())
        .unwrap();
    let white: Vec<_> = [1.0f32; 4].into_iter().flat_map(f32::to_le_bytes).collect();
    view.push_uniforms(GpuShaderStage::Fragment, 0, NcCopyFromBufferOffset::new(0, &white)).unwrap();
    view.push_view(Rect { x: 0, y: 0, width: 4, height: 8 }).unwrap();
    view.draw_indexed(6, 1, 0).unwrap();
    view.pop().unwrap();
    let image_b = if imported {
        let pixel = resources
            .create_texture(
                GpuTextureDesc {
                    size: Size { width: 2, height: 2 },
                    format: GpuTextureFormat::Rgba8Unorm,
                    mip_count: 1,
                },
                Bytes::source(vec![255; 16]).as_com_ref(),
            )
            .unwrap();
        let image_b = import_native_image(&gpu, &resources, &native, &queue, &mut images, pixel);
        view.bind_texture(GpuShaderStage::Fragment, 0, image_b.as_com_ref(), sampler.as_com_ref())
            .unwrap();
        Some(image_b)
    } else {
        resources
            .update_texture(
                image.as_com_ref(),
                Rect { x: 0, y: 0, width: 2, height: 2 },
                Bytes::source(vec![255; 16]).as_com_ref(),
            )
            .unwrap();
        None
    };
    let magenta: Vec<_> = [1.0f32, 0., 1., 1.].into_iter().flat_map(f32::to_le_bytes).collect();
    view.push_uniforms(GpuShaderStage::Fragment, 0, NcCopyFromBufferOffset::new(0, &magenta))
        .unwrap();
    view.push_view(Rect { x: 4, y: 0, width: 4, height: 8 }).unwrap();
    view.draw_indexed(6, 1, 0).unwrap();
    view.pop().unwrap();
    drop(image_b);
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    recording.finish().unwrap();
    let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
    let prepared = executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, true, &images).unwrap();
    drop(images);
    drop((snapshot, owner, view, session, recording, vertices, indices, pipeline, image, sampler, vs, fs));
    let target = native.create_texture(&wgpu::TextureDescriptor {
        label: Some("indexed acceptance color"),
        size: wgpu::Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = WgpuRenderer::create_depth_target(&native, 8, 8);
    let mut encoder = native.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(if operation == GpuBlendOp::Min {
                        wgpu::Color::WHITE
                    } else {
                        wgpu::Color::BLACK
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth.view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        executor
            .record(&mut pass, &prepared, Size { width: 8, height: 8 }, Size { width: 8, height: 8 })
            .unwrap();
    }
    queue.submit(Some(encoder.finish()));
    executor.submitted(vec![prepared]);
    let pixels = texture_pixels(&native, &queue, &target, Dimensioni::new(8, 8));
    let pixel = |x: usize, y: usize| &pixels[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4];
    assert_eq!(pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(3, 0), [0, 255, 0, 255]);
    assert_eq!(pixel(0, 7), [0, 0, 255, 255]);
    assert_eq!(pixel(3, 7), [255, 255, 0, 255]);
    assert_eq!(pixel(6, 3), [255, 0, 255, 255]);
    native.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    executor.collect().unwrap();
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
    assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap(), (0, 0));
    assert!(pollster::block_on(outer.pop()).is_none());
}

/// A clear-only block still initializes native uniform storage during preparation.
fn clear_block(gpu: &GpuDevice, resources: &ComPtr<IRenderResourcesAbi>) -> ComBox<IRenderViewBlockAbi> {
    let mut frame = gpu.broker().begin_frame().unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut session = frame.begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.clear_color(0xff123456).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    block
}

/// No acquired/rendered frame is needed to preserve and retire preparatory upload accounting.
#[test]
#[ignore = "requires a native WGPU adapter, including a software adapter"]
fn abandoned_preparation_retains_upload_capacity_until_completion() {
    let (native, queue) = device();
    let mut executor = crate::generic::Executor::new(&native, &queue);
    let gpu = GpuDevice::new();
    let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    let owner = clear_block(&gpu, &resources);
    let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
    let prepared = executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, true, &HashMap::new()).unwrap();
    let charged = gpu.recording_usage(resources.as_com_ref()).unwrap().0;
    assert!(charged > 0);
    drop((prepared, snapshot, owner));
    assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap(), (charged, 0));
    // The upload was submitted during preparation on the existing queue, independently of any
    // target acquisition. No later successful frame is required to make its completion progress.
    native.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    executor.collect().unwrap();
    assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap(), (0, 0));
    drop(queue);
}

/// Native creation errors return through normal scopes, preserve retry, and never panic in mapping.
#[test]
#[ignore = "requires a native WGPU adapter, including a software adapter"]
fn native_buffer_creation_failure_returns_error_without_panicking_or_leaking_scopes() {
    let (native, queue) = device_with_limits(wgpu::Limits {
        max_buffer_size: 16,
        ..Default::default()
    });
    let outer = native.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut executor = crate::generic::Executor::new(&native, &queue);
    let gpu = GpuDevice::new();
    let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    let owner = clear_block(&gpu, &resources);
    let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
    for _ in 0..2 {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, true, &HashMap::new())
        }));
        let error = match result {
            Ok(Err(error)) => error,
            Ok(Ok(_)) => panic!("native buffer creation must fail"),
            Err(_) => panic!("native creation failure must return an error"),
        };
        assert!(error.contains("maximum buffer size"), "unexpected native error: {error}");
    }
    assert!(pollster::block_on(outer.pop()).is_none());
}

/// A failed native preparation must retain earlier valid uploads after clearing unsuccessful caches.
#[test]
#[ignore = "requires a native WGPU adapter, including a software adapter"]
fn partial_preparation_failure_keeps_resource_versions_charged_through_upload_completion() {
    let (native, queue) = device_with_limits(wgpu::Limits {
        max_buffer_size: 32,
        ..Default::default()
    });
    let outer = native.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut executor = crate::generic::Executor::new(&native, &queue);
    let gpu = GpuDevice::new();
    let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    let source = Bytes::source(
        br#"
        @vertex fn vs(@location(0) p:vec3<f32>) -> @builtin(position) vec4<f32> { return vec4(p,1.); }
        @group(2) @binding(0) var image:texture_2d<f32>;
        @group(2) @binding(1) var image_sampler:sampler;
        struct Values { tint:vec4<f32> }
        @group(3) @binding(0) var<uniform> values:Values;
        @fragment fn fs() -> @location(0) vec4<f32> { return textureSample(image,image_sampler,vec2(0.5))*values.tint; }
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
        state: Default::default(),
        vertex_buffers: vec![GpuVertexBufferLayout {
            slot: 0,
            stride: 12,
            step: GpuStepMode::Vertex,
        }],
        attributes: vec![GpuVertexAttribute {
            location: 0,
            slot: 0,
            offset: 0,
            format: GpuVertexFormat::Float32x3,
        }],
        bindings: vec![
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 0,
                kind: GpuBindingKind::SampledTexture,
                byte_len: 0,
            },
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 0,
                kind: GpuBindingKind::Uniform,
                byte_len: 16,
            },
        ],
    }
    .source()
    .unwrap();
    let pipeline = resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let vertices = resources
        .create_buffer(
            GpuBufferDesc {
                role: GpuBufferRole::Vertex,
                byte_len: 36,
            },
            Bytes::source(vec![0; 36]).as_com_ref(),
        )
        .unwrap();
    let image = resources
        .create_texture(
            GpuTextureDesc {
                size: Size { width: 2, height: 2 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            Bytes::source(vec![255; 16]).as_com_ref(),
        )
        .unwrap();
    let sampler = resources.create_sampler(GpuSamplerDesc::default()).unwrap();
    let mut recording = gpu.broker().begin_frame().unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut session = recording
        .begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.bind_vertex_buffer(0, vertices.as_com_ref(), 0).unwrap();
    view.bind_texture(GpuShaderStage::Fragment, 0, image.as_com_ref(), sampler.as_com_ref())
        .unwrap();
    view.push_uniforms(GpuShaderStage::Fragment, 0, NcCopyFromBufferOffset::new(0, &[0; 16]))
        .unwrap();
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    recording.finish().unwrap();
    let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
    let error = match executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, true, &HashMap::new()) {
        Ok(_) => panic!("native vertex allocation must fail"),
        Err(error) => error,
    };
    assert!(error.contains("maximum buffer size"), "unexpected error: {error}");
    drop((snapshot, owner, view, session, recording, pipeline, vertices, image, sampler, vs, fs));
    // The CPU versions include 36 vertex bytes and 16 image bytes. Both still have private
    // upload owners even though the frame failed and its native caches have been invalidated.
    assert!(gpu.live_bytes(resources.as_com_ref()).unwrap() >= 52);
    assert!(gpu.recording_usage(resources.as_com_ref()).unwrap().0 > 0);
    native.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    executor.collect().unwrap();
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
    assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap(), (0, 0));
    assert!(pollster::block_on(outer.pop()).is_none());
}

/// Trusted fixture capability represents the host-authenticated immutable image.
#[neocogi_com::com_object(interfaces(ITexture))]
struct ImportedImage(Size);
impl ITexture for ImportedImage {
    /// Initialization and dimensions belong to the native map, without a device getter.
    fn size(&self) -> Result<Size, neocogi_com::NcStatus> {
        Ok(self.0)
    }
}

/// Real native handles survive removal of their ordinary-image facade before submission.
fn import_native_image(
    gpu: &GpuDevice,
    resources: &ComPtr<IRenderResourcesAbi>,
    native: &wgpu::Device,
    queue: &wgpu::Queue,
    images: &mut HashMap<neocogi_render_core::TextureId, crate::GpuTexture>,
    pixel: ComBox<IGpuTextureAbi>,
) -> ComBox<IGpuTextureAbi> {
    let snapshot = gpu.texture_snapshot(resources.as_com_ref(), pixel.as_com_ref()).unwrap();
    let size = wgpu::Extent3d {
        width: 2,
        height: 2,
        depth_or_array_layers: 1,
    };
    let texture = native.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        snapshot.pixels().unwrap(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(2),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let id = neocogi_render_core::TextureId::for_upload(2, 2, &[0; 16]).unwrap();
    // The unused legacy UI bind group disappears with the UI executor cutover; import uses
    // only the actual image and view. No mock GPU resource reaches the tested sampling path.
    let layout = native.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[] });
    let bind_group = native.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[],
    });
    images.insert(
        id,
        crate::GpuTexture {
            _texture: texture,
            view,
            render_target: false,
            bind_group,
        },
    );
    let lease: ComPtr<ITextureAbi> = ImportedImage::from_value(ImportedImage(Size { width: 2, height: 2 })).query().unwrap();
    gpu.import_ui_texture(resources.as_com_ref(), id, lease).unwrap()
}

/// An import-only preflight must flush source initialization and retire it without a target frame.
#[test]
#[ignore = "requires a native WGPU adapter with native allocation counters"]
fn imported_preparation_flushes_initialization_before_refunding_native_storage() {
    let (native, queue) = device();
    let (_window, _texture, layout, sampler) = WgpuRenderer::create_pipeline(&native, wgpu::TextureFormat::Rgba8Unorm);
    let uniforms = native.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: false,
    });
    let mut executor = crate::generic::Executor::new(&native, &queue);
    let gpu = GpuDevice::new();
    let controller = gpu.create_ui_client(RenderClientLimits { live_bytes: 8, ..Default::default() }, None).unwrap();
    let resources = controller.resources().unwrap();
    let mut images = HashMap::new();
    let baseline = native.get_internal_counters().hal.texture_memory.read();
    for value in 0..8 {
        let pixels = [value, 0, 0, 255];
        let image = WgpuRenderer::create_gpu_texture(&native, &queue, &layout, &sampler, &uniforms, 1, 1, Some(&pixels)).unwrap();
        assert!(
            native.get_internal_counters().hal.texture_memory.read() > baseline,
            "this native regression requires observable image allocation counters"
        );
        let id = TextureId::for_upload(1, 1, &pixels).unwrap();
        images.insert(id, image);
        let lease: ComPtr<ITextureAbi> = ImportedImage::from_value(ImportedImage(Size { width: 1, height: 1 })).query().unwrap();
        let imported = gpu.import_ui_texture(resources.as_com_ref(), id, lease).unwrap();
        executor
            .prepare_texture(gpu.ui_texture_snapshot(resources.as_com_ref(), imported.as_com_ref()).unwrap(), &images)
            .unwrap();
        drop(imported);
        images.remove(&id);
        assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 4);
        // Wait/poll only observe submissions. This deliberately never submits a compensating
        // empty batch or renders a target: preparation must already have made progress possible.
        native.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        executor.collect().unwrap();
        native.poll(wgpu::PollType::Poll).unwrap();
        assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
        assert_eq!(
            native.get_internal_counters().hal.texture_memory.read(),
            baseline,
            "pending ordinary initialization outlived its imported receipt"
        );
    }
}
