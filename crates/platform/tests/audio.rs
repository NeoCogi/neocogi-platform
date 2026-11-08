//! Real SDL dummy devices and generated COM calls; no display, GPU or physical microphone is used.
#![cfg(not(target_arch = "wasm32"))]
use neocogi_com::{ComBox, INcStringComPtrExt, NcCopyFromBufferOffset, NcOutputBuffer, NC_E_INVALID_ARG, NC_E_NOINTERFACE};
use neocogi_platform::{
    audio::open,
    render::platform::{*, audio::*},
};
use std::time::{Duration, Instant};

/// Encode fixture PCM through the same declared representation an application would supply.
fn pcm(config: AudioConfig, frames: usize, value: f32) -> Vec<u8> {
    let bytes = config.format.sample_bytes().unwrap();
    let sample = config.format.encode(value).unwrap();
    sample[..bytes].repeat(frames * config.channels.count() as usize)
}

/// The output variant is the only place where starvation accounting exists.
fn silent(info: AudioInfo) -> u64 {
    let AudioQueueInfo::Playback { silent_frames, .. } = info.queue else {
        panic!("expected playback metadata")
    };
    silent_frames
}
/// Capture-overrun accounting cannot be accidentally read from an output snapshot.
fn dropped(info: AudioInfo) -> u64 {
    let AudioQueueInfo::Capture { dropped_frames, .. } = info.queue else {
        panic!("expected capture metadata")
    };
    dropped_frames
}

/// Native requests complete synchronously; a wrong direction must preserve their untaken stream.
fn playback(config: AudioConfig) -> ComBox<IAudioPlaybackAbi> {
    let mut request = open(AudioDirection::Playback, config).unwrap();
    assert_eq!(request.poll().unwrap(), AudioOpenState::Ready);
    assert_eq!(request.take_capture().err().unwrap(), NC_E_NOINTERFACE);
    assert_eq!(request.poll().unwrap(), AudioOpenState::Ready);
    let stream = request.take_playback().unwrap();
    assert_eq!(request.poll().unwrap(), AudioOpenState::Taken);
    assert_eq!(request.take_playback().err().unwrap(), AUDIO_E_STATE);
    request.cancel().unwrap();
    assert_eq!(request.poll().unwrap(), AudioOpenState::Taken);
    assert_eq!(stream.query_ref::<IAudioCaptureAbi>().err().unwrap(), NC_E_NOINTERFACE);
    stream
}

