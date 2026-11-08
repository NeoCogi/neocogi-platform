//! Compiler regressions share admission while selecting one concrete output per call.
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

/// Paired mip-zero loads complete both native emitters, not only profile admission.
#[test]
#[cfg(feature = "shader-glsl")]
fn paired_image_loads_emit_complete_guarded_artifacts() {
    for level in ["0", "0u", "LEVEL"] {
        let source = format!(
            "const LEVEL:i32=0; {}",
            TEXTURED.replace(
                "textureSample(image, image_sampler, input.uv) * tint.value",
                &format!("textureSample(image, image_sampler, input.uv) + textureLoad(image, vec2<i32>(0), {level})")
            )
        );
        let shader = compile_glsl(GpuShaderStage::Fragment, &source, "fragment_main").unwrap();
        assert!(shader.code.wgsl().is_none());
        assert!(shader.code.spirv().is_none());
        assert!(shader.code.glsl().unwrap().contains("texelFetch"));
        assert!(!shader.code.glsl().unwrap().contains("textureQueryLevels"));
    }
}

/// Count-query specialization must preserve scopes inside helper functions and nested control flow.
#[test]
#[cfg(feature = "shader-glsl")]
fn exact_reads_in_nested_helper_control_flow_emit_both_targets() {
    let source = r#"
        @group(2) @binding(0) var t:texture_2d<f32>;
        @group(2) @binding(1) var s:sampler;
        fn fetch(p:vec2<i32>)->vec4<f32>{
            var value=vec4(0.);
            for(var i=0;i<1;i++){
                switch i { default: { if p.x>=0 {value=textureLoad(t,p,0);} else {value=textureLoad(t,p,0);} } }
            }
            return value;
        }
        @fragment fn fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32>{
            return fetch(vec2<i32>(p.xy))+textureSample(t,s,vec2(.5))*0.001;
        }"#;
    let shader = compile_glsl(GpuShaderStage::Fragment, source, "fs").unwrap();
    assert!(shader.code.wgsl().is_none());
    assert!(shader.code.spirv().is_none());
    assert!(shader.code.glsl().unwrap().contains("texelFetch"));
}

/// Float syntax and stage-distinct native block names are executable data in every Cargo profile.
#[test]
#[cfg(feature = "shader-glsl")]
fn code_generation_keeps_literals_and_distinct_stage_symbols() {
    let vertex = compile_glsl(GpuShaderStage::Vertex, PROCEDURAL, "vertex_main").unwrap();
    let fragment = compile_glsl(GpuShaderStage::Fragment, PROCEDURAL, "fragment_main").unwrap();
    assert!(vertex.code.glsl().unwrap().contains("0.25"), "GLSL must retain the vertex depth literal");
    for literal in ["0.1", "0.3", "0.7", "1.0"] {
        assert!(fragment.code.glsl().unwrap().contains(literal), "GLSL lost float literal {literal}");
    }
    let vertex = compile_glsl(
        GpuShaderStage::Vertex,
        r#"
struct Values { value:vec4<f32> }
@group(1) @binding(0) var<uniform> values:Values;
@vertex fn main() -> @builtin(position) vec4<f32> { return values.value; }
"#,
        "main",
    )
    .unwrap();
    let fragment = compile_glsl(
        GpuShaderStage::Fragment,
        r#"
struct Values { value:vec4<f32> }
@group(3) @binding(0) var<uniform> values:Values;
@fragment fn main() -> @location(0) vec4<f32> { return values.value; }
"#,
        "main",
    )
    .unwrap();
    assert_ne!(
        vertex.uniforms[0].gl_block, fragment.uniforms[0].gl_block,
        "independent stage bindings cannot collapse onto one linked GLSL block name"
    );
}

/// Reflected stage inputs, generated block/sampler names and SPIR-V are actual compiler outputs.
#[test]
#[cfg(feature = "shader-glsl")]
fn generic_textured_stages_emit_portable_artifacts_and_exact_layouts() {
    let vertex = compile_glsl(GpuShaderStage::Vertex, TEXTURED, "vertex_main").unwrap();
    assert!(vertex.code.glsl().unwrap().starts_with("#version 300 es"));
    assert!(vertex.code.spirv().is_none());
    assert_eq!(
        vertex.inputs.iter().map(|input| (input.location, input.shape.components)).collect::<Vec<_>>(),
        [(0, 3), (1, 2)]
    );
    assert_eq!(vertex.uniforms.len(), 1);
    assert_eq!((vertex.uniforms[0].slot, vertex.uniforms[0].byte_len), (0, 64));
    assert!(vertex.code.glsl().unwrap().contains(vertex.uniforms[0].gl_block.as_deref().unwrap()));
    assert!(vertex.textures.is_empty());
    let fragment = compile_glsl(GpuShaderStage::Fragment, TEXTURED, "fragment_main").unwrap();
    assert_eq!(fragment.uniforms[0].byte_len, 16);
    assert_eq!(fragment.textures.len(), 1);
    assert_eq!(fragment.textures[0].slot, 0);
    assert!(fragment.code.glsl().unwrap().contains(fragment.textures[0].gl_name.as_deref().unwrap()));
    assert_eq!(vertex.outputs, fragment.inputs);
    assert!(fragment.byte_len() > TEXTURED.len());
}

