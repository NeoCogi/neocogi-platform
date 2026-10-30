//! Native acceptance uses the production platform constructor and real COM resource calls.
//! Run explicitly with a usable SDL video driver; ordinary unit runs do not open OS windows.
use super::*;
use neocogi_com::{NcOutputWriter, INcStringComPtrExt};
use neocogi_com::ComBox;
use neocogi_render_gpu::source::{Bytes, PipelineDesc};

#[cfg(feature = "glow")]
mod view_geometry;

/// SDL permits one process-wide video owner and event pump. Each fixture keeps
/// this permit until all of its native windows, contexts and resources drop.
static NATIVE_SESSION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Serializes complete native fixture lifetimes even under the parallel harness.
fn native_session() -> std::sync::MutexGuard<'static, ()> {
    NATIVE_SESSION.lock().expect("earlier native fixture panicked")
}

/// Shared ordered stream fixture can inject native reentry and release a resource during bulk copying.
struct Input {
    /// Immutable atlas passed explicitly to every native render, including reentry probes.
    atlas: ComPtr<IRgbaImageAbi>,
    /// Atlas background and image quads share the same native coordinate convention.
    vertices: Vec<RenderVertex>,
    /// Validated pointer-free painter order.
    batches: Vec<RenderBatch>,
    /// Background image authenticated by the tested renderer.
    background: ComPtr<ITextureAbi>,
    /// Later overlay image proves that custom GPU work does not move to the end of the frame.
    overlay: ComPtr<ITextureAbi>,
    /// An optional foreign texture tests image ownership independently of scene ownership.
    foreign_texture: RefCell<Option<ComPtr<ITextureAbi>>>,
    /// A foreign block injects a late canonical-provenance failure after an image was resolved.
    block: RefCell<ComBox<IRenderViewBlockAbi>>,
    /// A harmless empty stream supplies nested calls without creating a self-owning COM cycle.
    empty: ComPtr<IRenderStreamAbi>,
    /// Renderer capability reentered during vertices(), before any native frame may be acquired.
    renderer: ComPtr<IRendererAbi>,
    /// Resource creation must reject reentry during native frame preparation.
    editing: ComPtr<IRenderResourcesAbi>,
    /// Final release occurs while native state is detached, exercising deferred texture deletion.
    retire: RefCell<Option<ComPtr<ITextureAbi>>>,
    /// Observes retirement without retaining the device through fixture bookkeeping.
    device: Weak<Device>,
    /// Confirms the source actually executed the nested operations.
    reentries: Cell<u32>,
}
#[com_object(interfaces(IRenderStream))]
/// A source owning no geometry models an empty frame for reentry attempts.
struct Empty;
impl IRenderStream for Empty {
    /// This fixture carries no generic GPU block.
    fn view(&self, _: u64, _: u32) -> Result<neocogi_com::WeakComBox<neocogi_render_core::abi::IRenderViewBlockAbi>, NcStatus> {
        Err(neocogi_com::NC_E_INVALID_ARG)
    }

    /// Nested render must fail before consulting even this harmless source.
    fn vertices(&self, _: u64, _: NcOutputWriter<'_, RenderVertex>) -> Result<u32, NcStatus> {
        panic!("nested renderer read a stream")
    }
    /// No record read is allowed before the nested render gate rejects.
    fn batches(&self, _: u64, _: NcOutputWriter<'_, RenderBatch>) -> Result<u32, NcStatus> {
        panic!("nested renderer read batches")
    }
    /// The empty source has no texture capability.
    fn texture(&self, _: u64, _: u32) -> Result<ComPtr<ITextureAbi>, NcStatus> {
        Err(NC_E_INVALID_ARG)
    }
}
#[com_object(interfaces(IRenderStream))]
/// Foreign-style input uses real generated vtable calls into the native implementation.
struct Stream {
    /// Shared owned fixture data; no stack borrow crosses COM.
    input: Rc<Input>,
}
impl IRenderStream for Stream {
    /// The overall batch index selects a sealed weak observer without a semantic item adapter.
    fn view(&self, _: u64, index: u32) -> Result<neocogi_com::WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        if index != 2 {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(self.input.block.borrow().downgrade())
    }

