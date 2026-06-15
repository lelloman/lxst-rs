use std::collections::HashSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use lxst_core::{CallProfile, Signal, SignalCode};

use crate::audio::{list_audio_devices, AudioDeviceInfo, AudioDeviceKind, AudioError};

pub const MIN_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(60 * 5);

#[derive(Debug, Clone, PartialEq)]
pub struct TelephoneConfig {
    pub ring_time: Duration,
    pub wait_time: Duration,
    pub connect_time: Duration,
    pub announce_interval: Duration,
    pub profile: CallProfile,
    pub allowed_callers: CallerPolicy,
    pub blocked_callers: HashSet<[u8; 16]>,
    pub receive_gain_db: f32,
    pub transmit_gain_db: f32,
    pub use_agc: bool,
    pub auto_answer_after: Option<Duration>,
    pub busy_tone_duration: Duration,
    pub transmit_start_skip: Duration,
    pub transmit_start_ease_in: Duration,
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
            receive_gain_db: 0.0,
            transmit_gain_db: 0.0,
            use_agc: true,
            auto_answer_after: None,
            busy_tone_duration: Duration::from_millis(4_250),
            transmit_start_skip: Duration::from_millis(75),
            transmit_start_ease_in: Duration::from_millis(225),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, PartialEq)]
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
    ReceiveGainChanged(f32),
    TransmitGainChanged(f32),
    ReceiveMutedChanged(bool),
    TransmitMutedChanged(bool),
    AgcChanged(bool),
    ProfileChanged(CallProfile),
    SignalReceived(Signal),
}

#[derive(Debug)]
pub struct Telephone {
    config: TelephoneConfig,
    state: CallState,
    active_identity: Option<[u8; 16]>,
    active_direction: Option<CallDirection>,
    active_profile: CallProfile,
    external_busy: bool,
    low_latency_output: bool,
    receive_muted: bool,
    transmit_muted: bool,
    call_deadline: Option<Instant>,
    auto_answer_deadline: Option<Instant>,
    events: mpsc::Sender<CallEvent>,
}

impl Telephone {
    pub fn available_outputs() -> Result<Vec<AudioDeviceInfo>, AudioError> {
        Ok(list_audio_devices()?
            .into_iter()
            .filter(|device| device.kind == AudioDeviceKind::Output)
            .collect())
    }

    pub fn available_inputs() -> Result<Vec<AudioDeviceInfo>, AudioError> {
        Ok(list_audio_devices()?
            .into_iter()
            .filter(|device| device.kind == AudioDeviceKind::Input)
            .collect())
    }

    pub fn default_output() -> Result<Option<AudioDeviceInfo>, AudioError> {
        Ok(Self::available_outputs()?
            .into_iter()
            .find(|device| device.is_default))
    }

    pub fn default_input() -> Result<Option<AudioDeviceInfo>, AudioError> {
        Ok(Self::available_inputs()?
            .into_iter()
            .find(|device| device.is_default))
    }

