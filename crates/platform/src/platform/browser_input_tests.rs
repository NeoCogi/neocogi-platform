//! Browser-executed DOM regressions exported from the wasm library test artifact.
//!
//! The final neocogi-browser-tests cdylib owns the public JavaScript export and
//! single host. `cargo tools test-browser` builds it, generates matching bindings,
//! serves the checked-in page and requires actual browser PASS.

use super::*;
use crate::platform::{PlatformObject, WindowObject, identity};
use neocogi_com_sdk::{ComPtr, NC_E_INVALID_ARG, com_object};
use neocogi_platform::render::abi::*;
use std::cell::Cell;
use wasm_bindgen::prelude::*;

use neocogi_platform::gpu::testing::{derivatives, sampled};

#[path = "browser_geometry_tests.rs"]
mod geometry_tests;

#[com_object(interfaces(IRenderStream))]
/// Browser-owned sealed contributions exercise the same generated COM path as native applications.
struct SurfaceBlocks {
    /// Exclusive owners remain alive through each synchronous image/window submission.
    blocks: Vec<neocogi_com_sdk::ComBox<IRenderViewBlockAbi>>,
}
impl IRenderStream for SurfaceBlocks {
    /// Borrows a sealed contribution without transferring its owner to the renderer.
    fn view(&self, _: u64, index: u32) -> Result<neocogi_com_sdk::WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        self.blocks.get(index as usize).map(neocogi_com_sdk::ComBox::downgrade).ok_or(NC_E_INVALID_ARG)
    }
}

/// Records a regional color clear with depth disabled; no automatic depth command may be required.
fn surface_clear_block(
    host: &ComPtr<IRenderHostAbi>,
    group: &neocogi_com_sdk::ComBox<IRenderResourceGroupAbi>,
) -> neocogi_com_sdk::ComBox<IRenderViewBlockAbi> {
    let bounds = Rect { x: 0, y: 0, width: 4, height: 4 };
    let mut frame = host.recording_broker().unwrap().begin_frame().unwrap();
    let resources = group.resources().unwrap();
    let mut session = frame
        .begin_view(RenderViewRoot { local: bounds, target: bounds }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.clear_color(0xff0000ff).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    block
}

/// Wraps one regional clear block for a normal synchronous submission.
fn surface_clear(host: &ComPtr<IRenderHostAbi>, group: &neocogi_com_sdk::ComBox<IRenderResourceGroupAbi>) -> ComPtr<IRenderStreamAbi> {
    SurfaceBlocks::from_value(SurfaceBlocks {
        blocks: vec![surface_clear_block(host, group)],
    })
    .query()
    .unwrap()
}

/// Actual WebGL COM submission distinguishes framebuffer views from exact image targets.
/// Pixel readback verifies that a view's regional clear preserves the rest of the parent frame.
fn browser_surface_targets() {
    use glow::HasContext;
    let size = Size { width: 8, height: 8 };
    let platform = PlatformObject::new();
    let object = platform.window("browser target contract", size).unwrap();
    object.device.window.style().set_css_text("width:8px;height:8px");
    let gl = object.device.access().unwrap().state().gpu.core().dispatch().clone();
    let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(object).query().unwrap();
    let drawable = window.size().unwrap();
    let host = window.graphics_host().unwrap();
    let groups: ComPtr<IRenderResourceGroupFactoryAbi> = host.query().unwrap();
    let surfaces: ComPtr<IRenderSurfaceFactoryAbi> = host.query().unwrap();
    assert!(host.query::<IRendererAbi>().is_err());
    let permissions = RenderGroupPermissions {
        clear_color: true,
        write_color: true,
        write_resources: true,
        import_images: true,
        ..RenderGroupPermissions::none()
    };
    let framebuffer = surfaces.create_surface(RenderSurfaceDesc::Framebuffer).unwrap();
    assert_eq!(framebuffer.size().unwrap(), drawable);
    assert!(framebuffer.query::<IRendererAbi>().is_err());
    let window_group = groups.create_group(Default::default(), framebuffer.as_com_ref(), permissions).unwrap();
    let image = surfaces.create_surface(RenderSurfaceDesc::Image(size)).unwrap();
    let image_group = groups.create_group(Default::default(), image.as_com_ref(), permissions).unwrap();
    let image_renderer: ComPtr<IRendererAbi> = image.query().unwrap();
    let image_stream = surface_clear(&host, &image_group);
    let info = RenderInfo {
        size,
        clear: 0xff00ff00,
        invocation: 1,
        block_count: 1,
    };
    image_renderer.render(info, image_stream.as_com_ref()).map_err(|error| error.status).unwrap();
    let old = image.completed_image().unwrap();
    let other = surfaces.create_surface(RenderSurfaceDesc::Image(size)).unwrap();
    assert_eq!(
        other
            .query::<IRendererAbi>()
            .unwrap()
            .render(info, image_stream.as_com_ref())
            .map_err(|error| error.status),
        Err(RENDER_E_OWNER)
    );
    assert!(matches!(other.completed_image(), Err(neocogi_com_sdk::NC_E_NOT_FOUND)));
    let renderer: ComPtr<IRendererAbi> = window.query().unwrap();
    assert_eq!(
        renderer
            .render(RenderInfo { size: drawable, ..info }, image_stream.as_com_ref())
            .map_err(|error| error.status),
        Err(RENDER_E_OWNER)
    );
    renderer
        .render(RenderInfo { size: drawable, ..info }, surface_clear(&host, &window_group).as_com_ref())
        .map_err(|error| error.status)
        .unwrap();
    let mut inside = [0u8; 4];
    let mut outside = [0u8; 4];
    // SAFETY: The live window retains this WebGL context and framebuffer; both outputs are initialized
    // RGBA storage, and coordinates are within the size just accepted by the production renderer.
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
        gl.read_pixels(
            1,
            drawable.height - 2,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut inside)),
        );
        gl.read_pixels(
            drawable.width - 1,
            0,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut outside)),
        );
        assert_eq!(gl.get_error(), glow::NO_ERROR);
    }
    assert_eq!(inside, [255, 0, 0, 255]);
    assert_eq!(outside, [0, 255, 0, 255]);
    let empty: ComPtr<IRenderStreamAbi> = SurfaceBlocks::from_value(SurfaceBlocks { blocks: vec![] }).query().unwrap();
    image_renderer
        .render(
            RenderInfo {
                block_count: 0,
                clear: 0xff00ffff,
                ..info
            },
            empty.as_com_ref(),
        )
        .map_err(|error| error.status)
        .unwrap();
    assert_ne!(identity(&old).unwrap(), identity(&image.completed_image().unwrap()).unwrap());
    let resources = window_group.resources().unwrap();
    let imported = host.import_texture(resources.as_com_ref(), old.as_com_ref()).unwrap();
    host.prepare_texture(resources.as_com_ref(), imported.as_com_ref()).unwrap();
}