/// One isolated process owns SDL initialization and tests real producer/consumer callbacks through COM.
#[test]
fn native_playback_capture_and_owned_failures() {
    const CHILD: &str = "NEOCOGI_AUDIO_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_playback_capture_and_owned_failures", "--nocapture"])
            .env(CHILD, "1")
            .env("SDL_AUDIODRIVER", "dummy")
            .env("SDL_VIDEODRIVER", "neocogi-no-video-for-audio")
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
    // SAFETY: This isolated executable owns its immutable process-lived COM host tables.
    let _runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let config = AudioConfig {
        capacity_frames: 128,
        ..AudioConfig::stereo()
    };
    let capabilities = neocogi_platform::audio::capabilities().unwrap();
    let info = capabilities.info().unwrap();
    assert_eq!(info.backend, AudioBackend::Sdl);
    assert!(matches!(info.playback, AudioSupport::Available { access: AudioAccess::System, .. }));
    assert!(matches!(info.capture, AudioSupport::Available { access: AudioAccess::System, .. }));
    assert!(capabilities.supports_config(config).unwrap());
    let types = capabilities.supported_pcm_types().unwrap();
    assert_eq!(types.len(), 90);
    assert!(types.contains(&PcmType::FixedPoint(32, 16)));
    assert!(types.contains(&PcmType::UnsignedInteger(8)));
    assert_eq!(capabilities.supported_channels().unwrap(), [AudioChannels::Mono, AudioChannels::Stereo]);
    assert_eq!(
        capabilities.supported_byte_orders().unwrap(),
        [PcmByteOrder::LittleEndian, PcmByteOrder::BigEndian]
    );
    for format in [
        config.format,
        PcmFormat {
            sample_type: PcmType::UnsignedInteger(8),
            ..config.format
        },
        PcmFormat {
            sample_type: PcmType::SignedInteger(16),
            byte_order: PcmByteOrder::BigEndian,
        },
        PcmFormat {
            sample_type: PcmType::FixedPoint(32, 16),
            ..config.format
        },
    ] {
        let config = AudioConfig { format, ..config };
        let mut output = playback(config);
        assert_eq!(output.info().unwrap().state, AudioState::Paused);
        let bytes = pcm(config, 128, 0.25);
        output.write_f32(&[0.25; 256]).unwrap();
        assert_eq!(output.info().unwrap().queued_frames(), 128);
        assert_eq!(output.write(NcCopyFromBufferOffset::new(0, &pcm(config, 1, 0.))), Err(AUDIO_E_LIMIT));
        assert_eq!(output.info().unwrap().queued_frames(), 128);
        assert_eq!(output.write(NcCopyFromBufferOffset::new(1, &bytes)), Err(NC_E_INVALID_ARG));
        if matches!(format.sample_type, PcmType::FloatingPoint(_)) {
            let invalid: Vec<_> = [0f32, f32::NAN].iter().flat_map(|sample| sample.to_le_bytes()).collect();
            assert_eq!(output.write(NcCopyFromBufferOffset::new(0, &invalid)), Err(NC_E_INVALID_ARG));
            assert_eq!(output.info().unwrap().queued_frames(), 128);
        }
        output.start().unwrap();
        output.start().unwrap();
        assert_eq!(output.clear(), Err(AUDIO_E_STATE));
        let deadline = Instant::now() + Duration::from_secs(2);
        while silent(output.info().unwrap()) == 0 {
            assert!(Instant::now() < deadline, "dummy playback callback did not run");
            std::thread::sleep(Duration::from_millis(5));
        }
        output.pause().unwrap();
        output.pause().unwrap();
        output.clear().unwrap();
        assert_eq!(output.info().unwrap().queued_frames(), 0);
        output.close().unwrap();
        output.close().unwrap();
        assert_eq!(output.info().unwrap().state, AudioState::Closed);
        assert!(silent(output.info().unwrap()) > 0);
        assert_eq!(output.write(NcCopyFromBufferOffset::new(0, &bytes)), Err(AUDIO_E_STATE));
        assert_eq!(output.start(), Err(AUDIO_E_STATE));
    }
    let mut cancelled = open(AudioDirection::Playback, config).unwrap();
    cancelled.cancel().unwrap();
    assert_eq!(cancelled.poll().unwrap(), AudioOpenState::Cancelled);
    assert_eq!(cancelled.take_playback().err().unwrap(), AUDIO_E_STATE);
    let mut request = open(AudioDirection::Capture, config).unwrap();
    assert_eq!(request.take_playback().err().unwrap(), NC_E_NOINTERFACE);
    let mut capture = request.take_capture().unwrap();
    assert_eq!(capture.query_ref::<IAudioPlaybackAbi>().err().unwrap(), NC_E_NOINTERFACE);
    drop(request);
    let mut bytes = NcOutputBuffer::<u8>::new();
    assert_eq!(capture.read(bytes.output(0, 1024)).unwrap(), 0);
    capture.start().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while dropped(capture.info().unwrap()) == 0 {
        assert!(Instant::now() < deadline, "dummy capture callback did not run");
        std::thread::sleep(Duration::from_millis(5));
    }
    capture.pause().unwrap();
    let queued = capture.info().unwrap().queued_frames();
    assert_eq!(queued, 128);
    assert_eq!(capture.read(bytes.output(1, 16)), Err(NC_E_INVALID_ARG));
    assert_eq!(capture.read(bytes.output(0, 3)), Err(NC_E_INVALID_ARG));
    assert_eq!(capture.info().unwrap().queued_frames(), queued);
    let frames = capture.read(bytes.output(0, 512)).unwrap();
    assert_eq!(frames, 64);
    assert_eq!(bytes.as_slice().len(), frames as usize * 8);
    for sample in bytes.as_slice().as_chunks::<4>().0 {
        config.format.decode(sample).unwrap();
    }
    assert_eq!(capture.read_f32(64).unwrap().len(), 128);
    capture.clear().unwrap();
    assert_eq!(capture.info().unwrap().queued_frames(), 0);
    capture.close().unwrap();
    capture.close().unwrap();
    assert_eq!(capture.info().unwrap().state, AudioState::Closed);
    assert_eq!(capture.read(bytes.output(0, 8)), Err(AUDIO_E_STATE));
    let mut invalid = open(AudioDirection::Capture, AudioConfig { sample_rate: 0, ..config }).unwrap();
    assert_eq!(invalid.poll().unwrap(), AudioOpenState::Failed(NC_E_INVALID_ARG.code()));
    let text = invalid.message().unwrap();
    assert_eq!(invalid.take_capture().err().unwrap(), NC_E_INVALID_ARG);
    drop(invalid);
    assert!(text.to_string().unwrap().contains("invalid PCM"));
}

/// Unavailable native drivers report owned per-request text without needing video initialization.
#[test]
fn unavailable_audio_driver_has_owned_diagnostic() {
    const CHILD: &str = "NEOCOGI_AUDIO_FAILURE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "unavailable_audio_driver_has_owned_diagnostic", "--nocapture"])
            .env(CHILD, "1")
            .env("SDL_AUDIODRIVER", "neocogi-unavailable-audio-driver")
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
    // SAFETY: This isolated child owns the process-lived runtime tables.
    let _runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let mut failed = open(AudioDirection::Playback, AudioConfig::stereo()).unwrap();
    let capabilities = neocogi_platform::audio::capabilities().unwrap().info().unwrap();
    assert!(matches!(capabilities.playback, AudioSupport::Available { devices: AudioDeviceCount::Failed(status), .. } if status == AUDIO_E_BACKEND.code()));
    assert_eq!(failed.poll().unwrap(), AudioOpenState::Failed(AUDIO_E_BACKEND.code()));
    let text = failed.message().unwrap();
    drop(failed);
    assert!(text.to_string().unwrap().contains("neocogi-unavailable-audio-driver"));
}
