//! Real native/WebGL COM submissions mutate the drawable in the last stream callback.
use super::*;
use glow::HasContext;
use neocogi_com::{ComBox, WeakComBox};

/// Shared-receiver COM callbacks record observations without retaining a borrow across reentry.
struct Observed {
    /// Number of actual stream calls, including the final mutating callback.
    calls: Cell<u32>,
    /// Actual physical extent returned by the reentrant public window query.
    size: Cell<Size>,
}

#[com_object(interfaces(IRenderStream))]
/// Owns real sealed blocks and resizes the same physical window before returning the final one.
struct ResizingStream {
    /// Sealed production contributions remain owned throughout synchronous submission.
    blocks: Vec<ComBox<IRenderViewBlockAbi>>,
    /// Public metadata query is deliberately allowed while native render state is detached.
    window: ComPtr<IPlatformWindowAbi>,
    /// Test-only native mutation requires &mut Window, but the COM stream receiver is shared.
    /// This short borrow ends before the public window query or any other foreign call.
    #[cfg(not(target_arch = "wasm32"))]
    native: RefCell<gpu::Window>,
    /// Application CSS changes are ordinary page operations on the production canvas.
    #[cfg(target_arch = "wasm32")]
    canvas: gpu::Window,
    /// Requested logical/CSS dimensions, converted by the actual window query to physical pixels.
    requested: Size,
    /// Concrete context owner makes a resized native drawable current before the sentinel clear.
    core: Rc<gpu::Core>,
    /// Independently retained observations prove both callbacks ran before admission rejected.
    observed: Rc<Observed>,
}
impl IRenderStream for ResizingStream {
    /// The second callback changes the drawable after the first valid block was already retained.
    fn view(&self, _: u64, index: u32) -> Result<WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        let block = self.blocks.get(index as usize).ok_or(NC_E_INVALID_ARG)?;
        self.observed.calls.set(self.observed.calls.get() + 1);
        if index == 1 {
            #[cfg(not(target_arch = "wasm32"))]
            self.native
                .borrow_mut()
                .set_size(self.requested.width as u32, self.requested.height as u32)
                .unwrap();
            #[cfg(target_arch = "wasm32")]
            self.canvas
                .style()
                .set_css_text(&format!("width:{}px;height:{}px", self.requested.width, self.requested.height));
            self.observed.size.set(self.window.size()?);
            self.core.current().unwrap();
            let gl = self.core.dispatch();
            // SAFETY: The same live window owns this current GL context. This application
            // clear initializes its entire new backing; no foreign memory is passed to GL.
            unsafe {
                gl.disable(glow::SCISSOR_TEST);
                gl.color_mask(true, true, true, true);
                gl.clear_color(0., 1., 0., 1.);
                gl.clear(glow::COLOR_BUFFER_BIT);
                assert_eq!(gl.get_error(), glow::NO_ERROR);
            }
        }
        Ok(block.downgrade())
    }
}