/// The Chromium software adapter clamps a tall drawing buffer below its
/// requested attributes. Check admission and input against that actual allocation.
fn clamped_drawing_buffer_uses_physical_coordinates() {
    use glow::HasContext;
    let object = PlatformObject::new().window("clamped drawing buffer", Size { width: 64, height: 64 }).unwrap();
    let canvas = object.device.window.clone();
    let text = object.input.text.clone();
    let core = object.device.drawable.clone();
    let gl = core.dispatch();
    let mut limits = [0; 2];
    // SAFETY: This test reads the viewport limits of the live owning context.
    unsafe { gl.get_parameter_i32_slice(glow::MAX_VIEWPORT_DIMS, &mut limits) };
    let content_height = limits[1] + 8;
    canvas
        .style()
        .set_css_text(&format!("width:64px;height:{content_height}px;border:3px solid black;padding:9px"));
    let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(object).query().unwrap();
    let actual = core.drawable_size().unwrap();
    let size = window.size().unwrap();
    assert_eq!(size, core.drawable_size().unwrap());
    assert_ne!(size, actual);
    let requested = Size {
        width: canvas.width() as i32,
        height: canvas.height() as i32,
    };
    assert!(requested.height > size.height);
    let surfaces: ComPtr<IRenderSurfaceFactoryAbi> = window.graphics_host().unwrap().query().unwrap();
    assert_eq!(surfaces.create_surface(RenderSurfaceDesc::Framebuffer).unwrap().size().unwrap(), size);

    let renderer: ComPtr<IRendererAbi> = window.query().unwrap();
    let stream: ComPtr<IRenderStreamAbi> = SurfaceBlocks::from_value(SurfaceBlocks { blocks: Vec::new() }).query().unwrap();
    let info = RenderInfo {
        size,
        clear: 0xffff0000,
        invocation: 1,
        block_count: 0,
    };
    assert_eq!(
        renderer.render(RenderInfo { size: requested, ..info }, stream.as_com_ref()).unwrap_err().status,
        NC_E_INVALID_ARG
    );
    renderer.render(info, stream.as_com_ref()).unwrap();
    let mut top = [0; 4];
    // SAFETY: The renderer just cleared this live drawing buffer. The actual
    // top row is inside its reported extent and fits this one-pixel output.
    unsafe {
        gl.read_pixels(
            0,
            size.height - 1,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut top)),
        );
        assert_eq!(gl.get_error(), glow::NO_ERROR);
    }
    assert_eq!(top, [0, 0, 255, 255]);

    let observations = Rc::new(RefCell::new(Vec::new()));
    let sink = ShortcutSink::from_value(ShortcutSink { received: observations.clone() })
        .query::<IInputSinkAbi>()
        .unwrap();
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    observations.borrow_mut().clear();
    let bounds = canvas.get_bounding_client_rect();
    let pointer_y = (bounds.top() + bounds.height() * 0.75).round();
    let content_top = bounds.top() + 12.0;
    let scale = f64::from(size.height) / f64::from(content_height);
    exercise(canvas.unchecked_ref(), "scaled-drawable");
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    assert!(matches!(observations.borrow().as_slice(), [
        (UiInput::Pointer(Point { y, .. }), _), (UiInput::Scroll(Point { x: 0, y: scroll }), _),
    ] if *y == ((pointer_y - content_top) * scale).round() as i32 && *scroll == size.height));

    canvas.focus().unwrap();
    let height = size.height / 2;
    window.text_input(TextInputArea::Area(Rect { x: 0, y: 0, width: 1, height })).unwrap();
    let applied = text.style().get_property_value("height").unwrap();
    let applied = applied.trim_end_matches("px").parse::<f64>().unwrap();
    assert!((applied - f64::from(height) / scale).abs() < 0.02);
    let top = text.style().get_property_value("top").unwrap();
    assert!((top.trim_end_matches("px").parse::<f64>().unwrap() - content_top).abs() < 0.02);
}

#[com_object(interfaces(IRenderStream))]
/// Real stream callback that loses its own drawable after renderer state is detached.
struct LosingStream {
    /// Context created for the same window whose submission is invoking this callback.
    context: web_sys::WebGl2RenderingContext,
    /// One sealed contribution, returned after the loss to reach final admission.
    block: neocogi_com_sdk::ComBox<IRenderViewBlockAbi>,
}

impl IRenderStream for LosingStream {
    /// Lose the live context while still supplying the authenticated contribution.
    fn view(&self, _: u64, index: u32) -> Result<neocogi_com_sdk::WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        assert_eq!(index, 0);
        lose_test_context(&self.context);
        assert!(self.context.is_context_lost());
        Ok(self.block.downgrade())
    }
}

/// Context loss reaches both final admission and preflight as an owned renderer error.
fn drawable_loss_keeps_render_diagnostics() {
    let object = PlatformObject::new().window("lost drawing buffer", Size { width: 8, height: 8 }).unwrap();
    object.device.window.style().set_css_text("width:8px;height:8px");
    let context = object
        .device
        .window
        .get_context("webgl2")
        .unwrap()
        .unwrap()
        .dyn_into::<web_sys::WebGl2RenderingContext>()
        .unwrap();
    let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(object).query().unwrap();
    let size = window.size().unwrap();
    let host = window.graphics_host().unwrap();
    let surfaces: ComPtr<IRenderSurfaceFactoryAbi> = host.query().unwrap();
    let framebuffer = surfaces.create_surface(RenderSurfaceDesc::Framebuffer).unwrap();
    let groups: ComPtr<IRenderResourceGroupFactoryAbi> = host.query().unwrap();
    let group = groups
        .create_group(
            Default::default(),
            framebuffer.as_com_ref(),
            RenderGroupPermissions {
                clear_color: true,
                ..RenderGroupPermissions::none()
            },
        )
        .unwrap();
    let block = surface_clear_block(&host, &group);
    let stream: ComPtr<IRenderStreamAbi> = LosingStream::from_value(LosingStream { context, block }).query().unwrap();
    let renderer: ComPtr<IRendererAbi> = window.query().unwrap();
    let info = RenderInfo {
        size,
        invocation: 1,
        clear: 0,
        block_count: 1,
    };
    let post_callback = renderer.render(info, stream.as_com_ref()).unwrap_err();
    assert_eq!(window.size(), Err(PLATFORM_E_BACKEND));
    assert_eq!(framebuffer.size(), Err(PLATFORM_E_BACKEND));
    let preflight = renderer.render(RenderInfo { invocation: 2, ..info }, stream.as_com_ref()).unwrap_err();
    assert_eq!(post_callback.status, RENDER_E_BACKEND);
    assert_eq!(preflight.status, RENDER_E_BACKEND);
    assert!(post_callback.message.contains("WebGL context was lost; recreate the driver and its resources"));
    assert_eq!(post_callback.message, preflight.message);
}

/// A native restoration between SDK calls ends the old COM driver's authority.
async fn restoration_requires_a_fresh_driver() {
    let object = PlatformObject::new().window("WebGL generation", Size { width: 8, height: 8 }).unwrap();
    let canvas = object.device.window.clone();
    canvas.style().set_css_text("width:8px;height:8px");
    let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(object).query().unwrap();
    let renderer: ComPtr<IRendererAbi> = window.query().unwrap();
    let surfaces: ComPtr<IRenderSurfaceFactoryAbi> = window.query().unwrap();
    let framebuffer = surfaces.create_surface(RenderSurfaceDesc::Framebuffer).unwrap();
    let bound: ComPtr<IRenderDriverBoundAbi> = renderer.query().unwrap();
    let original = bound.driver_identity().unwrap();
    let stream: ComPtr<IRenderStreamAbi> = SurfaceBlocks::from_value(SurfaceBlocks { blocks: Vec::new() }).query().unwrap();
    let info = RenderInfo {
        invocation: 1,
        size: window.size().unwrap(),
        clear: 0xff123456,
        block_count: 0,
    };
    renderer.render(info, stream.as_com_ref()).unwrap();

    // The page opts into restoration. No Platform method can observe the intermediate loss.
    wasm_bindgen_futures::JsFuture::from(restore_test_context(&canvas, false)).await.unwrap();
    let attributes = (canvas.width(), canvas.height());
    canvas.style().set_css_text("width:11px;height:13px");
    let failure = renderer.render(RenderInfo { invocation: 2, ..info }, stream.as_com_ref()).unwrap_err();
    assert_eq!(failure.status, RENDER_E_BACKEND);
    assert!(failure.message.contains("WebGL context was lost"));
    assert_eq!((canvas.width(), canvas.height()), attributes, "the stale renderer must not reset the canvas");
    assert_eq!(window.size(), Err(PLATFORM_E_BACKEND));
    assert_eq!(framebuffer.size(), Err(PLATFORM_E_BACKEND));
    assert_eq!(identity(&original).unwrap(), identity(&bound.driver_identity().unwrap()).unwrap());
    assert_eq!((canvas.width(), canvas.height()), attributes, "the stale window must not resize the canvas");

    let mut replacement = gpu::Gpu::from_canvas(canvas).unwrap();
    let core = replacement.core();
    assert_ne!(identity(&original).unwrap(), identity(core.identity()).unwrap());
    assert_eq!(core.drawable_size().unwrap(), info.size);
    replacement.render(info, &crate::platform::Prepared::default()).unwrap();
    assert_eq!(window.size(), Err(PLATFORM_E_BACKEND));
}

