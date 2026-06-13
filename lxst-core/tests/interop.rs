use lxst_core::{
    CallProfile, CodecHeader, CodecKind, CodecProfile, EncodedFrame, LxstPacket, OpusApplication,
    RawBitDepth, RawFrameHeader, Signal, SignalCode, FIELD_FRAMES, FIELD_SIGNALLING,
    PREFERRED_PROFILE_BASE,
};
use rns_core::msgpack::{pack, unpack_exact, Value};

#[derive(Debug)]
struct UpstreamCoreFixture {
    source_commit: &'static str,
    fields: FieldFixture,
    codec_headers: CodecHeaderFixture,
    signals: &'static [NamedValue],
    profiles: &'static [ProfileFixture],
    opus_profiles: &'static [OpusProfileFixture],
    codec2_modes: &'static [Codec2ModeFixture],
    raw_frame_headers: &'static [RawFrameHeaderFixture],
    packet_cases: &'static [PacketFixture],
}

#[derive(Debug)]
struct FieldFixture {
    signalling: u8,
    frames: u8,
}

#[derive(Debug)]
struct CodecHeaderFixture {
    raw: u8,
    opus: u8,
    codec2: u8,
    null: u8,
}

#[derive(Debug)]
struct NamedValue {
    name: &'static str,
    value: u16,
}

#[derive(Debug)]
struct ProfileFixture {
    name: &'static str,
    value: u8,
    index: usize,
    display_name: &'static str,
    abbreviation: &'static str,
    frame_time_ms: u16,
    next_value: u8,
}

#[derive(Debug)]
struct OpusProfileFixture {
    name: &'static str,
    value: u8,
    channels: u8,
    samplerate: u32,
    application: &'static str,
    bitrate_ceiling: u32,
}

#[derive(Debug)]
struct Codec2ModeFixture {
    mode: u16,
    header: u8,
}

#[derive(Debug)]
struct RawFrameHeaderFixture {
    bitdepth_name: &'static str,
    bitdepth_value: u8,
    channels: u8,
    header: u8,
}

#[derive(Debug)]
struct PacketFixture {
    name: &'static str,
    hex: &'static str,
}

fn upstream_fixture() -> UpstreamCoreFixture {
    include!("fixtures/upstream_core.rs")
}

fn fixture_value(values: &[NamedValue], name: &str) -> u16 {
    values
        .iter()
        .find(|value| value.name == name)
        .unwrap_or_else(|| panic!("missing upstream fixture value {name}"))
        .value
}

fn call_profile_from_upstream(name: &str) -> CallProfile {
    match name {
        "BANDWIDTH_ULTRA_LOW" => CallProfile::UltraLowBandwidth,
        "BANDWIDTH_VERY_LOW" => CallProfile::VeryLowBandwidth,
        "BANDWIDTH_LOW" => CallProfile::LowBandwidth,
        "QUALITY_MEDIUM" => CallProfile::MediumQuality,
        "QUALITY_HIGH" => CallProfile::HighQuality,
        "QUALITY_MAX" => CallProfile::MaxQuality,
        "LATENCY_LOW" => CallProfile::LowLatency,
        "LATENCY_ULTRA_LOW" => CallProfile::UltraLowLatency,
        other => panic!("unknown upstream call profile {other}"),
    }
}

fn opus_profile_from_upstream(name: &str) -> CodecProfile {
    match name {
        "PROFILE_VOICE_LOW" => CodecProfile::OpusVoiceLow,
        "PROFILE_VOICE_MEDIUM" => CodecProfile::OpusVoiceMedium,
        "PROFILE_VOICE_HIGH" => CodecProfile::OpusVoiceHigh,
        "PROFILE_VOICE_MAX" => CodecProfile::OpusVoiceMax,
        "PROFILE_AUDIO_MIN" => CodecProfile::OpusAudioMin,
        "PROFILE_AUDIO_LOW" => CodecProfile::OpusAudioLow,
        "PROFILE_AUDIO_MEDIUM" => CodecProfile::OpusAudioMedium,
        "PROFILE_AUDIO_HIGH" => CodecProfile::OpusAudioHigh,
        "PROFILE_AUDIO_MAX" => CodecProfile::OpusAudioMax,
        other => panic!("unknown upstream opus profile {other}"),
    }
}

