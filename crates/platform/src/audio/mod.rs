//! Typed PCM services. No application/COM callback executes on a native audio thread.
#![deny(missing_docs)]
use neocogi_com::{ComBox, ComPtr, INcStringAbi, NcCopyFromBufferOffset, NcOutputWriter, NcStatus, NcString, NC_E_NOINTERFACE, com_object};
/// Canonical audio values and generated clients, exposed here for linked Rust applications.
pub use neocogi_platform_contracts::audio::*;

/// Actual browser playback/capture acceptance under the repository's fake-device headless fixture.
#[cfg(all(target_arch = "wasm32", feature = "test-support"))]
pub mod testing;

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native as backend;
#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
use browser as backend;

#[cfg(test)]
mod diagnostic_tests;

/// Controls shared by concrete playback/capture owners without erasing their direction or implementation.
trait Driver {
    /// Fixed application PCM configuration.
    fn config(&self) -> AudioConfig;
    /// Updates asynchronous completion and observes bounded counters.
    fn info(&mut self) -> Result<AudioInfo, NcStatus>;
    /// Initiates playback/capture without resetting retained PCM.
    fn start(&mut self) -> Result<(), NcStatus>;
    /// Stops processing while retaining queued PCM.
    fn pause(&mut self) -> Result<(), NcStatus>;
    /// Discards PCM while paused, retaining lifetime counters.
    fn clear(&mut self) -> Result<(), NcStatus>;
    /// Releases native resources idempotently.
    fn close(&mut self);
    /// Terminal backend failure, independent of other streams and requests.
    fn failure(&self) -> Option<&AudioError>;
}

/// A ready opening transaction owns exactly one concrete direction until its successful typed take.
enum Stream {
    /// Playback driver grants no microphone capability.
    Playback(backend::Playback),
    /// Capture driver grants no output capability.
    Capture(backend::Capture),
}

/// Owned opening state; only browser event completion requires a shared mailbox.
enum RequestState {
    /// Async permission/worklet initialization has a weakly referenced completion mailbox.
    #[cfg(target_arch = "wasm32")]
    Pending(browser::Pending),
    /// Fully constructed paused stream, never exposed before one successful typed take.
    Ready(Stream),
    /// Immutable independently owned error, retained after rejected takes.
    Failed(AudioError),
    /// Terminal successful transfer.
    Taken,
    /// Terminal explicit cancellation, also invalidating a pending completion mailbox.
    Cancelled,
}

#[com_object(interfaces(IAudioOpenRequest), ownership = exclusive)]
/// Exclusive construction transaction; dropping a pending/ready state cleans up all native resources.
struct OpenRequest {
    /// One authoritative lifecycle value, mutated only under exclusive COM access.
    state: RequestState,
}

