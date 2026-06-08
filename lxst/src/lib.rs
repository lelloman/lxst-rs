//! High-level LXST Rust API.

pub mod audio;
pub mod codec;
pub mod network;
pub mod pipeline;
pub mod telephony;

pub use lxst_core as core;

pub use audio::{Agc, AudioFrame, BandPass, HighPass, LowPass, Mixer, ToneSource};
pub use codec::{
    AudioCodec, Codec2Codec, CodecError, CodecFactory, CodecSelection, CodecState, NullCodec,
    OpusCodec, RawCodec,
};
pub use network::{LxstLinkSender, NetworkError, PacketSender, Packetizer, TelephonyEndpoint};
pub use pipeline::{
    AudioSink, AudioSource, BufferedSink, BufferedSource, EncodedAudioFrame, Pipeline,
    PipelineError,
};

pub use lxst_core::{
    CallProfile, CodecHeader, CodecKind, CodecProfile, EncodedFrame, FrameDuration, LxstPacket,
    PacketError, RawBitDepth, RawFrameHeader, Signal, SignalCode,
};
pub use telephony::{CallEvent, CallState, CallerPolicy, Telephone, TelephoneConfig};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error(transparent)]
    Packet(#[from] PacketError),
    #[error("operation is not implemented yet")]
    NotImplemented,
}
