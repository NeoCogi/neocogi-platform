//! Runs independently generated COM calls against packaged platform and renderer providers.
use neocogi_com::{ComPtr, INcBytesComPtrExt, INcStringComPtrExt, NcCopyFromBufferOffset, Runtime, NC_OK};

/// Consumer-owned generated definitions negotiate by COM identity.
#[allow(dead_code, clippy::len_without_is_empty)]
mod client {
    include!(env!("IDL_NEOCOGI_RENDERER"));
}

/// Consumer-owned platform projection imports the canonical independent audio namespace.
#[allow(dead_code, clippy::len_without_is_empty)]
mod platform_client {
    include!(env!("IDL_NEOCOGI_PLATFORM"));
}

/// Separately emitted audio caller negotiates the canonical audio COM identities and enum codecs.
#[allow(dead_code, clippy::len_without_is_empty)]
mod audio_client {
    include!(env!("IDL_NEOCOGI_AUDIO"));
}

/// A separately generated caller reads nested enums and typed arrays from a canonical producer.
fn typed_pipeline_contract() {
    use neocogi_platform::render::{abi, source::PipelineDesc};
    let source: ComPtr<client::IGpuPipelineSourceAbi> = PipelineDesc {
        state: abi::GpuPipelineInfo {
            blend: abi::GpuBlendState::Blend {
                color: abi::GpuBlendEquation::Subtract {
                    src: abi::GpuBlendFactor::SrcAlpha,
                    dst: abi::GpuBlendFactor::OneMinusSrcAlpha,
                },
                alpha: abi::GpuBlendEquation::Min,
            },
            ..Default::default()
        },
        bindings: vec![
            abi::GpuShaderBinding {
                stage: abi::GpuShaderStage::Fragment,
                slot: 0,
                kind: abi::GpuBindingKind::Uniform(16),
            },
            abi::GpuShaderBinding {
                stage: abi::GpuShaderStage::Fragment,
                slot: 0,
                kind: abi::GpuBindingKind::SampledTexture,
            },
        ],
        ..Default::default()
    }
    .source()
    .unwrap()
    .query()
    .unwrap();
    assert_eq!(
        source.info().unwrap().blend,
        client::GpuBlendState::Blend {
            color: client::GpuBlendEquation::Subtract {
                src: client::GpuBlendFactor::SrcAlpha,
                dst: client::GpuBlendFactor::OneMinusSrcAlpha
            },
            alpha: client::GpuBlendEquation::Min,
        }
    );
    let mut bindings = neocogi_com::NcOutputBuffer::<client::GpuShaderBinding>::new();
    source.bindings(bindings.output(0, 2)).unwrap();
    assert_eq!(
        bindings.as_slice(),
        [
            client::GpuShaderBinding {
                stage: client::GpuShaderStage::Fragment,
                slot: 0,
                kind: client::GpuBindingKind::Uniform(16)
            },
            client::GpuShaderBinding {
                stage: client::GpuShaderStage::Fragment,
                slot: 0,
                kind: client::GpuBindingKind::SampledTexture
            },
        ]
    );
}

