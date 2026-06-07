use std::collections::HashSet;
use std::sync::mpsc;
use std::time::Duration;

use lxst_core::{CallProfile, Signal, SignalCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelephoneConfig {
    pub ring_time: Duration,
    pub wait_time: Duration,
    pub connect_time: Duration,
    pub announce_interval: Duration,
    pub profile: CallProfile,
    pub allowed_callers: CallerPolicy,
    pub blocked_callers: HashSet<[u8; 16]>,
    pub receive_gain_db: i16,
    pub transmit_gain_db: i16,
    pub use_agc: bool,
}

impl Default for TelephoneConfig {
    fn default() -> Self {
        Self {
            ring_time: Duration::from_secs(60),
            wait_time: Duration::from_secs(70),
            connect_time: Duration::from_secs(5),
            announce_interval: Duration::from_secs(60 * 60 * 3),
            profile: CallProfile::DEFAULT,
            allowed_callers: CallerPolicy::All,
            blocked_callers: HashSet::new(),
            receive_gain_db: 0,
            transmit_gain_db: 0,
            use_agc: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerPolicy {
    All,
    None,
    List(HashSet<[u8; 16]>),
}

impl CallerPolicy {
    pub fn allows(&self, identity_hash: &[u8; 16]) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::List(allowed) => allowed.contains(identity_hash),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CallState {
    #[default]
    Available,
    Calling,
    Ringing,
    Connecting,
    Established,
    Terminating,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallEvent {
    StateChanged(CallState),
    IncomingCall { identity_hash: [u8; 16] },
    CallEstablished { identity_hash: [u8; 16] },
    CallEnded { identity_hash: Option<[u8; 16]> },
    Busy { identity_hash: Option<[u8; 16]> },
    Rejected { identity_hash: Option<[u8; 16]> },
    ProfileChanged(CallProfile),
    SignalReceived(Signal),
}

#[derive(Debug)]
pub struct Telephone {
    config: TelephoneConfig,
    state: CallState,
    active_identity: Option<[u8; 16]>,
    active_profile: CallProfile,
    events: mpsc::Sender<CallEvent>,
}

impl Telephone {
    pub fn new(config: TelephoneConfig) -> (Self, mpsc::Receiver<CallEvent>) {
        let (events, rx) = mpsc::channel();
        let active_profile = config.profile;
        (
            Self {
                config,
                state: CallState::Available,
                active_identity: None,
                active_profile,
                events,
            },
            rx,
        )
    }

    pub fn config(&self) -> &TelephoneConfig {
        &self.config
    }

    pub fn state(&self) -> CallState {
        self.state
    }

    pub fn active_profile(&self) -> CallProfile {
        self.active_profile
    }

    pub fn is_busy(&self) -> bool {
        self.state != CallState::Available
    }

    pub fn can_accept_call(&self, identity_hash: &[u8; 16]) -> bool {
        !self.is_busy()
            && !self.config.blocked_callers.contains(identity_hash)
            && self.config.allowed_callers.allows(identity_hash)
    }

    pub fn begin_outgoing_call(&mut self, identity_hash: [u8; 16]) -> bool {
        if self.is_busy() {
            return false;
        }
        self.active_identity = Some(identity_hash);
        self.set_state(CallState::Calling);
        true
    }

    pub fn begin_incoming_call(&mut self, identity_hash: [u8; 16]) -> bool {
        if !self.can_accept_call(&identity_hash) {
            let _ = self.events.send(CallEvent::Busy {
                identity_hash: Some(identity_hash),
            });
            return false;
        }
        self.active_identity = Some(identity_hash);
        self.set_state(CallState::Ringing);
        let _ = self.events.send(CallEvent::IncomingCall { identity_hash });
        true
    }

    pub fn answer(&mut self) -> bool {
        if self.state != CallState::Ringing {
            return false;
        }
        self.set_state(CallState::Connecting);
        true
    }

    pub fn establish(&mut self) -> bool {
        if !matches!(
            self.state,
            CallState::Connecting | CallState::Calling | CallState::Ringing
        ) {
            return false;
        }
        self.set_state(CallState::Established);
        if let Some(identity_hash) = self.active_identity {
            let _ = self
                .events
                .send(CallEvent::CallEstablished { identity_hash });
        }
        true
    }

    pub fn hangup(&mut self) {
        let identity_hash = self.active_identity.take();
        self.set_state(CallState::Available);
        let _ = self.events.send(CallEvent::CallEnded { identity_hash });
    }

    pub fn reject(&mut self) {
        let identity_hash = self.active_identity.take();
        self.set_state(CallState::Available);
        let _ = self.events.send(CallEvent::Rejected { identity_hash });
    }

    pub fn apply_signal(&mut self, signal: Signal) {
        let _ = self.events.send(CallEvent::SignalReceived(signal));
        match signal {
            Signal::Code(SignalCode::Busy) => {
                let identity_hash = self.active_identity.take();
                self.set_state(CallState::Available);
                let _ = self.events.send(CallEvent::Busy { identity_hash });
            }
            Signal::Code(SignalCode::Rejected) => self.reject(),
            Signal::Code(SignalCode::Available) => self.set_state(CallState::Connecting),
            Signal::Code(SignalCode::Ringing) => self.set_state(CallState::Ringing),
            Signal::Code(SignalCode::Connecting) => self.set_state(CallState::Connecting),
            Signal::Code(SignalCode::Established) => {
                let _ = self.establish();
            }
            Signal::Code(SignalCode::Calling) => {}
            Signal::PreferredProfile(profile) => {
                self.active_profile = profile;
                let _ = self.events.send(CallEvent::ProfileChanged(profile));
            }
            Signal::Unknown(_) => {}
        }
    }

    fn set_state(&mut self, state: CallState) {
        if self.state != state {
            self.state = state;
            let _ = self.events.send(CallEvent::StateChanged(state));
        }
    }
}
