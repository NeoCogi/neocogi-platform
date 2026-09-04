//! Pixel regressions for native GL state isolation; run with SDL_VIDEODRIVER=offscreen.

use super::*;

/// Owns application-side objects used to leave deliberately hostile persistent GL state.
struct ForeignState {
    /// The live test context used for setup and deterministic cleanup.
    gl: Arc<glow::Context>,
    /// Incomplete application target that must never receive a UI clear or draw.
    framebuffer: Framebuffer,
    /// Vertex layout unrelated to the backend's private layout.
    array: VertexArray,
    /// Texture deliberately bound on a nonzero texture unit.
    texture: Texture,
    /// Sampler requiring mipmaps, unsuitable for the UI's one-level texture.
    sampler: Sampler,
    /// Unpack buffer that must not reinterpret an uploaded CPU slice as an offset.
    unpack: Buffer,
}

impl ForeignState {
    /// Creates independent resources without borrowing or modifying any backend-owned object.
    fn new(gl: Arc<glow::Context>) -> Self {
        unsafe {
            Self {
                framebuffer: gl.create_framebuffer().unwrap(),
                array: gl.create_vertex_array().unwrap(),
                texture: gl.create_texture().unwrap(),
                sampler: gl.create_sampler().unwrap(),
                unpack: gl.create_buffer().unwrap(),
                gl,
            }
        }
    }

    /// Contaminates bindings, write masks, tests, blending, and upload addressing at once.
    fn bind(&self) {
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
            gl.bind_vertex_array(Some(self.array));
            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            gl.bind_sampler(0, Some(self.sampler));
            gl.sampler_parameter_i32(self.sampler, glow::TEXTURE_MIN_FILTER, glow::NEAREST_MIPMAP_NEAREST as i32);
            gl.color_mask(false, false, false, false);
            gl.blend_equation(glow::FUNC_REVERSE_SUBTRACT);
            gl.enable(glow::STENCIL_TEST);
            gl.stencil_func(glow::NEVER, 0, u32::MAX);
            gl.enable(glow::RASTERIZER_DISCARD);
            gl.enable(glow::SAMPLE_COVERAGE);
            gl.sample_coverage(0.0, false);
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(self.unpack));
            gl.buffer_data_size(glow::PIXEL_UNPACK_BUFFER, 4, glow::STATIC_DRAW);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 8);
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 17);
            gl.pixel_store_i32(glow::UNPACK_SKIP_PIXELS, 3);
            gl.pixel_store_i32(glow::UNPACK_SKIP_ROWS, 2);
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        }
    }
}

impl Drop for ForeignState {
    /// Destroys only the test application's resources while its native context remains current.
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_framebuffer(self.framebuffer);
            self.gl.delete_vertex_array(self.array);
            self.gl.delete_texture(self.texture);
            self.gl.delete_sampler(self.sampler);
            self.gl.delete_buffer(self.unpack);
        }
    }
}

/// Builds an entirely public, valid single-texel atlas for the renderer's sampling path.
fn atlas() -> AtlasHandle {
    let icons = [
        "white",
        "close",
        "expand",
        "collapse",
        "check",
        "expand_down",
        "open_folder",
        "closed_folder",
        "file",
    ]
    .map(|name| (name, Recti::new(0, 0, 1, 1)));
    let entries = [(
        '_',
        CharEntry {
            offset: Vec2i::new(0, 0),
            advance: Vec2i::new(1, 0),
            rect: Recti::new(0, 0, 1, 1),
        },
    )];
    let fonts = [(
        "body",
        FontEntry {
            line_size: 10,
            baseline: 8,
            font_size: 10,
            entries: &entries,
        },
    )];
    AtlasHandle::try_from(&AtlasSource {
        width: 1,
        height: 1,
        pixels: &[255; 4],
        icons: &icons,
        fonts: &fonts,
        format: SourceFormat::Raw,
    })
    .unwrap()
}

/// Reads the default framebuffer regardless of the application-side read-target binding.
fn pixel(gl: &glow::Context, x: i32, y: i32) -> [u8; 4] {
    let mut bytes = [0; 4];
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
        gl.read_pixels(x, y, 1, 1, glow::RGBA, glow::UNSIGNED_BYTE, PixelPackData::Slice(Some(&mut bytes)));
        assert_eq!(gl.get_error(), glow::NO_ERROR);
    }
    bytes
}

