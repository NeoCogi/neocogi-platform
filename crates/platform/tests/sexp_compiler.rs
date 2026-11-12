//! Exercise typed frontend, independently owned reports, and actual native validators.
use neocogi_platform::render::compiler::{Compiler, CompileError};
#[cfg(feature = "wgpu")]
use neocogi_platform_contracts::render::shader::ShaderTarget;
use neocogi_platform_contracts::render::{abi::GpuShaderStage, shader::MAX_SHADER_ARTIFACT_BYTES};

const TRIANGLE: &str = r#"(module
  (vertex vs ((i u32 (builtin vertex_index))) (returns vec4 (builtin position))
    (let points ((array vec2 3) (vec2 -0.8 -0.8) (vec2 0.8 -0.8) (vec2 0.0 0.8)))
    (return (vec4 (index points i) 0.5 1.0)))
  (fragment fs () (returns vec4 (location 0)) (return (vec4 0.2 0.7 1.0 1.0))))"#;
const TEXTURED: &str = r#"(module
  (struct Camera (transform mat4))
  (struct Tint (color vec4))
  (struct Surface (clip vec4 (builtin position)) (uv vec2 (location 0)))
  (uniform camera Camera vertex 0)
  (uniform tint Tint fragment 0)
  (texture image fragment 0)
  (fn color ((uv vec2)) (returns vec4)
    (return (+ (sample-level image uv 0.0) (* (load image (ivec2 0)) 0.00001))))
  (vertex vs ((p vec3 (location 0)) (uv vec2 (location 1))) (returns Surface)
    (return (Surface (* (. camera transform) (vec4 p 1.0)) uv)))
  (fragment fs ((surface Surface)) (returns vec4 (location 0))
    (return (* (color (. surface uv)) (. tint color)))))"#;

#[path = "support/compilation.rs"]
mod compilation;
use compilation::{compile, targets, validate};

#[test]
fn native_artifacts_and_reflection() {
    for target in targets() {
        for source in [TRIANGLE, TEXTURED] {
            for (stage, entry) in [(GpuShaderStage::Vertex, "vs"), (GpuShaderStage::Fragment, "fs")] {
                let shader = compile(target, stage, source, entry);
                validate(&shader);
                if source == TEXTURED {
                    assert_eq!(shader.uniforms.len(), 1);
                    assert_eq!(shader.uniforms[0].byte_len, if stage == GpuShaderStage::Vertex { 64 } else { 16 });
                    assert_eq!(shader.textures.len(), usize::from(stage == GpuShaderStage::Fragment));
                }
            }
        }
    }
}
#[test]
fn selected_payload_has_an_exact_quota() {
    for target in targets() {
        let compiler = Compiler::new(target).unwrap();
        let full = compile(target, GpuShaderStage::Vertex, TRIANGLE, "vs");
        let size = full.byte_len();
        assert!(compiler.compile(GpuShaderStage::Vertex, TRIANGLE, "vs", size).is_ok());
        assert!(matches!(
            compiler.compile(GpuShaderStage::Vertex, TRIANGLE, "vs", size - 1),
            Err(CompileError::Limit)
        ));
    }
}
#[test]
fn structured_control_flow_and_private_writes() {
    let source = r#"(module
  (struct Values (items (array vec4 2)) (indices uvec4))
  (uniform data Values fragment 0)
  (fragment fs () (returns vec4 (location 0))
    (var result vec4)
    (var local (array vec4 2) ((array vec4 2) (vec4 0.25) (vec4 0.75)))
    (set (index local (. (. data indices) x)) (vec4 1.0))
    (var i u32 0u)
    (while
      (< i 2u)
      (if (== i 1u) (then (set i (+ i 1u)) (continue)))
      (set result (+ result (index (. data items) i)))
      (set i (+ i 1u)))
    (if (> (. result x) 0.0) (then (return (+ result (index local 0)))) (else (return (vec4 0.0))))))"#;
    for target in targets() {
        let shader = compile(target, GpuShaderStage::Fragment, source, "fs");
        validate(&shader);
        assert_eq!(shader.uniforms[0].byte_len, 48);
    }
}
#[test]
fn diagnostics_have_source_locations() {
    let Some(target) = targets().first().copied() else {
        return;
    };
    let error = Compiler::new(target)
        .unwrap()
        .compile(
            GpuShaderStage::Vertex,
            "(module\n  (vertex vs () (returns vec4 (builtin position))\n    (return missing)))",
            "vs",
            MAX_SHADER_ARTIFACT_BYTES,
        )
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("3:13:"), "{error}");
    assert!(error.contains("unknown value 'missing'"));
    assert!(error.contains('^'));
}
#[test]
fn invalid_programs_reject_for_every_target() {
    let cases = [
        "(module",
        "(module))",
        "(module (compute c () (returns void) (return)))",
        "(module (struct A (x A)))",
        "(module (struct A (x f32) (x f32)))",
        "(module (uniform u vec4 vertex 0))",
        "(module (struct A (v mat2)) (uniform u A vertex 0))",
        "(module (struct A (v (array f32 2))) (uniform u A vertex 0))",
        "(module (struct A (v (array vec4 1025))) (uniform u A vertex 0))",
        "(module (fn f () (returns f32) (return (f))))",
        "(module (vertex vs () (returns vec4 (builtin position)) (let x 0.0) (set x 1.0) (return (vec4 x))))",
        "(module (vertex vs () (returns vec4 (builtin position)) (return (vec4 (dpdy 1.0)))))",
        "(module (fragment fs () (returns vec4 (location 1)) (return (vec4 1.0))))",
        "(module (vertex vs ((x vec4 (location 16))) (returns vec4 (builtin position)) (return x)))",
        "(module (fn f () (returns f32) (if true (then (return 1.0)))))",
        "(module (fn f () (returns f32) (return (+ 1.0 1u))))",
        "(module (texture t fragment 0) (fragment fs () (returns vec4 (location 0)) (return (load t (ivec2 0)))))",
        "(module (texture t vertex 0) (fragment fs () (returns vec4 (location 0)) (return (sample t (vec2 0.5)))))",
        "(module (fn f () (returns void) (break)))",
        "(module (fn f ((p f32 (location 0))) (returns f32) (return p)))",
        "(module (struct Inner (v vec4 (location 0))) (struct Outer (n Inner)) (vertex vs ((p Outer)) (returns vec4 (builtin position)) (return (. (. p n) v))))",
        "(module (struct Bad (v vec4 (location 0) (interpolate flat center))))",
        "(module (struct Bad (v uvec4 (location 0) (interpolate perspective))))",
        "(module (struct Bad (v vec4 (interpolate flat))))",
        "(module (struct Bad (v vec4 (location 0) (interpolate flat) (interpolate flat))))",
    ];
    for target in targets() {
        let c = Compiler::new(target).unwrap();
        for source in cases {
            // A valid unrelated selection prevents missing-entry rejection from masking a validator bug.
            let source = if source.ends_with(')') && source.matches('(').count() == source.matches(')').count() {
                let body = source.strip_suffix(')').unwrap();
                format!("{body} (vertex selected () (returns vec4 (builtin position)) (return (vec4 1.0))))")
            } else {
                source.to_owned()
            };
            let error = c.compile(GpuShaderStage::Vertex, &source, "selected", MAX_SHADER_ARTIFACT_BYTES).unwrap_err();
            assert!(matches!(error, CompileError::Invalid(_)), "{source}: {error}");
            assert!(!error.to_string().contains("requested graphics entry"), "{source}: {error}");
        }
    }
}

