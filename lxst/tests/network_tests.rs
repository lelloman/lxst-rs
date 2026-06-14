use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use lxst::network::{
    create_telephony_link, recall_telephony_identity, request_path_until, telephony_dest_hash,
};
use lxst::{
    AudioSink, AudioSource, EncodedAudioFrame, LinkSource, NetworkError, PacketSender, Packetizer,
    RawBitDepth, RawCodec, TelephonyCallbacks, TelephonyEndpoint, TelephonyNetworkEvent,
    TelephonyNode,
};
use lxst_core::{CodecKind, EncodedFrame, LxstPacket, Signal, SignalCode};
use rns_crypto::identity::Identity;
use rns_net::{AnnouncedIdentity, Callbacks, DestHash, Destination, IdentityHash, InterfaceId};

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
fn signalling_packet_round_trips_call_state() {
    let packet = LxstPacket::signalling(Signal::Code(SignalCode::Established));
    let decoded = LxstPacket::decode(&packet.encode().unwrap()).unwrap();

    assert_eq!(decoded.signals, vec![Signal::Code(SignalCode::Established)]);
    assert!(decoded.frames.is_empty());
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

#[test]
fn telephony_callbacks_forward_link_data_events() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut callbacks = TelephonyCallbacks::new(tx);
    let link_id = rns_net::LinkId([0x77; 16]);

    callbacks.on_link_data(link_id, 0, vec![1, 2, 3]);

    assert!(matches!(
        rx.recv().unwrap(),
        TelephonyNetworkEvent::LinkData {
            link_id: id,
            context: 0,
            data
        } if id == link_id && data == vec![1, 2, 3]
    ));
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FakeOperation {
    RegisterDestination {
        dest_hash: [u8; 16],
        signing_key: Option<[u8; 64]>,
    },
    RegisterLinkDestination {
        dest_hash: [u8; 16],
        sig_prv: [u8; 32],
        sig_pub: [u8; 32],
        resource_strategy: u8,
    },
    Announce {
        dest_hash: [u8; 16],
        app_data: Option<Vec<u8>>,
    },
    HasPath([u8; 16]),
    RequestPath([u8; 16]),
    RecallIdentity([u8; 16]),
    CreateLink {
        dest_hash: [u8; 16],
        sig_pub: [u8; 32],
    },
}

#[derive(Debug)]
struct FakeTelephonyNode {
    operations: RefCell<Vec<FakeOperation>>,
    path_results: RefCell<VecDeque<bool>>,
    recalled_identity: RefCell<Option<AnnouncedIdentity>>,
    link_id: [u8; 16],
}

impl Default for FakeTelephonyNode {
    fn default() -> Self {
        Self {
            operations: RefCell::new(Vec::new()),
            path_results: RefCell::new(VecDeque::new()),
            recalled_identity: RefCell::new(None),
            link_id: [0xAB; 16],
        }
    }
}

impl FakeTelephonyNode {
    fn with_path_results(results: impl IntoIterator<Item = bool>) -> Self {
        Self {
            path_results: RefCell::new(results.into_iter().collect()),
            ..Self::default()
        }
    }

    fn operations(&self) -> Vec<FakeOperation> {
        self.operations.borrow().clone()
    }

    fn set_recalled_identity(&self, announced: AnnouncedIdentity) {
        *self.recalled_identity.borrow_mut() = Some(announced);
    }
}

impl TelephonyNode for FakeTelephonyNode {
    fn register_destination_with_proof(
        &self,
        destination: &Destination,
        signing_key: Option<[u8; 64]>,
    ) -> Result<(), NetworkError> {
        self.operations
            .borrow_mut()
            .push(FakeOperation::RegisterDestination {
                dest_hash: destination.hash.0,
                signing_key,
            });
        Ok(())
    }

    fn register_link_destination(
        &self,
        dest_hash: [u8; 16],
        sig_prv_bytes: [u8; 32],
        sig_pub_bytes: [u8; 32],
        resource_strategy: u8,
    ) -> Result<(), NetworkError> {
        self.operations
            .borrow_mut()
            .push(FakeOperation::RegisterLinkDestination {
                dest_hash,
                sig_prv: sig_prv_bytes,
                sig_pub: sig_pub_bytes,
                resource_strategy,
            });
        Ok(())
    }

    fn announce(
        &self,
        destination: &Destination,
        _identity: &Identity,
        app_data: Option<&[u8]>,
    ) -> Result<(), NetworkError> {
        self.operations.borrow_mut().push(FakeOperation::Announce {
            dest_hash: destination.hash.0,
            app_data: app_data.map(Vec::from),
        });
        Ok(())
    }

    fn has_path(&self, dest_hash: &DestHash) -> Result<bool, NetworkError> {
        self.operations
            .borrow_mut()
            .push(FakeOperation::HasPath(dest_hash.0));
        Ok(self.path_results.borrow_mut().pop_front().unwrap_or(false))
    }

    fn request_path(&self, dest_hash: &DestHash) -> Result<(), NetworkError> {
        self.operations
            .borrow_mut()
            .push(FakeOperation::RequestPath(dest_hash.0));
        Ok(())
    }

    fn recall_identity(
        &self,
        dest_hash: &DestHash,
    ) -> Result<Option<AnnouncedIdentity>, NetworkError> {
        self.operations
            .borrow_mut()
            .push(FakeOperation::RecallIdentity(dest_hash.0));
        Ok(self.recalled_identity.borrow().clone())
    }

    fn create_link(
        &self,
        dest_hash: [u8; 16],
        dest_sig_pub_bytes: [u8; 32],
    ) -> Result<[u8; 16], NetworkError> {
        self.operations
            .borrow_mut()
            .push(FakeOperation::CreateLink {
                dest_hash,
                sig_pub: dest_sig_pub_bytes,
            });
        Ok(self.link_id)
    }
}

fn fixed_identity() -> Identity {
    Identity::from_private_key(&[0x11; 64])
}

fn announced_identity(identity_hash: [u8; 16], public_key: [u8; 64]) -> AnnouncedIdentity {
    AnnouncedIdentity {
        dest_hash: telephony_dest_hash(identity_hash),
        identity_hash: IdentityHash(identity_hash),
        public_key,
        app_data: None,
        hops: 1,
        received_at: 0.0,
        receiving_interface: InterfaceId(0),
        rssi: None,
        snr: None,
    }
}

#[test]
fn telephony_endpoint_registers_destination_and_link_material() {
    let identity = fixed_identity();
    let endpoint = TelephonyEndpoint::new(&identity);
    let node = FakeTelephonyNode::default();

    endpoint.register(&node, &identity).unwrap();

    let private_key = identity.get_private_key().unwrap();
    let public_key = identity.get_public_key().unwrap();
    assert_eq!(
        node.operations(),
        vec![
            FakeOperation::RegisterDestination {
                dest_hash: endpoint.destination.hash.0,
                signing_key: Some(private_key),
            },
            FakeOperation::RegisterLinkDestination {
                dest_hash: endpoint.destination.hash.0,
                sig_prv: private_key[32..64].try_into().unwrap(),
                sig_pub: public_key[32..64].try_into().unwrap(),
                resource_strategy: 0,
            },
        ]
    );
}

#[test]
fn telephony_endpoint_announces_destination_without_app_data() {
    let identity = fixed_identity();
    let endpoint = TelephonyEndpoint::new(&identity);
    let node = FakeTelephonyNode::default();

    endpoint.announce(&node, &identity).unwrap();

    assert_eq!(
        node.operations(),
        vec![FakeOperation::Announce {
            dest_hash: endpoint.destination.hash.0,
            app_data: None,
        }]
    );
}

#[test]
fn request_path_until_returns_immediately_for_known_path() {
    let node = FakeTelephonyNode::with_path_results([true]);
    let dest_hash = DestHash([0x22; 16]);

    assert!(request_path_until(&node, dest_hash, Duration::ZERO, Duration::ZERO).unwrap());

    assert_eq!(node.operations(), vec![FakeOperation::HasPath(dest_hash.0)]);
}

#[test]
fn request_path_until_requests_unknown_path_before_rechecking() {
    let node = FakeTelephonyNode::with_path_results([false, true]);
    let dest_hash = DestHash([0x33; 16]);

    assert!(request_path_until(&node, dest_hash, Duration::ZERO, Duration::ZERO).unwrap());

    assert_eq!(
        node.operations(),
        vec![
            FakeOperation::HasPath(dest_hash.0),
            FakeOperation::RequestPath(dest_hash.0),
            FakeOperation::HasPath(dest_hash.0),
        ]
    );
}

#[test]
fn recall_telephony_identity_uses_derived_telephony_destination() {
    let identity_hash = [0x44; 16];
    let announced = announced_identity(identity_hash, [0x55; 64]);
    let node = FakeTelephonyNode::default();
    node.set_recalled_identity(announced.clone());

    let recalled = recall_telephony_identity(&node, identity_hash)
        .unwrap()
        .unwrap();

    assert_eq!(recalled.dest_hash.0, announced.dest_hash.0);
    assert_eq!(recalled.identity_hash.0, identity_hash);
    assert_eq!(
        node.operations(),
        vec![FakeOperation::RecallIdentity(
            telephony_dest_hash(identity_hash).0
        )]
    );
}

#[test]
fn create_telephony_link_uses_announced_destination_and_signing_public_key() {
    let identity_hash = [0x66; 16];
    let mut public_key = [0u8; 64];
    for (index, byte) in public_key.iter_mut().enumerate() {
        *byte = index as u8;
    }
    let announced = announced_identity(identity_hash, public_key);
    let node = FakeTelephonyNode::default();

    let link_id = create_telephony_link(&node, &announced).unwrap();

    assert_eq!(link_id, [0xAB; 16]);
    assert_eq!(
        node.operations(),
        vec![FakeOperation::CreateLink {
            dest_hash: announced.dest_hash.0,
            sig_pub: public_key[32..64].try_into().unwrap(),
        }]
    );
}
