//! Python-like syntax shares validation, optimization, reflection and code with S-expressions.
use neocogi_platform::render::compiler::{Compiler, CompileError};
use neocogi_platform_contracts::render::{
    abi::GpuShaderStage,
    shader::{ShaderCode, MAX_SHADER_ARTIFACT_BYTES},
};
#[path = "support/compilation.rs"]
mod compilation;
use compilation::{compile, targets, validate};

/// Full portable graphics source, with an inferred fixed-size array and explicit IO annotations.
const TRIANGLE: &str = include_str!("../examples/triangle.pysh");

/// Identical typed program in the existing frontend; no frontend should alter backend work.
const SEXP: &str = r#"(module
  (vertex vs ((i u32 (builtin vertex_index))) (returns vec4 (builtin position))
    (let positions ((array vec2 3) (vec2 -0.8 -0.8) (vec2 0.8 -0.8) (vec2 0.0 0.8)))
    (return (vec4 (index positions i) 0.5 1.0)))
  (fragment fs () (returns vec4 (location 0)) (return (vec4 0.2 0.7 1.0 1.0))))"#;

/// Equality includes the actual exported code, rather than just successful parsing.
#[test]
fn both_frontends_emit_identical_triangle_artifacts() {
    for target in targets() {
        for (stage, entry) in [(GpuShaderStage::Vertex, "vs"), (GpuShaderStage::Fragment, "fs")] {
            let python = compile(target, stage, TRIANGLE, entry);
            let sexp = compile(target, stage, SEXP, entry);
            match (&python.code, &sexp.code) {
                (ShaderCode::Wgsl(a), ShaderCode::Wgsl(b)) | (ShaderCode::GlslEs300(a), ShaderCode::GlslEs300(b)) => assert_eq!(a, b),
                (ShaderCode::SpirV(a), ShaderCode::SpirV(b)) => assert_eq!(a, b),
                _ => panic!("frontend changed output target"),
            }
            assert_eq!(python.inputs, sexp.inputs);
            assert_eq!(python.outputs, sexp.outputs);
            validate(&python);
        }
    }
}

/// Grammar wrappers do not conceal the position of a missing value or invalid type.
fn rejection(source: &str, line: usize, column: usize, message: &str) {
    for target in targets() {
        let error = Compiler::new(target)
            .unwrap()
            .compile(GpuShaderStage::Fragment, source, "fs", MAX_SHADER_ARTIFACT_BYTES)
            .unwrap_err();
        let CompileError::Invalid(error) = error else {
            panic!("wrong rejection: {error}")
        };
        assert!(error.starts_with(&format!("{line}:{column}:")), "{error}");
        assert!(error.contains(message), "{error}");
        assert!(error.contains('^'), "{error}");
    }
}

/// Original line/column information survives indentation, expressions and shared type validation.
#[test]
fn errors_identify_the_original_python_source_location() {
    rejection(
        "@fragment\ndef fs() -> location[vec4, 0]:\n    return missing\n",
        3,
        12,
        "unknown value 'missing'",
    );
    rejection("@fragment\ndef fs() -> location[vec4, 0]\n    return vec4(1.0)\n", 2, 30, "expected ':'");
    rejection("@fragment\ndef fs() -> location[vec4, 0]:\n\treturn vec4(1.0)\n", 3, 1, "tabs");
    rejection(
        "@fragment\ndef fs() -> location[vec4, 0]:\n    x = vec4(1.0]\n    return x\n",
        3,
        17,
        "mismatched",
    );
    rejection("@fragment\ndef fs() -> location[vec4, 0]:\n    x = vec4(1.0)\n  return x\n", 4, 3, "dedent");
    rejection("@fragment\ndef fs() -> location[vec4, 0]:\n    return vec4(1e)\n", 3, 19, "exponent");
}

/// Lexical scopes, mutable parameters, unsigned endpoints, matrix multiplication and power compose.
#[test]
fn typed_bindings_helpers_control_and_operators_validate() {
    let source = r#"class Data:
    palette: array[mat4, 4]
    color: vec4

data: uniform[Data, fragment, 0]

def pick(values: array[mat4, 4], i: i32) -> vec4:
    i += 1
    j: i32 = 0
    result = vec4(0.0)
    while j < 4:
        j += 1
        if j == 1:
            continue
        elif j == 3:
            break
        else:
            result += values[i] @ vec4(1.0)
    return result

@fragment
def fs(p: builtin[vec4, position]) -> location[vec4, 0]:
    flag = not (p.x < 0.0) and (True or False)
    lower = -2147483648
    upper = u32(4294967295)
    power = -2.0 ** 2.0 + 2.0 ** 3.0 ** 1.0
    result = pick(data.palette, i32(p.x))
    return select(data.color, result + vec4(power), flag and upper > u32(0) and lower < 0)
"#;
    for target in targets() {
        validate(&compile(target, GpuShaderStage::Fragment, source, "fs"));
    }
}

