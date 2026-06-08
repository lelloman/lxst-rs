use lxst::{
    AudioSink, AudioSource, EncodedAudioFrame, LinkSource, NetworkError, PacketSender, Packetizer,
    RawBitDepth, RawCodec,
};
use lxst_core::{CodecKind, EncodedFrame, LxstPacket, Signal, SignalCode};

#[derive(Debug, Default)]
struct MockSender {
    packets: Vec<LxstPacket>,
    fail: bool,
}

impl PacketSender for MockSender {
    fn send_packet(&mut self, packet: &LxstPacket) -> Result<(), NetworkError> {
        if self.fail {
            Err(NetworkError::Send)
        } else {
            self.packets.push(packet.clone());
            Ok(())
        }
    }
}

#[test]
fn packetizer_wraps_encoded_frame_in_lxst_packet() {
    let mut raw = RawCodec::default();
    let payload = lxst::AudioCodec::encode(
        &mut raw,
        &lxst::AudioFrame::new(8_000, 1, vec![0.0, 0.25]).unwrap(),
    )
    .unwrap();
    let mut packetizer = Packetizer::new(MockSender::default());

    packetizer
        .handle_frame(EncodedAudioFrame {
            codec: CodecKind::Raw,
            samplerate: 8_000,
            channels: 1,
            payload: payload.clone(),
        })
        .unwrap();

    let sender = packetizer.into_sender();
    assert_eq!(sender.packets.len(), 1);
    assert_eq!(sender.packets[0].frames.len(), 1);
    assert_eq!(sender.packets[0].frames[0].codec, CodecKind::Raw);
    assert_eq!(sender.packets[0].frames[0].payload, payload);
}

#[test]
fn packetizer_records_transmit_failure() {
    let mut packetizer = Packetizer::new(MockSender {
        fail: true,
        ..MockSender::default()
    });

    let result = packetizer.handle_frame(EncodedAudioFrame {
        codec: CodecKind::Raw,
        samplerate: 8_000,
        channels: 1,
        payload: vec![RawBitDepth::Float32 as u8],
    });

    assert!(result.is_err());
    assert!(packetizer.transmit_failure());
    assert!(!packetizer.can_receive());
}

#[test]
fn link_source_decodes_packet_frames_and_signals() {
    let mut raw = RawCodec::default();
    let payload = lxst::AudioCodec::encode(
        &mut raw,
        &lxst::AudioFrame::new(8_000, 1, vec![0.0, 0.25]).unwrap(),
    )
    .unwrap();
    let packet = LxstPacket {
        signals: vec![Signal::Code(SignalCode::Ringing)],
        frames: vec![EncodedFrame::new(CodecKind::Raw, payload)],
    };
    let encoded = packet.encode().unwrap();
    let mut source = LinkSource::with_null_codec(8_000, 1);

    source.handle_packet_bytes(&encoded).unwrap();
    assert_eq!(source.queued_signals(), 1);
    assert_eq!(source.pop_signal(), Some(Signal::Code(SignalCode::Ringing)));
    assert_eq!(source.queued_frames(), 1);

    source.start();
    let frame = source.next_frame().unwrap().unwrap();
    assert_eq!(frame.samplerate(), 8_000);
    assert_eq!(frame.channels(), 1);
    assert_eq!(frame.samples(), &[0.0, 0.25]);
}

#[test]
fn link_source_applies_frame_backpressure() {
    let mut raw = RawCodec::default();
    let payload = lxst::AudioCodec::encode(
        &mut raw,
        &lxst::AudioFrame::new(8_000, 1, vec![0.0]).unwrap(),
    )
    .unwrap();
    let mut source = LinkSource::with_null_codec(8_000, 1);
    source.set_max_frames(1);

    source
        .handle_packet(LxstPacket::frame(EncodedFrame::new(
            CodecKind::Raw,
            payload.clone(),
        )))
        .unwrap();
    source
        .handle_packet(LxstPacket::frame(EncodedFrame::new(
            CodecKind::Raw,
            payload,
        )))
        .unwrap();

    assert_eq!(source.queued_frames(), 1);
}
