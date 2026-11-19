//! Actual browser audio acceptance. The repository runner supplies fake microphone/permission devices.
use super::*;
use crate::render::platform::PlatformAbi;
use js_sys::{Function, Promise};
use neocogi_com_sdk::{ComStringExt, NcOutputBuffer};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(inline_js = "
let mediaResolve;
export function denyMedia() {
  const old = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
  navigator.mediaDevices.getUserMedia = () => Promise.reject(new DOMException('synthetic audio permission denied', 'NotAllowedError'));
  return old;
}
export function restoreMedia(old) { navigator.mediaDevices.getUserMedia = old; }
export function gateMedia() {
  const old = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
  const pending = new Promise(resolve => { mediaResolve = resolve; });
  navigator.mediaDevices.getUserMedia = () => pending;
  return old;
}
export async function releaseMedia(old) {
  navigator.mediaDevices.getUserMedia = old;
  const stream = await old({audio:true,video:false});
  const resolve = mediaResolve; mediaResolve = undefined; resolve(stream);
  return stream;
}
export function probeWorklet(mode) {
  const descriptor = Object.getOwnPropertyDescriptor(AudioContext.prototype, 'audioWorklet');
  const add = Worklet.prototype.addModule;
  const create = URL.createObjectURL;
  const revoke = URL.revokeObjectURL;
  const Node = globalThis.AudioWorkletNode;
  let url, created = 0, revoked = 0, nodes = 0, finishModule;
  globalThis.AudioWorkletNode = new Proxy(Node, {
    construct(target, argumentsList, newTarget) {
      nodes++;
      return Reflect.construct(target, argumentsList, newTarget);
    }
  });
  URL.createObjectURL = function (blob) {
    url = create.call(this, blob);
    created++;
    return url;
  };
  URL.revokeObjectURL = function (address) {
    if (url === address) revoked++;
    return revoke.call(this, address);
  };
  const failure = () => new Error('injected worklet ' + mode);
  if (mode === 'getter') {
    Object.defineProperty(AudioContext.prototype, 'audioWorklet', {configurable: true, get() { throw failure(); }});
  } else if (mode !== 'observe') {
    Worklet.prototype.addModule = function () {
      if (mode === 'throw') throw failure();
      if (mode === 'reject') return Promise.reject(failure());
      return new Promise((resolve, reject) => {
        finishModule = mode === 'resolve-late' ? resolve : () => reject(failure());
      });
    };
  }
  return {
    created() { return created; },
    revoked() { return revoked; },
    nodes() { return nodes; },
    pending() { return finishModule !== undefined; },
    async revokedFetch() {
      try { await fetch(url); return false; } catch { return true; }
    },
    settle() {
      if (finishModule) finishModule();
      finishModule = undefined;
    },
    restore() {
      this.settle();
      if (descriptor) Object.defineProperty(AudioContext.prototype, 'audioWorklet', descriptor);
      else delete AudioContext.prototype.audioWorklet;
      Worklet.prototype.addModule = add;
      globalThis.AudioWorkletNode = Node;
      URL.createObjectURL = create;
      URL.revokeObjectURL = revoke;
    }
  };
}
")]
extern "C" {
    /// Owns the injected module failure and the browser's actual URL observations.
    type WorkletProbe;
    #[wasm_bindgen(js_name = probeWorklet)]
    fn probe_worklet(mode: &str) -> WorkletProbe;
    #[wasm_bindgen(method)]
    fn created(this: &WorkletProbe) -> u32;
    #[wasm_bindgen(method)]
    fn revoked(this: &WorkletProbe) -> u32;
    /// Observe even attempted construction when a late mock completion did not load a real module.
    #[wasm_bindgen(method)]
    fn nodes(this: &WorkletProbe) -> u32;
    #[wasm_bindgen(method)]
    fn pending(this: &WorkletProbe) -> bool;
    #[wasm_bindgen(method, js_name = revokedFetch)]
    fn revoked_fetch(this: &WorkletProbe) -> Promise;
    #[wasm_bindgen(method)]
    fn settle(this: &WorkletProbe);
    #[wasm_bindgen(method)]
    fn restore(this: &WorkletProbe);
    /// Replace permission with an explicit rejection, retaining the exact callable for restoration.
    #[wasm_bindgen(js_name = denyMedia)]
    fn deny_media() -> Function;
    /// Restore the browser API after one isolated permission regression.
    #[wasm_bindgen(js_name = restoreMedia)]
    fn restore_media(previous: &Function);
    /// Hold permission unresolved so cancellation can precede an eventual successful grant.
    #[wasm_bindgen(js_name = gateMedia)]
    fn gate_media() -> Function;
    /// Complete held permission using the browser's real fake-device implementation.
    #[wasm_bindgen(js_name = releaseMedia)]
    fn release_media(previous: &Function) -> Promise;
}

/// Restores patched browser functions even if an assertion returns a failure.
struct ScopedWorkletProbe(WorkletProbe);

impl Drop for ScopedWorkletProbe {
    fn drop(&mut self) {
        self.0.restore();
    }
}

/// Yield to actual browser message/promise delivery between bounded acceptance polls.
pub(super) async fn tick() -> Result<(), JsValue> {
    let promise = Promise::new(&mut |resolve, _| {
        web_sys::window()
            .expect("browser fixture")
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 10)
            .expect("timer");
    });
    JsFuture::from(promise).await?;
    Ok(())
}
/// Map status failures to diagnostic strings the browser runner's FAIL element can preserve.
fn status(status: NcStatus) -> JsValue {
    JsValue::from_str(&status.to_string())
}
/// An assertion remains a rejected JS promise rather than an opaque Wasm abort.
fn ensure(condition: bool, message: &str) -> Result<(), JsValue> {
    if condition { Ok(()) } else { Err(JsValue::from_str(message)) }
}

