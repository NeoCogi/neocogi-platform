//! The actual example loop must use physical drawable dimensions through either plugin assembly.
#![cfg(not(target_arch = "wasm32"))]
#[path = "../triangle.rs"]
mod triangle;

use neocogi_com::{com_object, ComBox, ComPtr, ComRef, NcCopyFromBufferOffset, NcStatus};
use neocogi_platform_examples::sdk::render::abi::*;
use neocogi_platform_examples::sdk::render::platform::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// Cargo puts integration-test executables below the profile, while application loading is executable-relative.
/// Copy the composition's exact staged filenames beside this executable before acquisition; the SDK pins
/// loaded code for process lifetime, so these test-owned copies retire with Cargo's target cleanup.
#[cfg(feature = "dynamic")]
fn stage_test_plugins() -> neocogi_example_plugins::Backend {
    let executable = std::env::current_exe().unwrap();
    let profile = executable.ancestors().find(|path| path.join(".cargo-lock").is_file()).unwrap().to_path_buf();
    let directory = executable.parent().unwrap().join("lib");
    std::fs::create_dir_all(&directory).unwrap();
    let requested = std::env::var("NEOCOGI_TEST_BACKEND").ok();
    let backend = neocogi_example_plugins::Backend::parse(requested.as_deref().unwrap_or("glow")).expect("known test backend");
    let filename = neocogi_example_plugins::library_filename();
    std::fs::copy(profile.join("lib").join(filename), directory.join(filename)).unwrap();
    backend
}

#[com_object(interfaces(IPlatformWindow, IRenderer, IRenderClientFactory, IRenderSurfaceFactory))]
/// Counts application size queries and successful submissions while forwarding real native capabilities.
/// It never fabricates resource/target identities or bypasses the provider's frame-size validation.
struct ObservedWindow {
    /// Keeps the actual provider window and every forwarded interface alive throughout the loop.
    window: ComPtr<IPlatformWindowAbi>,
    /// Shared COM methods need an interior counter; no callback or resource owner is stored in it.
    size_calls: Rc<Cell<u32>>,
    /// Counts only frames successfully accepted by the real renderer.
    submissions: Rc<Cell<u32>>,
    /// Reports one suspended iteration before delegating actual positive geometry, when requested.
    initially_suspended: bool,
    /// Shared observation of the actual recording arguments, needed to check viewport bounds as well as frame size.
    roots: Rc<RefCell<Vec<RenderViewRoot>>>,
}
impl IRenderDriverBound for ObservedWindow {
    /// Retains the actual driver token so all forwarded factory projections remain compatible.
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        self.window.query::<IRendererAbi>()?.driver_identity()
    }
}
impl IPlatformWindow for ObservedWindow {
    /// The real host creates and authenticates the recorder's resource and target identities.
    fn graphics_host(&self) -> Result<ComPtr<IRenderHostAbi>, NcStatus> {
        ObservedHost::from_value(ObservedHost {
            host: self.window.graphics_host()?,
            roots: self.roots.clone(),
        })
        .query()
    }
    /// Clipboard authority is unchanged by this test's observation of drawing geometry.
    fn clipboard(&self) -> Result<ComPtr<ITextClipboardAbi>, NcStatus> {
        self.window.clipboard()
    }
    /// Only the optional first zero result is simulated; every submitted frame uses real native pixels.
    fn size(&self) -> Result<Size, NcStatus> {
        let previous = self.size_calls.get();
        self.size_calls.set(previous + 1);
        if self.initially_suspended && previous == 0 {
            Ok(Size::default())
        } else {
            self.window.size()
        }
    }
    /// Preserve ordinary event delivery if the example chooses to poll while suspended.
    fn poll(&self, destination: ComRef<'_, IInputSinkAbi>) -> Result<PollStatus, NcStatus> {
        self.window.poll(destination)
    }
    /// Preserve native IME behavior without retaining or rewriting the supplied rectangle.
    fn text_input(&self, area: TextInputArea) -> Result<(), NcStatus> {
        self.window.text_input(area)
    }
}

