//! Real minimal compiler builds: no test-support feature may enable another output.
use neocogi_platform::render::abi::*;
use neocogi_com::{com_object, ComPtr, NcCopyFromBufferOffset, NcStatus, NcBytes, INcStringAbi, INcBytesAbi, INcBytesComPtrExt, NC_OK, NC_E_INVALID_ARG};
use neocogi_platform::{
    render::compiler::CompilerObject,
    render::abi::GpuShaderStage,
    render::shader::{self, PreparedShader, ShaderCode, ShaderTarget},
};
use neocogi_com::INcStringComPtrExt;

/// Portable layout with one selected uniform and one unused declaration.
const SOURCE: &str = r#"(module
  (struct U (value vec4))
  (uniform u U vertex 0)
  (uniform unused U vertex 1)
  (vertex vs ((p vec2 (location 0))) (returns vec4 (builtin position))
    (return (+ (vec4 p 0.0 1.0) (. u value)))))"#;

/// This test is built separately for each output feature, including no outputs.
fn supported(target: ShaderTarget) -> bool {
    [
        (ShaderTarget::Wgsl, cfg!(feature = "shader-wgsl")),
        (ShaderTarget::SpirV, cfg!(feature = "shader-spirv")),
        (ShaderTarget::GlslEs300, cfg!(feature = "shader-glsl")),
    ]
    .into_iter()
    .any(|(format, enabled)| target == format && enabled)
}

/// Invokes the real generated ABI on a compiler with immutable renderer configuration.
fn compile(target: ShaderTarget) -> ComPtr<IShaderCompilationAbi> {
    compile_source(target, SOURCE)
}

/// Compiles real source so wire encoding checks exercise the generated backend-code getter.
fn compile_source(target: ShaderTarget, source: &str) -> ComPtr<IShaderCompilationAbi> {
    let root: ComPtr<ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject::new(target).unwrap()).query().unwrap();
    root.compile_result(
        ShaderCompileRequest {
            stage: GpuShaderStage::Vertex,
            max_output_bytes: 4194304,
        },
        NcCopyFromBufferOffset::new(0, source.as_bytes()),
        NcCopyFromBufferOffset::new(0, b"vs"),
    )
    .unwrap()
}

/// No disabled output can succeed, including WGSL in SPIR-V-only and GLSL-only builds.
#[test]
fn compiled_formats_match_explicit_output_features() {
    for target in [ShaderTarget::Wgsl, ShaderTarget::SpirV, ShaderTarget::GlslEs300] {
        if !supported(target) {
            assert!(matches!(CompilerObject::new(target), Err(shader::SHADER_E_TARGET_UNAVAILABLE)));
            continue;
        }
        let report = compile(target);
        assert_eq!(report.status().unwrap(), NC_OK.code());
        let info = report.info().unwrap();
        assert_eq!(report.target().unwrap(), target);
        assert_eq!(info.target, target);
        let prepared = PreparedShader::new(report.clone(), GpuShaderStage::Vertex, target, b"vs", 4194304).unwrap();
        assert_eq!(prepared.byte_len() as u64, info.entry_point_byte_count + info.code_byte_count);
        let snapshot = prepared.copy().unwrap();
        assert_eq!(snapshot.uniforms.len(), 1, "unused bindings are not selected reflection");
        assert_eq!(snapshot.uniforms[0].slot, 0);
        assert_eq!(snapshot.uniforms[0].byte_len, 16);
        assert_eq!(snapshot.byte_len(), prepared.byte_len());
        assert_eq!(snapshot.code.target(), target);
        match snapshot.code {
            ShaderCode::Wgsl(text) => assert!(text.contains("@vertex")),
            ShaderCode::SpirV(words) => assert_eq!(words[0], 0x07230203),
            ShaderCode::GlslEs300(text) => assert!(text.contains(snapshot.uniforms[0].gl_block.as_deref().unwrap())),
        }
        assert_eq!(snapshot.uniforms[0].gl_block.is_some(), target == ShaderTarget::GlslEs300);
    }
}

