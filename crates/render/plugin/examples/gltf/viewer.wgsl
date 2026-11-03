// Example vertex layout and explicitly encoded std140-compatible uniform blocks.
struct Frame { view_projection: mat4x4<f32> }
struct Skin { joints: array<mat4x4<f32>, 64> }
@group(1) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(1) var<uniform> skin: Skin;
struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
}
struct Surface {
    @builtin(position) clip: vec4<f32>,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}
@vertex fn vs(vertex: Vertex) -> Surface {
    let transform = skin.joints[vertex.joints.x] * vertex.weights.x
                  + skin.joints[vertex.joints.y] * vertex.weights.y
                  + skin.joints[vertex.joints.z] * vertex.weights.z
                  + skin.joints[vertex.joints.w] * vertex.weights.w;
    let position = transform * vec4(vertex.position, 1.0);
    // Cofactors implement inverse-transpose for nonuniform scaling without a native inverse.
    let a = transform[0].xyz;
    let b = transform[1].xyz;
    let c = transform[2].xyz;
    let determinant = dot(a, cross(b, c));
    var normal = vertex.normal;
    if abs(determinant) > 0.0000001 {
        normal = mat3x3(cross(b, c), cross(c, a), cross(a, b)) * normal / determinant;
    }
    var output: Surface;
    output.clip = frame.view_projection * position;
    output.position = position.xyz;
    output.normal = normal;
    output.uv = vertex.uv;
    return output;
}

struct Material {
    base: vec4<f32>,
    emissive: vec4<f32>,
    // Metallic, roughness, normal XY scale, reserved.
    parameters: vec4<f32>,
    eye: vec4<f32>,
}
@group(3) @binding(0) var<uniform> material: Material;
@group(2) @binding(0) var base_map: texture_2d<f32>;
@group(2) @binding(1) var base_sampler: sampler;
@group(2) @binding(2) var mr_map: texture_2d<f32>;
@group(2) @binding(3) var mr_sampler: sampler;
@group(2) @binding(4) var normal_map: texture_2d<f32>;
@group(2) @binding(5) var normal_sampler: sampler;
@group(2) @binding(6) var emissive_map: texture_2d<f32>;
@group(2) @binding(7) var emissive_sampler: sampler;

// Images are uploaded as UNORM. Color maps alone use the sRGB transfer function.
fn linear(color: vec3<f32>) -> vec3<f32> {
    return select(pow((color + vec3(0.055)) / 1.055, vec3(2.4)), color / 12.92, color <= vec3(0.04045));
}
fn srgb(color: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(color, vec3(1.0 / 2.4)) - vec3(0.055), color * 12.92, color <= vec3(0.0031308));
}

// Direct GGX lighting plus a simple hemisphere fill; this example does not supply an IBL map.
fn light(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, base: vec3<f32>, metallic: f32, roughness: f32) -> vec3<f32> {
    let h = normalize(v + l);
    let nl = max(dot(n, l), 0.0);
    let nv = max(dot(n, v), 0.001);
    let nh = max(dot(n, h), 0.0);
    let vh = max(dot(v, h), 0.0);
    let alpha = roughness * roughness;
    let alpha2 = alpha * alpha;
    let denom = nh * nh * (alpha2 - 1.0) + 1.0;
    let distribution = alpha2 / max(3.14159265 * denom * denom, 0.00001);
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    let geometry = nv / (nv * (1.0 - k) + k) * nl / (nl * (1.0 - k) + k);
    let f0 = mix(vec3(0.04), base, vec3(metallic));
    let fresnel = f0 + (vec3(1.0) - f0) * pow(1.0 - vh, 5.0);
    let specular = distribution * geometry * fresnel / max(4.0 * nv * nl, 0.001);
    let diffuse = (vec3(1.0) - fresnel) * (1.0 - metallic) * base / 3.14159265;
    return (diffuse + specular) * nl;
}

@fragment fn fs(surface: Surface) -> @location(0) vec4<f32> {
    let base = linear(textureSample(base_map, base_sampler, surface.uv).rgb) * material.base.rgb;
    let mr = textureSample(mr_map, mr_sampler, surface.uv);
    let metallic = clamp(mr.b * material.parameters.x, 0.0, 1.0);
    let roughness = clamp(mr.g * material.parameters.y, 0.045, 1.0);
    let sample_normal = textureSample(normal_map, normal_sampler, surface.uv).xyz * 2.0 - vec3(1.0);
    var n = normalize(surface.normal);
    // Derive the tangent frame from UV0; missing/degenerate UVs preserve the vertex normal.
    let px = dpdx(surface.position);
    let py = dpdy(surface.position);
    let ux = dpdx(surface.uv);
    let uy = dpdy(surface.uv);
    let determinant = ux.x * uy.y - ux.y * uy.x;
    if abs(determinant) > 0.0000001 {
        let t = normalize((px * uy.y - py * ux.y) / determinant);
        let b = normalize((py * ux.x - px * uy.x) / determinant);
        n = normalize(t * sample_normal.x * material.parameters.z + b * sample_normal.y * material.parameters.z + n * sample_normal.z);
    }
    let v = normalize(material.eye.xyz - surface.position);
    let key = light(n, v, normalize(vec3(0.5, 0.8, 1.0)), base, metallic, roughness) * vec3(3.8, 3.6, 3.3);
    let fill = light(n, v, normalize(vec3(-0.8, 0.3, -0.5)), base, metallic, roughness) * vec3(1.4, 1.7, 2.2);
    let ambient = base * mix(vec3(0.12, 0.13, 0.16), vec3(0.48, 0.52, 0.60), vec3(n.y * 0.5 + 0.5));
    let emissive = linear(textureSample(emissive_map, emissive_sampler, surface.uv).rgb) * material.emissive.rgb;
    let color = key + fill + ambient + emissive;
    return vec4(srgb(color / (vec3(1.0) + color)), 1.0);
}
