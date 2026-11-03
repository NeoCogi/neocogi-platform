//! Real generated calls exercise independent permissions, selected targets and group lifetime.
use super::*;
use crate::{command::Command, target::SurfaceTarget, testing};

type Device = GpuDevice<testing::Backend>;

/// Each fixture has its own native identity and the actual portable compiler provider.
fn device() -> Device {
    testing::device(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    )
}

/// Bounded full view, expressed in the same coordinates for the test's input and output.
fn root() -> RenderViewRoot {
    let bounds = Rect { x: 0, y: 0, width: 4, height: 4 };
    RenderViewRoot { client: bounds, target: bounds }
}

/// Hold the exclusive group controller while callers share only its resource factory.
fn group(
    device: &Device,
    target: Rc<SurfaceTarget<testing::Backend>>,
    permissions: RenderClientPermissions,
) -> (ComBox<IRenderClientDomainAbi>, ComPtr<IRenderResourcesAbi>) {
    let owner = device.create_client(RenderClientLimits::default(), target, permissions).unwrap();
    let resources = owner.resources().unwrap();
    (owner, resources)
}

/// Color and depth clears have independent permissions and rejected attempts reject their session.
#[test]
fn clear_permissions_are_independent_and_enforced_through_recording_calls() {
    let device = device();
    for color in [false, true] {
        for depth in [false, true] {
            let target = device.surface_target(RenderSurfaceKind::Framebuffer).unwrap();
            let (_owner, resources) = group(
                &device,
                target,
                RenderClientPermissions {
                    clear_color: color,
                    clear_depth: depth,
                    ..RenderClientPermissions::none()
                },
            );
            for test_depth in [false, true] {
                let mut frame = device.broker().begin_frame().unwrap();
                let mut session = frame.begin_view(root(), resources.as_com_ref()).unwrap();
                let mut view = session.view().unwrap();
                let result = if test_depth { view.clear_depth(0.5) } else { view.clear_color(0xff123456) };
                let allowed = if test_depth { depth } else { color };
                assert_eq!(result, if allowed { Ok(()) } else { Err(RENDER_E_PERMISSION) });
                if allowed {
                    view.finish().unwrap();
                    let owner = session.take_block().unwrap();
                    let block = device.block_snapshot(owner.as_com_ref()).unwrap();
                    assert_eq!(block.commands.len(), 1, "no implicit role-based clear is inserted");
                    frame.finish().unwrap();
                } else {
                    assert_eq!(session.state().unwrap(), RenderViewState::Rejected);
                    session.expire().unwrap();
                    frame.finish().unwrap();
                }
            }
        }
    }
}

/// Explicit depth initialization is a creation option; target kind alone never enables it.
#[test]
fn depth_initialization_requires_clear_permission_and_a_depth_attachment() {
    let device = device();
    let framebuffer = device.surface_target(RenderSurfaceKind::Framebuffer).unwrap();
    let image = device.surface_target(RenderSurfaceKind::Image).unwrap();
    let initialize = RenderClientPermissions {
        initialize_depth: true,
        ..RenderClientPermissions::none()
    };
    assert!(matches!(
        device.create_client(Default::default(), framebuffer.clone(), initialize),
        Err(NC_E_INVALID_ARG)
    ));
    let initialize = RenderClientPermissions { clear_depth: true, ..initialize };
    assert!(matches!(device.create_client(Default::default(), image, initialize), Err(NC_E_INVALID_ARG)));
    let (_owner, resources) = group(&device, framebuffer, initialize);
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), resources.as_com_ref()).unwrap();
    session.view().unwrap().finish().unwrap();
    let owner = session.take_block().unwrap();
    let block = device.block_snapshot(owner.as_com_ref()).unwrap();
    assert!(matches!(block.commands.as_slice(), [Command::ClearDepth { depth: 1.0, clip, .. }] if *clip == root().target));
    frame.finish().unwrap();
}

/// Disabling updates still allows initialized creation; a denied update publishes no new version.
#[test]
fn resource_updates_obey_write_permission_independently_of_drawing_permissions() {
    let device = device();
    for writable in [false, true] {
        let target = device.surface_target(RenderSurfaceKind::Framebuffer).unwrap();
        let (_owner, resources) = group(
            &device,
            target,
            RenderClientPermissions {
                write_resources: writable,
                ..RenderClientPermissions::none()
            },
        );
        let buffer = resources
            .create_buffer(
                GpuBufferDesc { role: GpuBufferRole::Vertex, byte_len: 4 },
                crate::source::initial_bytes(vec![1, 2, 3, 4]).borrow().unwrap(),
            )
            .unwrap();
        let texture = resources
            .create_texture(
                GpuTextureDesc {
                    size: Size { width: 1, height: 1 },
                    format: GpuTextureFormat::Rgba8Unorm,
                    mip_count: 1,
                },
                crate::source::initial_bytes(vec![1, 2, 3, 4]).borrow().unwrap(),
            )
            .unwrap();
        let patch = crate::source::Bytes::source(vec![9; 4]);
        let expected = if writable { Ok(()) } else { Err(RENDER_E_PERMISSION) };
        assert_eq!(resources.update_buffer(buffer.as_com_ref(), 0, patch.as_com_ref()), expected);
        assert_eq!(
            resources.update_texture(texture.as_com_ref(), Rect { x: 0, y: 0, width: 1, height: 1 }, patch.as_com_ref()),
            expected
        );
        let bytes = device.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
        assert_eq!(bytes.bytes.as_slice().unwrap(), if writable { &[9; 4] } else { &[1, 2, 3, 4] });
        let pixels = device.texture_snapshot(resources.as_com_ref(), texture.as_com_ref()).unwrap();
        assert_eq!(pixels.pixels().unwrap().as_slice().unwrap(), if writable { &[9; 4] } else { &[1, 2, 3, 4] });
    }
}