fn decode_hex(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0, "hex fixtures must have even length");
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
        .collect()
}

#[test]
fn field_constants_match_python_lxst() {
    assert_eq!(FIELD_SIGNALLING, 0x00);
    assert_eq!(FIELD_FRAMES, 0x01);
    assert_eq!(CodecHeader::Raw.as_u8(), 0x00);
    assert_eq!(CodecHeader::Opus.as_u8(), 0x01);
    assert_eq!(CodecHeader::Codec2.as_u8(), 0x02);
    assert_eq!(CodecHeader::Null.as_u8(), 0xff);
}

#[test]
fn generated_fixture_is_from_current_upstream_shape() {
    let fixture = upstream_fixture();

    assert_eq!(fixture.source_commit.len(), 40);
    assert_eq!(fixture.fields.signalling, FIELD_SIGNALLING);
    assert_eq!(fixture.fields.frames, FIELD_FRAMES);
    assert_eq!(fixture.codec_headers.raw, CodecHeader::Raw.as_u8());
    assert_eq!(fixture.codec_headers.opus, CodecHeader::Opus.as_u8());
    assert_eq!(fixture.codec_headers.codec2, CodecHeader::Codec2.as_u8());
    assert_eq!(fixture.codec_headers.null, CodecHeader::Null.as_u8());
    assert_eq!(
        fixture_value(fixture.signals, "PREFERRED_PROFILE"),
        PREFERRED_PROFILE_BASE
    );
    assert_eq!(
        fixture
            .codec2_modes
            .iter()
            .map(|mode| (mode.mode, mode.header))
            .collect::<Vec<_>>(),
        vec![
            (700, 0),
            (1200, 1),
            (1300, 2),
            (1400, 3),
            (1600, 4),
            (2400, 5),
            (3200, 6),
        ]
    );
    assert!(fixture.packet_cases.len() >= 4);
}

#[test]
fn telephony_profiles_match_python_lxst() {
    let profiles = CallProfile::available_profiles();
    let values: Vec<u8> = profiles.iter().map(|profile| profile.as_u8()).collect();
    assert_eq!(values, vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x80, 0x70]);
    assert_eq!(CallProfile::LowLatency.as_u8(), 0x80);
    assert_eq!(CallProfile::UltraLowLatency.as_u8(), 0x70);
    assert_eq!(CallProfile::MediumQuality.profile_index(), 3);
    assert_eq!(
        CallProfile::LowLatency.next_profile(),
        CallProfile::UltraLowLatency
    );
    assert_eq!(
        CallProfile::UltraLowLatency.next_profile(),
        CallProfile::UltraLowBandwidth
    );
    assert_eq!(CallProfile::LowLatency.frame_duration().as_millis(), 20);
    assert_eq!(
        CallProfile::UltraLowLatency.frame_duration().as_millis(),
        10
    );
}

#[test]
fn call_profiles_match_generated_python_fixture() {
    let fixture = upstream_fixture();
    let values: Vec<u8> = fixture
        .profiles
        .iter()
        .map(|profile| profile.value)
        .collect();
    assert_eq!(
        values,
        CallProfile::available_profiles()
            .iter()
            .map(|profile| profile.as_u8())
            .collect::<Vec<_>>()
    );

    for profile in fixture.profiles {
        let rust_profile = call_profile_from_upstream(profile.name);
        assert_eq!(rust_profile.as_u8(), profile.value);
        assert_eq!(rust_profile.profile_index(), profile.index);
        assert_eq!(rust_profile.name(), profile.display_name);
        assert_eq!(rust_profile.abbreviation(), profile.abbreviation);
        assert_eq!(
            rust_profile.frame_duration().as_millis(),
            profile.frame_time_ms
        );
        assert_eq!(rust_profile.next_profile().as_u8(), profile.next_value);
    }
}

