use lxst::audio::AudioFilter;
use lxst::{
    plan_line_source_frame, plan_mixer_frame, Agc, AudioCodec, AudioDeviceKind, AudioFrame,
    AudioSource, CallProfile, CallState, CallerPolicy, Codec2Codec, CodecError, CodecFactory,
    CodecSelection, LinePlayback, LineSourceProcessor, Mixer, MixerRuntime, MixerSink, OpusCodec,
    QueuedLineSink, QueuedLineSinkConfig, RawBitDepth, RawCodec, Signal, SignalCode, Telephone,
    TelephoneConfig, ToneSource,
};
use lxst_core::CodecProfile;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn raw_codec_round_trips_f32_frames() {
    let frame = AudioFrame::new(48_000, 2, vec![0.0, 0.5, -0.5, 1.0]).unwrap();
    let mut codec = RawCodec::new(RawBitDepth::Float32);
    let encoded = codec.encode(&frame).unwrap();
    assert_eq!(encoded[0], 0b0100_0001);

    let decoded = codec.decode(&encoded, 48_000).unwrap();
    assert_eq!(decoded.channels(), 2);
    assert_eq!(decoded.samplerate(), 48_000);
    assert_eq!(decoded.samples(), frame.samples());
}

#[test]
fn raw_codec_round_trips_f16_frames_like_python() {
    let frame = AudioFrame::new(48_000, 1, vec![0.0, 0.5, -0.5, 1.0]).unwrap();
    let mut codec = RawCodec::new(RawBitDepth::Float16);
    let encoded = codec.encode(&frame).unwrap();
    assert_eq!(
        encoded,
        vec![0x00, 0x00, 0x00, 0x00, 0x38, 0x00, 0xb8, 0x00, 0x3c]
    );

    let decoded = codec.decode(&encoded, 48_000).unwrap();
    assert_eq!(decoded.channels(), 1);
    assert_eq!(decoded.samplerate(), 48_000);
    assert_eq!(decoded.samples(), frame.samples());
}

#[test]
fn raw_codec_rejects_truncated_f16_payloads() {
    let mut codec = RawCodec::new(RawBitDepth::Float16);
    assert!(matches!(
        codec.decode(&[0x00, 0x00], 48_000),
        Err(CodecError::InvalidPayloadLength(1))
    ));
}

#[test]
fn raw_codec_fixed_channels_duplicates_missing_channels_like_python() {
    let frame = AudioFrame::new(48_000, 1, vec![0.25, -0.5]).unwrap();
    let mut codec = RawCodec::with_channels(RawBitDepth::Float32, 2);

    let encoded = codec.encode(&frame).unwrap();
    assert_eq!(encoded[0], 0b0100_0001);

    let decoded = codec.decode(&encoded, 48_000).unwrap();
    assert_eq!(codec.channels(), Some(2));
    assert_eq!(decoded.channels(), 2);
    assert_eq!(decoded.samples(), &[0.25, 0.25, -0.5, -0.5]);
}

#[test]
fn raw_codec_fixed_channels_truncates_extra_channels_like_python() {
    let frame = AudioFrame::new(48_000, 2, vec![0.25, 0.75, -0.5, -1.0]).unwrap();
    let mut codec = RawCodec::with_channels(RawBitDepth::Float32, 1);

    let encoded = codec.encode(&frame).unwrap();
    assert_eq!(encoded[0], 0b0100_0000);

    let decoded = codec.decode(&encoded, 48_000).unwrap();
    assert_eq!(codec.channels(), Some(1));
    assert_eq!(decoded.channels(), 1);
    assert_eq!(decoded.samples(), &[0.25, -0.5]);
}

#[test]
fn raw_codec_fixed_channels_are_clamped_like_python() {
    assert_eq!(
        RawCodec::with_channels(RawBitDepth::Float32, 0).channels(),
        Some(1)
    );
    assert_eq!(
        RawCodec::with_channels(RawBitDepth::Float32, u8::MAX).channels(),
        Some(32)
    );
}

#[test]
fn audio_frame_normalizes_channels_for_device_output() {
    let mono = AudioFrame::new(48_000, 1, vec![0.25, -0.5]).unwrap();
    let stereo = mono.with_channels(2).unwrap();

    assert_eq!(stereo.channels(), 2);
    assert_eq!(stereo.samples(), &[0.25, 0.25, -0.5, -0.5]);

    let folded = stereo.with_channels(1).unwrap();
    assert_eq!(folded.channels(), 1);
    assert_eq!(folded.samples(), &[0.25, -0.5]);
}

#[test]
fn audio_frame_resamples_for_device_output() {
    let frame = AudioFrame::new(8_000, 1, vec![0.0; 80]).unwrap();
    let resampled = frame.resampled(16_000).unwrap();

    assert_eq!(resampled.samplerate(), 16_000);
    assert_eq!(resampled.channels(), 1);
    assert_eq!(resampled.frame_count(), 160);
}

#[test]
fn opus_codec_round_trips_voice_frame() {
    let samples: Vec<f32> = (0..160)
        .map(|n| ((n as f32 / 8_000.0) * 440.0 * std::f32::consts::TAU).sin() * 0.2)
        .collect();
    let frame = AudioFrame::new(8_000, 1, samples).unwrap();
    let mut codec = OpusCodec::new(CodecProfile::OpusVoiceLow);

    let encoded = codec.encode(&frame).unwrap();
    assert!(!encoded.is_empty());
    assert!(encoded.len() <= 15);

    let decoded = codec.decode(&encoded, 8_000).unwrap();
    assert_eq!(decoded.samplerate(), 8_000);
    assert_eq!(decoded.channels(), 1);
    assert_eq!(decoded.frame_count(), 160);
}

