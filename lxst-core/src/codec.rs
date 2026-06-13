use serde::{Deserialize, Serialize};

/// Codec families supported by LXST frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CodecKind {
    Null,
    Raw,
    Opus,
    Codec2,
}

/// Codec header byte prepended to every LXST encoded frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum CodecHeader {
    Raw = 0x00,
    Opus = 0x01,
    Codec2 = 0x02,
    Null = 0xFF,
}

impl CodecHeader {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for CodecHeader {
    type Error = CodecHeaderError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Raw),
            0x01 => Ok(Self::Opus),
            0x02 => Ok(Self::Codec2),
            0xFF => Ok(Self::Null),
            other => Err(CodecHeaderError(other)),
        }
    }
}

impl From<CodecHeader> for CodecKind {
    fn from(value: CodecHeader) -> Self {
        match value {
            CodecHeader::Raw => Self::Raw,
            CodecHeader::Opus => Self::Opus,
            CodecHeader::Codec2 => Self::Codec2,
            CodecHeader::Null => Self::Null,
        }
    }
}

impl TryFrom<CodecKind> for CodecHeader {
    type Error = CodecHeaderError;

    fn try_from(value: CodecKind) -> Result<Self, Self::Error> {
        match value {
            CodecKind::Raw => Ok(Self::Raw),
            CodecKind::Opus => Ok(Self::Opus),
            CodecKind::Codec2 => Ok(Self::Codec2),
            CodecKind::Null => Ok(Self::Null),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown LXST codec header byte 0x{0:02x}")]
pub struct CodecHeaderError(pub u8);

/// High-level codec profile identifiers used by the Python LXST primitives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CodecProfile {
    OpusVoiceLow,
    OpusVoiceMedium,
    OpusVoiceHigh,
    OpusVoiceMax,
    OpusAudioMin,
    OpusAudioLow,
    OpusAudioMedium,
    OpusAudioHigh,
    OpusAudioMax,
    Codec2_700C,
    Codec2_1200,
    Codec2_1300,
    Codec2_1400,
    Codec2_1600,
    Codec2_2400,
    Codec2_3200,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OpusApplication {
    Voip,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CodecProfileInfo {
    pub profile: CodecProfile,
    pub channels: u8,
    pub samplerate: u32,
    pub bitrate_ceiling: u32,
    pub opus_application: Option<OpusApplication>,
}

impl CodecProfile {
    pub const fn info(self) -> CodecProfileInfo {
        match self {
            Self::OpusVoiceLow => {
                CodecProfileInfo::opus(self, 1, 8_000, 6_000, OpusApplication::Voip)
            }
            Self::OpusVoiceMedium => {
                CodecProfileInfo::opus(self, 1, 24_000, 8_000, OpusApplication::Voip)
            }
            Self::OpusVoiceHigh => {
                CodecProfileInfo::opus(self, 1, 48_000, 16_000, OpusApplication::Voip)
            }
            Self::OpusVoiceMax => {
                CodecProfileInfo::opus(self, 2, 48_000, 32_000, OpusApplication::Voip)
            }
            Self::OpusAudioMin => {
                CodecProfileInfo::opus(self, 1, 8_000, 8_000, OpusApplication::Audio)
            }
            Self::OpusAudioLow => {
                CodecProfileInfo::opus(self, 1, 12_000, 14_000, OpusApplication::Audio)
            }
            Self::OpusAudioMedium => {
                CodecProfileInfo::opus(self, 2, 24_000, 28_000, OpusApplication::Audio)
            }
            Self::OpusAudioHigh => {
                CodecProfileInfo::opus(self, 2, 48_000, 56_000, OpusApplication::Audio)
            }
            Self::OpusAudioMax => {
                CodecProfileInfo::opus(self, 2, 48_000, 128_000, OpusApplication::Audio)
            }
            Self::Codec2_700C
            | Self::Codec2_1200
            | Self::Codec2_1300
            | Self::Codec2_1400
            | Self::Codec2_1600
            | Self::Codec2_2400
            | Self::Codec2_3200 => CodecProfileInfo {
                profile: self,
                channels: 1,
                samplerate: 8_000,
                bitrate_ceiling: 0,
                opus_application: None,
            },
            Self::Raw => CodecProfileInfo {
                profile: self,
                channels: 0,
                samplerate: 0,
                bitrate_ceiling: 0,
                opus_application: None,
            },
        }
    }
}

impl CodecProfileInfo {
    const fn opus(
        profile: CodecProfile,
        channels: u8,
        samplerate: u32,
        bitrate_ceiling: u32,
        opus_application: OpusApplication,
    ) -> Self {
        Self {
            profile,
            channels,
            samplerate,
            bitrate_ceiling,
            opus_application: Some(opus_application),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RawBitDepth {
    Float16 = 0x00,
    Float32 = 0x01,
    Float64 = 0x02,
    Float128 = 0x03,
}

impl RawBitDepth {
    pub const fn bits(self) -> u16 {
        match self {
            Self::Float16 => 16,
            Self::Float32 => 32,
            Self::Float64 => 64,
            Self::Float128 => 128,
        }
    }
}

impl TryFrom<u8> for RawBitDepth {
    type Error = RawFrameHeaderError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Float16),
            0x01 => Ok(Self::Float32),
            0x02 => Ok(Self::Float64),
            0x03 => Ok(Self::Float128),
            other => Err(RawFrameHeaderError::InvalidBitDepth(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RawFrameHeader {
    pub bit_depth: RawBitDepth,
    pub channels: u8,
}

impl RawFrameHeader {
    pub fn new(bit_depth: RawBitDepth, channels: u8) -> Result<Self, RawFrameHeaderError> {
        if !(1..=32).contains(&channels) {
            return Err(RawFrameHeaderError::InvalidChannelCount(channels));
        }
        Ok(Self {
            bit_depth,
            channels,
        })
    }

    pub fn encode(self) -> u8 {
        ((self.bit_depth as u8) << 6) | (self.channels - 1)
    }

    pub fn decode(byte: u8) -> Result<Self, RawFrameHeaderError> {
        let bit_depth = RawBitDepth::try_from(byte >> 6)?;
        let channels = (byte & 0b0011_1111) + 1;
        Self::new(bit_depth, channels)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RawFrameHeaderError {
    #[error("invalid LXST raw bit depth header value {0}")]
    InvalidBitDepth(u8),
    #[error("invalid LXST raw channel count {0}; expected 1..=32")]
    InvalidChannelCount(u8),
}
