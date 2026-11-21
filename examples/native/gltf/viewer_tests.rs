//! Viewer command-line and optional real software-GPU qualification.
use super::*;
#[cfg(feature = "test-support")]
use neocogi_com_sdk::{ComRef, NcOutputWriter, NC_E_OUT_OF_RANGE};

/// SDL permits one event-pump owner per process. Native cases hold this permit until
/// every window/context/resource drops, while CPU-only scene tests remain independent.
#[cfg(feature = "test-support")]
static NATIVE_SESSION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Writes and loads an actual tiny glTF with external mesh bytes and an optional PNG.
/// The returned scene owns every imported byte, so temporary files can retire before atlas/GPU use.
fn emissive_scene(factor: Option<[f32; 3]>, pixel: Option<[u8; 4]>) -> Scene {
    triangle_scene(factor, pixel, |_, _, _| {})
}

/// Builds one localized input file and lets a fixture author its own transforms or material maps.
fn triangle_scene(factor: Option<[f32; 3]>, pixel: Option<[u8; 4]>, customize: impl FnOnce(&std::path::Path, &mut serde_json::Value, &mut Vec<u8>)) -> Scene {
    use serde_json::json;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut bytes = Vec::new();
    floats(&mut bytes, [-1., -1., 0., 1., -1., 0., 0., 1., 0.]);
    floats(&mut bytes, [0., 0., 1., 0., 0., 1., 0., 0., 1.]);
    bytes.extend([0u16, 1, 2].into_iter().flat_map(u16::to_le_bytes));
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
    customize(root, &mut document, &mut bytes);
    document["buffers"][0]["byteLength"] = json!(bytes.len());
    std::fs::write(root.join("mesh.bin"), &bytes).unwrap();
    let path = root.join("scene.gltf");
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    Scene::load(&path).unwrap()
}

/// Reimports the same authored triangle with scaled binary positions and accessor bounds.
#[cfg(feature = "test-support")]
fn camera_triangle(scale: f32) -> Scene {
    triangle_scene(Some([1., 0., 0.]), None, |_, document, bytes| {
        let mut positions = Vec::new();
        floats(&mut positions, [-scale, -scale, 0., scale, -scale, 0., 0., scale, 0.]);
        bytes[..36].copy_from_slice(&positions);
        document["accessors"][0]["min"] = serde_json::json!([-scale, -scale, 0.]);
        document["accessors"][0]["max"] = serde_json::json!([scale, scale, 0.]);
    })
}

/// Emission isolates rasterized coverage from lighting at very large coordinates.
#[cfg(feature = "test-support")]
fn camera_lighting(background: Background) -> Lighting {
    let region = environment::Region { x: 0, y: 0, width: 1, height: 1 };
    Lighting {
        environment: Environment {
            width: 1,
            height: 1,
            pixels: vec![128, 0, 0, 129],
            regions: [region; environment::REGION_COUNT],
        },
        exposure: 1.,
        intensity: 0.,
        background,
    }
}

/// The same finite silhouette reaches native pixels even when its far plane exceeds f32.
#[cfg(all(feature = "test-support", feature = "glow"))]
#[test]
#[ignore = "requires a native SDL GL driver; SDL_VIDEODRIVER=offscreen is sufficient"]
fn large_finite_camera_frame_retains_native_triangle_coverage() {
    let _native = NATIVE_SESSION.lock().unwrap();
    let _runtime = neocogi_com_host::initialize_runtime();
    let lighting = camera_lighting(Background::Hidden);
    let native = neocogi_platform_examples::sdk::platform::testing::window("finite glTF camera", Size { width: 64, height: 64 }).unwrap();
    let target = Target::new(native.window).unwrap();
    let size = target.window.size().unwrap();
    assert_eq!(size.width, size.height);
    let coverage = [(1, 1.0), (2, 2e36)].map(|(invocation, scale)| {
        let scene = camera_triangle(scale);
        let (center, radius) = scene.bounds();
        assert!(radius.is_finite() && center.iter().all(|value| value.is_finite()));
        let camera = Camera::new(center, radius, size);
        let mut graphics = Graphics::new(&scene, prepare_materials(&scene).unwrap(), &lighting.environment, &target.resources).unwrap();
        target.render(&scene, &mut graphics, &camera, 0., &lighting, invocation).unwrap();
        let pixels = native.probe.pixels().unwrap();
        assert_eq!(pixels.len(), (size.width * size.height * 4) as usize);
        let offset = ((size.height / 2 * size.width + size.width / 2) * 4) as usize;
        assert!(pixels[offset] > 180, "scale {scale:e} center pixel {:?}", &pixels[offset..offset + 4]);
        assert_ne!(&pixels[offset..offset + 4], &pixels[..4], "scale {scale:e} must draw a silhouette");
        pixels.chunks_exact(4).map(|pixel| pixel != &pixels[..4]).collect::<Vec<_>>()
    });
    assert_eq!(coverage[0], coverage[1]);
}

