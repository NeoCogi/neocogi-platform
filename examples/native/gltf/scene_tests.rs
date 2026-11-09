//! Synthetic, offline fixtures exercise actual accessor readers and skin/animation resolution.
use super::*;
use serde_json::{Value, json};

/// A complete tiny triangle with a mesh, child joint, parent joint and animated translation.
/// Node indices deliberately put the child before its parent.
fn fixture(change: impl FnOnce(&mut Value, &mut Vec<u8>)) -> Result<Scene> {
    let mut bytes = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut add = |values: &[f32], count: usize, shape: &str| {
        let offset = bytes.len();
        floats(&mut bytes, values.iter().copied());
        let index = accessors.len();
        views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":values.len()*4}));
        accessors.push(json!({"bufferView":index,"componentType":5126,"count":count,"type":shape}));
        index
    };
    let positions = add(&[0., 0., 0., 1., 0., 0., 0., 1., 0.], 3, "VEC3");
    let normals = add(&[0., 0., 1., 0., 0., 1., 0., 0., 1.], 3, "VEC3");
    let weights = add(&[2., 0., 0., 0., 2., 0., 0., 0., 2., 0., 0., 0.], 3, "VEC4");
    let times = add(&[0., 2.], 2, "SCALAR");
    let translations = add(&[0., 0., 0., 4., 0., 0.], 2, "VEC3");
    let index_offset = bytes.len();
    bytes.extend([0_u16, 1, 2].into_iter().flat_map(u16::to_le_bytes));
    bytes.extend([0; 2]);
    let index_view = views.len();
    views.push(json!({"buffer":0,"byteOffset":index_offset,"byteLength":6}));
    let indices = accessors.len();
    accessors.push(json!({"bufferView":index_view,"componentType":5123,"count":3,"type":"SCALAR"}));
    let joint_offset = bytes.len();
    bytes.extend([0_u16; 12].into_iter().flat_map(u16::to_le_bytes));
    let joint_view = views.len();
    views.push(json!({"buffer":0,"byteOffset":joint_offset,"byteLength":24}));
    let joints = accessors.len();
    accessors.push(json!({"bufferView":joint_view,"componentType":5123,"count":3,"type":"VEC4"}));
    accessors[positions]["min"] = json!([0., 0., 0.]);
    accessors[positions]["max"] = json!([1., 1., 0.]);
    let mut document = json!({
        "asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0,2]}],
        "nodes":[{"mesh":0,"skin":0,"translation":[100.,0.,0.]},{"translation":[0.,3.,0.]},{"children":[1]}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":positions,"NORMAL":normals,"JOINTS_0":joints,"WEIGHTS_0":weights},"indices":indices}]}],
        "skins":[{"joints":[1]}],
        "animations":[{"channels":[{"sampler":0,"target":{"node":2,"path":"translation"}}],"samplers":[{"input":times,"output":translations}]}],
        "buffers":[{"byteLength":bytes.len()}],"bufferViews":views,"accessors":accessors
    });
    change(&mut document, &mut bytes);
    let parsed = gltf::Gltf::from_slice(&serde_json::to_vec(&document).unwrap()).map_err(|e| e.to_string())?;
    Scene::import(parsed.document, vec![gltf::buffer::Data(bytes)], vec![])
}

/// Checks parent ordering, weight normalization, absent inverse binds and skin-space ownership.
#[test]
fn samples_hierarchy_and_skin_without_double_mesh_transform() {
    let scene = fixture(|_, _| {}).unwrap();
    assert_eq!(scene.primitives.len(), 1);
    assert_eq!(scene.primitives[0].vertices[0].weights, [1., 0., 0., 0.]);
    let pose = scene.pose(1.0);
    let palette = scene.palette(&scene.primitives[0], &pose);
    assert_eq!(palette[0].col[3].x, 2.0);
    assert_eq!(palette[0].col[3].y, 3.0);
    assert_eq!(scene.pose(3.0)[1].col[3].x, 2.0);
    assert_eq!(scene.pose(2.0)[1].col[3].x, 0.0);
    let (center, radius) = scene.bounds();
    assert_eq!(center, [0.5, 3.5, 0.0]);
    assert!((radius - 0.5_f32.sqrt()).abs() < 1e-6);
}

/// Rigid geometry uses its own world matrix, while STEP holds its previous key.
#[test]
fn rigid_nodes_and_step_animation_keep_authored_semantics() {
    let scene = fixture(|doc, _| {
        doc["nodes"][0].as_object_mut().unwrap().remove("skin");
        doc["animations"][0]["samplers"][0]["interpolation"] = json!("STEP");
    })
    .unwrap();
    assert_eq!(scene.palette(&scene.primitives[0], &scene.pose(1.0))[0].col[3].x, 100.0);
    assert_eq!(scene.pose(1.0)[1].col[3].x, 0.0);
}

/// Each unsupported or inconsistent boundary fails before any native resource is created.
#[test]
fn rejects_unsupported_and_inconsistent_inputs() {
    assert!(fixture(|d, _| d["meshes"][0]["primitives"][0]["mode"] = json!(1)).is_err());
    assert!(fixture(|d, _| d["accessors"][1]["count"] = json!(2)).is_err());
    assert!(
        fixture(|d, _| {
            d["meshes"][0]["primitives"][0]["attributes"].as_object_mut().unwrap().remove("JOINTS_0");
        })
        .is_err()
    );
    assert!(fixture(|d, _| d["animations"][0]["samplers"][0]["interpolation"] = json!("CUBICSPLINE")).is_err());
    assert!(
        fixture(|d, b| {
            let offset = d["bufferViews"][3]["byteOffset"].as_u64().unwrap() as usize;
            b[offset + 4..offset + 8].copy_from_slice(&0_f32.to_le_bytes());
        })
        .is_err()
    );
    assert!(
        fixture(|d, b| {
            let offset = d["bufferViews"][6]["byteOffset"].as_u64().unwrap() as usize;
            b[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
        })
        .is_err()
    );
    assert!(
        fixture(|d, b| {
            let offset = d["bufferViews"][5]["byteOffset"].as_u64().unwrap() as usize;
            b[offset..offset + 2].copy_from_slice(&3_u16.to_le_bytes());
        })
        .is_err()
    );
    assert!(fixture(|d, _| d["nodes"][1]["children"] = json!([2])).is_err());
}

/// Exact key boundaries, clamping and antipodal quaternions avoid jumps and NaNs.
#[test]
fn interpolation_boundaries_and_shortest_arc() {
    assert_eq!(interval(&[1., 2., 3.], 0., false), (0, 1, 0.));
    assert_eq!(interval(&[1., 2., 3.], 2., false), (1, 2, 0.));
    assert_eq!(interval(&[1., 2., 3.], 4., false), (2, 2, 0.));
    assert_eq!(interval(&[1.], 1., false), (0, 0, 0.));
    assert_eq!(interval(&[0., 2.], 1., true), (0, 1, 0.));
    let same = slerp(Quatf::identity(), Quatf::new(0., 0., 0., -1.), 0.5);
    assert_eq!([same.x, same.y, same.z, same.w], [0., 0., 0., 1.]);
    let halfway = slerp(Quatf::identity(), Quatf::new(0., 0., 1., 0.), 0.5);
    assert!((halfway.z - 0.5_f32.sqrt()).abs() < 1e-6);
    let rotated = halfway.mat4() * Vec4f::new(1., 0., 0., 0.);
    assert!(rotated.x.abs() < 1e-6 && (rotated.y - 1.).abs() < 1e-6);
    // Near-parallel keys use the linear limit, with final normalization owned by the library.
    let small = Quatf::of_axis_angle(&Vec3f::new(0., 0., 1.), 0.001);
    let midway = slerp(Quatf::identity(), small, 0.5);
    let rotated = midway.mat4() * Vec4f::new(1., 0., 0., 0.);
    assert!((rotated.y - 0.0005_f32.sin()).abs() < 1e-6);
    assert!((midway.length() - 1.).abs() < 1e-6);
}

/// Bounds remain finite when the radius fits f32 but the old squared-distance sum would overflow.
#[test]
fn large_scene_bounds_use_a_representable_radius() {
    let mut scene = fixture(|_, _| {}).unwrap();
    for vertex in &mut scene.primitives[0].vertices {
        vertex.position = [1e30 + vertex.position[0] * 1e25, -1e30 + vertex.position[1] * 1e25, 0.];
    }
    let (center, radius) = scene.bounds();
    assert!(center.iter().all(|v| v.is_finite()) && radius.is_finite() && radius > 1e24);
    for vertex in &scene.primitives[0].vertices {
        let distance = vertex
            .position
            .into_iter()
            .zip(center)
            .map(|(v, c)| (f64::from(v) - f64::from(c)).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(distance <= f64::from(radius) * 1.02);
    }
}

/// Rounding a very narrow large box's center onto an edge must not collapse the camera sphere.
#[test]
fn adjacent_large_coordinates_remain_inside_the_camera_sphere() {
    let mut scene = fixture(|_, _| {}).unwrap();
    let low = f32::from_bits(1e30_f32.to_bits() | 1);
    let high = f32::from_bits(low.to_bits() + 1);
    for (vertex, [x, y]) in scene.primitives[0].vertices.iter_mut().zip([[low, low], [high, low], [low, high]]) {
        vertex.position = [x, y, 0.];
    }
    let (center, radius) = scene.bounds();
    assert!(radius.is_finite() && radius >= (high - low) * 1.4);
    for vertex in &scene.primitives[0].vertices {
        let distance = vertex
            .position
            .into_iter()
            .zip(center)
            .map(|(v, c)| (f64::from(v) - f64::from(c)).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(distance <= f64::from(radius) * 1.000001);
    }
}

/// Upload bytes have the documented stride and little-endian joint/matrix offsets.
#[test]
fn explicit_gpu_encoding() {
    let scene = fixture(|_, _| {}).unwrap();
    let mut bytes = Vec::new();
    scene.primitives[0].vertices[0].encode(&mut bytes);
    assert_eq!(bytes.len(), VERTEX_STRIDE as usize);
    assert_eq!(&bytes[48..52], &1_f32.to_le_bytes());
    matrix_bytes(&mut bytes, translate(Vec3f::new(2., 3., 4.)));
    assert_eq!(&bytes[112..116], &2_f32.to_le_bytes());
}

/// Matrix packing preserves all sixteen column-major scalar bit patterns and appends to owned bytes.
#[test]
fn column_major_packing_preserves_exact_scalar_bits() {
    let bits = [
        0x80000000, 0x7fc12345, 0x3f800000, 0xbf800000, 0x40000000, 0x40400000, 0x40800000, 0x40a00000, 0x40c00000, 0x40e00000, 0x41000000, 0x41100000,
        0x41200000, 0x41300000, 0x41400000, 0x41500000,
    ];
    let columns = std::array::from_fn(|column| std::array::from_fn(|row| f32::from_bits(bits[column * 4 + row])));
    let mut bytes = vec![0x5a; 3];
    matrix_bytes(&mut bytes, matrix_from_columns(columns));
    assert_eq!(&bytes[..3], &[0x5a; 3]);
    assert_eq!(&bytes[3..], bits.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>());
}

/// Explicit asset acceptance runs after the pinned download; ordinary tests stay offline.
#[test]
#[ignore = "requires cargo tools fetch-gltf-examples"]
fn pinned_assets_import_and_brainstem_deforms() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("gltf/assets");
    let helmet = Scene::load(&directory.join("DamagedHelmet.glb")).unwrap();
    assert_eq!(helmet.primitives.len(), 1);
    assert_eq!(helmet.duration, 0.0);
    assert!(helmet.materials[0].images.iter().all(Option::is_some));
    let brainstem = Scene::load(&directory.join("BrainStem.glb")).unwrap();
    assert_eq!(brainstem.skins[0].joints.len(), 18);
    assert!(brainstem.duration > 0.0);
    assert!(brainstem.primitives.iter().all(|p| p.skin.is_some()));
    let start = brainstem.pose(0.0);
    let later = brainstem.pose(brainstem.duration * 0.37);
    assert!(brainstem.skins[0].joints.iter().any(|j| {
        let mut a = Vec::new();
        let mut b = Vec::new();
        matrix_bytes(&mut a, start[*j]);
        matrix_bytes(&mut b, later[*j]);
        a != b
    }));
}
