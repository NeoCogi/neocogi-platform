//! Native acceptance uses the production platform constructor and real COM resource calls.
//! Run explicitly with a usable SDL video driver; ordinary unit runs do not open OS windows.
use super::*;
use nc_com::{NcCopyToBufferOffset, INcStringComPtrExt};

/// Shared ordered stream fixture can inject native reentry and release a resource during bulk copying.
struct Input {
    /// Complete copied geometry; three texture quads share the same native coordinate convention.
    vertices: Vec<RenderVertex>,
    /// Validated pointer-free painter order.
    batches: Vec<RenderBatchAbi>,
    /// Background image authenticated by the tested renderer.
    background: ComPtr<ITextureAbi>,
    /// Later overlay image proves that custom GPU work does not move to the end of the frame.
    overlay: ComPtr<ITextureAbi>,
    /// An optional foreign texture tests image ownership independently of scene ownership.
    foreign_texture: RefCell<Option<ComPtr<ITextureAbi>>>,
    /// Scene can be replaced by a foreign owner's capability to inject a late preparation failure.
    scene: RefCell<ComPtr<IRenderItemAbi>>,
    /// A harmless empty stream supplies nested calls without creating a self-owning COM cycle.
    empty: ComPtr<IRenderStreamAbi>,
    /// Renderer capability reentered during vertices(), before any native frame may be acquired.
    renderer: ComPtr<IRendererAbi>,
    /// Editable scene must reject mutations while its owner is preparing a complete frame.
    editing: ComPtr<IMeshSceneAbi>,
    /// Final release occurs while native state is detached, exercising deferred texture deletion.
    retire: RefCell<Option<ComPtr<ITextureAbi>>>,
    /// Observes retirement without retaining the device through fixture bookkeeping.
    device: Weak<Device>,
    /// Confirms the source actually executed the nested operations.
    reentries: Cell<u32>,
}
#[com_class(interfaces(IRenderStream))]
/// A source owning no geometry models an empty frame for reentry attempts.
struct Empty;
impl IRenderStream for Empty {
    /// Nested render must fail before consulting even this harmless source.
    fn vertices(&self, _: u64, _: NcCopyToBufferOffset<'_, RenderVertex>) -> Result<u32, NcStatus> {
        panic!("nested renderer read a stream")
    }
    /// No record read is allowed before the nested render gate rejects.
    fn batches(&self, _: u64, _: NcCopyToBufferOffset<'_, RenderBatchAbi>) -> Result<u32, NcStatus> {
        panic!("nested renderer read batches")
    }
    /// The empty source has no texture capability.
    fn texture(&self, _: u64, _: u32) -> Result<ComPtr<ITextureAbi>, NcStatus> {
        Err(NC_E_INVALID_ARG)
    }
    /// The empty source has no scene capability.
    fn item(&self, _: u64, _: u32) -> Result<ComPtr<IRenderItemAbi>, NcStatus> {
        Err(NC_E_INVALID_ARG)
    }
}
#[com_class(interfaces(IRenderStream))]
/// Foreign-style input uses real generated vtable calls into the native implementation.
struct Stream {
    /// Shared owned fixture data; no stack borrow crosses COM.
    input: Rc<Input>,
}
impl IRenderStream for Stream {
    /// Reentry and resource destruction happen while the renderer owns detached preparation storage.
    fn vertices(&self, _: u64, mut output: NcCopyToBufferOffset<'_, RenderVertex>) -> Result<u32, NcStatus> {
        assert_eq!(self.input.renderer.render(info(), self.input.empty.as_com_ref()), Err(UI_E_BUSY));
        let factory: ComPtr<ITextureFactoryAbi> = self.input.renderer.query()?;
        assert_eq!(
            factory
                .create_texture(Size { width: 1, height: 1 }, NcCopyFromBufferOffset::new(0, &[0; 4]))
                .unwrap_err(),
            UI_E_BUSY
        );
        assert_eq!(
            self.input.editing.set_transforms(SceneTransforms {
                pvm: Matrix4::identity(),
                view_model: Matrix4::identity()
            }),
            Err(UI_E_BUSY)
        );
        let retire = self.input.retire.borrow_mut().take();
        let retired = retire.is_some();
        drop(retire);
        if retired {
            assert_eq!(self.input.device.upgrade().unwrap().retired.borrow().len(), 1);
        }
        self.input.reentries.set(self.input.reentries.get() + 1);
        output.as_mut_slice().copy_from_slice(&self.input.vertices);
        Ok(self.input.vertices.len() as u32)
    }
    /// One bulk read supplies complete ordering metadata.
    fn batches(&self, _: u64, mut output: NcCopyToBufferOffset<'_, RenderBatchAbi>) -> Result<u32, NcStatus> {
        output.as_mut_slice().copy_from_slice(&self.input.batches);
        Ok(self.input.batches.len() as u32)
    }
    /// Overall batch indices select their exact image rather than using a separate untyped resource list.
    fn texture(&self, _: u64, index: u32) -> Result<ComPtr<ITextureAbi>, NcStatus> {
        match index {
            0 => Ok(self.input.foreign_texture.borrow().as_ref().unwrap_or(&self.input.background).clone()),
            2 => Ok(self.input.overlay.clone()),
            _ => Err(NC_E_INVALID_ARG),
        }
    }
    /// Cloning the known local scene has no application callback; replacement occurs only between renders.
    fn item(&self, _: u64, index: u32) -> Result<ComPtr<IRenderItemAbi>, NcStatus> {
        if index != 1 {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(self.input.scene.borrow().clone())
    }
}
/// Small physical viewport keeps this GPU lifetime/order gate fast on software drivers too.
const SIZE: Size = Size { width: 64, height: 64 };
/// Exact info for background image, mesh and a small foreground image.
fn info() -> RenderInfo {
    RenderInfo {
        size: SIZE,
        clear: 0xff000000,
        invocation: 1,
        vertex_count: 8,
        batch_count: 3,
    }
}
/// Creates a clockwise full-image quad in the final UI vertex format.
fn quad(rect: Rect) -> [RenderVertex; 4] {
    let (x, y, right, bottom) = (rect.x as f32, rect.y as f32, (rect.x + rect.width) as f32, (rect.y + rect.height) as f32);
    [
        RenderVertex { x, y, u: 0., v: 0., rgba: 0xffffffff },
        RenderVertex {
            x: right,
            y,
            u: 1.,
            v: 0.,
            rgba: 0xffffffff,
        },
        RenderVertex {
            x: right,
            y: bottom,
            u: 1.,
            v: 1.,
            rgba: 0xffffffff,
        },
        RenderVertex {
            x,
            y: bottom,
            u: 0.,
            v: 1.,
            rgba: 0xffffffff,
        },
    ]
}
/// Uploads a triangle using the same application contract as the independently loaded demo host.
fn mesh(scene: &ComPtr<IMeshSceneAbi>) {
    let vertices = [
        MeshVertex {
            position: [-0.9, -0.9, 0.].into(),
            normal: [0., 0., 1.].into(),
            uv: [0., 0.].into(),
        },
        MeshVertex {
            position: [0.9, -0.9, 0.].into(),
            normal: [0., 0., 1.].into(),
            uv: [1., 0.].into(),
        },
        MeshVertex {
            position: [0., 0.9, 0.].into(),
            normal: [0., 0., 1.].into(),
            uv: [0.5, 1.].into(),
        },
    ];
    scene
        .set_mesh(NcCopyFromBufferOffset::new(0, &vertices), NcCopyFromBufferOffset::new(0, &[0, 1, 2]))
        .unwrap();
}
/// Real native resources survive reentry, owner rejection, retries and public-window release with exact cleanup.
#[test]
#[ignore = "requires a native SDL display; use SDL_VIDEODRIVER=offscreen for Glow or a native display for Vulkan/WGPU"]
fn native_order_ownership_reentry_and_final_release() {
    let platform = Platform::default();
    let native = platform.window("COM GPU acceptance", SIZE).unwrap();
    let device = Rc::downgrade(&native.device);
    let unknown = NativeWindow::from_value(native);
    let renderer: ComPtr<IRendererAbi> = unknown.query().unwrap();
    let factory: ComPtr<ITextureFactoryAbi> = unknown.query().unwrap();
    let scenes: ComPtr<IDemoScenesAbi> = unknown.query().unwrap();
    let scene = scenes.create_scene().unwrap();
    mesh(&scene);
    let base: ComPtr<IRenderItemAbi> = scene.query().unwrap();
    assert_eq!(identity(&scene).unwrap(), identity(&base).unwrap());
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
    let input = Rc::new(Input {
        vertices: quad(bounds).into_iter().chain(quad(Rect { x: 28, y: 28, width: 8, height: 8 })).collect(),
        batches: vec![
            RenderBatch::Texture { first: 0 }.into(),
            RenderBatch::Item { rect: bounds, clip: bounds }.into(),
            RenderBatch::Texture { first: 4 }.into(),
        ],
        background: background.clone(),
        foreign_texture: RefCell::new(None),
        overlay,
        scene: RefCell::new(base),
        empty: Empty::from_value(Empty).query().unwrap(),
        renderer: renderer.clone(),
        editing: scene.clone(),
        retire: RefCell::new(Some(spare)),
        device: device.clone(),
        reentries: Cell::new(0),
    });
    let stream: ComPtr<IRenderStreamAbi> = Stream::from_value(Stream { input: input.clone() }).query().unwrap();
    renderer.render(info(), stream.as_com_ref()).unwrap();
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
    // The foreign scene has a valid ABI but belongs to a different native window/context.
    let foreign = platform.window("COM foreign resource owner", SIZE).unwrap();
    let foreign_device = Rc::downgrade(&foreign.device);
    let foreign_scene = IDemoScenes::create_scene(&foreign).unwrap();
    let foreign_texture = ITextureFactory::create_texture(&foreign, Size { width: 1, height: 1 }, NcCopyFromBufferOffset::new(0, &[0; 4])).unwrap();
    *input.foreign_texture.borrow_mut() = Some(foreign_texture);
    assert_eq!(renderer.render(info(), stream.as_com_ref()), Err(UI_E_OWNER));
    let foreign_texture = input.foreign_texture.borrow_mut().take();
    drop(foreign_texture);
    *input.scene.borrow_mut() = foreign_scene.query().unwrap();
    assert_eq!(renderer.render(info(), stream.as_com_ref()), Err(UI_E_OWNER));
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
    *input.scene.borrow_mut() = scene.query().unwrap();
    // A failed frame must not poison the next explicit native submission.
    renderer.render(info(), stream.as_com_ref()).unwrap();
    drop((foreign_scene, foreign));
    assert!(foreign_device.upgrade().is_none());
    drop((stream, input, scene, factory, scenes, renderer, unknown, platform));
    assert!(device.upgrade().is_some(), "image must retain window and context after public window release");
    drop(background);
    assert!(device.upgrade().is_none(), "no registry or idle prepared-frame cycle may retain the device");
}
