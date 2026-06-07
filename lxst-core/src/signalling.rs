use serde::{Deserialize, Serialize};

/// In-band signalling field used in LXST network packets.
pub const FIELD_SIGNALLING: u8 = 0x00;

/// Frame field used in LXST network packets.
pub const FIELD_FRAMES: u8 = 0x01;

/// Telephony signalling codes from the Python LXST primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SignalCode {
    Busy = 0x00,
    Rejected = 0x01,
    Calling = 0x02,
    Available = 0x03,
    Ringing = 0x04,
    Connecting = 0x05,
    Established = 0x06,
}

/// A parsed signalling value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Signal {
    Code(SignalCode),
    PreferredProfile(u8),
    Unknown(u8),
}

impl From<u8> for Signal {
    fn from(value: u8) -> Self {
        match value {
            0x00 => Self::Code(SignalCode::Busy),
            0x01 => Self::Code(SignalCode::Rejected),
            0x02 => Self::Code(SignalCode::Calling),
            0x03 => Self::Code(SignalCode::Available),
            0x04 => Self::Code(SignalCode::Ringing),
            0x05 => Self::Code(SignalCode::Connecting),
            0x06 => Self::Code(SignalCode::Established),
            0x10..=0x80 => Self::PreferredProfile(value),
            other => Self::Unknown(other),
        }
    }
}