/// Explicit finish preserves a primary error while latching loss during a real sync observation.
async fn explicit_finish_latches_webgl_loss() {
    use crate::gpu::backends::webgl::{Core, WebGlCanvasRenderer};
    use crate::render::{FrameInfo, RendererBackend, RendererFrame, prelude::{Dimensioni, color}};

    let document = web_sys::window().unwrap().document().unwrap();
    let canvas = document.create_element("canvas").unwrap().dyn_into::<HtmlCanvasElement>().unwrap();
    canvas.set_width(8);
    canvas.set_height(8);
    let native = canvas.get_context("webgl2").unwrap().unwrap().dyn_into::<web_sys::WebGl2RenderingContext>().unwrap();
    let original = Core::from_canvas(canvas.clone(), native.clone()).unwrap();
    let mut renderer = WebGlCanvasRenderer::new(original.clone(), 8, 8).unwrap();
    let gpu = crate::gpu::GpuDevice::new(original.clone()).unwrap();
    let group = gpu
        .create_group(
            Default::default(),
            gpu.surface_target(RenderSurfaceKind::Framebuffer).unwrap(),
            RenderGroupPermissions {
                write_color: true,
                ..RenderGroupPermissions::none()
            },
        )
        .unwrap();
    let case = derivatives::cases().into_iter().next().unwrap();
    let owner = derivatives::block(&gpu, &group.resources().unwrap(), &case.source);
    let prepared = renderer.prepare_block(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), false).unwrap();
    let size = Size { width: 8, height: 8 };
    let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap();
    let output = renderer.create_render_texture(size).unwrap();
    let mut frame = renderer.texture_frame(info, output).unwrap();
    frame.execute_block(&prepared);
    let restored = restore_test_context(&canvas, true);
    assert_eq!(frame.finish().err().unwrap().to_string(), "prepared GL block belongs to another driver or target");
    assert!(native.is_context_lost(), "explicit finish must have reached the sync wait");
    wasm_bindgen_futures::JsFuture::from(restored).await.unwrap();
    let loss = original.current().unwrap_err();
    assert!(loss.contains("0x9242"), "finish must preserve the native loss diagnostic before the event: {loss}");

    let replacement = Core::from_canvas(canvas, native).unwrap();
    assert_ne!(identity(original.identity()).unwrap(), identity(replacement.identity()).unwrap());
    let mut renderer = WebGlCanvasRenderer::new(replacement, 8, 8).unwrap();
    let output = renderer.create_render_texture(size).unwrap();
    assert!(renderer.texture_frame(info, output).unwrap().finish().unwrap().is_some());
}

#[com_object(interfaces(IGpuDataSource))]
/// Supplies valid static contents and loses the browser context in the final header callback.
struct LosingBufferSource {
    /// Only this exact test window's browser context is changed.
    context: web_sys::WebGl2RenderingContext,
    /// Counts the initial header and the consistency check at the end of the bulk read.
    headers: Rc<Cell<u32>>,
}
impl IGpuDataSource for LosingBufferSource {
    fn byte_count(&self) -> Result<u64, NcStatus> {
        let previous = self.headers.replace(self.headers.get() + 1);
        if previous == 1 {
            lose_test_context(&self.context);
            assert!(self.context.is_context_lost());
        }
        Ok(4)
    }
    fn read(&self, mut output: neocogi_com_sdk::NcOutputWriter<'_, u8>) -> Result<(), NcStatus> {
        assert_eq!(output.offset(), 0);
        output.extend_from_slice(&[1, 2, 3, 4])
    }
}

/// Public buffer creation/import rejects observed loss before publication and refunds failed reservations.
fn buffer_publication_requires_live_storage() {
    let size = Size { width: 1, height: 1 };
    let object = PlatformObject::new().window("WebGL buffer publication", size).unwrap();
    let device = object.device.clone();
    let context = device
        .window
        .get_context("webgl2")
        .unwrap()
        .unwrap()
        .dyn_into::<web_sys::WebGl2RenderingContext>()
        .unwrap();
    let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(object).query().unwrap();
    let images: ComPtr<ITextureFactoryAbi> = window.query().unwrap();
    let image = images
        .create_texture(size, neocogi_platform::render::source::initial(None).borrow().unwrap())
        .unwrap();
    let host = window.graphics_host().unwrap();
    let groups: ComPtr<IRenderResourceGroupFactoryAbi> = host.query().unwrap();
    let surfaces: ComPtr<IRenderSurfaceFactoryAbi> = host.query().unwrap();
    let framebuffer = surfaces.create_surface(RenderSurfaceDesc::Framebuffer).unwrap();
    let limits = RenderGroupLimits { resources: 1, ..Default::default() };
    let owner = groups
        .create_group(
            limits,
            framebuffer.as_com_ref(),
            RenderGroupPermissions {
                import_images: true,
                ..RenderGroupPermissions::none()
            },
        )
        .unwrap();
    let denied = groups.create_group(limits, framebuffer.as_com_ref(), RenderGroupPermissions::none()).unwrap();
    let resources = owner.resources().unwrap();
    let denied_resources = denied.resources().unwrap();
    let account = device.logical.domain(resources.as_com_ref()).unwrap().account.clone();
    let headers = Rc::new(Cell::new(0));
    let source: ComPtr<IGpuDataSourceAbi> = LosingBufferSource::from_value(LosingBufferSource { context, headers: headers.clone() })
        .query()
        .unwrap();
    let desc = GpuBufferDesc::Vertex { byte_len: 4 };
    assert!(matches!(resources.create_static_buffer(desc, source.as_com_ref()), Err(RENDER_E_BACKEND)));
    assert_eq!(headers.get(), 2, "the final header caused context loss");
    assert!(matches!(resources.create_static_buffer(desc, source.as_com_ref()), Err(RENDER_E_BACKEND)));
    assert_eq!(headers.get(), 2, "known loss stops before calling the source");
    assert!(matches!(resources.create_dynamic_buffer(desc), Err(RENDER_E_BACKEND)));
    assert!(matches!(resources.create_streamed_buffer(desc), Err(RENDER_E_BACKEND)));
    assert!(matches!(
        resources.create_dynamic_buffer(GpuBufferDesc::Vertex { byte_len: 0 }),
        Err(neocogi_com_sdk::NC_E_OUT_OF_RANGE)
    ));
    let failure = host.import_texture(resources.as_com_ref(), image.as_com_ref()).err().unwrap();
    assert_eq!(failure.status, RENDER_E_BACKEND);
    assert!(failure.message.contains("WebGL context was lost"));
    let permission = host.import_texture(denied_resources.as_com_ref(), image.as_com_ref()).err().unwrap();
    assert_eq!(permission.status, RENDER_E_PERMISSION);
    assert_eq!(account.live_bytes(), 0);
    assert!(account.resource().is_ok(), "the only owner slot was refunded");
}

