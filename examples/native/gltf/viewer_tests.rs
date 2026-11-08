//! Viewer command-line and optional real software-GPU qualification.
use super::*;

/// SDL permits one event-pump owner per process. Native cases hold this permit until
/// every window/context/resource drops, while CPU-only scene tests remain independent.
#[cfg(all(feature = "test-support", feature = "glow"))]
static NATIVE_SESSION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Writes and loads an actual tiny glTF with external mesh bytes and an optional PNG.
/// The returned scene owns every imported byte, so temporary files can retire before atlas/GPU use.
fn emissive_scene(factor: Option<[f32; 3]>, pixel: Option<[u8; 4]>) -> Scene {
    use serde_json::json;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut bytes = Vec::new();
    floats(&mut bytes, [-1., -1., 0., 1., -1., 0., 0., 1., 0.]);
    floats(&mut bytes, [0., 0., 1., 0., 0., 1., 0., 0., 1.]);
    bytes.extend([0u16, 1, 2].into_iter().flat_map(u16::to_le_bytes));
    std::fs::write(root.join("mesh.bin"), &bytes).unwrap();
    let mut document = json!({
        "asset":{"version":"2.0"}, "scene":0, "scenes":[{"nodes":[0]}], "nodes":[{"mesh":0}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1},"indices":2,"material":0}]}],
        "materials":[{}], "buffers":[{"uri":"mesh.bin","byteLength":bytes.len()}],
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36}, {"buffer":0,"byteOffset":36,"byteLength":36}, {"buffer":0,"byteOffset":72,"byteLength":6}],
        "accessors":[
            {"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[-1,-1,0],"max":[1,1,0]},
            {"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"},
            {"bufferView":2,"componentType":5123,"count":3,"type":"SCALAR"}
        ]
    });
    if let Some(factor) = factor {
        document["materials"][0]["emissiveFactor"] = json!(factor);
    }
    if let Some(pixel) = pixel {
        image::RgbaImage::from_pixel(1, 1, image::Rgba(pixel)).save(root.join("emissive.png")).unwrap();
        document["images"] = json!([{"uri":"emissive.png"}]);
        document["textures"] = json!([{"source":0}]);
        document["materials"][0]["emissiveTexture"] = json!({"index":0});
    }
    let path = root.join("scene.gltf");
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    Scene::load(&path).unwrap()
}

/// Actual glTF defaults and image decoding must reach the atlas without erasing an authored emissive factor.
#[test]
fn emissive_factors_and_optional_images_survive_loading_and_atlas_preparation() {
    for (factor, pixel) in [
        (None, None),
        (Some([1., 0., 0.]), None),
        (None, Some([255, 0, 0, 255])),
        (Some([0.25, 0., 0.]), Some([128, 0, 0, 255])),
    ] {
        let scene = emissive_scene(factor, pixel);
        let material = &scene.materials[scene.primitives[0].material];
        assert_eq!(material.emissive, factor.unwrap_or([0.; 3]));
        let atlas = MaterialAtlas::prepare(material, &scene.images).unwrap();
        let tile = atlas.tiles[3];
        let offset = ((tile.y * atlas.width + tile.x) * 4) as usize;
        assert_eq!(&atlas.pixels[offset..offset + 4], &pixel.unwrap_or([255; 4]));
    }
}

