//! Rust PCM values, bounded conversion and typed capability readers without a backend implementation.
use super::*;
use neocogi_com::{NcOutputBuffer, NcStatus, NC_E_INVALID_ARG};

/// Browser permission/control or reset completion has not arrived.
pub const AUDIO_E_PENDING: NcStatus = NcStatus::from_code(-2001);
/// A bounded queue, allocation or per-call transfer limit would be exceeded.
pub const AUDIO_E_LIMIT: NcStatus = NcStatus::from_code(-2002);
/// Operation is illegal in the current phase or after close/failure.
pub const AUDIO_E_STATE: NcStatus = NcStatus::from_code(-2003);
/// Native device, browser permission or worklet processing failed.
pub const AUDIO_E_BACKEND: NcStatus = NcStatus::from_code(-2005);
/// Maximum complete frames copied by one operation.
pub const MAX_AUDIO_TRANSFER_FRAMES: usize = 4096;
/// Maximum bytes owned by one immutable UTF-8 diagnostic.
pub const MAX_AUDIO_ERROR_BYTES: usize = 4096;
/// Minimum application-side sample rate.
pub const MIN_AUDIO_SAMPLE_RATE: u32 = 8000;
/// Maximum application-side sample rate.
pub const MAX_AUDIO_SAMPLE_RATE: u32 = 192000;
/// Minimum bounded queue capacity in frames.
pub const MIN_AUDIO_CAPACITY_FRAMES: u32 = 128;
/// Maximum bounded queue capacity in frames.
pub const MAX_AUDIO_CAPACITY_FRAMES: u32 = 65536;

