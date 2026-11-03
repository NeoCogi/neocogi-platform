//! Native COM acceptance for explicit target selection and independently permissioned groups.
use super::*;

#[com_object(interfaces(IRenderStream))]
/// Owns complete sealed contributions until the synchronous renderer consumes their snapshots.
struct Blocks {
    /// Every block was recorded through the production broker and its selected resource group.
    blocks: Vec<ComBox<IRenderViewBlockAbi>>,
}
impl IRenderStream for Blocks {
    fn view(&self, _: u64, index: u32) -> Result<neocogi_com::WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        self.blocks.get(index as usize).map(ComBox::downgrade).ok_or(NC_E_INVALID_ARG)
    }
}

/// No depth permission is needed for composing color into either kind of surface.
fn color_permissions() -> RenderClientPermissions {
    RenderClientPermissions {
        clear_color: true,
        write_color: true,
        write_resources: true,
        import_images: true,
        ..RenderClientPermissions::none()
    }
}

/// Construct through the actual platform, preserving the production native device and public identities.
fn window() -> crate::testing::NativeWindow {
    crate::testing::window(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
        "explicit surface target",
        SIZE,
    )
    .unwrap()
}

#[cfg(feature = "glow")]
/// Acceptance coordinates are top-left; the GL readback rows are bottom-up.
fn pixel(probe: &crate::testing::NativeProbe, x: usize, y: usize) -> [u8; 4] {
    let pixels = probe.pixels().unwrap();
    let offset = ((SIZE.height as usize - 1 - y) * SIZE.width as usize + x) * 4;
    pixels[offset..offset + 4].try_into().unwrap()
}

