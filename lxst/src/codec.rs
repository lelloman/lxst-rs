use codec2::{Codec2 as InnerCodec2, Codec2Mode};
use libloading::Library;
use lxst_core::{
    CodecKind, CodecProfile, CodecProfileInfo, OpusApplication, RawBitDepth, RawFrameHeader,
};
use opus::{Application, Bitrate, Channels, Decoder, Encoder};
use std::ffi::{c_int, c_void};
use std::ptr::NonNull;

use crate::audio::{AudioError, AudioFrame};

pub trait AudioCodec: Send {
    fn kind(&self) -> CodecKind;
    fn encode(&mut self, frame: &AudioFrame) -> Result<Vec<u8>, CodecError>;
    fn decode(&mut self, data: &[u8], samplerate: u32) -> Result<AudioFrame, CodecError>;
}

#[derive(Debug, Default, Clone)]
pub struct NullCodec;

impl AudioCodec for NullCodec {
    fn kind(&self) -> CodecKind {
        CodecKind::Null
    }

    fn encode(&mut self, frame: &AudioFrame) -> Result<Vec<u8>, CodecError> {
        let mut raw = RawCodec::new(RawBitDepth::Float32);
        raw.encode(frame)
    }

    fn decode(&mut self, data: &[u8], samplerate: u32) -> Result<AudioFrame, CodecError> {
        let mut raw = RawCodec::new(RawBitDepth::Float32);
        raw.decode(data, samplerate)
    }
}

#[derive(Debug, Clone)]
pub struct RawCodec {
    bit_depth: RawBitDepth,
    channels: Option<u8>,
}

impl RawCodec {
    pub fn new(bit_depth: RawBitDepth) -> Self {
        Self {
            bit_depth,
            channels: None,
        }
    }

    pub fn channels(&self) -> Option<u8> {
        self.channels
    }
}

impl Default for RawCodec {
    fn default() -> Self {
        Self::new(RawBitDepth::Float32)
    }
}

impl AudioCodec for RawCodec {
    fn kind(&self) -> CodecKind {
        CodecKind::Raw
    }

    fn encode(&mut self, frame: &AudioFrame) -> Result<Vec<u8>, CodecError> {
        self.channels = Some(frame.channels());
        let header = RawFrameHeader::new(self.bit_depth, frame.channels())?;
        let mut bytes = Vec::with_capacity(1 + frame.samples().len() * 4);
        bytes.push(header.encode());
        match self.bit_depth {
            RawBitDepth::Float32 => {
                for sample in frame.samples() {
                    bytes.extend_from_slice(&sample.to_le_bytes());
                }
            }
            RawBitDepth::Float64 => {
                for sample in frame.samples() {
                    bytes.extend_from_slice(&(*sample as f64).to_le_bytes());
                }
            }
            RawBitDepth::Float16 | RawBitDepth::Float128 => {
                return Err(CodecError::Unsupported(format!(
                    "raw {}-bit samples are not implemented",
                    self.bit_depth.bits()
                )));
            }
        }
        Ok(bytes)
    }

    fn decode(&mut self, data: &[u8], samplerate: u32) -> Result<AudioFrame, CodecError> {
        let (&header, payload) = data.split_first().ok_or(CodecError::EmptyFrame)?;
        let header = RawFrameHeader::decode(header)?;
        self.channels = Some(header.channels);

        let samples = match header.bit_depth {
            RawBitDepth::Float32 => {
                if payload.len() % 4 != 0 {
                    return Err(CodecError::InvalidPayloadLength(payload.len()));
                }
                payload
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
                    .collect()
            }
            RawBitDepth::Float64 => {
                if payload.len() % 8 != 0 {
                    return Err(CodecError::InvalidPayloadLength(payload.len()));
                }
                payload
                    .chunks_exact(8)
                    .map(|chunk| f64::from_le_bytes(chunk.try_into().unwrap()) as f32)
                    .collect()
            }
            RawBitDepth::Float16 | RawBitDepth::Float128 => {
                return Err(CodecError::Unsupported(format!(
                    "raw {}-bit samples are not implemented",
                    header.bit_depth.bits()
                )));
            }
        };

        Ok(AudioFrame::new(samplerate, header.channels, samples)?)
    }
}

#[derive(Debug)]
pub struct OpusCodec {
    profile: CodecProfile,
    encoder: Option<Encoder>,
    decoder: Option<Decoder>,
}