    pub fn new(config: TelephoneConfig) -> (Self, mpsc::Receiver<CallEvent>) {
        let (events, rx) = mpsc::channel();
        let active_profile = config.profile;
        (
            Self {
                config,
                state: CallState::Available,
                active_identity: None,
                active_direction: None,
                active_profile,
                external_busy: false,
                low_latency_output: false,
                receive_muted: false,
                transmit_muted: false,
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

    pub fn active_call_is_outgoing(&self) -> bool {
        self.active_direction == Some(CallDirection::Outgoing)
    }

    pub fn active_call_is_incoming(&self) -> bool {
        self.active_direction == Some(CallDirection::Incoming)
    }

    pub fn low_latency_output(&self) -> bool {
        self.low_latency_output
    }

    pub fn receive_gain_db(&self) -> f32 {
        self.config.receive_gain_db
    }

    pub fn set_receive_gain_db(&mut self, gain_db: f32) {
        if self.config.receive_gain_db != gain_db {
            self.config.receive_gain_db = gain_db;
            let _ = self.events.send(CallEvent::ReceiveGainChanged(gain_db));
        }
    }

    pub fn transmit_gain_db(&self) -> f32 {
        self.config.transmit_gain_db
    }

    pub fn set_transmit_gain_db(&mut self, gain_db: f32) {
        if self.config.transmit_gain_db != gain_db {
            self.config.transmit_gain_db = gain_db;
            let _ = self.events.send(CallEvent::TransmitGainChanged(gain_db));
        }
    }

    pub fn use_agc(&self) -> bool {
        self.config.use_agc
    }

    pub fn enable_agc(&mut self, enable: bool) {
        if self.config.use_agc != enable {
            self.config.use_agc = enable;
            let _ = self.events.send(CallEvent::AgcChanged(enable));
        }
    }

    pub fn disable_agc(&mut self, disable: bool) {
        self.enable_agc(!disable);
    }

    pub fn receive_muted(&self) -> bool {
        self.receive_muted
    }

    pub fn mute_receive(&mut self, mute: bool) {
        if self.receive_muted != mute {
            self.receive_muted = mute;
            let _ = self.events.send(CallEvent::ReceiveMutedChanged(mute));
        }
    }

    pub fn unmute_receive(&mut self, unmute: bool) {
        self.mute_receive(!unmute);
    }

    pub fn transmit_muted(&self) -> bool {
        self.transmit_muted
    }

    pub fn mute_transmit(&mut self, mute: bool) {
        if self.transmit_muted != mute {
            self.transmit_muted = mute;
            let _ = self.events.send(CallEvent::TransmitMutedChanged(mute));
        }
    }

    pub fn unmute_transmit(&mut self, unmute: bool) {
        self.mute_transmit(!unmute);
    }

    pub fn set_connect_timeout(&mut self, timeout: Duration) {
        self.config.connect_time = timeout;
    }

    pub fn set_announce_interval(&mut self, announce_interval: Duration) {
        self.config.announce_interval = announce_interval.max(MIN_ANNOUNCE_INTERVAL);
    }

    pub fn busy_tone_duration(&self) -> Duration {
        self.config.busy_tone_duration
    }

    pub fn set_busy_tone_duration(&mut self, duration: Duration) {
        self.config.busy_tone_duration = duration;
    }

    pub fn transmit_start_skip(&self) -> Duration {
        self.config.transmit_start_skip
    }

    pub fn set_transmit_start_skip(&mut self, duration: Duration) {
        self.config.transmit_start_skip = duration;
    }

    pub fn transmit_start_ease_in(&self) -> Duration {
        self.config.transmit_start_ease_in
    }

    pub fn set_transmit_start_ease_in(&mut self, duration: Duration) {
        self.config.transmit_start_ease_in = duration;
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
        self.active_direction = Some(CallDirection::Outgoing);
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
        self.active_direction = Some(CallDirection::Incoming);
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
        if !matches!(self.state, CallState::Connecting | CallState::Calling) {
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
        if !self.has_active_call() {
            return;
        }
        if self.active_direction == Some(CallDirection::Incoming)
            && self.state == CallState::Ringing
        {
            self.reject();
            return;
        }
        self.end_call();
    }

    pub fn reject(&mut self) {
        if !self.has_active_call() {
            return;
        }
        let identity_hash = self.clear_active_call();
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
            self.end_call();
        }
    }

    pub fn apply_signal(&mut self, signal: Signal) {
        let _ = self.events.send(CallEvent::SignalReceived(signal));
        if self.should_ignore_status_signal(signal) {
            return;
        }
        match signal {
            Signal::Code(SignalCode::Busy) => {
                let identity_hash = self.clear_active_call();
                self.set_state(CallState::Available);
                let _ = self.events.send(CallEvent::Busy { identity_hash });
            }
            Signal::Code(SignalCode::Rejected) => self.reject(),
            Signal::Code(SignalCode::Available) => {}
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
                if self.has_active_call() {
                    self.select_call_profile(Some(profile));
                }
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

    fn should_ignore_status_signal(&self, signal: Signal) -> bool {
        if !matches!(signal, Signal::Code(_)) {
            return false;
        }
        !self.has_active_call()
            || (self.active_direction == Some(CallDirection::Incoming)
                && self.state == CallState::Ringing)
    }

    fn has_active_call(&self) -> bool {
        self.active_identity.is_some() || self.state != CallState::Available
    }

    fn end_call(&mut self) {
        let identity_hash = self.clear_active_call();
        self.set_state(CallState::Available);
        let _ = self.events.send(CallEvent::CallEnded { identity_hash });
    }

    fn clear_active_call(&mut self) -> Option<[u8; 16]> {
        self.call_deadline = None;
        self.auto_answer_deadline = None;
        self.active_direction = None;
        self.active_profile = self.config.profile;
        self.clear_mutes();
        self.active_identity.take()
    }

    fn clear_mutes(&mut self) {
        if self.receive_muted {
            self.receive_muted = false;
            let _ = self.events.send(CallEvent::ReceiveMutedChanged(false));
        }
        if self.transmit_muted {
            self.transmit_muted = false;
            let _ = self.events.send(CallEvent::TransmitMutedChanged(false));
        }
    }
}
