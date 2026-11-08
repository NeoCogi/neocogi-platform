//! Native mapping regressions share the production context and completion path.
use super::*;

/// SubData and TexSubImage retain fixed native names through repeated completed writes.
#[test]
#[ignore = "requires native GL ES3; run with SDL_VIDEODRIVER=offscreen"]
fn uploaded_resource_slots_keep_native_names_across_uploads() {
    let _session = super::super::tests::NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    video.gl_attr().set_context_profile(sdl2::video::GLProfile::GLES);
    video.gl_attr().set_context_version(3, 0);
    let window = video.window("persistent mappings", 8, 8).opengl().hidden().build().unwrap();
    let context = window.gl_create_context().unwrap();
    // SAFETY: The current SDL context is moved into Core with its matching dispatch.
    let gl = Rc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) });
    // SAFETY: This unique context and its actual parent window become Core-owned.
    let core = unsafe { Core::from_sdl(gl.clone(), window, context) }.unwrap();
    let mut executor = Executor::new(core.clone()).unwrap();
    let gpu = crate::gpu::GpuDevice::<Backend>::new(
        core,
        neocogi_platform::compiler::CompilerObject::from_value(neocogi_platform::compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(RenderSurfaceKind::Framebuffer).unwrap(),
            Default::default(),
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let buffer = resources.create_dynamic_buffer(GpuBufferDesc::Uniform { byte_len: 32 }).unwrap();
    let image = resources
        .create_dynamic_buffer(GpuBufferDesc::Texture {
            size: Size { width: 2, height: 2 },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        })
        .unwrap();
    let mut expected = None;
    for fill in 1..=8u8 {
        crate::gpu::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 32]).unwrap();
        crate::gpu::testing::write_pixels(image.as_com_ref(), Rect { x: 0, y: 0, width: 2, height: 2 }, &[fill; 16]).unwrap();
        let source = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
        let pixels = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
        let backing = source.native(&mut Preparation { executor: &mut executor }).unwrap();
        let texture = pixels.native(&mut Preparation { executor: &mut executor }).unwrap();
        let identity = (backing.raw, texture.raw);
        assert_eq!(*expected.get_or_insert(identity), identity);
        // Inspect the real GL uniform allocation, including its creation usage hint.
        // SAFETY: This context owns the live buffer and all reads stay within its capacity.
        unsafe {
            gl.bind_buffer(glow::UNIFORM_BUFFER, Some(backing.raw));
            assert_eq!(gl.get_buffer_parameter_i32(glow::UNIFORM_BUFFER, glow::BUFFER_USAGE) as u32, glow::DYNAMIC_DRAW);
            let pointer = gl.map_buffer_range(glow::UNIFORM_BUFFER, 0, 32, glow::MAP_READ_BIT);
            assert!(!pointer.is_null());
            assert_eq!(std::slice::from_raw_parts(pointer, 32), &[fill; 32]);
            gl.unmap_buffer(glow::UNIFORM_BUFFER);
        }
        assert_eq!(super::super::tests::read_image(&gl, &texture, 2, 2), [fill; 16]);
        drop((source, pixels));
        assert_eq!(crate::gpu::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 4]), Err(RENDER_E_BUSY));
        assert_eq!(crate::gpu::testing::write_buffer(image.as_com_ref(), 0, &[fill; 4]), Err(RENDER_E_BUSY));
        drop((backing, texture));
        executor.complete().unwrap();
        // SAFETY: The current context completes issued uploads before the completion poll.
        unsafe { gl.finish() };
        executor.collect().unwrap();
        crate::gpu::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 4]).unwrap();
        crate::gpu::testing::write_buffer(image.as_com_ref(), 0, &[fill; 4]).unwrap();
    }
    drop((buffer, image));
    executor.collect().unwrap();
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
}
