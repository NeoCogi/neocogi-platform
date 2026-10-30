//! Pixel regressions for native GL state isolation; run with SDL_VIDEODRIVER=offscreen.

use super::*;

/// SDL initialization is process-wide and rejects concurrent owners on different test threads.
static NATIVE_SESSION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Owns application-side objects used to leave deliberately hostile persistent GL state.
struct ForeignState {
    /// The live test context used for setup and deterministic cleanup.
    gl: Rc<glow::Context>,
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
    fn new(gl: Rc<glow::Context>) -> Self {
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

#[path = "../../gpu/tests/support/sampled.rs"]
mod sampled;

#[path = "../../gpu/tests/support/derivatives.rs"]
mod derivatives;

/// Window and FBO draws must use the same canonical signed Y differential axis.
#[test]
#[ignore = "requires native GL ES3; run with SDL_VIDEODRIVER=offscreen"]
fn canonical_signed_derivatives_match_window_and_texture_pixels() {
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    video.gl_attr().set_context_profile(sdl2::video::GLProfile::GLES);
    video.gl_attr().set_context_version(3, 0);
    let window = video.window("canonical derivatives", 8, 8).opengl().hidden().build().unwrap();
    let _context = window.gl_create_context().unwrap();
    // SAFETY: SDL's current live context owns every GL operation and resource below.
    let gl = Rc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) });
    let mut renderer = GLRenderer::new(gl.clone(), 8, 8).unwrap();
    let gpu = neocogi_render_gpu::GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu.create_ui_client(Default::default(), None).unwrap();
    let resources = domain.resources().unwrap();
    let output = TextureId::for_render_target(8, 8).unwrap();
    renderer.create_render_texture(output).unwrap();
    for case in derivatives::cases() {
        let owner = derivatives::block(&gpu, &resources, &case.source);
        let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
        for offscreen in [false, true] {
            let prepared = renderer.prepare_block(&snapshot, offscreen).unwrap();
            let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap();
            let mut frame = if offscreen {
                renderer.texture_frame(info, output).unwrap()
            } else {
                renderer.frame(info).unwrap()
            };
            frame.execute_block(&prepared);
            let mut pixel = [0; 4];
            // SAFETY: current target is 8x8 and pixel is writable RGBA storage.
            unsafe {
                let target = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, target);
                gl.read_pixels(4, 4, 1, 1, glow::RGBA, glow::UNSIGNED_BYTE, PixelPackData::Slice(Some(&mut pixel)));
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
    renderer.destroy_texture(output);
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

/// Validates rendered pixels, not a mock's agreement with the implementation's call sequence.
#[test]
#[ignore = "requires a native GL ES 3 driver; run with SDL_VIDEODRIVER=offscreen"]
fn ui_draws_clears_and_uploads_survive_application_gl_state() {
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    video.gl_attr().set_context_profile(sdl2::video::GLProfile::GLES);
    video.gl_attr().set_context_version(3, 0);
    let window = video.window("GL state regression", 8, 8).opengl().hidden().build().unwrap();
    let _context = window.gl_create_context().unwrap();
    let gl = Rc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) });
    let foreign = ForeignState::new(gl.clone());
    foreign.bind();
    let mut renderer = GLRenderer::new(gl.clone(), 8, 8).unwrap();
    let gpu = neocogi_render_gpu::GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu.create_ui_client(Default::default(), None).unwrap();
    let resources = domain.resources().unwrap();
    let root = neocogi_render_gpu::abi::Rect { x: 1, y: 1, width: 6, height: 6 };
    let block = sampled::block(&gpu, &resources, root, neocogi_render_gpu::abi::Size { width: 1, height: 1 }, &[255, 0, 0, 255]);
    foreign.bind();
    let prepared = renderer.prepare_block(&gpu.block_snapshot(block.as_com_ref()).unwrap(), false).unwrap();
    let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(17, 19, 23, 255)).unwrap();
    for _ in 0..2 {
        foreign.bind();
        let mut frame = renderer.frame(info).unwrap();
        assert_eq!(pixel(&gl, 0, 0), [17, 19, 23, 255]);
        foreign.bind();
        frame.execute_block(&prepared);
        frame.finish().unwrap();
        assert_eq!(pixel(&gl, 4, 4), [255, 0, 0, 255]);
        assert_eq!(pixel(&gl, 0, 0), [17, 19, 23, 255]);
    }
    // Native image upload must independently reset hostile unpack state.
    foreign.bind();
    let image = TextureId::for_upload(1, 1, &[255; 4]).unwrap();
    // Admission must use this call's bytes, not the earlier slice that minted the key.
    for length in [0, 1, 3, 5] {
        assert!(matches!(renderer.create_texture(image, &vec![0; length]),
            Err(TextureError::Image { source: neocogi_render_core::ImageValidationError::PixelLength { expected: 4, actual } }) if actual == length));
        assert!(!renderer.textures.contains_key(&image));
    }
    renderer.create_texture(image, &[255; 4]).unwrap();
    renderer.destroy_texture(image);
    // Abandoning an unexecuted target never manufactures a draw or completion submission.
    drop(renderer.frame(info).unwrap());
    assert_eq!(pixel(&gl, 4, 4), [17, 19, 23, 255]);
}