/// Mutable native slots must ignore application unpack addressing on every reuse.
async fn browser_texture_refresh(
    gpu: &crate::gpu::GpuDevice<crate::gpu::backends::webgl::Backend>,
    resources: &ComPtr<IRenderResourcesAbi>,
    renderer: &mut crate::gpu::backends::webgl::WebGlCanvasRenderer,
    gl: &glow::Context,
    native: &web_sys::WebGl2RenderingContext,
) {
    use glow::HasContext;
    use crate::render::{
        FrameInfo, RendererBackend, RendererFrame,
        prelude::{Dimensioni, color},
    };
    use web_sys::WebGl2RenderingContext as WebGl;
    // SAFETY: this private test buffer belongs to the live context and is deleted after all cases.
    let unpack = unsafe { gl.create_buffer().unwrap() };
    for usage in [GpuBufferUsage::Dynamic, GpuBufferUsage::Streamed] {
        let desc = GpuBufferDesc::Texture {
            size: Size { width: 2, height: 2 },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        };
        let mut buffer = if usage == GpuBufferUsage::Dynamic {
            resources.create_dynamic_buffer(desc)
        } else {
            resources.create_streamed_buffer(desc)
        }
        .unwrap();
        let slots = if usage == GpuBufferUsage::Dynamic { 1 } else { 3 };
        let mut names = Vec::new();
        let mut pinned = Vec::new();
        for fill in 1..=8u8 {
            // Unequal rows and partial alpha detect accidental flip/premultiplication as well as bad strides.
            let bytes = [fill, 47, 83, 128, 11, 31, fill, 255, 17, fill, 71, 255, 99, 51, 23, 128];
            // WebGL exposes completed fences only after a browser task boundary. Keep
            // the real in-flight exclusion and wait for ordinary maintenance to retire it.
            for attempt in 0..200 {
                match buffer.upload(neocogi_com_sdk::NcCopyFromBufferOffset::new(0, &bytes)) {
                    Ok(()) => break,
                    Err(crate::render::abi::RENDER_E_BUSY) if attempt < 199 => {
                        wasm_bindgen_futures::JsFuture::from(next_test_task()).await.unwrap();
                        renderer.maintain().unwrap();
                    }
                    Err(error) => panic!("upload did not become writable: {error}"),
                }
            }
            // SAFETY: valid private application state; the small PBO makes an omitted reset reject
            // without exposing a short native CPU pointer to a driver.
            unsafe {
                gl.active_texture(glow::TEXTURE3);
                gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(unpack));
                gl.buffer_data_size(glow::PIXEL_UNPACK_BUFFER, 4, glow::STATIC_DRAW);
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 8);
                gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 3);
                gl.pixel_store_i32(glow::UNPACK_IMAGE_HEIGHT, 5);
                gl.pixel_store_i32(glow::UNPACK_SKIP_PIXELS, 1);
                gl.pixel_store_i32(glow::UNPACK_SKIP_ROWS, 1);
                gl.pixel_store_i32(glow::UNPACK_SKIP_IMAGES, 1);
            }
            native.pixel_storei(WebGl::UNPACK_FLIP_Y_WEBGL, 1);
            native.pixel_storei(WebGl::UNPACK_PREMULTIPLY_ALPHA_WEBGL, 1);
            native.pixel_storei(WebGl::UNPACK_COLORSPACE_CONVERSION_WEBGL, WebGl::BROWSER_DEFAULT_WEBGL as i32);
            assert_eq!(unsafe { gl.get_error() }, glow::NO_ERROR);
            let version = gpu.texture_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
            renderer.prepare_texture(version.clone()).unwrap();
            // SAFETY: query this live context's scalar unpack state after the actual upload.
            unsafe {
                assert_eq!(gl.get_parameter_i32(glow::ACTIVE_TEXTURE) as u32, glow::TEXTURE0);
                for parameter in [
                    glow::UNPACK_ROW_LENGTH,
                    glow::UNPACK_IMAGE_HEIGHT,
                    glow::UNPACK_SKIP_PIXELS,
                    glow::UNPACK_SKIP_ROWS,
                    glow::UNPACK_SKIP_IMAGES,
                ] {
                    assert_eq!(gl.get_parameter_i32(parameter), 0);
                }
                assert!(native.get_parameter(WebGl::PIXEL_UNPACK_BUFFER_BINDING).unwrap().is_null());
                assert_eq!(native.get_parameter(WebGl::UNPACK_FLIP_Y_WEBGL).unwrap().as_bool(), Some(false));
                assert_eq!(native.get_parameter(WebGl::UNPACK_PREMULTIPLY_ALPHA_WEBGL).unwrap().as_bool(), Some(false));
                assert_eq!(
                    native.get_parameter(WebGl::UNPACK_COLORSPACE_CONVERSION_WEBGL).unwrap().as_f64(),
                    Some(f64::from(glow::NONE))
                );
            }
            let root = Rect { x: 0, y: 0, width: 8, height: 8 };
            let owner = sampled::buffer_block(gpu, resources, root, buffer.as_com_ref());
            let block = gpu.block_snapshot(owner.as_com_ref()).unwrap();
            let prepared = renderer.prepare_block(&block, false).unwrap();
            let mut frame = renderer.frame(FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap()).unwrap();
            frame.execute_block(&prepared);
            // SAFETY: the actual sampled draw has bound this initialized texture. The temporary
            // read attachment borrows its live allocation and reads exactly its 2x2 RGBA contents.
            unsafe {
                let texture = gl.get_parameter_texture(glow::TEXTURE_BINDING_2D).unwrap();
                if names.len() < slots {
                    assert!(!names.contains(&texture));
                    names.push(texture);
                    pinned.push(version.clone());
                } else {
                    assert!(names.contains(&texture), "refresh must retain its native texture name");
                }
                let framebuffer = gl.create_framebuffer().unwrap();
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
                gl.framebuffer_texture_2d(glow::READ_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
                assert_eq!(gl.check_framebuffer_status(glow::READ_FRAMEBUFFER), glow::FRAMEBUFFER_COMPLETE);
                let mut pixels = [0; 16];
                gl.read_pixels(0, 0, 2, 2, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixels)));
                assert_eq!(gl.get_error(), glow::NO_ERROR);
                assert_eq!(pixels, bytes, "usage={usage:?}, upload={fill}");
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
                gl.delete_framebuffer(framebuffer);
            }
            frame.finish().unwrap();
            drop((owner, block, prepared));
            drop(version);
            renderer.maintain().unwrap();
            // SAFETY: finish this context's uploads, then retire their in-flight owners before slot reuse.
            unsafe { gl.finish() };
            renderer.maintain().unwrap();
            if pinned.len() == slots {
                pinned.clear();
            }
        }
    }
    // SAFETY: no upload uses this application-side unpack buffer; the backend has unbound it.
    unsafe { gl.delete_buffer(unpack) };
}

