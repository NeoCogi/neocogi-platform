//! Actual generated recorder ABI tests cover admission, revocation, ordering and immutable draws.
use super::*;
use crate::gpu::testing::device as new_device;
type GpuDevice = crate::gpu::GpuDevice<crate::gpu::testing::Backend>;
use neocogi_com::{NcCopyFromBufferOffset, ComInlineValue, NcOutputWriter, NC_E_DROPPED};

#[com_object(interfaces(IGpuDataSource))]
/// Independently callable exact byte producer, without access to renderer internals.
struct Bytes {
    data: Vec<u8>,
}
impl IGpuDataSource for Bytes {
    /// Fixed total length.
    fn byte_count(&self) -> Result<u64, NcStatus> {
        Ok(self.data.len() as u64)
    }
    /// Initialize the complete requested prefix.
    fn read(&self, output: NcOutputWriter<'_, u8>) -> Result<(), NcStatus> {
        copy(&self.data, output)
    }
}
/// Typed source constructor; bytes remain owned until the synchronous factory copy ends.
fn bytes(data: Vec<u8>) -> ComPtr<IGpuDataSourceAbi> {
    Bytes::from_value(Bytes { data }).query().unwrap()
}

/// Exact initialized output copied through generated thunks, not a native slice alias.
fn copy<T: ComInlineValue>(data: &[T], mut output: NcOutputWriter<'_, T>) -> Result<(), NcStatus> {
    let start = usize::try_from(output.offset()).map_err(|_| NC_E_OUT_OF_RANGE)?;
    let end = start.checked_add(output.capacity()).ok_or(NC_E_OUT_OF_RANGE)?;
    output.extend_from_slice(data.get(start..end).ok_or(NC_E_OUT_OF_RANGE)?)
}

#[com_object(interfaces(IGpuPipelineSource))]
/// Pipeline descriptor reader owned by the client fixture.
struct PipelineSource {
    /// Counts derive from the arrays in fixture construction.
    info: GpuPipelineInfo,
    /// Explicit stream stepping.
    layouts: Vec<GpuVertexBufferLayout>,
    /// Explicit attribute footprints.
    attributes: Vec<GpuVertexAttribute>,
    /// Exact stage resource expectations.
    bindings: Vec<GpuShaderBinding>,
}
impl IGpuPipelineSource for PipelineSource {
    /// Fixed header snapshot.
    fn info(&self) -> Result<GpuPipelineInfo, NcStatus> {
        Ok(self.info)
    }
    /// Exact stream output.
    fn vertex_buffers(&self, output: NcOutputWriter<'_, GpuVertexBufferLayout>) -> Result<(), NcStatus> {
        copy(&self.layouts, output)
    }
    /// Exact attribute output.
    fn attributes(&self, output: NcOutputWriter<'_, GpuVertexAttribute>) -> Result<(), NcStatus> {
        copy(&self.attributes, output)
    }
    /// Exact binding output.
    fn bindings(&self, output: NcOutputWriter<'_, GpuShaderBinding>) -> Result<(), NcStatus> {
        copy(&self.bindings, output)
    }
}

/// Translated visible panel, distinct from its widget-local root origin.
fn root() -> RenderViewRoot {
    RenderViewRoot {
        client: Rect { x: 5, y: 7, width: 100, height: 80 },
        target: Rect { x: 10, y: 20, width: 100, height: 80 },
    }
}

/// A facade's lifetime cannot turn an unfinished host-owned recording into deliberate cancellation.
#[test]
fn dropping_unfinished_view_preserves_recording_until_host_expiry() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.clear_color(0xff112233).unwrap();
    let pending = device.recording_usage(factory.as_com_ref()).unwrap().0;
    assert!(pending > 0);
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap().1, 1);
    drop(view);
    assert_eq!(session.state().unwrap(), RenderViewState::Recording);
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap(), (pending, 0));
    assert_eq!(session.view().err(), Some(RENDER_E_STATE));
    assert_eq!(frame.finish(), Err(RENDER_E_STATE));
    session.expire().unwrap();
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap(), (0, 0));
    frame.finish().unwrap();
}

