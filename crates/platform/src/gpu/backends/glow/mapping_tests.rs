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
    let gpu = crate::gpu::GpuDevice::<Backend>::new(core).unwrap();
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(RenderSurfaceKind::Framebuffer).unwrap(),
            Default::default(),
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let foreign = super::super::tests::ForeignState::new(gl.clone());
    for usage in [GpuBufferUsage::Dynamic, GpuBufferUsage::Streamed] {
        let create = |desc| {
            if usage == GpuBufferUsage::Dynamic {
                resources.create_dynamic_buffer(desc)
            } else {
                resources.create_streamed_buffer(desc)
            }
            .unwrap()
        };
        let buffer = create(GpuBufferDesc::Uniform { byte_len: 260 });
        let image = create(GpuBufferDesc::Texture {
            size: Size { width: 2, height: 2 },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        });
        let slots = if usage == GpuBufferUsage::Dynamic { 1 } else { 3 };
        let mut expected = Vec::new();
        let mut pinned = Vec::new();
        for fill in 1..=8u8 {
            crate::gpu::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 260]).unwrap();
            crate::gpu::testing::write_pixels(image.as_com_ref(), Rect { x: 0, y: 0, width: 2, height: 2 }, &[fill; 16]).unwrap();
            let source = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
            let pixels = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
            // A valid application's PBO/strides must not change the CPU source addressed on creation or refresh.
            foreign.bind();
            let backing = source.native(&mut Preparation { executor: &mut executor }).unwrap();
            let texture = pixels.native(&mut Preparation { executor: &mut executor }).unwrap();
            let identity = (backing.raw, texture.raw);
            if expected.len() < slots {
                assert!(!expected.contains(&identity));
                expected.push(identity);
                // Force every reserved streamed slot to be realized before releasing captures.
                pinned.push((source.clone(), pixels.clone()));
            } else {
                assert!(expected.contains(&identity), "completed uploads must reuse the existing native slots");
            }
            // Inspect the real GL uniform allocation, including its creation usage hint.
            // SAFETY: This context owns the live buffer and all reads stay within its capacity.
            unsafe {
                gl.bind_buffer(glow::UNIFORM_BUFFER, Some(backing.raw));
                assert_eq!(
                    gl.get_buffer_parameter_i32(glow::UNIFORM_BUFFER, glow::BUFFER_USAGE) as u32,
                    if usage == GpuBufferUsage::Dynamic {
                        glow::DYNAMIC_DRAW
                    } else {
                        glow::STREAM_DRAW
                    }
                );
                let pointer = gl.map_buffer_range(glow::UNIFORM_BUFFER, 0, 272, glow::MAP_READ_BIT);
                assert!(!pointer.is_null());
                assert_eq!(&std::slice::from_raw_parts(pointer, 272)[..260], &[fill; 260]);
                assert_eq!(&std::slice::from_raw_parts(pointer, 272)[260..], &[0; 12]);
                gl.unmap_buffer(glow::UNIFORM_BUFFER);
            }
            assert_eq!(super::super::tests::read_image(&gl, &texture, 2, 2), [fill; 16]);
            drop((source, pixels));
            if usage == GpuBufferUsage::Dynamic {
                assert_eq!(crate::gpu::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 4]), Err(RENDER_E_BUSY));
                assert_eq!(crate::gpu::testing::write_buffer(image.as_com_ref(), 0, &[fill; 4]), Err(RENDER_E_BUSY));
            }
            drop((backing, texture));
            executor.complete().unwrap();
            // SAFETY: The current context completes issued uploads before the completion poll.
            unsafe { gl.finish() };
            executor.collect().unwrap();
            if pinned.len() == slots {
                pinned.clear();
            }
            crate::gpu::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 4]).unwrap();
            crate::gpu::testing::write_buffer(image.as_com_ref(), 0, &[fill; 4]).unwrap();
        }
        drop((buffer, image));
        executor.collect().unwrap();
        assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
    }
}