/// With environment illumination disabled, visible model pixels come only from emissive texture × factor.
/// Uses the production loader, atlas, shader and native COM renderer without downloaded model/HDR files.
#[cfg(all(feature = "test-support", feature = "glow"))]
#[test]
#[ignore = "requires a native SDL GL driver; SDL_VIDEODRIVER=offscreen is sufficient"]
fn emissive_materials_render_their_factor_and_texture_independently_of_environment_light() {
    let _native = NATIVE_SESSION.lock().expect("previous native viewer case panicked");
    // SAFETY: This final test executable retains the statically linked process host for every COM call.
    let _runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let region = environment::Region { x: 0, y: 0, width: 1, height: 1 };
    let lighting = Lighting {
        environment: Environment {
            width: 1,
            height: 1,
            pixels: vec![128, 0, 0, 129],
            regions: [region; environment::REGION_COUNT],
        },
        exposure: 1.,
        intensity: 0.,
        background: Background::Hidden,
    };
    let size = Size { width: 64, height: 64 };
    let mut results = Vec::new();
    for (factor, pixel) in [
        (Some([1., 0., 0.]), None),
        (None, None),
        (None, Some([255; 4])),
        (Some([1., 0., 0.]), Some([255; 4])),
        (Some([1., 0., 0.]), Some([0, 255, 0, 255])),
        (Some([0.25, 0., 0.]), Some([255; 4])),
    ] {
        let scene = emissive_scene(factor, pixel);
        let native = neocogi_platform_examples::sdk::platform::testing::window("emissive material regression", size).unwrap();
        let target = Target::new(native.window).unwrap();
        let mut graphics = Graphics::new(&scene, prepare_materials(&scene).unwrap(), &lighting.environment, &target.resources).unwrap();
        let (center, radius) = scene.bounds();
        let camera = Camera::new(center, radius, size);
        target.render(&scene, &mut graphics, &camera, 0., &lighting, 1).unwrap();
        let pixels = native.probe.pixels().unwrap();
        let offset = ((size.height / 2 * size.width + size.width / 2) * 4) as usize;
        results.push(<[u8; 4]>::try_from(&pixels[offset..offset + 4]).unwrap());
    }
    // PBR Neutral tone mapping can desaturate bright red; compare dominance and relative output
    // rather than requiring zero green/blue after the production display transform.
    assert!(
        results[0][0] > 150 && u16::from(results[0][0]) > u16::from(results[0][1]) * 4 && results[0][1] == results[0][2] && results[0][3] == 255,
        "factor-only red emission: {:?}",
        results[0]
    );
    assert_eq!(results[1], [0, 0, 0, 255], "an absent factor defaults to no emission");
    assert_eq!(results[2], results[1], "an authored image does not enable emission without a factor");
    assert_eq!(results[3], results[0], "an authored white image has the same effect as no image");
    assert_eq!(results[4], results[1], "green texture times red factor is black");
    assert!(
        results[5][0] > results[5][1] && results[5][0] < results[0][0] && results[5][1] == results[5][2] && results[5][3] == 255,
        "the factor scales authored texture emission: {:?}",
        results[5]
    );
}

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
        vec!["--exposure", "0"],
        vec!["--exposure", "inf"],
    ] {
        assert!(parse(&args).is_err());
    }
    let options = parse(&["--frames", "3", "--time", "1.25", "--asset", "custom.glb"]).unwrap().unwrap();
    assert_eq!(options.frames, Some(3));
    assert_eq!(options.time, Some(1.25));
    assert_eq!(options.asset, PathBuf::from("custom.glb"));
    let options = parse(&["--no-skybox", "--environment", "custom.hdr", "--exposure", "0.75"]).unwrap().unwrap();
    assert_eq!(options.exposure, 0.75);
    assert_eq!(options.environment, PathBuf::from("custom.hdr"));
    assert!(options.background == Background::Hidden);
}

