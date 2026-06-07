use lxst::audio::AudioFilter;
use lxst::{
    Agc, AudioCodec, AudioFrame, CallProfile, CallState, CallerPolicy, CodecError, Mixer,
    RawBitDepth, RawCodec, Signal, SignalCode, Telephone, TelephoneConfig, ToneSource,
};

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
fn mixer_sums_and_clips_frames() {
    let mut mixer = Mixer::default();
    mixer.push(1, AudioFrame::new(8_000, 1, vec![0.75, 0.25]).unwrap());
    mixer.push(2, AudioFrame::new(8_000, 1, vec![0.75, -0.5]).unwrap());

    let mixed = mixer.mix_next().unwrap().unwrap();
    assert_eq!(mixed.samples(), &[1.0, -0.25]);
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
        .all(|sample| (-1.0..=1.0).contains(sample)));
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