#[com_object(interfaces(IRenderHost))]
/// Observes recording geometry while leaving maintenance/import preparation with the real host.
struct ObservedHost {
    /// Real native graphics authority; no resource or target identity is recreated by the fixture.
    host: ComPtr<IRenderHostAbi>,
    /// COM's shared host/broker methods publish these small test observations through short local borrows.
    roots: Rc<RefCell<Vec<RenderViewRoot>>>,
}
impl IRenderDriverBound for ObservedHost {
    /// This forwarding host remains bound to the actual selected provider's driver.
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        self.host.driver_identity()
    }
}
impl IRenderHost for ObservedHost {
    /// Keep real completion/retirement behavior before client geometry creation.
    fn maintain_result(&self) -> Result<ComPtr<IRenderOutcomeAbi>, NcStatus> {
        self.host.maintain_result()
    }
    /// Ordinary image imports still pass through the native host's permission/identity checks.
    fn import_texture_result(
        &self,
        resources: ComRef<'_, IRenderResourcesAbi>,
        image: ComRef<'_, ITextureAbi>,
    ) -> Result<ComBox<ITextureImportResultAbi>, NcStatus> {
        self.host.import_texture_result(resources, image)
    }
    /// Native preparation errors remain visible rather than being replaced by fixture success.
    fn prepare_texture_result(
        &self,
        resources: ComRef<'_, IRenderResourcesAbi>,
        texture: ComRef<'_, IGpuBufferAbi>,
    ) -> Result<ComPtr<IRenderOutcomeAbi>, NcStatus> {
        self.host.prepare_texture_result(resources, texture)
    }
    /// Add observation around the real broker without changing any session or sealed-block identity.
    fn recording_broker(&self) -> Result<ComPtr<IRenderViewBrokerAbi>, NcStatus> {
        ObservedBroker::from_value(ObservedBroker {
            broker: self.host.recording_broker()?,
            roots: self.roots.clone(),
        })
        .query()
    }
}

#[com_object(interfaces(IRenderViewBroker))]
/// Returns a bounded recording owner around the actual provider's frame.
struct ObservedBroker {
    /// Canonical broker remains responsible for operation gates and group accounting.
    broker: ComPtr<IRenderViewBrokerAbi>,
    /// The test retains observations after the recording owner has dropped.
    roots: Rc<RefCell<Vec<RenderViewRoot>>>,
}
impl IRenderViewBroker for ObservedBroker {
    /// Preserve the real broker's admission and cleanup while capturing its view arguments.
    fn begin_frame(&self) -> Result<ComBox<IRenderRecordingFrameAbi>, NcStatus> {
        ObservedFrame::into_box(ObservedFrame {
            frame: self.broker.begin_frame()?,
            roots: self.roots.clone(),
        })
        .query()
        .map_err(|(status, _)| status)
    }
}

