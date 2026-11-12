//! Headless native-adapter acceptance exercises real COM recording and WGPU validation/pixels.
use super::*;
use crate::gpu::abi::{Rect, Size};
use neocogi_com::{ComBox, ComPtr, NcCopyFromBufferOffset};
use crate::gpu::{
    abi::*,
    source::{Bytes, PipelineDesc},
};

type GpuDevice = crate::gpu::GpuDevice<Backend>;

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
const SHADER: &str = r#"(module
  (struct Values (color vec4) (params vec4))
  (uniform vertex_values Values vertex 0)
  (uniform fragment_values Values fragment 0)
  (vertex vs ((i u32 (builtin vertex_index))) (returns vec4 (builtin position))
    (let p ((array vec2 3) (vec2 (- 1.0) (- 1.0)) (vec2 3.0 (- 1.0)) (vec2 (- 1.0) 3.0)))
    (return (vec4 (index p i) (. (. vertex_values params) x) 1.0)))
  (fragment fs () (returns vec4 (location 0)) (return (. fragment_values color))))"#;

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
                kind: GpuBindingKind::Uniform(32),
            })
            .to_vec(),
        ..Default::default()
    }
    .source()
    .unwrap();
    resources.create_pipeline(description.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap()
}

/// Explicit little-endian bytes, independent of semantic Rust structure layout.
fn values(resources: &ComPtr<IRenderResourcesAbi>, view: &mut ComBox<IRenderViewAbi>, color: [f32; 4], depth: f32) -> Vec<ComBox<IGpuBufferAbi>> {
    let bytes: Vec<_> = color.into_iter().chain([depth, 0., 0., 0.]).flat_map(f32::to_le_bytes).collect();
    [GpuShaderStage::Vertex, GpuShaderStage::Fragment]
        .into_iter()
        .map(|stage| crate::gpu::testing::bind_uniform(resources.as_com_ref(), view, stage, 0, &bytes).unwrap())
        .collect()
}

/// Full-screen shader geometry stays confined by the host root, regardless of clip reset.
#[test]
#[ignore = "requires a native WGPU adapter, including a software adapter"]
fn regional_depth_clears_ordered_views_and_completion_keep_their_contracts() {
    let (native, queue) = device();
    let outer = native.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut executor = crate::gpu::backends::wgpu::generic::Executor::new(Core::new(native.clone(), queue.clone()));
    let gpu = GpuDevice::new(executor.core.clone()).unwrap();
    let controller = gpu
        .create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions::default(),
        )
        .unwrap();
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
    let _uniforms = values(&resources, &mut view, [1., 0., 0., 1.], 0.25);
    view.draw(3, 1, 0).unwrap();
    drop(_uniforms);
    view.push_clip(Rect { x: 28, y: 0, width: 28, height: 56 }).unwrap();
    view.clear_depth(1.).unwrap();
    let _uniforms = values(&resources, &mut view, [0., 0., 1., 1.], 0.75);
    view.draw(3, 1, 0).unwrap();
    drop(_uniforms);
    view.pop().unwrap();
    view.reset().unwrap();
    let _uniforms = values(&resources, &mut view, [1., 1., 0., 1.], 0.9);
    view.draw(3, 1, 0).unwrap();
    drop(_uniforms);
    view.finish().unwrap();
    let first = session.take_block().unwrap();
    let second_root = Rect { x: 20, y: 20, width: 24, height: 24 };
    let mut second = recording
        .begin_view(RenderViewRoot { client: second_root, target: second_root }, resources.as_com_ref())
        .unwrap();
    let mut second_view = second.view().unwrap();
    second_view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    let _uniforms = values(&resources, &mut second_view, [0., 1., 0., 1.], 0.75);
    second_view.draw(3, 1, 0).unwrap();
    drop(_uniforms);
    second_view.finish().unwrap();
    let second_owner = second.take_block().unwrap();
    let third_root = Rect { x: 1, y: 1, width: 2, height: 2 };
    let mut third = recording
        .begin_view(RenderViewRoot { client: third_root, target: third_root }, resources.as_com_ref())
        .unwrap();
    let mut third_view = third.view().unwrap();
    third_view.bind_pipeline(hidden.as_com_ref()).unwrap();
    let _uniforms = values(&resources, &mut third_view, [1., 0., 1., 1.], 0.5);
    third_view.draw(3, 1, 0).unwrap();
    drop(_uniforms);
    third_view.clear_color(0xffffffff).unwrap();
    third_view.finish().unwrap();
    let third_owner = third.take_block().unwrap();
    recording.finish().unwrap();
    let mut blocks = Vec::new();
    for owner in [&first, &second_owner, &third_owner] {
        let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
        blocks.push(executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, true).unwrap());
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
    let depth = WgpuRenderer::create_depth_target(&executor.core, 64, 64).unwrap();
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
    executor.reserve_submission().unwrap();
    executor.submitted(blocks, Vec::new());
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

