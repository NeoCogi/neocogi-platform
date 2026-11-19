//! Runs independently generated COM calls against packaged platform and renderer providers.
use neocogi_com_sdk::{ComInterface, ComPtr, ComBytesExt, ComStringExt, NcCopyFromBufferOffset, RuntimeHandle, NC_OK};
use neocogi_platform_contracts::audio as audio_contracts;

extern crate self as package_projection;

/// Consumer-owned generated definitions negotiate by COM identity.
#[allow(dead_code, clippy::len_without_is_empty)]
mod gpu_contracts {
    include!(env!("IDL_NEOCOGI_RENDERER"));
}

/// Consumer-owned platform projection imports local renderer and canonical audio types.
#[allow(dead_code, clippy::len_without_is_empty)]
mod platform_contracts {
    include!(env!("IDL_NEOCOGI_PLATFORM"));
}

/// A separately generated caller reads nested enums and typed arrays from a canonical producer.
fn typed_pipeline_contract() {
    use neocogi_platform::render::{abi, source::PipelineDesc};
    let source: ComPtr<gpu_contracts::IGpuPipelineSourceAbi> = PipelineDesc {
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
        gpu_contracts::GpuBlendState::Blend {
            color: gpu_contracts::GpuBlendEquation::Subtract {
                src: gpu_contracts::GpuBlendFactor::SrcAlpha,
                dst: gpu_contracts::GpuBlendFactor::OneMinusSrcAlpha
            },
            alpha: gpu_contracts::GpuBlendEquation::Min,
        }
    );
    let mut bindings = neocogi_com_sdk::NcOutputBuffer::<gpu_contracts::GpuShaderBinding>::new();
    source.bindings(bindings.output(0, 2)).unwrap();
    assert_eq!(
        bindings.as_slice(),
        [
            gpu_contracts::GpuShaderBinding {
                stage: gpu_contracts::GpuShaderStage::Fragment,
                slot: 0,
                kind: gpu_contracts::GpuBindingKind::Uniform(16)
            },
            gpu_contracts::GpuShaderBinding {
                stage: gpu_contracts::GpuShaderStage::Fragment,
                slot: 0,
                kind: gpu_contracts::GpuBindingKind::SampledTexture
            },
        ]
    );
}

/// Inspects, loads and calls the packaged compiler through independently generated COM bindings.
fn dynamic_platform_contract(runtime: RuntimeHandle, path: std::path::PathBuf) {
    let expected_id = neocogi_com_sdk::guid_from_qualified_name("neocogi.platform.glow");
    // SAFETY: Qualification supplies the trusted library built from the extracted shim archive.
    let inspection = unsafe { neocogi_com_host::inspect_plugin_library(&path) }.unwrap();
    let expected_root = platform_contracts::PlatformAbi::INTERFACE_ID;
    assert_eq!(inspection.description_abi_version, 3);
    assert_eq!(inspection.core_abi_version, 21);
    assert_eq!(inspection.plugin_id, expected_id);
    assert_eq!(inspection.factory.root_factory_id, expected_root.iid);
    // SAFETY: The generated interface identity borrows an immutable static digest.
    assert_eq!(
        inspection.factory.root_factory_signature,
        *unsafe { expected_root.abi_signature.as_digest() }.unwrap()
    );
    assert!(runtime.plugin_info(expected_id).unwrap().is_none());
    // SAFETY: The qualification command supplies the trusted library built from the extracted shim archive.
    // The linked host validates its descriptor and pins the provider code for process lifetime.
    let id = unsafe { runtime.load_plugin_library(path) };
    assert_eq!(id, expected_id);
    // A fresh handle must see the registration performed through the original handle.
    let platform = neocogi_com_host::initialize_runtime()
        .acquire_plugin_root::<platform_contracts::PlatformAbi>(id)
        .unwrap();
    let audio: ComPtr<audio_contracts::IAudioCapabilitiesAbi> = platform.audio_capabilities().unwrap();
    assert_eq!(audio.info().unwrap().backend, audio_contracts::AudioBackend::Sdl);
    let factory = platform.query::<gpu_contracts::ShaderCompilerFactoryAbi>().unwrap();
    drop(platform);
    let compiler = factory.create(gpu_contracts::ShaderTarget::GlslEs300).unwrap();
    drop(factory);
    let request = gpu_contracts::ShaderCompileRequest {
        stage: gpu_contracts::GpuShaderStage::Vertex,
        max_output_bytes: 4194304,
    };
    let source = b"(module (vertex vs () (returns vec4 (builtin position)) (return (vec4 0.25 0.5 0.75 1.0))))";
    let report = compiler
        .compile_result(request, NcCopyFromBufferOffset::new(0, source), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    assert_eq!(report.status().unwrap(), NC_OK.code());
    assert_eq!(report.target().unwrap(), gpu_contracts::ShaderTarget::GlslEs300);
    let code = report.code().unwrap();
    drop(report);
    let error = compiler
        .compile_result(request, NcCopyFromBufferOffset::new(0, b"(module"), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    assert_eq!(error.status().unwrap(), neocogi_com_sdk::NC_E_INVALID_ARG.code());
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
    let path = std::env::args_os().nth(1).expect("pass the packaged platform library path");
    let runtime = neocogi_com_host::initialize_runtime();
    dynamic_platform_contract(runtime, path.into());
    typed_pipeline_contract();
    let audio = neocogi_platform::audio::capabilities().unwrap();
    assert_eq!(audio.info().unwrap().backend, audio_contracts::AudioBackend::Sdl);
    let mut supported = neocogi_com_sdk::NcOutputBuffer::<audio_contracts::PcmType>::new();
    audio.pcm_types(supported.output(0, audio.pcm_type_count().unwrap() as usize)).unwrap();
    drop(audio);
    assert!(supported.as_slice().contains(&audio_contracts::PcmType::FixedPoint(32, 16)));
    assert!(supported.as_slice().contains(&audio_contracts::PcmType::UnsignedInteger(8)));
    let factory: ComPtr<gpu_contracts::ShaderCompilerFactoryAbi> =
        neocogi_platform::render::compiler::Factory::from_value(neocogi_platform::render::compiler::Factory)
            .query()
            .unwrap();
    let root = factory.create(gpu_contracts::ShaderTarget::GlslEs300).unwrap();
    let source = b"(module (vertex vs () (returns vec4 (builtin position)) (return (vec4 0.0 0.0 0.0 1.0))))";
    let report = root
        .compile_result(
            gpu_contracts::ShaderCompileRequest {
                stage: gpu_contracts::GpuShaderStage::Vertex,
                max_output_bytes: 4 * 1024 * 1024,
            },
            NcCopyFromBufferOffset::new(0, source),
            NcCopyFromBufferOffset::new(0, b"vs"),
        )
        .unwrap();
    assert_eq!(report.status().unwrap(), NC_OK.code());
    assert_eq!(report.target().unwrap(), gpu_contracts::ShaderTarget::GlslEs300);
    assert_eq!(report.info().unwrap().target, gpu_contracts::ShaderTarget::GlslEs300);
    // A consumer generated from the merged renderer schema interoperates with
    // ordinary SDK readers by typed QI, including backend format validation.
    use neocogi_platform::render::{
        abi,
        shader::{PreparedShader, ShaderTarget},
    };
    let canonical = report.query::<abi::IShaderCompilationAbi>().unwrap();
    assert!(matches!(
        PreparedShader::new(canonical.clone(), abi::GpuShaderStage::Vertex, ShaderTarget::SpirV, b"vs", 4 * 1024 * 1024),
        Err(neocogi_com_sdk::NC_E_INVALID_ARG)
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
