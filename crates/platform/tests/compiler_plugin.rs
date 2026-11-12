//! Real COM calls through the built-in factory and a loaded platform plugin.
use neocogi_com::{ComPtr, NcCopyFromBufferOffset, INcStringComPtrExt, INcBytesComPtrExt, NC_E_INVALID_ARG, NC_E_OUT_OF_RANGE};
use neocogi_platform_contracts::render::{
    abi::*,
    shader::{ShaderTarget, PreparedShader},
};

#[test]
fn static_com_factory_and_owned_reports() {
    // SAFETY: This executable owns the immutable process-lived host services.
    let _runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let factory: ComPtr<ShaderCompilerFactoryAbi> = neocogi_platform::render::compiler::Factory::from_value(neocogi_platform::render::compiler::Factory)
        .query()
        .unwrap();
    let source = b"(module (vertex vs () (returns vec4 (builtin position)) (return (vec4 0.0 0.0 0.5 1.0))))";
    for (target, enabled) in [
        (ShaderTarget::Wgsl, cfg!(feature = "shader-wgsl")),
        (ShaderTarget::GlslEs300, cfg!(feature = "shader-glsl")),
        (ShaderTarget::SpirV, cfg!(feature = "shader-spirv")),
    ] {
        if !enabled {
            assert!(matches!(
                factory.create(target),
                Err(neocogi_platform_contracts::render::shader::SHADER_E_TARGET_UNAVAILABLE)
            ));
            continue;
        }
        let compiler = factory.create(target).unwrap();
        let request = ShaderCompileRequest {
            stage: GpuShaderStage::Vertex,
            max_output_bytes: 4194304,
        };
        let bad = compiler
            .compile_result(request, NcCopyFromBufferOffset::new(0, b"(module"), NcCopyFromBufferOffset::new(0, b"vs"))
            .unwrap();
        assert_eq!(bad.status().unwrap(), NC_E_INVALID_ARG.code());
        assert_eq!(bad.code().unwrap_err(), NC_E_INVALID_ARG);
        let diagnostic = bad.diagnostic().unwrap();
        drop(bad);
        let result = compiler
            .compile(request, NcCopyFromBufferOffset::new(0, source), NcCopyFromBufferOffset::new(0, b"vs"))
            .unwrap();
        let prepared = PreparedShader::new(result.clone(), GpuShaderStage::Vertex, target, b"vs", 4194304).unwrap();
        assert_eq!(prepared.copy().unwrap().entry, "vs");
        let code = result.code().unwrap();
        let inputs = result.inputs().unwrap();
        assert_eq!(inputs.get(0).unwrap_err(), NC_E_OUT_OF_RANGE);
        drop((result, prepared, compiler));
        assert!(!code.to_vec().unwrap().is_empty());
        assert!(diagnostic.to_string().unwrap().contains("unclosed parenthesis"));
    }
}

/// Loads one actual platform library and obtains its built-in compiler through QueryInterface.
/// This also exercises executable literals with the min-size profile's Debug formatting disabled.
#[test]
#[ignore = "set NEOCOGI_PLATFORM_DYLIB to the selected platform library"]
fn dynamic_platform_plugin_exposes_its_builtin_compiler() {
    const CHILD: &str = "NEOCOGI_BUILTIN_COMPILER_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "dynamic_platform_plugin_exposes_its_builtin_compiler", "--nocapture"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let path = std::env::var_os("NEOCOGI_PLATFORM_DYLIB").expect("NEOCOGI_PLATFORM_DYLIB must name the final platform shim");
    let bytes = std::fs::metadata(&path).unwrap().len();
    if let Some(limit) = std::env::var_os("NEOCOGI_PLATFORM_SIZE_LIMIT") {
        let limit = limit.to_str().unwrap().parse::<u64>().unwrap();
        assert!(bytes < limit, "platform dylib is {bytes} bytes, expected less than {limit}");
    }
    // SAFETY: This executable owns the immutable host service graph for process lifetime.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    // SAFETY: Qualification explicitly selects the trusted Cargo-built shim; the host pins it.
    let id = unsafe { runtime.load_plugin_library(std::path::PathBuf::from(path)) };
    let backend = std::env::var("NEOCOGI_TEST_BACKEND").unwrap_or_else(|_| "vulkan".into());
    let target = match backend.as_str() {
        "glow" => ShaderTarget::GlslEs300,
        "vulkan" => ShaderTarget::SpirV,
        "wgpu" => ShaderTarget::Wgsl,
        _ => panic!("unknown test backend"),
    };
    assert_eq!(id, neocogi_com::guid_from_qualified_name(&format!("neocogi.platform.{backend}")));
    let platform: ComPtr<neocogi_platform_contracts::render::platform::PlatformAbi> = runtime.acquire_plugin_root(id).unwrap();
    let factory = platform.query::<ShaderCompilerFactoryAbi>().unwrap();
    drop(platform);
    {
        let compiler = factory.create(target).unwrap();
        drop(factory);
        let result = compiler
            .compile(
                ShaderCompileRequest {
                    stage: GpuShaderStage::Vertex,
                    max_output_bytes: 4194304,
                },
                NcCopyFromBufferOffset::new(
                    0,
                    b"(module (vertex vs () (returns vec4 (builtin position)) (return (vec4 0.25 0.5 0.75 1.0))))",
                ),
                NcCopyFromBufferOffset::new(0, b"vs"),
            )
            .unwrap();
        let shader = PreparedShader::new(result, GpuShaderStage::Vertex, target, b"vs", 4194304)
            .unwrap()
            .copy()
            .unwrap();
        match shader.code {
            neocogi_platform_contracts::render::shader::ShaderCode::Wgsl(s) | neocogi_platform_contracts::render::shader::ShaderCode::GlslEs300(s) => {
                assert!(s.contains("0.25"))
            }
            neocogi_platform_contracts::render::shader::ShaderCode::SpirV(words) => assert!(words.contains(&0.25f32.to_bits())),
        }
        let error = compiler
            .compile(
                ShaderCompileRequest {
                    stage: GpuShaderStage::Vertex,
                    max_output_bytes: 4194304,
                },
                NcCopyFromBufferOffset::new(0, b"(module (vertex vs () (returns vec4 (builtin position)) (return unknown)))"),
                NcCopyFromBufferOffset::new(0, b"vs"),
            )
            .unwrap_err();
        assert_eq!(error.status, NC_E_INVALID_ARG);
        assert!(error.message.contains("unknown value 'unknown'"));
    }
}
