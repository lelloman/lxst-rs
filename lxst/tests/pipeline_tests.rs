use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use lxst::{
    AudioSink, AudioSource, BufferedSink, BufferedSource, EncodedAudioFrame, Pipeline,
    PipelineError, PipelineRunner, RawBitDepth, RawCodec,
};
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

#[test]
fn pipeline_runner_drains_source_frames_until_stopped() {
    let mut source = BufferedSource::new(8_000, 1).unwrap();
    source
        .push_frame(lxst::AudioFrame::new(8_000, 1, vec![0.0, 0.25]).unwrap())
        .unwrap();
    source
        .push_frame(lxst::AudioFrame::new(8_000, 1, vec![0.5, 0.75]).unwrap())
        .unwrap();

    let frames = Arc::new(Mutex::new(Vec::new()));
    let sink = CollectingSink {
        frames: Arc::clone(&frames),
    };
    let pipeline = Pipeline::new(
        Box::new(source),
        Box::new(RawCodec::new(RawBitDepth::Float32)),
        Box::new(sink),
    );
    let mut runner = PipelineRunner::start(pipeline, Duration::from_millis(1));

    wait_for(|| frames.lock().unwrap().len() == 2);

    runner.stop().unwrap();
    runner.stop().unwrap();
    assert!(!runner.is_running());

    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 2);
    assert!(frames.iter().all(|frame| frame.codec == CodecKind::Raw));
}

#[test]
fn pipeline_runner_can_stop_idle_pipeline() {
    let source = BufferedSource::new(8_000, 1).unwrap();
    let frames = Arc::new(Mutex::new(Vec::new()));
    let sink = CollectingSink {
        frames: Arc::clone(&frames),
    };
    let pipeline = Pipeline::new(
        Box::new(source),
        Box::new(RawCodec::new(RawBitDepth::Float32)),
        Box::new(sink),
    );
    let mut runner = PipelineRunner::start(pipeline, Duration::from_millis(1));

    wait_for(|| runner.is_running());

    runner.stop().unwrap();
    assert!(!runner.is_running());
    assert!(frames.lock().unwrap().is_empty());
}

struct CollectingSink {
    frames: Arc<Mutex<Vec<EncodedAudioFrame>>>,
}

impl AudioSink for CollectingSink {
    fn handle_frame(&mut self, frame: EncodedAudioFrame) -> Result<(), PipelineError> {
        self.frames.lock().unwrap().push(frame);
        Ok(())
    }
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(condition());
}
