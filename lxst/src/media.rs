use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use lxst_core::CodecProfile;
use ogg::{PacketReader, PacketWriteEndInfo, PacketWriter};

use crate::audio::{
    Agc, AudioError, AudioFrame, BandPass, CpalInputConfig, CpalInputSource, CpalOutputConfig,
    CpalOutputSink,
};
use crate::codec::{AudioCodec, CodecError, OpusCodec};
use crate::pipeline::{AudioSource, PipelineError};

const OPUS_SERIAL: u32 = 0x4c58_5354;
const OPUS_PRESKIP: u16 = 312;
const OPUS_GRANULE_RATE: u32 = 48_000;
const OPUS_FINAL_SILENCE_FRAMES: usize = 10;

pub struct OpusFileSink {
    path: PathBuf,
    profile: CodecProfile,
    codec: OpusCodec,
    writer: PacketWriter<'static, File>,
    samples_written_48k: u64,
    last_output_frame_samples: Option<usize>,
    finalized: bool,
}

impl OpusFileSink {
    pub fn create(path: impl AsRef<Path>, profile: CodecProfile) -> Result<Self, MediaError> {
        let info = OpusCodec::profile_info(profile)?;
        let file = File::create(path.as_ref())?;
        let mut writer = PacketWriter::new(file);
        writer.write_packet(
            opus_head(info.channels, info.samplerate),
            OPUS_SERIAL,
            PacketWriteEndInfo::EndPage,
            0,
        )?;
        writer.write_packet(opus_tags(), OPUS_SERIAL, PacketWriteEndInfo::EndPage, 0)?;
        Ok(Self {
            path: path.as_ref().to_path_buf(),
            profile,
            codec: OpusCodec::new(profile),
            writer,
            samples_written_48k: 0,
            last_output_frame_samples: None,
            finalized: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn profile(&self) -> CodecProfile {
        self.profile
    }

    pub fn handle_frame(&mut self, frame: &AudioFrame) -> Result<(), MediaError> {
        self.write_frame(frame, true)
    }

    fn write_frame(
        &mut self,
        frame: &AudioFrame,
        remember_duration: bool,
    ) -> Result<(), MediaError> {
        let info = OpusCodec::profile_info(self.profile)?;
        let encoded = self.codec.encode(frame)?;
        let output_frame_count = frame.resampled(info.samplerate)?.frame_count();
        let frame_samples = output_frame_count.saturating_mul(OPUS_GRANULE_RATE as usize)
            / info.samplerate as usize;
        if remember_duration {
            self.last_output_frame_samples = Some(output_frame_count);
        }
        self.samples_written_48k += frame_samples as u64;
        self.writer.write_packet(
            encoded,
            OPUS_SERIAL,
            PacketWriteEndInfo::NormalPacket,
            self.samples_written_48k + OPUS_PRESKIP as u64,
        )?;
        Ok(())
    }

    pub fn finalize(&mut self) -> Result<(), MediaError> {
        if !self.finalized {
            if let Some(frame_samples) = self.last_output_frame_samples {
                let info = OpusCodec::profile_info(self.profile)?;
                let silence = AudioFrame::silence(info.samplerate, info.channels, frame_samples)?;
                for _ in 0..OPUS_FINAL_SILENCE_FRAMES {
                    self.write_frame(&silence, false)?;
                }
            }
            self.writer.write_packet(
                Vec::<u8>::new(),
                OPUS_SERIAL,
                PacketWriteEndInfo::EndStream,
                self.samples_written_48k + OPUS_PRESKIP as u64,
            )?;
            self.finalized = true;
        }
        Ok(())
    }
}

impl Drop for OpusFileSink {
    fn drop(&mut self) {
        let _ = self.finalize();
    }
}

#[derive(Debug, Clone)]
pub struct OpusFileSource {
    samplerate: u32,
    channels: u8,
    samples: Vec<f32>,
    samples_per_frame: usize,
    position: usize,
    running: bool,
    looping: bool,
}

impl OpusFileSource {
    pub fn open(
        path: impl AsRef<Path>,
        target_frame_ms: u16,
        looping: bool,
    ) -> Result<Self, MediaError> {
        let file = File::open(path.as_ref())?;
        let mut reader = PacketReader::new(file);
        let head = reader.read_packet_expected()?;
        let header = OpusHeader::parse(&head.data)?;
        let tags = reader.read_packet_expected()?;
        if !tags.data.starts_with(b"OpusTags") {
            return Err(MediaError::InvalidOpusFile("missing OpusTags packet"));
        }

        let profile = opus_file_profile(header.channels, header.input_samplerate);
        let mut codec = OpusCodec::new(profile);
        let mut samples = Vec::new();
        while let Some(packet) = reader.read_packet()? {
            if packet.data.is_empty() {
                continue;
            }
            let decoded = codec.decode(&packet.data, header.input_samplerate)?;
            samples.extend_from_slice(decoded.samples());
        }
        let preskip = header.preskip as usize * header.channels as usize;
        if samples.len() > preskip {
            samples.drain(..preskip);
        }
        let samples_per_frame = samples_per_frame(header.input_samplerate, target_frame_ms).max(1);
        Ok(Self {
            samplerate: header.input_samplerate,
            channels: header.channels,
            samples,
            samples_per_frame,
            position: 0,
            running: false,
            looping,
        })
    }

    pub fn len_samples(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.len_samples() as f64 / self.samplerate as f64)
    }
}

impl AudioSource for OpusFileSource {
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
        if !self.running {
            return Ok(None);
        }
        if self.position >= self.len_samples() {
            if self.looping {
                self.position = 0;
            } else {
                self.running = false;
                return Ok(None);
            }
        }
        let channels = self.channels as usize;
        let start = self.position * channels;
        let end_frame = (self.position + self.samples_per_frame).min(self.len_samples());
        let end = end_frame * channels;
        self.position = end_frame;
        Ok(Some(AudioFrame::new(
            self.samplerate,
            self.channels,
            self.samples[start..end].to_vec(),
        )?))
    }
}

pub struct FileRecorder {
    source: CpalInputSource,
    sink: OpusFileSink,
}

impl FileRecorder {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, MediaError> {
        let mut source = CpalInputSource::new(CpalInputConfig {
            target_frame_ms: 20,
            ease_in: Duration::from_millis(125),
            skip: Duration::from_millis(75),
            ..CpalInputConfig::default()
        })?;
        source.add_filter(BandPass::new(25.0, 24_000.0)?);
        source.add_filter(Agc::new(-12.0, 12.0));
        let sink = OpusFileSink::create(path, CodecProfile::OpusAudioMax)?;
        Ok(Self { source, sink })
    }

