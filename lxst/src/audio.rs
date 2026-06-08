use std::collections::{HashMap, VecDeque};
use std::f32::consts::PI;

use cpal::traits::{DeviceTrait, HostTrait};

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
    #[error("audio device error: {0}")]
    Device(String),
}

pub trait AudioFilter: Send {
    fn process(&mut self, frame: AudioFrame) -> AudioFrame;
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

#[derive(Debug, Clone)]
pub struct ToneSource {
    frequency: f32,
    samplerate: u32,
    channels: u8,
    phase: f32,
    gain: f32,
}

impl ToneSource {
    pub fn new(frequency: f32, samplerate: u32, channels: u8, gain: f32) -> Self {
        Self {
            frequency,
            samplerate,
            channels,
            phase: 0.0,
            gain,
        }
    }

    pub fn next_frame(&mut self, frames: usize) -> Result<AudioFrame, AudioError> {
        let mut samples = Vec::with_capacity(frames * self.channels as usize);
        let step = 2.0 * PI * self.frequency / self.samplerate as f32;
        for _ in 0..frames {
            let value = self.phase.sin() * self.gain;
            self.phase = (self.phase + step) % (2.0 * PI);
            for _ in 0..self.channels {
                samples.push(value);
            }
        }
        AudioFrame::new(self.samplerate, self.channels, samples)
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