impl OpenRequest {
    /// Incorporates a browser completion without holding a mailbox borrow during native destruction.
    fn refresh(&mut self) {
        #[cfg(target_arch = "wasm32")]
        if let RequestState::Pending(pending) = &mut self.state
            && let Some(result) = pending.poll()
        {
            self.state = match result {
                Ok(stream) => RequestState::Ready(stream),
                Err(error) => RequestState::Failed(error),
            };
        }
    }
    /// Read the request discriminator without changing any capability ownership.
    fn phase(&self) -> AudioOpenState {
        match &self.state {
            #[cfg(target_arch = "wasm32")]
            RequestState::Pending(_) => AudioOpenState::Pending,
            RequestState::Ready(_) => AudioOpenState::Ready,
            RequestState::Failed(error) => AudioOpenState::Failed(error.status.code()),
            RequestState::Taken => AudioOpenState::Taken,
            RequestState::Cancelled => AudioOpenState::Cancelled,
        }
    }
    /// Failure and terminal takes preserve the original request state; only the matching ready direction moves.
    fn take(&mut self, direction: AudioDirection) -> Result<Stream, NcStatus> {
        self.refresh();
        match &self.state {
            RequestState::Ready(Stream::Playback(_)) if direction != AudioDirection::Playback => return Err(NC_E_NOINTERFACE),
            RequestState::Ready(Stream::Capture(_)) if direction != AudioDirection::Capture => return Err(NC_E_NOINTERFACE),
            RequestState::Ready(_) => (),
            RequestState::Failed(error) => return Err(error.status),
            #[cfg(target_arch = "wasm32")]
            RequestState::Pending(_) => return Err(AUDIO_E_PENDING),
            _ => return Err(AUDIO_E_STATE),
        }
        let RequestState::Ready(stream) = std::mem::replace(&mut self.state, RequestState::Taken) else {
            unreachable!("checked ready state")
        };
        Ok(stream)
    }
}
impl IAudioOpenRequest for OpenRequest {
    fn poll(&mut self) -> Result<AudioOpenState, NcStatus> {
        self.refresh();
        Ok(self.phase())
    }
    fn message(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(NcString::from_utf8_str(match &self.state {
            RequestState::Failed(error) => &error.message,
            _ => "",
        }))
    }
    fn take_playback(&mut self) -> Result<ComBox<IAudioPlaybackAbi>, NcStatus> {
        let Stream::Playback(driver) = self.take(AudioDirection::Playback)? else {
            unreachable!("checked playback direction")
        };
        Ok(PlaybackObject::into_local_box(PlaybackObject { driver })
            .into_interface()
            .unwrap_or_else(|(status, _)| panic!("playback interface: {status}")))
    }
    fn take_capture(&mut self) -> Result<ComBox<IAudioCaptureAbi>, NcStatus> {
        let Stream::Capture(driver) = self.take(AudioDirection::Capture)? else {
            unreachable!("checked capture direction")
        };
        Ok(CaptureObject::into_local_box(CaptureObject { driver })
            .into_interface()
            .unwrap_or_else(|(status, _)| panic!("capture interface: {status}")))
    }
    fn cancel(&mut self) -> Result<(), NcStatus> {
        if !matches!(self.state, RequestState::Taken) {
            self.state = RequestState::Cancelled;
        }
        Ok(())
    }
}

#[com_object(interfaces(IAudioPlayback), ownership = exclusive)]
/// Exclusive PCM producer, exposing only playback and inherited stream controls.
struct PlaybackObject {
    /// Concrete owner keeps its SDL subsystem or browser graph alive through final release.
    driver: backend::Playback,
}
#[com_object(interfaces(IAudioCapture), ownership = exclusive)]
/// Exclusive PCM consumer, exposing only capture and inherited stream controls.
struct CaptureObject {
    /// Concrete owner retains microphone resources until explicit close or final release.
    driver: backend::Capture,
}

/// Generate only the identical typed control forwarding, keeping directional I/O implementations explicit.
macro_rules! controls {
    ($object:ty) => {
        impl IAudioStream for $object {
            fn info(&mut self) -> Result<AudioInfo, NcStatus> {
                self.driver.info()
            }
            fn start(&mut self) -> Result<(), NcStatus> {
                self.driver.start()
            }
            fn pause(&mut self) -> Result<(), NcStatus> {
                self.driver.pause()
            }
            fn clear(&mut self) -> Result<(), NcStatus> {
                self.driver.clear()
            }
            fn close(&mut self) -> Result<(), NcStatus> {
                self.driver.close();
                Ok(())
            }
            fn failure_message(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
                Ok(NcString::from_utf8_str(self.driver.failure().map_or("", |error| error.message.as_str())))
            }
        }
    };
}
controls!(PlaybackObject);
controls!(CaptureObject);

impl IAudioPlayback for PlaybackObject {
    fn write(&mut self, samples: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        let mut processing = [0.; MAX_AUDIO_TRANSFER_FRAMES * 2];
        let count = self.driver.config().decode(samples.offset(), samples.as_slice(), &mut processing)?;
        self.driver.write(&processing[..count])
    }
}
impl IAudioCapture for CaptureObject {
    fn read(&mut self, mut output: NcOutputWriter<'_, u8>) -> Result<u32, NcStatus> {
        let config = self.driver.config();
        let frames = config.transfer_frames(output.offset(), output.capacity())?;
        let mut processing = [0.; MAX_AUDIO_TRANSFER_FRAMES * 2];
        let read = self.driver.read(&mut processing[..frames * config.channels.count() as usize])?;
        let bytes = config.format.sample_bytes()?;
        for sample in &processing[..read as usize * config.channels.count() as usize] {
            let encoded = config.format.encode(*sample).expect("captured processing PCM is normalized");
            output.extend_from_slice(&encoded[..bytes]).expect("validated PCM output capacity");
        }
        Ok(read)
    }
}