/// Covers scalar/vector overloads, conversions, eager selection, and native matrix products.
fn operator_sources() -> Vec<String> {
    let values = [
        "(vec4 (vec3 0.25) 1.0)",
        "(vec4 (vec3 (ivec3 1)) 1.0)",
        "(vec4 (vec3 (uvec3 1u)) 1.0)",
        "(select (vec4 0.0) (vec4 1.0) (< (vec4 0.0) (vec4 1.0)))",
        "(vec4 (f32 (select 0 1 true)))",
        "(vec4 (f32 (select 0u 1u false)))",
        "(select (vec4 0.0) (vec4 1.0) (select false true true))",
        "(select (vec4 0.0) (vec4 1.0) (! (bvec4 false)))",
        "(vec4 (f32 (. (select (ivec2 0) (ivec2 1) (bvec2 true)) x)))",
        "(vec4 (f32 (>> -8 1u)))",
        "(vec4 (f32 (& (^ 8u 2u) 7u)))",
        "(vec4 (f32 (<< 1u 3u)))",
        "(/ (+ (* (vec4 0.25) 2.0) 1.0) 2.0)",
        "(- 1.0 (vec4 0.25))",
        "(vec4 (round 0.5) (fract 0.25) (trunc 0.5) (abs -0.5))",
        "(vec4 (dot (vec3 1.0) (vec3 0.25)))",
        "(vec4 (cross (vec3 1.0 0.0 0.0) (vec3 0.0 1.0 0.0)) 1.0)",
        "(vec4 (normalize (vec3 0.0 0.0 1.0)) 1.0)",
        "(vec4 (reflect (vec3 1.0) (vec3 0.0 1.0 0.0)) 1.0)",
        "(vec4 (length (vec3 0.5)) (distance (vec3 0.0) (vec3 0.5)) (atan2 0.5 1.0) (inverseSqrt 4.0))",
        "(vec4 (sin 0.25) (cos 0.25) (tan 0.25) (asin 0.25))",
        "(vec4 (acos 0.25) (atan 0.25) (exp 0.25) (exp2 0.25))",
        "(vec4 (log 0.25) (log2 0.25) (pow 0.25 2.0) (sqrt 0.25))",
        "(mix (vec4 0.0) (vec4 1.0) (vec4 0.5))",
        "(vec4 (f32 (min 1u 2u)))",
        "(vec4 (f32 (clamp -1 0 1)))",
        "(vec4 (select 0.0 1.0 (&& (all (bvec2 true)) (any (bvec2 true false)))))",
        "(vec4 (* (mat3 (vec3 1.0 0.0 0.0) (vec3 0.0 1.0 0.0) (vec3 0.0 0.0 1.0)) (vec3 0.5)) 1.0)",
        "(vec4 (index (/ (* (mat3 (vec3 1.0) (vec3 1.0) (vec3 1.0)) 2.0) 2.0) 0) 1.0)",
    ];
    values
        .into_iter()
        .map(|value| format!("(module (fragment fs () (returns vec4 (location 0)) (return {value})))"))
        .collect()
}

