//! Real GL shader inputs and pixels guard the application-to-GPU representation boundary.

use super::*;
use neocogi_render_core::prelude::Dimensioni;

/// Exercise the actual upload, shader attributes, transforms, clipping and repeated cached draws.
#[test]
#[ignore = "requires a native GL ES 3 driver; run with SDL_VIDEODRIVER=offscreen"]
fn visible_client_rectangles_fit_the_scene_in_actual_pixels() {
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    video.gl_attr().set_context_profile(sdl2::video::GLProfile::GLES);
    video.gl_attr().set_context_version(3, 0);
    video.gl_attr().set_depth_size(24);
    video.gl_attr().set_alpha_size(8);
    let window = video.window("mesh vertex layout regression", 32, 32).opengl().hidden().build().unwrap();
    let _context = window.gl_create_context().unwrap();
    // SAFETY: SDL made this context current and supplies its entrypoints. All GL owners below
    // are dropped before the context and window, with no context switch during the test.
    let gl = Rc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) });
    let mut renderer = MeshRenderer::new();
    let MeshCommand::DrawMesh(submission) = crate::pixel_tests::triangle_command();
    let mut upload = None;
    crate::pixel_tests::assert_visible_fit(|content_area, view| {
        // SAFETY: The hidden window's default framebuffer and current ES 3 context are live.
        // Clear the full target rather than inheriting the previous item's scissor.
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.color_mask(true, true, true, true);
            gl.depth_mask(true);
            gl.clear_color(0., 0., 0., 1.);
            gl.clear_depth_f32(1.);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        }
        renderer
            .render(
                GlApplicationRenderContext {
                    gl: &gl,
                    framebuffer_size: (32, 32),
                    args: RenderItemArgs {
                        dimensions: Dimensioni::new(32, 32),
                        content_area,
                        view,
                    },
                },
                MeshCommand::DrawMesh(submission.clone()),
            )
            .unwrap();
        let meshes = &renderer.resources.as_ref().unwrap().meshes;
        assert_eq!(meshes.len(), 1, "one immutable source needs one cached upload");
        let buffer = meshes[0].vertex_buffer;
        assert_eq!(*upload.get_or_insert(buffer), buffer, "later frames must reuse the original upload");
        let mut pixels = vec![0; 32 * 32 * 4];
        // SAFETY: RGBA/u8 readback writes exactly the initialized allocation's extent;
        // no pack buffer is bound and the current default framebuffer remains live.
        unsafe {
            gl.read_pixels(0, 0, 32, 32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixels)));
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        }
        pixels
    });
}