/// Records any attempted frame-side effect through the generated COM calls.
#[cfg(feature = "test-support")]
#[com_object(interfaces(IRenderHost, IRenderer))]
struct FrameEntryProbe {
    /// Shared with the upload capabilities and retained after fixture teardown.
    entries: Rc<RefCell<Vec<&'static str>>>,
}

#[cfg(feature = "test-support")]
impl FrameEntryProbe {
    /// Refuses the operation after observing entry, so a regression cannot render through the fixture.
    fn reject<T>(&self, operation: &'static str) -> Result<T, NcStatus> {
        self.entries.borrow_mut().push(operation);
        Err(neocogi_com_sdk::NC_E_INVALID_ARG)
    }
}

#[cfg(feature = "test-support")]
impl IRenderDriverBound for FrameEntryProbe {
    /// A skipped frame needs no driver identity.
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        self.reject("driver identity")
    }
}

#[cfg(feature = "test-support")]
impl IRenderHost for FrameEntryProbe {
    /// Captures maintenance entry before any provider work.
    fn maintain_result(&self) -> Result<ComPtr<IRenderOutcomeAbi>, NcStatus> {
        self.reject("maintenance")
    }

    /// Rejected frames must never import a texture.
    fn import_texture_result(&self, _: ComRef<'_, IRenderResourcesAbi>, _: ComRef<'_, ITextureAbi>) -> Result<ComBox<ITextureImportResultAbi>, NcStatus> {
        self.reject("texture import")
    }

    /// Rejected frames must never prepare a texture.
    fn prepare_texture_result(&self, _: ComRef<'_, IRenderResourcesAbi>, _: ComRef<'_, IGpuBufferAbi>) -> Result<ComPtr<IRenderOutcomeAbi>, NcStatus> {
        self.reject("texture preparation")
    }

    /// Captures recording admission before any native recorder exists.
    fn recording_broker(&self) -> Result<ComPtr<IRenderViewBrokerAbi>, NcStatus> {
        self.reject("recording broker")
    }
}

#[cfg(feature = "test-support")]
impl IRenderer for FrameEntryProbe {
    /// Captures entry before any renderer work.
    fn render_result(&self, _: RenderInfo, _: ComRef<'_, IRenderStreamAbi>) -> Result<ComPtr<IRenderOutcomeAbi>, NcStatus> {
        self.reject("renderer")
    }
}

/// Replaces a single frame uniform with the same declared shape and observable upload entry.
#[cfg(feature = "test-support")]
#[com_object(interfaces(IGpuBuffer), ownership = exclusive)]
struct UploadEntryProbe {
    /// Exact metadata from the production buffer replaced by this fixture.
    info: GpuBufferInfo,
    /// Reports through the same owned trace as the host and renderer.
    entries: Rc<RefCell<Vec<&'static str>>>,
}

#[cfg(feature = "test-support")]
impl IRenderDriverBound for UploadEntryProbe {
    /// These test buffers never pretend to belong to the window's driver.
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        Err(neocogi_com_sdk::NC_E_INVALID_ARG)
    }
}

#[cfg(feature = "test-support")]
impl IGpuBuffer for UploadEntryProbe {
    /// Permits the typed wrapper's setup-time shape check.
    fn info(&self) -> Result<GpuBufferInfo, NcStatus> {
        Ok(self.info)
    }