    pub fn start(&mut self) {
        self.source.start();
    }

    pub fn stop(&mut self) -> Result<(), MediaError> {
        self.source.stop();
        self.sink.finalize()
    }

    pub fn process_next(&mut self) -> Result<bool, MediaError> {
        let Some(frame) = self.source.next_frame()? else {
            return Ok(false);
        };
        self.sink.handle_frame(&frame)?;
        Ok(true)
    }
}

pub struct FilePlayer {
    source: OpusFileSource,
    sink: CpalOutputSink,
}

impl FilePlayer {
    pub fn new(path: impl AsRef<Path>, looping: bool) -> Result<Self, MediaError> {
        let source = OpusFileSource::open(path, 100, looping)?;
        let sink = CpalOutputSink::new(CpalOutputConfig::default())?;
        Ok(Self { source, sink })
    }

    pub fn start(&mut self) -> Result<(), MediaError> {
        self.source.start();
        self.sink.start()?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), MediaError> {
        self.source.stop();
        self.sink.stop()?;
        Ok(())
    }

    pub fn process_next(&mut self) -> Result<bool, MediaError> {
        if !self.sink.can_receive() {
            return Ok(false);
        }
        let Some(frame) = self.source.next_frame()? else {
            return Ok(false);
        };
        self.sink.handle_frame(frame)?;
        Ok(true)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    OggRead(#[from] ogg::OggReadError),
    #[error("invalid Ogg Opus file: {0}")]
    InvalidOpusFile(&'static str),
}

fn opus_head(channels: u8, input_samplerate: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity(19);
    data.extend_from_slice(b"OpusHead");
    data.push(1);
    data.push(channels);
    data.extend_from_slice(&OPUS_PRESKIP.to_le_bytes());
    data.extend_from_slice(&input_samplerate.to_le_bytes());
    data.extend_from_slice(&0i16.to_le_bytes());
    data.push(0);
    data
}

fn opus_tags() -> Vec<u8> {
    let vendor = b"lxst-rs";
    let mut data = Vec::new();
    data.extend_from_slice(b"OpusTags");
    data.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    data.extend_from_slice(vendor);
    data.extend_from_slice(&0u32.to_le_bytes());
    data
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpusHeader {
    channels: u8,
    preskip: u16,
    input_samplerate: u32,
}

impl OpusHeader {
    fn parse(data: &[u8]) -> Result<Self, MediaError> {
        if data.len() < 19 || !data.starts_with(b"OpusHead") {
            return Err(MediaError::InvalidOpusFile("missing OpusHead packet"));
        }
        let channels = data[9];
        let mut cursor = Cursor::new(&data[10..]);
        let mut preskip = [0u8; 2];
        let mut samplerate = [0u8; 4];
        cursor.read_exact(&mut preskip)?;
        cursor.read_exact(&mut samplerate)?;
        Ok(Self {
            channels,
            preskip: u16::from_le_bytes(preskip),
            input_samplerate: u32::from_le_bytes(samplerate),
        })
    }
}

fn opus_file_profile(channels: u8, samplerate: u32) -> CodecProfile {
    match (channels, samplerate) {
        (1, 8_000) => CodecProfile::OpusAudioMin,
        (1, 12_000) => CodecProfile::OpusAudioLow,
        (2, 24_000) => CodecProfile::OpusAudioMedium,
        (2, 48_000) => CodecProfile::OpusAudioMax,
        (1, 24_000) => CodecProfile::OpusVoiceMedium,
        (1, 48_000) => CodecProfile::OpusVoiceHigh,
        _ => CodecProfile::OpusAudioMax,
    }
}

fn samples_per_frame(samplerate: u32, target_frame_ms: u16) -> usize {
    ((samplerate as u64 * target_frame_ms.max(1) as u64).div_ceil(1000)) as usize
}
