//! Test-only executable: standalone platform acquisition without constructing a GPU.
/// Loads the single selected platform library with no external compiler registration.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let launch = neocogi_platform_examples::LaunchArguments::parse(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(24);
    });
    let platform = neocogi_platform_examples::acquire_platform(launch.backend.as_deref()).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(24);
    });
    // The compiler is a COM capability of this same platform root and requires no second library.
    {
        use neocogi_example_plugins::sdk::render::abi::*;
        let factory = platform.query::<ShaderCompilerFactoryAbi>().unwrap();
        let mut supported = 0;
        for target in [ShaderTarget::Wgsl, ShaderTarget::GlslEs300, ShaderTarget::SpirV] {
            let compiler = match factory.create(target) {
                Ok(compiler) => compiler,
                Err(status) if status == neocogi_example_plugins::sdk::render::shader::SHADER_E_TARGET_UNAVAILABLE => continue,
                Err(status) => panic!("compiler creation failed: {status}"),
            };
            let report = compiler
                .compile(
                    ShaderCompileRequest {
                        stage: GpuShaderStage::Vertex,
                        max_output_bytes: 4194304,
                    },
                    neocogi_com::NcCopyFromBufferOffset::new(0, b"(module (vertex vs () (returns vec4 (builtin position)) (return (vec4 0.0 0.0 0.5 1.0))))"),
                    neocogi_com::NcCopyFromBufferOffset::new(0, b"vs"),
                )
                .unwrap();
            assert_eq!(report.target().unwrap(), target);
            supported += 1;
        }
        assert!(supported >= 1);
    }
    if launch.arguments == ["--audio"] {
        use neocogi_example_plugins::sdk::audio::{AudioConfig, AudioDirection, AudioOpenState};
        use neocogi_com::{NcCopyFromBufferOffset, NcOutputBuffer};
        let capabilities = platform.audio_capabilities().unwrap();
        let config = AudioConfig {
            capacity_frames: 128,
            ..AudioConfig::stereo()
        };
        assert!(capabilities.supports_config(config).unwrap());
        let mut request = platform.open_audio(AudioDirection::Playback, config).unwrap();
        assert_eq!(request.poll().unwrap(), AudioOpenState::Ready);
        let mut playback = request.take_playback().unwrap();
        drop(request);
        playback.write(NcCopyFromBufferOffset::new(0, &[0; 16])).unwrap();
        assert_eq!(playback.info().unwrap().queued_frames(), 2);
        playback.start().unwrap();
        playback.pause().unwrap();
        playback.close().unwrap();
        let mut request = platform.open_audio(AudioDirection::Capture, config).unwrap();
        let mut capture = request.take_capture().unwrap();
        drop(request);
        drop(platform);
        let mut output = NcOutputBuffer::<u8>::new();
        assert_eq!(capture.read(output.output(0, 16)).unwrap(), 0);
        capture.start().unwrap();
        capture.pause().unwrap();
        capture.close().unwrap();
        println!("Dynamic audio playback and capture acquired without a window or external plugin.");
    }
    // SAFETY: This executable owns the process-lived host, already initialized by acquisition.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let loaded: Vec<_> = neocogi_example_plugins::Backend::ALL
        .into_iter()
        .filter(|backend| {
            runtime
                .plugin_info(neocogi_com::guid_from_qualified_name(backend.plugin_name()))
                .unwrap()
                .is_some()
        })
        .collect();
    assert_eq!(loaded.len(), 1, "exactly one requested backend is activated");
    let name = loaded[0].plugin_name();
    assert!(
        runtime
            .plugin_info(neocogi_com::guid_from_qualified_name("neocogi.shader.compiler"))
            .unwrap()
            .is_none()
    );
    println!("Dynamic platform root acquired through its single shared library: {name}.");
}
/// This deployment fixture has no browser entry behavior.
#[cfg(target_arch = "wasm32")]
fn main() {}