    /// Records even uploads that would leave identical bytes in the original buffer.
    fn upload(&mut self, _: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        self.entries.borrow_mut().push("uniform upload");
        Err(neocogi_com_sdk::NC_E_INVALID_ARG)
    }

    /// No application frame reads uniform contents back.
    fn read(&self, _: NcOutputWriter<'_, u8>) -> Result<(), NcStatus> {
        Err(neocogi_com_sdk::NC_E_INVALID_ARG)
    }
}

/// A finite imported sphere whose fitted eye exceeds f32 fails before the complete frame boundary.
#[cfg(feature = "test-support")]
#[test]
#[ignore = "requires a native SDL display; Glow also permits SDL_VIDEODRIVER=offscreen"]
fn unrepresentable_camera_frame_returns_owned_error_before_frame_entry() {
    let _native = NATIVE_SESSION.lock().unwrap();
    let _runtime = neocogi_com_host::initialize_runtime();
    let entries = Rc::new(RefCell::new(Vec::new()));
    let error = {
        let scene = camera_triangle(1e38);
        let (center, radius) = scene.bounds();
        assert!(radius.is_finite() && center.iter().all(|value| value.is_finite()));
        let lighting = camera_lighting(Background::Environment);
        let native = neocogi_platform_examples::sdk::platform::testing::window("out of range glTF camera", Size { width: 64, height: 64 }).unwrap();
        let mut target = Target::new(native.window).unwrap();
        let size = target.window.size().unwrap();
        assert_eq!(size.width, size.height);
        let camera = Camera::new(center, radius, size);
        let mut graphics = Graphics::new(&scene, prepare_materials(&scene).unwrap(), &lighting.environment, &target.resources).unwrap();
        let probe = FrameEntryProbe::from_value(FrameEntryProbe { entries: entries.clone() });
        target.host = probe.query().unwrap();
        target.renderer = probe.query().unwrap();
        for uniform in [&mut graphics.camera, &mut graphics.sky, &mut graphics.lighting]
            .into_iter()
            .chain(&mut graphics.skins)
            .chain(&mut graphics.parameters)
        {
            let buffer = UploadEntryProbe::into_box(UploadEntryProbe {
                entries: entries.clone(),
                info: uniform.info(),
            })
            .query()
            .unwrap();
            *uniform = TypedGpuBuffer::wrap(buffer).unwrap();
        }
        target.render(&scene, &mut graphics, &camera, 0., &lighting, 1).unwrap_err()
    };
    assert!(
        entries.borrow().is_empty(),
        "graphics work entered before camera validation: {:?}",
        entries.borrow()
    );
    assert_eq!(error.status, NC_E_OUT_OF_RANGE);
    assert_eq!(error.message, "glTF camera eye is outside the finite f32 upload range");
}