/// Explicit vertex/index bytes, sampled-image maps and uniform-buffer captures survive owner expiry.
#[test]
#[ignore = "requires a native WGPU adapter, including a software adapter"]
fn indexed_textures_and_uniform_versions_execute_without_public_owners() {
    for operation in [GpuBlendEquation::Min, GpuBlendEquation::Max] {
        for imported in [false, true] {
            indexed_texture_case(operation, imported);
        }
    }
}

/// Factor-free min/max become valid WebGPU state while retaining exact resource snapshots.
fn indexed_texture_case(operation: GpuBlendEquation, imported: bool) {
    let (native, queue) = device();
    let outer = native.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut executor = crate::gpu::backends::wgpu::generic::Executor::new(Core::new(native.clone(), queue.clone()));
    let gpu = GpuDevice::new(executor.core.clone()).unwrap();
    let controller = if imported {
        gpu.create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions {
                clear_color: true,
                write_color: true,
                write_resources: true,
                import_images: true,
                ..neocogi_platform::render::abi::RenderClientPermissions::none()
            },
        )
    } else {
        gpu.create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions::default(),
        )
    }
    .unwrap();
    let resources = controller.resources().unwrap();
    let source = Bytes::source(
        br#"(module
  (struct Output (pos vec4 (builtin position)) (uv vec2 (location 0)) (color vec4 (location 1)))
  (vertex vs ((p vec2 (location 0)) (color vec4 (location 1))) (returns Output)
    (var out Output)
    (set (. out pos) (vec4 p 0.5 1.0))
    (set (. out uv) (vec2 (+ (* (. p x) 0.5) 0.5) (- 0.5 (* (. p y) 0.5))))
    (set (. out color) color)
    (return out))
  (struct Values (tint vec4))
  (texture image fragment 0)
  (uniform values Values fragment 0)
  (fragment fs ((input Output)) (returns vec4 (location 0))
    (return (* (* (sample image (. input uv)) (. input color)) (. values tint)))))"#
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
            blend: GpuBlendState::Blend { color: operation, alpha: operation },
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
            },
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 0,
                kind: GpuBindingKind::Uniform(16),
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
        .create_static_buffer(GpuBufferDesc::Vertex { byte_len: 48 }, source.as_com_ref())
        .unwrap();
    let source = Bytes::source([0u16, 1, 2, 0, 2, 3].into_iter().flat_map(u16::to_le_bytes).collect());
    let indices = resources
        .create_static_buffer(GpuBufferDesc::Index { byte_len: 12 }, source.as_com_ref())
        .unwrap();
    let image = resources
        .create_streamed_buffer(GpuBufferDesc::Texture {
            size: Size { width: 2, height: 2 },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        })
        .unwrap();
    crate::gpu::testing::write_pixels(
        image.as_com_ref(),
        Rect { x: 0, y: 0, width: 2, height: 2 },
        &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255],
    )
    .unwrap();
    let image = if imported {
        import_native_image(&gpu, &resources, &mut executor, image)
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
    let _uniform = crate::gpu::testing::bind_uniform(resources.as_com_ref(), &mut view, GpuShaderStage::Fragment, 0, &white).unwrap();
    view.push_view(Rect { x: 0, y: 0, width: 4, height: 8 }).unwrap();
    view.draw_indexed(6, 1, 0).unwrap();
    drop(_uniform);
    view.pop().unwrap();
    let image_b = if imported {
        let pixel = resources
            .create_static_buffer(
                GpuBufferDesc::Texture {
                    size: Size { width: 2, height: 2 },
                    format: GpuTextureFormat::Rgba8Unorm,
                    mip_count: 1,
                },
                Bytes::source(vec![255; 16]).as_com_ref(),
            )
            .unwrap();
        let image_b = import_native_image(&gpu, &resources, &mut executor, pixel);
        view.bind_texture(GpuShaderStage::Fragment, 0, image_b.as_com_ref(), sampler.as_com_ref())
            .unwrap();
        Some(image_b)
    } else {
        crate::gpu::testing::write_pixels(image.as_com_ref(), Rect { x: 0, y: 0, width: 2, height: 2 }, &[255; 16]).unwrap();
        None
    };
    let magenta: Vec<_> = [1.0f32, 0., 1., 1.].into_iter().flat_map(f32::to_le_bytes).collect();
    let _uniform = crate::gpu::testing::bind_uniform(resources.as_com_ref(), &mut view, GpuShaderStage::Fragment, 0, &magenta).unwrap();
    view.push_view(Rect { x: 4, y: 0, width: 4, height: 8 }).unwrap();
    view.draw_indexed(6, 1, 0).unwrap();
    drop(_uniform);
    view.pop().unwrap();
    drop(image_b);
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    recording.finish().unwrap();
    let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
    let prepared = executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, true).unwrap();
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
    let depth = WgpuRenderer::create_depth_target(&executor.core, 8, 8).unwrap();
    let mut encoder = native.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(if operation == GpuBlendEquation::Min {
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
    executor.reserve_submission().unwrap();
    executor.submitted(vec![prepared], Vec::new());
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
    let mut executor = crate::gpu::backends::wgpu::generic::Executor::new(Core::new(native.clone(), queue.clone()));
    let gpu = GpuDevice::new(executor.core.clone()).unwrap();
    let controller = gpu
        .create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions::default(),
        )
        .unwrap();
    let resources = controller.resources().unwrap();
    let owner = clear_block(&gpu, &resources);
    let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
    let prepared = executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, true).unwrap();
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
    let mut executor = crate::gpu::backends::wgpu::generic::Executor::new(Core::new(native.clone(), queue.clone()));
    let gpu = GpuDevice::new(executor.core.clone()).unwrap();
    let controller = gpu
        .create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions::default(),
        )
        .unwrap();
    let resources = controller.resources().unwrap();
    let owner = clear_block(&gpu, &resources);
    let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
    for _ in 0..2 {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, true)
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
    // Both paths reach the same scoped error after capturing their native image.
    // The imported path has pending ordinary queue initialization and a facade
    // which is removed before completion, rather than a generic pixel upload.
    for imported in [false, true] {
        let (native, queue) = device_with_limits(wgpu::Limits {
            max_buffer_size: 32,
            ..Default::default()
        });
        let outer = native.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut executor = crate::gpu::backends::wgpu::generic::Executor::new(Core::new(native.clone(), queue.clone()));
        let gpu = GpuDevice::new(executor.core.clone()).unwrap();
        let controller = gpu
            .create_client(
                RenderClientLimits::default(),
                gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
                neocogi_platform::render::abi::RenderClientPermissions {
                    clear_color: true,
                    write_color: true,
                    write_resources: true,
                    import_images: true,
                    ..neocogi_platform::render::abi::RenderClientPermissions::none()
                },
            )
            .unwrap();
        let resources = controller.resources().unwrap();
        let source = Bytes::source(
            br#"(module
  (vertex vs ((p vec3 (location 0))) (returns vec4 (builtin position)) (return (vec4 p 1.0)))
  (texture image fragment 0)
  (struct Values (tint vec4))
  (uniform values Values fragment 0)
  (fragment fs () (returns vec4 (location 0)) (return (* (sample image (vec2 0.5)) (. values tint)))))"#
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
                },
                GpuShaderBinding {
                    stage: GpuShaderStage::Fragment,
                    slot: 0,
                    kind: GpuBindingKind::Uniform(16),
                },
            ],
        }
        .source()
        .unwrap();
        let pipeline = resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
        let vertices = resources
            .create_static_buffer(GpuBufferDesc::Vertex { byte_len: 36 }, Bytes::source(vec![0; 36]).as_com_ref())
            .unwrap();
        let image = resources
            .create_static_buffer(
                GpuBufferDesc::Texture {
                    size: Size { width: 2, height: 2 },
                    format: GpuTextureFormat::Rgba8Unorm,
                    mip_count: 1,
                },
                Bytes::source(vec![255; 16]).as_com_ref(),
            )
            .unwrap();
        let image = if imported {
            import_native_image(&gpu, &resources, &mut executor, image)
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
        view.bind_vertex_buffer(0, vertices.as_com_ref(), 0).unwrap();
        view.bind_texture(GpuShaderStage::Fragment, 0, image.as_com_ref(), sampler.as_com_ref())
            .unwrap();
        let _uniform = crate::gpu::testing::bind_uniform(resources.as_com_ref(), &mut view, GpuShaderStage::Fragment, 0, &[0; 16]).unwrap();
        view.draw(3, 1, 0).unwrap();
        drop(_uniform);
        view.finish().unwrap();
        let owner = session.take_block().unwrap();
        recording.finish().unwrap();
        let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
        let error = match executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, true) {
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
        // Scoped validation errors leave this executor usable. A fresh valid image
        // must not observe a leaked scope or a poisoned native cache from the failure.
        let retry = resources
            .create_static_buffer(
                GpuBufferDesc::Texture {
                    size: Size { width: 1, height: 1 },
                    format: GpuTextureFormat::Rgba8Unorm,
                    mip_count: 1,
                },
                Bytes::source(vec![255; 4]).as_com_ref(),
            )
            .unwrap();
        executor
            .prepare_texture(gpu.texture_snapshot(resources.as_com_ref(), retry.as_com_ref()).unwrap())
            .unwrap();
        drop(retry);
        native.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        executor.collect().unwrap();
        assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
        assert!(pollster::block_on(outer.pop()).is_none());
    }
}

