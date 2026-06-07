use lxst_core::{CodecKind, CodecProfile, RawBitDepth, RawFrameHeader};

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

#[derive(Debug, Clone)]
pub struct OpusCodec {
    profile: CodecProfile,
}

impl OpusCodec {
    pub fn new(profile: CodecProfile) -> Self {
        Self { profile }
    }

    pub fn profile(&self) -> CodecProfile {
        self.profile
    }
}

impl AudioCodec for OpusCodec {
    fn kind(&self) -> CodecKind {
        CodecKind::Opus
    }

    fn encode(&mut self, _frame: &AudioFrame) -> Result<Vec<u8>, CodecError> {
        Err(CodecError::Unsupported(
            "Opus codec binding is not wired in this milestone".to_string(),
        ))
    }

    fn decode(&mut self, _data: &[u8], _samplerate: u32) -> Result<AudioFrame, CodecError> {
        Err(CodecError::Unsupported(
            "Opus codec binding is not wired in this milestone".to_string(),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct Codec2Codec {
    profile: CodecProfile,
}

impl Codec2Codec {
    pub fn new(profile: CodecProfile) -> Self {
        Self { profile }
    }

    pub fn profile(&self) -> CodecProfile {
        self.profile
    }
}

impl AudioCodec for Codec2Codec {
    fn kind(&self) -> CodecKind {
        CodecKind::Codec2
    }

    fn encode(&mut self, _frame: &AudioFrame) -> Result<Vec<u8>, CodecError> {
        Err(CodecError::Unsupported(
            "Codec2 FFI binding requires a later codec2 feature milestone".to_string(),
        ))
    }

    fn decode(&mut self, _data: &[u8], _samplerate: u32) -> Result<AudioFrame, CodecError> {
        Err(CodecError::Unsupported(
            "Codec2 FFI binding requires a later codec2 feature milestone".to_string(),
        ))
    }
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
    #[error("unsupported codec operation: {0}")]
    Unsupported(String),
}