/// Exercises actual browser WebGL target adaptation and the delegated safe upload boundary.
async fn canonical_browser_derivatives() {
    use glow::HasContext;
    use neocogi_platform::render::abi::GpuTextureFormat;
    use neocogi_platform::render::{FrameInfo, RendererBackend, RendererFrame};
    use neocogi_platform::render::prelude::{Dimensioni, color};
    let document = web_sys::window().unwrap().document().unwrap();
    let canvas = document.create_element("canvas").unwrap().dyn_into::<HtmlCanvasElement>().unwrap();
    canvas.set_width(8);
    canvas.set_height(8);
    let native = canvas
        .get_context("webgl2")
        .unwrap()
        .unwrap()
        .dyn_into::<web_sys::WebGl2RenderingContext>()
        .unwrap();
    let core = neocogi_platform::gpu::backends::webgl::Core::from_canvas(canvas, native.clone()).unwrap();
    let gl = core.dispatch().clone();
    let mut renderer = neocogi_platform::gpu::backends::webgl::WebGlCanvasRenderer::new(core, 8, 8).unwrap();
    let size = Size { width: 1, height: 1 };
    for len in [0, 1, 3, 5] {
        assert!(matches!(
            renderer.create_texture(size, Some(&vec![0; len])),
            Err(neocogi_platform::render::TextureError::Image { .. })
        ));
    }
    renderer.create_texture(size, Some(&[0; 4])).unwrap();
    let gpu = neocogi_platform::gpu::GpuDevice::new(renderer.core()).unwrap();
    let domain = gpu
        .create_group(
            Default::default(),
            gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderGroupPermissions {
                clear_color: true,
                write_color: true,
                write_resources: true,
                import_images: true,
                ..neocogi_platform::render::abi::RenderGroupPermissions::none()
            },
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    browser_texture_refresh(&gpu, &resources, &mut renderer, &gl, &native).await;
    // Strict WebGL validates the complete native std140 block, unlike lenient native drivers.
    // Exercise the real compiler, COM buffer/pipeline/recording and actual renderer path.
    for usage in [GpuBufferUsage::Static, GpuBufferUsage::Dynamic, GpuBufferUsage::Streamed] {
        for members in 1..=8 {
            for offset in [0, 256] {
                let fixture = crate::gpu::testing::uniforms::fixture(&gpu, &resources, members, offset, usage);
                let block = gpu.block_snapshot(fixture.block.as_com_ref()).unwrap();
                let prepared = renderer.prepare_block(&block, false).unwrap();
                let mut frame = renderer.frame(FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap()).unwrap();
                frame.execute_block(&prepared);
                let mut pixel = [0; 4];
                // SAFETY: the current frame owns this complete 8x8 target and initialized RGBA destination.
                unsafe {
                    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
                    gl.read_pixels(4, 4, 1, 1, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixel)));
                    assert_eq!(gl.get_error(), glow::NO_ERROR);
                }
                assert_eq!(pixel, [255, 0, 0, 255], "members={members}, offset={offset}, usage={usage:?}");
                frame.finish().unwrap();
            }
        }
    }
    for case in derivatives::cases() {
        let owner = derivatives::block(&gpu, &resources, &case.source);
        let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
        for offscreen in [false, true] {
            let prepared = renderer.prepare_block(&snapshot, offscreen).unwrap();
            let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap();
            let mut frame = if offscreen {
                let output = renderer.create_render_texture(Size { width: 8, height: 8 }).unwrap();
                renderer.texture_frame(info, output).unwrap()
            } else {
                renderer.frame(info).unwrap()
            };
            frame.execute_block(&prepared);
            let mut pixel = [0; 4];
            // SAFETY: current WebGL context/target remain live; pixel is initialized writable RGBA.
            unsafe {
                // Glow WebGL cannot map a null getParameter result to a tracked resource.
                let target = if offscreen {
                    gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING)
                } else {
                    None
                };
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, target);
                gl.read_pixels(4, 4, 1, 1, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixel)));
                assert_eq!(gl.get_error(), glow::NO_ERROR);
            }
            frame.finish().unwrap();
            assert!(
                (i32::from(pixel[0]) - i32::from(case.red)).abs() <= 1,
                "{} offscreen={offscreen}: {pixel:?}",
                case.name
            );
        }
    }
    // Strict WebGL links the same aggregate corpus as native GLES3. Read every
    // texel from an RGBA target so zero bounds results include their alpha.
    for case in crate::gpu::testing::optimization::cases() {
        let owner = crate::gpu::testing::optimization::block(&gpu, &resources, &case);
        let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
        let prepared = renderer.prepare_block(&snapshot, true).unwrap();
        let output = renderer.create_render_texture(Size { width: 8, height: 8 }).unwrap();
        let mut frame = renderer
            .texture_frame(FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap(), output)
            .unwrap();
        frame.execute_block(&prepared);
        let mut pixels = [0; 8 * 8 * 4];
        // SAFETY: This live context owns the bound 8x8 RGBA framebuffer, and the
        // complete initialized destination has exactly the required byte extent.
        unsafe {
            let target = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, target);
            gl.read_pixels(0, 0, 8, 8, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixels)));
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        }
        frame.finish().unwrap();
        crate::gpu::testing::optimization::assert_pixels(&case, &pixels);
    }
    // Real ES300 image reads exercise the same guarded compiler artifacts as native GL.
    for case in sampled::load_cases() {
        let owner = sampled::load_block(&gpu, &resources, &case);
        let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
        let prepared = renderer.prepare_block(&snapshot, true).unwrap();
        let output = renderer.create_render_texture(Size { width: 8, height: 8 }).unwrap();
        let mut frame = renderer
            .texture_frame(FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap(), output)
            .unwrap();
        frame.execute_block(&prepared);
        let mut actual = [0; 4];
        // SAFETY: The current WebGL framebuffer and initialized RGBA output remain live.
        unsafe {
            let target = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, target);
            gl.read_pixels(4, 4, 1, 1, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut actual)));
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        }
        frame.finish().unwrap();
        let expected: [u8; 4] = if case.outside { [0; 4] } else { [191, 63, 127, 255] };
        assert!(
            actual.into_iter().zip(expected).all(|(a, b)| (i32::from(a) - i32::from(b)).abs() <= 1),
            "{}: {actual:?}",
            case.name
        );
    }
    // Upload and defined-zero storage cross the same actual WebGL fetch boundary.
    let white = sampled::block(
        &gpu,
        &resources,
        Rect { x: 0, y: 0, width: 8, height: 8 },
        Size { width: 1, height: 1 },
        &[255; 4],
    );
    let zero = sampled::zero_storage_block(&gpu, &resources);
    for (owner, expected) in [(white, Some([255u8; 4])), (zero, None)] {
        let prepared = renderer.prepare_block(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), true).unwrap();
        let output = renderer.create_render_texture(Size { width: 8, height: 8 }).unwrap();
        let mut frame = renderer
            .texture_frame(FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 255, 0, 255)).unwrap(), output)
            .unwrap();
        frame.execute_block(&prepared);
        let mut pixels = [0; 8 * 8 * 4];
        // SAFETY: The selected 8x8 RGBA target and initialized destination remain live.
        unsafe {
            let target = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, target);
            gl.read_pixels(0, 0, 8, 8, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixels)));
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        }
        frame.finish().unwrap();
        match expected {
            Some(pixel) => assert!(pixels.as_chunks::<4>().0.iter().all(|value| *value == pixel)),
            None => sampled::assert_zero_storage_pixels(&pixels),
        }
    }
    let mut image = resources
        .create_dynamic_buffer(GpuBufferDesc::Texture {
            size: Size { width: 1, height: 1 },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        })
        .unwrap();
    let version = gpu.imported_texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
    renderer.prepare_texture(version.clone()).unwrap();
    lose_test_context(&native);
    let late = renderer.core().check_error("WebGL diagnostic").unwrap_err();
    assert!(late.contains("0x9242"), "{late}");
    assert_eq!(renderer.make_current(), Err(late), "the first diagnosis survives later health checks");
    assert!(renderer.prepare_texture(version).is_err(), "a Ready image cannot bypass terminal context loss");
    assert_eq!(
        image.upload(neocogi_com_sdk::NcCopyFromBufferOffset::new(0, &[0; 4])),
        Err(neocogi_platform::render::abi::RENDER_E_BACKEND),
        "uploads must observe actual WebGL loss"
    );
}