/// Uses real uploaded meshes, palette uniforms, textures and native depth-tested indexed draws.
/// Optional PPMs go outside the checkout for human inspection and never affect assertions.
#[cfg(all(feature = "test-support", feature = "glow"))]
#[test]
#[ignore = "requires downloaded GLBs and a native SDL GL driver (offscreen is sufficient)"]
fn models_render_animate_and_respond_to_arcball_pan_zoom() {
    let _native = NATIVE_SESSION.lock().expect("previous native viewer case panicked");
    // SAFETY: This test executable owns the statically linked immutable process host.
    let _runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.expect("COM host");
    // Keep all native probes in this one test so SDL's event/context owner is never concurrent.
    hdr_interpolation_probe();
    let mut lighting = Lighting {
        environment: Environment::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("gltf/assets/Cannon_Exterior.hdr")).unwrap(),
        exposure: 1.0,
        intensity: 1.0,
        background: Background::Hidden,
    };
    for model in ["DamagedHelmet", "BrainStem"] {
        let scene = Scene::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("gltf/assets/{model}.glb"))).unwrap();
        let native = neocogi_platform_examples::sdk::platform::testing::window(model, SIZE).unwrap();
        let target = Target::new(native.window).unwrap();
        let window = &target.window;
        let mut graphics =
            Graphics::new(&scene, prepare_materials(&scene).unwrap(), &lighting.environment, &target.resources).expect("validated sample pipeline");
        let (center, radius) = scene.bounds();
        let mut camera = Camera::new(center, radius, window.size().unwrap());
        let mut invocation = 0;
        let mut draw = |camera: &Camera, lighting: &Lighting, time: f32| {
            invocation += 1;
            target.render(&scene, &mut graphics, camera, time, lighting, invocation).unwrap();
            native.probe.pixels().unwrap()
        };
        lighting.background = Background::Hidden;
        let initial = draw(&camera, &lighting, 0.);
        assert_eq!(initial.len(), (SIZE.width * SIZE.height * 4) as usize);
        let background = &initial[..4];
        let visible = initial.as_chunks::<4>().0.iter().filter(|p| p.as_slice() != background).count();
        assert!(visible > 1000, "{model} must cover a visible region, found {visible} pixels");
        lighting.intensity = 0.;
        assert_ne!(
            initial,
            draw(&camera, &lighting, 0.),
            "the HDR atlas must illuminate the model independently of its background"
        );
        lighting.intensity = 1.;
        lighting.exposure = 0.5;
        assert_ne!(initial, draw(&camera, &lighting, 0.), "exposure is applied to HDR material radiance");
        lighting.exposure = 1.;
        lighting.background = Background::Environment;
        let with_sky = draw(&camera, &lighting, 0.);
        assert_ne!(&with_sky[..4], background, "the environment must cover the cleared background");
        let unchanged = initial
            .as_chunks::<4>()
            .0
            .iter()
            .zip(with_sky.as_chunks::<4>().0)
            .filter(|(a, b)| a.as_slice() != background && a == b)
            .count();
        assert!(
            unchanged * 10 > visible * 9,
            "skybox visibility must preserve the model's material illumination"
        );
        if let Ok(directory) = std::env::var("NEOCOGI_GLTF_CAPTURE") {
            let mut ppm = format!("P6\n{} {}\n255\n", SIZE.width, SIZE.height).into_bytes();
            // GL acceptance pixels follow glReadPixels bottom-up row order.
            for row in with_sky.as_chunks::<{ SIZE.width as usize * 4 }>().0.iter().rev() {
                ppm.extend(row.as_chunks::<4>().0.iter().flat_map(|p| p[..3].iter().copied()));
            }
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(PathBuf::from(directory).join(format!("{model}.ppm")), ppm).unwrap();
        }
        lighting.background = Background::Hidden;
        camera.input(UiInput::Down {
            position: Point { x: 480, y: 360 },
            buttons: 1,
        });
        camera.input(UiInput::Pointer(Point { x: 620, y: 410 }));
        camera.input(UiInput::Up {
            position: Point { x: 620, y: 410 },
            buttons: 1,
        });
        assert_ne!(initial, draw(&camera, &lighting, 0.), "{model} arcball changes rendered output");
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
        assert_ne!(initial, draw(&camera, &lighting, 0.), "{model} pan changes rendered output");
        camera.reset();
        camera.input(UiInput::Scroll(Point { x: 0, y: -180 }));
        assert_ne!(initial, draw(&camera, &lighting, 0.), "{model} zoom changes rendered output");
        camera.reset();
        assert_eq!(initial, draw(&camera, &lighting, 0.), "{model} reset restores the original view");
        if scene.duration > 0. {
            assert_ne!(
                initial,
                draw(&camera, &lighting, scene.duration * 0.37),
                "GPU skinning must visibly deform BrainStem"
            );
        }
        let mut sky_only = |camera: &Camera| {
            invocation += 1;
            target.submit(invocation, |view| graphics.record_sky(camera, &lighting, view)).unwrap();
            native.probe.pixels().unwrap()
        };
        let sky = sky_only(&camera);
        camera.input(UiInput::Down {
            position: Point { x: 480, y: 360 },
            buttons: 2,
        });
        camera.input(UiInput::Pointer(Point { x: 580, y: 400 }));
        camera.input(UiInput::Scroll(Point { x: 0, y: -180 }));
        assert_eq!(sky, sky_only(&camera), "an infinitely distant sky must ignore pan and zoom");
        camera.reset();
        camera.input(UiInput::Down {
            position: Point { x: 480, y: 360 },
            buttons: 1,
        });
        camera.input(UiInput::Pointer(Point { x: 620, y: 410 }));
        assert_ne!(sky, sky_only(&camera), "the sky must follow arcball orientation");
    }
}

