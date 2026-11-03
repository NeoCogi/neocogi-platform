//! Example-owned glTF scene data. Import/animation never becomes a renderer contract.
//!
//! The viewer supports the two pinned core assets, not arbitrary glTF extensions: indexed
//! triangles, normals, UV0, four joint influences, opaque materials and LINEAR/STEP TRS.
//! Unsupported topology, morphing and animation interpolation fail explicitly.

use rs_math3d::{Mat4f, Quatf, Vec3f, Vec4f, scale, translate};
use std::path::Path;

/// Palette capacity shared with the example vertex shader (4 KiB, below portable limits).
pub const MAX_JOINTS: usize = 64;
/// Explicit packed position/normal/UV/joints/weights layout, independent of Rust struct layout.
pub const VERTEX_STRIDE: u32 = 64;

/// One example failure with operation-specific diagnostics; no erased payload or downcast.
pub type Result<T> = std::result::Result<T, String>;

/// Imported mesh-local vertex. Joint numbers index its skin, not the scene node array.
pub struct Vertex {
    /// Mesh-local position.
    pub position: [f32; 3],
    /// Mesh-local surface normal.
    pub normal: [f32; 3],
    /// Top-left-origin primary texture coordinates.
    pub uv: [f32; 2],
    /// Four bounded palette indices, explicitly uploaded as u32.
    pub joints: [u32; 4],
    /// Nonnegative weights normalized once on import.
    pub weights: [f32; 4],
}

impl Vertex {
    /// Serializes the declared 64-byte GPU stream using little-endian scalars.
    pub fn encode(&self, bytes: &mut Vec<u8>) {
        floats(bytes, self.position.into_iter().chain(self.normal).chain(self.uv));
        bytes.extend(self.joints.iter().flat_map(|value| value.to_le_bytes()));
        floats(bytes, self.weights);
    }
}

/// One primitive instance. Buffer owners will live in the viewer for its full render loop.
pub struct Primitive {
    /// Scene node carrying this instance's static transform or skin.
    pub node: usize,
    /// Optional scene skin index; absent uses a rigid one-matrix palette.
    pub skin: Option<usize>,
    /// Index into the scene material list, including its final default material.
    pub material: usize,
    /// Validated vertices retained for upload and initial camera bounds.
    pub vertices: Vec<Vertex>,
    /// Triangle-list indices, all strictly inside `vertices`.
    pub indices: Vec<u32>,
}

/// Linear material values and image references; sampled color images are decoded in the shader.
pub struct Material {
    /// Linear RGBA base color multiplier.
    pub base: [f32; 4],
    /// Linear emissive RGB multiplier.
    pub emissive: [f32; 3],
    /// Metallic and roughness multipliers, in that order.
    pub metal_rough: [f32; 2],
    /// Normal-map XY scale.
    pub normal_scale: f32,
    /// Base color, metallic/roughness, normal and emissive image indices.
    pub images: [Option<usize>; 4],
}

/// Immutable tightly packed RGBA8 image with glTF row ordering (no vertical flip).
pub struct Image {
    /// Positive pixel width.
    pub width: u32,
    /// Positive pixel height.
    pub height: u32,
    /// Exactly width * height * 4 initialized bytes.
    pub pixels: Vec<u8>,
}

#[derive(Clone)]
/// Rest or sampled node transform; glTF multiplication order is T * R * S.
struct Transform {
    /// Parent-local translation.
    translation: [f32; 3],
    /// Normalized XYZW rotation quaternion.
    rotation: [f32; 4],
    /// Parent-local axis scales.
    scale: [f32; 3],
}

impl Transform {
    /// Builds a column-major matrix, keeping the file's quaternion ordering explicit.
    fn matrix(&self) -> Mat4f {
        translate(Vec3f::new(self.translation[0], self.translation[1], self.translation[2]))
            * Quatf::new(self.rotation[0], self.rotation[1], self.rotation[2], self.rotation[3]).mat4()
            * scale(Vec3f::new(self.scale[0], self.scale[1], self.scale[2]))
    }
}

/// Rest transform plus an optional exact matrix for non-animated matrix nodes.
struct Node {
    /// At most one parent; cycles and shared children reject at import.
    parent: Option<usize>,
    /// TRS rest pose restored before every animation sample.
    rest: Transform,
    /// Exact authored matrix, preserved without decomposition when present.
    matrix: Option<Mat4f>,
}

