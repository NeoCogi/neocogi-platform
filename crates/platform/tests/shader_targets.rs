//! Real minimal compiler builds: no test-support feature may enable another output.
use neocogi_platform::render::abi::*;
use neocogi_com::{com_object, ComPtr, NcCopyFromBufferOffset, NcStatus, INcStringAbi, INcBytesAbi, NC_OK, NC_E_INVALID_ARG, NC_E_LOAD_FAILED};
use neocogi_platform::{
    render::compiler::CompilerObject,
    render::abi::GpuShaderStage,
    render::shader::{self, PreparedShader, ShaderCode, ShaderTarget},
};

/// Portable layout with one selected uniform and one unused declaration.
const SOURCE: &str = "struct U { value:vec4<f32> } @group(1) @binding(0) var<uniform> u:U; @group(1) @binding(1) var<uniform> unused:U; @vertex fn vs(@location(0) p:vec2<f32>)->@builtin(position) vec4<f32> {return vec4<f32>(p,0.,1.)+u.value;}";

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
    let root: ComPtr<ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject::new(target).unwrap()).query().unwrap();
    root.compile_result(
        ShaderCompileRequest {
            stage: GpuShaderStage::Vertex,
            max_output_bytes: 4194304,
        },
        NcCopyFromBufferOffset::new(0, SOURCE.as_bytes()),
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
            ShaderCode::Wgsl(text) => assert_eq!(text, SOURCE),
            ShaderCode::SpirV(words) => assert_eq!(words[0], 0x07230203),
            ShaderCode::GlslEs300(text) => assert!(text.contains(snapshot.uniforms[0].gl_block.as_deref().unwrap())),
        }
        assert_eq!(snapshot.uniforms[0].gl_block.is_some(), target == ShaderTarget::GlslEs300);
        for unused in [ShaderTarget::Wgsl, ShaderTarget::SpirV, ShaderTarget::GlslEs300] {
            if unused == target {
                continue;
            }
            let error = match unused {
                ShaderTarget::Wgsl => report.wgsl().map(|_| ()).unwrap_err(),
                ShaderTarget::SpirV => report.spirv().map(|_| ()).unwrap_err(),
                ShaderTarget::GlslEs300 => report.glsl().map(|_| ()).unwrap_err(),
                _ => unreachable!(),
            };
            assert_eq!(error, neocogi_com::NC_E_NOINTERFACE);
        }
    }
}

/// Every unused code getter is poisoned; import must query exactly the selected representation.
#[com_object(interfaces(neocogi_platform::render::abi::IShaderCompilation))]
struct ReportProbe {
    /// Actual successful report retains one independently owned code object.
    inner: ComPtr<IShaderCompilationAbi>,
    /// Selected getter fails without changing the successful status or summary.
    missing: bool,
}
impl ReportProbe {
    /// Unused getters fail differently, so accidental universal import cannot pass.
    fn selected(&self, target: ShaderTarget) -> Result<(), NcStatus> {
        if self.inner.target()? != target {
            return Err(NC_E_LOAD_FAILED);
        }
        if self.missing {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(())
    }
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
    fn wgsl(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        self.selected(ShaderTarget::Wgsl)?;
        self.inner.wgsl()
    }
    fn spirv(&self) -> Result<ComPtr<INcBytesAbi>, NcStatus> {
        self.selected(ShaderTarget::SpirV)?;
        self.inner.spirv()
    }
    fn glsl(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        self.selected(ShaderTarget::GlslEs300)?;
        self.inner.glsl()
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
fn unused_formats_are_never_acquired_and_selected_failures_propagate() {
    for target in [ShaderTarget::Wgsl, ShaderTarget::SpirV, ShaderTarget::GlslEs300] {
        if !supported(target) {
            continue;
        }
        for missing in [false, true] {
            let report = ReportProbe::from_value(ReportProbe { inner: compile(target), missing }).query().unwrap();
            let prepared = PreparedShader::new(report, GpuShaderStage::Vertex, target, b"vs", 4194304);
            if missing {
                assert!(matches!(prepared, Err(NC_E_INVALID_ARG)));
            } else {
                assert_eq!(prepared.unwrap().copy().unwrap().code.target(), target);
            }
        }
    }
}