/// Trusted native image is captured directly, without a public COM image lease or identity map.
fn import_native_image(
    gpu: &GpuDevice,
    resources: &ComPtr<IRenderResourcesAbi>,
    executor: &mut crate::gpu::backends::wgpu::generic::Executor,
    pixel: ComBox<IGpuBufferAbi>,
) -> ComBox<IGpuBufferAbi> {
    let source = gpu.texture_snapshot(resources.as_com_ref(), pixel.as_com_ref()).unwrap();
    let image = executor
        .image(
            source.desc().size,
            wgpu::TextureFormat::Rgba8Unorm,
            Some(source.pixels().unwrap().as_slice()),
            None,
            false,
        )
        .unwrap();
    gpu.import_texture(resources.as_com_ref(), image).unwrap()
}

/// An import-only preflight must flush source initialization and retire it without a target frame.
#[test]
#[cfg(feature = "test-support")]
#[ignore = "requires wgpu,test-support and a native WGPU adapter with allocation counters"]
fn imported_preparation_flushes_initialization_before_refunding_native_storage() {
    let (native, queue) = device();
    let mut executor = crate::gpu::backends::wgpu::generic::Executor::new(Core::new(native.clone(), queue.clone()));
    let gpu = GpuDevice::new(executor.core.clone()).unwrap();
    let controller = gpu
        .create_client(
            RenderClientLimits { live_bytes: 8, ..Default::default() },
            gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions {
                clear_color: true,
                write_color: true,
                write_resources: true,
                import_images: true,
                ..neocogi_platform::render::abi::RenderClientPermissions::none()
            },
        )
        .unwrap();
    let resources = controller.resources().unwrap();
    let baseline = native.get_internal_counters().hal.texture_memory.read();
    for value in 0..8 {
        let pixels = [value, 0, 0, 255];
        let image = executor
            .image(Size { width: 1, height: 1 }, wgpu::TextureFormat::Rgba8Unorm, Some(&pixels), None, false)
            .unwrap();
        assert!(
            native.get_internal_counters().hal.texture_memory.read() > baseline,
            "this native regression requires observable image allocation counters"
        );
        let imported = gpu.import_texture(resources.as_com_ref(), image).unwrap();
        executor
            .prepare_texture(gpu.imported_texture_snapshot(resources.as_com_ref(), imported.as_com_ref()).unwrap())
            .unwrap();
        drop(imported);
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

/// Successful target variants own native objects; failed variants neither poison siblings nor pin sources.
#[test]
#[ignore = "requires a native WGPU adapter"]
fn native_slots_reuse_variants_reject_other_devices_and_release_without_submission() {
    let (native, queue) = device();
    let mut executor = crate::gpu::backends::wgpu::generic::Executor::new(Core::new(native.clone(), queue.clone()));
    let gpu = GpuDevice::new(executor.core.clone()).unwrap();
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions {
                clear_color: true,
                write_color: true,
                write_resources: true,
                import_images: true,
                ..neocogi_platform::render::abi::RenderClientPermissions::none()
            },
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let (other_native, other_queue) = device();
    let mut other = crate::gpu::backends::wgpu::generic::Executor::new(Core::new(other_native, other_queue));
    assert_ne!(gpu.driver_identity().as_raw().object, other.core.identity.as_raw().object);
    let shader = &super::derivatives::cases()[0].source;
    for _ in 0..8 {
        let owner = super::derivatives::block(&gpu, &resources, shader);
        let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
        let crate::gpu::command::Command::Draw(draw) = &snapshot.commands()[0] else {
            panic!("draw")
        };
        let source = Rc::downgrade(&draw.pipeline);
        assert!(other.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, false).is_err());
        let first = executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, false).unwrap();
        let second = executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, true).unwrap();
        assert!(executor.prepare(&snapshot, wgpu::TextureFormat::Depth24Plus, false).is_err());
        let repeated = executor.prepare(&snapshot, wgpu::TextureFormat::Rgba8Unorm, false).unwrap();
        assert!(Rc::ptr_eq(&first.first_pipeline(), &repeated.first_pipeline()));
        assert!(!Rc::ptr_eq(&first.first_pipeline(), &second.first_pipeline()));
        assert!(other.admit(&first, (wgpu::TextureFormat::Rgba8Unorm, false)).is_err());
        drop((snapshot, owner));
        assert!(source.upgrade().is_none(), "native siblings must not retain their parent version");
        drop((first, second, repeated));
        executor.collect().unwrap();
        assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
        assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap(), (0, 0));
    }
}