/// A normal packaged cdylib exposes the built-in compiler through independently generated COM bindings.
/// No SDK descriptor registration or renderer linkage in the caller supplies the loaded provider.
fn dynamic_platform_contract(runtime: Runtime, path: std::path::PathBuf) {
    // SAFETY: The qualification command supplies the trusted library built from the extracted shim archive.
    // Runtime validates its descriptor and pins the provider code for process lifetime.
    let id = unsafe { runtime.load_plugin_library(path) };
    assert_eq!(id, neocogi_com::guid_from_qualified_name("neocogi.platform.glow"));
    let platform = runtime.acquire_plugin_root::<platform_client::PlatformAbi>(id).unwrap();
    let factory = platform.query::<client::ShaderCompilerFactoryAbi>().unwrap();
    drop(platform);
    let compiler = factory.create(client::ShaderTarget::GlslEs300).unwrap();
    drop(factory);
    let request = client::ShaderCompileRequest {
        stage: client::GpuShaderStage::Vertex,
        max_output_bytes: 4194304,
    };
    let source = b"(module (vertex vs () (returns vec4 (builtin position)) (return (vec4 0.25 0.5 0.75 1.0))))";
    let report = compiler
        .compile_result(request, NcCopyFromBufferOffset::new(0, source), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    assert_eq!(report.status().unwrap(), NC_OK.code());
    assert_eq!(report.target().unwrap(), client::ShaderTarget::GlslEs300);
    let code = report.code().unwrap();
    drop(report);
    let error = compiler
        .compile_result(request, NcCopyFromBufferOffset::new(0, b"(module"), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    assert_eq!(error.status().unwrap(), neocogi_com::NC_E_INVALID_ARG.code());
    let diagnostic = error.diagnostic().unwrap();
    drop((error, compiler));
    let emitted = code.to_vec().unwrap();
    let emitted = std::str::from_utf8(&emitted).unwrap();
    assert!(emitted.starts_with("#version 300 es"));
    assert!(emitted.contains("0.25"));
    assert!(diagnostic.to_string().unwrap().contains("unclosed parenthesis"));
    println!("PASS: normal packaged platform library and independent COM factory calls");
}

/// Retained artifact exports stay usable after their report and compiler are released.
fn main() {
    // SAFETY: This executable owns the process-lived immutable host tables.
    let runtime = unsafe { Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    if let Some(path) = std::env::args_os().nth(1) {
        dynamic_platform_contract(runtime, path.into());
    }
    typed_pipeline_contract();
    let audio = neocogi_platform::audio::capabilities()
        .unwrap()
        .query::<audio_client::IAudioCapabilitiesAbi>()
        .unwrap();
    assert_eq!(audio.info().unwrap().backend, audio_client::AudioBackend::Sdl);
    let mut supported = neocogi_com::NcOutputBuffer::<audio_client::PcmType>::new();
    audio.pcm_types(supported.output(0, audio.pcm_type_count().unwrap() as usize)).unwrap();
    drop(audio);
    assert!(supported.as_slice().contains(&audio_client::PcmType::FixedPoint(32, 16)));
    assert!(supported.as_slice().contains(&audio_client::PcmType::UnsignedInteger(8)));
    let factory: ComPtr<client::ShaderCompilerFactoryAbi> =
        neocogi_platform::render::compiler::Factory::from_value(neocogi_platform::render::compiler::Factory)
            .query()
            .unwrap();
    let root = factory.create(client::ShaderTarget::GlslEs300).unwrap();
    let source = b"(module (vertex vs () (returns vec4 (builtin position)) (return (vec4 0.0 0.0 0.0 1.0))))";
    let report = root
        .compile_result(
            client::ShaderCompileRequest {
                stage: client::GpuShaderStage::Vertex,
                max_output_bytes: 4 * 1024 * 1024,
            },
            NcCopyFromBufferOffset::new(0, source),
            NcCopyFromBufferOffset::new(0, b"vs"),
        )
        .unwrap();
    assert_eq!(report.status().unwrap(), NC_OK.code());
    assert_eq!(report.target().unwrap(), client::ShaderTarget::GlslEs300);
    assert_eq!(report.info().unwrap().target, client::ShaderTarget::GlslEs300);
    // A consumer generated from the merged renderer schema interoperates with
    // ordinary SDK readers by typed QI, including backend format validation.
    use neocogi_platform::render::{
        abi,
        shader::{PreparedShader, ShaderTarget},
    };
    let canonical = report.query::<abi::IShaderCompilationAbi>().unwrap();
    assert!(matches!(
        PreparedShader::new(canonical.clone(), abi::GpuShaderStage::Vertex, ShaderTarget::SpirV, b"vs", 4 * 1024 * 1024),
        Err(neocogi_com::NC_E_INVALID_ARG)
    ));
    let prepared = PreparedShader::new(canonical, abi::GpuShaderStage::Vertex, ShaderTarget::GlslEs300, b"vs", 4 * 1024 * 1024).unwrap();
    assert_eq!(prepared.copy().unwrap().code.target(), ShaderTarget::GlslEs300);
    drop(prepared);
    let code = report.code().unwrap();
    let emitted = code.to_vec().unwrap();
    assert!(std::str::from_utf8(&emitted).unwrap().contains("#version 300 es"));
    drop(report);
    drop(root);
    drop(factory);
    assert_eq!(code.to_vec().unwrap(), emitted);
    assert_eq!(
        std::fs::read(std::path::Path::new(env!("NEOCOGI_NATIVE_OUTPUT_DIR")).join("asset.txt")).unwrap(),
        include_bytes!("../asset.txt")
    );
    println!("PASS: packaged sources, independent COM caller and retained shader exports");
}