/// Exact COM spans and offset-at-capacity ranges draw correctly with initialized native tails.
#[test]
#[ignore = "requires native GL ES3; run with SDL_VIDEODRIVER=offscreen"]
fn logical_uniform_spans_use_private_native_extents() {
    use crate::render::{
        FrameInfo, RendererBackend, RendererFrame,
        prelude::{Dimensioni, color},
    };
    let _session = super::super::tests::NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    video.gl_attr().set_context_profile(sdl2::video::GLProfile::GLES);
    video.gl_attr().set_context_version(3, 0);
    let window = video.window("private uniform extent", 8, 8).opengl().hidden().build().unwrap();
    let context = window.gl_create_context().unwrap();
    // SAFETY: the selected SDL context supplies this dispatch and transfers into its physical Core.
    let gl = Rc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) });
    let core = unsafe { Core::from_sdl(gl.clone(), window, context) }.unwrap();
    let mut renderer = super::super::GLRenderer::new(core.clone(), 8, 8).unwrap();
    let gpu = crate::gpu::GpuDevice::<Backend>::new(core).unwrap();
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(RenderSurfaceKind::Framebuffer).unwrap(),
            Default::default(),
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    for usage in [GpuBufferUsage::Static, GpuBufferUsage::Dynamic, GpuBufferUsage::Streamed] {
        for members in 1..=8 {
            for offset in [0, 256] {
                let fixture = crate::gpu::testing::uniforms::fixture(&gpu, &resources, members, offset, usage);
                let logical_len = offset as usize + members as usize * 4;
                let native_len = logical_len.next_multiple_of(16);
                let source = gpu.buffer_snapshot(resources.as_com_ref(), fixture.buffer.as_com_ref()).unwrap();
                let backing = source.native(&mut Preparation { executor: &mut renderer.generic }).unwrap();
                // SAFETY: inspect initialized storage only after checking the actual allocation size.
                unsafe {
                    gl.bind_buffer(glow::UNIFORM_BUFFER, Some(backing.raw));
                    assert_eq!(gl.get_buffer_parameter_i32(glow::UNIFORM_BUFFER, glow::BUFFER_SIZE), native_len as i32);
                    let pointer = gl.map_buffer_range(glow::UNIFORM_BUFFER, 0, native_len as i32, glow::MAP_READ_BIT);
                    assert!(!pointer.is_null());
                    let bytes = std::slice::from_raw_parts(pointer, native_len);
                    assert!(bytes[..offset as usize].iter().all(|value| *value == 0xa5));
                    assert!(bytes[logical_len..].iter().all(|value| *value == 0));
                    gl.unmap_buffer(glow::UNIFORM_BUFFER);
                }
                let block = gpu.block_snapshot(fixture.block.as_com_ref()).unwrap();
                let prepared = renderer.prepare_block(&block, false).unwrap();
                let mut frame = renderer.frame(FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap()).unwrap();
                frame.execute_block(&prepared);
                let mut pixel = [0; 4];
                // SAFETY: the live frame selects the complete 8x8 target; pixel is writable initialized RGBA.
                unsafe {
                    gl.read_pixels(4, 4, 1, 1, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixel)));
                    assert_eq!(gl.get_error(), glow::NO_ERROR);
                }
                assert_eq!(pixel, [255, 0, 0, 255], "members={members}, offset={offset}, usage={usage:?}");
                frame.finish().unwrap();
                drop((prepared, block, backing, source, fixture));
                // SAFETY: complete this context's issued work before retiring the fixture.
                unsafe { gl.finish() };
                renderer.maintain().unwrap();
            }
        }
    }
}

/// Checked private rounding rejects overflow before constructing any native buffer.
#[test]
fn native_uniform_extent_checks_rounding_and_signed_gl_capacity() {
    for (logical, native) in [(4, 16), (8, 16), (12, 16), (16, 16), (20, 32), (260, 272)] {
        assert_eq!(uniform_extent(logical).unwrap(), native);
    }
    assert!(uniform_extent(u64::MAX).is_err());
    assert!(uniform_extent(i32::MAX as u64).is_err());
    assert_eq!(uniform_extent(i32::MAX as u64 - 15).unwrap(), i32::MAX - 15);
}