/// Positive and negative sampled world determinants must keep the authored front face visible.
#[cfg(all(feature = "test-support", feature = "glow"))]
#[test]
#[ignore = "requires a native SDL GL driver; SDL_VIDEODRIVER=offscreen is sufficient"]
fn rigid_mesh_facing_follows_sampled_world_orientation() {
    use serde_json::json;
    let _native = NATIVE_SESSION.lock().unwrap();
    let _runtime = neocogi_com_host::initialize_runtime();
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
    let native = neocogi_platform_examples::sdk::platform::testing::window("rigid glTF facing", size).unwrap();
    let target = Target::new(native.window).unwrap();
    let normal = [1., 1., 1.];
    let mirror = [-1., 1., 1.];
    for (label, mesh_scale, ancestors, animated, reversed) in [
        ("ordinary", normal, vec![], false, false),
        ("mirrored node", mirror, vec![], false, false),
        ("nonuniform mirror", [-0.4, 1.5, 0.7], vec![], false, false),
        ("inherited mirror", normal, vec![mirror], false, false),
        ("two inherited mirrors", normal, vec![mirror, mirror], false, false),
        ("animated mirror", normal, vec![], true, false),
        ("ordinary back", normal, vec![], false, true),
        ("mirrored back", mirror, vec![], false, true),
    ] {
        let scene = triangle_scene(Some([1., 0., 0.]), None, |_, document, bytes| {
            if reversed {
                bytes[74..78].copy_from_slice(&[2, 0, 1, 0]);
            }
            let mut nodes = vec![json!({"mesh":0,"scale":mesh_scale})];
            for scale in ancestors {
                nodes.push(json!({"scale":scale,"children":[nodes.len()-1]}));
            }
            document["scenes"][0]["nodes"] = json!([nodes.len() - 1]);
            document["nodes"] = json!(nodes);
            if animated {
                bytes.resize(80, 0);
                floats(bytes, [0., 1., 2.]);
                floats(bytes, [normal, mirror, mirror].into_iter().flatten());
                document["bufferViews"].as_array_mut().unwrap().extend([
                    json!({"buffer":0,"byteOffset":80,"byteLength":12}),
                    json!({"buffer":0,"byteOffset":92,"byteLength":36}),
                ]);
                document["accessors"].as_array_mut().unwrap().extend([
                    json!({"bufferView":3,"componentType":5126,"count":3,"type":"SCALAR","min":[0],"max":[2]}),
                    json!({"bufferView":4,"componentType":5126,"count":3,"type":"VEC3"}),
                ]);
                document["animations"] = json!([{
                    "samplers":[{"input":3,"output":4,"interpolation":"STEP"}],
                    "channels":[{"sampler":0,"target":{"node":0,"path":"scale"}}]
                }]);
            }
        });
        let mut graphics = Graphics::new(&scene, prepare_materials(&scene).unwrap(), &lighting.environment, &target.resources).unwrap();
        let (center, radius) = scene.bounds();
        let camera = Camera::new(center, radius, size);
        for (frame, seconds) in [0.25, 1.25].into_iter().take(if animated { 2 } else { 1 }).enumerate() {
            target.render(&scene, &mut graphics, &camera, seconds, &lighting, frame as u64 + 1).unwrap();
            let pixels = native.probe.pixels().unwrap();
            let offset = ((size.height / 2 * size.width + size.width / 2) * 4) as usize;
            let center = &pixels[offset..offset + 4];
            if reversed {
                assert_eq!(center, &pixels[..4], "{label} must be culled");
            } else {
                assert!(
                    center[0] > 150 && u16::from(center[0]) > u16::from(center[1]) * 2,
                    "{label} at {seconds} must be visible: {center:?}"
                );
            }
        }
    }
}

/// Directional diffuse texels distinguish correct mapped-normal reversal from either missing culling change or flipping only geometric Z.
#[cfg(all(feature = "test-support", feature = "glow"))]
#[test]
#[ignore = "requires a native SDL GL driver; SDL_VIDEODRIVER=offscreen is sufficient"]
fn double_sided_materials_reverse_the_mapped_back_face_normal() {
    use serde_json::json;
    let _native = NATIVE_SESSION.lock().unwrap();
    let _runtime = neocogi_com_host::initialize_runtime();
    let dark = environment::Region { x: 0, y: 1, width: 1, height: 1 };
    let mut regions = [dark; environment::REGION_COUNT];
    regions[environment::DIFFUSE_REGION] = environment::Region { x: 0, y: 0, width: 4, height: 1 };
    let lighting = Lighting {
        environment: Environment {
            width: 4,
            height: 2,
            // Longitude centers from left to right are green, red, white and blue.
            pixels: vec![
                0, 128, 0, 129, 128, 0, 0, 129, 128, 128, 128, 129, 0, 0, 128, 129, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
            regions,
        },
        exposure: 1.,
        intensity: 1.,
        background: Background::Hidden,
    };
    let size = Size { width: 64, height: 64 };
    let native = neocogi_platform_examples::sdk::platform::testing::window("two-sided glTF material", size).unwrap();
    let target = Target::new(native.window).unwrap();
    for (mirror, reverse, two_sided, dominant) in [
        (false, false, true, Some(0)),
        (false, true, true, Some(2)),
        (true, true, true, Some(1)),
        (false, true, false, None),
    ] {
        let scene = triangle_scene(None, None, |root, document, bytes| {
            if reverse {
                bytes[74..78].copy_from_slice(&[2, 0, 1, 0]);
            }
            let x = if mirror { -1.0 } else { 1.0 };
            document["nodes"][0]["scale"] = json!([x, 1.0, 1.0]);
            bytes.resize(80, 0);
            floats(bytes, [0., 0., 1., 0., 0.5, 1.]);
            document["bufferViews"]
                .as_array_mut()
                .unwrap()
                .push(json!({"buffer":0,"byteOffset":80,"byteLength":24}));
            document["accessors"]
                .as_array_mut()
                .unwrap()
                .push(json!({"bufferView":3,"componentType":5126,"count":3,"type":"VEC2"}));
            document["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = json!(3);
            image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 128, 255, 255]))
                .save(root.join("normal.png"))
                .unwrap();
            document["images"] = json!([{"uri":"normal.png"}]);
            document["textures"] = json!([{"source":0}]);
            document["materials"][0] = json!({
                "doubleSided":two_sided, "normalTexture":{"index":0},
                "pbrMetallicRoughness":{"metallicFactor":0.0}
            });
        });
        assert_eq!(scene.materials[0].double_sided, two_sided);
        let (center, radius) = scene.bounds();
        let camera = Camera::new(center, radius, size);
        let mut graphics = Graphics::new(&scene, prepare_materials(&scene).unwrap(), &lighting.environment, &target.resources).unwrap();
        target.render(&scene, &mut graphics, &camera, 0., &lighting, 1).unwrap();
        let pixels = native.probe.pixels().unwrap();
        let offset = ((size.height / 2 * size.width + size.width / 2) * 4) as usize;
        let center = &pixels[offset..offset + 4];
        if let Some(dominant) = dominant {
            assert!(
                center[dominant] > 150
                    && center[..3]
                        .iter()
                        .enumerate()
                        .all(|(c, value)| c == dominant || u16::from(*value) * 4 < u16::from(center[dominant])),
                "mirror={mirror}, reverse={reverse} expected color channel {dominant}: {center:?}",
            );
        } else {
            assert_eq!(center, &pixels[..4], "reversed one-sided mesh must remain culled");
        }
    }
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
    let _runtime = neocogi_com_host::initialize_runtime();
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

