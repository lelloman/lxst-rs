use std::collections::{HashMap, VecDeque};
use std::f32::consts::PI;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Condvar, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use lxst_core::CodecProfile;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    samplerate: u32,
    channels: u8,
    samples: Vec<f32>,
}

impl AudioFrame {
    pub fn new(samplerate: u32, channels: u8, samples: Vec<f32>) -> Result<Self, AudioError> {
        if samplerate == 0 {
            return Err(AudioError::InvalidSamplerate(samplerate));
        }
        if channels == 0 {
            return Err(AudioError::InvalidChannels(channels));
        }
        if !samples.len().is_multiple_of(channels as usize) {
            return Err(AudioError::SampleCountNotDivisible {
                samples: samples.len(),
                channels,
            });
        }
        Ok(Self {
            samplerate,
            channels,
            samples,
        })
    }

    pub fn silence(samplerate: u32, channels: u8, frames: usize) -> Result<Self, AudioError> {
        Self::new(samplerate, channels, vec![0.0; frames * channels as usize])
    }

    pub fn samplerate(&self) -> u32 {
        self.samplerate
    }

    pub fn channels(&self) -> u8 {
        self.channels
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn samples_mut(&mut self) -> &mut [f32] {
        &mut self.samples
    }

    pub fn frame_count(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn duration_ms(&self) -> f32 {
        self.frame_count() as f32 * 1000.0 / self.samplerate as f32
    }

    pub fn map_samples(mut self, mut f: impl FnMut(f32) -> f32) -> Self {
        for sample in &mut self.samples {
            *sample = f(*sample);
        }
        self
    }

    pub fn apply_gain_db(self, gain_db: f32) -> Self {
        if gain_db == 0.0 {
            return self;
        }
        let gain = 10.0_f32.powf(gain_db / 10.0);
        self.map_samples(|sample| sample * gain)
    }

    pub fn with_channels(&self, channels: u8) -> Result<Self, AudioError> {
        let samples = normalize_audio_channels(self.samples(), self.channels(), channels)?;
        Self::new(self.samplerate(), channels, samples)
    }

    pub fn resampled(&self, samplerate: u32) -> Result<Self, AudioError> {
        let samples = resample_audio_linear(
            self.samples(),
            self.channels(),
            self.samplerate(),
            samplerate,
        )?;
        Self::new(samplerate, self.channels(), samples)
    }

    pub fn clipped(mut self) -> Self {
        for sample in &mut self.samples {
            *sample = sample.clamp(-1.0, 1.0);
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AudioError {
    #[error("invalid audio samplerate {0}")]
    InvalidSamplerate(u32),
    #[error("invalid audio channel count {0}")]
    InvalidChannels(u8),
    #[error("sample count {samples} is not divisible by channel count {channels}")]
    SampleCountNotDivisible { samples: usize, channels: u8 },
    #[error("audio frames are incompatible")]
    IncompatibleFrames,
    #[error("no {0:?} audio device is available")]
    NoDevice(AudioDeviceKind),
    #[error("unsupported audio device format: {0}")]
    UnsupportedDeviceFormat(String),
    #[error("audio stream error: {0}")]
    Stream(String),
    #[error("audio device error: {0}")]
    Device(String),
}

pub trait AudioFilter: Send {
    fn process(&mut self, frame: AudioFrame) -> AudioFrame;
}

pub trait LinePlayback: Send + 'static {
    fn start(&mut self) -> Result<(), AudioError> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        Ok(())
    }

    fn play(&mut self, frame: AudioFrame) -> Result<(), AudioError>;

    fn enable_low_latency(&mut self) -> Result<bool, AudioError> {
        Ok(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedLineSinkConfig {
    pub samplerate: u32,
    pub channels: u8,
    pub max_frames: usize,
    pub autostart_min: usize,
    pub frame_timeout: usize,
    pub autodigest: bool,
    pub low_latency: bool,
}

impl Default for QueuedLineSinkConfig {
    fn default() -> Self {
        Self {
            samplerate: 48_000,
            channels: 1,
            max_frames: 6,
            autostart_min: 1,
            frame_timeout: 8,
            autodigest: true,
            low_latency: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct QueuedLineSinkStats {
    pub output_latency: Duration,
    pub max_latency: Duration,
    pub underrun_at: Option<Instant>,
    pub low_latency_enabled: bool,
}

pub struct QueuedLineSink<P>
where
    P: LinePlayback,
{
    config: QueuedLineSinkConfig,
    buffer_max_height: usize,
    queue: Arc<(Mutex<VecDeque<AudioFrame>>, Condvar)>,
    should_run: Arc<AtomicBool>,
    wants_low_latency: Arc<AtomicBool>,
    stats: Arc<Mutex<QueuedLineSinkStats>>,
    backend: Option<P>,
    worker: Option<JoinHandle<Result<P, AudioError>>>,
    samples_per_frame: Option<usize>,
    frame_time: Option<Duration>,
}

impl<P> QueuedLineSink<P>
where
    P: LinePlayback,
{
    pub fn new(backend: P, config: QueuedLineSinkConfig) -> Result<Self, AudioError> {
        AudioFrame::silence(config.samplerate, config.channels, 0)?;
        let max_frames = config.max_frames.max(1);
        Ok(Self {
            buffer_max_height: max_frames.saturating_sub(3).max(1),
            config: QueuedLineSinkConfig {
                max_frames,
                autostart_min: config.autostart_min.max(1),
                frame_timeout: config.frame_timeout.max(1),
                ..config
            },
            queue: Arc::new((
                Mutex::new(VecDeque::with_capacity(max_frames)),
                Condvar::new(),
            )),
            should_run: Arc::new(AtomicBool::new(false)),
            wants_low_latency: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(Mutex::new(QueuedLineSinkStats::default())),
            backend: Some(backend),
            worker: None,
            samples_per_frame: None,
            frame_time: None,
        })
    }

    pub fn can_receive(&self) -> bool {
        self.queue
            .0
            .lock()
            .map(|queue| queue.len() < self.buffer_max_height)
            .unwrap_or(false)
    }

    pub fn queued_frames(&self) -> usize {
        self.queue
            .0
            .lock()
            .map(|queue| queue.len())
            .unwrap_or_default()
    }

    pub fn is_running(&self) -> bool {
        self.should_run.load(Ordering::SeqCst)
    }

    pub fn stats(&self) -> QueuedLineSinkStats {
        self.stats.lock().map(|stats| *stats).unwrap_or_default()
    }

    pub fn samples_per_frame(&self) -> Option<usize> {
        self.samples_per_frame
    }

    pub fn frame_time(&self) -> Option<Duration> {
        self.frame_time
    }

    pub fn enable_low_latency(&self) {
        self.wants_low_latency.store(true, Ordering::SeqCst);
        self.queue.1.notify_all();
    }

    pub fn handle_frame(&mut self, frame: AudioFrame) -> Result<(), AudioError> {
        let frame = frame
            .with_channels(self.config.channels)?
            .resampled(self.config.samplerate)?
            .clipped();
        if self.samples_per_frame.is_none() {
            let frame_count = frame.frame_count();
            self.samples_per_frame = Some(frame_count);
            self.frame_time = Some(Duration::from_secs_f64(
                frame_count as f64 / self.config.samplerate as f64,
            ));
        }

        let should_start = {
            let (lock, cvar) = &*self.queue;
            let mut queue = lock
                .lock()
                .map_err(|err| AudioError::Stream(err.to_string()))?;
            if queue.len() >= self.config.max_frames {
                queue.pop_front();
            }
            queue.push_back(frame);
            cvar.notify_one();
            self.config.autodigest && queue.len() >= self.config.autostart_min && !self.is_running()
        };

        if should_start {
            self.start()?;
        }
        Ok(())
    }

    pub fn start(&mut self) -> Result<(), AudioError> {
        if self.worker.is_some() {
            self.should_run.store(true, Ordering::SeqCst);
            self.queue.1.notify_all();
            return Ok(());
        }
        let Some(mut backend) = self.backend.take() else {
            return Err(AudioError::Stream(
                "queued line sink backend is unavailable".to_string(),
            ));
        };
        backend.start()?;
        self.should_run.store(true, Ordering::SeqCst);
        if self.config.low_latency {
            self.wants_low_latency.store(true, Ordering::SeqCst);
        }

        let queue = Arc::clone(&self.queue);
        let should_run = Arc::clone(&self.should_run);
        let wants_low_latency = Arc::clone(&self.wants_low_latency);
        let stats = Arc::clone(&self.stats);
        let buffer_max_height = self.buffer_max_height;
        let frame_timeout = self.config.frame_timeout;
        let frame_time = self.frame_time.unwrap_or(Duration::from_millis(20));

        self.worker = Some(thread::spawn(move || {
            loop {
                let frame = {
                    let (lock, cvar) = &*queue;
                    let mut queue = lock
                        .lock()
                        .map_err(|err| AudioError::Stream(err.to_string()))?;
                    if queue.is_empty() && should_run.load(Ordering::SeqCst) {
                        let wait = frame_time.mul_f32(0.1).max(Duration::from_millis(1));
                        let (guard, _) = cvar
                            .wait_timeout(queue, wait)
                            .map_err(|err| AudioError::Stream(err.to_string()))?;
                        queue = guard;
                    }
                    queue.pop_front()
                };

                if let Some(frame) = frame {
                    {
                        let queued = queue.0.lock().map(|queue| queue.len()).unwrap_or_default();
                        let mut stats = stats
                            .lock()
                            .map_err(|err| AudioError::Stream(err.to_string()))?;
                        stats.output_latency = frame_time.saturating_mul(queued as u32);
                        stats.max_latency = frame_time.saturating_mul(buffer_max_height as u32);
                        stats.underrun_at = None;
                    }
                    backend.play(frame)?;
                    let mut queue = queue
                        .0
                        .lock()
                        .map_err(|err| AudioError::Stream(err.to_string()))?;
                    if queue.len() > buffer_max_height {
                        queue.pop_front();
                    }
                } else if should_run.load(Ordering::SeqCst) {
                    let mut stats = stats
                        .lock()
                        .map_err(|err| AudioError::Stream(err.to_string()))?;
                    if let Some(underrun_at) = stats.underrun_at {
                        if underrun_at.elapsed() > frame_time.saturating_mul(frame_timeout as u32) {
                            should_run.store(false, Ordering::SeqCst);
                        }
                    } else {
                        stats.underrun_at = Some(Instant::now());
                    }
                } else {
                    break;
                }

                if wants_low_latency.swap(false, Ordering::SeqCst)
                    && backend.enable_low_latency()?
                {
                    let mut stats = stats
                        .lock()
                        .map_err(|err| AudioError::Stream(err.to_string()))?;
                    stats.low_latency_enabled = true;
                }
            }
            backend.stop()?;
            Ok(backend)
        }));
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), AudioError> {
        self.should_run.store(false, Ordering::SeqCst);
        self.queue.1.notify_all();
        if let Some(worker) = self.worker.take() {
            match worker.join() {
                Ok(result) => {
                    self.backend = Some(result?);
                }
                Err(_) => return Err(AudioError::Stream("line sink worker panicked".to_string())),
            }
        }
        Ok(())
    }
}

impl<P> Drop for QueuedLineSink<P>
where
    P: LinePlayback,
{
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioFramePlan {
    pub requested_frame_ms: f32,
    pub target_frame_ms: f32,
    pub samplerate: u32,
    pub channels: u8,
    pub frame_count: usize,
    pub sample_count: usize,
}

pub fn plan_line_source_frame(
    requested_frame_ms: f32,
    codec_profile: Option<CodecProfile>,
    samplerate: u32,
    channels: u8,
) -> Result<AudioFramePlan, AudioError> {
    AudioFrame::silence(samplerate, channels, 0)?;
    let mut target_frame_ms = requested_frame_ms.max(1.0);

    if let Some(timing) = codec_profile.and_then(codec_timing) {
        if let Some(quanta) = timing.frame_quanta_ms {
            let remainder = target_frame_ms % quanta;
            if remainder.abs() > f32::EPSILON {
                target_frame_ms = (target_frame_ms / quanta).ceil() * quanta;
            }
        }
        if let Some(maximum) = timing.frame_max_ms {
            if target_frame_ms > maximum {
                target_frame_ms = maximum;
            }
        }
        if let Some(valid) = timing.valid_frame_ms {
            if !valid
                .iter()
                .any(|duration| (*duration - target_frame_ms).abs() < f32::EPSILON)
            {
                target_frame_ms = valid
                    .iter()
                    .copied()
                    .min_by(|left, right| {
                        let left_delta = (*left - target_frame_ms).abs();
                        let right_delta = (*right - target_frame_ms).abs();
                        left_delta
                            .partial_cmp(&right_delta)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .unwrap_or(target_frame_ms);
            }
        }
    }

    let frame_count = ((samplerate as f32 * target_frame_ms) / 1000.0).ceil() as usize;
    let sample_count = frame_count * channels as usize;
    Ok(AudioFramePlan {
        requested_frame_ms,
        target_frame_ms,
        samplerate,
        channels,
        frame_count,
        sample_count,
    })
}

pub type LineSourceFramePlan = AudioFramePlan;
pub type MixerFramePlan = AudioFramePlan;

pub fn plan_mixer_frame(
    requested_frame_ms: f32,
    codec_profile: Option<CodecProfile>,
    samplerate: u32,
    channels: u8,
) -> Result<MixerFramePlan, AudioError> {
    plan_line_source_frame(requested_frame_ms, codec_profile, samplerate, channels)
}

#[derive(Debug, Clone, Copy)]
struct CodecTiming {
    frame_quanta_ms: Option<f32>,
    frame_max_ms: Option<f32>,
    valid_frame_ms: Option<&'static [f32]>,
}

fn codec_timing(profile: CodecProfile) -> Option<CodecTiming> {
    match profile {
        CodecProfile::OpusVoiceLow
        | CodecProfile::OpusVoiceMedium
        | CodecProfile::OpusVoiceHigh
        | CodecProfile::OpusVoiceMax
        | CodecProfile::OpusAudioMin
        | CodecProfile::OpusAudioLow
        | CodecProfile::OpusAudioMedium
        | CodecProfile::OpusAudioHigh
        | CodecProfile::OpusAudioMax => Some(CodecTiming {
            frame_quanta_ms: Some(2.5),
            frame_max_ms: Some(60.0),
            valid_frame_ms: Some(&[2.5, 5.0, 10.0, 20.0, 40.0, 60.0]),
        }),
        CodecProfile::Codec2_700C
        | CodecProfile::Codec2_1200
        | CodecProfile::Codec2_1300
        | CodecProfile::Codec2_1400
        | CodecProfile::Codec2_1600
        | CodecProfile::Codec2_2400
        | CodecProfile::Codec2_3200 => Some(CodecTiming {
            frame_quanta_ms: Some(40.0),
            frame_max_ms: None,
            valid_frame_ms: None,
        }),
        CodecProfile::Raw => None,
        _ => None,
    }
}

pub struct LineSourceProcessor {
    filters: Vec<Box<dyn AudioFilter>>,
    gain_db: f32,
    target_gain: f32,
    current_gain: f32,
    ease_samples: usize,
    skip_samples: usize,
    skipped_samples: usize,
    processed_samples: usize,
}

impl LineSourceProcessor {
    pub fn new(
        gain_db: f32,
        ease_in: Duration,
        skip: Duration,
        samplerate: u32,
        channels: u8,
    ) -> Self {
        let target_gain = linear_gain(gain_db);
        let ease_samples = duration_samples(ease_in, samplerate, channels);
        let current_gain = if ease_samples == 0 { target_gain } else { 0.0 };

        Self {
            filters: Vec::new(),
            gain_db,
            target_gain,
            current_gain,
            ease_samples,
            skip_samples: duration_samples(skip, samplerate, channels),
            skipped_samples: 0,
            processed_samples: 0,
        }
    }

    pub fn add_filter(&mut self, filter: impl AudioFilter + 'static) {
        self.filters.push(Box::new(filter));
    }

    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    pub fn skipped_samples(&self) -> usize {
        self.skipped_samples
    }

    pub fn processed_samples(&self) -> usize {
        self.processed_samples
    }

    pub fn process_frame(&mut self, mut frame: AudioFrame) -> Option<AudioFrame> {
        if self.skipped_samples < self.skip_samples {
            self.skipped_samples += frame.samples().len();
            return None;
        }

        for filter in &mut self.filters {
            frame = filter.process(frame);
        }

        if (self.current_gain - 1.0).abs() > f32::EPSILON {
            for sample in frame.samples_mut() {
                *sample *= self.current_gain;
            }
        }

        self.processed_samples += frame.samples().len();
        if self.ease_samples > 0 && self.current_gain < self.target_gain {
            let progress =
                (self.processed_samples as f32 / self.ease_samples as f32).clamp(0.0, 1.0);
            self.current_gain = (self.target_gain * progress).min(self.target_gain);
        }

        Some(frame.clipped())
    }
}

#[derive(Debug, Clone)]
pub struct HighPass {
    cut: f32,
    samplerate: Option<u32>,
    alpha: f32,
    states: Vec<f32>,
    last_inputs: Vec<f32>,
}

impl HighPass {
    pub fn new(cut: f32) -> Self {
        Self {
            cut,
            samplerate: None,
            alpha: 0.0,
            states: Vec::new(),
            last_inputs: Vec::new(),
        }
    }

    fn configure(&mut self, samplerate: u32, channels: u8) {
        if self.samplerate != Some(samplerate) {
            self.samplerate = Some(samplerate);
            let dt = 1.0 / samplerate as f32;
            let rc = 1.0 / (2.0 * PI * self.cut);
            self.alpha = rc / (rc + dt);
        }
        if self.states.len() != channels as usize {
            self.states = vec![0.0; channels as usize];
            self.last_inputs = vec![0.0; channels as usize];
        }
    }
}

impl AudioFilter for HighPass {
    fn process(&mut self, frame: AudioFrame) -> AudioFrame {
        if frame.samples.is_empty() {
            return frame;
        }
        self.configure(frame.samplerate, frame.channels);
        let channels = frame.channels as usize;
        let mut out = frame.samples.clone();
        for frame_index in 0..frame.frame_count() {
            for channel in 0..channels {
                let idx = frame_index * channels + channel;
                let input = frame.samples[idx];
                let previous_input = self.last_inputs[channel];
                let previous_output = self.states[channel];
                let output = self.alpha * (previous_output + input - previous_input);
                out[idx] = output;
                self.states[channel] = output;
                self.last_inputs[channel] = input;
            }
        }
        AudioFrame {
            samples: out,
            ..frame
        }
    }
}

#[derive(Debug, Clone)]
pub struct LowPass {
    cut: f32,
    samplerate: Option<u32>,
    alpha: f32,
    states: Vec<f32>,
}

impl LowPass {
    pub fn new(cut: f32) -> Self {
        Self {
            cut,
            samplerate: None,
            alpha: 0.0,
            states: Vec::new(),
        }
    }

    fn configure(&mut self, samplerate: u32, channels: u8) {
        if self.samplerate != Some(samplerate) {
            self.samplerate = Some(samplerate);
            let dt = 1.0 / samplerate as f32;
            let rc = 1.0 / (2.0 * PI * self.cut);
            self.alpha = dt / (rc + dt);
        }
        if self.states.len() != channels as usize {
            self.states = vec![0.0; channels as usize];
        }
    }
}

impl AudioFilter for LowPass {
    fn process(&mut self, frame: AudioFrame) -> AudioFrame {
        if frame.samples.is_empty() {
            return frame;
        }
        self.configure(frame.samplerate, frame.channels);
        let channels = frame.channels as usize;
        let mut out = frame.samples.clone();
        for frame_index in 0..frame.frame_count() {
            for channel in 0..channels {
                let idx = frame_index * channels + channel;
                let input = frame.samples[idx];
                let output = self.alpha * input + (1.0 - self.alpha) * self.states[channel];
                out[idx] = output;
                self.states[channel] = output;
            }
        }
        AudioFrame {
            samples: out,
            ..frame
        }
    }
}

#[derive(Debug, Clone)]
pub struct BandPass {
    high_pass: HighPass,
    low_pass: LowPass,
}

impl BandPass {
    pub fn new(low_cut: f32, high_cut: f32) -> Result<Self, AudioError> {
        if low_cut >= high_cut {
            return Err(AudioError::IncompatibleFrames);
        }
        Ok(Self {
            high_pass: HighPass::new(low_cut),
            low_pass: LowPass::new(high_cut),
        })
    }
}

impl AudioFilter for BandPass {
    fn process(&mut self, frame: AudioFrame) -> AudioFrame {
        let frame = self.high_pass.process(frame);
        self.low_pass.process(frame)
    }
}

#[derive(Debug, Clone)]
pub struct Agc {
    trigger_level: f32,
    target_linear: f32,
    max_gain_linear: f32,
    attack_time: f32,
    release_time: f32,
    hold_time: f32,
    samplerate: Option<u32>,
    attack_coeff: f32,
    release_coeff: f32,
    hold_samples: usize,
    hold_counter: usize,
    block_target_seconds: f32,
    current_gain: Vec<f32>,
}

impl Agc {
    pub fn new(target_level_db: f32, max_gain_db: f32) -> Self {
        Self::with_timing(target_level_db, max_gain_db, 0.0001, 0.002, 0.001)
    }

    pub fn with_timing(
        target_level_db: f32,
        max_gain_db: f32,
        attack_time: f32,
        release_time: f32,
        hold_time: f32,
    ) -> Self {
        Self {
            trigger_level: 0.003,
            target_linear: 10.0_f32.powf(target_level_db / 10.0),
            max_gain_linear: 10.0_f32.powf(max_gain_db / 10.0),
            attack_time,
            release_time,
            hold_time,
            samplerate: None,
            attack_coeff: 0.1,
            release_coeff: 0.01,
            hold_samples: 1000,
            hold_counter: 0,
            block_target_seconds: 0.01,
            current_gain: Vec::new(),
        }
    }

    fn configure(&mut self, samplerate: u32, channels: u8) {
        if self.samplerate != Some(samplerate) {
            self.samplerate = Some(samplerate);
            self.attack_coeff = 1.0 - (-1.0 / (self.attack_time * samplerate as f32)).exp();
            self.release_coeff = 1.0 - (-1.0 / (self.release_time * samplerate as f32)).exp();
            self.hold_samples = (self.hold_time * samplerate as f32) as usize;
        }
        if self.current_gain.len() != channels as usize {
            self.current_gain = vec![1.0; channels as usize];
            self.hold_counter = 0;
        }
    }
}

impl AudioFilter for Agc {
    fn process(&mut self, frame: AudioFrame) -> AudioFrame {
        if frame.samples.is_empty() {
            return frame;
        }
        self.configure(frame.samplerate, frame.channels);

        let channels = frame.channels as usize;
        let frames = frame.frame_count();
        let block_target = ((frames as f32 / frame.samplerate as f32) / self.block_target_seconds)
            .floor() as usize;
        let block_size = (frames / block_target.max(1)).max(1);
        let mut samples = frame.samples.clone();

        for block_start in (0..frames).step_by(block_size) {
            let block_end = (block_start + block_size).min(frames);
            let block_samples = block_end - block_start;

            for channel in 0..channels {
                let mut sum_squares = 0.0_f32;
                for frame_index in block_start..block_end {
                    let sample = samples[frame_index * channels + channel];
                    sum_squares += sample * sample;
                }
                let rms = (sum_squares / block_samples as f32).sqrt();
                let target_gain = if rms > 1e-9 && rms > self.trigger_level {
                    (self.target_linear / rms).min(self.max_gain_linear)
                } else {
                    self.current_gain[channel]
                };

                if target_gain < self.current_gain[channel] {
                    self.current_gain[channel] = self.attack_coeff * target_gain
                        + (1.0 - self.attack_coeff) * self.current_gain[channel];
                    self.hold_counter = self.hold_samples;
                } else if self.hold_counter > 0 {
                    self.hold_counter = self.hold_counter.saturating_sub(block_samples);
                } else {
                    self.current_gain[channel] = self.release_coeff * target_gain
                        + (1.0 - self.release_coeff) * self.current_gain[channel];
                }

                for frame_index in block_start..block_end {
                    samples[frame_index * channels + channel] *= self.current_gain[channel];
                }
            }
        }

        const PEAK_LIMIT: f32 = 0.75;
        for channel in 0..channels {
            let mut peak = 0.0_f32;
            for frame_index in 0..frames {
                peak = peak.max(samples[frame_index * channels + channel].abs());
            }
            if peak > PEAK_LIMIT {
                let scale = PEAK_LIMIT / peak;
                for frame_index in 0..frames {
                    samples[frame_index * channels + channel] *= scale;
                }
            }
        }

        AudioFrame { samples, ..frame }
    }
}

pub trait MixerSink: Send + 'static {
    fn can_receive(&self) -> bool {
        true
    }

    fn handle_frame(&mut self, frame: AudioFrame) -> Result<(), AudioError>;
}

#[derive(Debug, Clone)]
pub struct Mixer {
    queues: HashMap<u64, VecDeque<AudioFrame>>,
    max_frames_per_source: usize,
    source_max_frames: HashMap<u64, usize>,
    gain_db: f32,
    muted: bool,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            queues: HashMap::new(),
            max_frames_per_source: 8,
            source_max_frames: HashMap::new(),
            gain_db: 0.0,
            muted: false,
        }
    }
}

impl Mixer {
    pub fn set_source_max_frames(&mut self, source_id: u64, max_frames: usize) {
        let max_frames = max_frames.max(1);
        self.source_max_frames.insert(source_id, max_frames);
        if let Some(queue) = self.queues.get_mut(&source_id) {
            while queue.len() > max_frames {
                queue.pop_front();
            }
        }
    }

    pub fn set_gain_db(&mut self, gain_db: f32) {
        self.gain_db = gain_db;
    }

    pub fn mute(&mut self, muted: bool) {
        self.muted = muted;
    }

    pub fn can_receive(&self, source_id: u64) -> bool {
        self.queued_frames(source_id) < self.source_frame_limit(source_id)
    }

    pub fn queued_frames(&self, source_id: u64) -> usize {
        self.queues
            .get(&source_id)
            .map(VecDeque::len)
            .unwrap_or_default()
    }

    pub fn push(&mut self, source_id: u64, frame: AudioFrame) {
        let max_frames = self.source_frame_limit(source_id);
        let queue = self
            .queues
            .entry(source_id)
            .or_insert_with(|| VecDeque::with_capacity(max_frames));
        if queue.len() >= max_frames {
            queue.pop_front();
        }
        queue.push_back(frame);
    }

    pub fn mix_next(&mut self) -> Result<Option<AudioFrame>, AudioError> {
        let mut mixed: Option<AudioFrame> = None;
        for queue in self.queues.values_mut() {
            let Some(frame) = queue.pop_front() else {
                continue;
            };
            match &mut mixed {
                Some(existing) => {
                    if existing.samplerate != frame.samplerate
                        || existing.channels != frame.channels
                        || existing.samples.len() != frame.samples.len()
                    {
                        return Err(AudioError::IncompatibleFrames);
                    }
                    for (dst, src) in existing.samples.iter_mut().zip(frame.samples.iter()) {
                        *dst += *src;
                    }
                }
                None => mixed = Some(frame),
            }
        }
        Ok(mixed.map(|frame| {
            if self.muted {
                frame.map_samples(|_| 0.0)
            } else {
                frame.apply_gain_db(self.gain_db).clipped()
            }
        }))
    }

    fn source_frame_limit(&self, source_id: u64) -> usize {
        self.source_max_frames
            .get(&source_id)
            .copied()
            .unwrap_or(self.max_frames_per_source)
    }
}

pub struct MixerRuntime<S>
where
    S: MixerSink,
{
    mixer: Arc<Mutex<Mixer>>,
    stop_requested: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<S, AudioError>>>,
}

impl<S> MixerRuntime<S>
where
    S: MixerSink,
{
    pub fn start(mixer: Mixer, sink: S, poll_interval: Duration) -> Self {
        let mixer = Arc::new(Mutex::new(mixer));
        Self::start_shared(mixer, sink, poll_interval)
    }

    pub fn start_shared(mixer: Arc<Mutex<Mixer>>, mut sink: S, poll_interval: Duration) -> Self {
        let stop_requested = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(true));
        let worker_stop = Arc::clone(&stop_requested);
        let worker_running = Arc::clone(&running);
        let worker_mixer = Arc::clone(&mixer);

        let worker = thread::spawn(move || {
            let result = loop {
                if worker_stop.load(Ordering::SeqCst) {
                    break Ok(sink);
                }

                if !sink.can_receive() {
                    thread::sleep(poll_interval);
                    continue;
                }

                let mixed = match worker_mixer.lock() {
                    Ok(mut mixer) => match mixer.mix_next() {
                        Ok(mixed) => mixed,
                        Err(error) => break Err(error),
                    },
                    Err(error) => break Err(AudioError::Stream(error.to_string())),
                };

                match mixed {
                    Some(frame) => {
                        if let Err(error) = sink.handle_frame(frame) {
                            break Err(error);
                        }
                    }
                    None => thread::sleep(poll_interval),
                }
            };

            worker_running.store(false, Ordering::SeqCst);
            result
        });

        Self {
            mixer,
            stop_requested,
            running,
            worker: Some(worker),
        }
    }

    pub fn mixer(&self) -> Arc<Mutex<Mixer>> {
        Arc::clone(&self.mixer)
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn stop(&mut self) -> Result<Option<S>, AudioError> {
        self.stop_requested.store(true, Ordering::SeqCst);
        let Some(worker) = self.worker.take() else {
            return Ok(None);
        };
        match worker.join() {
            Ok(result) => result.map(Some),
            Err(_) => Err(AudioError::Stream("mixer worker thread panicked".into())),
        }
    }
}

impl<S> Drop for MixerRuntime<S>
where
    S: MixerSink,
{
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[derive(Debug, Clone)]
pub struct ToneSource {
    frequency: f32,
    samplerate: u32,
    channels: u8,
    phase: f32,
    gain: f32,
    target_gain: f32,
    gain_step: f32,
    samples_per_frame: usize,
    ease: bool,
    ease_gain: f32,
    ease_step: f32,
    easing_out: bool,
    running: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CpalInputConfig {
    pub preferred_device: Option<String>,
    pub target_frame_ms: u16,
    pub codec_profile: Option<CodecProfile>,
    pub gain_db: f32,
    pub ease_in: Duration,
    pub skip: Duration,
    pub max_queued_frames: usize,
}

impl Default for CpalInputConfig {
    fn default() -> Self {
        Self {
            preferred_device: None,
            target_frame_ms: 80,
            codec_profile: None,
            gain_db: 0.0,
            ease_in: Duration::ZERO,
            skip: Duration::ZERO,
            max_queued_frames: 128,
        }
    }
}

pub struct CpalInputSource {
    samplerate: u32,
    channels: u8,
    stream: cpal::Stream,
    rx: mpsc::Receiver<AudioFrame>,
    running: bool,
    processor: LineSourceProcessor,
}

impl CpalInputSource {
    pub fn new(config: CpalInputConfig) -> Result<Self, AudioError> {
        let device = select_device(AudioDeviceKind::Input, config.preferred_device.as_deref())?;
        let supported_config = device
            .default_input_config()
            .map_err(|err| AudioError::Device(err.to_string()))?;
        let samplerate = supported_config.sample_rate();
        let channels = u8::try_from(supported_config.channels())
            .map_err(|_| AudioError::InvalidChannels(u8::MAX))?;
        let frame_plan = plan_line_source_frame(
            config.target_frame_ms as f32,
            config.codec_profile,
            samplerate,
            channels,
        )?;
        let samples_per_frame = frame_plan.sample_count;
        let (tx, rx) = mpsc::sync_channel(config.max_queued_frames.max(1));
        let stream_config = supported_config.config();
        let stream = match supported_config.sample_format() {
            cpal::SampleFormat::F32 => {
                build_input_stream::<f32>(&device, &stream_config, samples_per_frame, tx)?
            }
            cpal::SampleFormat::I16 => {
                build_input_stream::<i16>(&device, &stream_config, samples_per_frame, tx)?
            }
            cpal::SampleFormat::U16 => {
                build_input_stream::<u16>(&device, &stream_config, samples_per_frame, tx)?
            }
            other => {
                return Err(AudioError::UnsupportedDeviceFormat(other.to_string()));
            }
        };
        let processor = LineSourceProcessor::new(
            config.gain_db,
            config.ease_in,
            config.skip,
            samplerate,
            channels,
        );
        Ok(Self {
            samplerate,
            channels,
            stream,
            rx,
            running: false,
            processor,
        })
    }

    pub fn add_filter(&mut self, filter: impl AudioFilter + 'static) {
        self.processor.add_filter(filter);
    }

    pub fn gain_db(&self) -> f32 {
        self.processor.gain_db()
    }
}

impl crate::pipeline::AudioSource for CpalInputSource {
    fn start(&mut self) {
        if self.stream.play().is_ok() {
            self.running = true;
        }
    }

    fn stop(&mut self) {
        let _ = self.stream.pause();
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

    fn next_frame(&mut self) -> Result<Option<AudioFrame>, crate::pipeline::PipelineError> {
        if !self.running {
            return Ok(None);
        }
        let Ok(frame) = self.rx.try_recv() else {
            return Ok(None);
        };
        Ok(self.processor.process_frame(frame))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CpalOutputConfig {
    pub preferred_device: Option<String>,
    pub max_queued_frames: usize,
    pub low_latency: bool,
}

impl Default for CpalOutputConfig {
    fn default() -> Self {
        Self {
            preferred_device: None,
            max_queued_frames: 6,
            low_latency: false,
        }
    }
}

pub struct CpalOutputSink {
    samplerate: u32,
    channels: u8,
    sink: QueuedLineSink<CpalPlaybackBackend>,
}

impl CpalOutputSink {
    pub fn new(config: CpalOutputConfig) -> Result<Self, AudioError> {
        let device = select_device(AudioDeviceKind::Output, config.preferred_device.as_deref())?;
        let supported_config = device
            .default_output_config()
            .map_err(|err| AudioError::Device(err.to_string()))?;
        let samplerate = supported_config.sample_rate();
        let channels = u8::try_from(supported_config.channels())
            .map_err(|_| AudioError::InvalidChannels(u8::MAX))?;
        let mut stream_config = supported_config.config();
        if config.low_latency {
            stream_config.buffer_size = cpal::BufferSize::Fixed(128);
        }
        let backend = CpalPlaybackBackend::new(
            &device,
            supported_config.sample_format(),
            &stream_config,
            config.max_queued_frames.max(1),
        )?;
        let sink = QueuedLineSink::new(
            backend,
            QueuedLineSinkConfig {
                samplerate,
                channels,
                max_frames: config.max_queued_frames.max(1),
                low_latency: false,
                ..QueuedLineSinkConfig::default()
            },
        )?;
        Ok(Self {
            samplerate,
            channels,
            sink,
        })
    }

    pub fn start(&mut self) -> Result<(), AudioError> {
        self.sink.start()
    }

    pub fn stop(&mut self) -> Result<(), AudioError> {
        self.sink.stop()
    }

    pub fn is_running(&self) -> bool {
        self.sink.is_running()
    }

    pub fn samplerate(&self) -> u32 {
        self.samplerate
    }

    pub fn channels(&self) -> u8 {
        self.channels
    }

    pub fn can_receive(&self) -> bool {
        self.sink.can_receive()
    }

    pub fn queued_frames(&self) -> usize {
        self.sink.queued_frames()
    }

    pub fn handle_frame(&mut self, frame: AudioFrame) -> Result<(), AudioError> {
        self.sink.handle_frame(frame)
    }
}

impl MixerSink for CpalOutputSink {
    fn can_receive(&self) -> bool {
        CpalOutputSink::can_receive(self)
    }

    fn handle_frame(&mut self, frame: AudioFrame) -> Result<(), AudioError> {
        CpalOutputSink::handle_frame(self, frame)
    }
}

struct CpalPlaybackBackend {
    stream: cpal::Stream,
    queue: Arc<Mutex<VecDeque<AudioFrame>>>,
    max_queued_frames: usize,
}

impl CpalPlaybackBackend {
    fn new(
        device: &cpal::Device,
        sample_format: cpal::SampleFormat,
        stream_config: &cpal::StreamConfig,
        max_queued_frames: usize,
    ) -> Result<Self, AudioError> {
        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(max_queued_frames)));
        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                build_output_stream::<f32>(device, stream_config, queue.clone())?
            }
            cpal::SampleFormat::I16 => {
                build_output_stream::<i16>(device, stream_config, queue.clone())?
            }
            cpal::SampleFormat::U16 => {
                build_output_stream::<u16>(device, stream_config, queue.clone())?
            }
            other => return Err(AudioError::UnsupportedDeviceFormat(other.to_string())),
        };
        Ok(Self {
            stream,
            queue,
            max_queued_frames,
        })
    }
}

impl LinePlayback for CpalPlaybackBackend {
    fn start(&mut self) -> Result<(), AudioError> {
        self.stream
            .play()
            .map_err(|err| AudioError::Stream(err.to_string()))
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        self.stream
            .pause()
            .map_err(|err| AudioError::Stream(err.to_string()))
    }

    fn play(&mut self, frame: AudioFrame) -> Result<(), AudioError> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|err| AudioError::Stream(err.to_string()))?;
        if queue.len() >= self.max_queued_frames {
            queue.pop_front();
        }
        queue.push_back(frame);
        Ok(())
    }
}

impl ToneSource {
    pub fn new(frequency: f32, samplerate: u32, channels: u8, gain: f32) -> Self {
        Self::with_frame_ms(frequency, samplerate, channels, gain, 80)
    }

    pub fn with_frame_ms(
        frequency: f32,
        samplerate: u32,
        channels: u8,
        gain: f32,
        target_frame_ms: u16,
    ) -> Self {
        let samples_per_frame =
            ((samplerate as u64 * target_frame_ms.max(1) as u64).div_ceil(1000)) as usize;
        Self::with_samples_per_frame(frequency, samplerate, channels, gain, samples_per_frame)
    }

    pub fn with_frame_plan(frequency: f32, gain: f32, plan: AudioFramePlan) -> Self {
        Self::with_samples_per_frame(
            frequency,
            plan.samplerate,
            plan.channels,
            gain,
            plan.frame_count,
        )
    }

    pub fn with_codec_profile(
        frequency: f32,
        channels: u8,
        gain: f32,
        target_frame_ms: u16,
        codec_profile: CodecProfile,
    ) -> Result<Self, AudioError> {
        let info = codec_profile.info();
        let samplerate = if info.samplerate == 0 {
            48_000
        } else {
            info.samplerate
        };
        let channels = if info.channels == 0 {
            channels
        } else {
            info.channels
        };
        let plan = plan_line_source_frame(
            target_frame_ms as f32,
            Some(codec_profile),
            samplerate,
            channels,
        )?;
        Ok(Self::with_frame_plan(frequency, gain, plan))
    }

    fn with_samples_per_frame(
        frequency: f32,
        samplerate: u32,
        channels: u8,
        gain: f32,
        samples_per_frame: usize,
    ) -> Self {
        let ease_time = Duration::from_millis(20);
        let ease_samples = duration_samples(ease_time, samplerate, 1).max(1);
        Self {
            frequency,
            samplerate,
            channels,
            phase: 0.0,
            gain,
            target_gain: gain,
            gain_step: 0.02 / ease_samples as f32,
            samples_per_frame,
            ease: true,
            ease_gain: 0.0,
            ease_step: 1.0 / ease_samples as f32,
            easing_out: false,
            running: false,
        }
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.target_gain = gain;
    }

    pub fn set_ease(&mut self, enabled: bool) {
        self.ease = enabled;
    }

    pub fn next_frame(&mut self, frames: usize) -> Result<AudioFrame, AudioError> {
        self.generate_frame(frames, false)
    }

    fn generate_frame(
        &mut self,
        frames: usize,
        apply_ease: bool,
    ) -> Result<AudioFrame, AudioError> {
        let mut samples = Vec::with_capacity(frames * self.channels as usize);
        let step = 2.0 * PI * self.frequency / self.samplerate as f32;
        for _ in 0..frames {
            let ease_gain = if apply_ease && self.ease {
                self.ease_gain
            } else {
                1.0
            };
            self.phase = (self.phase + step) % (2.0 * PI);
            let value = self.phase.sin() * self.gain * ease_gain;
            for _ in 0..self.channels {
                samples.push(value);
            }

            if apply_ease {
                if self.gain < self.target_gain {
                    self.gain = (self.gain + self.gain_step).min(self.target_gain);
                } else if self.gain > self.target_gain {
                    self.gain = (self.gain - self.gain_step).max(self.target_gain);
                }

                if self.ease {
                    if self.easing_out {
                        self.ease_gain = (self.ease_gain - self.ease_step).max(0.0);
                        if self.ease_gain == 0.0 {
                            self.easing_out = false;
                            self.running = false;
                        }
                    } else if self.ease_gain < 1.0 {
                        self.ease_gain = (self.ease_gain + self.ease_step).min(1.0);
                    }
                }
            }
        }
        AudioFrame::new(self.samplerate, self.channels, samples)
    }
}

impl crate::pipeline::AudioSource for ToneSource {
    fn start(&mut self) {
        self.ease_gain = if self.ease { 0.0 } else { 1.0 };
        self.easing_out = false;
        self.running = true;
    }

    fn stop(&mut self) {
        if self.ease {
            self.easing_out = true;
        } else {
            self.running = false;
        }
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

    fn next_frame(&mut self) -> Result<Option<AudioFrame>, crate::pipeline::PipelineError> {
        if !self.running {
            return Ok(None);
        }
        Ok(Some(self.generate_frame(self.samples_per_frame, true)?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDeviceKind {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub kind: AudioDeviceKind,
    pub is_default: bool,
    pub default_config: Option<AudioStreamConfigInfo>,
    pub supported_configs: Vec<AudioStreamConfigInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStreamConfigInfo {
    pub channels: u16,
    pub sample_format: String,
    pub min_sample_rate: u32,
    pub max_sample_rate: u32,
    pub buffer_size: Option<(u32, u32)>,
}

pub fn list_audio_devices() -> Result<Vec<AudioDeviceInfo>, AudioError> {
    let host = cpal::default_host();
    let default_input = host.default_input_device().map(|device| device.to_string());
    let default_output = host
        .default_output_device()
        .map(|device| device.to_string());

    let mut devices = Vec::new();
    let input_devices = host
        .input_devices()
        .map_err(|err| AudioError::Device(err.to_string()))?;
    for device in input_devices {
        devices.push(device_info(
            &device,
            AudioDeviceKind::Input,
            default_input.as_deref(),
        ));
    }

    let output_devices = host
        .output_devices()
        .map_err(|err| AudioError::Device(err.to_string()))?;
    for device in output_devices {
        devices.push(device_info(
            &device,
            AudioDeviceKind::Output,
            default_output.as_deref(),
        ));
    }

    Ok(devices)
}

pub fn select_audio_device_info(
    devices: &[AudioDeviceInfo],
    kind: AudioDeviceKind,
    preferred_name: Option<&str>,
) -> Option<AudioDeviceInfo> {
    let candidates = devices.iter().filter(|device| device.kind == kind);
    if let Some(preferred_name) = preferred_name {
        if let Some(device) = candidates
            .clone()
            .find(|device| device_name_matches(preferred_name, &device.name))
        {
            return Some(device.clone());
        }
    }
    candidates
        .clone()
        .find(|device| device.is_default)
        .or_else(|| candidates.into_iter().next())
        .cloned()
}

fn device_info(
    device: &cpal::Device,
    kind: AudioDeviceKind,
    default_name: Option<&str>,
) -> AudioDeviceInfo {
    let name = device.to_string();
    let default_config = match kind {
        AudioDeviceKind::Input => device.default_input_config().ok(),
        AudioDeviceKind::Output => device.default_output_config().ok(),
    }
    .map(|config| AudioStreamConfigInfo {
        channels: config.channels(),
        sample_format: config.sample_format().to_string(),
        min_sample_rate: config.sample_rate(),
        max_sample_rate: config.sample_rate(),
        buffer_size: None,
    });

    let supported_configs = match kind {
        AudioDeviceKind::Input => device
            .supported_input_configs()
            .map(|configs| configs.map(stream_config_info).collect())
            .unwrap_or_default(),
        AudioDeviceKind::Output => device
            .supported_output_configs()
            .map(|configs| configs.map(stream_config_info).collect())
            .unwrap_or_default(),
    };

    AudioDeviceInfo {
        is_default: default_name == Some(name.as_str()),
        name,
        kind,
        default_config,
        supported_configs,
    }
}

fn stream_config_info(config: cpal::SupportedStreamConfigRange) -> AudioStreamConfigInfo {
    let buffer_size = match config.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
        cpal::SupportedBufferSize::Unknown => None,
    };
    AudioStreamConfigInfo {
        channels: config.channels(),
        sample_format: config.sample_format().to_string(),
        min_sample_rate: config.min_sample_rate(),
        max_sample_rate: config.max_sample_rate(),
        buffer_size,
    }
}

fn select_device(
    kind: AudioDeviceKind,
    preferred_name: Option<&str>,
) -> Result<cpal::Device, AudioError> {
    let host = cpal::default_host();
    if let Some(preferred_name) = preferred_name {
        let devices = match kind {
            AudioDeviceKind::Input => host.input_devices(),
            AudioDeviceKind::Output => host.output_devices(),
        }
        .map_err(|err| AudioError::Device(err.to_string()))?;
        for device in devices {
            if device_name_matches(preferred_name, &device.to_string()) {
                return Ok(device);
            }
        }
    }
    match kind {
        AudioDeviceKind::Input => host.default_input_device(),
        AudioDeviceKind::Output => host.default_output_device(),
    }
    .ok_or(AudioError::NoDevice(kind))
}

fn device_name_matches(preferred_name: &str, candidate_name: &str) -> bool {
    candidate_name == preferred_name
        || candidate_name.contains(preferred_name)
        || is_subsequence(preferred_name, candidate_name)
}

fn is_subsequence(pattern: &str, candidate: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    let mut pattern = pattern.chars();
    let Some(mut wanted) = pattern.next() else {
        return true;
    };
    for ch in candidate.chars() {
        if ch == wanted {
            match pattern.next() {
                Some(next) => wanted = next,
                None => return true,
            }
        }
    }
    false
}

fn build_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples_per_frame: usize,
    tx: mpsc::SyncSender<AudioFrame>,
) -> Result<cpal::Stream, AudioError>
where
    T: cpal::SizedSample + CpalSampleToF32,
{
    let samplerate = config.sample_rate;
    let channels =
        u8::try_from(config.channels).map_err(|_| AudioError::InvalidChannels(u8::MAX))?;
    let pending = Arc::new(Mutex::new(Vec::with_capacity(samples_per_frame)));
    let callback_pending = pending.clone();
    device
        .build_input_stream(
            *config,
            move |data: &[T], _| {
                let Ok(mut pending) = callback_pending.lock() else {
                    return;
                };
                for sample in data {
                    pending.push(sample.to_f32_sample());
                    if pending.len() >= samples_per_frame {
                        let frame_samples: Vec<f32> = pending.drain(..samples_per_frame).collect();
                        if let Ok(frame) = AudioFrame::new(samplerate, channels, frame_samples) {
                            let _ = tx.try_send(frame);
                        }
                    }
                }
            },
            move |err| {
                eprintln!("lxst input stream error: {err}");
            },
            None,
        )
        .map_err(|err| AudioError::Stream(err.to_string()))
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: Arc<Mutex<VecDeque<AudioFrame>>>,
) -> Result<cpal::Stream, AudioError>
where
    T: cpal::SizedSample + CpalSampleFromF32,
{
    let channels = usize::from(config.channels);
    let mut pending = VecDeque::<f32>::new();
    device
        .build_output_stream(
            *config,
            move |data: &mut [T], _| {
                for sample in data.iter_mut() {
                    if pending.is_empty() {
                        if let Ok(mut queue) = queue.lock() {
                            if let Some(frame) = queue.pop_front() {
                                pending.extend(frame.samples().iter().copied());
                            }
                        }
                    }
                    let value = pending.pop_front().unwrap_or(0.0);
                    *sample = T::from_f32_sample(value);
                }
                let remainder = data.len() % channels;
                if remainder != 0 {
                    let start = data.len() - remainder;
                    for sample in &mut data[start..] {
                        *sample = T::from_f32_sample(0.0);
                    }
                }
            },
            move |err| {
                eprintln!("lxst output stream error: {err}");
            },
            None,
        )
        .map_err(|err| AudioError::Stream(err.to_string()))
}

trait CpalSampleToF32 {
    fn to_f32_sample(&self) -> f32;
}

impl CpalSampleToF32 for f32 {
    fn to_f32_sample(&self) -> f32 {
        *self
    }
}

impl CpalSampleToF32 for i16 {
    fn to_f32_sample(&self) -> f32 {
        *self as f32 / i16::MAX as f32
    }
}

impl CpalSampleToF32 for u16 {
    fn to_f32_sample(&self) -> f32 {
        (*self as f32 - 32768.0) / 32768.0
    }
}

trait CpalSampleFromF32 {
    fn from_f32_sample(sample: f32) -> Self;
}

impl CpalSampleFromF32 for f32 {
    fn from_f32_sample(sample: f32) -> Self {
        sample.clamp(-1.0, 1.0)
    }
}

impl CpalSampleFromF32 for i16 {
    fn from_f32_sample(sample: f32) -> Self {
        (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    }
}

impl CpalSampleFromF32 for u16 {
    fn from_f32_sample(sample: f32) -> Self {
        ((sample.clamp(-1.0, 1.0) * 32767.0) + 32768.0) as u16
    }
}

fn duration_samples(duration: Duration, samplerate: u32, channels: u8) -> usize {
    ((duration.as_secs_f64() * samplerate as f64).ceil() as usize) * channels as usize
}

fn linear_gain(gain_db: f32) -> f32 {
    if gain_db == 0.0 {
        1.0
    } else {
        10.0_f32.powf(gain_db / 10.0)
    }
}

fn normalize_audio_channels(
    samples: &[f32],
    input_channels: u8,
    output_channels: u8,
) -> Result<Vec<f32>, AudioError> {
    if input_channels == 0 {
        return Err(AudioError::InvalidChannels(input_channels));
    }
    if output_channels == 0 {
        return Err(AudioError::InvalidChannels(output_channels));
    }
    let input_channels = input_channels as usize;
    let output_channels = output_channels as usize;
    let frames = samples.len() / input_channels;
    let mut normalized = Vec::with_capacity(frames * output_channels);
    for frame in 0..frames {
        let base = frame * input_channels;
        for channel in 0..output_channels {
            normalized.push(samples[base + channel.min(input_channels - 1)]);
        }
    }
    Ok(normalized)
}

fn resample_audio_linear(
    samples: &[f32],
    channels: u8,
    input_rate: u32,
    output_rate: u32,
) -> Result<Vec<f32>, AudioError> {
    if input_rate == 0 {
        return Err(AudioError::InvalidSamplerate(input_rate));
    }
    if output_rate == 0 {
        return Err(AudioError::InvalidSamplerate(output_rate));
    }
    if input_rate == output_rate {
        return Ok(samples.to_vec());
    }
    let channels = channels as usize;
    if samples.is_empty() {
        return Ok(Vec::new());
    }
    let input_frames = samples.len() / channels;
    let output_frames =
        ((input_frames as u64 * output_rate as u64) + input_rate as u64 / 2) / input_rate as u64;
    let output_frames = output_frames.max(1) as usize;
    let mut out = Vec::with_capacity(output_frames * channels);
    for out_frame in 0..output_frames {
        let position = out_frame as f64 * input_rate as f64 / output_rate as f64;
        let left = position.floor() as usize;
        let right = (left + 1).min(input_frames - 1);
        let frac = (position - left as f64) as f32;
        for channel in 0..channels {
            let a = samples[left * channels + channel];
            let b = samples[right * channels + channel];
            out.push(a + (b - a) * frac);
        }
    }
    Ok(out)
}