/// Real fragment execution must decode RGBE before mixing. The midpoint of radiance 1 and 8
/// is 4.5; filtering their exponent bytes first would produce a different pixel value.
#[cfg(all(feature = "test-support", feature = "glow"))]
fn hdr_interpolation_probe() {
    let native = neocogi_platform_examples::sdk::platform::testing::window("RGBE interpolation", SIZE).unwrap();
    let target = Target::new(native.window).unwrap();
    let region = environment::Region { x: 0, y: 0, width: 2, height: 1 };
    let data = Environment {
        width: 2,
        height: 1,
        pixels: vec![128, 0, 0, 129, 128, 0, 0, 132],
        regions: [region; environment::REGION_COUNT],
    };
    let image = texture(&target.resources, Size { width: 2, height: 1 }, data.pixels.clone()).unwrap();
    let sampler = target.resources.create_sampler(GpuSamplerDesc::default()).unwrap();
    let shader = Bytes::source(shader_source(
        r#"
        @vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
            let points=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
            return vec4(points[index],0.5,1.0);
        }
        @fragment fn fs() -> @location(0) vec4<f32> {
            return vec4(environment_tile(0u,vec2(0.5),true,true)/8.0,1.0);
        }
    "#,
    ));
    let vs = target
        .resources
        .create_shader(GpuShaderStage::Vertex, shader.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = target
        .resources
        .create_shader(GpuShaderStage::Fragment, shader.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let description = PipelineDesc {
        bindings: vec![
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 1,
                kind: GpuBindingKind::Uniform,
                byte_len: environment::UNIFORM_BYTES,
            },
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 1,
                kind: GpuBindingKind::SampledTexture,
                byte_len: 0,
            },
        ],
        ..PipelineDesc::default()
    }
    .source()
    .unwrap();
    let pipeline = target
        .resources
        .create_pipeline(description.as_com_ref(), vs.as_com_ref(), fs.as_com_ref())
        .unwrap();
    target
        .submit(1, |view| {
            view.bind_pipeline(pipeline.as_com_ref())?;
            let uniform = TypedGpuBufferFactory::new(target.resources.clone()).create_static(
                TypedGpuBufferDesc::Uniform {
                    element_count: u64::from(environment::UNIFORM_BYTES),
                },
                &data.uniforms(1., 1.),
            )?;
            view.bind_uniform_buffer(GpuShaderStage::Fragment, 1, uniform.as_gpu_buffer(), 0, u64::from(environment::UNIFORM_BYTES))?;
            view.bind_texture(GpuShaderStage::Fragment, 1, image.as_com_ref(), sampler.as_com_ref())?;
            view.draw(3, 1, 0)
        })
        .unwrap();
    let pixels = native.probe.pixels().unwrap();
    let offset = ((SIZE.height / 2 * SIZE.width + SIZE.width / 2) * 4) as usize;
    assert!(
        (i32::from(pixels[offset]) - 143).abs() <= 1,
        "linear HDR midpoint: {:?}",
        &pixels[offset..offset + 4]
    );
    assert_eq!(&pixels[offset + 1..offset + 4], &[0, 0, 255]);
}

/// The application's rendered-scene error keeps native detail after all target owners drop.
#[cfg(all(feature = "test-support", feature = "glow"))]
#[test]
#[ignore = "requires native SDL GL; SDL_VIDEODRIVER=offscreen is sufficient"]
fn scene_render_failure_keeps_native_texture_reason_after_target_destruction() {
    let _native = NATIVE_SESSION.lock().expect("previous native viewer case panicked");
    let error = {
        let mut scene = emissive_scene(Some([1., 0., 0.]), Some([255; 4]));
        // The atlas fits the logical byte quota, but exceeds the tested native
        // maximum width. This is the failure that the former root lookup lost.
        scene.images[0] = scene::Image {
            width: 32769,
            height: 1,
            pixels: vec![255; 32769 * 4],
        };
        let region = environment::Region { x: 0, y: 0, width: 1, height: 1 };
        let lighting = Lighting {
            environment: Environment {
                width: 1,
                height: 1,
                pixels: vec![128, 0, 0, 129],
                regions: [region; environment::REGION_COUNT],
            },
            exposure: 1.,
            intensity: 0.,
            background: Background::Hidden,
        };
        let native = neocogi_platform_examples::sdk::platform::testing::window("retained scene error", SIZE).unwrap();
        let target = Target::new(native.window).unwrap();
        let mut graphics = Graphics::new(&scene, prepare_materials(&scene).unwrap(), &lighting.environment, &target.resources).unwrap();
        let (center, radius) = scene.bounds();
        let camera = Camera::new(center, radius, SIZE);
        target.render(&scene, &mut graphics, &camera, 0., &lighting, 1).unwrap_err()
    };
    assert_eq!(error.status, RENDER_E_BACKEND);
    let displayed = format!("DamagedHelmet: {error}");
    assert!(displayed.contains("texture 32770x2 exceeds native dimension limit"), "{displayed}");
}