#[test]
fn codec_paths_share_audio_frame_conversion_shape() {
    let samples: Vec<f32> = (0..960)
        .flat_map(|n| {
            let sample = ((n as f32 / 48_000.0) * 180.0 * std::f32::consts::TAU).sin() * 0.2;
            [sample, -sample]
        })
        .collect();
    let frame = AudioFrame::new(48_000, 2, samples).unwrap();

    let mut opus = OpusCodec::new(CodecProfile::OpusVoiceLow);
    let opus_encoded = opus.encode(&frame).unwrap();
    let opus_decoded = opus.decode(&opus_encoded, 8_000).unwrap();
    assert_eq!(opus_decoded.samplerate(), 8_000);
    assert_eq!(opus_decoded.channels(), 1);
    assert_eq!(opus_decoded.frame_count(), 160);

    let mut codec2 = Codec2Codec::new(CodecProfile::Codec2_3200);
    let codec2_encoded = codec2.encode(&frame).unwrap();
    let codec2_decoded = codec2.decode(&codec2_encoded, 8_000).unwrap();
    assert_eq!(codec2_decoded.samplerate(), 8_000);
    assert_eq!(codec2_decoded.channels(), 1);
    assert_eq!(codec2_decoded.frame_count(), 160);
}

#[test]
fn opus_codec_resamples_and_normalizes_channels() {
    let samples: Vec<f32> = (0..960)
        .flat_map(|n| {
            let sample = ((n as f32 / 48_000.0) * 220.0 * std::f32::consts::TAU).sin() * 0.2;
            [sample, -sample]
        })
        .collect();
    let frame = AudioFrame::new(48_000, 2, samples).unwrap();
    let mut codec = OpusCodec::new(CodecProfile::OpusVoiceLow);

    let encoded = codec.encode(&frame).unwrap();
    let decoded = codec.decode(&encoded, 8_000).unwrap();
    assert_eq!(decoded.samplerate(), 8_000);
    assert_eq!(decoded.channels(), 1);
    assert_eq!(decoded.frame_count(), 160);
}

#[test]
fn opus_codec_rejects_invalid_frame_duration() {
    let frame = AudioFrame::new(8_000, 1, vec![0.0; 123]).unwrap();
    let mut codec = OpusCodec::new(CodecProfile::OpusVoiceLow);

    assert!(matches!(
        codec.encode(&frame),
        Err(CodecError::InvalidFrameDuration {
            sample_count: 123,
            samplerate: 8_000,
        })
    ));
}

#[test]
fn opus_profile_helpers_match_python_tables() {
    assert_eq!(
        OpusCodec::profile_channels(CodecProfile::OpusVoiceMax).unwrap(),
        2
    );
    assert_eq!(
        OpusCodec::profile_samplerate(CodecProfile::OpusAudioLow).unwrap(),
        12_000
    );
    assert_eq!(
        OpusCodec::profile_bitrate_ceiling(CodecProfile::OpusAudioMax).unwrap(),
        128_000
    );
    assert_eq!(OpusCodec::max_bytes_per_frame(8_000, 60.0), 60);
    assert_eq!(OpusCodec::max_bytes_per_frame(6_000, 2.5), 2);
    assert!(OpusCodec::profile_channels(CodecProfile::Raw).is_err());
}

#[test]
fn codec2_3200_round_trips_one_frame_with_python_header() {
    let samples: Vec<f32> = (0..160)
        .map(|n| ((n as f32 / 8_000.0) * 180.0 * std::f32::consts::TAU).sin() * 0.2)
        .collect();
    let frame = AudioFrame::new(8_000, 1, samples).unwrap();
    let mut codec = Codec2Codec::new(CodecProfile::Codec2_3200);

    let encoded = codec.encode(&frame).unwrap();
    assert_eq!(encoded[0], 0x06);
    assert_eq!(encoded.len(), 9);

    let decoded = codec.decode(&encoded, 8_000).unwrap();
    assert_eq!(decoded.samplerate(), 8_000);
    assert_eq!(decoded.channels(), 1);
    assert_eq!(decoded.frame_count(), 160);
}

#[test]
fn codec2_1600_round_trips_one_frame_with_python_header() {
    let samples: Vec<f32> = (0..320)
        .map(|n| ((n as f32 / 8_000.0) * 180.0 * std::f32::consts::TAU).sin() * 0.2)
        .collect();
    let frame = AudioFrame::new(8_000, 1, samples).unwrap();
    let mut codec = Codec2Codec::new(CodecProfile::Codec2_1600);

    let encoded = codec.encode(&frame).unwrap();
    assert_eq!(encoded[0], 0x04);
    assert_eq!(encoded.len(), 9);

    let decoded = codec.decode(&encoded, 8_000).unwrap();
    assert_eq!(decoded.samplerate(), 8_000);
    assert_eq!(decoded.channels(), 1);
    assert_eq!(decoded.frame_count(), 320);
}

#[test]
fn codec2_profiles_cover_python_mode_headers() {
    let cases = [
        (CodecProfile::Codec2_700C, 0x00, 320),
        (CodecProfile::Codec2_1200, 0x01, 320),
        (CodecProfile::Codec2_1300, 0x02, 320),
        (CodecProfile::Codec2_1400, 0x03, 320),
        (CodecProfile::Codec2_1600, 0x04, 320),
        (CodecProfile::Codec2_2400, 0x05, 160),
        (CodecProfile::Codec2_3200, 0x06, 160),
    ];

    for (profile, header, samples_per_frame) in cases {
        let frame = AudioFrame::new(8_000, 1, vec![0.0; samples_per_frame]).unwrap();
        let mut codec = Codec2Codec::new(profile);
        let encoded = codec.encode(&frame).unwrap();
        assert_eq!(encoded[0], header, "{profile:?}");
        assert!(!encoded[1..].is_empty(), "{profile:?}");

        let decoded = codec.decode(&encoded, 8_000).unwrap();
        assert_eq!(decoded.samplerate(), 8_000, "{profile:?}");
        assert_eq!(decoded.channels(), 1, "{profile:?}");
        assert_eq!(decoded.frame_count(), samples_per_frame, "{profile:?}");
    }
}

#[test]
fn codec_factory_creates_all_python_codec2_profiles() {
    for profile in [
        CodecProfile::Codec2_700C,
        CodecProfile::Codec2_1200,
        CodecProfile::Codec2_1300,
        CodecProfile::Codec2_1400,
        CodecProfile::Codec2_1600,
        CodecProfile::Codec2_2400,
        CodecProfile::Codec2_3200,
    ] {
        let codec = CodecFactory::create(CodecSelection::Profile(profile));
        assert_eq!(codec.kind(), lxst::CodecKind::Codec2, "{profile:?}");
    }
}

