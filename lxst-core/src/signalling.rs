use serde::{Deserialize, Serialize};

use crate::telephony::CallProfile;

/// In-band signalling field used in LXST network packets.
pub const FIELD_SIGNALLING: u8 = 0x00;

/// Frame field used in LXST network packets.
pub const FIELD_FRAMES: u8 = 0x01;

/// Python LXST encodes preferred profiles as `0xFF + profile`.
pub const PREFERRED_PROFILE_BASE: u16 = 0x00FF;

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

impl SignalCode {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for SignalCode {
    type Error = SignalCodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Busy),
            0x01 => Ok(Self::Rejected),
            0x02 => Ok(Self::Calling),
            0x03 => Ok(Self::Available),
            0x04 => Ok(Self::Ringing),
            0x05 => Ok(Self::Connecting),
            0x06 => Ok(Self::Established),
            other => Err(SignalCodeError(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown LXST telephony signal code 0x{0:02x}")]
pub struct SignalCodeError(pub u8);

/// A parsed signalling value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Signal {
    Code(SignalCode),
    PreferredProfile(CallProfile),
    Unknown(u64),
}

impl Signal {
    pub fn to_wire_value(self) -> u64 {
        match self {
            Self::Code(code) => code.as_u8() as u64,
            Self::PreferredProfile(profile) => {
                PREFERRED_PROFILE_BASE as u64 + profile.as_u8() as u64
            }
            Self::Unknown(value) => value,
        }
    }

    pub fn from_wire_value(value: u64) -> Self {
        if value <= SignalCode::Established.as_u8() as u64 {
            return SignalCode::try_from(value as u8)
                .map(Self::Code)
                .unwrap_or(Self::Unknown(value));
        }

        if value >= PREFERRED_PROFILE_BASE as u64 {
            let profile = value - PREFERRED_PROFILE_BASE as u64;
            if profile <= u8::MAX as u64 {
                if let Ok(profile) = CallProfile::try_from(profile as u8) {
                    return Self::PreferredProfile(profile);
                }
            }
        }

        Self::Unknown(value)
    }
}

impl From<u8> for Signal {
    fn from(value: u8) -> Self {
        Self::from_wire_value(value as u64)
    }
}