// Synthetic DOM events exercise translation deterministically. Pointer capture itself requires
// trusted browser input, so this fixture stubs only that platform primitive, not the Rust routing.
#[wasm_bindgen(inline_js = r#"
export function lose_test_context(context) {
    const extension = context.getExtension("WEBGL_lose_context");
    if (!extension) throw new Error("WebGL loss extension unavailable");
    extension.loseContext();
}
export function restore_test_context(canvas, atWait) {
    const context = canvas.getContext("webgl2");
    const extension = context.getExtension("WEBGL_lose_context");
    return new Promise((resolve, reject) => {
        if (!extension) {
            reject(new Error("WebGL loss extension unavailable"));
            return;
        }
        const timeout = setTimeout(() => reject(new Error("WebGL restoration timed out")), 5000);
        canvas.addEventListener("webglcontextlost", event => {
            event.preventDefault();
            setTimeout(() => extension.restoreContext(), 100);
        }, {once: true});
        canvas.addEventListener("webglcontextrestored", () => {
            clearTimeout(timeout);
            if (canvas.getContext("webgl2") === context && !context.isContextLost()) {
                resolve();
            } else {
                reject(new Error("WebGL lost context was not restored"));
            }
        }, {once: true});
        if (atWait) {
            const wait = context.clientWaitSync;
            context.clientWaitSync = function(...args) {
                delete context.clientWaitSync;
                extension.loseContext();
                return wait.apply(this, args);
            };
        } else {
            extension.loseContext();
        }
    });
}
export function record_panic(text) {
    document.getElementById('result').dataset.panic = text;
}
export function next_test_task() {
    return new Promise(resolve => setTimeout(resolve, 0));
}
export function exercise_shortcut(target, key, code, location, modifiers) {
    const init = {key, code, location, altKey:!!(modifiers & 1), ctrlKey:!!(modifiers & 2),
        shiftKey:!!(modifiers & 4), metaKey:!!(modifiers & 8)};
    target.dispatchEvent(new KeyboardEvent('keydown', {...init, repeat:false}));
    target.dispatchEvent(new KeyboardEvent('keydown', {...init, repeat:true}));
    target.dispatchEvent(new KeyboardEvent('keyup', init));
}
export function keyboard_test_event(target, key, code, flags) {
    const event = new KeyboardEvent(flags & 1 ? 'keyup' : 'keydown', {
        key, code, repeat:!!(flags & 2), shiftKey:!!(flags & 4),
        isComposing:!!(flags & 8), cancelable:true,
    });
    target.dispatchEvent(event);
    return event.defaultPrevented;
}
export function boundary_text(target, mode, limit) {
    if (mode === 'late-composition-input') {
        target.value = 'discarded composition';
        const input = new InputEvent('input');
        // Chromium can normalize an older browser's input type in the constructor.
        Object.defineProperty(input, 'inputType', {value:'insertFromComposition'});
        target.dispatchEvent(input);
        return;
    }
    if (mode === 'preedit') {
        target.dispatchEvent(new CompositionEvent('compositionstart'));
        target.value = '🦀'.repeat(limit / 4);
        target.dispatchEvent(new InputEvent('input', {isComposing:true}));
        return;
    }
    if (mode === 'commit') {
        target.dispatchEvent(new CompositionEvent('compositionend'));
        target.dispatchEvent(new InputEvent('input'));
        return;
    }
    target.value = mode === 'ascii-over' ? 'a'.repeat(limit + 1) :
        mode === 'utf8-over' ? '🦀'.repeat(limit / 4) + 'a' : mode;
    target.dispatchEvent(new InputEvent('input', {inputType:'insertFromPaste'}));
}
export function exercise(target, name) {
    if (name === 'compose-start') {
        target.dispatchEvent(new CompositionEvent('compositionstart'));
        target.value = '日本';
        target.dispatchEvent(new InputEvent('input', {isComposing:true}));
        target.dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', isComposing:true}));
    } else if (name === 'compose-end') {
        target.dispatchEvent(new CompositionEvent('compositionend', {data:'日本'}));
        target.dispatchEvent(new InputEvent('input', {data:'日本'}));
    } else if (name === 'paste') {
        target.value = 'é\n pasted';
        target.dispatchEvent(new InputEvent('input', {inputType:'insertFromPaste'}));
    } else if (name === 'tab') {
        target.dispatchEvent(new KeyboardEvent('keydown', {key:'Tab'}));
        target.dispatchEvent(new KeyboardEvent('keyup', {key:'Tab'}));
    } else if (name === 'key') {
        target.dispatchEvent(new KeyboardEvent('keydown', {key:'F'}));
    } else if (name === 'blur') {
        target.dispatchEvent(new FocusEvent('blur'));
    } else if (name === 'half-wheel') {
        target.dispatchEvent(new WheelEvent('wheel', {deltaY:0.5, deltaMode:0}));
    } else if (name === 'wheel') {
        target.dispatchEvent(new WheelEvent('wheel', {deltaY:0.5, deltaMode:0}));
        target.dispatchEvent(new WheelEvent('wheel', {deltaY:0.5, deltaMode:0}));
    } else if (name === 'scaled-drawable') {
        const bounds = target.getBoundingClientRect();
        target.dispatchEvent(new PointerEvent('pointermove', {isPrimary:true, clientY:Math.round(bounds.top + bounds.height * 0.75)}));
        target.dispatchEvent(new WheelEvent('wheel', {deltaY:1, deltaMode:2}));
    } else if (name === 'down') {
        target.setPointerCapture = () => {};
        target.dispatchEvent(new PointerEvent('pointerdown', {pointerId:7, isPrimary:true, buttons:1, clientX:10, clientY:10}));
    } else if (name === 'chord') {
        target.dispatchEvent(new PointerEvent('pointermove', {pointerId:7, isPrimary:true, buttons:3}));
    } else if (name === 'up') {
        target.dispatchEvent(new PointerEvent('pointerup', {pointerId:7, isPrimary:true, buttons:0}));
        target.dispatchEvent(new PointerEvent('lostpointercapture', {pointerId:7, isPrimary:true}));
    } else if (name === 'leave') {
        target.dispatchEvent(new PointerEvent('pointerleave', {pointerId:7, isPrimary:true, buttons:1}));
    } else if (name === 'foreign-leave') {
        target.dispatchEvent(new PointerEvent('pointerleave', {pointerId:8, isPrimary:true, pointerType:'pen'}));
    } else if (name === 'cancel') {
        target.dispatchEvent(new PointerEvent('pointercancel', {pointerId:7, isPrimary:true}));
    }
}
"#)]
extern "C" {
    /// Keeps the Rust panic diagnostic in the dumped DOM even when wasm traps without unwinding.
    fn record_panic(text: &str);
    /// Triggers actual WebGL context loss after an image has reached its prepared state.
    fn lose_test_context(context: &web_sys::WebGl2RenderingContext);
    /// Completes native loss and restoration on separate browser tasks.
    fn restore_test_context(canvas: &HtmlCanvasElement, at_wait: bool) -> js_sys::Promise;
    /// Dispatches one deterministic DOM sequence against the actual installed Rust listeners.
    fn exercise(target: &EventTarget, name: &str);
    /// Dispatches press/repeat/release with an explicit logical key, physical location and modifiers.
    fn exercise_shortcut(target: &EventTarget, key: &str, code: &str, location: u32, modifiers: u8);
    /// Dispatches one key. Flags represent release, repeat, Shift and IME composition.
    fn keyboard_test_event(target: &EventTarget, key: &str, code: &str, flags: u8) -> bool;
    /// Drives either actual textarea composition or full-value paste events using browser strings.
    fn boundary_text(target: &EventTarget, mode: &str, limit: u32);
    fn next_test_task() -> js_sys::Promise;
}

#[com_object(interfaces(IInputSink))]
/// Copies synchronous COM delivery without retaining the window or any event-buffer borrow.
struct ShortcutSink {
    /// COM requires a shared receiver; each short observation borrow ends before polling resumes.
    received: Rc<RefCell<Vec<(UiInput, String)>>>,
}
impl IInputSink for ShortcutSink {
    /// Preserves the complete canonical event and exact text argument for comparison after dispatch.
    fn input(&self, event: UiInput, text: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        self.received
            .borrow_mut()
            .push((event, std::str::from_utf8(text.as_slice()).unwrap().to_owned()));
        Ok(())
    }
}

/// Ordinary and keypad operators/Enter cross production DOM listeners and public window COM polling.
/// Synthetic location/code values cover logical normalization, not trusted physical keyboard input.
fn browser_shortcuts_through_com() {
    let native = PlatformObject::new().window("browser shortcut contract", Size { width: 8, height: 8 }).unwrap();
    let canvas = native.device.window.clone();
    let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(native).query().unwrap();
    let received = Rc::new(RefCell::new(Vec::new()));
    let sink: ComPtr<IInputSinkAbi> = ShortcutSink::from_value(ShortcutSink { received: received.clone() }).query().unwrap();
    for modifiers in 0u8..16 {
        for (text, code, location, key) in [
            ("+", "NumpadAdd", 3, KeyCode::Character(u32::from('+'))),
            ("+", "Equal", 0, KeyCode::Character(u32::from('+'))),
            ("-", "NumpadSubtract", 3, KeyCode::Character(u32::from('-'))),
            ("-", "Minus", 0, KeyCode::Character(u32::from('-'))),
            ("*", "NumpadMultiply", 3, KeyCode::Character(u32::from('*'))),
            ("*", "Digit8", 0, KeyCode::Character(u32::from('*'))),
            ("/", "NumpadDivide", 3, KeyCode::Character(u32::from('/'))),
            ("/", "Slash", 0, KeyCode::Character(u32::from('/'))),
            ("Enter", "NumpadEnter", 3, KeyCode::Enter),
            ("Enter", "Enter", 0, KeyCode::Enter),
        ] {
            exercise_shortcut(canvas.unchecked_ref(), text, code, location, modifiers);
            let actions = [KeyAction::Pressed { repeat: false }, KeyAction::Pressed { repeat: true }, KeyAction::Released];
            for _ in actions {
                assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Event));
            }
            assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Idle));
            let expected = actions.map(|action| (UiInput::Key(KeyStroke { key, action, modifiers }), String::new()));
            assert_eq!(received.borrow().as_slice(), expected, "{code}, modifiers {modifiers}");
            received.borrow_mut().clear();
        }
    }
    drop(sink);
    assert_eq!(Rc::strong_count(&received), 1, "polling does not retain the borrowed sink");
}

