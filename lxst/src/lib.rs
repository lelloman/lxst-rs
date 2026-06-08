//! High-level LXST Rust API.

pub mod audio;
pub mod codec;
pub mod hardware;
pub mod media;
pub mod network;
pub mod pipeline;
pub mod telephony;

pub use lxst_core as core;

pub use audio::{
    list_audio_devices, Agc, AudioDeviceInfo, AudioDeviceKind, AudioError, AudioFrame,
    AudioStreamConfigInfo, BandPass, CpalInputConfig, CpalInputSource, CpalOutputConfig,
    CpalOutputSink, HighPass, LowPass, Mixer, ToneSource,
};
pub use codec::{
    AudioCodec, Codec2Codec, CodecError, CodecFactory, CodecSelection, CodecState, NullCodec,
    OpusCodec, RawCodec,
};
pub use hardware::{Key, KeyTransition, KeypadEvent, Lcd1602Buffer, MatrixKeypad};
pub use media::{FilePlayer, FileRecorder, MediaError, OpusFileSink, OpusFileSource};
pub use network::{
    telephony_callback_channel, LinkSource, LxstLinkSender, NetworkError, PacketSender, Packetizer,
    TelephonyCallbacks, TelephonyEndpoint, TelephonyNetworkEvent,
};
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