/// Wait for one construction transaction without mutating its ownership on pending/error phases.
async fn ready(request: &mut IAudioOpenRequestWrapper) -> Result<(), JsValue> {
    for _ in 0..2000 {
        match request.poll().map_err(status)? {
            AudioOpenState::Ready => return Ok(()),
            AudioOpenState::Failed(code) => {
                return Err(JsValue::from_str(&format!(
                    "audio open {code}: {}",
                    request.message().map_err(status)?.to_string().map_err(status)?
                )));
            }
            AudioOpenState::Pending => tick().await?,
            _ => return Err("audio request unexpectedly terminal".into()),
        }
    }
    Err("audio open timed out".into())
}

/// Calls the generated request API and checks URL ownership on every module exit.
async fn worklet_module_cleanup(platform: &ComPtr<PlatformAbi>, config: AudioConfig) -> Result<(), JsValue> {
    for mode in ["getter", "throw", "reject"] {
        let probe = ScopedWorkletProbe(probe_worklet(mode));
        let mut request = platform.open_audio(AudioDirection::Playback, config).map_err(status)?;
        let Err(failure) = ready(&mut request).await else {
            return Err("module failure must reject the audio open".into());
        };
        ensure(
            failure.as_string().is_some_and(|s| s.contains(&format!("injected worklet {mode}"))),
            "module failure lost its diagnostic",
        )?;
        let expected = u32::from(mode != "getter");
        ensure(probe.0.created() == expected && probe.0.revoked() == expected, "module failure left a live URL")?;
    }
    for (mode, cancel) in [("resolve-late", true), ("reject-late", false)] {
        let probe = ScopedWorkletProbe(probe_worklet(mode));
        let mut request = platform.open_audio(AudioDirection::Playback, config).map_err(status)?;
        for _ in 0..2000 {
            if probe.0.pending() {
                break;
            }
            tick().await?;
        }
        ensure(probe.0.pending() && probe.0.created() == 1, "worklet load never entered the pending gate")?;
        let retained = if cancel {
            request.cancel().map_err(status)?;
            ensure(
                request.poll().map_err(status)? == AudioOpenState::Cancelled,
                "cancel failed to retire the request",
            )?;
            Some(request)
        } else {
            drop(request);
            None
        };
        ensure(probe.0.revoked() == 1, "pending retirement did not revoke the URL")?;
        let unavailable = JsFuture::from(probe.0.revoked_fetch()).await?.as_bool().unwrap_or(false);
        ensure(unavailable, "a new fetch could still read the revoked worklet module")?;
        probe.0.settle();
        tick().await?;
        ensure(probe.0.revoked() == 1, "late module completion revoked the URL twice")?;
        ensure(probe.0.nodes() == 0, "late module completion constructed an audio node")?;
        if let Some(mut request) = retained {
            ensure(
                request.poll().map_err(status)? == AudioOpenState::Cancelled,
                "late success reopened the request",
            )?;
        }
    }
    Ok(())
}
/// Observe exact control completion through the generated inherited stream wrapper.
async fn phase(stream: &mut IAudioStreamWrapper, target: AudioState) -> Result<(), JsValue> {
    for _ in 0..2000 {
        let info = stream.info().map_err(status)?;
        if info.state == target {
            return Ok(());
        }
        if let AudioState::Failed(code) = info.state {
            return Err(JsValue::from_str(&format!(
                "audio control {code}: {}",
                stream.failure_message().map_err(status)?.to_string().map_err(status)?
            )));
        }
        tick().await?;
    }
    Err(JsValue::from_str(&format!("audio phase {target:?} timed out")))
}

