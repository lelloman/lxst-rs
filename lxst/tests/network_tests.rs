use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
use rns_crypto::OsRng;
use rns_net::{
    AnnouncedIdentity, Callbacks, DestHash, Destination, IdentityHash, InterfaceConfig,
    InterfaceId, NodeConfig, RnsNode, TcpClientConfig, TcpServerConfig, MODE_FULL,
};

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

#[derive(Debug, Clone)]
enum SmokeEvent {
    Announce(AnnouncedIdentity),
    InterfaceUp,
    LinkEstablished {
        link_id: [u8; 16],
        is_initiator: bool,
    },
    LinkClosed {
        link_id: [u8; 16],
    },
    RemoteIdentified {
        link_id: [u8; 16],
        identity_hash: IdentityHash,
    },
    LinkData {
        link_id: [u8; 16],
        context: u8,
        data: Vec<u8>,
    },
}

struct SmokeCallbacks {
    tx: mpsc::Sender<SmokeEvent>,
}

impl SmokeCallbacks {
    fn new(tx: mpsc::Sender<SmokeEvent>) -> Self {
        Self { tx }
    }
}

impl Callbacks for SmokeCallbacks {
    fn on_announce(&mut self, announced: AnnouncedIdentity) {
        let _ = self.tx.send(SmokeEvent::Announce(announced));
    }

    fn on_path_updated(&mut self, _dest_hash: DestHash, _hops: u8) {}

    fn on_local_delivery(
        &mut self,
        _dest_hash: DestHash,
        _raw: Vec<u8>,
        _packet_hash: rns_net::PacketHash,
    ) {
    }

    fn on_interface_up(&mut self, _id: InterfaceId) {
        let _ = self.tx.send(SmokeEvent::InterfaceUp);
    }

    fn on_link_established(
        &mut self,
        link_id: rns_net::LinkId,
        _dest_hash: DestHash,
        _rtt: f64,
        is_initiator: bool,
    ) {
        let _ = self.tx.send(SmokeEvent::LinkEstablished {
            link_id: link_id.0,
            is_initiator,
        });
    }

    fn on_link_closed(
        &mut self,
        link_id: rns_net::LinkId,
        _reason: Option<rns_net::TeardownReason>,
    ) {
        let _ = self.tx.send(SmokeEvent::LinkClosed { link_id: link_id.0 });
    }

    fn on_remote_identified(
        &mut self,
        link_id: rns_net::LinkId,
        identity_hash: IdentityHash,
        _public_key: [u8; 64],
    ) {
        let _ = self.tx.send(SmokeEvent::RemoteIdentified {
            link_id: link_id.0,
            identity_hash,
        });
    }

    fn on_link_data(&mut self, link_id: rns_net::LinkId, context: u8, data: Vec<u8>) {
        let _ = self.tx.send(SmokeEvent::LinkData {
            link_id: link_id.0,
            context,
            data,
        });
    }
}

struct TransportCallbacks;

impl Callbacks for TransportCallbacks {
    fn on_announce(&mut self, _announced: AnnouncedIdentity) {}

    fn on_path_updated(&mut self, _dest_hash: DestHash, _hops: u8) {}

    fn on_local_delivery(
        &mut self,
        _dest_hash: DestHash,
        _raw: Vec<u8>,
        _packet_hash: rns_net::PacketHash,
    ) {
    }
}