/// Per-skin joint order and the corresponding inverse bind matrices.
struct Skin {
    /// Scene node index for each palette entry.
    joints: Vec<usize>,
    /// Matching mesh-to-joint rest-space transforms.
    inverse_bind: Vec<Mat4f>,
}

/// Typed animation outputs prevent rotation data from being treated as translation/scale.
enum Values {
    /// Translation keys in parent space.
    Translation(Vec<[f32; 3]>),
    /// Quaternion XYZW keys, interpolated along the shortest spherical arc.
    Rotation(Vec<[f32; 4]>),
    /// Axis-scale keys.
    Scale(Vec<[f32; 3]>),
}

/// One validated channel in the first clip; time is in seconds.
struct Channel {
    /// Scene node whose TRS component is replaced.
    node: usize,
    /// Finite, nonnegative, strictly increasing input keys.
    times: Vec<f32>,
    /// A value per input key, with explicit property type.
    values: Values,
    /// STEP holds the earlier key; false means LINEAR (spherical for rotations).
    step: bool,
}

/// Complete owned sample scene; mutable pose is produced separately for each frame.
pub struct Scene {
    /// Primitive instances in the selected default (or first) scene.
    pub primitives: Vec<Primitive>,
    /// Authored materials followed by a default material.
    pub materials: Vec<Material>,
    /// Decoded images shared by material references.
    pub images: Vec<Image>,
    /// Complete node array, including joints without meshes.
    nodes: Vec<Node>,
    /// Parent-before-child node indices, independent of document index order.
    order: Vec<usize>,
    /// Palettes referenced by primitive instances.
    skins: Vec<Skin>,
    /// First animation clip; the pinned BrainStem has exactly one.
    channels: Vec<Channel>,
    /// Last key time of the first clip, or zero for a static scene.
    pub duration: f32,
}

impl Scene {
    /// Imports a local glTF/GLB plus referenced files, reporting a useful missing-asset hint.
    pub fn load(path: &Path) -> Result<Self> {
        let (document, buffers, images) =
            gltf::import(path).map_err(|error| format!("{}: {error}; fetch samples with python3 scripts/fetch_gltf_examples.py", path.display()))?;
        Self::import(document, buffers, images)
    }

