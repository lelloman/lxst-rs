use serde::{Deserialize, Serialize};

use crate::codec::CodecProfile;

/// Telephony call profiles from `LXST.Primitives.Telephony.Profiles`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum CallProfile {
    UltraLowBandwidth = 0x10,
    VeryLowBandwidth = 0x20,
    LowBandwidth = 0x30,
    MediumQuality = 0x40,
    HighQuality = 0x50,
    MaxQuality = 0x60,
    UltraLowLatency = 0x70,
    LowLatency = 0x80,
}

impl CallProfile {
    pub const DEFAULT: Self = Self::MediumQuality;

    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn available_profiles() -> [Self; 8] {
        [
            Self::UltraLowBandwidth,
            Self::VeryLowBandwidth,
            Self::LowBandwidth,
            Self::MediumQuality,
            Self::HighQuality,
            Self::MaxQuality,
            Self::LowLatency,
            Self::UltraLowLatency,
        ]
    }

    pub fn codec_profile(self) -> CodecProfile {
        match self {
            Self::UltraLowBandwidth => CodecProfile::Codec2_700C,
            Self::VeryLowBandwidth => CodecProfile::Codec2_1600,
            Self::LowBandwidth => CodecProfile::Codec2_3200,
            Self::MediumQuality => CodecProfile::OpusVoiceMedium,
            Self::HighQuality => CodecProfile::OpusVoiceHigh,
            Self::MaxQuality => CodecProfile::OpusVoiceMax,
            Self::LowLatency | Self::UltraLowLatency => CodecProfile::OpusVoiceMedium,
        }
    }

    pub fn frame_duration(self) -> FrameDuration {
        match self {
            Self::UltraLowBandwidth => FrameDuration::from_millis(400),
            Self::VeryLowBandwidth => FrameDuration::from_millis(320),
            Self::LowBandwidth => FrameDuration::from_millis(200),
            Self::MediumQuality | Self::HighQuality | Self::MaxQuality => {
                FrameDuration::from_millis(60)
            }
            Self::LowLatency => FrameDuration::from_millis(20),
            Self::UltraLowLatency => FrameDuration::from_millis(10),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::UltraLowBandwidth => "Ultra Low Bandwidth",
            Self::VeryLowBandwidth => "Very Low Bandwidth",
            Self::LowBandwidth => "Low Bandwidth",
            Self::MediumQuality => "Medium Quality",
            Self::HighQuality => "High Quality",
            Self::MaxQuality => "Super High Quality",
            Self::LowLatency => "Low Latency",
            Self::UltraLowLatency => "Ultra Low Latency",
        }
    }

    pub fn abbreviation(self) -> &'static str {
        match self {
            Self::UltraLowBandwidth => "ULBW",
            Self::VeryLowBandwidth => "VLBW",
            Self::LowBandwidth => "LBW",
            Self::MediumQuality => "MQ",
            Self::HighQuality => "HQ",
            Self::MaxQuality => "SHQ",
            Self::LowLatency => "LL",
            Self::UltraLowLatency => "ULL",
        }
    }
}

impl TryFrom<u8> for CallProfile {
    type Error = CallProfileError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x10 => Ok(Self::UltraLowBandwidth),
            0x20 => Ok(Self::VeryLowBandwidth),
            0x30 => Ok(Self::LowBandwidth),
            0x40 => Ok(Self::MediumQuality),
            0x50 => Ok(Self::HighQuality),
            0x60 => Ok(Self::MaxQuality),
            0x70 => Ok(Self::UltraLowLatency),
            0x80 => Ok(Self::LowLatency),
            other => Err(CallProfileError(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown LXST telephony call profile 0x{0:02x}")]
pub struct CallProfileError(pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameDuration {
    millis: u16,
}

impl FrameDuration {
    pub const fn from_millis(millis: u16) -> Self {
        Self { millis }
    }

    pub const fn as_millis(self) -> u16 {
        self.millis
    }
}
