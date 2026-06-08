use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use lxst::{AudioFrame, AudioSource, OpusFileSink, OpusFileSource};
use lxst_core::CodecProfile;

#[test]
fn ogg_opus_file_sink_and_source_round_trip_audio() {
    let path = std::env::temp_dir().join(format!(
        "lxst-rs-media-{}-{}.opus",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
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
