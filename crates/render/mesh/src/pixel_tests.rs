//! Shared pixel expectations for the production mesh renderers; no native handle crosses this layer.
use super::*;
use neocogi_render_core::prelude::Recti;
use rs_math3d::{Vec2f, Vec3f};

/// A deliberately asymmetric triangle exposes viewport translations, scaling, and vertical flips.
pub(crate) fn triangle_command() -> MeshCommand {
    MeshCommand::DrawMesh(MeshSubmission {
        mesh: MeshBuffers::from_vecs(
            [(-0.7, -0.6), (0.6, -0.5), (-0.2, 0.8)]
                .map(|(x, y)| MeshVertex {
                    position: Vec3f::new(x, y, 0.).into(),
                    normal: Vec3f::new(0., 0., 1.).into(),
                    uv: Vec2f::new(0., 0.).into(),
                })
                .to_vec(),
            vec![0, 1, 2],
        ),
        pvm: Mat4f::identity(),
        view_model: Mat4f::identity(),
    })
}

/// Offscreen edges and interior scissors must refit identically to a full client of the visible size.
/// Repeated ordinary frames also prove that an item's viewport/scissor cannot leak into later frames.
pub(crate) fn assert_visible_fit(mut draw: impl FnMut(Recti, Recti) -> Vec<u8>) {
    let bounds = Recti::new(0, 0, 32, 32);
    let ordinary = draw(bounds, bounds);
    assert!(ordinary.chunks_exact(4).any(|pixel| pixel != [0, 0, 0, 255]));
    for (content, visible) in [
        (Recti::new(-16, 0, 32, 32), Recti::new(0, 0, 16, 32)),
        (Recti::new(16, 0, 32, 32), Recti::new(16, 0, 16, 32)),
        (Recti::new(0, -16, 32, 32), Recti::new(0, 0, 32, 16)),
        (Recti::new(0, 16, 32, 32), Recti::new(0, 16, 32, 16)),
        (bounds, Recti::new(7, 5, 19, 13)),
        (Recti::new(1, 1, i32::MAX, i32::MAX), Recti::new(1, 1, 31, 31)),
    ] {
        let reference = draw(visible, visible);
        assert!(reference.chunks_exact(4).any(|pixel| pixel != [0, 0, 0, 255]));
        assert_eq!(draw(content, visible), reference, "visible client {visible:?}");
        assert_eq!(draw(bounds, bounds), ordinary, "item state must not leak into the next frame");
    }
    assert!(draw(bounds, Recti::new(0, 0, 0, 0)).chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 255]));
}
