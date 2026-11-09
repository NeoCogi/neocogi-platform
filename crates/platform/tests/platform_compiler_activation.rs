//! Platform roots keep compilation internal and audio independent of native video services.
#![cfg(not(target_arch = "wasm32"))]
use neocogi_platform::render::platform::PlatformAbi;

/// Registering only the platform creates owned roots without adding a compiler plugin to the catalog.
#[test]
fn renderer_owns_compilation_without_a_separate_compiler_plugin() {
    const CHILD: &str = "NEOCOGI_OWNED_COMPILER_ACTIVATION";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "renderer_owns_compilation_without_a_separate_compiler_plugin", "--nocapture"])
            .env(CHILD, "1")
            .env("SDL_VIDEODRIVER", "deliberately-invalid")
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
    // SAFETY: This executable owns the immutable process-lived host and linked descriptor graph.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let descriptor = neocogi_platform::platform::plugin_descriptor();
    // SAFETY: All descriptor metadata and code remain statically mapped for process lifetime.
    unsafe { runtime.register_plugin_descriptor(descriptor) }.unwrap();
    let first = runtime.acquire_plugin_root::<PlatformAbi>(descriptor.plugin_id).unwrap();
    let second = runtime.acquire_plugin_root::<PlatformAbi>(descriptor.plugin_id).unwrap();
    drop((first, second));
    assert!(
        runtime
            .plugin_info(neocogi_com::guid_from_qualified_name("neocogi.shader.compiler"))
            .unwrap()
            .is_none()
    );
}

/// Platform's generated external audio signatures use the canonical namespace without initializing video.
#[test]
fn platform_audio_imports_preserve_typed_stream_authority() {
    use neocogi_com::NC_E_NOINTERFACE;
    use neocogi_platform_contracts::audio::*;
    const CHILD: &str = "NEOCOGI_PLATFORM_AUDIO_NAMESPACE";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "platform_audio_imports_preserve_typed_stream_authority", "--nocapture"])
            .env(CHILD, "1")
            .env("SDL_AUDIODRIVER", "dummy")
            .env("SDL_VIDEODRIVER", "deliberately-invalid")
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
    // SAFETY: This final executable owns the process-lived host tables and linked descriptor.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let descriptor = neocogi_platform::platform::plugin_descriptor();
    // SAFETY: Descriptor metadata and provider code stay statically mapped throughout the process.
    unsafe { runtime.register_plugin_descriptor(descriptor) }.unwrap();
    let platform = runtime.acquire_plugin_root::<PlatformAbi>(descriptor.plugin_id).unwrap();
    let capabilities = platform.audio_capabilities().unwrap();
    assert_eq!(capabilities.info().unwrap().backend, AudioBackend::Sdl);
    assert!(capabilities.supported_pcm_types().unwrap().contains(&PcmType::FixedPoint(16, 8)));
    let config = AudioConfig {
        format: PcmFormat {
            sample_type: PcmType::FixedPoint(16, 8),
            byte_order: PcmByteOrder::LittleEndian,
        },
        ..AudioConfig::stereo()
    };
    let mut request = platform.open_audio(AudioDirection::Playback, config).unwrap();
    assert_eq!(request.poll().unwrap(), AudioOpenState::Ready);
    assert_eq!(request.take_capture().err(), Some(NC_E_NOINTERFACE));
    assert_eq!(request.poll().unwrap(), AudioOpenState::Ready);
    let mut playback = request.take_playback().unwrap();
    assert_eq!(request.poll().unwrap(), AudioOpenState::Taken);
    drop((platform, request, capabilities));
    playback.write_f32(&[0.25, -0.25]).unwrap();
    let info = playback.info().unwrap();
    assert_eq!(info.config, config);
    assert!(matches!(info.queue, AudioQueueInfo::Playback { queued_frames: 1, .. }));
    playback.close().unwrap();
}