#[test]
fn operators_and_intrinsics_emit_valid_native_code() {
    for source in operator_sources() {
        for target in targets() {
            validate(&compile(target, GpuShaderStage::Fragment, &source, "fs"));
        }
    }
}

/// Validates actual generated WGSL through WGPU's native checked module path.
#[test]
#[cfg(feature = "wgpu")]
#[ignore = "requires a native WGPU adapter, including a software adapter"]
fn generated_wgsl_passes_native_module_validation() {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, _) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let mut programs = operator_sources().into_iter().map(|s| (s, GpuShaderStage::Fragment, "fs")).collect::<Vec<_>>();
    for source in [TRIANGLE, TEXTURED] {
        programs.push((source.into(), GpuShaderStage::Vertex, "vs"));
        programs.push((source.into(), GpuShaderStage::Fragment, "fs"));
    }
    for (source, stage, entry) in programs {
        let shader = compile(ShaderTarget::Wgsl, stage, &source, entry);
        let text = shader.code.wgsl().unwrap();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("S-expression output"),
            source: wgpu::ShaderSource::Wgsl(text.into()),
        });
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "invalid WGSL for {source}:\n{text}\n{error:?}");
    }
}

/// Malformed and adversarial source must reject without panicking or escaping parser bounds.
#[test]
fn bounded_reader_rejects_adversarial_forms() {
    let Some(target) = targets().first().copied() else {
        return;
    };
    let compiler = Compiler::new(target).unwrap();
    let mut seed = 0x9e3779b9u32;
    let alphabet = b"() abcdef0123456789;\n@\"'";
    for length in 0..512 {
        let source = (0..length)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                alphabet[seed as usize % alphabet.len()] as char
            })
            .collect::<String>();
        assert!(compiler.compile(GpuShaderStage::Vertex, &source, "vs", MAX_SHADER_ARTIFACT_BYTES).is_err());
    }
    for source in [
        format!("(module {})", "(".repeat(100)),
        format!("(module {})", "x ".repeat(100_001)),
        "(module (struct Huge (items (array (array vec4 4096) 4096))))".into(),
        "(module (texture t fragment 0) (fn helper () (returns void) (return t)))".into(),
        "(module (fragment fs () (returns vec4 (location 0)) (return (vec4 (bvec4 true)))))".into(),
    ] {
        assert!(compiler.compile(GpuShaderStage::Vertex, &source, "vs", MAX_SHADER_ARTIFACT_BYTES).is_err());
    }
}

/// Conservative control-flow admission catches invalid derivatives before native shader creation.
#[test]
fn conditional_derivatives_and_transitive_implicit_samples_reject() {
    for body in [
        "(if true (then (return (vec4 (dpdy 1.0))))) (return (vec4 0.0))",
        "(if true (then (return (vec4 0.0)))) (return (vec4 (dpdy 1.0)))",
        "(var value vec4) (while true (set value (vec4 (dpdx 1.0))) (break)) (return value)",
    ] {
        for target in targets() {
            let source = format!("(module (fragment fs () (returns vec4 (location 0)) {body}))");
            let error = Compiler::new(target)
                .unwrap()
                .compile(GpuShaderStage::Fragment, &source, "fs", MAX_SHADER_ARTIFACT_BYTES)
                .unwrap_err();
            assert!(error.to_string().contains("unconditional control flow"), "{error}");
        }
    }
    for target in targets() {
        let source = r#"(module
  (texture t fragment 0)
  (fn helper () (returns vec4) (return (sample t (vec2 0.5))))
  (fragment fs () (returns vec4 (location 0)) (if true (then (return (helper)))) (return (vec4 0.0))))"#;
        assert!(
            Compiler::new(target)
                .unwrap()
                .compile(GpuShaderStage::Fragment, source, "fs", MAX_SHADER_ARTIFACT_BYTES)
                .unwrap_err()
                .to_string()
                .contains("unconditional control flow")
        );
    }
}
