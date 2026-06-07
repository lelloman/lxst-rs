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
    LowLatency = 0x70,
    UltraLowLatency = 0x80,
}

impl CallProfile {
    pub const DEFAULT: Self = Self::MediumQuality;

    pub fn codec_profile(self) -> CodecProfile {
        match self {
            Self::UltraLowBandwidth => CodecProfile::Codec2_700C,
            Self::VeryLowBandwidth => CodecProfile::Codec2_1600,
            Self::LowBandwidth => CodecProfile::Codec2_3200,
            Self::MediumQuality => CodecProfile::OpusVoiceMedium,
            Self::HighQuality => CodecProfile::OpusVoiceHigh,
            Self::MaxQuality => CodecProfile::OpusVoiceMax,
            Self::LowLatency => CodecProfile::OpusVoiceMedium,
            Self::UltraLowLatency => CodecProfile::OpusVoiceMedium,
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
}

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