impl OpusCodec {
    pub fn new(profile: CodecProfile) -> Self {
        Self {
            profile,
            encoder: None,
            decoder: None,
        }
    }

    pub fn profile(&self) -> CodecProfile {
        self.profile
    }

    pub fn info(&self) -> Result<CodecProfileInfo, CodecError> {
        Self::profile_info(self.profile)
    }

    pub fn profile_info(profile: CodecProfile) -> Result<CodecProfileInfo, CodecError> {
        let info = profile.info();
        if info.opus_application.is_none() {
            return Err(CodecError::InvalidProfile(format!(
                "{profile:?} is not an Opus profile"
            )));
        }
        Ok(info)
    }

    pub fn profile_channels(profile: CodecProfile) -> Result<u8, CodecError> {
        Ok(Self::profile_info(profile)?.channels)
    }

    pub fn profile_samplerate(profile: CodecProfile) -> Result<u32, CodecError> {
        Ok(Self::profile_info(profile)?.samplerate)
    }

    pub fn profile_application(profile: CodecProfile) -> Result<OpusApplication, CodecError> {
        Ok(Self::profile_info(profile)?.opus_application.unwrap())
    }

    pub fn profile_bitrate_ceiling(profile: CodecProfile) -> Result<u32, CodecError> {
        Ok(Self::profile_info(profile)?.bitrate_ceiling)
    }

    pub fn max_bytes_per_frame(bitrate_ceiling: u32, frame_duration_ms: f32) -> usize {
        ((bitrate_ceiling as f32 / 8.0) * (frame_duration_ms / 1000.0)).ceil() as usize
    }

    fn encoder(&mut self, info: CodecProfileInfo) -> Result<&mut Encoder, CodecError> {
        if self.encoder.is_none() {
            let channels = opus_channels(info.channels)?;
            let application = opus_application(info.opus_application.unwrap())?;
            let mut encoder = Encoder::new(info.samplerate, channels, application)?;
            encoder.set_bitrate(Bitrate::Bits(info.bitrate_ceiling as i32))?;
            encoder.set_vbr(true)?;
            self.encoder = Some(encoder);
        }
        Ok(self.encoder.as_mut().unwrap())
    }

    fn decoder(&mut self, samplerate: u32, channels: u8) -> Result<&mut Decoder, CodecError> {
        if self.decoder.is_none() {
            self.decoder = Some(Decoder::new(samplerate, opus_channels(channels)?)?);
        }
        Ok(self.decoder.as_mut().unwrap())
    }
}

impl AudioCodec for OpusCodec {
    fn kind(&self) -> CodecKind {
        CodecKind::Opus
    }