#[test]
fn codec2_decode_uses_embedded_mode_header() {
    let samples = vec![0.0; 160];
    let frame = AudioFrame::new(8_000, 1, samples).unwrap();
    let mut encoder = Codec2Codec::new(CodecProfile::Codec2_3200);
    let encoded = encoder.encode(&frame).unwrap();
    let mut decoder = Codec2Codec::new(CodecProfile::Codec2_1600);

    let decoded = decoder.decode(&encoded, 8_000).unwrap();
    assert_eq!(decoded.frame_count(), 160);
}

#[test]
fn codec2_700c_round_trips_with_python_header() {
    let samples: Vec<f32> = (0..320)
        .map(|n| ((n as f32 / 8_000.0) * 180.0 * std::f32::consts::TAU).sin() * 0.2)
        .collect();
    let frame = AudioFrame::new(8_000, 1, samples).unwrap();
    let mut codec = Codec2Codec::new(CodecProfile::Codec2_700C);

    let encoded = codec.encode(&frame).unwrap();
    assert_eq!(encoded[0], 0x00);
    assert_eq!(encoded.len(), 5);

    let decoded = codec.decode(&encoded, 8_000).unwrap();
    assert_eq!(decoded.samplerate(), 8_000);
    assert_eq!(decoded.channels(), 1);
    assert_eq!(decoded.frame_count(), 320);
}

#[test]
fn mixer_sums_and_clips_frames() {
    let mut mixer = Mixer::default();
    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.75, 0.25]).unwrap());
    mixer.push(2, AudioFrame::new(8_000, 1, vec![0.75, -0.5]).unwrap());

    let mixed = mixer.mix_next().unwrap().unwrap();
    assert_eq!(mixed.samples(), &[1.0, -0.25]);
}

#[test]
fn mixer_reports_source_backpressure() {
    let mut mixer = Mixer::default();
    mixer.set_source_max_frames(1, 2);

    assert!(mixer.can_receive(1));
    assert!(mixer.can_receive(42));
    assert_eq!(mixer.queued_frames(1), 0);

    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.2]).unwrap());
    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.4]).unwrap());

    assert_eq!(mixer.queued_frames(1), 2);
    assert!(!mixer.can_receive(1));
    assert!(mixer.can_receive(2));
}

#[test]
fn mixer_limits_frames_per_source() {
    let mut mixer = Mixer::default();
    mixer.set_source_max_frames(1, 2);
    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.2]).unwrap());
    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.4]).unwrap());
    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.6]).unwrap());

    assert_eq!(mixer.mix_next().unwrap().unwrap().samples(), &[0.4]);
    assert_eq!(mixer.mix_next().unwrap().unwrap().samples(), &[0.6]);
    assert!(mixer.mix_next().unwrap().is_none());
}

#[test]
fn mixer_tightening_source_limit_updates_backpressure() {
    let mut mixer = Mixer::default();
    mixer.set_source_max_frames(1, 3);
    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.2]).unwrap());
    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.4]).unwrap());

    assert!(mixer.can_receive(1));

    mixer.set_source_max_frames(1, 1);

    assert_eq!(mixer.queued_frames(1), 1);
    assert!(!mixer.can_receive(1));
}

#[test]
fn mixer_tightening_source_limit_drops_oldest_frames() {
    let mut mixer = Mixer::default();
    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.2]).unwrap());
    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.4]).unwrap());
    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.6]).unwrap());

    mixer.set_source_max_frames(1, 1);

    assert_eq!(mixer.mix_next().unwrap().unwrap().samples(), &[0.6]);
    assert!(mixer.mix_next().unwrap().is_none());
}

#[test]
fn mixer_runtime_drains_mixed_frames_to_sink() {
    let sink = FakeMixerSink::new(true);
    let received = Arc::clone(&sink.frames);
    let mut runtime = MixerRuntime::start(Mixer::default(), sink, Duration::from_millis(1));
    let mixer = runtime.mixer();

    {
        let mut mixer = mixer.lock().unwrap();
        mixer.push(1, AudioFrame::new(8_000, 1, vec![0.5]).unwrap());
        mixer.push(2, AudioFrame::new(8_000, 1, vec![0.25]).unwrap());
    }

    wait_until(Duration::from_millis(100), || {
        received.lock().unwrap().len() == 1
    });
    {
        let frames = received.lock().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].samples(), &[0.75]);
    }
    assert!(runtime.stop().unwrap().is_some());
    assert!(!runtime.is_running());
}

#[test]
fn mixer_runtime_respects_sink_backpressure() {
    let sink = FakeMixerSink::new(false);
    let received = Arc::clone(&sink.frames);
    let accepting = Arc::clone(&sink.accepting);
    let mut runtime = MixerRuntime::start(Mixer::default(), sink, Duration::from_millis(1));
    let mixer = runtime.mixer();

    mixer
        .lock()
        .unwrap()
        .push(1, AudioFrame::new(8_000, 1, vec![0.5]).unwrap());
    thread::sleep(Duration::from_millis(20));

    assert_eq!(mixer.lock().unwrap().queued_frames(1), 1);
    assert!(received.lock().unwrap().is_empty());

    accepting.store(true, Ordering::SeqCst);
    wait_until(Duration::from_millis(100), || {
        received.lock().unwrap().len() == 1
    });
    assert_eq!(mixer.lock().unwrap().queued_frames(1), 0);
    assert_eq!(received.lock().unwrap()[0].samples(), &[0.5]);
    assert!(runtime.stop().unwrap().is_some());
}

#[test]
fn line_source_frame_plan_clamps_opus_to_valid_maximum() {
    let plan = plan_line_source_frame(80.0, Some(CodecProfile::OpusVoiceLow), 48_000, 1).unwrap();

    assert_eq!(plan.requested_frame_ms, 80.0);
    assert_eq!(plan.target_frame_ms, 60.0);
    assert_eq!(plan.frame_count, 2_880);
    assert_eq!(plan.sample_count, 2_880);
}