/// Open the default output or microphone independently of a window, graphics backend or shader compiler.
///
/// Call `poll` on the returned exclusive request. A native result is immediately Ready/Failed;
/// browsers await permission/worklet initialization. Cancel/drop closes any untaken device and
/// prevents late microphone completion from publishing a stream. All successful streams start paused.
pub fn open(direction: AudioDirection, config: AudioConfig) -> Result<ComBox<IAudioOpenRequestAbi>, NcStatus> {
    let admission = direction.validate().and_then(|()| config.validate());
    let state = match admission {
        Err(status) => RequestState::Failed(AudioError::new(status, "invalid PCM direction or configuration")),
        Ok(()) => backend::open(direction, config),
    };
    Ok(OpenRequest::into_local_box(OpenRequest { state })
        .into_interface()
        .unwrap_or_else(|(status, _)| panic!("audio opening interface: {status}")))
}

/// Query PCM profile, backend API support and available device-count observations without opening a device.
/// Browser queries never request microphone permission; enumeration has a typed Unknown result. Format support describes
/// application-side conversion and does not claim native hardware precision or guarantee a later open.
pub fn capabilities() -> Result<ComPtr<IAudioCapabilitiesAbi>, NcStatus> {
    let mut types = vec![PcmType::FloatingPoint(32), PcmType::FloatingPoint(64)];
    for bits in [8, 16, 24, 32] {
        types.extend([PcmType::UnsignedInteger(bits), PcmType::SignedInteger(bits)]);
        for fraction in 0..bits {
            types.push(PcmType::FixedPoint(bits, fraction));
        }
    }
    CapabilitySnapshot::from_value(CapabilitySnapshot { info: backend::capabilities(), types }).query()
}

#[com_object(interfaces(IAudioCapabilities))]
/// Immutable backend observation and exact typed numerical formats; no native devices are retained.
struct CapabilitySnapshot {
    /// Copied typed availability/access and common limits.
    info: AudioCapabilities,
    /// Concrete supported variants, without masks or independent fractional-bit fields.
    types: Vec<PcmType>,
}
/// Validate a complete copied interval before publishing any output element.
fn copy_capabilities<T: neocogi_com::ComInlineValue>(values: &[T], mut output: NcOutputWriter<'_, T>) -> Result<(), NcStatus> {
    let start = usize::try_from(output.offset()).map_err(|_| neocogi_com::NC_E_INVALID_ARG)?;
    let end = start.checked_add(output.capacity()).ok_or(neocogi_com::NC_E_INVALID_ARG)?;
    output.extend_from_slice(values.get(start..end).ok_or(neocogi_com::NC_E_INVALID_ARG)?)
}
impl IAudioCapabilities for CapabilitySnapshot {
    fn info(&self) -> Result<AudioCapabilities, NcStatus> {
        Ok(self.info)
    }
    fn pcm_type_count(&self) -> Result<u32, NcStatus> {
        Ok(self.types.len() as u32)
    }
    fn pcm_types(&self, output: NcOutputWriter<'_, PcmType>) -> Result<(), NcStatus> {
        copy_capabilities(&self.types, output)
    }
    fn byte_order_count(&self) -> Result<u32, NcStatus> {
        Ok(2)
    }
    fn byte_orders(&self, output: NcOutputWriter<'_, PcmByteOrder>) -> Result<(), NcStatus> {
        copy_capabilities(&[PcmByteOrder::LittleEndian, PcmByteOrder::BigEndian], output)
    }
    fn channel_count(&self) -> Result<u32, NcStatus> {
        Ok(2)
    }
    fn channels(&self, output: NcOutputWriter<'_, AudioChannels>) -> Result<(), NcStatus> {
        copy_capabilities(&[AudioChannels::Mono, AudioChannels::Stereo], output)
    }
}
