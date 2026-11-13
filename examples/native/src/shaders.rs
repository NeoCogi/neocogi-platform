//! One source inventory for the applications and their backend qualification.

/// Unfiltered RGBE source at roughness zero, followed by eight filtered GGX levels.
pub const SPECULAR_LEVELS: usize = 9;
/// Cosine-weighted radiance integral (irradiance / pi), ready to multiply by albedo.
pub const DIFFUSE_REGION: usize = SPECULAR_LEVELS;
/// Ordinary linear UNORM R/G store the scale/bias of the integrated GGX response.
pub const BRDF_REGION: usize = DIFFUSE_REGION + 1;
/// Each region occupies one vec4 in the lighting uniform block.
pub const REGION_COUNT: usize = BRDF_REGION + 1;
/// Explicit vec4 rectangles, then exposure, IBL intensity and two reserved scalars.
pub const UNIFORM_BYTES: u32 = (REGION_COUNT as u32 + 1) * 16;

/// Actual standalone Triangle program, including its dynamically selected vertex position.
pub const TRIANGLE: &str = include_str!("../../../crates/platform/examples/triangle.pysh");

/// Join shared lighting and one graphics program with the authoritative atlas layout.
pub fn lighting(body: &str) -> String {
    let lighting = include_str!("../gltf/lighting.sexp")
        .replace("@REGION_COUNT@", &REGION_COUNT.to_string())
        .replace("@LAST_SPECULAR@", &(SPECULAR_LEVELS - 1).to_string())
        .replace("@DIFFUSE_REGION@", &DIFFUSE_REGION.to_string())
        .replace("@BRDF_REGION@", &BRDF_REGION.to_string());
    format!("(module\n{lighting}\n{body}\n)")
}

/// Skinned PBR program shared by both actual glTF applications.
pub fn mesh() -> String {
    lighting(include_str!("../gltf/viewer.sexp"))
}
/// Full-screen HDR background program used by both actual glTF applications.
pub fn skybox() -> String {
    lighting(include_str!("../gltf/skybox.sexp"))
}
