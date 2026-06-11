use std::fs;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lxst::{
    AudioFrame, AudioSource, MediaError, OpusFileSink, OpusFileSource, QueuedOpusFileSink,
    QueuedOpusFileSinkConfig, SourceRecorder,
};
use lxst_core::CodecProfile;

#[test]
fn ogg_opus_file_sink_and_source_round_trip_audio() {
    let path = temp_opus_path("round-trip");
    let samples: Vec<f32> = (0..960)
        .flat_map(|n| {
            let sample = ((n as f32 / 48_000.0) * 440.0 * std::f32::consts::TAU).sin() * 0.2;
            [sample, sample]
        })
        .collect();
    let frame = AudioFrame::new(48_000, 2, samples).unwrap();

    {
        let mut sink = OpusFileSink::create(&path, CodecProfile::OpusAudioMax).unwrap();
        sink.handle_frame(&frame).unwrap();
        sink.finalize().unwrap();
    }

    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"OggS"));
    assert!(bytes.windows("OpusHead".len()).any(|w| w == b"OpusHead"));

    let mut source = OpusFileSource::open(&path, 20, false).unwrap();
    assert_eq!(source.samplerate(), 48_000);
    assert_eq!(source.channels(), 2);
    assert!(source.duration().as_millis() > 0);
    source.start();
    let decoded = source.next_frame().unwrap().unwrap();
    assert_eq!(decoded.samplerate(), 48_000);
    assert_eq!(decoded.channels(), 2);
    assert!(decoded.frame_count() > 0);

    let _ = fs::remove_file(path);
}

#[test]
fn ogg_opus_file_sink_writes_final_silence_padding() {
    let path = temp_opus_path("padding");
    let frame = AudioFrame::new(48_000, 2, vec![0.0; 960 * 2]).unwrap();

    {
        let mut sink = OpusFileSink::create(&path, CodecProfile::OpusAudioMax).unwrap();
        sink.handle_frame(&frame).unwrap();
        sink.finalize().unwrap();
    }

    let source = OpusFileSource::open(&path, 20, false).unwrap();
    assert!(source.len_samples() >= 960 * 10);

    let _ = fs::remove_file(path);
}

#[test]
fn queued_opus_file_sink_applies_backpressure_before_autodigest() {
    let path = temp_opus_path("queued-backpressure");
    let frame = AudioFrame::new(48_000, 2, vec![0.0; 960 * 2]).unwrap();
    let mut sink = QueuedOpusFileSink::create(
        &path,
        QueuedOpusFileSinkConfig {
            max_queued_frames: 1,
            autodigest: false,
            ..QueuedOpusFileSinkConfig::default()
        },
    )
    .unwrap();

    assert!(sink.can_receive());
    sink.handle_frame(frame.clone()).unwrap();
    assert_eq!(sink.frames_waiting(), 1);
    assert!(!sink.can_receive());
    assert!(matches!(
        sink.handle_frame(frame),
        Err(MediaError::SinkFull)
    ));

    let _ = fs::remove_file(path);
}

#[test]
fn queued_opus_file_sink_drains_and_finalizes_on_stop() {
    let path = temp_opus_path("queued-drain");
    let frame = AudioFrame::new(48_000, 2, vec![0.0; 960 * 2]).unwrap();
    let mut sink = QueuedOpusFileSink::create(
        &path,
        QueuedOpusFileSinkConfig {
            finalize_timeout: Duration::from_secs(2),
            ..QueuedOpusFileSinkConfig::default()
        },
    )
    .unwrap();

    sink.handle_frame(frame.clone()).unwrap();
    sink.handle_frame(frame).unwrap();
    sink.stop().unwrap();

    assert!(sink.is_finalized());
    assert!(!sink.can_receive());
    assert!(matches!(
        sink.handle_frame(AudioFrame::silence(48_000, 2, 960).unwrap()),
        Err(MediaError::SinkClosed)
    ));

    let source = OpusFileSource::open(&path, 20, false).unwrap();
    assert!(source.len_samples() >= 960 * 10);

    let _ = fs::remove_file(path);
}

