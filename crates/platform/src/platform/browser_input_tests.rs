//! Browser-executed DOM regressions exported from the wasm library test artifact.
//!
//! The final neocogi-browser-tests cdylib owns the public JavaScript export and
//! single host. `cargo tools test-browser` builds it, generates matching bindings,
//! serves the checked-in page and requires actual browser PASS.

use super::*;
use crate::platform::{PlatformObject, WindowObject, identity};
use neocogi_com::{ComPtr, NC_E_INVALID_ARG, com_object};
use neocogi_platform::render::abi::*;
use wasm_bindgen::prelude::*;

use neocogi_platform::gpu::testing::{derivatives, sampled};

#[com_object(interfaces(IRenderStream))]
/// Browser-owned sealed contributions exercise the same generated COM path as native applications.
struct SurfaceBlocks {
    /// Exclusive owners remain alive through each synchronous image/window submission.
    blocks: Vec<neocogi_com::ComBox<IRenderViewBlockAbi>>,
}
impl IRenderStream for SurfaceBlocks {
    /// Borrows a sealed contribution without transferring its owner to the renderer.
    fn view(&self, _: u64, index: u32) -> Result<neocogi_com::WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        self.blocks.get(index as usize).map(neocogi_com::ComBox::downgrade).ok_or(NC_E_INVALID_ARG)
    }
}

