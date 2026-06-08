use lxst::audio::AudioFilter;
use lxst::{
    Agc, AudioCodec, AudioDeviceKind, AudioFrame, CallProfile, CallState, CallerPolicy,
    Codec2Codec, CodecError, Mixer, OpusCodec, RawBitDepth, RawCodec, Signal, SignalCode,
    Telephone, TelephoneConfig, ToneSource,
};
use lxst_core::CodecProfile;
use std::time::Duration;

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
fn raw_codec_rejects_unsupported_float16_encode() {
    let frame = AudioFrame::new(48_000, 1, vec![0.0]).unwrap();
    let mut codec = RawCodec::new(RawBitDepth::Float16);
    assert!(matches!(
        codec.encode(&frame),
        Err(CodecError::Unsupported(_))
    ));
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
fn tone_source_generates_expected_shape() {
    let mut tone = ToneSource::new(1_000.0, 8_000, 2, 0.5);
    let frame = tone.next_frame(8).unwrap();
    assert_eq!(frame.channels(), 2);
    assert_eq!(frame.frame_count(), 8);
    assert_eq!(frame.samples().len(), 16);
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
fn telephone_applies_profile_signalling() {
    let (mut telephone, _rx) = Telephone::new(TelephoneConfig::default());
    telephone.apply_signal(Signal::PreferredProfile(CallProfile::LowLatency));
    assert_eq!(telephone.active_profile(), CallProfile::LowLatency);
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
    telephone.set_busy_tone_duration(Duration::ZERO);
    telephone.set_transmit_start_skip(Duration::from_millis(10));
    telephone.set_transmit_start_ease_in(Duration::from_millis(20));

    assert_eq!(telephone.receive_gain_db(), 3.0);
    assert_eq!(telephone.transmit_gain_db(), -2.5);
    assert!(!telephone.use_agc());
    assert!(telephone.receive_muted());
    assert!(telephone.transmit_muted());
    assert_eq!(telephone.config().connect_time, Duration::from_secs(2));
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