#[test]
fn line_source_frame_plan_matches_python_opus_quantize_then_nearest_valid() {
    let plan = plan_line_source_frame(7.0, Some(CodecProfile::OpusVoiceLow), 8_000, 1).unwrap();

    assert_eq!(plan.target_frame_ms, 5.0);
    assert_eq!(plan.frame_count, 40);
    assert_eq!(plan.sample_count, 40);
}

#[test]
fn line_source_frame_plan_quantizes_all_codec2_profiles_to_40ms_blocks() {
    for profile in [
        CodecProfile::Codec2_700C,
        CodecProfile::Codec2_1200,
        CodecProfile::Codec2_1300,
        CodecProfile::Codec2_1400,
        CodecProfile::Codec2_1600,
        CodecProfile::Codec2_2400,
        CodecProfile::Codec2_3200,
    ] {
        let plan = plan_line_source_frame(45.0, Some(profile), 8_000, 1).unwrap();
        assert_eq!(plan.target_frame_ms, 80.0, "{profile:?}");
        assert_eq!(plan.frame_count, 640, "{profile:?}");
        assert_eq!(plan.sample_count, 640, "{profile:?}");
    }
}

#[test]
fn line_source_frame_plan_quantizes_codec2_to_40ms_blocks() {
    let plan = plan_line_source_frame(45.0, Some(CodecProfile::Codec2_3200), 8_000, 1).unwrap();

    assert_eq!(plan.target_frame_ms, 80.0);
    assert_eq!(plan.frame_count, 640);
    assert_eq!(plan.sample_count, 640);
}

#[test]
fn line_source_frame_plan_leaves_unconstrained_profiles_unchanged() {
    let plan = plan_line_source_frame(80.0, Some(CodecProfile::Raw), 48_000, 2).unwrap();

    assert_eq!(plan.target_frame_ms, 80.0);
    assert_eq!(plan.frame_count, 3_840);
    assert_eq!(plan.sample_count, 7_680);
}

#[test]
fn mixer_frame_plan_reuses_codec_target_sizing() {
    let opus = plan_mixer_frame(7.0, Some(CodecProfile::OpusVoiceLow), 8_000, 1).unwrap();
    assert_eq!(opus.requested_frame_ms, 7.0);
    assert_eq!(opus.target_frame_ms, 5.0);
    assert_eq!(opus.frame_count, 40);
    assert_eq!(opus.sample_count, 40);

    let codec2 = plan_mixer_frame(45.0, Some(CodecProfile::Codec2_3200), 8_000, 1).unwrap();
    assert_eq!(codec2.target_frame_ms, 80.0);
    assert_eq!(codec2.frame_count, 640);
}

#[test]
fn mixer_frame_plan_keeps_raw_output_unconstrained() {
    let plan = plan_mixer_frame(33.0, None, 48_000, 2).unwrap();

    assert_eq!(plan.target_frame_ms, 33.0);
    assert_eq!(plan.frame_count, 1_584);
    assert_eq!(plan.sample_count, 3_168);
}

#[test]
fn line_source_processor_skips_before_filters() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut processor =
        LineSourceProcessor::new(0.0, Duration::ZERO, Duration::from_millis(1), 1_000, 1);
    processor.add_filter(CountingFilter {
        calls: Arc::clone(&calls),
    });

    let skipped = processor.process_frame(AudioFrame::new(1_000, 1, vec![0.5]).unwrap());
    assert!(skipped.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(processor.skipped_samples(), 1);

    let passed = processor
        .process_frame(AudioFrame::new(1_000, 1, vec![0.5]).unwrap())
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(passed.samples(), &[0.5]);
}

#[test]
fn line_source_processor_filters_before_frame_level_ease() {
    let mut processor =
        LineSourceProcessor::new(0.0, Duration::from_millis(2), Duration::ZERO, 1_000, 1);
    processor.add_filter(AddFilter { amount: 1.0 });

    let muted = processor
        .process_frame(AudioFrame::new(1_000, 1, vec![-0.5, -0.5]).unwrap())
        .unwrap();
    assert_eq!(muted.samples(), &[0.0, 0.0]);
    assert_eq!(processor.processed_samples(), 2);

    let gained = processor
        .process_frame(AudioFrame::new(1_000, 1, vec![-0.5]).unwrap())
        .unwrap();
    assert_eq!(gained.samples(), &[0.5]);
}

#[test]
fn line_source_processor_applies_gain_after_filters_and_clips() {
    let mut processor = LineSourceProcessor::new(10.0, Duration::ZERO, Duration::ZERO, 8_000, 1);
    processor.add_filter(AddFilter { amount: 0.05 });

    let frame = processor
        .process_frame(AudioFrame::new(8_000, 1, vec![0.05, 0.2]).unwrap())
        .unwrap();

    assert_eq!(frame.samples(), &[1.0, 1.0]);
}

#[test]
fn queued_line_sink_applies_backpressure_height() {
    let fake = FakePlayback::default();
    let mut sink = QueuedLineSink::new(
        fake,
        QueuedLineSinkConfig {
            samplerate: 8_000,
            channels: 1,
            max_frames: 5,
            autodigest: false,
            ..QueuedLineSinkConfig::default()
        },
    )
    .unwrap();

    assert!(sink.can_receive());
    sink.handle_frame(AudioFrame::new(8_000, 1, vec![0.1; 8]).unwrap())
        .unwrap();
    sink.handle_frame(AudioFrame::new(8_000, 1, vec![0.2; 8]).unwrap())
        .unwrap();

    assert_eq!(sink.queued_frames(), 2);
    assert_eq!(sink.samples_per_frame(), Some(8));
    assert_eq!(sink.frame_time(), Some(Duration::from_millis(1)));
    assert!(!sink.can_receive());
}

#[test]
fn queued_line_sink_autodigests_frames() {
    let fake = FakePlayback::default();
    let played = Arc::clone(&fake.played);
    let mut sink = QueuedLineSink::new(
        fake,
        QueuedLineSinkConfig {
            samplerate: 8_000,
            channels: 1,
            frame_timeout: 100,
            ..QueuedLineSinkConfig::default()
        },
    )
    .unwrap();

    sink.handle_frame(AudioFrame::new(8_000, 1, vec![0.3; 8]).unwrap())
        .unwrap();

    wait_for(|| played.lock().unwrap().len() == 1);
    assert_eq!(played.lock().unwrap()[0].samples(), &[0.3; 8]);
    assert_eq!(sink.stats().max_latency, Duration::from_millis(3));
    sink.stop().unwrap();
}