/// A real native queue-submit validation error stays in Result and retains uncertain initialization.
#[test]
#[ignore = "requires a native WGPU adapter"]
fn initialization_submission_is_scoped_and_invalidates_all_compatible_facades_on_failure() {
    let (native, queue) = device();
    let core = Core::new(native.clone(), queue);
    let mut executor = crate::gpu::backends::wgpu::generic::Executor::new(core.clone());
    let gpu = GpuDevice::new(core.clone()).unwrap();
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions::default(),
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let image = resources
        .create_static_buffer(
            GpuBufferDesc::Texture {
                size: Size { width: 1, height: 1 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            neocogi_platform::render::source::Bytes::source(vec![255; 4]).as_com_ref(),
        )
        .unwrap();
    let source = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
    let outer = native.push_error_scope(wgpu::ErrorFilter::Validation);
    executor.submit_failure = true;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| executor.prepare_texture(source.clone())));
    let error = match result {
        Ok(Err(error)) => error,
        Ok(Ok(())) => panic!("native submission must fail"),
        Err(_) => panic!("submission must return its scoped error"),
    };
    assert!(error.to_lowercase().contains("mapped"), "unexpected error: {error}");
    assert!(pollster::block_on(outer.pop()).is_none(), "submission scopes must own the native error");
    let mut compatible = crate::gpu::backends::wgpu::generic::Executor::new(core);
    assert!(
        compatible.prepare_texture(source.clone()).is_err(),
        "a Ready slot cannot bypass failed initialization through another facade"
    );
    drop((source, image, compatible));
    native.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    assert!(executor.collect().is_err());
    assert_eq!(
        gpu.live_bytes(resources.as_com_ref()).unwrap(),
        4,
        "uncertain ownership must survive a callback until final idle teardown"
    );
    drop(executor);
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
}