    /// Reentry and resource destruction happen while the renderer owns detached preparation storage.
    fn vertices(&self, _: u64, mut output: NcOutputWriter<'_, RenderVertex>) -> Result<u32, NcStatus> {
        assert_eq!(
            self.input.renderer.render(info(), self.input.atlas.as_com_ref(), self.input.empty.as_com_ref()),
            Err(RENDER_E_BUSY)
        );
        let factory: ComPtr<ITextureFactoryAbi> = self.input.renderer.query()?;
        assert_eq!(
            factory
                .create_texture(Size { width: 1, height: 1 }, NcCopyFromBufferOffset::new(0, &[0; 4]))
                .unwrap_err(),
            RENDER_E_BUSY
        );
        assert_eq!(
            self.input
                .editing
                .create_buffer(
                    GpuBufferDesc { role: GpuBufferRole::Vertex, byte_len: 4 },
                    Bytes::source(vec![0; 4]).as_com_ref()
                )
                .err(),
            Some(RENDER_E_BUSY)
        );
        let retire = self.input.retire.borrow_mut().take();
        let retired = retire.is_some();
        drop(retire);
        if retired {
            assert_eq!(self.input.device.upgrade().unwrap().retired.borrow().len(), 1);
        }
        self.input.reentries.set(self.input.reentries.get() + 1);
        output.extend_from_slice(&self.input.vertices)?;
        Ok(self.input.vertices.len() as u32)
    }
    /// One bulk read supplies complete ordering metadata.
    fn batches(&self, _: u64, mut output: NcOutputWriter<'_, RenderBatch>) -> Result<u32, NcStatus> {
        output.extend_from_slice(&self.input.batches)?;
        Ok(self.input.batches.len() as u32)
    }
    /// Overall batch indices select their exact image rather than using a separate untyped resource list.
    fn texture(&self, _: u64, index: u32) -> Result<ComPtr<ITextureAbi>, NcStatus> {
        match index {
            1 => Ok(self.input.foreign_texture.borrow().as_ref().unwrap_or(&self.input.background).clone()),
            3 => Ok(self.input.overlay.clone()),
            _ => Err(NC_E_INVALID_ARG),
        }
    }
}
/// Small physical viewport keeps this GPU lifetime/order gate fast on software drivers too.
const SIZE: Size = Size { width: 64, height: 64 };
/// Exact info for atlas background, background image, mesh and a small foreground image.
fn info() -> RenderInfo {
    RenderInfo {
        size: SIZE,
        clear: 0xff000000,
        invocation: 1,
        vertex_count: 14,
        batch_count: 4,
    }
}