    /// Resolves supported file semantics into owned example data before GPU allocation.
    fn import(document: gltf::Document, buffers: Vec<gltf::buffer::Data>, images: Vec<gltf::image::Data>) -> Result<Self> {
        if document.extensions_required().next().is_some() {
            return Err("required glTF extensions are unsupported by these examples".into());
        }
        let data = |buffer: gltf::Buffer<'_>| buffers.get(buffer.index()).map(|data| data.0.as_slice());
        let mut nodes: Vec<_> = document
            .nodes()
            .map(|node| {
                let (translation, rotation, scale) = node.transform().decomposed();
                Node {
                    parent: None,
                    rest: Transform { translation, rotation, scale },
                    matrix: match node.transform() {
                        gltf::scene::Transform::Matrix { matrix } => Some(matrix_from_columns(matrix)),
                        gltf::scene::Transform::Decomposed { .. } => None,
                    },
                }
            })
            .collect();
        for node in document.nodes() {
            for child in node.children() {
                if nodes[child.index()].parent.replace(node.index()).is_some() {
                    return Err("a node has multiple parents".into());
                }
            }
        }
        let mut order = Vec::new();
        let mut pending: Vec<_> = (0..nodes.len()).collect();
        while !pending.is_empty() {
            let before = pending.len();
            pending.retain(|index| {
                if nodes[*index].parent.is_none_or(|parent| order.contains(&parent)) {
                    order.push(*index);
                    false
                } else {
                    true
                }
            });
            if pending.len() == before {
                return Err("cyclic node hierarchy".into());
            }
        }
        let skins: Vec<_> = document
            .skins()
            .map(|skin| {
                let joints: Vec<_> = skin.joints().map(|node| node.index()).collect();
                if joints.is_empty() || joints.len() > MAX_JOINTS {
                    return Err("skin must contain 1..=64 joints".into());
                }
                let inverse_bind: Vec<_> = skin
                    .reader(data)
                    .read_inverse_bind_matrices()
                    .map(|values| values.map(matrix_from_columns).collect())
                    .unwrap_or_else(|| vec![Mat4f::identity(); joints.len()]);
                if inverse_bind.len() != joints.len() {
                    return Err("inverse bind matrix count differs from joint count".into());
                }
                Ok(Skin { joints, inverse_bind })
            })
            .collect::<Result<_>>()?;
        let mut materials = document
            .materials()
            .map(|material| {
                if material.alpha_mode() != gltf::material::AlphaMode::Opaque {
                    return Err("only opaque sample materials are supported".into());
                }
                let pbr = material.pbr_metallic_roughness();
                let base = pbr.base_color_texture();
                let mr = pbr.metallic_roughness_texture();
                let normal = material.normal_texture();
                let emissive = material.emissive_texture();
                if base.as_ref().is_some_and(|t| t.tex_coord() != 0)
                    || mr.as_ref().is_some_and(|t| t.tex_coord() != 0)
                    || normal.as_ref().is_some_and(|t| t.tex_coord() != 0)
                    || emissive.as_ref().is_some_and(|t| t.tex_coord() != 0)
                {
                    return Err("only TEXCOORD_0 is supported".into());
                }
                Ok(Material {
                    base: pbr.base_color_factor(),
                    emissive: material.emissive_factor(),
                    metal_rough: [pbr.metallic_factor(), pbr.roughness_factor()],
                    normal_scale: normal.as_ref().map_or(1.0, |t| t.scale()),
                    images: [
                        base.map(|t| t.texture().source().index()),
                        mr.map(|t| t.texture().source().index()),
                        normal.map(|t| t.texture().source().index()),
                        emissive.map(|t| t.texture().source().index()),
                    ],
                })
            })
            .collect::<Result<Vec<_>>>()?;
        materials.push(Material {
            base: [1.0; 4],
            emissive: [0.0; 3],
            metal_rough: [1.0; 2],
            normal_scale: 1.0,
            images: [None; 4],
        });
        let scene = document.default_scene().or_else(|| document.scenes().next()).ok_or("glTF has no scene")?;
        let mut stack: Vec<_> = scene.nodes().collect();
        let mut primitives = Vec::new();
        while let Some(node) = stack.pop() {
            stack.extend(node.children());
            let Some(mesh) = node.mesh() else { continue };
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles || primitive.morph_targets().next().is_some() {
                    return Err("examples require triangle lists without morph targets".into());
                }
                let reader = primitive.reader(data);
                let positions: Vec<_> = reader.read_positions().ok_or("primitive lacks POSITION")?.collect();
                let normals: Vec<_> = reader.read_normals().ok_or("primitive lacks NORMAL")?.collect();
                let uvs: Vec<_> = reader
                    .read_tex_coords(0)
                    .map(|v| v.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.0; 2]; positions.len()]);
                let skin = node.skin().map(|skin| skin.index());
                let joints: Vec<_> = reader
                    .read_joints(0)
                    .map(|v| v.into_u16().map(|v| v.map(u32::from)).collect())
                    .unwrap_or_else(|| if skin.is_none() { vec![[0; 4]; positions.len()] } else { vec![] });
                let weights: Vec<_> = reader.read_weights(0).map(|v| v.into_f32().collect()).unwrap_or_else(|| {
                    if skin.is_none() {
                        vec![[1.0, 0.0, 0.0, 0.0]; positions.len()]
                    } else {
                        vec![]
                    }
                });
                let count = positions.len();
                if count == 0 || [normals.len(), uvs.len(), joints.len(), weights.len()].iter().any(|len| *len != count) {
                    return Err("missing or mismatched vertex attributes".into());
                }
                let mut vertices = Vec::with_capacity(count);
                for i in 0..count {
                    let mut weights = weights[i];
                    let sum: f32 = weights.iter().sum();
                    let joint_count = skin.map_or(1, |index| skins[index].joints.len());
                    if !sum.is_finite()
                        || sum <= 0.0
                        || weights.iter().any(|w| !w.is_finite() || *w < 0.0)
                        || joints[i].iter().any(|j| *j as usize >= joint_count)
                        || positions[i].iter().chain(normals[i].iter()).chain(uvs[i].iter()).any(|v| !v.is_finite())
                    {
                        return Err("invalid vertex, joint index or skin weight".into());
                    }
                    weights.iter_mut().for_each(|w| *w /= sum);
                    vertices.push(Vertex {
                        position: positions[i],
                        normal: normals[i],
                        uv: uvs[i],
                        joints: joints[i],
                        weights,
                    });
                }
                let indices: Vec<_> = reader.read_indices().ok_or("examples require indexed geometry")?.into_u32().collect();
                if indices.is_empty() || indices.len() % 3 != 0 || indices.iter().any(|i| *i as usize >= count) {
                    return Err("invalid triangle index list".into());
                }
                primitives.push(Primitive {
                    node: node.index(),
                    skin,
                    material: primitive.material().index().unwrap_or(materials.len() - 1),
                    vertices,
                    indices,
                });
            }
        }
        if primitives.is_empty() {
            return Err("selected scene contains no meshes".into());
        }
        let mut channels = Vec::new();
        let mut duration: f32 = 0.0;
        if let Some(animation) = document.animations().next() {
            for channel in animation.channels() {
                let node = channel.target().node().index();
                if nodes[node].matrix.is_some() {
                    return Err("animation targets a matrix node".into());
                }
                let step = match channel.sampler().interpolation() {
                    gltf::animation::Interpolation::Linear => false,
                    gltf::animation::Interpolation::Step => true,
                    gltf::animation::Interpolation::CubicSpline => return Err("CUBICSPLINE animation is unsupported".into()),
                };
                let reader = channel.reader(data);
                let times: Vec<_> = reader.read_inputs().ok_or("animation lacks input keys")?.collect();
                if times.is_empty() || times.iter().any(|t| !t.is_finite() || *t < 0.0) || times.windows(2).any(|v| v[0] >= v[1]) {
                    return Err("animation times must be finite and strictly increasing".into());
                }
                use gltf::animation::util::ReadOutputs;
                let values = match reader.read_outputs().ok_or("animation lacks output keys")? {
                    ReadOutputs::Translations(v) => Values::Translation(v.collect()),
                    ReadOutputs::Rotations(v) => Values::Rotation(v.into_f32().collect()),
                    ReadOutputs::Scales(v) => Values::Scale(v.collect()),
                    ReadOutputs::MorphTargetWeights(_) => return Err("morph animation is unsupported".into()),
                };
                let valid = match &values {
                    Values::Translation(v) | Values::Scale(v) => v.len() == times.len() && v.iter().flatten().all(|v| v.is_finite()),
                    Values::Rotation(v) => {
                        v.len() == times.len()
                            && v.iter()
                                .all(|q| q.iter().all(|v| v.is_finite()) && q.iter().map(|v| v * v).sum::<f32>() > 1e-12)
                    }
                };
                if !valid {
                    return Err("invalid animation output keys".into());
                }
                duration = duration.max(*times.last().unwrap());
                channels.push(Channel { node, times, values, step });
            }
        }
        let images = images
            .into_iter()
            .map(|image| {
                let channels = match image.format {
                    gltf::image::Format::R8 => 1,
                    gltf::image::Format::R8G8 => 2,
                    gltf::image::Format::R8G8B8 => 3,
                    gltf::image::Format::R8G8B8A8 => 4,
                    _ => return Err("examples support 8-bit sample images only".into()),
                };
                let pixels = image
                    .pixels
                    .chunks_exact(channels)
                    .flat_map(|p| match channels {
                        1 => [p[0], p[0], p[0], 255],
                        2 => [p[0], p[0], p[0], p[1]],
                        3 => [p[0], p[1], p[2], 255],
                        _ => [p[0], p[1], p[2], p[3]],
                    })
                    .collect();
                Ok(Image {
                    width: image.width,
                    height: image.height,
                    pixels,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            primitives,
            materials,
            images,
            nodes,
            order,
            skins,
            channels,
            duration,
        })
    }

    /// Samples the first clip in a loop. Static scenes keep their exact authored transforms.
    pub fn pose(&self, seconds: f32) -> Vec<Mat4f> {
        let time = if self.duration > 0.0 { seconds.rem_euclid(self.duration) } else { 0.0 };
        let mut transforms: Vec<_> = self.nodes.iter().map(|n| n.rest.clone()).collect();
        for channel in &self.channels {
            let (a, b, factor) = interval(&channel.times, time, channel.step);
            let transform = &mut transforms[channel.node];
            match &channel.values {
                Values::Translation(v) => transform.translation = std::array::from_fn(|i| v[a][i] + (v[b][i] - v[a][i]) * factor),
                Values::Scale(v) => transform.scale = std::array::from_fn(|i| v[a][i] + (v[b][i] - v[a][i]) * factor),
                Values::Rotation(v) => transform.rotation = slerp(v[a], v[b], factor),
            }
        }
        let mut world = vec![Mat4f::identity(); self.nodes.len()];
        for &index in &self.order {
            let node = &self.nodes[index];
            let local = node.matrix.unwrap_or_else(|| transforms[index].matrix());
            world[index] = node.parent.map_or(local, |parent| world[parent] * local);
        }
        world
    }

    /// Produces world-space joint matrices: joint_world * inverse_bind. Skinned nodes' own
    /// transforms are intentionally excluded, avoiding applying the mesh transform twice.
    pub fn palette(&self, primitive: &Primitive, pose: &[Mat4f]) -> Vec<Mat4f> {
        match primitive.skin {
            Some(index) => self.skins[index]
                .joints
                .iter()
                .zip(&self.skins[index].inverse_bind)
                .map(|(&joint, inverse)| pose[joint] * *inverse)
                .collect(),
            None => vec![pose[primitive.node]],
        }
    }

    /// Computes initial animated world-space bounds, including weighted bind matrices.
    pub fn bounds(&self) -> ([f32; 3], f32) {
        let pose = self.pose(0.0);
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for primitive in &self.primitives {
            let palette = self.palette(primitive, &pose);
            for vertex in &primitive.vertices {
                let mut position = Vec4f::new(0.0, 0.0, 0.0, 0.0);
                for i in 0..4 {
                    position = position
                        + (palette[vertex.joints[i] as usize] * Vec4f::new(vertex.position[0], vertex.position[1], vertex.position[2], 1.0))
                            * vertex.weights[i];
                }
                for (axis, value) in [position.x, position.y, position.z].into_iter().enumerate() {
                    min[axis] = min[axis].min(value);
                    max[axis] = max[axis].max(value);
                }
            }
        }
        let center = std::array::from_fn(|i| (min[i] + max[i]) * 0.5);
        let radius = (0..3).map(|i| (max[i] - min[i]).powi(2)).sum::<f32>().sqrt() * 0.5;
        (center, radius.max(0.001))
    }
}