#[test]
fn opus_profiles_match_generated_python_fixture() {
    let fixture = upstream_fixture();
    assert_eq!(fixture.opus_profiles.len(), 9);

    for profile in fixture.opus_profiles {
        assert!(profile.value <= 0x08);
        let info = opus_profile_from_upstream(profile.name).info();
        assert_eq!(info.channels, profile.channels, "{profile:?}");
        assert_eq!(info.samplerate, profile.samplerate, "{profile:?}");
        assert_eq!(info.bitrate_ceiling, profile.bitrate_ceiling, "{profile:?}");
        assert_eq!(
            info.opus_application,
            Some(match profile.application {
                "voip" => OpusApplication::Voip,
                "audio" => OpusApplication::Audio,
                other => panic!("unknown upstream opus application {other}"),
            }),
            "{profile:?}"
        );
    }
}

#[test]
fn preferred_profile_signalling_uses_python_offset() {
    let signal = Signal::PreferredProfile(CallProfile::MediumQuality);
    assert_eq!(signal.to_wire_value(), PREFERRED_PROFILE_BASE as u64 + 0x40);
    assert_eq!(Signal::from_wire_value(0xff + 0x40), signal);
    assert_eq!(
        Signal::from_wire_value(0xff + 0x70),
        Signal::PreferredProfile(CallProfile::UltraLowLatency)
    );
    assert_eq!(
        Signal::from_wire_value(0xff + 0x80),
        Signal::PreferredProfile(CallProfile::LowLatency)
    );
}

#[test]
fn raw_frame_header_matches_python_layout() {
    let header = RawFrameHeader::new(RawBitDepth::Float32, 2).unwrap();
    assert_eq!(header.encode(), 0b0100_0001);
    assert_eq!(RawFrameHeader::decode(0b0100_0001).unwrap(), header);

    let header = RawFrameHeader::new(RawBitDepth::Float16, 32).unwrap();
    assert_eq!(header.encode(), 31);
    assert_eq!(RawFrameHeader::decode(31).unwrap(), header);
}

#[test]
fn raw_frame_headers_match_generated_python_fixture() {
    let fixture = upstream_fixture();

    for header in fixture.raw_frame_headers {
        let bit_depth = match header.bitdepth_name {
            "BITDEPTH_16" => RawBitDepth::Float16,
            "BITDEPTH_32" => RawBitDepth::Float32,
            "BITDEPTH_64" => RawBitDepth::Float64,
            "BITDEPTH_128" => RawBitDepth::Float128,
            other => panic!("unknown upstream raw bit depth {other}"),
        };
        assert_eq!(bit_depth as u8, header.bitdepth_value);
        let rust_header = RawFrameHeader::new(bit_depth, header.channels).unwrap();
        assert_eq!(rust_header.encode(), header.header);
        assert_eq!(RawFrameHeader::decode(header.header).unwrap(), rust_header);
    }
}

#[test]
fn encodes_single_frame_like_python_packetizer() {
    let packet = LxstPacket::frame(EncodedFrame::new(CodecKind::Opus, vec![0xaa, 0xbb]));
    let encoded = packet.encode().unwrap();

    let expected = pack(&Value::Map(vec![(
        Value::UInt(FIELD_FRAMES as u64),
        Value::Bin(vec![0x01, 0xaa, 0xbb]),
    )]));
    assert_eq!(encoded, expected);
}

