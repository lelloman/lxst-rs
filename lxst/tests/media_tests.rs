use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lxst::{
    AudioFrame, AudioSource, MediaError, OpusFileSink, OpusFileSource, QueuedOpusFileSink,
    QueuedOpusFileSinkConfig,
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