    fn encode(&mut self, frame: &AudioFrame) -> Result<Vec<u8>, CodecError> {
        let info = self.info()?;
        let normalized = normalize_channels(frame.samples(), frame.channels(), info.channels)?;
        let resampled = resample_linear(
            &normalized,
            info.channels,
            frame.samplerate(),
            info.samplerate,
        )?;
        let sample_count = resampled.len() / info.channels as usize;
        let frame_duration_tenths = valid_opus_frame_duration_tenths(sample_count, info.samplerate)
            .ok_or(CodecError::InvalidFrameDuration {
                sample_count,
                samplerate: info.samplerate,
            })?;
        let max_frame_bytes =
            max_bytes_per_frame_tenths(info.bitrate_ceiling, frame_duration_tenths).max(1);
        let input: Vec<i16> = resampled
            .into_iter()
            .map(|sample| (sample.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        self.encoder(info)?
            .encode_vec(&input, max_frame_bytes)
            .map_err(Into::into)
    }

    fn decode(&mut self, data: &[u8], samplerate: u32) -> Result<AudioFrame, CodecError> {
        if data.is_empty() {
            return Err(CodecError::EmptyFrame);
        }
        let info = self.info()?;
        let samplerate = if samplerate == 0 {
            info.samplerate
        } else {
            samplerate
        };
        validate_opus_samplerate(samplerate)?;
        let channels = info.channels;
        let decoder = self.decoder(samplerate, channels)?;
        let samples_per_channel = decoder.get_nb_samples(data)?;
        let mut output = vec![0i16; samples_per_channel * channels as usize];
        let decoded = decoder.decode(data, &mut output, false)?;
        output.truncate(decoded * channels as usize);
        let samples = output
            .into_iter()
            .map(|sample| sample as f32 / 32767.0)
            .collect();
        Ok(AudioFrame::new(samplerate, channels, samples)?)
    }
}

#[derive(Debug)]
pub struct Codec2Codec {
    profile: CodecProfile,
    codec: Option<InnerCodec2>,
    mode: Option<Codec2Mode>,
    system_codec: Option<SystemCodec2>,
    system_mode: Option<c_int>,
}

impl Codec2Codec {
    pub fn new(profile: CodecProfile) -> Self {
        Self {
            profile,
            codec: None,
            mode: None,
            system_codec: None,
            system_mode: None,
        }
    }

    pub fn profile(&self) -> CodecProfile {
        self.profile
    }

    fn codec(&mut self, mode: Codec2Mode) -> &mut InnerCodec2 {
        if !matches!(self.mode, Some(current) if codec2_modes_equal(current, mode)) {
            self.codec = Some(InnerCodec2::new(mode));
            self.mode = Some(mode);
        }
        self.codec.as_mut().unwrap()
    }

    fn system_codec(&mut self, mode: c_int) -> Result<&mut SystemCodec2, CodecError> {
        if self.system_mode != Some(mode) {
            self.system_codec = Some(SystemCodec2::new(mode)?);
            self.system_mode = Some(mode);
        }
        Ok(self.system_codec.as_mut().unwrap())
    }

    fn encode_system(
        &mut self,
        frame: &AudioFrame,
        mode_header: u8,
        mode: c_int,
    ) -> Result<Vec<u8>, CodecError> {
        let normalized = normalize_channels(frame.samples(), frame.channels(), 1)?;
        let resampled = resample_linear(&normalized, 1, frame.samplerate(), 8_000)?;
        let input: Vec<i16> = resampled
            .into_iter()
            .map(|sample| (sample.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        let codec = self.system_codec(mode)?;
        let samples_per_frame = codec.samples_per_frame();
        let bytes_per_frame = codec.bits_per_frame().div_ceil(8);
        let frame_count = input.len() / samples_per_frame;
        let mut encoded = Vec::with_capacity(1 + frame_count * bytes_per_frame);
        encoded.push(mode_header);
        for frame in input.chunks_exact(samples_per_frame) {
            let mut output = vec![0u8; bytes_per_frame];
            codec.encode(&mut output, frame);
            encoded.extend_from_slice(&output);
        }
        Ok(encoded)
    }

    fn decode_system(
        &mut self,
        payload: &[u8],
        samplerate: u32,
        mode: c_int,
    ) -> Result<AudioFrame, CodecError> {
        let codec = self.system_codec(mode)?;
        let samples_per_frame = codec.samples_per_frame();
        let bytes_per_frame = codec.bits_per_frame().div_ceil(8);
        let frame_count = payload.len() / bytes_per_frame;
        let mut decoded = Vec::with_capacity(frame_count * samples_per_frame);
        for encoded_frame in payload.chunks_exact(bytes_per_frame) {
            let mut output = vec![0i16; samples_per_frame];
            codec.decode(&mut output, encoded_frame);
            decoded.extend(output);
        }
        let samples: Vec<f32> = decoded
            .into_iter()
            .map(|sample| sample as f32 / 32767.0)
            .collect();
        let samplerate = if samplerate == 0 { 8_000 } else { samplerate };
        let samples = resample_linear(&samples, 1, 8_000, samplerate)?;
        Ok(AudioFrame::new(samplerate, 1, samples)?)
    }
}

impl AudioCodec for Codec2Codec {
    fn kind(&self) -> CodecKind {
        CodecKind::Codec2
    }

    fn encode(&mut self, frame: &AudioFrame) -> Result<Vec<u8>, CodecError> {
        if self.profile == CodecProfile::Codec2_700C {
            return self.encode_system(frame, 0x00, CODEC2_MODE_700C);
        }
        let mode = codec2_mode(self.profile)?;
        let mode_header = codec2_mode_header(self.profile)?;
        let normalized = normalize_channels(frame.samples(), frame.channels(), 1)?;
        let resampled = resample_linear(&normalized, 1, frame.samplerate(), 8_000)?;
        let input: Vec<i16> = resampled
            .into_iter()
            .map(|sample| (sample.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        let codec = self.codec(mode);
        let samples_per_frame = codec.samples_per_frame();
        let bytes_per_frame = codec.bits_per_frame().div_ceil(8);
        let frame_count = input.len() / samples_per_frame;
        let mut encoded = Vec::with_capacity(1 + frame_count * bytes_per_frame);
        encoded.push(mode_header);
        for frame in input.chunks_exact(samples_per_frame) {
            let mut output = vec![0u8; bytes_per_frame];
            codec.encode(&mut output, frame);
            encoded.extend_from_slice(&output);
        }
        Ok(encoded)
    }

    fn decode(&mut self, data: &[u8], samplerate: u32) -> Result<AudioFrame, CodecError> {
        let (&mode_header, payload) = data.split_first().ok_or(CodecError::EmptyFrame)?;
        if mode_header == 0x00 {
            return self.decode_system(payload, samplerate, CODEC2_MODE_700C);
        }
        let mode = codec2_mode_from_header(mode_header)?;
        let codec = self.codec(mode);
        let samples_per_frame = codec.samples_per_frame();
        let bytes_per_frame = codec.bits_per_frame().div_ceil(8);
        let frame_count = payload.len() / bytes_per_frame;
        let mut decoded = Vec::with_capacity(frame_count * samples_per_frame);
        for encoded_frame in payload.chunks_exact(bytes_per_frame) {
            let mut output = vec![0i16; samples_per_frame];
            codec.decode(&mut output, encoded_frame);
            decoded.extend(output);
        }
        let samples: Vec<f32> = decoded
            .into_iter()
            .map(|sample| sample as f32 / 32767.0)
            .collect();
        let samplerate = if samplerate == 0 { 8_000 } else { samplerate };
        let samples = resample_linear(&samples, 1, 8_000, samplerate)?;
        Ok(AudioFrame::new(samplerate, 1, samples)?)
    }
}

type Codec2Create = unsafe extern "C" fn(c_int) -> *mut c_void;
type Codec2Destroy = unsafe extern "C" fn(*mut c_void);
type Codec2Encode = unsafe extern "C" fn(*mut c_void, *mut u8, *const i16);
type Codec2Decode = unsafe extern "C" fn(*mut c_void, *mut i16, *const u8);
type Codec2SamplesPerFrame = unsafe extern "C" fn(*mut c_void) -> c_int;
type Codec2BitsPerFrame = unsafe extern "C" fn(*mut c_void) -> c_int;

const CODEC2_MODE_700C: c_int = 8;

#[derive(Debug)]
struct SystemCodec2 {
    _library: Library,
    state: NonNull<c_void>,
    destroy: Codec2Destroy,
    encode: Codec2Encode,
    decode: Codec2Decode,
    samples_per_frame: Codec2SamplesPerFrame,
    bits_per_frame: Codec2BitsPerFrame,
}

impl SystemCodec2 {
    fn new(mode: c_int) -> Result<Self, CodecError> {
        let library = load_codec2_library()?;
        unsafe {
            let create = *library
                .get::<Codec2Create>(b"codec2_create\0")
                .map_err(system_codec2_error)?;
            let destroy = *library
                .get::<Codec2Destroy>(b"codec2_destroy\0")
                .map_err(system_codec2_error)?;
            let encode = *library
                .get::<Codec2Encode>(b"codec2_encode\0")
                .map_err(system_codec2_error)?;
            let decode = *library
                .get::<Codec2Decode>(b"codec2_decode\0")
                .map_err(system_codec2_error)?;
            let samples_per_frame = *library
                .get::<Codec2SamplesPerFrame>(b"codec2_samples_per_frame\0")
                .map_err(system_codec2_error)?;
            let bits_per_frame = *library
                .get::<Codec2BitsPerFrame>(b"codec2_bits_per_frame\0")
                .map_err(system_codec2_error)?;
            let state = NonNull::new(create(mode)).ok_or_else(|| {
                CodecError::Unsupported(format!("system libcodec2 rejected mode {mode}"))
            })?;
            Ok(Self {
                _library: library,
                state,
                destroy,
                encode,
                decode,
                samples_per_frame,
                bits_per_frame,
            })
        }
    }

    fn samples_per_frame(&self) -> usize {
        unsafe { (self.samples_per_frame)(self.state.as_ptr()) as usize }
    }

    fn bits_per_frame(&self) -> usize {
        unsafe { (self.bits_per_frame)(self.state.as_ptr()) as usize }
    }

    fn encode(&mut self, output: &mut [u8], input: &[i16]) {
        unsafe {
            (self.encode)(self.state.as_ptr(), output.as_mut_ptr(), input.as_ptr());
        }
    }

    fn decode(&mut self, output: &mut [i16], input: &[u8]) {
        unsafe {
            (self.decode)(self.state.as_ptr(), output.as_mut_ptr(), input.as_ptr());
        }
    }
}

impl Drop for SystemCodec2 {
    fn drop(&mut self) {
        unsafe {
            (self.destroy)(self.state.as_ptr());
        }
    }
}

unsafe impl Send for SystemCodec2 {}

fn load_codec2_library() -> Result<Library, CodecError> {
    for name in ["libcodec2.so.1.2", "libcodec2.so"] {
        match unsafe { Library::new(name) } {
            Ok(library) => return Ok(library),
            Err(_) => continue,
        }
    }
    Err(CodecError::Unsupported(
        "Codec2 700C requires a runtime-loadable system libcodec2".to_string(),
    ))
}

fn system_codec2_error(error: libloading::Error) -> CodecError {
    CodecError::Unsupported(format!("system libcodec2 error: {error}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecSelection {
    Null,
    Raw(RawBitDepth),
    Profile(CodecProfile),
}

#[derive(Debug, Default)]
pub struct CodecFactory;

impl CodecFactory {
    pub fn create(selection: CodecSelection) -> Box<dyn AudioCodec> {
        match selection {
            CodecSelection::Null => Box::new(NullCodec),
            CodecSelection::Raw(bit_depth) => Box::new(RawCodec::new(bit_depth)),
            CodecSelection::Profile(profile) => match profile {
                CodecProfile::Raw => Box::new(RawCodec::default()),
                CodecProfile::Codec2_700C
                | CodecProfile::Codec2_1600
                | CodecProfile::Codec2_3200 => Box::new(Codec2Codec::new(profile)),
                _ => Box::new(OpusCodec::new(profile)),
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CodecState {
    pub active_kind: Option<CodecKind>,
    pub channels: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CodecError {
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error(transparent)]
    RawHeader(#[from] lxst_core::codec::RawFrameHeaderError),
    #[error("codec frame is empty")]
    EmptyFrame,
    #[error("invalid codec payload length {0}")]
    InvalidPayloadLength(usize),
    #[error("invalid codec profile: {0}")]
    InvalidProfile(String),
    #[error("invalid Opus samplerate {0}")]
    InvalidOpusSamplerate(u32),
    #[error("invalid Opus channel count {0}")]
    InvalidOpusChannels(u8),
    #[error("invalid Opus frame duration: {sample_count} samples at {samplerate} Hz")]
    InvalidFrameDuration {
        sample_count: usize,
        samplerate: u32,
    },
    #[error("invalid Codec2 mode header 0x{0:02x}")]
    InvalidCodec2ModeHeader(u8),
    #[error("opus codec error: {0}")]
    Opus(String),
    #[error("unsupported codec operation: {0}")]
    Unsupported(String),
}

impl From<opus::Error> for CodecError {
    fn from(value: opus::Error) -> Self {
        Self::Opus(value.to_string())
    }
}

fn opus_application(application: OpusApplication) -> Result<Application, CodecError> {
    match application {
        OpusApplication::Voip => Ok(Application::Voip),
        OpusApplication::Audio => Ok(Application::Audio),
    }
}

fn opus_channels(channels: u8) -> Result<Channels, CodecError> {
    match channels {
        1 => Ok(Channels::Mono),
        2 => Ok(Channels::Stereo),
        other => Err(CodecError::InvalidOpusChannels(other)),
    }
}

fn validate_opus_samplerate(samplerate: u32) -> Result<(), CodecError> {
    match samplerate {
        8_000 | 12_000 | 16_000 | 24_000 | 48_000 => Ok(()),
        other => Err(CodecError::InvalidOpusSamplerate(other)),
    }
}

fn normalize_channels(
    samples: &[f32],
    input_channels: u8,
    output_channels: u8,
) -> Result<Vec<f32>, CodecError> {
    if input_channels == 0 {
        return Err(CodecError::InvalidOpusChannels(input_channels));
    }
    opus_channels(output_channels)?;
    let input_channels = input_channels as usize;
    let output_channels = output_channels as usize;
    let frames = samples.len() / input_channels;
    let mut normalized = Vec::with_capacity(frames * output_channels);
    for frame in 0..frames {
        let base = frame * input_channels;
        for channel in 0..output_channels {
            let source_channel = channel.min(input_channels - 1);
            normalized.push(samples[base + source_channel]);
        }
    }
    Ok(normalized)
}

fn resample_linear(
    samples: &[f32],
    channels: u8,
    input_rate: u32,
    output_rate: u32,
) -> Result<Vec<f32>, CodecError> {
    if input_rate == 0 {
        return Err(CodecError::Audio(AudioError::InvalidSamplerate(input_rate)));
    }
    if output_rate == 0 {
        return Err(CodecError::Audio(AudioError::InvalidSamplerate(
            output_rate,
        )));
    }
    if input_rate == output_rate {
        return Ok(samples.to_vec());
    }
    let channels = channels as usize;
    if samples.is_empty() {
        return Ok(Vec::new());
    }
    let input_frames = samples.len() / channels;
    let output_frames =
        ((input_frames as u64 * output_rate as u64) + input_rate as u64 / 2) / input_rate as u64;
    let output_frames = output_frames.max(1) as usize;
    let mut out = Vec::with_capacity(output_frames * channels);
    for out_frame in 0..output_frames {
        let position = out_frame as f64 * input_rate as f64 / output_rate as f64;
        let left = position.floor() as usize;
        let right = (left + 1).min(input_frames - 1);
        let frac = (position - left as f64) as f32;
        for channel in 0..channels {
            let a = samples[left * channels + channel];
            let b = samples[right * channels + channel];
            out.push(a + (b - a) * frac);
        }
    }
    Ok(out)
}

fn valid_opus_frame_duration_tenths(sample_count: usize, samplerate: u32) -> Option<u32> {
    const VALID_TENTHS_MS: [u32; 6] = [25, 50, 100, 200, 400, 600];
    VALID_TENTHS_MS
        .into_iter()
        .find(|tenths| (samplerate as u64 * *tenths as u64) == sample_count as u64 * 10_000)
}

fn max_bytes_per_frame_tenths(bitrate_ceiling: u32, frame_duration_tenths_ms: u32) -> usize {
    ((bitrate_ceiling as u64 * frame_duration_tenths_ms as u64).div_ceil(80_000)) as usize
}

fn codec2_mode(profile: CodecProfile) -> Result<Codec2Mode, CodecError> {
    match profile {
        CodecProfile::Codec2_1600 => Ok(Codec2Mode::MODE_1600),
        CodecProfile::Codec2_3200 => Ok(Codec2Mode::MODE_3200),
        CodecProfile::Codec2_700C => Err(CodecError::Unsupported(
            "Codec2 700C is not implemented by the pure Rust codec2 backend".to_string(),
        )),
        other => Err(CodecError::InvalidProfile(format!(
            "{other:?} is not a Codec2 profile"
        ))),
    }
}

fn codec2_mode_header(profile: CodecProfile) -> Result<u8, CodecError> {
    match profile {
        CodecProfile::Codec2_700C => Ok(0x00),
        CodecProfile::Codec2_1600 => Ok(0x04),
        CodecProfile::Codec2_3200 => Ok(0x06),
        other => Err(CodecError::InvalidProfile(format!(
            "{other:?} is not a Codec2 profile"
        ))),
    }
}

fn codec2_mode_from_header(header: u8) -> Result<Codec2Mode, CodecError> {
    match header {
        0x04 => Ok(Codec2Mode::MODE_1600),
        0x06 => Ok(Codec2Mode::MODE_3200),
        0x00 => Err(CodecError::Unsupported(
            "Codec2 700C is not implemented by the pure Rust codec2 backend".to_string(),
        )),
        other => Err(CodecError::InvalidCodec2ModeHeader(other)),
    }
}

fn codec2_modes_equal(left: Codec2Mode, right: Codec2Mode) -> bool {
    matches!(
        (left, right),
        (Codec2Mode::MODE_1600, Codec2Mode::MODE_1600)
            | (Codec2Mode::MODE_3200, Codec2Mode::MODE_3200)
    )
}
