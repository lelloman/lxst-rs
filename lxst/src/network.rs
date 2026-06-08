use std::collections::VecDeque;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use lxst_core::{CodecKind, EncodedFrame, LxstPacket, PacketError, Signal};
use rns_core::constants::CONTEXT_NONE;
use rns_core::types::{DestHash, IdentityHash, LinkId, PacketHash};
use rns_crypto::identity::Identity;
use rns_net::{AnnouncedIdentity, Callbacks, Destination, RnsNode, SendError, TeardownReason};

use crate::audio::AudioFrame;
use crate::codec::{AudioCodec, CodecFactory, CodecSelection, NullCodec};
use crate::pipeline::{AudioSink, AudioSource, EncodedAudioFrame, PipelineError};

pub const APP_NAME: &str = "lxst";
pub const TELEPHONY_PRIMITIVE: &str = "telephony";

#[derive(Debug, Clone)]
pub enum TelephonyNetworkEvent {
    Announce(AnnouncedIdentity),
    PathUpdated {
        dest_hash: DestHash,
        hops: u8,
    },
    LocalDelivery {
        dest_hash: DestHash,
        raw: Vec<u8>,
        packet_hash: PacketHash,
    },
    LinkEstablished {
        link_id: LinkId,
        dest_hash: DestHash,
        rtt: f64,
        is_initiator: bool,
    },
    LinkClosed {
        link_id: LinkId,
        reason: Option<TeardownReason>,
    },
    RemoteIdentified {
        link_id: LinkId,
        identity_hash: IdentityHash,
        public_key: [u8; 64],
    },
    LinkData {
        link_id: LinkId,
        context: u8,
        data: Vec<u8>,
    },
}

pub struct TelephonyCallbacks {
    events: mpsc::Sender<TelephonyNetworkEvent>,
}

impl TelephonyCallbacks {
    pub fn new(events: mpsc::Sender<TelephonyNetworkEvent>) -> Self {
        Self { events }
    }
}

impl Callbacks for TelephonyCallbacks {
    fn on_announce(&mut self, announced: AnnouncedIdentity) {
        let _ = self.events.send(TelephonyNetworkEvent::Announce(announced));
    }

    fn on_path_updated(&mut self, dest_hash: DestHash, hops: u8) {
        let _ = self
            .events
            .send(TelephonyNetworkEvent::PathUpdated { dest_hash, hops });
    }

    fn on_local_delivery(&mut self, dest_hash: DestHash, raw: Vec<u8>, packet_hash: PacketHash) {
        let _ = self.events.send(TelephonyNetworkEvent::LocalDelivery {
            dest_hash,
            raw,
            packet_hash,
        });
    }

    fn on_link_established(
        &mut self,
        link_id: LinkId,
        dest_hash: DestHash,
        rtt: f64,
        is_initiator: bool,
    ) {
        let _ = self.events.send(TelephonyNetworkEvent::LinkEstablished {
            link_id,
            dest_hash,
            rtt,
            is_initiator,
        });
    }

    fn on_link_closed(&mut self, link_id: LinkId, reason: Option<TeardownReason>) {
        let _ = self
            .events
            .send(TelephonyNetworkEvent::LinkClosed { link_id, reason });
    }

    fn on_remote_identified(
        &mut self,
        link_id: LinkId,
        identity_hash: IdentityHash,
        public_key: [u8; 64],
    ) {
        let _ = self.events.send(TelephonyNetworkEvent::RemoteIdentified {
            link_id,
            identity_hash,
            public_key,
        });
    }

    fn on_link_data(&mut self, link_id: LinkId, context: u8, data: Vec<u8>) {
        let _ = self.events.send(TelephonyNetworkEvent::LinkData {
            link_id,
            context,
            data,
        });
    }
}

pub fn telephony_callback_channel() -> (Box<dyn Callbacks>, mpsc::Receiver<TelephonyNetworkEvent>) {
    let (tx, rx) = mpsc::channel();
    (Box::new(TelephonyCallbacks::new(tx)), rx)
}

#[derive(Debug, Clone)]
pub struct TelephonyEndpoint {
    pub destination: Destination,
}