#[test]
fn opus_file_source_timed_mode_waits_between_frames() {
    let path = temp_opus_path("timed-source");
    let frame = AudioFrame::new(48_000, 2, vec![0.0; 960 * 2]).unwrap();

    {
        let mut sink = OpusFileSink::create(&path, CodecProfile::OpusAudioMax).unwrap();
        sink.handle_frame(&frame).unwrap();
        sink.handle_frame(&frame).unwrap();
        sink.finalize().unwrap();
    }

    let mut source = OpusFileSource::open_timed(&path, 20, false, true).unwrap();
    assert!(source.timed());
    assert_eq!(source.frame_time(), Duration::from_millis(20));

    source.start();
    assert!(source.next_frame().unwrap().is_some());
    assert!(source.next_frame().unwrap().is_none());

    thread::sleep(source.frame_time() + Duration::from_millis(5));
    assert!(source.next_frame().unwrap().is_some());

    let _ = fs::remove_file(path);
}

#[test]
fn source_recorder_obeys_sink_backpressure_before_pulling() {
    let path = temp_opus_path("source-recorder-backpressure");
    let frame = AudioFrame::new(48_000, 2, vec![0.0; 960 * 2]).unwrap();
    let source = FakeMediaSource::new(48_000, 2, vec![frame.clone(), frame]);
    let mut recorder = SourceRecorder::create(
        source,
        &path,
        QueuedOpusFileSinkConfig {
            max_queued_frames: 1,
            autodigest: false,
            ..QueuedOpusFileSinkConfig::default()
        },
    )
    .unwrap();

    recorder.start();
    assert!(recorder.process_next().unwrap());
    assert_eq!(recorder.source().pulls, 1);
    assert_eq!(recorder.frames_waiting(), 1);
    assert!(!recorder.can_receive());

    assert!(!recorder.process_next().unwrap());
    assert_eq!(recorder.source().pulls, 1);

    let _ = fs::remove_file(path);
}

#[test]
fn source_recorder_records_source_to_opus_file() {
    let path = temp_opus_path("source-recorder-file");
    let frame = AudioFrame::new(48_000, 2, vec![0.0; 960 * 2]).unwrap();
    let source = FakeMediaSource::new(48_000, 2, vec![frame]);
    let mut recorder = SourceRecorder::create(
        source,
        &path,
        QueuedOpusFileSinkConfig {
            finalize_timeout: Duration::from_secs(2),
            ..QueuedOpusFileSinkConfig::default()
        },
    )
    .unwrap();

    recorder.start();
    assert!(recorder.is_recording());
    assert!(recorder.process_next().unwrap());
    recorder.stop().unwrap();
    assert!(!recorder.is_recording());

    let source = OpusFileSource::open(&path, 20, false).unwrap();
    assert!(source.len_samples() >= 960 * 10);

    let _ = fs::remove_file(path);
}

struct FakeMediaSource {
    samplerate: u32,
    channels: u8,
    frames: std::collections::VecDeque<AudioFrame>,
    running: bool,
    pulls: usize,
}

impl FakeMediaSource {
    fn new(samplerate: u32, channels: u8, frames: Vec<AudioFrame>) -> Self {
        Self {
            samplerate,
            channels,
            frames: frames.into(),
            running: false,
            pulls: 0,
        }
    }
}

impl AudioSource for FakeMediaSource {
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

    fn next_frame(&mut self) -> Result<Option<AudioFrame>, lxst::PipelineError> {
        if !self.running {
            return Ok(None);
        }
        self.pulls += 1;
        Ok(self.frames.pop_front())
    }
}

fn temp_opus_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "lxst-rs-media-{name}-{}-{}.opus",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
