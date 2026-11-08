//! SDL callback ownership. Only SDL's required device lock crosses the application/audio-thread boundary.
use super::*;
use sdl2::audio::{AudioCallback, AudioDevice, AudioSpecDesired};
use std::collections::VecDeque;

/// Preallocated callback-owned PCM; no Rust allocation, COM or application call occurs in a callback.
struct Pcm {
    /// Interleaved complete frames with a fixed logical capacity.
    samples: VecDeque<f32>,
    /// Immutable application-side layout and queue limit.
    config: AudioConfig,
    /// Newest capture frames discarded on capacity exhaustion.
    dropped: u64,
    /// Output frames replaced by silence on queue starvation.
    silent: u64,
}
impl Pcm {
    /// Reserve all callback storage before opening/starting the native device.
    fn new(config: AudioConfig) -> Result<Self, AudioError> {
        let mut samples = VecDeque::new();
        samples
            .try_reserve_exact(config.capacity_frames as usize * config.channels.count() as usize)
            .map_err(|error| AudioError::new(AUDIO_E_LIMIT, error.to_string()))?;
        Ok(Self { samples, config, dropped: 0, silent: 0 })
    }
    /// Count whole retained frames; all producers and consumers preserve channel alignment.
    fn frames(&self) -> usize {
        self.samples.len() / self.config.channels.count() as usize
    }
    /// Render available frames and initialize every remaining native output sample to silence.
    fn playback(&mut self, output: &mut [f32]) {
        let available = self.samples.len().min(output.len());
        for sample in &mut output[..available] {
            *sample = self.samples.pop_front().expect("counted PCM sample");
        }
        output[available..].fill(0.);
        self.silent = self
            .silent
            .saturating_add(((output.len() - available) / self.config.channels.count() as usize) as u64);
    }
    /// Retain complete normalized frames only; excess newest frames are dropped without reallocating.
    fn capture(&mut self, input: &[f32]) {
        let channels = self.config.channels.count() as usize;
        let available = (self.config.capacity_frames as usize - self.frames()).min(input.len() / channels);
        self.samples.extend(
            input[..available * channels]
                .iter()
                .map(|sample| if sample.is_finite() { sample.clamp(-1., 1.) } else { 0. }),
        );
        self.dropped = self.dropped.saturating_add((input.len() / channels - available) as u64);
    }
}

/// Playback callback owns its queue; the application borrows it only while SDL stops callback access.
struct Output(Pcm);
impl AudioCallback for Output {
    type Channel = f32;
    fn callback(&mut self, output: &mut [f32]) {
        self.0.playback(output);
    }
}
/// Capture callback owns the bounded microphone queue and never lends native callback storage.
struct Input(Pcm);
impl AudioCallback for Input {
    type Channel = f32;
    fn callback(&mut self, input: &mut [f32]) {
        self.0.capture(input);
    }
}

/// A typed native device owns both its callback and SDL audio-subsystem guard.
struct Native<C: AudioCallback> {
    /// None after close; dropping AudioDevice waits for its callback before releasing storage.
    device: Option<AudioDevice<C>>,
    /// Stable PCM request, retained for metadata after close.
    config: AudioConfig,
    /// Application-side lifecycle, with synchronous native start/pause.
    state: AudioState,
    /// Final counters retained after native device destruction.
    counters: (u64, u64),
}

/// Playback owner is distinct from capture at both Rust and COM boundaries.
pub(super) struct Playback(Native<Output>);
/// Capture owner retains microphone lifetime without exposing native handles.
pub(super) struct Capture(Native<Input>);

/// SDL enumeration initializes only the audio subsystem; lack of enumeration does not erase codec support.
pub(super) fn capabilities() -> AudioCapabilities {
    let mut capabilities = AudioCapabilities::portable(AudioBackend::Sdl);
    match sdl2::init().and_then(|sdl| sdl.audio()) {
        Ok(audio) => {
            capabilities.playback = AudioSupport::Available {
                devices: audio.num_audio_playback_devices().map_or(AudioDeviceCount::Unknown, AudioDeviceCount::Known),
                access: AudioAccess::System,
            };
            capabilities.capture = AudioSupport::Available {
                devices: audio.num_audio_capture_devices().map_or(AudioDeviceCount::Unknown, AudioDeviceCount::Known),
                access: AudioAccess::System,
            };
        }
        Err(_) => {
            let unavailable = AudioSupport::Available {
                devices: AudioDeviceCount::Failed(AUDIO_E_BACKEND.code()),
                access: AudioAccess::System,
            };
            capabilities.playback = unavailable;
            capabilities.capture = unavailable;
        }
    }
    capabilities
}

