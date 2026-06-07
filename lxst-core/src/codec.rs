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

/// High-level codec profile identifiers used by the Python LXST telephony
/// primitive. These are separate from frame codec header bytes.
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
    Codec2_1600,
    Codec2_3200,
    Raw,
}