impl TelephonyEndpoint {
    pub fn new(identity: &Identity) -> Self {
        let destination = Destination::single_in(
            APP_NAME,
            &[TELEPHONY_PRIMITIVE],
            rns_core::types::IdentityHash(*identity.hash()),
        );
        Self { destination }
    }

    pub fn register(&self, node: &RnsNode, identity: &Identity) -> Result<(), NetworkError> {
        node.register_destination_with_proof(&self.destination, identity.get_private_key())?;
        let private_key = identity
            .get_private_key()
            .ok_or(NetworkError::MissingIdentityPrivateKey)?;
        let public_key = identity
            .get_public_key()
            .ok_or(NetworkError::MissingIdentityPublicKey)?;
        let sig_prv = private_key[32..64].try_into().unwrap();
        let sig_pub = public_key[32..64].try_into().unwrap();
        node.register_link_destination(self.destination.hash.0, sig_prv, sig_pub, 0)?;
        Ok(())
    }

    pub fn announce(&self, node: &RnsNode, identity: &Identity) -> Result<(), NetworkError> {
        node.announce(&self.destination, identity, None)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct LxstLinkSender {
    node: Arc<RnsNode>,
    link_id: [u8; 16],
}

impl LxstLinkSender {
    pub fn new(node: Arc<RnsNode>, link_id: [u8; 16]) -> Self {
        Self { node, link_id }
    }

    pub fn link_id(&self) -> [u8; 16] {
        self.link_id
    }

    pub fn send_packet(&self, packet: &LxstPacket) -> Result<(), NetworkError> {
        let data = packet.encode()?;
        self.node.send_on_link(self.link_id, data, CONTEXT_NONE)?;
        Ok(())
    }

    pub fn teardown(&self) -> Result<(), NetworkError> {
        self.node.teardown_link(self.link_id)?;
        Ok(())
    }
}

pub trait PacketSender {
    fn send_packet(&mut self, packet: &LxstPacket) -> Result<(), NetworkError>;
}

impl PacketSender for LxstLinkSender {
    fn send_packet(&mut self, packet: &LxstPacket) -> Result<(), NetworkError> {
        LxstLinkSender::send_packet(self, packet)
    }
}

#[derive(Debug)]
pub struct Packetizer<S> {
    sender: S,
    transmit_failure: bool,
}

impl<S> Packetizer<S>
where
    S: PacketSender,
{
    pub fn new(sender: S) -> Self {
        Self {
            sender,
            transmit_failure: false,
        }
    }

    pub fn transmit_failure(&self) -> bool {
        self.transmit_failure
    }

    pub fn sender(&self) -> &S {
        &self.sender
    }

    pub fn sender_mut(&mut self) -> &mut S {
        &mut self.sender
    }

    pub fn into_sender(self) -> S {
        self.sender
    }
}

impl<S> AudioSink for Packetizer<S>
where
    S: PacketSender + Send,
{
    fn can_receive(&self) -> bool {
        !self.transmit_failure
    }

    fn handle_frame(&mut self, frame: EncodedAudioFrame) -> Result<(), PipelineError> {
        let packet = LxstPacket::frame(EncodedFrame::new(frame.codec, frame.payload));
        if let Err(error) = self.sender.send_packet(&packet) {
            self.transmit_failure = true;
            return Err(PipelineError::Network(error));
        }
        Ok(())
    }
}

pub struct LinkSource {
    codec: Box<dyn AudioCodec>,
    queue: VecDeque<AudioFrame>,
    signals: VecDeque<Signal>,
    max_frames: usize,
    samplerate: u32,
    channels: u8,
    running: bool,
}

impl LinkSource {
    pub fn new(codec: Box<dyn AudioCodec>, samplerate: u32, channels: u8) -> Self {
        Self {
            codec,
            queue: VecDeque::new(),
            signals: VecDeque::new(),
            max_frames: 128,
            samplerate,
            channels,
            running: false,
        }
    }

    pub fn with_null_codec(samplerate: u32, channels: u8) -> Self {
        Self::new(Box::new(NullCodec), samplerate, channels)
    }

    pub fn set_codec(&mut self, codec: Box<dyn AudioCodec>) {
        self.codec = codec;
    }

    pub fn queued_frames(&self) -> usize {
        self.queue.len()
    }

    pub fn queued_signals(&self) -> usize {
        self.signals.len()
    }

    pub fn pop_signal(&mut self) -> Option<Signal> {
        self.signals.pop_front()
    }

    pub fn set_max_frames(&mut self, max_frames: usize) {
        self.max_frames = max_frames.max(1);
        while self.queue.len() > self.max_frames {
            self.queue.pop_front();
        }
    }

    pub fn handle_packet_bytes(&mut self, data: &[u8]) -> Result<(), NetworkError> {
        let packet = LxstPacket::decode(data)?;
        self.handle_packet(packet)
    }

    pub fn handle_packet(&mut self, packet: LxstPacket) -> Result<(), NetworkError> {
        for signal in packet.signals {
            self.signals.push_back(signal);
        }
        for frame in packet.frames {
            if self.codec.kind() != frame.codec {
                self.codec = default_codec_for_kind(frame.codec);
            }
            let decoded = self.codec.decode(&frame.payload, self.samplerate)?;
            self.samplerate = decoded.samplerate();
            self.channels = decoded.channels();
            if self.queue.len() >= self.max_frames {
                self.queue.pop_front();
            }
            self.queue.push_back(decoded);
        }
        Ok(())
    }
}

impl AudioSource for LinkSource {
    fn start(&mut self) {
        self.running = true;
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn samplerate(&self) -> u32 {
        self.samplerate
    }

    fn channels(&self) -> u8 {
        self.channels
    }

    fn next_frame(&mut self) -> Result<Option<AudioFrame>, PipelineError> {
        if self.running {
            Ok(self.queue.pop_front())
        } else {
            Ok(None)
        }
    }
}

fn default_codec_for_kind(kind: CodecKind) -> Box<dyn AudioCodec> {
    match kind {
        CodecKind::Null => Box::new(NullCodec),
        CodecKind::Raw => {
            CodecFactory::create(CodecSelection::Raw(lxst_core::RawBitDepth::Float32))
        }
        CodecKind::Opus => CodecFactory::create(CodecSelection::Profile(
            lxst_core::CodecProfile::OpusVoiceMedium,
        )),
        CodecKind::Codec2 => CodecFactory::create(CodecSelection::Profile(
            lxst_core::CodecProfile::Codec2_3200,
        )),
        _ => Box::new(NullCodec),
    }
}

pub fn telephony_dest_hash(identity_hash: [u8; 16]) -> DestHash {
    Destination::single_in(
        APP_NAME,
        &[TELEPHONY_PRIMITIVE],
        rns_core::types::IdentityHash(identity_hash),
    )
    .hash
}

pub fn request_path_until(
    node: &RnsNode,
    dest_hash: DestHash,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<bool, NetworkError> {
    if node.has_path(&dest_hash)? {
        return Ok(true);
    }
    node.request_path(&dest_hash)?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if node.has_path(&dest_hash)? {
            return Ok(true);
        }
        thread::sleep(poll_interval);
    }
    Ok(node.has_path(&dest_hash)?)
}

pub fn recall_telephony_identity(
    node: &RnsNode,
    identity_hash: [u8; 16],
) -> Result<Option<AnnouncedIdentity>, NetworkError> {
    let dest_hash = telephony_dest_hash(identity_hash);
    Ok(node.recall_identity(&dest_hash)?)
}

pub fn create_telephony_link(
    node: &RnsNode,
    announced: &AnnouncedIdentity,
) -> Result<[u8; 16], NetworkError> {
    let sig_pub = announced.public_key[32..64].try_into().unwrap();
    Ok(node.create_link(announced.dest_hash.0, sig_pub)?)
}

#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error(transparent)]
    Packet(#[from] PacketError),
    #[error(transparent)]
    Codec(#[from] crate::codec::CodecError),
    #[error("RNS send failed")]
    Send,
    #[error("identity has no private key")]
    MissingIdentityPrivateKey,
    #[error("identity has no public key")]
    MissingIdentityPublicKey,
}

impl From<SendError> for NetworkError {
    fn from(_: SendError) -> Self {
        Self::Send
    }
}
