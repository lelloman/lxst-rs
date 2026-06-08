use std::collections::HashSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
    pub auto_answer_after: Option<Duration>,
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
            auto_answer_after: None,
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
    IncomingCall {
        identity_hash: [u8; 16],
    },
    CallEstablished {
        identity_hash: [u8; 16],
    },
    CallEnded {
        identity_hash: Option<[u8; 16]>,
    },
    Busy {
        identity_hash: Option<[u8; 16]>,
    },
    Rejected {
        identity_hash: Option<[u8; 16]>,
    },
    TimedOut {
        identity_hash: Option<[u8; 16]>,
        state: CallState,
    },
    LowLatencyOutputChanged(bool),
    ProfileChanged(CallProfile),
    SignalReceived(Signal),
}

#[derive(Debug)]
pub struct Telephone {
    config: TelephoneConfig,
    state: CallState,
    active_identity: Option<[u8; 16]>,
    active_profile: CallProfile,
    external_busy: bool,
    low_latency_output: bool,
    call_deadline: Option<Instant>,
    auto_answer_deadline: Option<Instant>,
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
                external_busy: false,
                low_latency_output: false,
                call_deadline: None,
                auto_answer_deadline: None,
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

    pub fn low_latency_output(&self) -> bool {
        self.low_latency_output
    }

    pub fn set_low_latency_output(&mut self, enabled: bool) {
        if self.low_latency_output != enabled {
            self.low_latency_output = enabled;
            let _ = self
                .events
                .send(CallEvent::LowLatencyOutputChanged(enabled));
        }
    }

    pub fn select_call_profile(&mut self, profile: Option<CallProfile>) -> CallProfile {
        let profile = profile.unwrap_or(self.config.profile);
        if self.active_profile != profile {
            self.active_profile = profile;
            let _ = self.events.send(CallEvent::ProfileChanged(profile));
        }
        profile
    }

    pub fn is_busy(&self) -> bool {
        self.external_busy || self.state != CallState::Available
    }

    pub fn set_busy(&mut self, busy: bool) {
        self.external_busy = busy;
    }

    pub fn can_accept_call(&self, identity_hash: &[u8; 16]) -> bool {
        !self.is_busy()
            && !self.config.blocked_callers.contains(identity_hash)
            && self.config.allowed_callers.allows(identity_hash)
    }

    pub fn begin_outgoing_call(&mut self, identity_hash: [u8; 16]) -> bool {
        self.begin_outgoing_call_with_profile(identity_hash, None)
    }

    pub fn begin_outgoing_call_with_profile(
        &mut self,
        identity_hash: [u8; 16],
        profile: Option<CallProfile>,
    ) -> bool {
        if self.is_busy() {
            return false;
        }
        self.select_call_profile(profile);
        self.active_identity = Some(identity_hash);
        self.call_deadline = Some(Instant::now() + self.config.wait_time);
        self.auto_answer_deadline = None;
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
        self.call_deadline = Some(Instant::now() + self.config.ring_time);
        self.auto_answer_deadline = self
            .config
            .auto_answer_after
            .map(|delay| Instant::now() + delay);
        self.set_state(CallState::Ringing);
        let _ = self.events.send(CallEvent::IncomingCall { identity_hash });
        true
    }

    pub fn answer(&mut self) -> bool {
        if self.state != CallState::Ringing {
            return false;
        }
        self.call_deadline = Some(Instant::now() + self.config.connect_time);
        self.auto_answer_deadline = None;
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
        self.call_deadline = None;
        self.auto_answer_deadline = None;
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
        self.call_deadline = None;
        self.auto_answer_deadline = None;
        self.set_state(CallState::Available);
        let _ = self.events.send(CallEvent::CallEnded { identity_hash });
    }

    pub fn reject(&mut self) {
        let identity_hash = self.active_identity.take();
        self.call_deadline = None;
        self.auto_answer_deadline = None;
        self.set_state(CallState::Available);
        let _ = self.events.send(CallEvent::Rejected { identity_hash });
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        if self.state == CallState::Ringing
            && self
                .auto_answer_deadline
                .is_some_and(|deadline| now >= deadline)
        {
            let _ = self.answer();
            return;
        }

        if self.state != CallState::Established
            && self.call_deadline.is_some_and(|deadline| now >= deadline)
        {
            let identity_hash = self.active_identity;
            let state = self.state;
            let _ = self.events.send(CallEvent::TimedOut {
                identity_hash,
                state,
            });
            self.hangup();
        }
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
            Signal::Code(SignalCode::Ringing) => {
                self.call_deadline = Some(Instant::now() + self.config.wait_time);
                self.set_state(CallState::Ringing);
            }
            Signal::Code(SignalCode::Connecting) => {
                self.call_deadline = Some(Instant::now() + self.config.connect_time);
                self.set_state(CallState::Connecting);
            }
            Signal::Code(SignalCode::Established) => {
                let _ = self.establish();
            }
            Signal::Code(SignalCode::Calling) => {}
            Signal::PreferredProfile(profile) => {
                self.select_call_profile(Some(profile));
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
