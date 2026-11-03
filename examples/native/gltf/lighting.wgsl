// Shared by the mesh and skybox programs. Region counts come from environment.rs.
struct Environment {
    regions: array<vec4<f32>, @REGION_COUNT@>,
    // Exposure multiplier, IBL intensity, reserved, reserved.
    settings: vec4<f32>,
}
@group(3) @binding(1) var<uniform> environment: Environment;
@group(2) @binding(2) var environment_map: texture_2d<f32>;
@group(2) @binding(3) var environment_sampler: sampler;

// Color maps alone use the sRGB transfer. All radiance processing stays scene-linear.
fn linear(color: vec3<f32>) -> vec3<f32> {
    return select(pow((color + vec3(0.055)) / 1.055, vec3(2.4)), color / 12.92, color <= vec3(0.04045));
}
fn srgb(color: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(color, vec3(1.0 / 2.4)) - vec3(0.055), color * 12.92, color <= vec3(0.0031308));
}

// Khronos PBR Neutral curve, used by the reference viewer. Apply once after summing HDR light.
// Algorithm: https://github.com/KhronosGroup/ToneMapping/tree/main/PBR_Neutral
fn display_color(radiance: vec3<f32>) -> vec3<f32> {
    var color = max(radiance * environment.settings.x, vec3(0.0));
    let darkest = min(color.r, min(color.g, color.b));
    color -= select(0.04, darkest - 6.25 * darkest * darkest, darkest < 0.08);
    let peak = max(color.r, max(color.g, color.b));
    if peak > 0.76 {
        let compressed = 1.0 - 0.0576 / (peak - 0.52);
        color *= compressed / peak;
        let whiten = 1.0 - 1.0 / (1.0 + 0.15 * (peak - compressed));
        color = mix(color, vec3(compressed), vec3(whiten));
    }
    return srgb(clamp(color, vec3(0.0), vec3(1.0)));
}

// Nearest texel reads preserve the RGBE exponent. Decode each texel before interpolation.
fn environment_texel(region: vec4<f32>, coordinate: vec2<f32>, repeat_x: bool, hdr: bool) -> vec3<f32> {
    var pixel = clamp(coordinate, vec2(0.0), region.zw - vec2(1.0));
    if repeat_x { pixel.x = coordinate.x - floor(coordinate.x / region.z) * region.z; }
    let sample = textureSampleLevel(environment_map, environment_sampler,
        (region.xy + pixel + vec2(0.5)) / vec2<f32>(textureDimensions(environment_map)), 0.0);
    if !hdr { return sample.rgb; }
    if sample.a == 0.0 { return vec3(0.0); }
    return sample.rgb * 255.0 * exp2(round(sample.a * 255.0) - 136.0);
}
fn environment_tile(index: u32, uv: vec2<f32>, repeat_x: bool, hdr: bool) -> vec3<f32> {
    let region = environment.regions[index];
    let pixel = uv * region.zw - vec2(0.5);
    let first = floor(pixel);
    let weight = pixel - first;
    let a = environment_texel(region, first, repeat_x, hdr);
    let b = environment_texel(region, first + vec2(1.0, 0.0), repeat_x, hdr);
    let c = environment_texel(region, first + vec2(0.0, 1.0), repeat_x, hdr);
    let d = environment_texel(region, first + vec2(1.0, 1.0), repeat_x, hdr);
    return mix(mix(a, b, vec3(weight.x)), mix(c, d, vec3(weight.x)), vec3(weight.y));
}

// The reference environment faces +Z, a 90-degree rotation about the glTF +Y axis.
// HDR row zero is north; skybox, diffuse and specular all share this exact orientation.
fn environment_uv(direction: vec3<f32>) -> vec2<f32> {
    let rotated = vec3(direction.z, direction.y, -direction.x);
    return vec2(0.5 + atan2(rotated.z, rotated.x) / 6.28318530718,
        acos(clamp(rotated.y, -1.0, 1.0)) / 3.14159265359);
}
fn specular_radiance(direction: vec3<f32>, roughness: f32) -> vec3<f32> {
    let level = clamp(roughness, 0.0, 1.0) * f32(@LAST_SPECULAR@);
    let first = u32(floor(level));
    let second = min(first + 1u, @LAST_SPECULAR@u);
    let uv = environment_uv(direction);
    return mix(environment_tile(first, uv, true, true), environment_tile(second, uv, true, true), vec3(fract(level)));
}
fn diffuse_radiance(direction: vec3<f32>) -> vec3<f32> {
    return environment_tile(@DIFFUSE_REGION@u, environment_uv(direction), true, true);
}
fn brdf_response(nv: f32, roughness: f32) -> vec2<f32> {
    return environment_tile(@BRDF_REGION@u, clamp(vec2(nv, roughness), vec2(0.0), vec2(1.0)), false, false).rg;
}
