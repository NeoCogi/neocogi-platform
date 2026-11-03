//! Viewer command-line and optional real software-GPU qualification.
use super::*;

/// Options reject malformed values before any display/host interaction.
#[test]
fn command_line_is_checked_before_window_creation() {
    let parse = |args: &[&str]| Options::parse("BrainStem", args.iter().map(|v| v.to_string()));
    assert!(parse(&["--help"]).unwrap().is_none());
    for args in [
        vec!["--frames"],
        vec!["--frames", "0"],
        vec!["--frames", "-1"],
        vec!["--time", "NaN"],
        vec!["--time", "-1"],
        vec!["--unknown", "value"],
    ] {
        assert!(parse(&args).is_err());
    }
    let options = parse(&["--frames", "3", "--time", "1.25", "--asset", "custom.glb"]).unwrap().unwrap();
    assert_eq!(options.frames, Some(3));
    assert_eq!(options.time, Some(1.25));
    assert_eq!(options.asset, PathBuf::from("custom.glb"));
}

/// Uses real uploaded meshes, palette uniforms, textures and native depth-tested indexed draws.
/// Optional PPMs go outside the checkout for human inspection and never affect assertions.
#[cfg(all(feature = "test-support", feature = "glow"))]
#[test]
#[ignore = "requires downloaded GLBs and a native SDL GL driver (offscreen is sufficient)"]
fn models_render_animate_and_respond_to_arcball_pan_zoom() {
    // SAFETY: This test executable owns the statically linked immutable process host.
    let _runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.expect("COM host");
    for model in ["DamagedHelmet", "BrainStem"] {
        let scene = Scene::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("examples/gltf/assets/{model}.glb"))).unwrap();
        let compiler = nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap();
        let native = nc_render::testing::window(compiler, model, SIZE).unwrap();
        let target = Target::new(native.window).unwrap();
        let window = &target.window;
        let graphics = Graphics::new(&scene, &target.resources).expect("validated sample pipeline");
        let (center, radius) = scene.bounds();
        let mut camera = Camera::new(center, radius, window.size().unwrap());
        let draw = |camera: &Camera, time: f32, invocation: u64| {
            target.render(&scene, &graphics, camera, time, invocation).unwrap();
            native.probe.pixels().unwrap()
        };
        let initial = draw(&camera, 0., 1);
        assert_eq!(initial.len(), (SIZE.width * SIZE.height * 4) as usize);
        let background = &initial[..4];
        let visible = initial.as_chunks::<4>().0.iter().filter(|p| p.as_slice() != background).count();
        assert!(visible > 1000, "{model} must cover a visible region, found {visible} pixels");
        if let Ok(directory) = std::env::var("NEOCOGI_GLTF_CAPTURE") {
            let mut ppm = format!("P6\n{} {}\n255\n", SIZE.width, SIZE.height).into_bytes();
            // GL acceptance pixels follow glReadPixels bottom-up row order.
            for row in initial.as_chunks::<{ SIZE.width as usize * 4 }>().0.iter().rev() {
                ppm.extend(row.as_chunks::<4>().0.iter().flat_map(|p| p[..3].iter().copied()));
            }
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(PathBuf::from(directory).join(format!("{model}.ppm")), ppm).unwrap();
        }
        camera.input(UiInput::Down {
            position: Point { x: 480, y: 360 },
            buttons: 1,
        });
        camera.input(UiInput::Pointer(Point { x: 620, y: 410 }));
        camera.input(UiInput::Up {
            position: Point { x: 620, y: 410 },
            buttons: 1,
        });
        assert_ne!(initial, draw(&camera, 0., 2), "{model} arcball changes rendered output");
        camera.reset();
        camera.input(UiInput::Down {
            position: Point { x: 480, y: 360 },
            buttons: 2,
        });
        camera.input(UiInput::Pointer(Point { x: 580, y: 400 }));
        camera.input(UiInput::Up {
            position: Point { x: 580, y: 400 },
            buttons: 2,
        });
        assert_ne!(initial, draw(&camera, 0., 3), "{model} pan changes rendered output");
        camera.reset();
        camera.input(UiInput::Scroll(Point { x: 0, y: -180 }));
        assert_ne!(initial, draw(&camera, 0., 4), "{model} zoom changes rendered output");
        camera.reset();
        assert_eq!(initial, draw(&camera, 0., 5), "{model} reset restores the original view");
        if scene.duration > 0. {
            assert_ne!(initial, draw(&camera, scene.duration * 0.37, 6), "GPU skinning must visibly deform BrainStem");
        }
    }
}
