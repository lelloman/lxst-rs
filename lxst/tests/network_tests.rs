use lxst::{
    AudioSink, EncodedAudioFrame, NetworkError, PacketSender, Packetizer, RawBitDepth, RawCodec,
};
use lxst_core::{CodecKind, LxstPacket};

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