/// Imports a real rigid or skinned triangle whose normal is tilted off the rotation axis.
#[cfg(all(feature = "test-support", feature = "glow"))]
fn normal_scene(linear_transforms: &[[[f32; 3]; 3]], skin_weights: Option<[[f32; 2]; 3]>) -> Scene {
    use serde_json::json;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut bytes = Vec::new();
    floats(&mut bytes, [-0.75, -0.75, 0.5, 0.75, -0.75, 0.5, 0., 0.75, 0.5]);
    floats(&mut bytes, [0.6, 0., 0.8, 0.6, 0., 0.8, 0.6, 0., 0.8]);
    bytes.extend([0u16, 1, 2, 0].into_iter().flat_map(u16::to_le_bytes));
    let mut views = vec![
        json!({"buffer":0,"byteOffset":0,"byteLength":36}),
        json!({"buffer":0,"byteOffset":36,"byteLength":36}),
        json!({"buffer":0,"byteOffset":72,"byteLength":6}),
    ];
    let mut accessors = vec![
        json!({"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[-0.75,-0.75,0.5],"max":[0.75,0.75,0.5]}),
        json!({"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}),
        json!({"bufferView":2,"componentType":5123,"count":3,"type":"SCALAR"}),
    ];
    let nodes: Vec<_> = linear_transforms
        .iter()
        .map(|[a, b, c]| json!({"matrix":[a[0],a[1],a[2],0,b[0],b[1],b[2],0,c[0],c[1],c[2],0,0,0,0,1]}))
        .collect();
    let mut primitive = json!({"attributes":{"POSITION":0,"NORMAL":1},"indices":2});
    let mut document = json!({"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}]});
    if let Some(weights) = skin_weights {
        views.push(json!({"buffer":0,"byteOffset":bytes.len(),"byteLength":24}));
        bytes.extend([0u16, 1, 0, 0].repeat(3).into_iter().flat_map(u16::to_le_bytes));
        views.push(json!({"buffer":0,"byteOffset":bytes.len(),"byteLength":48}));
        floats(&mut bytes, weights.into_iter().flat_map(|[first, second]| [first, second, 0., 0.]));
        accessors.push(json!({"bufferView":3,"componentType":5123,"count":3,"type":"VEC4"}));
        accessors.push(json!({"bufferView":4,"componentType":5126,"count":3,"type":"VEC4"}));
        primitive["attributes"]["JOINTS_0"] = json!(3);
        primitive["attributes"]["WEIGHTS_0"] = json!(4);
        document["nodes"] = json!([{"mesh":0,"skin":0}, nodes[0], nodes[1]]);
        document["scenes"][0]["nodes"] = json!([0, 1, 2]);
        document["skins"] = json!([{"joints":[1,2]}]);
    } else {
        document["nodes"] = json!(nodes);
        document["nodes"][0]["mesh"] = json!(0);
    }
    document["meshes"] = json!([{"primitives":[primitive]}]);
    document["buffers"] = json!([{"uri":"triangle.bin","byteLength":bytes.len()}]);
    document["bufferViews"] = json!(views);
    document["accessors"] = json!(accessors);
    std::fs::write(root.join("triangle.bin"), bytes).unwrap();
    let path = root.join("normals.gltf");
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    Scene::load(&path).unwrap()
}

/// The reference uses a double-precision row-based inverse and affine pixel barycentrics.
#[cfg(all(feature = "test-support", feature = "glow"))]
fn reference_normal_pixel(primitive: &scene::Primitive, palette: &[rs_math3d::Mat4f], projection_scale: f32, clip_xy: f64, fallback: bool) -> [u8; 4] {
    use rs_math3d::{Mat3d, Mat4d, RealVector, Vec3d, Vec4d};

    let palette: Vec<_> = palette
        .iter()
        .map(|matrix| Mat4d {
            col: matrix.col.map(|column| Vec4d::from_array(column.to_array().map(f64::from))),
        })
        .collect();
    let deformed = |vertex: &scene::Vertex| {
        let transform = Mat4d {
            col: std::array::from_fn(|axis| {
                let joint = |index: usize| palette[vertex.joints[index] as usize].col[axis] * f64::from(vertex.weights[index]);
                joint(0) + joint(1) + joint(2) + joint(3)
            }),
        };
        let [x, y, z] = vertex.position.map(f64::from);
        let clip = transform * Vec4d::new(x, y, z, 1.0) * f64::from(projection_scale);
        let normal = Vec3d::from_array(vertex.normal.map(f64::from));
        let linear = Mat3d {
            col: std::array::from_fn(|i| {
                let axis = transform.col[i];
                Vec3d::new(axis.x, axis.y, axis.z)
            }),
        };
        let normal = if fallback { normal } else { linear.inverse_rows().transpose() * normal };
        ([clip.x, clip.y], normal)
    };
    let (a, na) = deformed(&primitive.vertices[0]);
    let (b, nb) = deformed(&primitive.vertices[1]);
    let (c, nc) = deformed(&primitive.vertices[2]);
    // All fixture clip-space w values are 1, so perspective interpolation is affine.
    let cross = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let p = [clip_xy; 2];
    let area = cross(a, b, c);
    let weights = [cross(p, b, c) / area, cross(p, c, a) / area, cross(p, a, b) / area];
    assert!(weights.iter().all(|w| *w > 0.03), "reference pixel must be inside the triangle: {weights:?}");
    let rgb = (na * weights[0] + nb * weights[1] + nc * weights[2])
        .normalize()
        .to_array()
        .map(|n| ((n * 0.5 + 0.5) * 255.0).round() as u8);
    [rgb[0], rgb[1], rgb[2], 255]
}

/// Runs the production vertex shader and normal normalization on imported geometry through native COM.
#[cfg(all(feature = "test-support", feature = "glow"))]
#[test]
#[ignore = "requires a native SDL GL driver; SDL_VIDEODRIVER=offscreen is sufficient"]
fn normals_preserve_rotation_mirroring_and_skin_interpolation() {
    let _native = NATIVE_SESSION.lock().expect("previous native viewer case panicked");
    let _runtime = neocogi_com_host::initialize_runtime();
    let size = Size { width: 64, height: 64 };
    let native = neocogi_platform_examples::sdk::platform::testing::window("glTF normal transforms", size).unwrap();
    let target = Target::new(native.window).unwrap();
    let vertex = include_str!("viewer.sexp").split_once("(struct Material").unwrap().0;
    let source = Bytes::source(
        format!(
            "(module {vertex}
         (fragment fs ((surface Surface)) (returns vec4 (location 0))
           (return (vec4 (+ (* (surface_normal (. surface normal)) 0.5) (vec3 0.5)) 1.0))))"
        )
        .into_bytes(),
    );
    let resources = &target.resources;
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let descriptor = PipelineDesc {
        // Keep reflected geometry visible to compare the sign of its normal as well as its magnitude.
        state: GpuPipelineInfo {
            cull: GpuCullMode::None,
            ..Default::default()
        },
        vertex_buffers: vec![GpuVertexBufferLayout {
            slot: 0,
            stride: VERTEX_STRIDE,
            step: GpuStepMode::Vertex,
        }],
        attributes: [
            (GpuVertexFormat::Float32x3, 0),
            (GpuVertexFormat::Float32x3, 12),
            (GpuVertexFormat::Float32x2, 24),
            (GpuVertexFormat::Uint32x4, 32),
            (GpuVertexFormat::Float32x4, 48),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (format, offset))| GpuVertexAttribute {
            location: index as u32,
            slot: 0,
            format,
            offset,
        })
        .collect(),
        bindings: [64, (MAX_JOINTS * 64) as u32]
            .into_iter()
            .enumerate()
            .map(|(index, bytes)| GpuShaderBinding {
                stage: GpuShaderStage::Vertex,
                slot: index as u32,
                kind: GpuBindingKind::Uniform(bytes),
            })
            .collect(),
    }
    .source()
    .unwrap();
    let pipeline = resources.create_pipeline(descriptor.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let rotate = |xy, z| [[0., xy, 0.], [-xy, 0., 0.], [0., 0., z]];
    let cases = [
        ("unit", vec![rotate(1., 1.)], None, 1., false),
        ("centi", vec![rotate(0.01, 0.01)], None, 100., false),
        ("milli", vec![rotate(0.001, 0.001)], None, 1000., false),
        ("tiny", vec![rotate(1e-20, 1e-20)], None, 1e20, false),
        ("large", vec![rotate(1e20, 1e20)], None, 1e-20, false),
        ("subnormal inverse", vec![rotate(8e37, 8e37)], None, 1.25e-38, true),
        ("mirrored nonuniform", vec![[[0., -0.8, 0.], [-0.55, 0., 0.], [0., 0., 0.4]]], None, 1., false),
        ("near singular", vec![rotate(1., 1e-9)], None, 1., true),
        ("singular", vec![rotate(1., 0.)], None, 1., true),
        (
            "unrepresentable inverse",
            vec![[[4e-38, 0., 4e-38], [0., 2e-38, 0.], [2e-38, 0., 2.00002e-38]]],
            None,
            2.5e37,
            true,
        ),
        (
            "varying skin magnitudes",
            vec![
                [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                [[0.125, 0.21650635, 0.], [-0.21650635, 0.125, 0.], [0., 0., 0.25]],
            ],
            Some([[1., 0.], [0., 1.], [0.5, 0.5]]),
            1.,
            false,
        ),
    ];
    let offset = ((size.height / 2 * size.width + size.width / 2) * 4) as usize;
    let clip_xy = 1.0 / f64::from(size.width);
    for (index, (label, transforms, weights, projection, fallback)) in cases.into_iter().enumerate() {
        let scene = normal_scene(&transforms, weights);
        let primitive = &scene.primitives[0];
        let palette = scene.palette(primitive, &scene.pose(0.));
        let expected = reference_normal_pixel(primitive, &palette, projection, clip_xy, fallback);
        let mut vertices = Vec::new();
        primitive.vertices.iter().for_each(|vertex| vertex.encode(&mut vertices));
        let vertices = buffer(resources, GpuBufferRole::Vertex, vertices).unwrap();
        let mut skin = Vec::new();
        palette.into_iter().for_each(|matrix| matrix_bytes(&mut skin, matrix));
        skin.resize(MAX_JOINTS * 64, 0);
        let skin = buffer(resources, GpuBufferRole::Uniform, skin).unwrap();
        let mut frame = Vec::new();
        floats(
            &mut frame,
            [projection, 0., 0., 0., 0., projection, 0., 0., 0., 0., projection, 0., 0., 0., 0., 1.],
        );
        let frame = buffer(resources, GpuBufferRole::Uniform, frame).unwrap();
        target
            .submit(size, (index + 1) as u64, |view| {
                view.bind_pipeline(pipeline.as_com_ref())?;
                view.bind_uniform_buffer(GpuShaderStage::Vertex, 0, frame.as_com_ref(), 0, 64)?;
                view.bind_uniform_buffer(GpuShaderStage::Vertex, 1, skin.as_com_ref(), 0, (MAX_JOINTS * 64) as u64)?;
                view.bind_vertex_buffer(0, vertices.as_com_ref(), 0)?;
                view.draw(3, 1, 0)
            })
            .unwrap();
        let pixels = native.probe.pixels().unwrap();
        let actual = &pixels[offset..offset + 4];
        assert!(
            actual.iter().zip(expected).all(|(&actual, expected)| actual.abs_diff(expected) <= 1),
            "{label}: GPU {actual:?}, inverse-transpose reference {expected:?}"
        );
    }
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
        vec!["--backend"],
        vec!["--backend", "--frames", "3"],
        vec!["--backend", "vulkan", "--backend", "glow"],
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
    let options = parse(&["--backend", "vulkan", "--frames", "3"]).unwrap().unwrap();
    assert_eq!(options.backend.as_deref(), Some("vulkan"));
    assert_eq!(options.frames, Some(3));
}

/// Uses real uploaded meshes, palette uniforms, textures and native depth-tested indexed draws.
/// Optional documentation PNGs go outside the checkout and never change the acceptance camera.
#[cfg(all(feature = "test-support", feature = "glow"))]
#[test]
#[ignore = "requires downloaded GLBs and a native SDL GL driver (offscreen is sufficient)"]
fn models_render_animate_and_respond_to_arcball_pan_zoom() {
    let _native = NATIVE_SESSION.lock().expect("previous native viewer case panicked");
    let _runtime = neocogi_com_host::initialize_runtime();
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
            let pixels = if model == "DamagedHelmet" {
                let mut preview = Camera::new(center, radius, window.size().unwrap());
                // A sideward/downward arcball drag puts the eye slightly above and
                // beside the helmet. Keep the interactive reset view unchanged.
                preview.input(UiInput::Down {
                    position: Point { x: 480, y: 360 },
                    buttons: 1,
                });
                preview.input(UiInput::Pointer(Point { x: 680, y: 460 }));
                preview.input(UiInput::Up {
                    position: Point { x: 680, y: 460 },
                    buttons: 1,
                });
                preview.input(UiInput::Scroll(Point { x: 0, y: -130 }));
                draw(&preview, &lighting, 0.)
            } else {
                with_sky
            };
            let mut image = image::RgbaImage::from_raw(SIZE.width as u32, SIZE.height as u32, pixels).unwrap();
            // GL acceptance pixels follow glReadPixels bottom-up row order.
            image::imageops::flip_vertical_in_place(&mut image);
            std::fs::create_dir_all(&directory).unwrap();
            image.save(PathBuf::from(directory).join(format!("{model}.png"))).unwrap();
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
            let camera = camera.frame().unwrap();
            target
                .submit(camera.drawable_size(), invocation, |view| graphics.record_sky(&camera, &lighting, view))
                .unwrap();
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
    let shader = Bytes::source(neocogi_platform_examples::shaders::lighting(
        r#"
(vertex vs ((index u32 (builtin vertex_index))) (returns vec4 (builtin position)) (let points ((array vec2 3) (vec2 (- 1.0) (- 1.0)) (vec2 3.0 (- 1.0)) (vec2 (- 1.0) 3.0))) (return (vec4 (index points index) 0.5 1.0)))
(fragment fs () (returns vec4 (location 0)) (return (vec4 (/ (environment_tile 0u (vec2 0.5) true true) 8.0) 1.0)))
"#,
    ).into_bytes());
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
                kind: GpuBindingKind::Uniform(environment::UNIFORM_BYTES),
            },
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 1,
                kind: GpuBindingKind::SampledTexture,
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
        .submit(target.window.size().unwrap(), 1, |view| {
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