/// Queue writes refresh the same native destinations after completed upload owners release them.
#[test]
#[ignore = "requires a native WGPU adapter, including a software adapter"]
fn uploaded_resource_slots_keep_native_storage_across_queue_updates() {
    let (native, queue) = device();
    let scope = native.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut executor = crate::gpu::backends::wgpu::generic::Executor::new(Core::new(native.clone(), queue));
    let gpu = GpuDevice::new(executor.core.clone()).unwrap();
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(RenderSurfaceKind::Framebuffer).unwrap(),
            Default::default(),
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let buffer = resources.create_dynamic_buffer(GpuBufferDesc::Vertex { byte_len: 17 }).unwrap();
    let image = resources
        .create_dynamic_buffer(GpuBufferDesc::Texture {
            size: Size { width: 2, height: 2 },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        })
        .unwrap();
    let mut expected = None;
    for fill in 1..=8u8 {
        crate::gpu::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 17]).unwrap();
        crate::gpu::testing::write_pixels(image.as_com_ref(), Rect { x: 0, y: 0, width: 2, height: 2 }, &[fill; 16]).unwrap();
        let source = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
        let pixels = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
        let backing = source.native(&mut executor.test_preparation()).unwrap();
        let texture = pixels.native(&mut executor.test_preparation()).unwrap();
        let identity = (Rc::as_ptr(&backing), Rc::as_ptr(&texture));
        assert_eq!(*expected.get_or_insert(identity), identity);
        drop((source, pixels));
        assert_eq!(crate::gpu::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 4]), Err(RENDER_E_BUSY));
        assert_eq!(crate::gpu::testing::write_buffer(image.as_com_ref(), 0, &[fill; 4]), Err(RENDER_E_BUSY));
        drop((backing, texture));
        executor.submit(None, Vec::new(), Vec::new(), Vec::new()).unwrap();
        native.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        executor.collect().unwrap();
        crate::gpu::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 4]).unwrap();
        crate::gpu::testing::write_buffer(image.as_com_ref(), 0, &[fill; 4]).unwrap();
    }
    drop((buffer, image));
    executor.collect().unwrap();
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
    assert!(pollster::block_on(scope.pop()).is_none());
}