#[test]
fn queued_line_sink_stops_after_underrun_timeout() {
    let fake = FakePlayback::default();
    let mut sink = QueuedLineSink::new(
        fake,
        QueuedLineSinkConfig {
            samplerate: 8_000,
            channels: 1,
            frame_timeout: 2,
            ..QueuedLineSinkConfig::default()
        },
    )
    .unwrap();

    sink.handle_frame(AudioFrame::new(8_000, 1, vec![0.0; 8]).unwrap())
        .unwrap();

    wait_for(|| !sink.is_running());
    assert!(sink.stats().underrun_at.is_some());
    sink.stop().unwrap();
}

#[test]
fn queued_line_sink_can_restart_after_stop() {
    let fake = FakePlayback::default();
    let played = Arc::clone(&fake.played);
    let mut sink = QueuedLineSink::new(
        fake,
        QueuedLineSinkConfig {
            samplerate: 8_000,
            channels: 1,
            frame_timeout: 100,
            ..QueuedLineSinkConfig::default()
        },
    )
    .unwrap();

    sink.handle_frame(AudioFrame::new(8_000, 1, vec![0.1; 8]).unwrap())
        .unwrap();
    wait_for(|| played.lock().unwrap().len() == 1);
    sink.stop().unwrap();

    sink.handle_frame(AudioFrame::new(8_000, 1, vec![0.2; 8]).unwrap())
        .unwrap();
    wait_for(|| played.lock().unwrap().len() == 2);
    sink.stop().unwrap();
}

#[test]
fn queued_line_sink_enables_low_latency_at_runtime() {
    let fake = FakePlayback::default();
    let low_latency_requests = Arc::clone(&fake.low_latency_requests);
    let mut sink = QueuedLineSink::new(
        fake,
        QueuedLineSinkConfig {
            samplerate: 8_000,
            channels: 1,
            frame_timeout: 100,
            ..QueuedLineSinkConfig::default()
        },
    )
    .unwrap();

    sink.handle_frame(AudioFrame::new(8_000, 1, vec![0.0; 8]).unwrap())
        .unwrap();
    sink.enable_low_latency();

    wait_for(|| low_latency_requests.load(Ordering::SeqCst) == 1);
    assert!(sink.stats().low_latency_enabled);
    sink.stop().unwrap();
}

#[test]
fn tone_source_generates_expected_shape() {
    let mut tone = ToneSource::new(1_000.0, 8_000, 2, 0.5);
    let frame = tone.next_frame(8).unwrap();
    assert_eq!(frame.channels(), 2);
    assert_eq!(frame.frame_count(), 8);
    assert_eq!(frame.samples().len(), 16);
}

#[test]
fn tone_source_obeys_audio_source_lifecycle() {
    let mut tone = ToneSource::with_frame_ms(1_000.0, 8_000, 1, 0.5, 20);

    assert!(AudioSource::next_frame(&mut tone).unwrap().is_none());

    tone.start();
    assert!(tone.is_running());
    let frame = AudioSource::next_frame(&mut tone).unwrap().unwrap();
    assert_eq!(frame.samplerate(), 8_000);
    assert_eq!(frame.channels(), 1);
    assert_eq!(frame.frame_count(), 160);
}

#[test]
fn tone_source_with_codec_profile_uses_codec_frame_plan() {
    let mut tone =
        ToneSource::with_codec_profile(1_000.0, 2, 0.5, 7, CodecProfile::OpusVoiceLow).unwrap();

    tone.start();
    let frame = AudioSource::next_frame(&mut tone).unwrap().unwrap();
    assert_eq!(frame.samplerate(), 8_000);
    assert_eq!(frame.channels(), 1);
    assert_eq!(frame.frame_count(), 40);
}

#[test]
fn tone_source_with_codec2_profile_quantizes_frame_time() {
    let mut tone =
        ToneSource::with_codec_profile(1_000.0, 2, 0.5, 45, CodecProfile::Codec2_3200).unwrap();

    tone.start();
    let frame = AudioSource::next_frame(&mut tone).unwrap().unwrap();
    assert_eq!(frame.samplerate(), 8_000);
    assert_eq!(frame.channels(), 1);
    assert_eq!(frame.frame_count(), 640);
}

#[test]
fn tone_source_eases_out_before_stopping() {
    let mut tone = ToneSource::with_frame_ms(1_000.0, 8_000, 1, 0.5, 20);
    tone.start();
    assert!(AudioSource::next_frame(&mut tone).unwrap().is_some());

    tone.stop();
    let mut frames_after_stop = 0;
    while tone.is_running() && frames_after_stop < 8 {
        assert!(AudioSource::next_frame(&mut tone).unwrap().is_some());
        frames_after_stop += 1;
    }

    assert!(frames_after_stop > 0);
    assert!(!tone.is_running());
    assert!(AudioSource::next_frame(&mut tone).unwrap().is_none());
}

#[test]
fn agc_keeps_output_bounded() {
    let frame = AudioFrame::new(8_000, 1, vec![0.01; 128]).unwrap();
    let mut agc = Agc::new(-12.0, 12.0);
    let processed = agc.process(frame);
    assert!(processed
        .samples()
        .iter()
        .all(|sample| (-0.75..=0.75).contains(sample)));
}

#[test]
fn agc_does_not_raise_below_trigger_noise() {
    let frame = AudioFrame::new(8_000, 1, vec![0.001; 128]).unwrap();
    let mut agc = Agc::new(-12.0, 12.0);
    let processed = agc.process(frame);

    assert!(processed
        .samples()
        .iter()
        .all(|sample| (*sample - 0.001).abs() < f32::EPSILON));
}

