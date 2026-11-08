//! Wide rectangles cross the real importer and generic renderer without overflow or skipping.
use super::*;

#[com_object(interfaces(IRenderStream))]
/// One authenticated view, with no texture or atlas vertices to obscure the graphics result.
pub(super) struct ViewStream {
    /// Canonical immutable root and generic commands survive the generated stream calls.
    pub(super) blocks: Vec<ComBox<IRenderViewBlockAbi>>,
}
impl IRenderStream for ViewStream {
    /// Borrows the only sealed contribution through the exclusive weak protocol.
    fn view(&self, _: u64, index: u32) -> Result<neocogi_com::WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        self.blocks.get(index as usize).map(ComBox::downgrade).ok_or(NC_E_INVALID_ARG)
    }
}

/// Accepted wide x/y edges draw pixels and leave the following ordinary frame usable.
/// The bounded reference checks both widened arithmetic and fitting to the visible client area.
#[test]
#[ignore = "requires SDL GL; run with SDL_VIDEODRIVER=offscreen"]
fn wide_view_edges_draw_and_recover_through_generated_com() {
    let _session = native_session();
    let size = Size { width: 32, height: 32 };
    let native = PlatformObject::new(
        neocogi_platform::compiler::CompilerObject::from_value(neocogi_platform::compiler::CompilerObject)
            .query()
            .unwrap(),
    )
    .window("wide graphics arithmetic", size)
    .unwrap();
    let device = Rc::downgrade(&native.device);
    let window = WindowObject::from_value(native);
    let renderer: ComPtr<IRendererAbi> = window.query().unwrap();
    let clients: ComPtr<IRenderClientFactoryAbi> = window.query().unwrap();
    let controller = framebuffer_group(&clients, RenderClientLimits::default(), RenderClientPermissions::default());
    let resources = controller.resources().unwrap();
    let broker = window
        .query::<IPlatformWindowAbi>()
        .unwrap()
        .graphics_host()
        .unwrap()
        .recording_broker()
        .unwrap();
    let bounds = Rect { x: 0, y: 0, width: 32, height: 32 };
    let info = RenderInfo {
        size,
        clear: 0xff000000,
        invocation: 1,
        block_count: 1,
    };
    // Capture only after COM render returns and has released its exclusive device access.
    let draw = |rect: Rect, clip: Rect| {
        let client = Rect {
            x: (i64::from(clip.x) - i64::from(rect.x)) as i32,
            y: (i64::from(clip.y) - i64::from(rect.y)) as i32,
            width: clip.width,
            height: clip.height,
        };
        let block = graphics(&resources, &broker, RenderViewRoot { client, target: clip });
        let stream: ComPtr<IRenderStreamAbi> = ViewStream::from_value(ViewStream { blocks: vec![block] }).query().unwrap();
        renderer.render(info, stream.as_com_ref()).map_err(|error| error.status).unwrap();
        let owner = device.upgrade().unwrap();
        let mut access = owner.access().unwrap();
        let pixels = access.state().gpu.last_pixels().to_vec();
        assert_eq!(pixels.len(), 32 * 32 * 4);
        assert!(
            pixels.as_chunks::<4>().0.iter().any(|pixel| *pixel != [0, 0, 0, 255]),
            "the graphics must emit visible pixels"
        );
        pixels
    };
    let ordinary = draw(bounds, bounds);
    for (wide, visible) in [
        (Rect { x: -16, ..bounds }, Rect { width: 16, ..bounds }),
        (Rect { y: -16, ..bounds }, Rect { height: 16, ..bounds }),
        (Rect { x: 16, ..bounds }, Rect { x: 16, width: 16, ..bounds }),
        (Rect { y: 16, ..bounds }, Rect { y: 16, height: 16, ..bounds }),
        (bounds, Rect { x: 7, y: 5, width: 19, height: 13 }),
        (Rect { x: 1, width: i32::MAX, ..bounds }, Rect { x: 1, width: 31, ..bounds }),
        (Rect { y: 1, height: i32::MAX, ..bounds }, Rect { y: 1, height: 31, ..bounds }),
        (
            Rect {
                x: 1,
                y: 1,
                width: i32::MAX,
                height: i32::MAX,
            },
            Rect { x: 1, y: 1, width: 31, height: 31 },
        ),
        (
            Rect {
                x: i32::MIN + 17,
                width: i32::MAX,
                ..bounds
            },
            Rect { width: 16, ..bounds },
        ),
        (
            Rect {
                y: i32::MIN + 17,
                height: i32::MAX,
                ..bounds
            },
            Rect { height: 16, ..bounds },
        ),
    ] {
        let bounded_reference = draw(visible, visible);
        assert_eq!(draw(wide, visible), bounded_reference);
        assert_eq!(draw(bounds, bounds), ordinary, "a wide item must not poison the next frame");
    }
}