/// The first command error survives both explicit cancellation and client-owner destruction.
#[test]
fn cancel_and_drop_preserve_rejection_and_its_first_status() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.clear_color(0xff112233).unwrap();
    assert_eq!(view.clear_depth(f32::NAN), Err(NC_E_INVALID_ARG));
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap().0, 0);
    view.cancel().unwrap();
    view.cancel().unwrap();
    assert_eq!(session.state().unwrap(), RenderViewState::Rejected);
    assert_eq!(view.clear_color(0), Err(NC_E_INVALID_ARG));
    assert_eq!(view.finish(), Err(NC_E_INVALID_ARG));
    drop(view);
    assert_eq!(session.state().unwrap(), RenderViewState::Rejected);
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap(), (0, 0));
    assert_eq!(frame.finish(), Err(RENDER_E_STATE));
    frame.abort().unwrap();
    assert_eq!(session.state().unwrap(), RenderViewState::Expired);
}

/// Healthy cancellation releases payload immediately and is the only client abandonment success.
#[test]
fn healthy_cancel_is_idempotent_and_finish_survives_facade_drop() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.clear_color(0xff112233).unwrap();
    view.cancel().unwrap();
    view.cancel().unwrap();
    assert_eq!(session.state().unwrap(), RenderViewState::Cancelled);
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap().0, 0);
    drop(view);
    frame.finish().unwrap();
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.finish().unwrap();
    view.cancel().unwrap();
    drop(view);
    assert_eq!(session.state().unwrap(), RenderViewState::Committed);
    let block = session.take_block().unwrap();
    assert_eq!(session.take_block().err(), Some(RENDER_E_STATE));
    frame.finish().unwrap();
    drop(block);
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap(), (0, 0));
}
/// Retains the host controller while clients share a restricted factory.
fn client(device: &GpuDevice, limits: RenderClientLimits) -> (ComBox<IRenderClientDomainAbi>, ComPtr<IRenderResourcesAbi>) {
    let controller = device
        .create_client(
            limits,
            device.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions::default(),
        )
        .unwrap();
    let factory = controller.resources().unwrap();
    (controller, factory)
}
/// Publishes GPU bytes through the actual resource factory ABI.
fn buffer(factory: &ComPtr<IRenderResourcesAbi>, role: GpuBufferRole, data: Vec<u8>) -> ComBox<IGpuBufferAbi> {
    let len = data.len();
    factory
        .create_static_buffer(GpuBufferDesc::linear(role, len as u64).unwrap(), bytes(data).as_com_ref())
        .unwrap()
}
/// Client-side explicit f32 layout; no generated struct is reinterpreted.
fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|value| value.to_le_bytes()).collect()
}

/// Ordinary shader with one float3 stream and no semantic transforms.
const VERTICES: &str = r#"
@vertex fn vs(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> { return vec4<f32>(position, 1.0); }
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.2, 0.4, 0.8, 1.0); }
"#;
/// A generic procedural pipeline needs no vertex-buffer binding at all.
const PROCEDURAL: &str = r#"
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> { return vec4<f32>(f32(index % 2u), 0.0, 0.5, 1.0); }
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
"#;