#[test]
fn agc_limits_per_channel_peaks() {
    let samples: Vec<f32> = (0..128).flat_map(|_| [1.0, 0.25]).collect();
    let frame = AudioFrame::new(8_000, 2, samples).unwrap();
    let mut agc = Agc::new(20.0, 20.0);
    let processed = agc.process(frame);

    let mut peaks = [0.0_f32; 2];
    for sample in processed.samples().chunks(2) {
        peaks[0] = peaks[0].max(sample[0].abs());
        peaks[1] = peaks[1].max(sample[1].abs());
    }
    assert!(peaks.iter().all(|peak| *peak <= 0.75));
}

#[test]
fn telephone_rejects_blocked_callers_and_emits_busy() {
    let blocked = [0x42; 16];
    let mut config = TelephoneConfig::default();
    config.blocked_callers.insert(blocked);
    let (mut telephone, rx) = Telephone::new(config);

    assert!(!telephone.begin_incoming_call(blocked));
    assert_eq!(telephone.state(), CallState::Available);
    assert!(matches!(
        rx.recv().unwrap(),
        lxst::telephony::CallEvent::Busy { .. }
    ));
}

#[test]
fn telephone_follows_basic_incoming_flow() {
    let caller = [0x11; 16];
    let (mut telephone, _rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_incoming_call(caller));
    assert_eq!(telephone.state(), CallState::Ringing);
    assert!(telephone.answer());
    assert_eq!(telephone.state(), CallState::Connecting);
    assert!(telephone.establish());
    assert_eq!(telephone.state(), CallState::Established);
    telephone.hangup();
    assert_eq!(telephone.state(), CallState::Available);
}

#[test]
fn telephone_incoming_event_order_matches_call_flow() {
    let caller = [0x18; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_incoming_call(caller));
    assert!(telephone.answer());
    assert!(telephone.establish());

    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                lxst::telephony::CallEvent::StateChanged(state) => Some(*state),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![
            CallState::Ringing,
            CallState::Connecting,
            CallState::Established
        ]
    );
    assert!(matches!(
        events.get(1),
        Some(lxst::telephony::CallEvent::IncomingCall { identity_hash })
            if *identity_hash == caller
    ));
    assert!(matches!(
        events.last(),
        Some(lxst::telephony::CallEvent::CallEstablished { identity_hash })
            if *identity_hash == caller
    ));
}

#[test]
fn telephone_outgoing_event_order_matches_call_flow() {
    let remote = [0x19; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_outgoing_call(remote));
    telephone.apply_signal(Signal::Code(SignalCode::Ringing));
    telephone.apply_signal(Signal::Code(SignalCode::Connecting));
    telephone.apply_signal(Signal::Code(SignalCode::Established));

    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                lxst::telephony::CallEvent::StateChanged(state) => Some(*state),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![
            CallState::Calling,
            CallState::Ringing,
            CallState::Connecting,
            CallState::Established
        ]
    );
    assert!(matches!(
        events.last(),
        Some(lxst::telephony::CallEvent::CallEstablished { identity_hash })
            if *identity_hash == remote
    ));
}

#[test]
fn telephone_cannot_establish_unanswered_incoming_call() {
    let caller = [0x17; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_incoming_call(caller));
    assert!(!telephone.establish());

    assert_eq!(telephone.state(), CallState::Ringing);
    let events: Vec<_> = rx.try_iter().collect();
    assert!(!events
        .iter()
        .any(|event| matches!(event, lxst::telephony::CallEvent::CallEstablished { .. })));
}

#[test]
fn telephone_idle_hangup_is_noop() {
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    telephone.hangup();

    assert_eq!(telephone.state(), CallState::Available);
    assert!(rx.try_iter().next().is_none());
}

#[test]
fn telephone_hangup_rejects_ringing_incoming_call() {
    let caller = [0x12; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_incoming_call(caller));
    telephone.hangup();

    assert_eq!(telephone.state(), CallState::Available);
    let events: Vec<_> = rx.try_iter().collect();
    assert!(events.iter().any(|event| matches!(
        event,
        lxst::telephony::CallEvent::Rejected {
            identity_hash: Some(identity)
        } if *identity == caller
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        lxst::telephony::CallEvent::CallEnded {
            identity_hash: Some(identity)
        } if *identity == caller
    )));
}

#[test]
fn telephone_ignores_idle_profile_signalling() {
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    telephone.apply_signal(Signal::PreferredProfile(CallProfile::LowLatency));

    assert_eq!(telephone.active_profile(), CallProfile::DEFAULT);
    let events: Vec<_> = rx.try_iter().collect();
    assert!(!events
        .iter()
        .any(|event| matches!(event, lxst::telephony::CallEvent::ProfileChanged(_))));
}

#[test]
fn telephone_applies_active_profile_signalling_without_state_change() {
    let remote = [0x14; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_outgoing_call(remote));
    telephone.apply_signal(Signal::PreferredProfile(CallProfile::LowLatency));

    assert_eq!(telephone.state(), CallState::Calling);
    assert_eq!(telephone.active_profile(), CallProfile::LowLatency);
    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, lxst::telephony::CallEvent::ProfileChanged(_)))
            .count(),
        1
    );
}

#[test]
fn telephone_suppresses_duplicate_profile_signalling() {
    let remote = [0x15; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_outgoing_call(remote));
    telephone.apply_signal(Signal::PreferredProfile(CallProfile::LowLatency));
    telephone.apply_signal(Signal::PreferredProfile(CallProfile::LowLatency));

    assert_eq!(telephone.active_profile(), CallProfile::LowLatency);
    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    lxst::telephony::CallEvent::ProfileChanged(CallProfile::LowLatency)
                )
            })
            .count(),
        1
    );
}

#[test]
fn telephone_established_profile_signal_keeps_call_established() {
    let remote = [0x16; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_outgoing_call(remote));
    assert!(telephone.establish());
    telephone.apply_signal(Signal::PreferredProfile(CallProfile::UltraLowLatency));

    assert_eq!(telephone.state(), CallState::Established);
    assert_eq!(telephone.active_profile(), CallProfile::UltraLowLatency);
    let events: Vec<_> = rx.try_iter().collect();
    assert!(events.iter().any(|event| matches!(
        event,
        lxst::telephony::CallEvent::ProfileChanged(CallProfile::UltraLowLatency)
    )));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, lxst::telephony::CallEvent::StateChanged(_)))
            .count(),
        2
    );
}