/// Attachment writes, depth comparisons and resource updates are not represented by one role flag.
#[test]
fn pipeline_color_write_depth_write_and_depth_read_permissions_are_separate() {
    let device = device();
    let source = crate::source::Bytes::source(
        br#"
        @vertex fn vs() -> @builtin(position) vec4<f32> { return vec4(0.0,0.0,0.5,1.0); }
        @fragment fn fs() -> @location(0) vec4<f32> { return vec4(1.0); }
    "#
        .to_vec(),
    );
    for permission in 0..8 {
        let permissions = RenderClientPermissions {
            write_color: permission & 1 != 0,
            write_depth: permission & 2 != 0,
            read_depth: permission & 4 != 0,
            ..RenderClientPermissions::none()
        };
        let target = device.surface_target(RenderSurfaceKind::Framebuffer).unwrap();
        let (_owner, resources) = group(&device, target, permissions);
        let vertex = resources
            .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), neocogi_com::NcCopyFromBufferOffset::new(0, b"vs"))
            .unwrap();
        let fragment = resources
            .create_shader(
                GpuShaderStage::Fragment,
                source.as_com_ref(),
                neocogi_com::NcCopyFromBufferOffset::new(0, b"fs"),
            )
            .unwrap();
        for requested in 0..8 {
            let description = crate::source::PipelineDesc {
                state: GpuPipelineInfo {
                    color_write: if requested & 1 != 0 { 15 } else { 0 },
                    depth_write: requested & 2 != 0,
                    depth_compare: if requested & 4 != 0 { GpuCompare::LessEqual } else { GpuCompare::Always },
                    ..GpuPipelineInfo::default()
                },
                ..crate::source::PipelineDesc::default()
            }
            .source()
            .unwrap();
            let pipeline = resources.create_pipeline(description.as_com_ref(), vertex.as_com_ref(), fragment.as_com_ref());
            if requested & !permission == 0 {
                assert!(pipeline.is_ok(), "permissions={permission}, requested={requested}");
            } else {
                assert!(matches!(pipeline, Err(RENDER_E_PERMISSION)), "permissions={permission}, requested={requested}");
            }
        }
    }
}

/// Several groups may select one image, while another image or native device cannot substitute for it.
#[test]
fn target_identity_group_ownership_and_closure_are_independent() {
    let device = device();
    let image = device.surface_target(RenderSurfaceKind::Image).unwrap();
    let other_image = device.surface_target(RenderSurfaceKind::Image).unwrap();
    let (mut owner, resources) = group(&device, image.clone(), RenderClientPermissions::none());
    let (_second, _resources) = group(&device, image.clone(), RenderClientPermissions::none());
    let mut frame = device.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(root(), resources.as_com_ref()).unwrap();
    session.view().unwrap().finish().unwrap();
    let owner_block = session.take_block().unwrap();
    let block = device.block_snapshot(owner_block.as_com_ref()).unwrap();
    frame.finish().unwrap();
    assert_eq!(block.admit_target(&image), Ok(()));
    assert_eq!(block.admit_target(&other_image), Err(RENDER_E_OWNER));
    assert_eq!(block.admit_framebuffer(), Err(RENDER_E_OWNER));
    let foreign = testing::device(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    assert!(matches!(
        foreign.create_client(Default::default(), image.clone(), RenderClientPermissions::none()),
        Err(RENDER_E_OWNER)
    ));
    owner.close().unwrap();
    assert_eq!(block.admit_target(&image), Err(neocogi_com::NC_E_DROPPED));
}

/// Existing native image import is explicitly delegated, regardless of which target was selected.
#[test]
fn image_import_permission_is_independent_of_target_and_resource_update_permission() {
    let device = device();
    let image = testing::Image::ordinary(device.inner.core.clone(), Size { width: 1, height: 1 }, 7);
    for kind in [RenderSurfaceKind::Framebuffer, RenderSurfaceKind::Image] {
        for allow in [false, true] {
            let target = device.surface_target(kind).unwrap();
            let (_owner, resources) = group(
                &device,
                target,
                RenderClientPermissions {
                    import_images: allow,
                    ..RenderClientPermissions::none()
                },
            );
            let result = device.import_texture(resources.as_com_ref(), image.clone());
            if allow {
                let imported = result.unwrap();
                assert!(device.imported_texture_snapshot(resources.as_com_ref(), imported.as_com_ref()).is_ok());
            } else {
                assert!(matches!(result, Err(RENDER_E_PERMISSION)));
            }
        }
    }
}