/// Captures a generic procedural triangle; all client shader semantics live in this fixture.
fn graphics(resources: &ComPtr<IRenderResourcesAbi>, broker: &ComPtr<IRenderViewBrokerAbi>, root: RenderViewRoot) -> ComBox<IRenderViewBlockAbi> {
    let code = Bytes::source(
        br#"
        @vertex fn vs(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
            let p = array<vec2<f32>,3>(vec2(-0.9,-0.9),vec2(0.9,-0.9),vec2(0.0,0.9));
            return vec4(p[i],0.5,1.0);
        }
        @fragment fn fs() -> @location(0) vec4<f32> { return vec4(0.0,0.0,1.0,1.0); }
    "#
        .to_vec(),
    );
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, code.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, code.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let descriptor = PipelineDesc::default().source().unwrap();
    let pipeline = resources.create_pipeline(descriptor.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let mut frame = broker.begin_frame().unwrap();
    let mut session = frame.begin_view(root, resources.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    block
}
/// Real native resources survive reentry, owner rejection, retries and public-window release with exact cleanup.
#[test]
#[ignore = "requires a native SDL display; use SDL_VIDEODRIVER=offscreen for Glow or a native display for Vulkan/WGPU"]
fn native_order_ownership_reentry_and_final_release() {
    let _session = native_session();
    let platform = PlatformObject::default();
    let assets = neocogi_demo_assets::load_bundle(crate::host_runtime()).unwrap();
    let atlas = assets.image().unwrap();
    let native = platform.window("COM GPU acceptance", SIZE, atlas.as_com_ref()).unwrap();
    let device = Rc::downgrade(&native.device);
    let unknown = WindowObject::from_value(native);
    {
        let window: ComPtr<IPlatformWindowAbi> = unknown.query().unwrap();
        assert!(unknown.query::<IRenderHostAbi>().is_err());
        let graphics = window.graphics_host().unwrap();
        assert!(graphics.query::<IPlatformWindowAbi>().is_err());
        assert!(graphics.query::<IRendererAbi>().is_err());
        assert!(graphics.query::<ITextureFactoryAbi>().is_err());
        assert!(graphics.query::<IRenderClientFactoryAbi>().is_err());
        assert!(graphics.query::<IRenderSurfaceFactoryAbi>().is_err());
        assert!(graphics.query::<ITextClipboardAbi>().is_err());
        let private_ui = graphics.create_ui_client(RenderClientLimits::default()).unwrap();
        assert!(private_ui.resources().unwrap().query::<IRenderHostAbi>().is_err());
        assert!(unknown.query::<ITextClipboardAbi>().is_err());
        let clipboard = window.clipboard().unwrap();
        assert!(clipboard.query::<IRendererAbi>().is_err());
        assert!(clipboard.query::<IPlatformWindowAbi>().is_err());
        assert!(clipboard.query::<ITextureFactoryAbi>().is_err());
        assert!(clipboard.query::<IRenderSurfaceFactoryAbi>().is_err());
        assert!(clipboard.query::<IRenderClientFactoryAbi>().is_err());
        assert!(clipboard.query::<IRenderHostAbi>().is_err());
    }
    input_delivery(&unknown, &device);
    let renderer: ComPtr<IRendererAbi> = unknown.query().unwrap();
    let factory: ComPtr<ITextureFactoryAbi> = unknown.query().unwrap();
    let clients: ComPtr<IRenderClientFactoryAbi> = unknown.query().unwrap();
    let controller = clients.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    let broker = unknown.query::<IPlatformWindowAbi>().unwrap().graphics_host().unwrap().recording_broker().unwrap();
    let root = Rect { x: 0, y: 0, width: 64, height: 64 };
    let block = graphics(&resources, &broker, RenderViewRoot { client: root, target: root });
    let background = factory
        .create_texture(Size { width: 1, height: 1 }, NcCopyFromBufferOffset::new(0, &[0, 255, 0, 255]))
        .unwrap();
    let overlay = factory
        .create_texture(Size { width: 1, height: 1 }, NcCopyFromBufferOffset::new(0, &[255, 0, 0, 255]))
        .unwrap();
    let spare = factory
        .create_texture(Size { width: 1, height: 1 }, NcCopyFromBufferOffset::new(0, &[0; 4]))
        .unwrap();
    let spare_key = identity(&spare).unwrap();
    let bounds = Rect { x: 0, y: 0, width: 64, height: 64 };
    // An opaque atlas backdrop must remain behind the images even when no custom item has
    // flushed the atlas batch yet. The previous texture/item/texture fixture missed this boundary.
    let backdrop = quad(bounds).map(|vertex| RenderVertex { u: 0., v: 0., rgba: 0xffff0000, ..vertex });
    let input = Rc::new(Input {
        atlas: atlas.clone(),
        vertices: [backdrop[0], backdrop[1], backdrop[2], backdrop[0], backdrop[2], backdrop[3]]
            .into_iter()
            .chain(quad(bounds))
            .chain(quad(Rect { x: 28, y: 28, width: 8, height: 8 }))
            .collect(),
        batches: vec![
            RenderBatch::Atlas { first: 0, count: 6 },
            RenderBatch::Texture { first: 6 },
            RenderBatch::View,
            RenderBatch::Texture { first: 10 },
        ],
        background: background.clone(),
        foreign_texture: RefCell::new(None),
        overlay,
        block: RefCell::new(block),
        empty: Empty::from_value(Empty).query().unwrap(),
        renderer: renderer.clone(),
        editing: resources.clone(),
        retire: RefCell::new(Some(spare)),
        device: device.clone(),
        reentries: Cell::new(0),
    });
    let stream: ComPtr<IRenderStreamAbi> = Stream::from_value(Stream { input: input.clone() }).query().unwrap();
    renderer.render(info(), atlas.as_com_ref(), stream.as_com_ref()).unwrap();
    assert_eq!(input.reentries.get(), 1);
    {
        let owner = device.upgrade().unwrap();
        assert!(owner.retired.borrow().is_empty());
        assert!(!owner.textures.borrow().contains_key(&spare_key));
        assert!(owner.render.borrow().as_ref().unwrap().prepared.batches().is_empty());
        #[cfg(feature = "glow")]
        {
            let mut access = owner.access().unwrap();
            let pixels = access.state().gpu.last_pixels();
            // Readback uses GL's bottom-up coordinates: center overlay is red, outside mesh is green.
            assert_eq!(&pixels[(32 * 64 + 32) * 4..][..4], &[255, 0, 0, 255]);
            assert_eq!(&pixels[(2 * 64 + 2) * 4..][..4], &[0, 255, 0, 255]);
            let mesh_pixel = &pixels[(20 * 64 + 32) * 4..][..4];
            assert_ne!(mesh_pixel, &[0, 255, 0, 255]);
            assert_ne!(mesh_pixel, &[255, 0, 0, 255]);
        }
    }
    // A structurally valid, shallow atlas exceeds every tested device's 2D texture width while
    // remaining below the portable byte limit. Rejection must leave the previous GPU atlas usable.
    let mut oversized = neocogi_demo_assets::bundle_data().unwrap();
    oversized.tables.size = Size { width: 131072, height: 1 };
    oversized.pixels.to_mut().fill(255);
    for glyph in oversized.tables.glyphs.to_mut() {
        glyph.rect = Rect { x: 0, y: 0, width: 1, height: 1 };
    }
    for icon in oversized.tables.icons.to_mut() {
        icon.rect = Rect { x: 0, y: 0, width: 1, height: 1 };
    }
    let oversized = oversized.into_revision(assets.as_com_ref()).unwrap();
    let oversized_atlas = oversized.image().unwrap();
    assert_eq!(renderer.prepare_atlas(oversized_atlas.as_com_ref()), Err(RENDER_E_BACKEND));
    assert_eq!(
        device.upgrade().unwrap().render.borrow().as_ref().unwrap().atlas.as_raw().object,
        atlas.as_raw().object
    );
    renderer.render(info(), atlas.as_com_ref(), stream.as_com_ref()).unwrap();
    // The foreign scene has a valid ABI but belongs to a different native window/context.
    let foreign = platform.window("COM foreign resource owner", SIZE, atlas.as_com_ref()).unwrap();
    let foreign_device = Rc::downgrade(&foreign.device);
    let foreign = WindowObject::from_value(foreign);
    let foreign_clients: ComPtr<IRenderClientFactoryAbi> = foreign.query().unwrap();
    let foreign_controller = foreign_clients.create_client(RenderClientLimits::default()).unwrap();
    let foreign_resources = foreign_controller.resources().unwrap();
    let foreign_broker = foreign.query::<IPlatformWindowAbi>().unwrap().graphics_host().unwrap().recording_broker().unwrap();
    let foreign_block = graphics(&foreign_resources, &foreign_broker, RenderViewRoot { client: root, target: root });
    let foreign_factory: ComPtr<ITextureFactoryAbi> = foreign.query().unwrap();
    let foreign_texture = foreign_factory
        .create_texture(Size { width: 1, height: 1 }, NcCopyFromBufferOffset::new(0, &[0; 4]))
        .unwrap();
    *input.foreign_texture.borrow_mut() = Some(foreign_texture);
    assert_eq!(renderer.render(info(), atlas.as_com_ref(), stream.as_com_ref()), Err(RENDER_E_OWNER));
    let foreign_texture = input.foreign_texture.borrow_mut().take();
    drop(foreign_texture);
    let original = input.block.replace(foreign_block);
    assert_eq!(renderer.render(info(), atlas.as_com_ref(), stream.as_com_ref()), Err(RENDER_E_OWNER));
    assert!(
        renderer
            .query::<IRenderDiagnosticsAbi>()
            .unwrap()
            .diagnostic()
            .unwrap()
            .to_string()
            .unwrap()
            .contains("frame preparation")
    );
    drop(input.block.replace(original));
    // A failed frame must not poison the next explicit native submission.
    renderer.render(info(), atlas.as_com_ref(), stream.as_com_ref()).unwrap();
    // Replaying sealed data and replacing it with another immutable block must keep ordering
    // and native lifetime valid without retaining any public shader/pipeline owner.
    for _ in 0..4 {
        renderer.render(info(), atlas.as_com_ref(), stream.as_com_ref()).unwrap();
    }
    let replacement = graphics(&resources, &broker, RenderViewRoot { client: root, target: root });
    drop(input.block.replace(replacement));
    for _ in 0..3 {
        renderer.render(info(), atlas.as_com_ref(), stream.as_com_ref()).unwrap();
    }
    drop((foreign_resources, foreign_controller, foreign_clients, foreign_broker, foreign_factory, foreign));
    assert!(foreign_device.upgrade().is_none());
    drop((stream, input, resources, controller, clients, broker, factory, renderer, unknown, platform));
    assert!(device.upgrade().is_some(), "image must retain window and context after public window release");
    drop(background);
    assert!(device.upgrade().is_none(), "no registry or idle prepared-frame cycle may retain the device");
}

#[com_object(interfaces(IInputSink))]
/// Concrete destination observes delivery, optionally reentering the same native window once.
struct InputProbe {
    /// Reentry target belongs to this short-lived test sink, never to a platform registration.
    window: Option<ComPtr<IPlatformWindowAbi>>,
    /// A harmless destination allows nested polling without giving the probe a reference to itself.
    empty: Option<ComPtr<IInputSinkAbi>>,
    /// Copied attempted inputs remain visible after the sink is released.
    received: Rc<RefCell<Vec<(UiInput, String)>>>,
    /// Injects one delivery error so the remaining native queue can be checked independently.
    fail: Cell<bool>,
}
impl IInputSink for InputProbe {
    /// Records the attempted transition before checking recursive polling and returning the injected error.
    fn input(&self, event: UiInput, text: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        self.received
            .borrow_mut()
            .push((event, std::str::from_utf8(text.as_slice()).unwrap().to_owned()));
        if let Some(window) = &self.window {
            assert_eq!(window.poll(self.empty.as_ref().unwrap().as_com_ref()), Err(PLATFORM_E_BUSY));
        }
        if self.fail.replace(false) { Err(NC_E_INVALID_ARG) } else { Ok(()) }
    }
}
/// Runs inside the existing single SDL/GPU test thread; no parallel native event pump is constructed.
fn input_delivery(unknown: &ComPtr<IUnknown>, device: &Weak<Device>) {
    let window: ComPtr<IPlatformWindowAbi> = unknown.query().unwrap();
    let received = Rc::new(RefCell::new(Vec::new()));
    let empty: ComPtr<IInputSinkAbi> = InputProbe::from_value(InputProbe {
        window: None,
        empty: None,
        received: received.clone(),
        fail: Cell::new(false),
    })
    .query()
    .unwrap();
    // Drain construction/focus events before inserting a deterministic pair of Unicode commits.
    while window.poll(empty.as_com_ref()).unwrap() == PollStatus::Event {}
    received.borrow_mut().clear();
    let owner = device.upgrade().unwrap();
    let id = owner.window.id();
    owner.host.queues.borrow_mut().get_mut(&id).unwrap().extend([
        sdl2::event::Event::TextInput {
            timestamp: 0,
            window_id: id,
            text: "first é".to_owned(),
        },
        sdl2::event::Event::TextInput {
            timestamp: 0,
            window_id: id,
            text: "second ☃".to_owned(),
        },
    ]);
    let sink: ComPtr<IInputSinkAbi> = InputProbe::from_value(InputProbe {
        window: Some(window.clone()),
        empty: Some(empty),
        received: received.clone(),
        fail: Cell::new(true),
    })
    .query()
    .unwrap();
    assert_eq!(window.poll(sink.as_com_ref()), Err(NC_E_INVALID_ARG));
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Event));
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    assert_eq!(
        &*received.borrow(),
        &[(UiInput::Text, "first é".to_owned()), (UiInput::Text, "second ☃".to_owned())]
    );
    assert_eq!(
        window.text_input(TextInputArea::Area(Rect { x: 0, y: 0, width: -1, height: 1 })),
        Err(NC_E_INVALID_ARG)
    );
    drop(sink);
    // No platform-owned sink reference remains after synchronous delivery.
    assert_eq!(Rc::strong_count(&received), 1);
}