/// Explicit depth permissions control initialization and clearing without assigning a rendering role.
#[test]
fn color_only_groups_seal_without_depth_initialization_and_cannot_clear_depth() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let color_only = device
        .create_client(
            RenderClientLimits::default(),
            device.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions {
                clear_color: true,
                write_color: true,
                write_resources: true,
                import_images: true,
                ..neocogi_platform::render::abi::RenderClientPermissions::none()
            },
        )
        .unwrap();
    let depth_enabled = device
        .create_client(
            RenderClientLimits::default(),
            device.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions::default(),
        )
        .unwrap();
    for (controller, initialize_depth) in [(&color_only, false), (&depth_enabled, true)] {
        let resources = controller.resources().unwrap();
        assert!(resources.query::<IRenderHostAbi>().is_err());
        let mut frame = device.broker().begin_frame().unwrap();
        let mut session = frame.begin_view(root(), resources.as_com_ref()).unwrap();
        session.view().unwrap().finish().unwrap();
        let owner = session.take_block().unwrap();
        let block = device.block_snapshot(owner.as_com_ref()).unwrap();
        assert_eq!(block.domain.permissions.initialize_depth, initialize_depth);
        if initialize_depth {
            assert!(matches!(block.commands.as_slice(), [Command::ClearDepth { depth: 1.0, .. }]));
        } else {
            assert!(block.commands.is_empty());
        }
        frame.finish().unwrap();
    }
    let resources = color_only.resources().unwrap();
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), resources.as_com_ref()).unwrap();
    assert_eq!(session.view().unwrap().clear_depth(1.0), Err(RENDER_E_PERMISSION));
    assert!(session.take_block().is_err());
}

/// A color-only group cannot test or write depth even when its framebuffer target has a depth attachment.
#[test]
fn color_only_group_pipeline_admission_excludes_depth_state() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let controller = device
        .create_client(
            RenderClientLimits::default(),
            device.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
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
    let source = crate::gpu::source::Bytes::source(PROCEDURAL.as_bytes().to_vec());
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    for state in [
        GpuPipelineInfo { depth_write: true, ..Default::default() },
        GpuPipelineInfo {
            depth_compare: GpuCompare::LessEqual,
            ..Default::default()
        },
    ] {
        let desc = crate::gpu::source::PipelineDesc { state, ..Default::default() }.source().unwrap();
        assert!(matches!(
            resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()),
            Err(RENDER_E_PERMISSION)
        ));
    }
    let desc = crate::gpu::source::PipelineDesc::default().source().unwrap();
    assert!(resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).is_ok());
}

/// Pipeline construction captures shader artifacts while the public shader owners are local.
fn make_pipeline(
    factory: &ComPtr<IRenderResourcesAbi>,
    code: &str,
    layouts: Vec<GpuVertexBufferLayout>,
    attributes: Vec<GpuVertexAttribute>,
    bindings: Vec<GpuShaderBinding>,
) -> ComBox<IGpuPipelineAbi> {
    let source = bytes(code.as_bytes().to_vec());
    let vertex = factory
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fragment = factory
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let info = GpuPipelineInfo {
        vertex_buffer_count: layouts.len() as u32,
        attribute_count: attributes.len() as u32,
        binding_count: bindings.len() as u32,
        ..GpuPipelineInfo::default()
    };
    let source: ComPtr<IGpuPipelineSourceAbi> = PipelineSource::from_value(PipelineSource { info, layouts, attributes, bindings })
        .query()
        .unwrap();
    factory
        .create_pipeline(source.as_com_ref(), vertex.as_com_ref(), fragment.as_com_ref())
        .unwrap()
}
/// Standard float3 test layout, still owned entirely by the client.
fn vertex_pipeline(factory: &ComPtr<IRenderResourcesAbi>) -> ComBox<IGpuPipelineAbi> {
    make_pipeline(
        factory,
        VERTICES,
        vec![GpuVertexBufferLayout {
            slot: 0,
            stride: 12,
            step: GpuStepMode::Vertex,
        }],
        vec![GpuVertexAttribute {
            location: 0,
            slot: 0,
            format: GpuVertexFormat::Float32x3,
            offset: 0,
        }],
        Vec::new(),
    )
}