/// Open SDL audio alone: no video subsystem, event pump, native window or graphics API is initialized.
pub(super) fn open(direction: AudioDirection, config: AudioConfig) -> RequestState {
    let result = (|| -> Result<Stream, AudioError> {
        let pcm = Pcm::new(config)?;
        let sdl = sdl2::init().map_err(|error| AudioError::new(AUDIO_E_BACKEND, error))?;
        let audio = sdl.audio().map_err(|error| AudioError::new(AUDIO_E_BACKEND, error))?;
        let desired = AudioSpecDesired {
            freq: Some(config.sample_rate as i32),
            channels: Some(config.channels.count() as u8),
            samples: Some(512),
        };
        if direction == AudioDirection::Playback {
            let device = audio
                .open_playback(None, &desired, |_| Output(pcm))
                .map_err(|error| AudioError::new(AUDIO_E_BACKEND, error))?;
            Ok(Stream::Playback(Playback(Native {
                device: Some(device),
                config,
                state: AudioState::Paused,
                counters: (0, 0),
            })))
        } else {
            let device = audio
                .open_capture(None, &desired, |_| Input(pcm))
                .map_err(|error| AudioError::new(AUDIO_E_BACKEND, error))?;
            Ok(Stream::Capture(Capture(Native {
                device: Some(device),
                config,
                state: AudioState::Paused,
                counters: (0, 0),
            })))
        }
    })();
    match result {
        Ok(stream) => RequestState::Ready(stream),
        Err(error) => RequestState::Failed(error),
    }
}

/// Forward typed queue access only while the SDL lock grants exclusive callback ownership.
macro_rules! native_controls {
    ($owner:ty, $capture:expr) => {
        impl Driver for $owner {
            fn config(&self) -> AudioConfig {
                self.0.config
            }
            fn info(&mut self) -> Result<AudioInfo, NcStatus> {
                let (queued_frames, dropped_frames, silent_frames) = if let Some(device) = &mut self.0.device {
                    let callback = device.lock();
                    (callback.0.frames() as u32, callback.0.dropped, callback.0.silent)
                } else {
                    (0, self.0.counters.0, self.0.counters.1)
                };
                Ok(AudioInfo {
                    config: self.0.config,
                    state: self.0.state,
                    queue: if $capture {
                        AudioQueueInfo::Capture { queued_frames, dropped_frames }
                    } else {
                        AudioQueueInfo::Playback { queued_frames, silent_frames }
                    },
                })
            }
            fn start(&mut self) -> Result<(), NcStatus> {
                let device = self.0.device.as_ref().ok_or(AUDIO_E_STATE)?;
                device.resume();
                self.0.state = AudioState::Running;
                Ok(())
            }
            fn pause(&mut self) -> Result<(), NcStatus> {
                let device = self.0.device.as_ref().ok_or(AUDIO_E_STATE)?;
                device.pause();
                self.0.state = AudioState::Paused;
                Ok(())
            }
            fn clear(&mut self) -> Result<(), NcStatus> {
                if self.0.state != AudioState::Paused {
                    return Err(AUDIO_E_STATE);
                }
                self.0.device.as_mut().ok_or(AUDIO_E_STATE)?.lock().0.samples.clear();
                Ok(())
            }
            fn close(&mut self) {
                if let Some(mut device) = self.0.device.take() {
                    device.pause();
                    {
                        let callback = device.lock();
                        self.0.counters = (callback.0.dropped, callback.0.silent);
                    }
                    drop(device);
                }
                self.0.state = AudioState::Closed;
            }
            fn failure(&self) -> Option<&AudioError> {
                None
            }
        }
    };
}
native_controls!(Playback, false);
native_controls!(Capture, true);

impl Playback {
    /// Admission precedes one bounded callback borrow; no partial PCM is committed on overflow.
    pub(super) fn write(&mut self, samples: &[f32]) -> Result<(), NcStatus> {
        let mut callback = self.0.device.as_mut().ok_or(AUDIO_E_STATE)?.lock();
        let frames = samples.len() / self.0.config.channels.count() as usize;
        if callback.0.frames() + frames > self.0.config.capacity_frames as usize {
            return Err(AUDIO_E_LIMIT);
        }
        callback.0.samples.extend(samples);
        Ok(())
    }
}
impl Capture {
    /// The validated writer admits a complete prefix before any callback-owned frame is consumed.
    pub(super) fn read(&mut self, output: &mut [f32]) -> Result<u32, NcStatus> {
        let mut callback = self.0.device.as_mut().ok_or(AUDIO_E_STATE)?.lock();
        let channels = self.0.config.channels.count() as usize;
        let frames = callback.0.frames().min(output.len() / channels);
        for sample in &mut output[..frames * channels] {
            *sample = callback.0.samples.pop_front().expect("counted capture frame");
        }
        Ok(frames as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Callbacks preserve channel ordering, initialize starvation output, drop newest overflow and allocate once.
    #[test]
    fn bounded_pcm_callbacks() {
        let mut pcm = Pcm::new(AudioConfig {
            capacity_frames: 128,
            ..AudioConfig::stereo()
        })
        .unwrap();
        let capacity = pcm.samples.capacity();
        pcm.samples.extend([1., -1., 0.5, -0.5]);
        let mut output = [f32::NAN; 6];
        pcm.playback(&mut output);
        assert_eq!(output, [1., -1., 0.5, -0.5, 0., 0.]);
        assert_eq!(pcm.silent, 1);
        pcm.capture(&[2., f32::NAN]);
        pcm.capture(&[0.25; 256]);
        assert_eq!(pcm.frames(), 128);
        assert_eq!(pcm.dropped, 1);
        assert_eq!(pcm.samples[0], 1.);
        assert_eq!(pcm.samples[1], 0.);
        assert_eq!(pcm.samples.capacity(), capacity);
    }
}