/// Parse-time admission and complete-module checks apply before choosing an unrelated valid entry.
#[test]
fn unsupported_or_invalid_python_never_silently_falls_back() {
    let cases = [
        "import os\n",
        "class Empty:\n    pass\n",
        "def f(x):\n    return x\n",
        "@compute\ndef f():\n    pass\n",
        "x = 1.0\n",
        "class Bad:\n    value: array[f32, 2]\nu: uniform[Bad, vertex, 0]\n",
        "def f() -> f32:\n    return f()\n",
        "def f() -> f32:\n    if True:\n        return 1.0\n",
        "def f() -> bool:\n    return True == True == False\n",
        "def f() -> f32:\n    return 2.0 @ 3.0\n",
        "def f() -> f32:\n    return [1.0, True][0]\n",
        "def f():\n    break\n",
        "def f() -> u32:\n    return u32(4294967296)\n",
        "def f() -> i32:\n    return -2147483649\n",
        "def f() -> i32:\n    if True:\n        x = 1\n    return x\n",
        "def f():\n    pass\n    return 1.0\n",
        "def f() -> f32:\n    return 1.0\n    return 2.0\n",
        "def f() -> f32:\n    return \"native shader\"\n",
        "def f() -> f32:\n    return [][0]\n",
        "def f() -> f32:\n    return sqrt(value=1.0)\n",
        "def f() -> f32:\n    elif True:\n        return 1.0\n    return 0.0\n",
        "def f() -> f32:\n    true = 1.0\n    return true\n",
        "def f() -> bool:\n    return True == not False\n",
        "def f():\n    True = 1\n",
        "def f():\n    False: bool = True\n",
        "def f():\n    x = 1\n    x = 2\n    +x = 3\n",
        "def f() -> bool:\n    return +True\n",
    ];
    for target in targets() {
        for bad in cases {
            let source = format!("{bad}\n@fragment\ndef fs() -> location[vec4, 0]:\n    return vec4(1.0)\n");
            let error = Compiler::new(target)
                .unwrap()
                .compile(GpuShaderStage::Fragment, &source, "fs", MAX_SHADER_ARTIFACT_BYTES)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("requested graphics entry"), "{source}\n{error}");
            assert!(error.contains('^'), "{source}\n{error}");
        }
    }
}

/// Flat expressions and unary/control chains cannot evade the syntax-tree height limit.
#[test]
fn complexity_limits_cover_written_and_generated_python_trees() {
    for expression in [
        "not ".repeat(200) + "True",
        "1.0 + ".repeat(200) + "1.0",
        "(".repeat(200) + "1.0" + &")".repeat(200),
        "2.0 ** ".repeat(200) + "1.0",
    ] {
        let source = format!("@fragment\ndef fs() -> location[vec4, 0]:\n    return vec4({expression})\n");
        for target in targets() {
            let error = Compiler::new(target)
                .unwrap()
                .compile(GpuShaderStage::Fragment, &source, "fs", MAX_SHADER_ARTIFACT_BYTES)
                .unwrap_err()
                .to_string();
            assert!(error.contains("complexity limit"), "{error}");
        }
    }
}

/// Physical line endings, trailing whitespace and comments do not change the logical program.
#[test]
fn crlf_and_missing_final_newline_preserve_the_program() {
    let source = TRIANGLE.trim_end().replace('\n', "\r\n");
    for target in targets() {
        let a = compile(target, GpuShaderStage::Fragment, TRIANGLE, "fs");
        let b = compile(target, GpuShaderStage::Fragment, &source, "fs");
        match (&a.code, &b.code) {
            (ShaderCode::Wgsl(a), ShaderCode::Wgsl(b)) | (ShaderCode::GlslEs300(a), ShaderCode::GlslEs300(b)) => assert_eq!(a, b),
            (ShaderCode::SpirV(a), ShaderCode::SpirV(b)) => assert_eq!(a, b),
            _ => unreachable!(),
        }
    }
}

/// Augmented assignment must not duplicate an expensive index helper call.
#[test]
fn augmented_assignment_calls_its_index_helper_once() {
    let source = r#"def choose(i: i32) -> i32:
    return i - 1

@fragment
def fs(p: builtin[vec4, position]) -> location[vec4, 0]:
    values = [vec4(0.0), vec4(0.25), vec4(0.5), vec4(1.0)]
    values[choose(i32(p.x))].x += 0.25
    return values[0]
"#;
    for target in targets() {
        let shader = compile(target, GpuShaderStage::Fragment, source, "fs");
        validate(&shader);
        match shader.code {
            ShaderCode::Wgsl(text) | ShaderCode::GlslEs300(text) => assert!(text.matches("nc_f0(").count() <= 2, "index helper duplicated:\n{text}"),
            ShaderCode::SpirV(words) => {
                let mut calls = 0;
                let mut remaining = &words[5..];
                while !remaining.is_empty() {
                    let count = (remaining[0] >> 16) as usize;
                    assert!(count > 0 && count <= remaining.len());
                    if remaining[0] & 0xffff == 57 {
                        calls += 1;
                    } // OpFunctionCall, including the entry wrapper.
                    remaining = &remaining[count..];
                }
                assert!(calls <= 2, "index helper duplicated: {calls} calls");
            }
        }
    }
}

/// Eager derivative arguments must stay outside an expanded helper's conditional returns.
#[cfg(feature = "wgpu")]
#[test]
#[ignore = "requires a native WGPU adapter"]
fn python_helper_arguments_pass_native_wgsl_uniformity_validation() {
    let source = r#"class Data:
    values: array[vec4, 4]
data: uniform[Data, fragment, 0]

def pick(values: array[vec4, 4], i: i32, unused: f32) -> vec4:
    if i < 0:
        return vec4(0.0)
    return values[i]

@fragment
def fs(p: builtin[vec4, position]) -> location[vec4, 0]:
    return pick(data.values, i32(p.x) - 1, dpdx(p.x))
"#;
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, _) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let shader = compile(
        neocogi_platform_contracts::render::shader::ShaderTarget::Wgsl,
        GpuShaderStage::Fragment,
        source,
        "fs",
    );
    let ShaderCode::Wgsl(code) = shader.code else { unreachable!() };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let _module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Python frontend eager helper derivative"),
        source: wgpu::ShaderSource::Wgsl(code.into()),
    });
    let error = pollster::block_on(scope.pop());
    assert!(error.is_none(), "{error:?}");
}