/// Records a regional color clear with depth disabled; no automatic depth command may be required.
fn surface_clear(host: &ComPtr<IRenderHostAbi>, group: &neocogi_com::ComBox<IRenderClientDomainAbi>) -> ComPtr<IRenderStreamAbi> {
    let bounds = Rect { x: 0, y: 0, width: 4, height: 4 };
    let mut frame = host.recording_broker().unwrap().begin_frame().unwrap();
    let resources = group.resources().unwrap();
    let mut session = frame
        .begin_view(RenderViewRoot { client: bounds, target: bounds }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.clear_color(0xff0000ff).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    SurfaceBlocks::from_value(SurfaceBlocks { blocks: vec![block] }).query().unwrap()
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
    let groups: ComPtr<IRenderClientFactoryAbi> = host.query().unwrap();
    let surfaces: ComPtr<IRenderSurfaceFactoryAbi> = host.query().unwrap();
    assert!(host.query::<IRendererAbi>().is_err());
    let permissions = RenderClientPermissions {
        clear_color: true,
        write_color: true,
        write_resources: true,
        import_images: true,
        ..RenderClientPermissions::none()
    };
    let framebuffer = surfaces.create_surface(RenderSurfaceDesc::Framebuffer).unwrap();
    assert_eq!(framebuffer.size().unwrap(), drawable);
    assert!(framebuffer.query::<IRendererAbi>().is_err());
    let window_group = groups.create_client(Default::default(), framebuffer.as_com_ref(), permissions).unwrap();
    let image = surfaces.create_surface(RenderSurfaceDesc::Image(size)).unwrap();
    let image_group = groups.create_client(Default::default(), image.as_com_ref(), permissions).unwrap();
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
    assert!(matches!(other.completed_image(), Err(neocogi_com::NC_E_NOT_FOUND)));
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
                match buffer.upload(neocogi_com::NcCopyFromBufferOffset::new(0, &bytes)) {
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
    assert!(renderer.make_current().is_err(), "actual WebGL loss invalidates the physical owner");
    assert!(renderer.prepare_texture(version).is_err(), "a Ready image cannot bypass terminal context loss");
    assert_eq!(
        image.upload(neocogi_com::NcCopyFromBufferOffset::new(0, &[0; 4])),
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
    } else if (name === 'wheel') {
        target.dispatchEvent(new WheelEvent('wheel', {deltaY:0.5, deltaMode:0}));
        target.dispatchEvent(new WheelEvent('wheel', {deltaY:0.5, deltaMode:0}));
    } else if (name === 'down') {
        target.setPointerCapture = () => {};
        target.dispatchEvent(new PointerEvent('pointerdown', {pointerId:7, isPrimary:true, buttons:1, clientX:10, clientY:10}));
    } else if (name === 'chord') {
        target.dispatchEvent(new PointerEvent('pointermove', {pointerId:7, isPrimary:true, buttons:3}));
    } else if (name === 'up') {
        target.dispatchEvent(new PointerEvent('pointerup', {pointerId:7, isPrimary:true, buttons:0}));
        target.dispatchEvent(new PointerEvent('lostpointercapture', {pointerId:7, isPrimary:true}));
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
    /// Dispatches one deterministic DOM sequence against the actual installed Rust listeners.
    fn exercise(target: &EventTarget, name: &str);
    /// Dispatches press/repeat/release with an explicit logical key, physical location and modifiers.
    fn exercise_shortcut(target: &EventTarget, key: &str, code: &str, location: u32, modifiers: u8);
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
    canonical_browser_derivatives().await;
    browser_surface_targets();
    crate::platform::extent_tests::run();
    let document = web_sys::window().unwrap().document().unwrap();
    let canvas = document.create_element("canvas").unwrap().dyn_into::<HtmlCanvasElement>().unwrap();
    canvas.set_width(200);
    canvas.set_height(200);
    canvas.style().set_css_text("width:100px;height:100px");
    document.body().unwrap().append_child(&canvas).unwrap();
    let host = BrowserInputHost::new(&canvas).unwrap();
    canvas.focus().unwrap();
    host.synchronize(&canvas, Some(Rect { x: 10, y: 10, width: 80, height: 20 }));
    assert_eq!(document.active_element(), Some(host.text.clone().into()));
    exercise(host.text.unchecked_ref(), "compose-start");
    assert!(host.drain().is_empty(), "preedit and its Enter key must stay browser-owned");
    exercise(host.text.unchecked_ref(), "compose-end");
    assert!(matches!(host.drain().as_slice(), [PendingInput { event: UiInput::Text, text }] if text == "日本"));
    exercise(host.text.unchecked_ref(), "paste");
    assert!(matches!(host.drain().as_slice(), [PendingInput { event: UiInput::Text, text }] if text == "é\n pasted"));
    exercise(host.text.unchecked_ref(), "key");
    assert!(matches!(host.drain().as_slice(), [PendingInput { event: UiInput::Key(event), .. }] if event.key == KeyCode::Character(u32::from('F'))));
    exercise(canvas.unchecked_ref(), "wheel");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput {
                event: UiInput::Scroll(Point { x: 0, y: 1 }),
                ..
            },
            PendingInput {
                event: UiInput::Scroll(Point { x: 0, y: 1 }),
                ..
            }
        ]
    ));

    exercise(canvas.unchecked_ref(), "down");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput { event: UiInput::Pointer(_), .. },
            PendingInput { event: UiInput::Down { .. }, .. }
        ]
    ));
    exercise(canvas.unchecked_ref(), "chord");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput { event: UiInput::Pointer(_), .. },
            PendingInput {
                event: UiInput::Down { buttons: 2, .. },
                ..
            }
        ]
    ));
    exercise(canvas.unchecked_ref(), "up");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput { event: UiInput::Pointer(_), .. },
            PendingInput {
                event: UiInput::Up { buttons: 1, .. },
                ..
            },
            PendingInput {
                event: UiInput::Up { buttons: 2, .. },
                ..
            }
        ]
    ));
    exercise(canvas.unchecked_ref(), "down");
    host.drain();
    exercise(canvas.unchecked_ref(), "cancel");
    assert!(matches!(host.drain().as_slice(), [PendingInput { event: UiInput::Cancel, .. }]));

    // Holding the queue here verifies teardown removes the actual registrations, not just handles.
    let queue = host.queue.clone();
    let text = host.text.clone();
    drop(host);
    assert!(text.parent_node().is_none());
    exercise(canvas.unchecked_ref(), "key");
    assert!(queue.borrow().events.is_empty());
    canvas.remove();
}