#[test]
fn telephone_ignores_idle_status_signalling() {
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    telephone.apply_signal(Signal::Code(SignalCode::Available));
    telephone.apply_signal(Signal::Code(SignalCode::Established));

    assert_eq!(telephone.state(), CallState::Available);
    let events: Vec<_> = rx.try_iter().collect();
    assert!(!events.iter().any(|event| matches!(
        event,
        lxst::telephony::CallEvent::StateChanged(_)
            | lxst::telephony::CallEvent::CallEstablished { .. }
            | lxst::telephony::CallEvent::CallEnded { .. }
    )));
}

#[test]
fn telephone_ignores_status_signalling_before_incoming_answer() {
    let caller = [0x13; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_incoming_call(caller));
    telephone.apply_signal(Signal::Code(SignalCode::Established));
    telephone.apply_signal(Signal::Code(SignalCode::Busy));

    assert_eq!(telephone.state(), CallState::Ringing);
    let events: Vec<_> = rx.try_iter().collect();
    assert!(!events.iter().any(|event| matches!(
        event,
        lxst::telephony::CallEvent::CallEstablished { .. }
            | lxst::telephony::CallEvent::Busy { .. }
            | lxst::telephony::CallEvent::CallEnded { .. }
    )));
}

#[test]
fn telephone_selects_outgoing_profile_preference() {
    let remote = [0x22; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_outgoing_call_with_profile(remote, Some(CallProfile::HighQuality)));

    assert_eq!(telephone.active_profile(), CallProfile::HighQuality);
    assert!(rx.try_iter().any(|event| matches!(
        event,
        lxst::telephony::CallEvent::ProfileChanged(CallProfile::HighQuality)
    )));
}

#[test]
fn telephone_outgoing_signalling_follows_python_order() {
    let remote = [0x23; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_outgoing_call(remote));
    assert_eq!(telephone.state(), CallState::Calling);

    telephone.apply_signal(Signal::Code(SignalCode::Available));
    assert_eq!(telephone.state(), CallState::Calling);

    telephone.apply_signal(Signal::Code(SignalCode::Ringing));
    assert_eq!(telephone.state(), CallState::Ringing);

    telephone.apply_signal(Signal::Code(SignalCode::Connecting));
    assert_eq!(telephone.state(), CallState::Connecting);

    telephone.apply_signal(Signal::Code(SignalCode::Established));
    assert_eq!(telephone.state(), CallState::Established);

    let events: Vec<_> = rx.try_iter().collect();
    assert!(events.iter().any(|event| matches!(
        event,
        lxst::telephony::CallEvent::CallEstablished { identity_hash } if *identity_hash == remote
    )));
}

#[test]
fn telephone_low_latency_output_setting_emits_change() {
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    telephone.set_low_latency_output(true);

    assert!(telephone.low_latency_output());
    assert!(matches!(
        rx.recv().unwrap(),
        lxst::telephony::CallEvent::LowLatencyOutputChanged(true)
    ));
}

#[test]
fn telephone_updates_audio_control_settings() {
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());

    telephone.set_receive_gain_db(3.0);
    telephone.set_transmit_gain_db(-2.5);
    telephone.disable_agc(true);
    telephone.mute_receive(true);
    telephone.mute_transmit(true);
    telephone.set_connect_timeout(Duration::from_secs(2));
    telephone.set_announce_interval(Duration::from_secs(600));
    telephone.set_busy_tone_duration(Duration::ZERO);
    telephone.set_transmit_start_skip(Duration::from_millis(10));
    telephone.set_transmit_start_ease_in(Duration::from_millis(20));

    assert_eq!(telephone.receive_gain_db(), 3.0);
    assert_eq!(telephone.transmit_gain_db(), -2.5);
    assert!(!telephone.use_agc());
    assert!(telephone.receive_muted());
    assert!(telephone.transmit_muted());
    assert_eq!(telephone.config().connect_time, Duration::from_secs(2));
    assert_eq!(
        telephone.config().announce_interval,
        Duration::from_secs(600)
    );
    assert_eq!(telephone.busy_tone_duration(), Duration::ZERO);
    assert_eq!(telephone.transmit_start_skip(), Duration::from_millis(10));
    assert_eq!(
        telephone.transmit_start_ease_in(),
        Duration::from_millis(20)
    );

    let events: Vec<_> = rx.try_iter().collect();
    assert!(events.contains(&lxst::telephony::CallEvent::ReceiveGainChanged(3.0)));
    assert!(events.contains(&lxst::telephony::CallEvent::TransmitGainChanged(-2.5)));
    assert!(events.contains(&lxst::telephony::CallEvent::AgcChanged(false)));
    assert!(events.contains(&lxst::telephony::CallEvent::ReceiveMutedChanged(true)));
    assert!(events.contains(&lxst::telephony::CallEvent::TransmitMutedChanged(true)));
}

#[test]
fn telephone_announce_interval_matches_upstream_minimum() {
    let (mut telephone, _rx) = Telephone::new(TelephoneConfig::default());

    telephone.set_announce_interval(Duration::from_secs(1));

    assert_eq!(
        telephone.config().announce_interval,
        lxst::MIN_ANNOUNCE_INTERVAL
    );
}

#[test]
fn telephone_device_helpers_filter_audio_devices() {
    match Telephone::available_outputs() {
        Ok(outputs) => assert!(outputs
            .iter()
            .all(|device| device.kind == AudioDeviceKind::Output)),
        Err(err) => assert!(!err.to_string().is_empty()),
    }
    match Telephone::available_inputs() {
        Ok(inputs) => assert!(inputs
            .iter()
            .all(|device| device.kind == AudioDeviceKind::Input)),
        Err(err) => assert!(!err.to_string().is_empty()),
    }

    let _ = Telephone::default_output();
    let _ = Telephone::default_input();
}