#[com_object(interfaces(IRenderRecordingFrame), ownership=exclusive)]
/// Owns the real exclusive frame; returned sessions and sealed blocks remain entirely native-provider objects.
struct ObservedFrame {
    /// Drop/finish/abort retain their normal revocation and immutable-block ownership semantics.
    frame: ComBox<IRenderRecordingFrameAbi>,
    /// Only metadata is recorded; no renderer resource is retained in the observation vector.
    roots: Rc<RefCell<Vec<RenderViewRoot>>>,
}
impl IRenderRecordingFrame for ObservedFrame {
    /// End the short observation borrow before invoking the foreign provider's recording method.
    fn begin_view(&mut self, root: RenderViewRoot, resources: ComRef<'_, IRenderResourcesAbi>) -> Result<ComBox<IRenderViewSessionAbi>, NcStatus> {
        self.roots.borrow_mut().push(root);
        self.frame.begin_view(root, resources)
    }
    /// A complete native frame still checks that every session was harvested or cancelled.
    fn finish(&mut self) -> Result<(), NcStatus> {
        self.frame.finish()
    }
    /// Abandonment delegates real payload revocation and releases the same frame's quota charges.
    fn abort(&mut self) -> Result<(), NcStatus> {
        self.frame.abort()
    }
}
impl IRenderClientFactory for ObservedWindow {
    /// Delegate unchanged target/permission arguments to the real authenticated factory.
    fn create_client(
        &self,
        limits: RenderClientLimits,
        surface: ComRef<'_, IRenderSurfaceAbi>,
        permissions: RenderClientPermissions,
    ) -> Result<ComBox<IRenderClientDomainAbi>, NcStatus> {
        self.window.query::<IRenderClientFactoryAbi>()?.create_client(limits, surface, permissions)
    }
}
impl IRenderSurfaceFactory for ObservedWindow {
    /// Returned targets belong to the real device; the test introduces no alternate surface implementation.
    fn create_surface(&self, desc: RenderSurfaceDesc) -> Result<ComPtr<IRenderSurfaceAbi>, NcStatus> {
        self.window.query::<IRenderSurfaceFactoryAbi>()?.create_surface(desc)
    }
}
impl IRenderer for ObservedWindow {
    /// Native submission enforces actual pixel dimensions and validates every recorded view's bounds.
    fn render_result(&self, info: RenderInfo, stream: ComRef<'_, IRenderStreamAbi>) -> Result<ComPtr<IRenderOutcomeAbi>, NcStatus> {
        let report = self.window.query::<IRendererAbi>()?.render_result(info, stream)?;
        if neocogi_com::NcStatus::from_code(report.status()?).is_ok() {
            self.submissions.set(self.submissions.get() + 1);
        }
        Ok(report)
    }
}

/// A genuine larger framebuffer exercises the same mismatch as HiDPI without assuming a display's scale.
/// Three successful native frames and one query per iteration prove the initial request is not reused.
#[test]
#[ignore = "requires a native display/adapter; Glow also permits SDL_VIDEODRIVER=offscreen"]
fn triangle_uses_current_drawable_pixels_and_suspended_iterations_do_not_consume_frames() {
    #[cfg(feature = "dynamic")]
    let requested = Some(stage_test_plugins().name());
    #[cfg(feature = "static")]
    let requested = None;
    let platform = neocogi_platform_examples::acquire_platform(requested).unwrap();
    for suspended in [false, true] {
        let window = platform
            .create_window(
                NcCopyFromBufferOffset::new(0, b"triangle drawable regression"),
                Size { width: 640, height: 480 },
            )
            .unwrap();
        let actual = window.size().unwrap();
        assert_ne!(actual, triangle::INITIAL_SIZE, "this regression must exercise a real extent mismatch");
        let size_calls = Rc::new(Cell::new(0));
        let submissions = Rc::new(Cell::new(0));
        let roots = Rc::new(RefCell::new(Vec::new()));
        let observed: ComPtr<IPlatformWindowAbi> = ObservedWindow::from_value(ObservedWindow {
            window,
            size_calls: size_calls.clone(),
            submissions: submissions.clone(),
            initially_suspended: suspended,
            roots: roots.clone(),
        })
        .query()
        .unwrap();
        triangle::render_frames(observed, 3).unwrap();
        assert_eq!(submissions.get(), 3);
        let bounds = Rect {
            x: 0,
            y: 0,
            width: actual.width,
            height: actual.height,
        };
        assert_eq!(roots.borrow().len(), 3);
        for root in roots.borrow().iter() {
            assert_eq!(root.client, bounds, "client recording uses the actual pixel extent");
            assert_eq!(root.target, bounds, "target recording uses the same extent as submission");
        }
        assert_eq!(
            size_calls.get(),
            3 + u32::from(suspended),
            "query size once before recording each attempted frame"
        );
    }
}