/// Actual native execution retains logical allocation charges after every public owner disappears.
/// Explicit completion and abandonment both wait for observed GPU completion before refunding.
#[test]
#[ignore = "requires SDL GL; run with SDL_VIDEODRIVER=offscreen"]
fn native_completion_owns_resource_and_command_charges() {
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    use neocogi_com::NcCopyFromBufferOffset;
    use neocogi_render_gpu::{
        GpuDevice,
        abi::*,
        source::{Bytes, PipelineDesc},
    };
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    video.gl_attr().set_context_profile(sdl2::video::GLProfile::GLES);
    video.gl_attr().set_context_version(3, 0);
    video.gl_attr().set_depth_size(24);
    let window = video.window("GPU completion regression", 8, 8).opengl().hidden().build().unwrap();
    let _context = window.gl_create_context().unwrap();
    // SAFETY: SDL's current context remains owned until after every native object below drops.
    let gl = Rc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) });
    let mut renderer = GLRenderer::new(gl.clone(), 8, 8).unwrap();
    let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap();
    for phase in 0..3 {
        let device = GpuDevice::new(
            nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
                .query()
                .unwrap(),
        );
        let controller = device.create_client(RenderClientLimits::default()).unwrap();
        let resources = controller.resources().unwrap();
        let source = Bytes::source(
            br#"
            struct Values { shift:vec4<f32> }
            @group(1) @binding(0) var<uniform> values:Values;
            @vertex fn vs(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
                let p = array<vec2<f32>, 3>(vec2(-1.,-1.), vec2(3.,-1.), vec2(-1.,3.));
                return vec4(p[i] + values.shift.xy, 0.5, 1.);
            }
            @fragment fn fs() -> @location(0) vec4<f32> { return vec4(1.,0.,0.,1.); }
        "#
            .to_vec(),
        );
        let vs = resources
            .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
            .unwrap();
        let fs = resources
            .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
            .unwrap();
        let desc = PipelineDesc {
            bindings: vec![GpuShaderBinding {
                stage: GpuShaderStage::Vertex,
                slot: 0,
                kind: GpuBindingKind::Uniform,
                byte_len: 16,
            }],
            ..Default::default()
        }
        .source()
        .unwrap();
        let pipeline = resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
        let mut recording = device.broker().begin_frame().unwrap();
        let rect = Rect { x: 1, y: 1, width: 6, height: 6 };
        let mut session = recording
            .begin_view(RenderViewRoot { client: rect, target: rect }, resources.as_com_ref())
            .unwrap();
        let mut view = session.view().unwrap();
        view.bind_pipeline(pipeline.as_com_ref()).unwrap();
        view.push_uniforms(GpuShaderStage::Vertex, 0, NcCopyFromBufferOffset::new(0, &[0; 16])).unwrap();
        view.draw(3, 1, 0).unwrap();
        view.finish().unwrap();
        let owner = session.take_block().unwrap();
        recording.finish().unwrap();
        let block = device.block_snapshot(owner.as_com_ref()).unwrap();
        let prepared = renderer.prepare_block(&block, false).unwrap();
        drop((view, session, recording, owner, block, pipeline, vs, fs));
        let pending = device.recording_usage(resources.as_com_ref()).unwrap().0;
        assert!(pending > 0);
        if phase != 2 {
            let mut frame = renderer.frame(info).unwrap();
            frame.execute_block(&prepared);
            if phase == 0 {
                frame.finish().unwrap();
            } else {
                drop(frame);
            }
            assert_eq!(pixel(&gl, 4, 4), [255, 0, 0, 255]);
        }
        drop(prepared);
        assert!(device.live_bytes(resources.as_com_ref()).unwrap() > 0);
        assert_eq!(device.recording_usage(resources.as_com_ref()).unwrap(), (pending, 0));
        // Finish immediate draws, seal any abandoned batch, then observe its fence. Tests avoid
        // assuming that an immediately issued fence has already signaled on every native driver.
        renderer.generic.complete().unwrap();
        unsafe {
            gl.finish();
        }
        renderer.generic.collect().unwrap();
        assert_eq!(device.live_bytes(resources.as_com_ref()).unwrap(), 0);
        assert_eq!(device.recording_usage(resources.as_com_ref()).unwrap(), (0, 0));
    }
}

