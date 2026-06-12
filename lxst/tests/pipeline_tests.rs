use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

use lxst::{
    AudioCodec, AudioSink, AudioSource, BufferedSink, BufferedSource, EncodedAudioFrame,
    EncodedMixerSink, Loopback, Mixer, MixerRuntime, Pipeline, PipelineError, PipelineRunner,
    RawBitDepth, RawCodec,
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

#[test]
fn loopback_decodes_sink_frames_into_source_queue() {
    let original = lxst::AudioFrame::new(8_000, 1, vec![0.0, 0.5]).unwrap();
    let mut encoder = RawCodec::new(RawBitDepth::Float32);
    let encoded = EncodedAudioFrame {
        codec: CodecKind::Raw,
        samplerate: 8_000,
        channels: 1,
        payload: encoder.encode(&original).unwrap(),
    };
    let loopback =
        Loopback::new(Box::new(RawCodec::new(RawBitDepth::Float32)), 8_000, 1, 2).unwrap();
    let mut sink = loopback.clone();
    let mut source = loopback.clone();

    sink.handle_frame(encoded).unwrap();
    assert_eq!(loopback.queued_frames(), 1);
    assert!(source.next_frame().unwrap().is_none());

    source.start();
    let decoded = source.next_frame().unwrap().unwrap();
    assert_eq!(decoded.samplerate(), 8_000);
    assert_eq!(decoded.channels(), 1);
    assert_eq!(decoded.samples(), original.samples());
    assert_eq!(loopback.queued_frames(), 0);
}

#[test]
fn loopback_drops_oldest_frame_when_full() {
    let loopback =
        Loopback::new(Box::new(RawCodec::new(RawBitDepth::Float32)), 8_000, 1, 1).unwrap();
    let mut sink = loopback.clone();
    let mut source = loopback.clone();

    sink.handle_frame(raw_encoded_frame(&[0.25])).unwrap();
    sink.handle_frame(raw_encoded_frame(&[0.75])).unwrap();

    source.start();
    let decoded = source.next_frame().unwrap().unwrap();
    assert_eq!(decoded.samples(), &[0.75]);
    assert!(source.next_frame().unwrap().is_none());
}

#[test]
fn encoded_mixer_sink_encodes_mixed_frames() {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let sink = CollectingSink {
        frames: Arc::clone(&frames),
    };
    let mut sink = EncodedMixerSink::new(Box::new(RawCodec::new(RawBitDepth::Float32)), sink);

    lxst::MixerSink::handle_frame(
        &mut sink,
        lxst::AudioFrame::new(8_000, 1, vec![0.25, -0.5]).unwrap(),
    )
    .unwrap();

    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].codec, CodecKind::Raw);
    assert_eq!(frames[0].samplerate, 8_000);
    assert_eq!(frames[0].channels, 1);

    let mut decoder = RawCodec::new(RawBitDepth::Float32);
    let decoded = decoder
        .decode(&frames[0].payload, frames[0].samplerate)
        .unwrap();
    assert_eq!(decoded.samples(), &[0.25, -0.5]);
}

#[test]
fn mixer_runtime_can_feed_encoded_sink() {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let sink = CollectingSink {
        frames: Arc::clone(&frames),
    };
    let sink = EncodedMixerSink::new(Box::new(RawCodec::new(RawBitDepth::Float32)), sink);
    let mut runtime = MixerRuntime::start(Mixer::default(), sink, Duration::from_millis(1));
    let mixer = runtime.mixer();

    {
        let mut mixer = mixer.lock().unwrap();
        mixer.push(1, lxst::AudioFrame::new(8_000, 1, vec![0.25]).unwrap());
        mixer.push(2, lxst::AudioFrame::new(8_000, 1, vec![0.5]).unwrap());
    }

    wait_for(|| frames.lock().unwrap().len() == 1);
    runtime.stop().unwrap();

    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].codec, CodecKind::Raw);
    let mut decoder = RawCodec::new(RawBitDepth::Float32);
    let decoded = decoder
        .decode(&frames[0].payload, frames[0].samplerate)
        .unwrap();
    assert_eq!(decoded.samples(), &[0.75]);
}

#[test]
fn encoded_mixer_sink_delegates_backpressure() {
    let accepting = Arc::new(AtomicBool::new(false));
    let sink = GatedSink {
        accepting: Arc::clone(&accepting),
        frames: Arc::new(Mutex::new(Vec::new())),
    };
    let mut sink = EncodedMixerSink::new(Box::new(RawCodec::new(RawBitDepth::Float32)), sink);

    assert!(!lxst::MixerSink::can_receive(&sink));
    assert!(lxst::MixerSink::handle_frame(
        &mut sink,
        lxst::AudioFrame::new(8_000, 1, vec![0.25]).unwrap(),
    )
    .is_err());

    accepting.store(true, Ordering::SeqCst);
    assert!(lxst::MixerSink::can_receive(&sink));
    lxst::MixerSink::handle_frame(
        &mut sink,
        lxst::AudioFrame::new(8_000, 1, vec![0.25]).unwrap(),
    )
    .unwrap();
    assert_eq!(sink.sink().frames.lock().unwrap().len(), 1);
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

struct GatedSink {
    accepting: Arc<AtomicBool>,
    frames: Arc<Mutex<Vec<EncodedAudioFrame>>>,
}

impl AudioSink for GatedSink {
    fn can_receive(&self) -> bool {
        self.accepting.load(Ordering::SeqCst)
    }

    fn handle_frame(&mut self, frame: EncodedAudioFrame) -> Result<(), PipelineError> {
        if !self.can_receive() {
            return Err(PipelineError::SinkFull);
        }
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

fn raw_encoded_frame(samples: &[f32]) -> EncodedAudioFrame {
    let frame = lxst::AudioFrame::new(8_000, 1, samples.to_vec()).unwrap();
    let mut codec = RawCodec::new(RawBitDepth::Float32);
    EncodedAudioFrame {
        codec: CodecKind::Raw,
        samplerate: 8_000,
        channels: 1,
        payload: codec.encode(&frame).unwrap(),
    }
}
