use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use lxst_core::CodecKind;

use crate::audio::{AudioError, AudioFrame, MixerSink};
use crate::codec::{AudioCodec, CodecError};
use crate::network::NetworkError;

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

#[derive(Clone)]
pub struct Loopback {
    inner: Arc<Mutex<LoopbackInner>>,
}

struct LoopbackInner {
    codec: Box<dyn AudioCodec>,
    samplerate: u32,
    channels: u8,
    frames: VecDeque<AudioFrame>,
    max_frames: usize,
    running: bool,
}

impl Loopback {
    pub fn new(
        codec: Box<dyn AudioCodec>,
        samplerate: u32,
        channels: u8,
        max_frames: usize,
    ) -> Result<Self, AudioError> {
        AudioFrame::silence(samplerate, channels, 0)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(LoopbackInner {
                codec,
                samplerate,
                channels,
                frames: VecDeque::with_capacity(max_frames.max(1)),
                max_frames: max_frames.max(1),
                running: false,
            })),
        })
    }

    pub fn queued_frames(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.frames.len())
            .unwrap_or_default()
    }
}

impl AudioSource for Loopback {
    fn start(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.running = true;
        }
    }

    fn stop(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.running = false;
        }
    }

    fn is_running(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.running)
            .unwrap_or(false)
    }

    fn samplerate(&self) -> u32 {
        self.inner
            .lock()
            .map(|inner| inner.samplerate)
            .unwrap_or_default()
    }

    fn channels(&self) -> u8 {
        self.inner
            .lock()
            .map(|inner| inner.channels)
            .unwrap_or_default()
    }

    fn next_frame(&mut self) -> Result<Option<AudioFrame>, PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|err| PipelineError::Synchronization(err.to_string()))?;
        if inner.running {
            Ok(inner.frames.pop_front())
        } else {
            Ok(None)
        }
    }
}

impl AudioSink for Loopback {
    fn can_receive(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.frames.len() < inner.max_frames)
            .unwrap_or(false)
    }

    fn handle_frame(&mut self, frame: EncodedAudioFrame) -> Result<(), PipelineError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|err| PipelineError::Synchronization(err.to_string()))?;
        if inner.frames.len() >= inner.max_frames {
            inner.frames.pop_front();
        }
        let decoded = inner.codec.decode(&frame.payload, frame.samplerate)?;
        inner.samplerate = decoded.samplerate();
        inner.channels = decoded.channels();
        inner.frames.push_back(decoded);
        Ok(())
    }
}

pub struct EncodedMixerSink<S>
where
    S: AudioSink,
{
    codec: Box<dyn AudioCodec>,
    sink: S,
}

impl<S> EncodedMixerSink<S>
where
    S: AudioSink,
{
    pub fn new(codec: Box<dyn AudioCodec>, sink: S) -> Self {
        Self { codec, sink }
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    pub fn into_sink(self) -> S {
        self.sink
    }
}

impl<S> MixerSink for EncodedMixerSink<S>
where
    S: AudioSink + 'static,
{
    fn can_receive(&self) -> bool {
        self.sink.can_receive()
    }

    fn handle_frame(&mut self, frame: AudioFrame) -> Result<(), AudioError> {
        let encoded = EncodedAudioFrame {
            codec: self.codec.kind(),
            samplerate: frame.samplerate(),
            channels: frame.channels(),
            payload: self
                .codec
                .encode(&frame)
                .map_err(|err| AudioError::Stream(err.to_string()))?,
        };
        self.sink
            .handle_frame(encoded)
            .map_err(|err| AudioError::Stream(err.to_string()))
    }
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

    pub fn source(&self) -> &dyn AudioSource {
        self.source.as_ref()
    }

    pub fn source_mut(&mut self) -> &mut dyn AudioSource {
        self.source.as_mut()
    }

    pub fn codec(&self) -> &dyn AudioCodec {
        self.codec.as_ref()
    }

    pub fn codec_mut(&mut self) -> &mut dyn AudioCodec {
        self.codec.as_mut()
    }

    pub fn sink(&self) -> &dyn AudioSink {
        self.sink.as_ref()
    }

    pub fn sink_mut(&mut self) -> &mut dyn AudioSink {
        self.sink.as_mut()
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

pub struct PipelineRunner {
    stop_requested: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<(), PipelineError>>>,
}

impl PipelineRunner {
    pub fn start(mut pipeline: Pipeline, poll_interval: Duration) -> Self {
        let stop_requested = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(true));
        let worker_stop = Arc::clone(&stop_requested);
        let worker_running = Arc::clone(&running);

        let worker = thread::spawn(move || {
            pipeline.start();

            let result = loop {
                if worker_stop.load(Ordering::SeqCst) {
                    break Ok(());
                }

                match pipeline.process_next() {
                    Ok(true) => {}
                    Ok(false) => thread::sleep(poll_interval),
                    Err(error) => break Err(error),
                }
            };

            pipeline.stop();
            worker_running.store(false, Ordering::SeqCst);
            result
        });

        Self {
            stop_requested,
            running,
            worker: Some(worker),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn stop(&mut self) -> Result<(), PipelineError> {
        self.stop_requested.store(true, Ordering::SeqCst);
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };

        let result = worker.join();
        self.running.store(false, Ordering::SeqCst);

        match result {
            Ok(result) => result,
            Err(_) => Err(PipelineError::WorkerPanic),
        }
    }
}

impl Drop for PipelineRunner {
    fn drop(&mut self) {
        let _ = self.stop();
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
    #[error("pipeline worker thread panicked")]
    WorkerPanic,
    #[error("pipeline synchronization error: {0}")]
    Synchronization(String),
    #[error(transparent)]
    Network(#[from] NetworkError),
}