/// Captured data survives resource owner removal and a successfully finished CPU frame.
#[test]
fn draws_snapshot_versions_in_order_and_finished_owner_drop_preserves_the_block() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let pipeline = vertex_pipeline(&factory);
    let vertices = factory.create_streamed_buffer(GpuBufferDesc::Vertex { byte_len: 36 }).unwrap();
    let broker = device.broker();
    let mut frame = broker.begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.bind_vertex_buffer(0, vertices.as_com_ref(), 0).unwrap();
    view.draw(3, 1, 0).unwrap();
    let replacement = floats(&[0.25]);
    crate::gpu::testing::write_buffer(vertices.as_com_ref(), 0, &replacement).unwrap();
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    assert_eq!(session.state().unwrap(), RenderViewState::Committed);
    drop(view);
    drop(vertices);
    drop(pipeline);
    assert!(frame.finish().is_err()); // Sealed does not mean harvested.
    let owner = session.take_block().unwrap();
    assert!(session.take_block().is_err());
    frame.finish().unwrap();
    let block = device.block_snapshot(owner.as_com_ref()).unwrap();
    assert_eq!(block.commands.len(), 3); // One root depth init plus two generic draws.
    let Command::Draw(a) = &block.commands[1] else { panic!("first generic draw") };
    let Command::Draw(b) = &block.commands[2] else {
        panic!("second generic draw")
    };
    assert!(!Rc::ptr_eq(&a.vertex_buffers[0].buffer, &b.vertex_buffers[0].buffer));
    assert_eq!(&a.vertex_buffers[0].buffer.bytes.as_slice()[..4], &0.0f32.to_le_bytes());
    assert_eq!(&b.vertex_buffers[0].buffer.bytes.as_slice()[..4], &0.25f32.to_le_bytes());
    assert_eq!(a.viewport.x, 10);
    assert_eq!(a.viewport.width, 100);
}

/// Native lowering retains capacity without retaining a block, public facade or domain.
#[test]
fn lowered_capacity_receipts_survive_blocks_and_refund_only_after_the_last_copy() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.clear_color(0xffabcdef).unwrap();
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    frame.finish().unwrap();
    let block = device.block_snapshot(owner.as_com_ref()).unwrap();
    let charges = block.retain_capacity();
    let before = device.recording_usage(factory.as_com_ref()).unwrap().0;
    assert!(before > 0);
    let retained = charges.clone();
    drop((block, owner, view, session, frame, charges));
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap(), (before, 0));
    drop(retained);
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap(), (0, 0));
}

/// Dropping a bound public owner before a consuming draw rejects without pinning it.
#[test]
fn weak_bindings_do_not_keep_resource_owners_alive() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let pipeline = vertex_pipeline(&factory);
    let vertices = buffer(&factory, GpuBufferRole::Vertex, floats(&[0.0; 9]));
    let weak = vertices.downgrade();
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.bind_vertex_buffer(0, weak.as_com_ref(), 0).unwrap();
    drop(vertices);
    assert!(!weak.is_alive());
    assert!(matches!(view.draw(3, 1, 0), Err(NC_E_DROPPED)));
    assert_eq!(session.state().unwrap(), RenderViewState::Rejected);
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap().0, 0);
    assert!(view.finish().is_err());
    view.cancel().unwrap();
    assert_eq!(frame.finish(), Err(RENDER_E_STATE));
    frame.abort().unwrap();
}

/// Host expiry releases payload/capacity even while a client keeps the ComBox facade.
#[test]
fn retained_expired_views_keep_only_bounded_shells_and_cannot_reactivate() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(
        &device,
        RenderClientLimits {
            live_views: 2,
            ..RenderClientLimits::default()
        },
    );
    let broker = device.broker();
    let mut retained = Vec::new();
    for expected in 1..=2 {
        let mut frame = broker.begin_frame().unwrap();
        let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
        let mut view = session.view().unwrap();
        view.clear_color(0xff123456).unwrap();
        assert!(device.recording_usage(factory.as_com_ref()).unwrap().0 > 0);
        drop(session);
        frame.finish().unwrap();
        assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap(), (0, expected));
        assert!(matches!(view.clear_color(0), Err(RENDER_E_EXPIRED)));
        retained.push(view);
    }
    let mut frame = broker.begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    assert!(matches!(session.view(), Err(NC_E_OUT_OF_RANGE)));
    drop(session);
    frame.finish().unwrap();
    for mut stale in retained {
        assert!(stale.reset().is_err());
        stale.cancel().unwrap();
    }
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap(), (0, 0));
    let mut frame = broker.begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.cancel().unwrap();
    frame.finish().unwrap();
}