/// Exercises browser audio through the platform root acquired from the linked host.
pub async fn run(platform: ComPtr<PlatformAbi>) -> Result<(), JsValue> {
    let capabilities = platform.audio_capabilities().map_err(status)?;
    let info = capabilities.info().map_err(status)?;
    ensure(info.backend == AudioBackend::WebAudio, "wrong browser audio backend")?;
    ensure(
        matches!(
            info.capture,
            AudioSupport::Available {
                access: AudioAccess::BrowserMicrophone,
                ..
            }
        ),
        "browser capture unavailable",
    )?;
    ensure(
        capabilities.supported_pcm_types().map_err(status)?.contains(&PcmType::FixedPoint(32, 16)),
        "fixed-point capability missing",
    )?;
    let config = AudioConfig {
        capacity_frames: 128,
        ..AudioConfig::stereo()
    };
    ensure(capabilities.supports_config(config).map_err(status)?, "default audio config unsupported")?;
    worklet_module_cleanup(&platform, config).await?;
    let mut request = platform.open_audio(AudioDirection::Playback, config).map_err(status)?;
    {
        let probe = ScopedWorkletProbe(probe_worklet("observe"));
        ready(&mut request).await?;
        ensure(
            probe.0.created() == 1 && probe.0.revoked() == 1 && probe.0.nodes() == 1,
            "successful audio open did not retire its URL before publishing the node",
        )?;
    }
    ensure(
        request.take_capture().err() == Some(NC_E_NOINTERFACE),
        "playback request leaked capture authority",
    )?;
    let mut playback = request.take_playback().map_err(status)?;
    ensure(playback.query_ref::<IAudioCaptureAbi>().is_err(), "playback QI leaked capture")?;
    let pcm = vec![0; 128 * 2 * 4];
    playback.write(NcCopyFromBufferOffset::new(0, &pcm)).map_err(status)?;
    ensure(playback.info().map_err(status)?.queued_frames() == 128, "playback admission lost frames")?;
    ensure(
        playback.write(NcCopyFromBufferOffset::new(0, &[0; 8])) == Err(AUDIO_E_LIMIT),
        "playback capacity was exceeded",
    )?;
    playback.start().map_err(status)?;
    phase(&mut playback, AudioState::Running).await?;
    for _ in 0..2000 {
        if playback.info().map_err(status)?.queued_frames() == 0 {
            break;
        }
        tick().await?;
    }
    ensure(playback.info().map_err(status)?.queued_frames() == 0, "audio worklet did not consume PCM")?;
    playback.pause().map_err(status)?;
    phase(&mut playback, AudioState::Paused).await?;
    playback.clear().map_err(status)?;
    phase(&mut playback, AudioState::Paused).await?;
    playback.close().map_err(status)?;
    playback.close().map_err(status)?;
    ensure(playback.info().map_err(status)?.state == AudioState::Closed, "playback close not terminal")?;

    let previous = deny_media();
    let mut denied = platform.open_audio(AudioDirection::Capture, config).map_err(status)?;
    let outcome = ready(&mut denied).await;
    restore_media(&previous);
    ensure(outcome.is_err(), "denied microphone permission unexpectedly opened")?;
    let diagnostic = denied.message().map_err(status)?;
    drop(denied);
    ensure(
        diagnostic.to_string().map_err(status)?.contains("synthetic audio permission denied"),
        "permission failure lost its owned diagnostic",
    )?;

    let previous = gate_media();
    let mut cancelled = platform.open_audio(AudioDirection::Capture, config).map_err(status)?;
    tick().await?;
    cancelled.cancel().map_err(status)?;
    let media = JsFuture::from(release_media(&previous)).await?.dyn_into::<web_sys::MediaStream>()?;
    for _ in 0..2000 {
        let ended = media
            .get_tracks()
            .iter()
            .all(|track| track.unchecked_into::<web_sys::MediaStreamTrack>().ready_state() == web_sys::MediaStreamTrackState::Ended);
        if ended {
            break;
        }
        tick().await?;
    }
    ensure(
        cancelled.poll().map_err(status)? == AudioOpenState::Cancelled,
        "late permission changed a cancelled request",
    )?;
    ensure(
        media
            .get_tracks()
            .iter()
            .all(|track| track.unchecked_into::<web_sys::MediaStreamTrack>().ready_state() == web_sys::MediaStreamTrackState::Ended),
        "cancelled permission leaked microphone tracks",
    )?;

    let config = AudioConfig {
        format: PcmFormat {
            sample_type: PcmType::SignedInteger(16),
            byte_order: PcmByteOrder::BigEndian,
        },
        channels: AudioChannels::Mono,
        capacity_frames: 256,
        ..config
    };
    let mut request = platform.open_audio(AudioDirection::Capture, config).map_err(status)?;
    ready(&mut request).await?;
    let mut capture = request.take_capture().map_err(status)?;
    ensure(capture.query_ref::<IAudioPlaybackAbi>().is_err(), "capture QI leaked output")?;
    capture.start().map_err(status)?;
    phase(&mut capture, AudioState::Running).await?;
    for _ in 0..2000 {
        if capture.info().map_err(status)?.queued_frames() != 0 {
            break;
        }
        tick().await?;
    }
    capture.pause().map_err(status)?;
    phase(&mut capture, AudioState::Paused).await?;
    let info = capture.info().map_err(status)?;
    ensure(info.queued_frames() > 0 && info.queued_frames() <= 256, "capture did not deliver bounded PCM")?;
    let mut output = NcOutputBuffer::<u8>::new();
    let frames = capture.read(output.output(0, 256)).map_err(status)?;
    ensure(
        frames > 0 && frames <= 128 && output.len() == frames as usize * 2,
        "capture output prefix/count invalid",
    )?;
    for sample in output.as_slice().as_chunks::<2>().0 {
        config.format.decode(sample).map_err(status)?;
    }
    capture.clear().map_err(status)?;
    phase(&mut capture, AudioState::Paused).await?;
    ensure(capture.info().map_err(status)?.queued_frames() == 0, "capture reset retained old generation")?;
    capture.close().map_err(status)?;
    capture.close().map_err(status)?;
    super::browser::verify_drop_cleanup().await?;
    Ok(())
}
