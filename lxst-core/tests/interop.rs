use lxst_core::{
    CallProfile, CodecHeader, CodecKind, EncodedFrame, LxstPacket, RawBitDepth, RawFrameHeader,
    Signal, SignalCode, FIELD_FRAMES, FIELD_SIGNALLING, PREFERRED_PROFILE_BASE,
};
use rns_core::msgpack::{pack, unpack_exact, Value};

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
