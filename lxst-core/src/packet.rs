use rns_core::msgpack::{pack, unpack_exact, Value};
use serde::{Deserialize, Serialize};

use crate::codec::{CodecHeader, CodecHeaderError, CodecKind};
use crate::signalling::{Signal, FIELD_FRAMES, FIELD_SIGNALLING};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodedFrame {
    pub codec: CodecKind,
    pub payload: Vec<u8>,
}

impl EncodedFrame {
    pub fn new(codec: CodecKind, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            codec,
            payload: payload.into(),
        }
    }

    pub fn to_wire_bytes(&self) -> Result<Vec<u8>, PacketError> {
        let header = CodecHeader::try_from(self.codec).map_err(PacketError::CodecHeader)?;
        let mut bytes = Vec::with_capacity(1 + self.payload.len());
        bytes.push(header.as_u8());
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }

    pub fn from_wire_bytes(bytes: &[u8]) -> Result<Self, PacketError> {
        let (&header, payload) = bytes.split_first().ok_or(PacketError::EmptyFrame)?;
        let header = CodecHeader::try_from(header).map_err(PacketError::CodecHeader)?;
        Ok(Self {
            codec: header.into(),
            payload: payload.to_vec(),
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LxstPacket {
    pub signals: Vec<Signal>,
    pub frames: Vec<EncodedFrame>,
}

impl LxstPacket {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn signalling(signal: Signal) -> Self {
        Self {
            signals: vec![signal],
            frames: Vec::new(),
        }
    }

    pub fn frame(frame: EncodedFrame) -> Self {
        Self {
            signals: Vec::new(),
            frames: vec![frame],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.signals.is_empty() && self.frames.is_empty()
    }

    pub fn encode(&self) -> Result<Vec<u8>, PacketError> {
        let mut entries = Vec::new();

        if !self.signals.is_empty() {
            let values = self
                .signals
                .iter()
                .map(|signal| Value::UInt(signal.to_wire_value()))
                .collect();
            entries.push((Value::UInt(FIELD_SIGNALLING as u64), Value::Array(values)));
        }

        if !self.frames.is_empty() {
            if self.frames.len() == 1 {
                entries.push((
                    Value::UInt(FIELD_FRAMES as u64),
                    Value::Bin(self.frames[0].to_wire_bytes()?),
                ));
            } else {
                let values = self
                    .frames
                    .iter()
                    .map(|frame| frame.to_wire_bytes().map(Value::Bin))
                    .collect::<Result<Vec<_>, _>>()?;
                entries.push((Value::UInt(FIELD_FRAMES as u64), Value::Array(values)));
            }
        }

        Ok(pack(&Value::Map(entries)))
    }

    pub fn decode(data: &[u8]) -> Result<Self, PacketError> {
        let value = unpack_exact(data).map_err(|err| PacketError::Msgpack(err.to_string()))?;
        Self::from_msgpack_value(&value)
    }

    pub fn from_msgpack_value(value: &Value) -> Result<Self, PacketError> {
        let entries = value.as_map().ok_or(PacketError::ExpectedMap)?;
        let mut packet = Self::default();

        for (key, value) in entries {
            match msgpack_key_to_u64(key) {
                Some(field) if field == FIELD_SIGNALLING as u64 => {
                    packet.signals.extend(parse_signals(value)?);
                }
                Some(field) if field == FIELD_FRAMES as u64 => {
                    packet.frames.extend(parse_frames(value)?);
                }
                _ => {}
            }
        }

        Ok(packet)
    }
}

fn msgpack_key_to_u64(value: &Value) -> Option<u64> {
    match value {
        Value::UInt(value) => Some(*value),
        Value::Int(value) if *value >= 0 => Some(*value as u64),
        _ => None,
    }
}

fn parse_signals(value: &Value) -> Result<Vec<Signal>, PacketError> {
    match value {
        Value::Array(values) => values.iter().map(parse_signal).collect(),
        other => Ok(vec![parse_signal(other)?]),
    }
}

fn parse_signal(value: &Value) -> Result<Signal, PacketError> {
    let wire_value = match value {
        Value::UInt(value) => *value,
        Value::Int(value) if *value >= 0 => *value as u64,
        other => return Err(PacketError::InvalidSignalValue(format!("{other:?}"))),
    };
    Ok(Signal::from_wire_value(wire_value))
}

fn parse_frames(value: &Value) -> Result<Vec<EncodedFrame>, PacketError> {
    match value {
        Value::Array(values) => values.iter().map(parse_frame).collect(),
        other => Ok(vec![parse_frame(other)?]),
    }
}

fn parse_frame(value: &Value) -> Result<EncodedFrame, PacketError> {
    let bytes = value
        .as_bin()
        .ok_or_else(|| PacketError::InvalidFrameValue(format!("{value:?}")))?;
    EncodedFrame::from_wire_bytes(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PacketError {
    #[error("LXST packet root must be a MessagePack map")]
    ExpectedMap,
    #[error("LXST frame value must be a binary object: {0}")]
    InvalidFrameValue(String),
    #[error("LXST frame is empty")]
    EmptyFrame,
    #[error(transparent)]
    CodecHeader(CodecHeaderError),
    #[error("LXST signal value must be a non-negative integer: {0}")]
    InvalidSignalValue(String),
    #[error("MessagePack error: {0}")]
    Msgpack(String),
}