fn find_free_port() -> u16 {
    static NEXT_PORT: AtomicU16 = AtomicU16::new(0);
    let pid = std::process::id() as u16;
    let base = 24_000 + (pid % 200) * 150;
    let _ = NEXT_PORT.compare_exchange(0, base, Ordering::SeqCst, Ordering::SeqCst);

    loop {
        let port = NEXT_PORT.fetch_add(1, Ordering::SeqCst);
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

fn start_transport_node(port: u16) -> RnsNode {
    let node = RnsNode::start(
        NodeConfig {
            panic_on_interface_error: true,
            transport_enabled: true,
            identity: Some(Identity::new(&mut OsRng)),
            interfaces: vec![InterfaceConfig {
                name: String::new(),
                type_name: "TCPServerInterface".to_string(),
                config_data: Box::new(TcpServerConfig {
                    name: "LXST smoke transport".into(),
                    listen_ip: "127.0.0.1".into(),
                    listen_port: port,
                    interface_id: InterfaceId(1),
                    max_connections: None,
                    ..TcpServerConfig::default()
                }),
                mode: MODE_FULL,
                ingress_control: rns_core::transport::types::IngressControlConfig::enabled(),
                ifac: None,
                discovery: None,
            }],
            ..NodeConfig::default()
        },
        Box::new(TransportCallbacks),
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => {
                drop(stream);
                break;
            }
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Err(err) => panic!("transport listener on {port} did not come up: {err}"),
        }
    }
    node
}

fn start_client_node(port: u16, identity: &Identity, callbacks: Box<dyn Callbacks>) -> RnsNode {
    RnsNode::start(
        NodeConfig {
            panic_on_interface_error: true,
            transport_enabled: false,
            identity: Some(Identity::from_private_key(
                &identity.get_private_key().unwrap(),
            )),
            interfaces: vec![InterfaceConfig {
                name: String::new(),
                type_name: "TCPClientInterface".to_string(),
                config_data: Box::new(TcpClientConfig {
                    name: "LXST smoke client".into(),
                    target_host: "127.0.0.1".into(),
                    target_port: port,
                    interface_id: InterfaceId(1),
                    ..TcpClientConfig::default()
                }),
                mode: MODE_FULL,
                ingress_control: rns_core::transport::types::IngressControlConfig::enabled(),
                ifac: None,
                discovery: None,
            }],
            ..NodeConfig::default()
        },
        callbacks,
    )
    .unwrap()
}

fn wait_for_smoke_event<F, T>(
    rx: &mpsc::Receiver<SmokeEvent>,
    timeout: Duration,
    mut predicate: F,
) -> Option<T>
where
    F: FnMut(SmokeEvent) -> Option<T>,
{
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            return None;
        }
        match rx.recv_timeout(remaining) {
            Ok(event) => {
                if let Some(result) = predicate(event) {
                    return Some(result);
                }
            }
            Err(_) => return None,
        }
    }
}

fn wait_for_interface_up(rx: &mpsc::Receiver<SmokeEvent>) {
    wait_for_smoke_event(rx, Duration::from_secs(5), |event| match event {
        SmokeEvent::InterfaceUp => Some(()),
        _ => None,
    })
    .expect("interface did not come up");
}

fn announce_with_retry(
    node: &RnsNode,
    endpoint: &TelephonyEndpoint,
    identity: &Identity,
    remote_rx: &mpsc::Receiver<SmokeEvent>,
) -> AnnouncedIdentity {
    for _ in 0..6 {
        endpoint.announce(node, identity).unwrap();
        if let Some(announced) =
            wait_for_smoke_event(remote_rx, Duration::from_secs(2), |event| match event {
                SmokeEvent::Announce(announced)
                    if announced.dest_hash == endpoint.destination.hash =>
                {
                    Some(announced)
                }
                _ => None,
            })
        {
            return announced;
        }
    }
    panic!("remote node never received LXST telephony announce");
}

fn wait_for_link_established(rx: &mpsc::Receiver<SmokeEvent>, is_initiator: bool) -> [u8; 16] {
    wait_for_smoke_event(rx, Duration::from_secs(10), |event| match event {
        SmokeEvent::LinkEstablished {
            link_id,
            is_initiator: event_is_initiator,
        } if event_is_initiator == is_initiator => Some(link_id),
        _ => None,
    })
    .expect("link did not establish")
}

fn wait_for_remote_identified(
    rx: &mpsc::Receiver<SmokeEvent>,
    expected_identity: [u8; 16],
) -> [u8; 16] {
    wait_for_smoke_event(rx, Duration::from_secs(10), |event| match event {
        SmokeEvent::RemoteIdentified {
            link_id,
            identity_hash,
        } if identity_hash.0 == expected_identity => Some(link_id),
        _ => None,
    })
    .expect("remote identity was not reported")
}

