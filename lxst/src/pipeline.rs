use std::collections::VecDeque;

use lxst_core::CodecKind;

use crate::audio::{AudioError, AudioFrame};
use crate::codec::{AudioCodec, CodecError};

#[derive(Debug, Clone, PartialEq)]
pub struct EncodedAudioFrame {
    pub codec: CodecKind,
    pub samplerate: u32,
    pub channels: u8,
    pub payload: Vec<u8>,
}

pub trait AudioSource: Send {
    fn start(&mut self);
    fn stop(&mut self);
    fn is_running(&self) -> bool;
    fn samplerate(&self) -> u32;
    fn channels(&self) -> u8;
    fn next_frame(&mut self) -> Result<Option<AudioFrame>, PipelineError>;
}

pub trait AudioSink: Send {
    fn can_receive(&self) -> bool {
        true
    }

    fn handle_frame(&mut self, frame: EncodedAudioFrame) -> Result<(), PipelineError>;
}

pub struct Pipeline {
    source: Box<dyn AudioSource>,
    codec: Box<dyn AudioCodec>,
    sink: Box<dyn AudioSink>,
}

impl Pipeline {
    pub fn new(
        source: Box<dyn AudioSource>,
        codec: Box<dyn AudioCodec>,
        sink: Box<dyn AudioSink>,
    ) -> Self {
        Self {
            source,
            codec,
            sink,
        }
    }

    pub fn start(&mut self) {
        self.source.start();
    }

    pub fn stop(&mut self) {
        self.source.stop();
    }

    pub fn is_running(&self) -> bool {
        self.source.is_running()
    }

    pub fn process_next(&mut self) -> Result<bool, PipelineError> {
        if !self.source.is_running() || !self.sink.can_receive() {
            return Ok(false);
        }
        let Some(frame) = self.source.next_frame()? else {
            return Ok(false);
        };
        let encoded = EncodedAudioFrame {
            codec: self.codec.kind(),
            samplerate: frame.samplerate(),
            channels: frame.channels(),
            payload: self.codec.encode(&frame)?,
        };
        self.sink.handle_frame(encoded)?;
        Ok(true)
    }
}

#[derive(Debug, Clone)]
pub struct BufferedSource {
    samplerate: u32,
    channels: u8,
    frames: VecDeque<AudioFrame>,
    running: bool,
}

impl BufferedSource {
    pub fn new(samplerate: u32, channels: u8) -> Result<Self, AudioError> {
        AudioFrame::silence(samplerate, channels, 0)?;
        Ok(Self {
            samplerate,
            channels,
            frames: VecDeque::new(),
            running: false,
        })
    }

    pub fn push_frame(&mut self, frame: AudioFrame) -> Result<(), PipelineError> {
        if frame.samplerate() != self.samplerate || frame.channels() != self.channels {
            return Err(PipelineError::IncompatibleSourceFrame);
        }
        self.frames.push_back(frame);
        Ok(())
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }
}

impl AudioSource for BufferedSource {
    fn start(&mut self) {
        self.running = true;
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn samplerate(&self) -> u32 {
        self.samplerate
    }

    fn channels(&self) -> u8 {
        self.channels
    }

    fn next_frame(&mut self) -> Result<Option<AudioFrame>, PipelineError> {
        if self.running {
            Ok(self.frames.pop_front())
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug, Clone)]
pub struct BufferedSink {
    frames: VecDeque<EncodedAudioFrame>,
    max_frames: usize,
}

impl BufferedSink {
    pub fn new(max_frames: usize) -> Self {
        Self {
            frames: VecDeque::with_capacity(max_frames),
            max_frames,
        }
    }

    pub fn pop_frame(&mut self) -> Option<EncodedAudioFrame> {
        self.frames.pop_front()
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }
}

impl AudioSink for BufferedSink {
    fn can_receive(&self) -> bool {
        self.frames.len() < self.max_frames
    }

    fn handle_frame(&mut self, frame: EncodedAudioFrame) -> Result<(), PipelineError> {
        if !self.can_receive() {
            return Err(PipelineError::SinkFull);
        }
        self.frames.push_back(frame);
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("source frame does not match source format")]
    IncompatibleSourceFrame,
    #[error("sink buffer is full")]
    SinkFull,
}
