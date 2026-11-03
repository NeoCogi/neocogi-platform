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
    // Base, ORM, normal and emissive rectangles; explicit pixel coordinates and extents.
    tiles: array<vec4<f32>, 4>,
}
@group(3) @binding(0) var<uniform> material: Material;
@group(2) @binding(0) var material_map: texture_2d<f32>;
@group(2) @binding(1) var material_sampler: sampler;

// Nearest samples at exact pixel centers allow repeat interpolation within a tile, without
// bleeding into a neighboring material map. Decode sRGB before interpolating color texels.
fn material_texel(tile: vec4<f32>, coordinate: vec2<f32>, color: bool) -> vec3<f32> {
    let pixel = coordinate - floor(coordinate / tile.zw) * tile.zw;
    let value = textureSampleLevel(material_map, material_sampler,
        (tile.xy + pixel + vec2(0.5)) / vec2<f32>(textureDimensions(material_map)), 0.0).rgb;
    if color { return linear(value); }
    return value;
}
fn material_sample(index: u32, uv: vec2<f32>, color: bool) -> vec3<f32> {
    let tile = material.tiles[index];
    let pixel = fract(uv) * tile.zw - vec2(0.5);
    let first = floor(pixel);
    let weight = pixel - first;
    let a = material_texel(tile, first, color);
    let b = material_texel(tile, first + vec2(1.0, 0.0), color);
    let c = material_texel(tile, first + vec2(0.0, 1.0), color);
    let d = material_texel(tile, first + vec2(1.0, 1.0), color);
    return mix(mix(a, b, vec3(weight.x)), mix(c, d, vec3(weight.x)), vec3(weight.y));
}

@fragment fn fs(surface: Surface) -> @location(0) vec4<f32> {
    let base = material_sample(0u, surface.uv, true) * material.base.rgb;
    let mr = material_sample(1u, surface.uv, false);
    let metallic = clamp(mr.b * material.parameters.x, 0.0, 1.0);
    let roughness = clamp(mr.g * material.parameters.y, 0.045, 1.0);
    let sample_normal = material_sample(2u, surface.uv, false) * 2.0 - vec3(1.0);
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
    let nv = max(dot(n, v), 0.001);
    let response = brdf_response(nv, roughness);
    let f0 = mix(vec3(0.04), base, vec3(metallic));
    let fresnel = f0 + (max(vec3(1.0 - roughness), f0) - f0) * pow(1.0 - nv, 5.0);
    let single = fresnel * response.x + vec3(response.y);
    // Compensate energy lost to multiple GGX scattering, including rough metallic surfaces.
    let missing = max(1.0 - response.x - response.y, 0.0);
    let average = f0 + (vec3(1.0) - f0) / 21.0;
    let multiple = missing * single * average / max(vec3(1.0) - average * missing, vec3(0.001));
    let reflected = single + multiple;
    let diffuse = diffuse_radiance(n) * base * (1.0 - metallic) * max(vec3(1.0) - reflected, vec3(0.0));
    let specular = specular_radiance(reflect(-v, n), roughness) * reflected;
    let emissive = material_sample(3u, surface.uv, true) * material.emissive.rgb;
    let color = (diffuse + specular) * mr.r * environment.settings.y + emissive;
    return vec4(display_color(color), 1.0);
}