/// Identical target-free requests on distinct instances retain each renderer's format after errors.
#[test]
fn repeated_requests_cannot_change_a_compilers_configured_output() {
    let compilers: Vec<_> = [ShaderTarget::Wgsl, ShaderTarget::SpirV, ShaderTarget::GlslEs300]
        .into_iter()
        .filter(|target| supported(*target))
        .map(|target| {
            let compiler: ComPtr<ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject::new(target).unwrap()).query().unwrap();
            (target, compiler)
        })
        .collect();
    for _ in 0..3 {
        for (target, compiler) in &compilers {
            for (source, cap, status) in [
                ("invalid WGSL", 4194304, NC_E_INVALID_ARG),
                (SOURCE, 1, neocogi_com::NC_E_OUT_OF_RANGE),
                (SOURCE, 4194304, NC_OK),
            ] {
                let report = compiler
                    .compile_result(
                        ShaderCompileRequest {
                            stage: GpuShaderStage::Vertex,
                            max_output_bytes: cap,
                        },
                        NcCopyFromBufferOffset::new(0, source.as_bytes()),
                        NcCopyFromBufferOffset::new(0, b"vs"),
                    )
                    .unwrap();
                assert_eq!(report.status().unwrap(), status.code());
                if status == NC_OK {
                    assert_eq!(report.target().unwrap(), *target);
                    assert_eq!(report.info().unwrap().target, *target);
                    let prepared = PreparedShader::new(report, GpuShaderStage::Vertex, *target, b"vs", cap as usize).unwrap();
                    assert_eq!(prepared.copy().unwrap().code.target(), *target);
                } else {
                    assert_eq!(report.target().unwrap_err(), status);
                }
            }
        }
    }
}

/// A real report with a controlled backend-code getter exercises import without any fallback.
#[com_object(interfaces(neocogi_platform::render::abi::IShaderCompilation))]
struct ReportProbe {
    /// Actual successful report retains one independently owned code object.
    inner: ComPtr<IShaderCompilationAbi>,
    /// Independently owned backend bytes or a deliberate getter failure.
    code: Result<ComPtr<INcBytesAbi>, NcStatus>,
}
impl IShaderCompilation for ReportProbe {
    fn status(&self) -> Result<i32, NcStatus> {
        self.inner.status()
    }
    fn diagnostic(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        self.inner.diagnostic()
    }
    fn info(&self) -> Result<ShaderInfo, NcStatus> {
        self.inner.info()
    }
    fn target(&self) -> Result<ShaderTarget, NcStatus> {
        self.inner.target()
    }
    fn entry_point(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        self.inner.entry_point()
    }
    fn code(&self) -> Result<ComPtr<INcBytesAbi>, NcStatus> {
        self.code.clone()
    }
    fn inputs(&self) -> Result<ComPtr<IShaderLocationsAbi>, NcStatus> {
        self.inner.inputs()
    }
    fn outputs(&self) -> Result<ComPtr<IShaderLocationsAbi>, NcStatus> {
        self.inner.outputs()
    }
    fn uniforms(&self) -> Result<ComPtr<IShaderUniformsAbi>, NcStatus> {
        self.inner.uniforms()
    }
    fn sampled_textures(&self) -> Result<ComPtr<IShaderSampledTexturesAbi>, NcStatus> {
        self.inner.sampled_textures()
    }
}

/// Both text import paths and aligned binary import reject missing selected code without fallback.
#[test]
fn backend_code_is_acquired_and_getter_failures_propagate() {
    for target in [ShaderTarget::Wgsl, ShaderTarget::SpirV, ShaderTarget::GlslEs300] {
        if !supported(target) {
            continue;
        }
        for missing in [false, true] {
            let inner = compile(target);
            let code = if missing { Err(NC_E_INVALID_ARG) } else { inner.code() };
            let report = ReportProbe::from_value(ReportProbe { inner, code }).query().unwrap();
            let prepared = PreparedShader::new(report, GpuShaderStage::Vertex, target, b"vs", 4194304);
            if missing {
                assert!(matches!(prepared, Err(NC_E_INVALID_ARG)));
            } else {
                assert_eq!(prepared.unwrap().copy().unwrap().code.target(), target);
            }
        }
    }
}