/// Concrete failure injection for the active GL application command boundary.
#[derive(Default)]
struct FailingApplication {
    /// Commands actually executed, excluding commands ignored after the first failure.
    calls: usize,
}

impl GlApplicationRenderer for FailingApplication {
    /// The requested error text; a second command must not replace the first failure.
    type Command = &'static str;

    fn render(&mut self, _: GlApplicationRenderContext<'_>, message: Self::Command) -> Result<(), String> {
        self.calls += 1;
        Err(message.to_owned())
    }
}

/// Validates rendered pixels, not a mock's agreement with the implementation's call sequence.
#[test]
#[ignore = "requires a native GL ES 3 driver; run with SDL_VIDEODRIVER=offscreen"]
fn ui_draws_clears_and_uploads_survive_application_gl_state() {
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    video.gl_attr().set_context_profile(sdl2::video::GLProfile::GLES);
    video.gl_attr().set_context_version(3, 0);
    let window = video.window("GL state regression", 8, 8).opengl().hidden().build().unwrap();
    let _context = window.gl_create_context().unwrap();
    let gl = Arc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) });
    let foreign = ForeignState::new(gl.clone());
    foreign.bind();
    let mut renderer = GLRenderer::new(gl.clone(), atlas(), 8, 8).unwrap();
    let vertices = [(1.0, 1.0), (7.0, 1.0), (7.0, 7.0), (1.0, 7.0)].map(|(x, y)| Vertex::new(Vec2f::new(x, y), Vec2f::new(0.5, 0.5), color4b(255, 0, 0, 255)));

    // Both indexed UI draws and colored triangle draws must use the same state restoration.
    for indexed in [true, false] {
        foreign.bind();
        renderer.begin(8, 8, color(17, 19, 23, 255));
        assert_eq!(pixel(&gl, 0, 0), [17, 19, 23, 255]);
        foreign.bind();
        if indexed {
            renderer.push_quad_vertices(&vertices[0], &vertices[1], &vertices[2], &vertices[3]);
            renderer.flush();
        } else {
            renderer.draw_colored_vertices(
                Recti::new(1, 1, 6, 6),
                &[vertices[0], vertices[1], vertices[2], vertices[2], vertices[3], vertices[0]],
            );
        }
        assert_eq!(pixel(&gl, 4, 4), [255, 0, 0, 255]);
        assert_eq!(pixel(&gl, 0, 0), [17, 19, 23, 255]);
    }

    // Replacement must reset hostile unpack state too, and the uploaded texel must remain usable.
    foreign.bind();
    renderer.replace_atlas(atlas()).unwrap();
    renderer.begin(8, 8, color(0, 0, 0, 255));
    renderer.draw_vertices(renderer.tex_o, &vertices, Some(&[0, 1, 2, 2, 3, 0]), renderer.full_clip());
    assert_eq!(pixel(&gl, 4, 4), [255, 0, 0, 255]);

    // The public frame contract must distinguish abandonment from explicit completion. A pending
    // quad is CPU-only until finish; dropping it must leave the cleared framebuffer untouched.
    let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap();
    let mut frame = renderer.frame(info).unwrap();
    frame.push_quad(vertices);
    drop(frame);
    assert_eq!(pixel(&gl, 4, 4), [0, 0, 0, 255]);
    let mut frame = renderer.frame(info).unwrap();
    frame.push_quad(vertices);
    frame.finish().unwrap();
    assert_eq!(pixel(&gl, 4, 4), [255, 0, 0, 255]);

    let mut failing = GLRenderer::new_with_application(gl.clone(), atlas(), 8, 8, FailingApplication::default()).unwrap();
    let args = CustomRenderArgs {
        dimensions: info.dimensions(),
        content_area: Recti::new(0, 0, 8, 8),
        view: Recti::new(0, 0, 8, 8),
    };
    let mut frame = failing.frame(info).unwrap();
    frame.enqueue_application(args, "first failure");
    frame.enqueue_application(args, "later failure");
    frame.push_quad(vertices);
    assert_eq!(frame.finish(), Err(FrameError::new("first failure")));
    assert_eq!(failing.application.calls, 1);
    assert_eq!(pixel(&gl, 4, 4), [0, 0, 0, 255]);
    // Failure is frame-local on GL; the next acquired frame can complete normally.
    failing.frame(info).unwrap().finish().unwrap();
}