/// Full UTF-8 payloads reach the COM sink once. Either size refusal keeps its FIFO slot.
fn browser_text_commit_limit_through_com() {
    let object = PlatformObject::new().window("text commit admission", Size { width: 8, height: 8 }).unwrap();
    let text = object.input.text.clone();
    let window = WindowObject::from_value(object).query::<IPlatformWindowAbi>().unwrap();
    window.text_input(TextInputArea::Area(Rect { x: 0, y: 0, width: 1, height: 1 })).unwrap();
    let received = Rc::new(RefCell::new(Vec::new()));
    let sink = ShortcutSink::from_value(ShortcutSink { received: received.clone() })
        .query::<IInputSinkAbi>()
        .unwrap();
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    received.borrow_mut().clear();
    let dispatch = |mode| boundary_text(text.unchecked_ref(), mode, MAX_TEXT_INPUT_BYTES as u32);
    dispatch("preedit");
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Idle));
    dispatch("commit");
    dispatch("ascii-over");
    dispatch("é between");
    dispatch("utf8-over");
    dispatch("after");
    for (expected, deliveries) in [
        (Ok(PollStatus::Event), 1),
        (Err(PLATFORM_E_INPUT_LIMIT), 1),
        (Ok(PollStatus::Event), 2),
        (Err(PLATFORM_E_INPUT_LIMIT), 2),
        (Ok(PollStatus::Event), 3),
        (Ok(PollStatus::Idle), 3),
    ] {
        assert_eq!(window.poll(sink.as_com_ref()), expected);
        assert_eq!(received.borrow().len(), deliveries);
    }
    let received = received.borrow();
    assert!(received.iter().all(|(event, _)| *event == UiInput::Text));
    assert_eq!(received[0].1.len(), MAX_TEXT_INPUT_BYTES);
    assert!(received[0].1.chars().all(|scalar| scalar == '🦀'));
    assert_eq!(received[1].1, "é between");
    assert_eq!(received[2].1, "after");
    assert!(text.value().is_empty());
}

#[com_object(interfaces(IInputSink))]
/// Reenters DOM input at the cancellation callback, then rejects that attempted delivery.
struct OverflowSink {
    window: ComPtr<IPlatformWindowAbi>,
    ordinary: ComPtr<IInputSinkAbi>,
    canvas: HtmlCanvasElement,
}
impl IInputSink for OverflowSink {
    fn input(&self, event: UiInput, text: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        assert_eq!(event, UiInput::Cancel);
        assert!(text.as_slice().is_empty());
        keyboard_test_event(self.canvas.unchecked_ref(), "n", "KeyN", 0);
        assert_eq!(self.window.poll(self.ordinary.as_com_ref()), Err(PLATFORM_E_BUSY));
        Err(NC_E_INVALID_ARG)
    }
}

/// Reserves one cancellation on aggregate byte loss and resumes DOM/COM delivery in order.
fn browser_text_backlog_recovers_through_com() {
    let platform = PlatformObject::new();
    let object = platform.window("bounded browser input", Size { width: 8, height: 8 }).unwrap();
    let sibling = platform.window("unaffected browser input", Size { width: 8, height: 8 }).unwrap();
    let sibling_canvas = sibling.device.window.clone();
    let sibling = WindowObject::from_value(sibling).query::<IPlatformWindowAbi>().unwrap();
    let text = object.input.text.clone();
    let canvas = object.device.window.clone();
    let window = WindowObject::from_value(object).query::<IPlatformWindowAbi>().unwrap();
    window.text_input(TextInputArea::Area(Rect { x: 0, y: 0, width: 1, height: 1 })).unwrap();
    let received = Rc::new(RefCell::new(Vec::new()));
    let sink = ShortcutSink::from_value(ShortcutSink { received: received.clone() })
        .query::<IInputSinkAbi>()
        .unwrap();
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    while sibling.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    received.borrow_mut().clear();

    exercise(canvas.unchecked_ref(), "down");
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    assert!(matches!(received.borrow().last(), Some((UiInput::Down { .. }, _))));
    received.borrow_mut().clear();
    exercise(canvas.unchecked_ref(), "half-wheel");
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Idle));
    let dispatch = |mode| boundary_text(text.unchecked_ref(), mode, MAX_TEXT_INPUT_BYTES as u32);
    for _ in 0..2 {
        dispatch("preedit");
        dispatch("commit");
    }
    dispatch("aggregate overflow");
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Event));
    assert!(
        keyboard_test_event(text.unchecked_ref(), "z", "KeyZ", 0),
        "a full route rejects the attempted down"
    );
    dispatch("drop until cancel");
    exercise(text.unchecked_ref(), "compose-start");
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Event));
    {
        let events = received.borrow();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|(event, text)| *event == UiInput::Text && text.len() == MAX_TEXT_INPUT_BYTES));
    }

    let cancellation = OverflowSink::from_value(OverflowSink {
        window: window.clone(),
        ordinary: sink.clone(),
        canvas: canvas.clone(),
    })
    .query::<IInputSinkAbi>()
    .unwrap();
    assert_eq!(window.poll(cancellation.as_com_ref()), Err(NC_E_INVALID_ARG));
    let sibling_events = Rc::new(RefCell::new(Vec::new()));
    let sibling_sink = ShortcutSink::from_value(ShortcutSink { received: sibling_events.clone() })
        .query::<IInputSinkAbi>()
        .unwrap();
    // Focusing the main editor can enqueue a normal blur on its sibling.
    while sibling.poll(sibling_sink.as_com_ref()).unwrap() == PollStatus::Event {}
    sibling_events.borrow_mut().clear();
    keyboard_test_event(sibling_canvas.unchecked_ref(), "q", "", 2);
    while sibling.poll(sibling_sink.as_com_ref()).unwrap() == PollStatus::Event {}
    let sibling_events = sibling_events.borrow();
    assert!(
        matches!(sibling_events.as_slice(), [(UiInput::Key(stroke), _)]
            if stroke.key == KeyCode::Character(u32::from('q')) && stroke.action == KeyAction::Pressed { repeat: true }),
        "unexpected sibling events after primary overflow: {sibling_events:?}"
    );
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Event));
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Idle));
    assert!(matches!(received.borrow().last(), Some((UiInput::Key(stroke), _)) if stroke.key == KeyCode::Character(u32::from('n'))));
    received.borrow_mut().clear();

    dispatch("late-composition-input");
    text.set_value("stale composition");
    exercise(text.unchecked_ref(), "compose-end");
    dispatch("late-composition-input");
    let key = |logical, physical, flags| keyboard_test_event(text.unchecked_ref(), logical, physical, flags);
    assert!(key("x", "KeyX", 2), "stale repeats must suppress DOM text insertion");
    assert!(key("z", "KeyZ", 2), "an unadmitted press cannot authorize a repeat");
    assert!(!key("n", "KeyN", 2), "fresh repeats must retain editable defaults");
    exercise(canvas.unchecked_ref(), "half-wheel");
    exercise(canvas.unchecked_ref(), "chord");
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    assert!(
        matches!(
            received.borrow().as_slice(),
            [(UiInput::Key(stroke), _), (UiInput::Pointer(_), _)] if stroke.key == KeyCode::Character(u32::from('n'))
        ),
        "unexpected events after overflow recovery: {:?}",
        *received.borrow()
    );
    received.borrow_mut().clear();

    assert!(!key("!", "Digit1", 4));
    assert!(!key("1", "Digit1", 2), "the same physical key repeats after Shift is released");
    assert!(key("1", "Numpad1", 2), "a logical alias cannot authorize a repeat");
    key("Dead", "Digit1", 1);
    assert!(key("1", "Digit1", 2), "an unmappable release ends freshness");
    assert!(!key("a", "KeyA", 0));
    key("a", "KeyA", 1 | 8);
    assert!(key("a", "KeyA", 2), "a release suppressed during composition ends freshness");
    let overlong = "X".repeat(65);
    for (character, anonymous) in [("b", ""), ("c", "Unidentified"), ("d", &overlong)] {
        assert!(!key(character, anonymous, 0));
        assert!(key(character, anonymous, 2));
    }
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    let strokes = std::mem::take(&mut *received.borrow_mut());
    assert_eq!(strokes.len(), 6);
    for ((event, _), (character, repeat, modifiers)) in strokes.iter().zip([
        ('!', false, 4),
        ('1', true, 0),
        ('a', false, 0),
        ('b', false, 0),
        ('c', false, 0),
        ('d', false, 0),
    ]) {
        assert_eq!(
            *event,
            UiInput::Key(KeyStroke {
                key: KeyCode::Character(u32::from(character)),
                action: KeyAction::Pressed { repeat },
                modifiers
            })
        );
    }
    assert!(!key("e", "KeyE", 0));
    exercise(text.unchecked_ref(), "blur");
    assert!(!key("e", "KeyE", 2), "ordinary focus cancellation leaves the overflow policy unchanged");
    key("e", "KeyE", 1);
    assert!(key("e", "KeyE", 2), "a mapped physical release ends repeat eligibility");
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    let strokes = std::mem::take(&mut *received.borrow_mut());
    let e = |action| {
        (
            UiInput::Key(KeyStroke {
                key: KeyCode::Character(u32::from('e')),
                action,
                modifiers: 0,
            }),
            String::new(),
        )
    };
    assert_eq!(
        strokes,
        [
            e(KeyAction::Pressed { repeat: false }),
            (UiInput::Cancel, String::new()),
            e(KeyAction::Pressed { repeat: true }),
            e(KeyAction::Released),
        ]
    );
    exercise(canvas.unchecked_ref(), "down");
    exercise(canvas.unchecked_ref(), "up");
    dispatch("preedit");
    dispatch("commit");
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    let events = received.borrow();
    assert!(events.iter().any(|(event, _)| matches!(event, UiInput::Down { .. })));
    assert!(events.iter().any(|(event, _)| matches!(event, UiInput::Up { .. })));
    assert!(matches!(events.last(), Some((UiInput::Text, text)) if text.len() == MAX_TEXT_INPUT_BYTES));
    dispatch("late-composition-input");
    assert!(text.value().is_empty(), "ignored tagged input must leave no value for the next commit");
}