/// Abandoned CPU traversals restore admission without needing a native render call.
#[test]
fn frame_drop_aborts_pending_work_and_old_finished_drop_cannot_abort_a_new_frame() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let broker = device.broker();
    let mut first = broker.begin_frame().unwrap();
    assert!(broker.begin_frame().is_err());
    let mut session = first.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    first.abort().unwrap();
    assert!(view.info().is_err());
    assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap().0, 0);
    let mut next = broker.begin_frame().unwrap();
    drop(first); // Must not reset the next frame's active flag.
    assert!(broker.begin_frame().is_err());
    next.finish().unwrap();
    let mut final_frame = broker.begin_frame().unwrap();
    let mut session = final_frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut final_view = session.view().unwrap();
    drop(final_frame);
    assert!(matches!(final_view.clear_color(0), Err(RENDER_E_EXPIRED)));
    assert!(view.finish().is_err());
}

/// Attempts span sessions, including cancelled/empty work and automatic regional depth setup.
#[test]
fn per_frame_work_is_not_refunded_by_cancel_or_sequential_view_creation() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(
        &device,
        RenderClientLimits {
            frame_draws: 3,
            ..RenderClientLimits::default()
        },
    );
    let broker = device.broker();
    let mut frame = broker.begin_frame().unwrap();
    let mut first = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut a = first.view().unwrap();
    a.clear_color(0).unwrap(); // Root init + clear = 2 attempts.
    a.cancel().unwrap();
    let mut second = frame.begin_view(root(), factory.as_com_ref()).unwrap(); // Third attempt.
    let mut b = second.view().unwrap();
    assert!(matches!(b.clear_depth(0.25), Err(NC_E_OUT_OF_RANGE)));
    b.cancel().unwrap();
    assert!(frame.begin_view(root(), factory.as_com_ref()).is_err());
    frame.abort().unwrap();
    let mut next = broker.begin_frame().unwrap();
    let mut session = next.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.clear_depth(0.25).unwrap();
    view.cancel().unwrap();
    next.finish().unwrap();
}

/// Explicit nested depth/color clears carry their bounded target rectangles and exact values.
#[test]
fn scoped_clears_flatten_clips_and_reset_does_not_clear_attachment_depth() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.push_view(Rect { x: 20, y: 10, width: 50, height: 40 }).unwrap();
    view.push_clip(Rect {
        x: -1000,
        y: -1000,
        width: i32::MAX,
        height: i32::MAX,
    })
    .unwrap();
    view.clear_color(0xff332211).unwrap();
    view.clear_depth(0.25).unwrap();
    view.reset().unwrap();
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    frame.finish().unwrap();
    let block = device.block_snapshot(owner.as_com_ref()).unwrap();
    assert_eq!(block.commands.len(), 3);
    let Command::ClearColor { rgba, clip, .. } = block.commands[1] else {
        panic!("regional color")
    };
    assert_eq!(rgba, 0xff332211);
    assert_eq!((clip.x, clip.y, clip.width, clip.height), (30, 30, 50, 40));
    let Command::ClearDepth { depth, clip, .. } = block.commands[2] else {
        panic!("regional depth")
    };
    assert_eq!(depth, 0.25);
    assert_eq!((clip.x, clip.y, clip.width, clip.height), (30, 30, 50, 40));
}

/// Closed-domain commands retire even when committed, and private provenance survives clear-only data.
#[test]
fn domain_close_drains_sealed_pending_work_and_rejects_already_harvested_admission() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (mut controller, factory) = client(&device, RenderClientLimits::default());
    let account = device.domain(factory.as_com_ref()).unwrap().account.clone();
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.clear_color(0xff123456).unwrap();
    view.finish().unwrap();
    controller.close().unwrap();
    assert_eq!(account.pending_bytes(), 0);
    assert_eq!(session.state().unwrap(), RenderViewState::Expired);
    assert!(session.take_block().is_err());
    assert!(view.info().is_err());
    frame.abort().unwrap();
    let (mut controller, factory) = client(&device, RenderClientLimits::default());
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    frame.finish().unwrap();
    let captured = device.block_snapshot(owner.as_com_ref()).unwrap();
    controller.close().unwrap();
    assert!(captured.admit().is_err());
    assert!(device.block_snapshot(owner.as_com_ref()).is_err());
}

