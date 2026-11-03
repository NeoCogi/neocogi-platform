// The application supplies an inverse-rotation perspective ray basis, with no translation.
struct SkyCamera { rays: mat4x4<f32> }
@group(1) @binding(0) var<uniform> sky_camera: SkyCamera;
struct Sky {
    @builtin(position) clip: vec4<f32>,
    @location(0) ray: vec3<f32>,
}
@vertex fn vs(@builtin(vertex_index) index: u32) -> Sky {
    let corners = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    let point = corners[index];
    var output: Sky;
    output.clip = vec4(point, 1.0, 1.0);
    output.ray = (sky_camera.rays * vec4(point, 1.0, 0.0)).xyz;
    return output;
}
@fragment fn fs(sky: Sky) -> @location(0) vec4<f32> {
    // A moderately blurred environment matches the reference viewer's default background mode.
    let radiance = specular_radiance(normalize(sky.ray), 0.25) * environment.settings.y;
    return vec4(display_color(radiance), 1.0);
}