/// Guarded GLSL must execute safely too; GLSL array policies alone do not inject Naga bounds checks.
#[test]
#[ignore = "requires SDL GL; run with SDL_VIDEODRIVER=offscreen"]
fn dynamic_shader_indices_cannot_escape_uniform_or_private_storage() {
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    use neocogi_com::NcCopyFromBufferOffset;
    use neocogi_render_gpu::{
        GpuDevice,
        abi::*,
        source::{Bytes, PipelineDesc},
    };
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    video.gl_attr().set_context_profile(sdl2::video::GLProfile::GLES);
    video.gl_attr().set_context_version(3, 0);
    video.gl_attr().set_depth_size(24);
    let window = video.window("GL shader bounds", 8, 8).opengl().hidden().build().unwrap();
    let _context = window.gl_create_context().unwrap();
    let gl = Rc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) });
    let mut renderer = GLRenderer::new(gl.clone(), 8, 8).unwrap();
    let gpu = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    let source = Bytes::source(
        br#"
        @vertex fn vs(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
            let p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));return vec4(p[i],0.5,1.);
        }
        struct Values { items:array<vec4<f32>,2>, indices:vec4<u32> }
        @group(3) @binding(0) var<uniform> data:Values;
        @fragment fn fs() -> @location(0) vec4<f32> {
            var local=array<vec4<f32>,2>(vec4(0.25),vec4(0.75));
            local[data.indices.x]=vec4(1.0);return data.items[data.indices.y]+local[0];
        }
    "#
        .to_vec(),
    );
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let desc = PipelineDesc {
        bindings: vec![GpuShaderBinding {
            stage: GpuShaderStage::Fragment,
            slot: 0,
            kind: GpuBindingKind::Uniform,
            byte_len: 48,
        }],
        ..Default::default()
    }
    .source()
    .unwrap();
    let pipeline = resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let mut recording = gpu.broker().begin_frame().unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut session = recording
        .begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    let mut bytes = vec![0; 32];
    for index in [2u32, 999, 0, 0] {
        bytes.extend_from_slice(&index.to_le_bytes());
    }
    view.push_uniforms(GpuShaderStage::Fragment, 0, NcCopyFromBufferOffset::new(0, &bytes)).unwrap();
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    recording.finish().unwrap();
    let prepared = renderer.prepare_block(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), false).unwrap();
    let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap();
    let mut frame = renderer.frame(info).unwrap();
    frame.execute_block(&prepared);
    frame.finish().unwrap();
    // A native window may have an opaque RGB default framebuffer with no alpha channel.
    assert_eq!(&pixel(&gl, 4, 4)[..3], &[64; 3]);
}

/// Legal client identifiers containing a builtin's spelling must survive native source normalization.
#[test]
#[ignore = "requires SDL GL; run with SDL_VIDEODRIVER=offscreen"]
fn fragment_identifiers_containing_builtin_names_compile_and_render() {
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    use neocogi_com::NcCopyFromBufferOffset;
    use neocogi_render_gpu::{
        GpuDevice,
        abi::*,
        source::{Bytes, PipelineDesc},
    };
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    video.gl_attr().set_context_profile(sdl2::video::GLProfile::GLES);
    video.gl_attr().set_context_version(3, 0);
    video.gl_attr().set_depth_size(24);
    let window = video.window("GL identifier normalization", 8, 8).opengl().hidden().build().unwrap();
    let _context = window.gl_create_context().unwrap();
    let gl = Rc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) });
    let mut renderer = GLRenderer::new(gl.clone(), 8, 8).unwrap();
    let gpu = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    let source = Bytes::source(
        br#"
        fn my_gl_FragCoord_color(v:vec4<f32>) -> vec4<f32> { return v; }
        @vertex fn vs(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
            let p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));return vec4(p[i],0.5,1.);
        }
        @fragment fn fs() -> @location(0) vec4<f32> { return my_gl_FragCoord_color(vec4(0.25,0.5,0.75,1.)); }
    "#
        .to_vec(),
    );
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let desc = PipelineDesc::default().source().unwrap();
    let pipeline = resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let mut recording = gpu.broker().begin_frame().unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut session = recording
        .begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    recording.finish().unwrap();
    let prepared = renderer.prepare_block(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), false).unwrap();
    let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap();
    let mut frame = renderer.frame(info).unwrap();
    frame.execute_block(&prepared);
    frame.finish().unwrap();
    assert_eq!(&pixel(&gl, 4, 4)[..3], &[64, 128, 191]);
}