/// Runtime authority is object identity, not interface compatibility or a guessed owner number.
#[test]
fn other_device_blocks_factories_and_client_qi_cannot_recover_host_authority() {
    #[com_object(interfaces(IRenderViewBlock), ownership = exclusive)]
    /// Correct interface, no registered renderer provenance.
    struct Forged;
    impl IRenderViewBlock for Forged {}
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let other = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let (_other_controller, foreign) = client(&other, RenderClientLimits::default());
    let mut frame = device.broker().begin_frame().unwrap();
    assert!(matches!(frame.begin_view(root(), foreign.as_com_ref()), Err(RENDER_E_OWNER)));
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    assert!(view.query_ref::<IRenderViewSessionAbi>().is_err());
    assert!(view.query_ref::<IRenderViewBrokerAbi>().is_err());
    assert!(view.query_ref::<IRendererAbi>().is_err());
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    assert!(matches!(other.block_snapshot(owner.as_com_ref()), Err(RENDER_E_OWNER)));
    let forged: ComBox<IRenderViewBlockAbi> = Forged::into_box(Forged).query().unwrap();
    assert!(matches!(device.block_snapshot(forged.as_com_ref()), Err(RENDER_E_OWNER)));
    frame.finish().unwrap();
}

/// Zero-instance calls still consume CPU scanning budget and cannot bypass bounded index work.
#[test]
fn zero_instance_index_work_and_unsupported_restart_values_reject() {
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(
        &device,
        RenderClientLimits {
            frame_indices: 6,
            ..RenderClientLimits::default()
        },
    );
    let pipeline = make_pipeline(&factory, PROCEDURAL, Vec::new(), Vec::new(), Vec::new());
    let indices = buffer(&factory, GpuBufferRole::Index, vec![0; 20]);
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.bind_index_buffer(indices.as_com_ref(), GpuIndexFormat::U16, 0).unwrap();
    assert!(matches!(view.draw_indexed(10, 0, 0), Err(NC_E_OUT_OF_RANGE)));
    view.cancel().unwrap();
    assert_eq!(frame.finish(), Err(RENDER_E_STATE));
    frame.abort().unwrap();
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let pipeline = make_pipeline(&factory, PROCEDURAL, Vec::new(), Vec::new(), Vec::new());
    let indices = buffer(&factory, GpuBufferRole::Index, [u16::MAX, 0, 1].iter().flat_map(|n| n.to_le_bytes()).collect());
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.bind_index_buffer(indices.as_com_ref(), GpuIndexFormat::U16, 0).unwrap();
    assert!(matches!(view.draw_indexed(3, 1, 0), Err(NC_E_INVALID_ARG)));
}

/// Pipeline layouts combine independently created buffers; draw admission validates every stream.
#[test]
fn index_range_alignment_vertex_coverage_and_invalid_depth_reject_atomically() {
    for case in 0..5 {
        let device = new_device(
            neocogi_platform::render::compiler::CompilerObject::from_value(
                neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
            )
            .query()
            .unwrap(),
        );
        let (_controller, factory) = client(&device, RenderClientLimits::default());
        let pipeline = vertex_pipeline(&factory);
        let vertices = buffer(&factory, GpuBufferRole::Vertex, floats(&[0.0; 9]));
        let indices = buffer(&factory, GpuBufferRole::Index, [0u32, 1, 3].iter().flat_map(|n| n.to_le_bytes()).collect());
        let mut frame = device.broker().begin_frame().unwrap();
        let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
        let mut view = session.view().unwrap();
        view.bind_pipeline(pipeline.as_com_ref()).unwrap();
        view.bind_vertex_buffer(0, vertices.as_com_ref(), 0).unwrap();
        let error = match case {
            0 => {
                view.bind_index_buffer(indices.as_com_ref(), GpuIndexFormat::U32, 0).unwrap();
                view.draw_indexed(3, 1, 0)
            }
            1 => view.bind_index_buffer(indices.as_com_ref(), GpuIndexFormat::U32, 1),
            2 => view.draw(4, 1, 0),
            3 => view.clear_depth(f32::NAN),
            _ => view.clear_depth(1.25),
        };
        assert!(error.is_err());
        assert_eq!(session.state().unwrap(), RenderViewState::Rejected);
        assert_eq!(device.recording_usage(factory.as_com_ref()).unwrap().0, 0);
        assert!(view.reset().is_err());
        assert!(view.finish().is_err());
        view.cancel().unwrap();
        assert_eq!(frame.finish(), Err(RENDER_E_STATE));
        frame.abort().unwrap();
    }
}

