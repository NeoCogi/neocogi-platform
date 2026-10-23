//! Wide rectangles cross the real importer and native mesh renderer without overflow or skipping.
use super::*;

#[com_object(interfaces(IRenderStream))]
/// One authenticated scene, with no texture or atlas vertices to obscure the mesh result.
struct MeshStream {
    /// Exact item geometry retained through the generated bulk-copy interface.
    batch: RenderBatch,
    /// Device-owned scene keeps its immutable geometry alive through submission.
    scene: ComPtr<IRenderItemAbi>,
}
impl IRenderStream for MeshStream {
    /// The mesh-only operation has no ordinary UI geometry.
    fn vertices(&self, _: u64, _: NcOutputWriter<'_, RenderVertex>) -> Result<u32, NcStatus> {
        Ok(0)
    }
    /// Preserve the wide content rectangle and its independently contained visible clip.
    fn batches(&self, _: u64, mut output: NcOutputWriter<'_, RenderBatch>) -> Result<u32, NcStatus> {
        output.push(self.batch)?;
        Ok(1)
    }
    /// There are no texture batches in this stream.
    fn texture(&self, _: u64, _: u32) -> Result<ComPtr<ITextureAbi>, NcStatus> {
        Err(NC_E_INVALID_ARG)
    }
    /// Only the sole item batch may resolve the renderer-owned scene.
    fn item(&self, _: u64, index: u32) -> Result<ComPtr<IRenderItemAbi>, NcStatus> {
        if index != 0 {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(self.scene.clone())
    }
}

/// Accepted wide x/y edges draw pixels and leave the following ordinary frame usable.
/// The bounded reference checks both widened arithmetic and fitting to the visible client area.
#[test]
#[ignore = "requires SDL GL; run with SDL_VIDEODRIVER=offscreen"]
fn wide_mesh_edges_draw_and_recover_through_generated_com() {
    let _session = native_session();
    let size = Size { width: 32, height: 32 };
    let assets = neocogi_demo_assets::load_bundle(crate::host_runtime()).unwrap();
    let atlas = assets.image().unwrap();
    let native = PlatformObject::default().window("wide mesh arithmetic", size, atlas.as_com_ref()).unwrap();
    let device = Rc::downgrade(&native.device);
    let window = WindowObject::from_value(native);
    let renderer: ComPtr<IRendererAbi> = window.query().unwrap();
    let scenes: ComPtr<IMeshScenesAbi> = window.query().unwrap();
    let scene = scenes.create_scene().unwrap();
    mesh(&scene);
    let item: ComPtr<IRenderItemAbi> = scene.query().unwrap();
    let bounds = Rect { x: 0, y: 0, width: 32, height: 32 };
    let info = RenderInfo {
        size,
        clear: 0xff000000,
        invocation: 1,
        vertex_count: 0,
        batch_count: 1,
    };
    // Capture only after COM render returns and has released its exclusive device access.
    let draw = |rect, clip| {
        let stream: ComPtr<IRenderStreamAbi> = MeshStream::from_value(MeshStream {
            batch: RenderBatch::Item { rect, clip },
            scene: item.clone(),
        })
        .query()
        .unwrap();
        renderer.render(info, atlas.as_com_ref(), stream.as_com_ref()).unwrap();
        let owner = device.upgrade().unwrap();
        let mut access = owner.access().unwrap();
        let pixels = access.state().gpu.last_pixels().to_vec();
        assert_eq!(pixels.len(), 32 * 32 * 4);
        assert!(
            pixels.as_chunks::<4>().0.iter().any(|pixel| *pixel != [0, 0, 0, 255]),
            "the mesh must emit visible pixels"
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
