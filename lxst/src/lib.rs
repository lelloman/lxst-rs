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
    list_audio_devices, plan_line_source_frame, plan_mixer_frame, select_audio_device_info, Agc,
    AudioDeviceInfo, AudioDeviceKind, AudioError, AudioFrame, AudioFramePlan,
    AudioStreamConfigInfo, BandPass, CpalInputConfig, CpalInputSource, CpalOutputConfig,
    CpalOutputSink, HighPass, LinePlayback, LineSourceFramePlan, LineSourceProcessor, LowPass,
    Mixer, MixerFramePlan, MixerRuntime, MixerSink, QueuedLineSink, QueuedLineSinkConfig,
    QueuedLineSinkStats, ToneSource,
};
pub use codec::{
    AudioCodec, Codec2Codec, CodecError, CodecFactory, CodecSelection, CodecState, NullCodec,
    OpusCodec, RawCodec,
};
#[cfg(feature = "gpio-rpi")]
pub use hardware::RpiMatrixKeypadBackend;
pub use hardware::{
    BufferedLcd1602, Key, KeyTransition, KeypadEvent, Lcd1602Buffer, Lcd1602Display, MatrixKeypad,
    MatrixKeypadBackend, MatrixKeypadPoller, MatrixKeypadScanner,
};
pub use media::{
    AudioFrameSink, FilePlayer, FileRecorder, MediaError, OpusFileSink, OpusFileSource,
    QueuedOpusFileSink, QueuedOpusFileSinkConfig, SourcePlayer, SourceRecorder,
};
pub use network::{
    telephony_callback_channel, LinkSource, LxstLinkSender, NetworkError, PacketSender, Packetizer,
    TelephonyCallbacks, TelephonyEndpoint, TelephonyNetworkEvent, TelephonyNode,
};
pub use pipeline::{
    AudioSink, AudioSource, BufferedSink, BufferedSource, EncodedAudioFrame, EncodedMixerSink,
    Loopback, MixerInputSink, Pipeline, PipelineError, PipelineRunner,
};

pub use lxst_core::{
    CallProfile, CodecHeader, CodecKind, CodecProfile, EncodedFrame, FrameDuration, LxstPacket,
    PacketError, RawBitDepth, RawFrameHeader, Signal, SignalCode,
};
pub use telephony::{
    CallEvent, CallState, CallerPolicy, Telephone, TelephoneConfig, MIN_ANNOUNCE_INTERVAL,
};

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