/// A provider may spell standard main/precision differently from Naga. Driver
/// pixels establish the semantic adapter, including canonical winding on both targets.
#[test]
#[ignore = "requires native GL ES3; run with SDL_VIDEODRIVER=offscreen"]
fn shader_handoff_adapts_varied_glsl_and_culling() {
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|e| e.into_inner());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    video.gl_attr().set_context_profile(sdl2::video::GLProfile::GLES);
    video.gl_attr().set_context_version(3, 0);
    let window = video.window("shader handoff", 8, 8).opengl().hidden().build().unwrap();
    let _context = window.gl_create_context().unwrap();
    // SAFETY: SDL owns the current live context throughout every program/VAO operation.
    let gl = unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) };
    let vertex = "#version 300 es\nvoid main();\nvoid /* entry */ main(void){ highp vec2 p[3]=vec2[3](vec2(-.8,-.8),vec2(.8,-.8),vec2(0.,.8)); gl_Position=vec4(p[gl_VertexID],.25,1.); gl_Position=vec4(gl_Position.x,-gl_Position.y,2.*gl_Position.z-gl_Position.w,gl_Position.w); }";
    let fragment = "#version 300 es\n#if 0\nprecision highp float;\n#endif\nlayout(location=0) out highp vec4 color;\nvoid main(){ color=vec4(gl_FragCoord.y / 8.,0.,0.,1.); }";
    for offscreen in [false, true] {
        let vertex = shader_adapter::vertex(vertex, offscreen).unwrap();
        let (fragment, height) = shader_adapter::fragment(fragment, offscreen).unwrap();
        let program = create_program(&gl, &vertex, &fragment).unwrap();
        // SAFETY: linked program and VAO are created in this current context;
        // no client data pointer is used by the procedural draw.
        unsafe {
            let vao = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(vao));
            gl.use_program(Some(program));
            if let Some(height) = height {
                let location = gl.get_uniform_location(program, &height);
                gl.uniform_1_f32(location.as_ref(), 8.);
            }
            gl.viewport(0, 0, 8, 8);
            gl.enable(glow::CULL_FACE);
            gl.cull_face(glow::BACK);
            gl.front_face(if offscreen { glow::CW } else { glow::CCW });
            gl.clear_color(0., 0., 0., 1.);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            let mut pixel = [0u8; 4];
            gl.read_pixels(4, 3, 1, 1, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixel)));
            let expected = if offscreen { 112i32 } else { 143 };
            assert!((pixel[0] as i32 - expected).abs() <= 1, "offscreen={offscreen}: {pixel:?}");
            assert_eq!(gl.get_error(), glow::NO_ERROR);
            gl.delete_vertex_array(vao);
            gl.delete_program(program);
        }
    }
}

/// One adapter realizes independently valid logical producers without numeric cache aliasing.
#[test]
#[ignore = "requires native GL ES3; run with SDL_VIDEODRIVER=offscreen"]
fn independent_logical_devices_keep_distinct_native_resources() {
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    video.gl_attr().set_context_profile(sdl2::video::GLProfile::GLES);
    video.gl_attr().set_context_version(3, 0);
    let window = video.window("independent logical producers", 8, 8).opengl().hidden().build().unwrap();
    let _context = window.gl_create_context().unwrap();
    // SAFETY: all native resources are released before the owning current SDL context.
    let gl = Rc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) });
    let mut renderer = GLRenderer::new(gl.clone(), 8, 8).unwrap();
    let mut owners = Vec::new();
    let mut snapshots = Vec::new();
    let root = neocogi_render_gpu::abi::Rect { x: 0, y: 0, width: 8, height: 8 };
    for rgba in [[255, 0, 0, 255], [0, 0, 255, 255]] {
        let gpu = neocogi_render_gpu::GpuDevice::new(
            nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
                .query()
                .unwrap(),
        );
        let domain = gpu.create_client(Default::default()).unwrap();
        let resources = domain.resources().unwrap();
        let owner = sampled::block(&gpu, &resources, root, neocogi_render_gpu::abi::Size { width: 1, height: 1 }, &rgba);
        snapshots.push(gpu.block_snapshot(owner.as_com_ref()).unwrap());
        owners.push((gpu, domain, resources, owner));
    }
    for (snapshot, expected) in snapshots.iter().zip([[255, 0, 0, 255], [0, 0, 255, 255]]) {
        let prepared = renderer.prepare_block(snapshot, false).unwrap();
        let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap();
        let mut frame = renderer.frame(info).unwrap();
        frame.execute_block(&prepared);
        assert_eq!(pixel(&gl, 4, 4), expected);
        frame.finish().unwrap();
    }
}