/// Records a small regional red clear through the real generated COM broker and resources.
fn clear_block(host: &ComPtr<IRenderHostAbi>, resources: &ComPtr<IRenderResourcesAbi>) -> ComBox<IRenderViewBlockAbi> {
    let bounds = Rect { x: 0, y: 0, width: 4, height: 4 };
    let mut frame = host.recording_broker().unwrap().begin_frame().unwrap();
    let mut session = frame
        .begin_view(RenderViewRoot { client: bounds, target: bounds }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.clear_color(0xff0000ff).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    block
}

/// Reads exactly the admitted current physical allocation, with bottom-up GL row ordering.
fn pixels(gl: &glow::Context, size: Size) -> Vec<[u8; 4]> {
    let mut pixels = vec![0; size.width as usize * size.height as usize * 4];
    // SAFETY: The window/context are live and current; initialized RGBA storage covers
    // the full actual extent just observed through the production public size query.
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
        gl.read_pixels(
            0,
            0,
            size.width,
            size.height,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
        assert_eq!(gl.get_error(), glow::NO_ERROR);
    }
    pixels.as_chunks::<4>().0.to_vec()
}

/// Native acceptance captures before swap, whose backbuffer contents may subsequently be discarded.
/// Browser calls read synchronously before yielding, while the same canvas contents remain available.
fn rendered_pixels(device: &Weak<Device>) -> Vec<[u8; 4]> {
    let owner = device.upgrade().unwrap();
    let mut access = owner.access().unwrap();
    #[cfg(not(target_arch = "wasm32"))]
    {
        access.state().gpu.last_pixels().as_chunks::<4>().0.to_vec()
    }
    #[cfg(target_arch = "wasm32")]
    {
        pixels(access.state().gpu.core().dispatch(), owner.drawable_size().unwrap())
    }
}

/// Growth, shrinkage and unchanged queries cross the real callback boundary on both GL hosts.
pub(super) fn run() {
    for (from, to) in [
        ((8, 8), (16, 8)),
        ((16, 8), (8, 8)),
        ((8, 8), (8, 16)),
        ((8, 16), (8, 8)),
        ((8, 8), (16, 16)),
        ((16, 16), (8, 8)),
        ((8, 8), (8, 8)),
    ] {
        let requested = Size { width: to.0, height: to.1 };
        let object = PlatformObject::new()
            .window("drawable callback admission", Size { width: from.0, height: from.1 })
            .unwrap();
        let native = object.device.window.clone();
        let device = Rc::downgrade(&object.device);
        #[cfg(target_arch = "wasm32")]
        native.style().set_css_text(&format!("width:{}px;height:{}px", from.0, from.1));
        let core = object.device.access().unwrap().state().gpu.core();
        let gl = core.dispatch().clone();
        let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(object).query().unwrap();
        let before = window.size().unwrap();
        let host = window.graphics_host().unwrap();
        let surfaces: ComPtr<IRenderSurfaceFactoryAbi> = host.query().unwrap();
        let surface = surfaces.create_surface(RenderSurfaceDesc::Framebuffer).unwrap();
        let clients: ComPtr<IRenderClientFactoryAbi> = host.query().unwrap();
        let group = clients
            .create_client(
                Default::default(),
                surface.as_com_ref(),
                RenderClientPermissions {
                    clear_color: true,
                    ..RenderClientPermissions::none()
                },
            )
            .unwrap();
        let resources = group.resources().unwrap();
        let observed = Rc::new(Observed {
            calls: Cell::new(0),
            size: Cell::new(before),
        });
        let stream: ComPtr<IRenderStreamAbi> = ResizingStream::from_value(ResizingStream {
            blocks: vec![clear_block(&host, &resources), clear_block(&host, &resources)],
            window: window.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            native: RefCell::new(native),
            #[cfg(target_arch = "wasm32")]
            canvas: native,
            requested,
            core,
            observed: observed.clone(),
        })
        .query()
        .unwrap();
        let renderer: ComPtr<IRendererAbi> = window.query().unwrap();
        let info = RenderInfo {
            size: before,
            clear: 0xffff0000,
            invocation: 1,
            block_count: 2,
        };
        let result = renderer.render(info, stream.as_com_ref());
        assert_eq!(observed.calls.get(), 2);
        let after = window.size().unwrap();
        assert_eq!(observed.size.get(), after);
        assert_eq!(surface.size().unwrap(), after);
        // SDL's offscreen driver can expose a larger requested extent before its fixed
        // pbuffer grows. Check its common old/new rectangle; WebGL checks the entire new backing.
        let preserved = if cfg!(target_arch = "wasm32") {
            after
        } else {
            Size {
                width: before.width.min(after.width),
                height: before.height.min(after.height),
            }
        };
        let retained_error = if from != to {
            assert_ne!(before, after);
            let error = result.unwrap_err();
            assert_eq!(error.status, NC_E_INVALID_ARG);
            assert!(error.message.contains("drawable size changed"));
            assert!(
                pixels(&gl, preserved).iter().all(|pixel| *pixel == [0, 255, 0, 255]),
                "rejected submission cleared/drew over callback pixels"
            );
            assert_eq!(
                renderer.render(RenderInfo { block_count: 0, ..info }, stream.as_com_ref()).unwrap_err().status,
                NC_E_INVALID_ARG
            );
            Some(error)
        } else {
            result.unwrap();
            for (index, pixel) in rendered_pixels(&device).into_iter().enumerate() {
                let x = index % after.width as usize;
                let y = index / after.width as usize;
                assert_eq!(
                    pixel,
                    if x < 4 && y >= after.height as usize - 4 {
                        [255, 0, 0, 255]
                    } else {
                        [0, 0, 255, 255]
                    }
                );
            }
            None
        };
        renderer
            .render(RenderInfo { size: after, block_count: 0, ..info }, stream.as_com_ref())
            .unwrap();
        let recovered = rendered_pixels(&device);
        for y in 0..preserved.height as usize {
            for x in 0..preserved.width as usize {
                assert_eq!(recovered[y * after.width as usize + x], [0, 0, 255, 255]);
            }
        }
        assert_eq!(observed.calls.get(), 2, "empty controls must not call the stream");
        if let Some(error) = retained_error {
            assert!(error.message.contains("drawable size changed"), "later success erased owned failure");
        }
    }
}

/// Serializes native SDL ownership and runs exactly the same COM/pixel cases as browser acceptance.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[test]
#[ignore = "requires SDL GL; run with SDL_VIDEODRIVER=offscreen"]
fn callback_drawable_changes_reject_before_native_clear_and_recover() {
    let _session = super::tests::native_session();
    run();
}
