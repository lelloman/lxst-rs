use lxst::{AudioSink, AudioSource, BufferedSink, BufferedSource, Pipeline, RawBitDepth, RawCodec};
use lxst_core::CodecKind;

#[test]
fn buffered_source_obeys_running_state() {
    let mut source = BufferedSource::new(8_000, 1).unwrap();
    source
        .push_frame(lxst::AudioFrame::new(8_000, 1, vec![0.25; 80]).unwrap())
        .unwrap();

    assert!(source.next_frame().unwrap().is_none());
    source.start();
    assert!(source.next_frame().unwrap().is_some());
    assert!(source.next_frame().unwrap().is_none());
}

#[test]
fn pipeline_encodes_source_frames_into_sink() {
    let mut source = BufferedSource::new(8_000, 1).unwrap();
    source
        .push_frame(lxst::AudioFrame::new(8_000, 1, vec![0.0, 0.5]).unwrap())
        .unwrap();
    let sink = BufferedSink::new(4);
    let mut pipeline = Pipeline::new(
        Box::new(source),
        Box::new(RawCodec::new(RawBitDepth::Float32)),
        Box::new(sink),
    );

    pipeline.start();
    assert!(pipeline.process_next().unwrap());
    assert!(!pipeline.process_next().unwrap());
    assert!(pipeline.is_running());
    pipeline.stop();
    assert!(!pipeline.is_running());
}

#[test]
fn buffered_sink_applies_backpressure() {
    let mut sink = BufferedSink::new(1);
    assert!(sink.can_receive());
    sink.handle_frame(lxst::EncodedAudioFrame {
        codec: CodecKind::Raw,
        samplerate: 8_000,
        channels: 1,
        payload: vec![0],
    })
    .unwrap();
    assert!(!sink.can_receive());
}