/// The backend authenticates the result tag before acquiring its shader payload.
#[test]
fn a_mismatched_target_rejects_before_the_code_getter_is_called() {
    for target in [ShaderTarget::Wgsl, ShaderTarget::SpirV, ShaderTarget::GlslEs300] {
        if !supported(target) {
            continue;
        }
        let report: ComPtr<IShaderCompilationAbi> = ReportProbe::from_value(ReportProbe {
            inner: compile(target),
            code: Err(neocogi_com::NC_E_LOAD_FAILED),
        })
        .query()
        .unwrap();
        let other = if target == ShaderTarget::Wgsl {
            ShaderTarget::SpirV
        } else {
            ShaderTarget::Wgsl
        };
        assert!(matches!(
            PreparedShader::new(report.clone(), GpuShaderStage::Vertex, other, b"vs", 4194304),
            Err(NC_E_INVALID_ARG)
        ));
        assert!(matches!(
            PreparedShader::new(report, GpuShaderStage::Vertex, target, b"vs", 4194304),
            Err(neocogi_com::NC_E_LOAD_FAILED)
        ));
    }
}

/// Byte storage does not imply arbitrary text: both text backends reject invalid UTF-8 before publication.
#[test]
fn text_shader_bytes_require_utf8_without_losing_non_ascii_source() {
    for target in [ShaderTarget::Wgsl, ShaderTarget::GlslEs300] {
        if !supported(target) {
            continue;
        }
        let source = format!("; shader π 🦀\n{SOURCE}");
        let inner = compile_source(target, &source);
        let bytes = inner.code().unwrap().to_vec().unwrap();
        assert!(std::str::from_utf8(&bytes).is_ok());
        let prepared = PreparedShader::new(inner.clone(), GpuShaderStage::Vertex, target, b"vs", 4194304).unwrap();
        let shader = prepared.copy().unwrap();
        if target == ShaderTarget::Wgsl {
            assert!(shader.code.wgsl().unwrap().contains("@vertex"));
        } else {
            assert!(shader.code.glsl().unwrap().starts_with("#version 300 es"));
        }
        let mut malformed = bytes;
        malformed[0] = 0xff;
        let report = ReportProbe::from_value(ReportProbe {
            inner,
            code: Ok(NcBytes::from_vec(malformed)),
        })
        .query()
        .unwrap();
        // Summary, length and reflection remain legitimate; only the wire encoding is invalid.
        let prepared = PreparedShader::new(report, GpuShaderStage::Vertex, target, b"vs", 4194304).unwrap();
        assert!(matches!(prepared.copy(), Err(NC_E_INVALID_ARG)));
    }
}

/// Python is independently optional: a real COM compiler admits it only after explicit opt-in.
#[test]
fn python_frontend_matches_its_explicit_cargo_feature() {
    let source = "@vertex\ndef vs() -> builtin[vec4, position]:\n    return vec4(0.0, 0.0, 0.5, 1.0)\n";
    for target in [ShaderTarget::Wgsl, ShaderTarget::SpirV, ShaderTarget::GlslEs300]
        .into_iter()
        .filter(|target| supported(*target))
    {
        let report = compile_source(target, source);
        if cfg!(feature = "shader-python") {
            assert_eq!(report.status().unwrap(), NC_OK.code());
            PreparedShader::new(report, GpuShaderStage::Vertex, target, b"vs", 4194304)
                .unwrap()
                .copy()
                .unwrap();
        } else {
            assert_eq!(report.status().unwrap(), NC_E_INVALID_ARG.code());
            assert_eq!(report.code().unwrap_err(), NC_E_INVALID_ARG);
            let text = report.diagnostic().unwrap().to_string().unwrap();
            assert!(
                text.starts_with("1:1:") && text.contains("disabled") && text.contains("shader-python") && text.contains('^'),
                "{text}"
            );
        }
        assert_eq!(compile(target).status().unwrap(), NC_OK.code(), "S-expressions remain available");
    }
}