/// No built-in semantic mesh, fixed vertex definition or transform is required to draw.
#[test]
fn procedural_graphics_need_no_vertex_or_uniform_bindings() {
    let vertex = Compiler::compile(GpuShaderStage::Vertex, PROCEDURAL, "vertex_main").unwrap();
    let fragment = Compiler::compile(GpuShaderStage::Fragment, PROCEDURAL, "fragment_main").unwrap();
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
        assert!(Compiler::compile(GpuShaderStage::Vertex, &source, "vertex_main").is_err(), "{forbidden}");
    }
}

/// Stage/slot authority and matching texture/sampler pairs are independent of parse validity.
#[test]
fn cross_stage_and_mismatched_sample_pairs_reject() {
    let cross_stage = TEXTURED
        .replace("@group(1)", "@group(3)")
        .replace("@binding(0) var<uniform> camera", "@binding(1) var<uniform> camera");
    assert!(Compiler::compile(GpuShaderStage::Vertex, &cross_stage, "vertex_main").is_err());
    let mismatch = TEXTURED.replace("@binding(1) var image_sampler", "@binding(3) var image_sampler");
    assert!(Compiler::compile(GpuShaderStage::Fragment, &mismatch, "fragment_main").is_err());
    let outside = TEXTURED.replace("@group(2)", "@group(4)");
    assert!(Compiler::compile(GpuShaderStage::Fragment, &outside, "fragment_main").is_err());
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
    let error = Compiler::compile(GpuShaderStage::Vertex, source, "main").unwrap_err();
    assert!(error.contains("portable"));
}

/// A client cannot add a second render target through a custom fragment output.
#[test]
fn extra_color_outputs_reject() {
    let source = r#"
struct Outputs { @location(0) a: vec4<f32>, @location(1) b: vec4<f32> }
@fragment fn main() -> Outputs { return Outputs(vec4<f32>(1.0), vec4<f32>(0.0)); }
"#;
    assert!(Compiler::compile(GpuShaderStage::Fragment, source, "main").unwrap_err().contains("exactly one"));
    // Selecting an innocent vertex entry must not admit a module containing a
    // second fragment attachment behind another unused entry point.
    let vertex = "@vertex fn vertex_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }";
    assert!(Compiler::compile(GpuShaderStage::Vertex, &format!("{source}\n{vertex}"), "vertex_main").is_err());
}

/// Header bounds, UTF-8 parsing and exact entry selection fail before backend resource work.
#[test]
fn invalid_sources_and_entry_points_reject() {
    assert!(Compiler::compile(GpuShaderStage::from_raw(255), PROCEDURAL, "vertex_main").is_err());
    assert!(Compiler::compile(GpuShaderStage::Vertex, "", "main").is_err());
    assert!(Compiler::compile(GpuShaderStage::Vertex, "not valid WGSL", "main").is_err());
    assert!(Compiler::compile(GpuShaderStage::Vertex, PROCEDURAL, "missing").is_err());
    assert!(Compiler::compile(GpuShaderStage::Vertex, &" ".repeat(MAX_SHADER_BYTES + 1), "main").is_err());
    assert!(Compiler::compile(GpuShaderStage::Vertex, PROCEDURAL, &"a".repeat(129)).is_err());
}

/// Actual emitted code guards dynamic private writes as well as read-only uniform indexing.
#[test]
#[cfg(feature = "shader-glsl")]
fn generated_graphics_artifacts_guard_dynamic_array_access() {
    let source = r#"
        struct Values { items:array<vec4<f32>,2>, indices:vec4<u32> }
        @group(3) @binding(0) var<uniform> data:Values;
        @fragment fn main() -> @location(0) vec4<f32> {
            var local=array<vec4<f32>,2>(vec4(0.25),vec4(0.75));
            local[data.indices.x]=vec4(1.0);
            return data.items[data.indices.y]+local[0];
        }
    "#;
    let shader = compile_glsl(GpuShaderStage::Fragment, source, "main").unwrap();
    assert_eq!(shader.uniforms[0].byte_len, 48);
    // The input contains no conditional control flow. This guard comes from actual
    // compiler lowering of the dynamic write, rather than merely checking option values.
    assert!(
        shader.code.glsl().unwrap().contains("if ("),
        "private array writes must have a generated bounds guard"
    );
    assert!(shader.code.spirv().is_none());
}

/// Exercises GL emission independently of the validation-only WGSL convenience.
#[cfg(feature = "shader-glsl")]
fn compile_glsl(stage: GpuShaderStage, source: &str, entry: &str) -> Result<Shader, String> {
    Compiler::compile_bounded(stage, ShaderTarget::GlslEs300, source, entry, MAX_SHADER_ARTIFACT_BYTES).map_err(|error| error.to_string())
}
