use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use lxst_core::{EncodedFrame, LxstPacket, PacketError};
use rns_core::constants::CONTEXT_NONE;
use rns_core::types::DestHash;
use rns_crypto::identity::Identity;
use rns_net::{AnnouncedIdentity, Destination, RnsNode, SendError};

use crate::pipeline::{AudioSink, EncodedAudioFrame, PipelineError};

pub const APP_NAME: &str = "lxst";
pub const TELEPHONY_PRIMITIVE: &str = "telephony";

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