/// Record a regional clear. Its group grants clearing separately from shader writes or depth access.
fn clear_block(host: &ComPtr<IRenderHostAbi>, resources: &ComPtr<IRenderResourcesAbi>, bounds: Rect, rgba: u32) -> ComBox<IRenderViewBlockAbi> {
    let mut frame = host.recording_broker().unwrap().begin_frame().unwrap();
    let mut session = frame
        .begin_view(RenderViewRoot { client: bounds, target: bounds }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.clear_color(rgba).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    block
}

/// Exact stream owner used by both image submission and the enclosing window frame.
fn stream(blocks: Vec<ComBox<IRenderViewBlockAbi>>) -> ComPtr<IRenderStreamAbi> {
    Blocks::from_value(Blocks { blocks }).query().unwrap()
}

/// Import a completed image using the group's explicit permission, then sample it through ordinary shaders.
fn image_block(host: &ComPtr<IRenderHostAbi>, resources: &ComPtr<IRenderResourcesAbi>, image: &ComPtr<ITextureAbi>) -> ComBox<IRenderViewBlockAbi> {
    let imported = host.import_texture(resources.as_com_ref(), image.as_com_ref()).unwrap();
    let sampler = resources.create_sampler(GpuSamplerDesc::default()).unwrap();
    let source = Bytes::source(
        br#"
        @group(2) @binding(0) var image: texture_2d<f32>;
        @group(2) @binding(1) var filtering: sampler;
        @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
            let p = array<vec2<f32>,3>(vec2(-1.0,-1.0), vec2(3.0,-1.0), vec2(-1.0,3.0));
            return vec4(p[i],0.5,1.0);
        }
        @fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
            return textureSample(image,filtering,p.xy/vec2(64.0,64.0));
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
    let description = PipelineDesc {
        bindings: vec![GpuShaderBinding {
            stage: GpuShaderStage::Fragment,
            slot: 0,
            kind: GpuBindingKind::SampledTexture,
            byte_len: 0,
        }],
        ..PipelineDesc::default()
    }
    .source()
    .unwrap();
    let pipeline = resources.create_pipeline(description.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let mut frame = host.recording_broker().unwrap().begin_frame().unwrap();
    let bounds = Rect { x: 0, y: 0, width: 64, height: 64 };
    let mut session = frame
        .begin_view(RenderViewRoot { client: bounds, target: bounds }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.bind_texture(GpuShaderStage::Fragment, 0, imported.as_com_ref(), sampler.as_com_ref())
        .unwrap();
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    block
}

/// Current-framebuffer views share the parent's target and cannot independently acquire/present a frame.
#[test]
#[ignore = "requires a native display; GL also supports SDL_VIDEODRIVER=offscreen"]
fn framebuffer_views_and_image_targets_select_actual_surfaces_without_ui_roles() {
    let _session = native_session();
    let native = window();
    let window = native.window;
    let probe = native.probe;
    let host = window.graphics_host().unwrap();
    let surfaces: ComPtr<IRenderSurfaceFactoryAbi> = window.query().unwrap();
    let groups: ComPtr<IRenderClientFactoryAbi> = window.query().unwrap();
    let renderer: ComPtr<IRendererAbi> = window.query().unwrap();
    let framebuffer = surfaces
        .create_surface(RenderSurfaceDesc {
            kind: RenderSurfaceKind::Framebuffer,
            size: Size::default(),
        })
        .unwrap();
    assert_eq!(framebuffer.kind().unwrap(), RenderSurfaceKind::Framebuffer);
    assert_eq!(framebuffer.size().unwrap(), window.size().unwrap());
    assert!(
        framebuffer.query::<IRendererAbi>().is_err(),
        "the view has no independent frame/presentation authority"
    );
    assert!(matches!(framebuffer.completed_image(), Err(neocogi_com::NC_E_NOT_FOUND)));
    assert_eq!(framebuffer.resize(SIZE), Err(NC_E_INVALID_ARG));
    assert!(
        surfaces
            .create_surface(RenderSurfaceDesc {
                kind: RenderSurfaceKind::Framebuffer,
                size: SIZE
            })
            .is_err()
    );
    let first = groups.create_client(Default::default(), framebuffer.as_com_ref(), color_permissions()).unwrap();
    let second = groups.create_client(Default::default(), framebuffer.as_com_ref(), color_permissions()).unwrap();
    let blocks = stream(vec![
        clear_block(&host, &first.resources().unwrap(), Rect { x: 0, y: 0, width: 32, height: 64 }, 0xff0000ff),
        clear_block(&host, &second.resources().unwrap(), Rect { x: 8, y: 8, width: 16, height: 16 }, 0xffff0000),
    ]);
    renderer
        .render(
            RenderInfo {
                size: SIZE,
                clear: 0xff00ff00,
                invocation: 1,
                block_count: 2,
            },
            blocks.as_com_ref(),
        )
        .unwrap();
    #[cfg(feature = "glow")]
    {
        assert_eq!(pixel(&probe, 4, 32), [255, 0, 0, 255]);
        assert_eq!(pixel(&probe, 12, 12), [0, 0, 255, 255]);
        assert_eq!(
            pixel(&probe, 50, 32),
            [0, 255, 0, 255],
            "view clears preserve the rest of the enclosing framebuffer"
        );
    }
    let key = identity(&framebuffer).unwrap();
    drop(framebuffer);
    assert!(
        !probe.contains_surface(key),
        "group target metadata must not keep a released surface address registered"
    );

    let extent = Size { width: 32, height: 32 };
    let image = surfaces
        .create_surface(RenderSurfaceDesc {
            kind: RenderSurfaceKind::Image,
            size: extent,
        })
        .unwrap();
    assert_eq!(image.kind().unwrap(), RenderSurfaceKind::Image);
    assert!(matches!(image.completed_image(), Err(neocogi_com::NC_E_NOT_FOUND)));
    let image_renderer: ComPtr<IRendererAbi> = image.query().unwrap();
    let first_image_group = groups.create_client(Default::default(), image.as_com_ref(), color_permissions()).unwrap();
    let second_image_group = groups.create_client(Default::default(), image.as_com_ref(), color_permissions()).unwrap();
    let image_blocks = stream(vec![
        clear_block(
            &host,
            &first_image_group.resources().unwrap(),
            Rect { x: 0, y: 0, width: 16, height: 32 },
            0xff0000ff,
        ),
        clear_block(
            &host,
            &second_image_group.resources().unwrap(),
            Rect { x: 4, y: 4, width: 8, height: 8 },
            0xffff0000,
        ),
    ]);
    image_renderer
        .render(
            RenderInfo {
                size: extent,
                clear: 0xff00ff00,
                invocation: 2,
                block_count: 2,
            },
            image_blocks.as_com_ref(),
        )
        .unwrap();
    let old = image.completed_image().unwrap();
    let old_identity = identity(&old).unwrap();
    // A group cannot substitute another image merely because its size and device match.
    let other = surfaces
        .create_surface(RenderSurfaceDesc {
            kind: RenderSurfaceKind::Image,
            size: extent,
        })
        .unwrap();
    let other_renderer: ComPtr<IRendererAbi> = other.query().unwrap();
    assert_eq!(
        other_renderer.render(
            RenderInfo {
                size: extent,
                clear: 0xff000000,
                invocation: 3,
                block_count: 2
            },
            image_blocks.as_com_ref()
        ),
        Err(RENDER_E_OWNER)
    );
    assert!(matches!(other.completed_image(), Err(neocogi_com::NC_E_NOT_FOUND)));
    assert_eq!(
        renderer.render(
            RenderInfo {
                size: SIZE,
                clear: 0xff000000,
                invocation: 4,
                block_count: 2
            },
            image_blocks.as_com_ref()
        ),
        Err(RENDER_E_OWNER)
    );
    image_renderer
        .render(
            RenderInfo {
                size: extent,
                clear: 0xff00ffff,
                invocation: 5,
                block_count: 0,
            },
            stream(Vec::new()).as_com_ref(),
        )
        .unwrap();
    assert_ne!(old_identity, identity(&image.completed_image().unwrap()).unwrap());
    assert_eq!(old.size().unwrap(), extent);
    let picture = stream(vec![image_block(&host, &first.resources().unwrap(), &old)]);
    renderer
        .render(
            RenderInfo {
                size: SIZE,
                clear: 0xff000000,
                invocation: 6,
                block_count: 1,
            },
            picture.as_com_ref(),
        )
        .unwrap();
    #[cfg(feature = "glow")]
    {
        assert_eq!(pixel(&probe, 4, 32), [255, 0, 0, 255]);
        assert_eq!(pixel(&probe, 12, 12), [0, 0, 255, 255]);
        assert_eq!(
            pixel(&probe, 50, 32),
            [0, 255, 0, 255],
            "the previous immutable image survives a later yellow image render"
        );
    }
    image.resize(Size { width: 16, height: 16 }).unwrap();
    assert_eq!(old.size().unwrap(), extent);
    assert_eq!(image.completed_image().unwrap().size().unwrap(), extent);
}