fn wait_for_link_data_signal(
    rx: &mpsc::Receiver<SmokeEvent>,
    expected_link: [u8; 16],
    expected_signal: Signal,
) {
    wait_for_smoke_event(rx, Duration::from_secs(10), |event| match event {
        SmokeEvent::LinkData {
            link_id,
            context,
            data,
        } if link_id == expected_link && context == rns_core::constants::CONTEXT_NONE => {
            let packet = LxstPacket::decode(&data).ok()?;
            packet.signals.contains(&expected_signal).then_some(())
        }
        _ => None,
    })
    .expect("expected LXST signalling packet was not delivered");
}

fn wait_for_link_closed(rx: &mpsc::Receiver<SmokeEvent>, expected_link: [u8; 16]) {
    wait_for_smoke_event(rx, Duration::from_secs(10), |event| match event {
        SmokeEvent::LinkClosed { link_id } if link_id == expected_link => Some(()),
        _ => None,
    })
    .expect("link close was not reported");
}

#[test]
fn reticulum_loopback_completes_telephony_call_flow() {
    let port = find_free_port();
    let transport = start_transport_node(port);

    let alice_identity = Identity::new(&mut OsRng);
    let alice_endpoint = TelephonyEndpoint::new(&alice_identity);
    let (alice_tx, alice_rx) = mpsc::channel();
    let alice_node = start_client_node(
        port,
        &alice_identity,
        Box::new(SmokeCallbacks::new(alice_tx)),
    );
    alice_endpoint
        .register(&alice_node, &alice_identity)
        .unwrap();

    let bob_identity = Identity::new(&mut OsRng);
    let bob_endpoint = TelephonyEndpoint::new(&bob_identity);
    let (bob_tx, bob_rx) = mpsc::channel();
    let bob_node = start_client_node(port, &bob_identity, Box::new(SmokeCallbacks::new(bob_tx)));
    bob_endpoint.register(&bob_node, &bob_identity).unwrap();

    wait_for_interface_up(&alice_rx);
    wait_for_interface_up(&bob_rx);
    std::thread::sleep(Duration::from_millis(500));

    let announced_bob = announce_with_retry(&bob_node, &bob_endpoint, &bob_identity, &alice_rx);
    assert_eq!(announced_bob.dest_hash, bob_endpoint.destination.hash);
    assert_eq!(announced_bob.identity_hash.0, *bob_identity.hash());

    assert!(request_path_until(
        &alice_node,
        bob_endpoint.destination.hash,
        Duration::from_secs(3),
        Duration::from_millis(50),
    )
    .unwrap());

    let recalled = recall_telephony_identity(&alice_node, *bob_identity.hash())
        .unwrap()
        .expect("Bob identity should be recalled after announce");
    assert_eq!(recalled.dest_hash, bob_endpoint.destination.hash);
    assert_eq!(recalled.identity_hash.0, *bob_identity.hash());

    let alice_link = create_telephony_link(&alice_node, &recalled).unwrap();
    assert_eq!(wait_for_link_established(&alice_rx, true), alice_link);
    let bob_link = wait_for_link_established(&bob_rx, false);
    assert_eq!(bob_link, alice_link);

    alice_node
        .identify_on_link(alice_link, alice_identity.get_private_key().unwrap())
        .unwrap();
    assert_eq!(
        wait_for_remote_identified(&bob_rx, *alice_identity.hash()),
        alice_link
    );

    let signal = Signal::Code(SignalCode::Established);
    alice_node
        .send_on_link(
            alice_link,
            LxstPacket::signalling(signal).encode().unwrap(),
            rns_core::constants::CONTEXT_NONE,
        )
        .unwrap();
    wait_for_link_data_signal(&bob_rx, alice_link, signal);

    alice_node.teardown_link(alice_link).unwrap();
    wait_for_link_closed(&bob_rx, alice_link);

    alice_node.shutdown();
    bob_node.shutdown();
    transport.shutdown();
}