impl AudioChannels {
    /// Samples per frame; unknown forward ABI values return zero and fail configuration admission.
    pub const fn count(self) -> u32 {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            _ => 0,
        }
    }
}
impl AudioLimits {
    /// One canonical admission profile, shared by queries and actual stream opening.
    pub const fn portable() -> Self {
        Self {
            sample_rate: AudioRange {
                min: MIN_AUDIO_SAMPLE_RATE,
                max: MAX_AUDIO_SAMPLE_RATE,
            },
            capacity_frames: AudioRange {
                min: MIN_AUDIO_CAPACITY_FRAMES,
                max: MAX_AUDIO_CAPACITY_FRAMES,
            },
            transfer_frames: MAX_AUDIO_TRANSFER_FRAMES as u32,
        }
    }
    /// Validate an application request; physical availability and permission are resolved during opening.
    pub fn admits(self, config: AudioConfig) -> Result<(), NcStatus> {
        config.format.validate()?;
        if config.channels.count() == 0
            || !(self.sample_rate.min..=self.sample_rate.max).contains(&config.sample_rate)
            || !(self.capacity_frames.min..=self.capacity_frames.max).contains(&config.capacity_frames)
        {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(())
    }
}
impl AudioCapabilities {
    /// Default portable profile; providers replace typed host/API observations, never PCM mask bits.
    pub const fn portable(backend: AudioBackend) -> Self {
        let support = AudioSupport::Available {
            devices: AudioDeviceCount::Unknown,
            access: AudioAccess::System,
        };
        Self {
            backend,
            playback: support,
            capture: support,
            limits: AudioLimits::portable(),
        }
    }
}
impl IAudioCapabilitiesClient {
    /// Read all supported numerical variants from a bounded immutable capability snapshot.
    pub fn supported_pcm_types(&self) -> Result<Vec<PcmType>, NcStatus> {
        let count = self.pcm_type_count()? as usize;
        if count > 128 {
            return Err(AUDIO_E_LIMIT);
        }
        let mut output = NcOutputBuffer::new();
        self.pcm_types(output.output(0, count))?;
        if output.len() != count {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(output.into_vec())
    }
    /// Read supported byte orders as typed values rather than flags.
    pub fn supported_byte_orders(&self) -> Result<Vec<PcmByteOrder>, NcStatus> {
        let count = self.byte_order_count()? as usize;
        if count > 2 {
            return Err(AUDIO_E_LIMIT);
        }
        let mut output = NcOutputBuffer::new();
        self.byte_orders(output.output(0, count))?;
        if output.len() != count {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(output.into_vec())
    }
    /// Read supported channel layouts as typed values rather than a numeric maximum.
    pub fn supported_channels(&self) -> Result<Vec<AudioChannels>, NcStatus> {
        let count = self.channel_count()? as usize;
        if count > 2 {
            return Err(AUDIO_E_LIMIT);
        }
        let mut output = NcOutputBuffer::new();
        self.channels(output.output(0, count))?;
        if output.len() != count {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(output.into_vec())
    }
    /// Compare a complete Rust descriptor with the actual advertised variants and byte orders.
    pub fn supports_format(&self, format: PcmFormat) -> Result<bool, NcStatus> {
        format.validate()?;
        Ok(self.supported_pcm_types()?.contains(&format.sample_type) && self.supported_byte_orders()?.contains(&format.byte_order))
    }
    /// Check advertised application admission without claiming a later device open or permission will succeed.
    pub fn supports_config(&self, config: AudioConfig) -> Result<bool, NcStatus> {
        Ok(self.info()?.limits.admits(config).is_ok() && self.supports_format(config.format)? && self.supported_channels()?.contains(&config.channels))
    }
}
impl AudioQueueInfo {
    /// Retained frame count has the same meaning in both typed direction variants.
    pub fn queued_frames(self) -> u32 {
        match self {
            Self::Playback { queued_frames, .. } | Self::Capture { queued_frames, .. } => queued_frames,
        }
    }
}
impl AudioInfo {
    /// Complete retained frames, without inventing unrelated loss counters for the other direction.
    pub fn queued_frames(self) -> u32 {
        self.queue.queued_frames()
    }
}
impl AudioConfig {
    /// Default application PCM is float32 LE, 48 kHz stereo and an 8192-frame bounded queue.
    pub const fn stereo() -> Self {
        Self {
            format: PcmFormat::float32(),
            sample_rate: 48_000,
            channels: AudioChannels::Stereo,
            capacity_frames: 8192,
        }
    }
    /// Reject unsupported representations and bound all backend allocations before initialization.
    pub fn validate(self) -> Result<(), NcStatus> {
        AudioLimits::portable().admits(self)
    }
    /// Encode ordinary Rust processing samples into this stream's selected PCM representation.
    /// Allocation and all sample checks complete before a caller can publish any queue write.
    pub fn encode(self, samples: &[f32]) -> Result<Vec<u8>, NcStatus> {
        let width = self.format.sample_bytes()?;
        let bytes = samples.len().checked_mul(width).ok_or(AUDIO_E_LIMIT)?;
        self.transfer_frames(0, bytes)?;
        let mut output = Vec::new();
        output.try_reserve_exact(bytes).map_err(|_| AUDIO_E_LIMIT)?;
        for sample in samples {
            output.extend_from_slice(&self.format.encode(*sample)?[..width]);
        }
        Ok(output)
    }
    /// Whole-frame byte admission with zero source offset and a bounded number of samples.
    pub fn transfer_frames(self, offset: u64, bytes: usize) -> Result<usize, NcStatus> {
        self.validate()?;
        let stride = self.channels.count() as usize * self.format.sample_bytes()?;
        if offset != 0 || !bytes.is_multiple_of(stride) {
            return Err(NC_E_INVALID_ARG);
        }
        let frames = bytes / stride;
        if frames > MAX_AUDIO_TRANSFER_FRAMES {
            return Err(AUDIO_E_LIMIT);
        }
        Ok(frames)
    }
    /// Validate every encoded sample before committing any playback queue data.
    pub fn validate_samples(self, offset: u64, bytes: &[u8]) -> Result<usize, NcStatus> {
        let frames = self.transfer_frames(offset, bytes.len())?;
        for sample in bytes.chunks_exact(self.format.sample_bytes()?) {
            self.format.decode(sample)?;
        }
        Ok(frames)
    }
    /// Decode into float32 processing storage; malformed input or capacity failure changes no destination element.
    pub fn decode(self, offset: u64, bytes: &[u8], output: &mut [f32]) -> Result<usize, NcStatus> {
        let frames = self.validate_samples(offset, bytes)?;
        let samples = frames * self.channels.count() as usize;
        if output.len() < samples {
            return Err(NC_E_INVALID_ARG);
        }
        for (destination, source) in output[..samples].iter_mut().zip(bytes.chunks_exact(self.format.sample_bytes()?)) {
            *destination = self.format.decode(source)?;
        }
        Ok(samples)
    }
}

impl IAudioPlaybackClient {
    /// Enqueue an ordinary Rust f32 slice using the stream's requested integer/fixed/float encoding.
    pub fn write_f32(&mut self, samples: &[f32]) -> Result<(), NcStatus> {
        let bytes = self.info()?.config.encode(samples)?;
        self.write(neocogi_com::NcCopyFromBufferOffset::new(0, &bytes))
    }
}
impl IAudioCaptureClient {
    /// Read and decode up to the requested complete frames into owned Rust f32 samples without raw casts.
    pub fn read_f32(&mut self, frames: usize) -> Result<Vec<f32>, NcStatus> {
        let config = self.info()?.config;
        let stride = config
            .format
            .sample_bytes()?
            .checked_mul(config.channels.count() as usize)
            .ok_or(AUDIO_E_LIMIT)?;
        let capacity = frames.checked_mul(stride).ok_or(AUDIO_E_LIMIT)?;
        config.transfer_frames(0, capacity)?;
        let mut output = NcOutputBuffer::new();
        let read = self.read(output.output(0, capacity))? as usize;
        if read > frames || output.len() != read * stride {
            return Err(NC_E_INVALID_ARG);
        }
        let count = read * config.channels.count() as usize;
        let mut samples = Vec::new();
        samples.try_reserve_exact(count).map_err(|_| AUDIO_E_LIMIT)?;
        samples.resize(count, 0.);
        config.decode(0, output.as_slice(), &mut samples)?;
        Ok(samples)
    }
}
impl PcmType {
    /// Storage width belongs to the selected numerical variant.
    pub const fn bits(self) -> u8 {
        match self {
            Self::UnsignedInteger(bits) | Self::SignedInteger(bits) | Self::FixedPoint(bits, _) | Self::FloatingPoint(bits) => bits,
        }
    }
    /// Validate parameters within their actual variant, without irrelevant scale fields on integer/float types.
    pub fn validate(self) -> Result<(), NcStatus> {
        let integer = matches!(self.bits(), 8 | 16 | 24 | 32);
        let valid = match self {
            Self::UnsignedInteger(_) | Self::SignedInteger(_) => integer,
            Self::FixedPoint(bits, fraction) => integer && fraction < bits,
            Self::FloatingPoint(bits) => matches!(bits, 32 | 64),
        };
        if valid { Ok(()) } else { Err(NC_E_INVALID_ARG) }
    }
}
impl PcmFormat {
    /// IEEE binary32 encoded least-significant byte first.
    pub const fn float32() -> Self {
        Self {
            sample_type: PcmType::FloatingPoint(32),
            byte_order: PcmByteOrder::LittleEndian,
        }
    }
    /// Validate the exact encoding and byte order as one descriptor.
    pub fn validate(self) -> Result<(), NcStatus> {
        self.sample_type.validate()?;
        if self.byte_order == PcmByteOrder::LittleEndian || self.byte_order == PcmByteOrder::BigEndian {
            Ok(())
        } else {
            Err(NC_E_INVALID_ARG)
        }
    }
    /// Packed sample storage without padding, including three-byte 24-bit PCM.
    pub fn sample_bytes(self) -> Result<usize, NcStatus> {
        self.validate()?;
        Ok(self.sample_type.bits() as usize / 8)
    }
    /// Decode one complete finite normalized sample at the backend's float32 processing precision.
    pub fn decode(self, bytes: &[u8]) -> Result<f32, NcStatus> {
        let size = self.sample_bytes()?;
        if bytes.len() != size {
            return Err(NC_E_INVALID_ARG);
        }
        let mut raw = 0u64;
        for (index, byte) in bytes.iter().enumerate() {
            let shift = if self.byte_order == PcmByteOrder::LittleEndian {
                index * 8
            } else {
                (size - index - 1) * 8
            };
            raw |= (*byte as u64) << shift;
        }
        let value = match self.sample_type {
            PcmType::UnsignedInteger(bits) => {
                let midpoint = (1u64 << (bits - 1)) as f64;
                (raw as f64 - midpoint) / midpoint
            }
            PcmType::SignedInteger(bits) | PcmType::FixedPoint(bits, _) => {
                let signed = ((raw << (64 - bits)) as i64) >> (64 - bits);
                let fraction = match self.sample_type {
                    PcmType::FixedPoint(_, fraction) => fraction,
                    _ => bits - 1,
                };
                signed as f64 / (1u64 << fraction) as f64
            }
            PcmType::FloatingPoint(32) => f32::from_bits(raw as u32) as f64,
            PcmType::FloatingPoint(64) => f64::from_bits(raw),
            PcmType::FloatingPoint(_) => return Err(NC_E_INVALID_ARG),
        };
        if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(value as f32)
    }
    /// Encode normalized processing PCM, rounding integer/fixed-point values and saturating positive endpoints.
    /// Exactly sample_bytes() of the result belongs to the selected representation.
    pub fn encode(self, sample: f32) -> Result<[u8; 8], NcStatus> {
        let size = self.sample_bytes()?;
        if !sample.is_finite() || !(-1.0..=1.0).contains(&sample) {
            return Err(NC_E_INVALID_ARG);
        }
        let raw = match self.sample_type {
            PcmType::UnsignedInteger(bits) => {
                let midpoint = (1u64 << (bits - 1)) as f64;
                (sample as f64 * midpoint + midpoint).round().clamp(0., (1u64 << bits) as f64 - 1.) as u64
            }
            PcmType::SignedInteger(bits) | PcmType::FixedPoint(bits, _) => {
                let fraction = match self.sample_type {
                    PcmType::FixedPoint(_, fraction) => fraction,
                    _ => bits - 1,
                };
                let midpoint = (1u64 << (bits - 1)) as f64;
                let signed = (sample as f64 * (1u64 << fraction) as f64).round().clamp(-midpoint, midpoint - 1.) as i64;
                signed as u64 & ((1u64 << bits) - 1)
            }
            PcmType::FloatingPoint(32) => sample.to_bits() as u64,
            PcmType::FloatingPoint(64) => (sample as f64).to_bits(),
            PcmType::FloatingPoint(_) => return Err(NC_E_INVALID_ARG),
        };
        let mut bytes = [0; 8];
        for (index, byte) in bytes[..size].iter_mut().enumerate() {
            let shift = if self.byte_order == PcmByteOrder::LittleEndian {
                index * 8
            } else {
                (size - index - 1) * 8
            };
            *byte = (raw >> shift) as u8;
        }
        Ok(bytes)
    }
}
impl AudioDirection {
    /// Reject unknown forward raw ABI values before opening a device or asking permission.
    pub fn validate(self) -> Result<(), NcStatus> {
        if self == Self::Playback || self == Self::Capture {
            Ok(())
        } else {
            Err(NC_E_INVALID_ARG)
        }
    }
}

/// One audio operation's independently owned status and bounded UTF-8 explanation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioError {
    /// Original failing status; successful error values normalize to InvalidArg.
    pub status: NcStatus,
    /// Copied explanation, independent of native/request/plugin lifetimes.
    pub message: String,
}
impl AudioError {
    /// Preserve scalar boundaries while bounding diagnostic allocation/retention.
    pub fn new(status: NcStatus, message: impl Into<String>) -> Self {
        let mut message = message.into();
        let status = if status.is_err() { status } else { NC_E_INVALID_ARG };
        let mut end = message.len().min(MAX_AUDIO_ERROR_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        Self { status, message }
    }
}
impl From<NcStatus> for AudioError {
    fn from(status: NcStatus) -> Self {
        Self::new(status, status.to_string())
    }
}
impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status, self.message)
    }
}
impl std::error::Error for AudioError {}

#[cfg(test)]
mod tests {
    use super::*;
    /// Admission rejects bad parameters and malformed floating PCM before modifying conversion storage.
    #[test]
    fn config_and_pcm_validation() {
        let config = AudioConfig::stereo();
        for rate in [0, 7999, 192001, u32::MAX] {
            assert!(AudioConfig { sample_rate: rate, ..config }.validate().is_err());
        }
        for capacity in [0, 127, 65537, u32::MAX] {
            assert!(AudioConfig { capacity_frames: capacity, ..config }.validate().is_err());
        }
        for rate in [8000, 48000, 192000] {
            AudioConfig { sample_rate: rate, ..config }.validate().unwrap();
        }
        for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.01, 1.01] {
            let bytes: Vec<_> = [0f32, sample].iter().flat_map(|sample| sample.to_le_bytes()).collect();
            let mut destination = [0.25; 2];
            assert_eq!(config.decode(0, &bytes, &mut destination), Err(NC_E_INVALID_ARG));
            assert_eq!(destination, [0.25; 2]);
        }
        assert_eq!(config.transfer_frames(1, 8), Err(NC_E_INVALID_ARG));
        assert_eq!(config.transfer_frames(0, 3), Err(NC_E_INVALID_ARG));
        assert_eq!(config.transfer_frames(0, 32776), Err(AUDIO_E_LIMIT));
        assert_eq!(config.transfer_frames(0, 32768), Ok(4096));
        assert_eq!(config.transfer_frames(0, 0), Ok(0));
    }
    /// Exhaustive supported integer/fixed widths and both IEEE widths have explicit byte-order semantics.
    #[test]
    fn pcm_variants_and_byte_orders() {
        let mut kinds = vec![PcmType::FloatingPoint(32), PcmType::FloatingPoint(64)];
        for bits in [8, 16, 24, 32] {
            kinds.extend([PcmType::UnsignedInteger(bits), PcmType::SignedInteger(bits)]);
            for fraction in 0..bits {
                kinds.push(PcmType::FixedPoint(bits, fraction));
            }
        }
        for byte_order in [PcmByteOrder::LittleEndian, PcmByteOrder::BigEndian] {
            for sample_type in &kinds {
                let format = PcmFormat { sample_type: *sample_type, byte_order };
                let size = format.sample_bytes().unwrap();
                for sample in [-1., 0., 1.] {
                    let encoded = format.encode(sample).unwrap();
                    let decoded = format.decode(&encoded[..size]).unwrap();
                    assert!((-1.0..=1.0).contains(&decoded));
                    if sample <= 0. {
                        assert_eq!(decoded, sample);
                    }
                }
            }
        }
        let signed = PcmFormat {
            sample_type: PcmType::SignedInteger(16),
            byte_order: PcmByteOrder::LittleEndian,
        };
        assert_eq!(&signed.encode(-1.).unwrap()[..2], &[0, 128]);
        assert_eq!(
            PcmFormat {
                byte_order: PcmByteOrder::BigEndian,
                ..signed
            }
            .decode(&[128, 0]),
            Ok(-1.)
        );
        let fixed = PcmFormat {
            sample_type: PcmType::FixedPoint(32, 16),
            ..signed
        };
        assert_eq!(&fixed.encode(0.5).unwrap()[..4], &[0, 128, 0, 0]);
        assert_eq!(fixed.decode(&[0, 0, 2, 0]), Err(NC_E_INVALID_ARG));
        for kind in [
            PcmType::FloatingPoint(16),
            PcmType::SignedInteger(64),
            PcmType::UnsignedInteger(0),
            PcmType::FixedPoint(16, 16),
        ] {
            assert_eq!(kind.validate(), Err(NC_E_INVALID_ARG));
        }
        let unsigned = PcmFormat {
            sample_type: PcmType::UnsignedInteger(8),
            ..signed
        };
        assert_eq!(unsigned.encode(0.).unwrap()[0], 128);
    }
    /// Error ownership never borrows native/global storage or truncates half a UTF-8 scalar.
    #[test]
    fn owned_diagnostics_are_bounded() {
        assert_eq!(AudioError::new(AUDIO_E_BACKEND, "é".repeat(4097)).message.len(), MAX_AUDIO_ERROR_BYTES);
        assert_eq!(AudioError::new(neocogi_com::NC_OK, "bad status").status, NC_E_INVALID_ARG);
    }
}