/// Runs WebGL target/ownership checks and DOM composition, paste, keyboard, wheel, pointer and teardown assertions.
pub async fn run_platform_input_tests() {
    std::panic::set_hook(Box::new(|info| record_panic(&info.to_string())));
    for (native, expected) in [
        ("F1", Some(KeyCode::Function(1))),
        ("F24", Some(KeyCode::Function(24))),
        ("F25", None),
        ("F0", None),
        ("🦀", Some(KeyCode::Character(u32::from('🦀')))),
        ("ArrowLeft", Some(KeyCode::ArrowLeft)),
        ("Unidentified", None),
    ] {
        assert_eq!(map_key(native), expected);
    }
    browser_shortcuts_through_com();
    browser_text_commit_limit_through_com();
    browser_text_backlog_recovers_through_com();
    canonical_browser_derivatives().await;
    browser_surface_targets();
    geometry_tests::canvas_content_coordinates_through_com();
    clamped_drawing_buffer_uses_physical_coordinates();
    drawable_loss_keeps_render_diagnostics();
    restoration_requires_a_fresh_driver().await;
    explicit_finish_latches_webgl_loss().await;
    buffer_publication_requires_live_storage();
    crate::platform::extent_tests::run();
    crate::platform::extent_tests::run_window_depth();
    let document = web_sys::window().unwrap().document().unwrap();
    let canvas = document.create_element("canvas").unwrap().dyn_into::<HtmlCanvasElement>().unwrap();
    canvas.set_width(200);
    canvas.set_height(200);
    canvas.style().set_css_text("width:100px;height:100px");
    document.body().unwrap().append_child(&canvas).unwrap();
    let gpu = gpu::Gpu::from_canvas(canvas.clone()).unwrap();
    let host = BrowserInputHost::new(&canvas, gpu.core()).unwrap();
    canvas.focus().unwrap();
    host.synchronize(&canvas, Some(Rect { x: 10, y: 10, width: 80, height: 20 })).unwrap();
    assert_eq!(document.active_element(), Some(host.text.clone().into()));
    exercise(host.text.unchecked_ref(), "compose-start");
    assert!(host.drain().is_empty(), "preedit and its Enter key must stay browser-owned");
    exercise(host.text.unchecked_ref(), "compose-end");
    assert!(matches!(host.drain().as_slice(), [PendingInput::Event { event: UiInput::Text, text }] if text.as_ref() == "日本"));
    exercise(host.text.unchecked_ref(), "paste");
    assert!(matches!(host.drain().as_slice(), [PendingInput::Event { event: UiInput::Text, text }] if text.as_ref() == "é\n pasted"));
    exercise(host.text.unchecked_ref(), "key");
    assert!(matches!(host.drain().as_slice(), [PendingInput::Event { event: UiInput::Key(event), .. }] if event.key == KeyCode::Character(u32::from('F'))));
    exercise(canvas.unchecked_ref(), "wheel");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput::Event {
                event: UiInput::Scroll(Point { x: 0, y: 1 }),
                ..
            },
            PendingInput::Event {
                event: UiInput::Scroll(Point { x: 0, y: 1 }),
                ..
            }
        ]
    ));

    exercise(canvas.unchecked_ref(), "down");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput::Event { event: UiInput::Pointer(_), .. },
            PendingInput::Event { event: UiInput::Down { .. }, .. }
        ]
    ));
    exercise(canvas.unchecked_ref(), "foreign-leave");
    assert!(host.drain().is_empty());
    exercise(canvas.unchecked_ref(), "leave");
    assert!(matches!(host.drain().as_slice(), [PendingInput::Event { event: UiInput::PointerLeft, .. }]));
    exercise(canvas.unchecked_ref(), "chord");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput::Event { event: UiInput::Pointer(_), .. },
            PendingInput::Event {
                event: UiInput::Down { buttons: 2, .. },
                ..
            }
        ]
    ));
    exercise(canvas.unchecked_ref(), "up");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput::Event { event: UiInput::Pointer(_), .. },
            PendingInput::Event {
                event: UiInput::Up { buttons: 1, .. },
                ..
            },
            PendingInput::Event {
                event: UiInput::Up { buttons: 2, .. },
                ..
            }
        ]
    ));
    exercise(canvas.unchecked_ref(), "down");
    host.drain();
    exercise(canvas.unchecked_ref(), "cancel");
    assert!(matches!(host.drain().as_slice(), [PendingInput::Event { event: UiInput::Cancel, .. }]));

    // Holding the queue here verifies teardown removes the actual registrations, not just handles.
    let queue = host.queue.clone();
    let text = host.text.clone();
    drop(host);
    assert!(text.parent_node().is_none());
    exercise(canvas.unchecked_ref(), "key");
    assert!(queue.borrow_mut().events.pop().is_none());
    canvas.remove();
}