#[test]
fn decodes_generated_python_packet_fixtures() {
    let fixture = upstream_fixture();

    for packet in fixture.packet_cases {
        let decoded = LxstPacket::decode(&decode_hex(packet.hex)).unwrap();
        match packet.name {
            "single_opus_frame" => {
                assert!(decoded.signals.is_empty());
                assert_eq!(
                    decoded.frames,
                    vec![EncodedFrame::new(CodecKind::Opus, vec![0xaa, 0xbb])]
                );
                assert_eq!(decoded.encode().unwrap(), decode_hex(packet.hex));
            }
            "scalar_ringing_signal" => {
                assert_eq!(decoded.signals, vec![Signal::Code(SignalCode::Ringing)]);
                assert!(decoded.frames.is_empty());
            }
            "calling_with_preferred_medium_quality" => {
                assert_eq!(
                    decoded.signals,
                    vec![
                        Signal::Code(SignalCode::Calling),
                        Signal::PreferredProfile(CallProfile::MediumQuality)
                    ]
                );
                assert!(decoded.frames.is_empty());
                assert_eq!(decoded.encode().unwrap(), decode_hex(packet.hex));
            }
            "mixed_raw_codec2_frames" => {
                assert!(decoded.signals.is_empty());
                assert_eq!(
                    decoded.frames,
                    vec![
                        EncodedFrame::new(CodecKind::Raw, vec![0x10]),
                        EncodedFrame::new(CodecKind::Codec2, vec![0x20]),
                    ]
                );
                assert_eq!(decoded.encode().unwrap(), decode_hex(packet.hex));
            }
            other => panic!("unhandled upstream packet fixture {other}"),
        }
    }
}

#[test]
fn decodes_scalar_and_list_packet_fields() {
    let value = Value::Map(vec![
        (Value::UInt(FIELD_SIGNALLING as u64), Value::UInt(0x04)),
        (
            Value::UInt(FIELD_FRAMES as u64),
            Value::Array(vec![
                Value::Bin(vec![0x00, 0x10]),
                Value::Bin(vec![0x02, 0x20]),
            ]),
        ),
    ]);
    let bytes = pack(&value);
    let packet = LxstPacket::decode(&bytes).unwrap();

    assert_eq!(packet.signals, vec![Signal::Code(SignalCode::Ringing)]);
    assert_eq!(packet.frames.len(), 2);
    assert_eq!(
        packet.frames[0],
        EncodedFrame::new(CodecKind::Raw, vec![0x10])
    );
    assert_eq!(
        packet.frames[1],
        EncodedFrame::new(CodecKind::Codec2, vec![0x20])
    );
}

#[test]
fn ignores_unknown_packet_fields() {
    let value = Value::Map(vec![
        (Value::UInt(99), Value::Str("ignored".to_string())),
        (
            Value::UInt(FIELD_SIGNALLING as u64),
            Value::Array(vec![Value::UInt(0x06)]),
        ),
    ]);
    let packet = LxstPacket::decode(&pack(&value)).unwrap();
    assert_eq!(packet.signals, vec![Signal::Code(SignalCode::Established)]);
    assert!(packet.frames.is_empty());
}

#[test]
fn rejects_empty_or_unknown_codec_frames() {
    let empty = Value::Map(vec![(Value::UInt(FIELD_FRAMES as u64), Value::Bin(vec![]))]);
    assert!(LxstPacket::decode(&pack(&empty)).is_err());

    let unknown = Value::Map(vec![(
        Value::UInt(FIELD_FRAMES as u64),
        Value::Bin(vec![0x7f]),
    )]);
    assert!(LxstPacket::decode(&pack(&unknown)).is_err());
}

#[test]
fn encoded_packet_round_trip_preserves_values() {
    let packet = LxstPacket {
        signals: vec![
            Signal::Code(SignalCode::Calling),
            Signal::PreferredProfile(CallProfile::HighQuality),
        ],
        frames: vec![EncodedFrame::new(CodecKind::Raw, vec![0x01, 0x02, 0x03])],
    };
    let encoded = packet.encode().unwrap();
    let decoded_value = unpack_exact(&encoded).unwrap();
    assert!(decoded_value.as_map().is_some());
    assert_eq!(LxstPacket::decode(&encoded).unwrap(), packet);
}