/// Selects a clamped key interval; exact keys and single-key channels never divide by zero.
fn interval(times: &[f32], time: f32, step: bool) -> (usize, usize, f32) {
    let a = times.partition_point(|t| *t <= time).saturating_sub(1);
    let b = (a + 1).min(times.len() - 1);
    let factor = if step || a == b {
        0.0
    } else {
        ((time - times[a]) / (times[b] - times[a])).clamp(0.0, 1.0)
    };
    (a, b, factor)
}

/// Normalized shortest-path quaternion interpolation, including antipodal equivalent keys.
fn slerp(a: [f32; 4], mut b: [f32; 4], t: f32) -> [f32; 4] {
    let dot: f32 = a.iter().zip(b).map(|(a, b)| a * b).sum();
    if dot < 0.0 {
        b.iter_mut().for_each(|v| *v = -*v);
    }
    let dot = dot.abs().min(1.0);
    let (left, right) = if dot > 0.9995 {
        (1.0 - t, t)
    } else {
        let angle = dot.acos();
        (((1.0 - t) * angle).sin() / angle.sin(), (t * angle).sin() / angle.sin())
    };
    let mut q: [f32; 4] = std::array::from_fn(|i| a[i] * left + b[i] * right);
    let norm = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    q.iter_mut().for_each(|v| *v /= norm);
    q
}

/// Converts glTF's explicit column vectors to the math crate without transposition.
fn matrix_from_columns(columns: [[f32; 4]; 4]) -> Mat4f {
    Mat4f {
        col: columns.map(|v| Vec4f::new(v[0], v[1], v[2], v[3])),
    }
}

/// Appends GPU f32 scalars without relying on Rust struct layout or unsafe casts.
pub fn floats(bytes: &mut Vec<u8>, values: impl IntoIterator<Item = f32>) {
    bytes.extend(values.into_iter().flat_map(f32::to_le_bytes));
}

/// Serializes one std140-compatible column-major matrix (64 bytes).
pub fn matrix_bytes(bytes: &mut Vec<u8>, matrix: Mat4f) {
    for column in matrix.col {
        floats(bytes, [column.x, column.y, column.z, column.w]);
    }
}

#[cfg(test)]
#[path = "scene_tests.rs"]
mod tests;