#[test]
fn telephone_busy_signal_clears_pending_timeout() {
    let remote = [0x24; 16];
    let config = TelephoneConfig {
        wait_time: Duration::ZERO,
        ..TelephoneConfig::default()
    };
    let (mut telephone, rx) = Telephone::new(config);

    assert!(telephone.begin_outgoing_call(remote));
    telephone.apply_signal(Signal::Code(SignalCode::Busy));
    telephone.tick();

    assert_eq!(telephone.state(), CallState::Available);
    let events: Vec<_> = rx.try_iter().collect();
    assert!(events.iter().any(|event| matches!(
        event,
        lxst::telephony::CallEvent::Busy {
            identity_hash: Some(identity)
        } if *identity == remote
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, lxst::telephony::CallEvent::TimedOut { .. })));
}

#[test]
fn telephone_hangup_resets_mute_state() {
    let remote = [0x25; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());
    assert!(telephone.begin_outgoing_call(remote));
    telephone.mute_receive(true);
    telephone.mute_transmit(true);

    telephone.hangup();

    assert!(!telephone.receive_muted());
    assert!(!telephone.transmit_muted());
    let events: Vec<_> = rx.try_iter().collect();
    assert!(events.contains(&lxst::telephony::CallEvent::ReceiveMutedChanged(false)));
    assert!(events.contains(&lxst::telephony::CallEvent::TransmitMutedChanged(false)));
}

#[test]
fn telephone_hangup_resets_active_profile_to_default() {
    let remote = [0x26; 16];
    let (mut telephone, _rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_outgoing_call(remote));
    telephone.apply_signal(Signal::PreferredProfile(CallProfile::UltraLowLatency));
    assert_eq!(telephone.active_profile(), CallProfile::UltraLowLatency);

    telephone.hangup();

    assert_eq!(telephone.state(), CallState::Available);
    assert_eq!(telephone.active_profile(), CallProfile::DEFAULT);
}

#[test]
fn telephone_reject_resets_active_profile_to_default() {
    let caller = [0x27; 16];
    let (mut telephone, _rx) = Telephone::new(TelephoneConfig::default());

    assert!(telephone.begin_incoming_call(caller));
    telephone.apply_signal(Signal::PreferredProfile(CallProfile::LowLatency));
    assert_eq!(telephone.active_profile(), CallProfile::LowLatency);

    telephone.reject();

    assert_eq!(telephone.state(), CallState::Available);
    assert_eq!(telephone.active_profile(), CallProfile::DEFAULT);
}

#[test]
fn caller_policy_list_allows_only_members() {
    let allowed = [0x01; 16];
    let denied = [0x02; 16];
    let mut set = std::collections::HashSet::new();
    set.insert(allowed);
    let policy = CallerPolicy::List(set);
    assert!(policy.allows(&allowed));
    assert!(!policy.allows(&denied));
}

#[test]
fn telephone_busy_signal_returns_to_available() {
    let remote = [0x33; 16];
    let (mut telephone, _rx) = Telephone::new(TelephoneConfig::default());
    assert!(telephone.begin_outgoing_call(remote));
    telephone.apply_signal(Signal::Code(SignalCode::Busy));
    assert_eq!(telephone.state(), CallState::Available);
}

#[test]
fn telephone_external_busy_blocks_incoming_calls() {
    let caller = [0x44; 16];
    let (mut telephone, rx) = Telephone::new(TelephoneConfig::default());
    telephone.set_busy(true);

    assert!(!telephone.begin_incoming_call(caller));
    assert_eq!(telephone.state(), CallState::Available);
    assert!(matches!(
        rx.recv().unwrap(),
        lxst::telephony::CallEvent::Busy {
            identity_hash: Some(identity)
        } if identity == caller
    ));
}

#[test]
fn telephone_tick_times_out_pending_call() {
    let remote = [0x55; 16];
    let config = TelephoneConfig {
        wait_time: Duration::ZERO,
        ..TelephoneConfig::default()
    };
    let (mut telephone, rx) = Telephone::new(config);

    assert!(telephone.begin_outgoing_call(remote));
    telephone.tick();
    assert_eq!(telephone.state(), CallState::Available);
    assert!(rx
        .try_iter()
        .any(|event| matches!(event, lxst::telephony::CallEvent::TimedOut { .. })));
}

#[test]
fn telephone_tick_auto_answers_ringing_call() {
    let caller = [0x66; 16];
    let config = TelephoneConfig {
        auto_answer_after: Some(Duration::ZERO),
        ..TelephoneConfig::default()
    };
    let (mut telephone, _rx) = Telephone::new(config);

    assert!(telephone.begin_incoming_call(caller));
    telephone.tick();
    assert_eq!(telephone.state(), CallState::Connecting);
}

struct CountingFilter {
    calls: Arc<AtomicUsize>,
}

impl AudioFilter for CountingFilter {
    fn process(&mut self, frame: AudioFrame) -> AudioFrame {
        self.calls.fetch_add(1, Ordering::SeqCst);
        frame
    }
}

struct AddFilter {
    amount: f32,
}

impl AudioFilter for AddFilter {
    fn process(&mut self, frame: AudioFrame) -> AudioFrame {
        frame.map_samples(|sample| sample + self.amount)
    }
}

struct FakeMixerSink {
    frames: Arc<Mutex<Vec<AudioFrame>>>,
    accepting: Arc<AtomicBool>,
}

impl FakeMixerSink {
    fn new(accepting: bool) -> Self {
        Self {
            frames: Arc::new(Mutex::new(Vec::new())),
            accepting: Arc::new(AtomicBool::new(accepting)),
        }
    }
}

impl MixerSink for FakeMixerSink {
    fn can_receive(&self) -> bool {
        self.accepting.load(Ordering::SeqCst)
    }

    fn handle_frame(&mut self, frame: AudioFrame) -> Result<(), lxst::AudioError> {
        self.frames.lock().unwrap().push(frame);
        Ok(())
    }
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[derive(Clone, Default)]
struct FakePlayback {
    played: Arc<Mutex<Vec<AudioFrame>>>,
    low_latency_requests: Arc<AtomicUsize>,
}

impl LinePlayback for FakePlayback {
    fn play(&mut self, frame: AudioFrame) -> Result<(), lxst::AudioError> {
        self.played.lock().unwrap().push(frame);
        Ok(())
    }

    fn enable_low_latency(&mut self) -> Result<bool, lxst::AudioError> {
        self.low_latency_requests.fetch_add(1, Ordering::SeqCst);
        Ok(true)
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
