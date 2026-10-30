//! Compiler-boundary regressions exercise real WGSL validation and both artifact emitters.
use super::*;

/// Two unrelated graphics stages share source while consuming only their own resources.
const TEXTURED: &str = r#"
struct Camera { transform: mat4x4<f32> }
struct Tint { value: vec4<f32> }
@group(1) @binding(0) var<uniform> camera: Camera;
@group(3) @binding(0) var<uniform> tint: Tint;
@group(2) @binding(0) var image: texture_2d<f32>;
@group(2) @binding(1) var image_sampler: sampler;
struct Out { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vertex_main(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>) -> Out {
    var result: Out;
    result.position = camera.transform * vec4<f32>(position, 1.0);
    result.uv = uv;
    return result;
}
@fragment fn fragment_main(input: Out) -> @location(0) vec4<f32> {
    return textureSample(image, image_sampler, input.uv) * tint.value;
}
"#;

/// A procedural triangle has no client vertex layout or uniform blocks.
const PROCEDURAL: &str = r#"
@vertex fn vertex_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32(index % 2u) * 2.0 - 1.0;
    return vec4<f32>(x, f32(index / 2u), 0.25, 1.0);
}
@fragment fn fragment_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.1, 0.3, 0.7, 1.0);
}
"#;

/// Reflected stage inputs, generated block/sampler names and SPIR-V are actual compiler outputs.
#[test]
fn generic_textured_stages_emit_portable_artifacts_and_exact_layouts() {
    let vertex = Shader::compile(GpuShaderStage::Vertex, TEXTURED, "vertex_main").unwrap();
    assert!(vertex.glsl.starts_with("#version 300 es"));
    assert_eq!(vertex.spirv[0], 0x0723_0203);
    assert_eq!(
        vertex.inputs.iter().map(|input| (input.location, input.shape.components)).collect::<Vec<_>>(),
        [(0, 3), (1, 2)]
    );
    assert_eq!(vertex.uniforms.len(), 1);
    assert_eq!((vertex.uniforms[0].slot, vertex.uniforms[0].byte_len), (0, 64));
    assert!(vertex.glsl.contains(&vertex.uniforms[0].gl_block));
    assert!(vertex.textures.is_empty());
    let fragment = Shader::compile(GpuShaderStage::Fragment, TEXTURED, "fragment_main").unwrap();
    assert_eq!(fragment.uniforms[0].byte_len, 16);
    assert_eq!(fragment.textures.len(), 1);
    assert_eq!(fragment.textures[0].slot, 0);
    assert!(fragment.glsl.contains(&fragment.textures[0].gl_name));
    assert_eq!(vertex.outputs, fragment.inputs);
    assert!(fragment.byte_len() > TEXTURED.len());
}

/// No built-in semantic mesh, fixed vertex definition or transform is required to draw.
#[test]
fn procedural_graphics_need_no_vertex_or_uniform_bindings() {
    let vertex = Shader::compile(GpuShaderStage::Vertex, PROCEDURAL, "vertex_main").unwrap();
    let fragment = Shader::compile(GpuShaderStage::Fragment, PROCEDURAL, "fragment_main").unwrap();
    assert!(vertex.inputs.is_empty() && vertex.uniforms.is_empty());
    assert!(fragment.inputs.is_empty() && fragment.uniforms.is_empty());
}

/// Forbidden effect classes reject even when an unrelated selected entry never uses them.
#[test]
fn unused_storage_atomic_compute_and_storage_images_are_not_a_bypass() {
    for forbidden in [
        "@group(0) @binding(4) var<storage, read_write> escaped: array<u32>;",
        "var<workgroup> escaped: u32;",
        "@compute @workgroup_size(1) fn escaped() {}",
        "@group(0) @binding(4) var escaped: texture_storage_2d<rgba8unorm, write>;",
        "@group(0) @binding(4) var<storage, read_write> escaped: atomic<u32>;",
    ] {
        let source = format!("{PROCEDURAL}\n{forbidden}");
        assert!(Shader::compile(GpuShaderStage::Vertex, &source, "vertex_main").is_err(), "{forbidden}");
    }
}

/// Stage/slot authority and matching texture/sampler pairs are independent of parse validity.
#[test]
fn cross_stage_and_mismatched_sample_pairs_reject() {
    let cross_stage = TEXTURED
        .replace("@group(1)", "@group(3)")
        .replace("@binding(0) var<uniform> camera", "@binding(1) var<uniform> camera");
    assert!(Shader::compile(GpuShaderStage::Vertex, &cross_stage, "vertex_main").is_err());
    let mismatch = TEXTURED.replace("@binding(1) var image_sampler", "@binding(3) var image_sampler");
    assert!(Shader::compile(GpuShaderStage::Fragment, &mismatch, "fragment_main").is_err());
    let outside = TEXTURED.replace("@group(2)", "@group(4)");
    assert!(Shader::compile(GpuShaderStage::Fragment, &outside, "fragment_main").is_err());
}

/// WGSL and std140 matrix stride differences cannot silently corrupt uniforms on GL.
#[test]
fn incompatible_uniform_matrix_layout_rejects_before_native_artifacts() {
    let source = r#"
struct Uniforms { transform: mat2x2<f32> }
@group(1) @binding(0) var<uniform> data: Uniforms;
@vertex fn main() -> @builtin(position) vec4<f32> {
    return vec4<f32>(data.transform * vec2<f32>(1.0), 0.0, 1.0);
}
"#;
    let error = Shader::compile(GpuShaderStage::Vertex, source, "main").unwrap_err();
    assert!(error.contains("portable"));
}

/// A client cannot add a second render target through a custom fragment output.
#[test]
fn extra_color_outputs_reject() {
    let source = r#"
struct Outputs { @location(0) a: vec4<f32>, @location(1) b: vec4<f32> }
@fragment fn main() -> Outputs { return Outputs(vec4<f32>(1.0), vec4<f32>(0.0)); }
"#;
    assert!(Shader::compile(GpuShaderStage::Fragment, source, "main").unwrap_err().contains("exactly one"));
    // Selecting an innocent vertex entry must not admit a module containing a
    // second fragment attachment behind another unused entry point.
    let vertex = "@vertex fn vertex_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }";
    assert!(Shader::compile(GpuShaderStage::Vertex, &format!("{source}\n{vertex}"), "vertex_main").is_err());
}

/// Header bounds, UTF-8 parsing and exact entry selection fail before backend resource work.
#[test]
fn invalid_sources_and_entry_points_reject() {
    assert!(Shader::compile(GpuShaderStage::from_raw(255), PROCEDURAL, "vertex_main").is_err());
    assert!(Shader::compile(GpuShaderStage::Vertex, "", "main").is_err());
    assert!(Shader::compile(GpuShaderStage::Vertex, "not valid WGSL", "main").is_err());
    assert!(Shader::compile(GpuShaderStage::Vertex, PROCEDURAL, "missing").is_err());
    assert!(Shader::compile(GpuShaderStage::Vertex, &" ".repeat(MAX_SHADER_BYTES + 1), "main").is_err());
    assert!(Shader::compile(GpuShaderStage::Vertex, PROCEDURAL, &"a".repeat(129)).is_err());
}