/// A balanced finish and explicit cancellation are distinct terminal outcomes.
#[test]
fn unbalanced_stack_and_root_pop_reject_even_after_cancel() {
    for unbalanced in [false, true] {
        let device = new_device(
            neocogi_platform::render::compiler::CompilerObject::from_value(
                neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
            )
            .query()
            .unwrap(),
        );
        let (_controller, factory) = client(&device, RenderClientLimits::default());
        let mut frame = device.broker().begin_frame().unwrap();
        let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
        let mut view = session.view().unwrap();
        if unbalanced {
            view.push_clip(Rect { x: 0, y: 0, width: 10, height: 10 }).unwrap();
            assert!(view.finish().is_err());
        } else {
            assert!(view.pop().is_err());
        }
        assert!(session.take_block().is_err());
        assert!(frame.finish().is_err());
        view.cancel().unwrap();
        assert_eq!(session.state().unwrap(), RenderViewState::Rejected);
        assert_eq!(frame.finish(), Err(RENDER_E_STATE));
        frame.abort().unwrap();
    }
}

/// Per-instance streams need their own coverage; vertex count cannot certify their bounds.
#[test]
fn instance_streams_are_validated_independently_and_capture_each_binding() {
    const CODE: &str = r#"
@vertex fn vs(@location(0) position: vec3<f32>, @location(1) offset: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position.xy + offset, position.z, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
"#;
    for valid in [false, true] {
        let device = new_device(
            neocogi_platform::render::compiler::CompilerObject::from_value(
                neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
            )
            .query()
            .unwrap(),
        );
        let (_controller, factory) = client(&device, RenderClientLimits::default());
        let pipeline = make_pipeline(
            &factory,
            CODE,
            vec![
                GpuVertexBufferLayout {
                    slot: 0,
                    stride: 12,
                    step: GpuStepMode::Vertex,
                },
                GpuVertexBufferLayout {
                    slot: 1,
                    stride: 8,
                    step: GpuStepMode::Instance,
                },
            ],
            vec![
                GpuVertexAttribute {
                    location: 0,
                    slot: 0,
                    format: GpuVertexFormat::Float32x3,
                    offset: 0,
                },
                GpuVertexAttribute {
                    location: 1,
                    slot: 1,
                    format: GpuVertexFormat::Float32x2,
                    offset: 0,
                },
            ],
            Vec::new(),
        );
        let vertices = buffer(&factory, GpuBufferRole::Vertex, floats(&[0.0; 9]));
        let offsets = buffer(&factory, GpuBufferRole::Vertex, floats(&vec![0.0; if valid { 4 } else { 2 }]));
        let mut frame = device.broker().begin_frame().unwrap();
        let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
        let mut view = session.view().unwrap();
        view.bind_pipeline(pipeline.as_com_ref()).unwrap();
        view.bind_vertex_buffer(0, vertices.as_com_ref(), 0).unwrap();
        view.bind_vertex_buffer(1, offsets.as_com_ref(), 0).unwrap();
        assert_eq!(view.draw(3, 2, 0).is_ok(), valid);
        if valid {
            view.finish().unwrap();
            let owner = session.take_block().unwrap();
            let block = device.block_snapshot(owner.as_com_ref()).unwrap();
            let Command::Draw(draw) = &block.commands[1] else {
                panic!("generic instanced draw")
            };
            assert_eq!(draw.vertex_buffers.len(), 2);
            assert!(matches!(draw.range, DrawRange::Vertices { instances: 2, .. }));
        } else {
            view.cancel().unwrap();
            assert_eq!(frame.finish(), Err(RENDER_E_STATE));
            frame.abort().unwrap();
        }
        if valid {
            frame.finish().unwrap();
        }
    }
}

/// Uniform pushes and texture updates between draws cannot rewrite earlier recorded data.
#[test]
fn sampled_and_uniform_versions_are_captured_per_draw() {
    const CODE: &str = r#"
struct Tint { color: vec4<f32> }
@group(3) @binding(0) var<uniform> tint: Tint;
@group(2) @binding(0) var image: texture_2d<f32>;
@group(2) @binding(1) var image_sampler: sampler;
struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs(@location(0) position: vec3<f32>) -> Out { return Out(vec4<f32>(position, 1.0), vec2<f32>(0.5)); }
@fragment fn fs(input: Out) -> @location(0) vec4<f32> { return textureSample(image, image_sampler, input.uv) * tint.color; }
"#;
    let device = new_device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let pipeline = make_pipeline(
        &factory,
        CODE,
        vec![GpuVertexBufferLayout {
            slot: 0,
            stride: 12,
            step: GpuStepMode::Vertex,
        }],
        vec![GpuVertexAttribute {
            location: 0,
            slot: 0,
            format: GpuVertexFormat::Float32x3,
            offset: 0,
        }],
        vec![
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 0,
                kind: GpuBindingKind::Uniform,
                byte_len: 16,
            },
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 0,
                kind: GpuBindingKind::SampledTexture,
                byte_len: 0,
            },
        ],
    );
    let vertices = buffer(&factory, GpuBufferRole::Vertex, floats(&[0.0; 9]));
    let texture = factory
        .create_streamed_buffer(GpuBufferDesc::Texture {
            size: Size { width: 1, height: 1 },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        })
        .unwrap();
    crate::gpu::testing::write_pixels(texture.as_com_ref(), Rect { x: 0, y: 0, width: 1, height: 1 }, &[255; 4]).unwrap();
    let sampler = factory.create_sampler(GpuSamplerDesc::default()).unwrap();
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), factory.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.bind_vertex_buffer(0, vertices.as_com_ref(), 0).unwrap();
    view.bind_texture(GpuShaderStage::Fragment, 0, texture.as_com_ref(), sampler.as_com_ref())
        .unwrap();
    let a = floats(&[1.0; 4]);
    let _uniform_a = crate::gpu::testing::bind_uniform(factory.as_com_ref(), &mut view, GpuShaderStage::Fragment, 0, &a).unwrap();
    view.draw(3, 1, 0).unwrap();
    crate::gpu::testing::write_pixels(texture.as_com_ref(), Rect { x: 0, y: 0, width: 1, height: 1 }, &[0; 4]).unwrap();
    let b = floats(&[0.5; 4]);
    let _uniform_b = crate::gpu::testing::bind_uniform(factory.as_com_ref(), &mut view, GpuShaderStage::Fragment, 0, &b).unwrap();
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    let block = device.block_snapshot(owner.as_com_ref()).unwrap();
    let Command::Draw(first) = &block.commands[1] else {
        panic!("first sampled draw")
    };
    let Command::Draw(second) = &block.commands[2] else {
        panic!("second sampled draw")
    };
    assert_eq!(first.uniforms[0].data.bytes(), a);
    assert_eq!(second.uniforms[0].data.bytes(), b);
    assert_eq!(first.textures[0].texture.pixels().unwrap().as_slice(), [255; 4]);
    assert_eq!(second.textures[0].texture.pixels().unwrap().as_slice(), [0; 4]);
    frame.finish().unwrap();
}
