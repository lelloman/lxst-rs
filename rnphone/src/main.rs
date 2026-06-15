use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use lxst::network::{
    create_telephony_link, recall_telephony_identity, request_path_until, telephony_dest_hash,
    LinkSource, LxstLinkSender, Packetizer, TelephonyEndpoint,
};
#[cfg(not(feature = "gpio-rpi"))]
use lxst::MatrixKeypadBackend;
#[cfg(feature = "gpio-rpi")]
use lxst::RpiMatrixKeypadBackend;
use lxst::{
    Agc, AudioCodec, AudioSink, AudioSource, BandPass, CallProfile, CallState, CodecFactory,
    CodecSelection, CpalInputConfig, CpalInputSource, CpalOutputConfig, CpalOutputSink,
    EncodedAudioFrame, Key, KeyTransition, KeypadEvent, Lcd1602Buffer, LxstPacket, MatrixKeypad,
    MatrixKeypadPoller, MatrixKeypadScanner, OpusFileSource, Signal, SignalCode, SourcePlayer,
    Telephone, TelephoneConfig, TelephonyNetworkEvent,
};
use rns_crypto::identity::Identity;
use rns_crypto::OsRng;
use rns_net::storage::{load_identity, save_identity};
use rns_net::RnsNode;

const VERSION: &str = env!("CARGO_PKG_VERSION");
#[cfg_attr(not(test), allow(dead_code))]
const HW_SLEEP_TIMEOUT: Duration = Duration::from_secs(15);

fn main() {
    if let Err(err) = run(env::args().skip(1).collect()) {
        eprintln!("rnphone: {err}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let args = Args::parse(args)?;
    if args.help {
        print_help();
        return Ok(());
    }
    if args.version {
        println!("rnphone {VERSION}");
        return Ok(());
    }
    if args.systemd {
        println!("{}", systemd_unit());
        return Ok(());
    }
    if args.list_devices {
        print_audio_devices();
        return Ok(());
    }

    let config_dir = args.config.unwrap_or_else(default_config_dir);
    let mut app = App::load(config_dir, args.rnsconfig, args.service)?;
    app.start()
}

fn print_audio_devices() {
    println!("Available audio devices:");
    match lxst::list_audio_devices() {
        Ok(devices) if devices.is_empty() => println!("  (no audio devices found)"),
        Ok(devices) => {
            for device in devices {
                let kind = match device.kind {
                    lxst::AudioDeviceKind::Input => "input",
                    lxst::AudioDeviceKind::Output => "output",
                };
                let default = if device.is_default { " default" } else { "" };
                if let Some(config) = device.default_config {
                    println!(
                        "  [{kind}{default}] {} - {} ch, {} Hz, {}",
                        device.name, config.channels, config.min_sample_rate, config.sample_format
                    );
                } else {
                    println!("  [{kind}{default}] {}", device.name);
                }
            }
        }
        Err(err) => println!("  audio device enumeration failed: {err}"),
    }
}

#[derive(Debug, Default)]
struct Args {
    list_devices: bool,
    config: Option<PathBuf>,
    rnsconfig: Option<PathBuf>,
    service: bool,
    systemd: bool,
    version: bool,
    verbose: u8,
    help: bool,
}

impl Args {
    fn parse(args: Vec<String>) -> Result<Self, String> {
        let mut parsed = Args::default();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-l" | "--list-devices" => parsed.list_devices = true,
                "--config" => {
                    i += 1;
                    parsed.config = Some(PathBuf::from(
                        args.get(i).ok_or("--config requires a path")?,
                    ));
                }
                "--rnsconfig" => {
                    i += 1;
                    parsed.rnsconfig = Some(PathBuf::from(
                        args.get(i).ok_or("--rnsconfig requires a path")?,
                    ));
                }
                "-s" | "--service" => parsed.service = true,
                "--systemd" => parsed.systemd = true,
                "--version" => parsed.version = true,
                "-v" | "--verbose" => parsed.verbose = parsed.verbose.saturating_add(1),
                "-h" | "--help" => parsed.help = true,
                other if other.starts_with('-') && other.chars().all(|c| c == '-' || c == 'v') => {
                    parsed.verbose = parsed
                        .verbose
                        .saturating_add(other.chars().filter(|c| *c == 'v').count() as u8);
                }
                other => return Err(format!("unknown argument {other}")),
            }
            i += 1;
        }
        Ok(parsed)
    }
}

#[cfg_attr(test, allow(dead_code))]
struct App {
    rnsconfig: Option<PathBuf>,
    config: RnphoneConfig,
    identity: Identity,
    telephone: Telephone,
    service: bool,
    node: Option<Arc<RnsNode>>,
    network_events: Option<mpsc::Receiver<TelephonyNetworkEvent>>,
    active_link: Option<[u8; 16]>,
    active_audio: Option<CallAudio>,
    active_ringer: Option<RingerAudio>,
    last_dialed: Option<[u8; 16]>,
    hardware_ui: HardwareUi,
    hardware_last_event: Instant,
    hardware_events: Option<mpsc::Receiver<KeypadEvent>>,
    hardware_poller: Option<MatrixKeypadPoller>,
    #[cfg(test)]
    test_sent_signals: Vec<([u8; 16], Signal)>,
    #[cfg(test)]
    test_started_audio: Vec<[u8; 16]>,
    #[cfg(test)]
    test_call_audio_running: bool,
    #[cfg(test)]
    test_ringer_running: bool,
    #[cfg(test)]
    test_ringer_starts: usize,
    #[cfg(test)]
    test_ringer_stops: usize,
    #[cfg(test)]
    test_call_audio_stops: usize,
}

impl App {
    fn load(
        config_dir: PathBuf,
        rnsconfig: Option<PathBuf>,
        service: bool,
    ) -> Result<Self, String> {
        fs::create_dir_all(config_dir.join("storage")).map_err(|e| e.to_string())?;
        install_default_sound_assets(&config_dir)?;
        let config_path = config_dir.join("config");
        if !config_path.exists() {
            fs::write(&config_path, DEFAULT_CONFIG).map_err(|e| e.to_string())?;
        }
        let mut config =
            RnphoneConfig::parse(&fs::read_to_string(&config_path).map_err(|e| e.to_string())?)?;
        let identity_path = config_dir.join("identity");
        let identity = if identity_path.exists() {
            load_identity(&identity_path).map_err(|e| e.to_string())?
        } else {
            let identity = Identity::new(&mut OsRng);
            save_identity(&identity, &identity_path).map_err(|e| e.to_string())?;
            identity
        };
        config.resolve_paths(&config_dir);
        config.finalize_for_identity(identity.hash());
        let hardware_ui = HardwareUi::from_config(&config.hardware)?;
        let hardware_last_event = Instant::now();
        let (hardware_events, hardware_poller) = start_hardware_keypad(&config.hardware)?;

        let telephone_config = TelephoneConfig {
            allowed_callers: config.allowed_callers.clone(),
            blocked_callers: config.blocked_callers.clone(),
            ..TelephoneConfig::default()
        };
        let (telephone, _events) = Telephone::new(telephone_config);

        Ok(Self {
            rnsconfig,
            config,
            identity,
            telephone,
            service,
            node: None,
            network_events: None,
            active_link: None,
            active_audio: None,
            active_ringer: None,
            last_dialed: None,
            hardware_ui,
            hardware_last_event,
            hardware_events,
            hardware_poller,
            #[cfg(test)]
            test_sent_signals: Vec::new(),
            #[cfg(test)]
            test_started_audio: Vec::new(),
            #[cfg(test)]
            test_call_audio_running: false,
            #[cfg(test)]
            test_ringer_running: false,
            #[cfg(test)]
            test_ringer_starts: 0,
            #[cfg(test)]
            test_ringer_stops: 0,
            #[cfg(test)]
            test_call_audio_stops: 0,
        })
    }

    fn start(&mut self) -> Result<(), String> {
        let endpoint = TelephonyEndpoint::new(&self.identity);

        if self.service {
            self.ensure_network(&endpoint)?;
            self.announce(&endpoint)?;
            println!("Reticulum Telephone Service is ready");
            println!("Identity hash: {}", pretty_hash(self.identity.hash()));
            self.became_available();
            loop {
                self.poll_network_events();
                self.poll_hardware_events(&endpoint, Instant::now())?;
                self.poll_hardware_idle(Instant::now());
                self.telephone.tick();
                thread::sleep(Duration::from_millis(100));
            }
        }

        println!("\nReticulum Telephone Utility is ready");
        println!("  Identity hash: {}\n", pretty_hash(self.identity.hash()));
        println!("Enter identity hash and hit enter to call (or ? for help)");
        self.became_available();
        let stdin = io::stdin();
        loop {
            print!("> ");
            io::stdout().flush().map_err(|e| e.to_string())?;
            let mut line = String::new();
            if stdin.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                break;
            }
            let line = line.trim();
            if line.is_empty() {
                if self.telephone.state() != CallState::Available {
                    self.hangup_current();
                }
                continue;
            }
            self.poll_network_events();
            self.poll_hardware_events(&endpoint, Instant::now())?;
            self.poll_hardware_idle(Instant::now());
            match line {
                "?" | "h" | "help" => print_help_menu(),
                "q" | "quit" | "exit" => break,
                "i" | "identity" => print!("{}", identity_status(self.identity.hash())),
                "d" | "desthash" => {
                    print!("{}", destination_status(&endpoint.destination.hash.0))
                }
                "p" | "phonebook" => self.print_phonebook(),
                "a" | "announce" | "anounce" => self.announce(&endpoint)?,
                "r" | "redial" => match self.last_dialed {
                    Some(hash) => self.dial_hash(&endpoint, hash)?,
                    None => println!("No last call to redial"),
                },
                "answer" => {
                    if self.answer_current() {
                        println!("Call answered");
                    } else {
                        println!("No incoming call to answer");
                    }
                }
                "hangup" => self.hangup_current(),
                value if value.len() == 32 && value.chars().all(|c| c.is_ascii_hexdigit()) => {
                    let mut hash = [0u8; 16];
                    decode_hex_into(value, &mut hash)?;
                    self.dial_hash(&endpoint, hash)?;
                }
                other => match self.config.resolve_dial_target(other) {
                    Some(target) => {
                        println!("Calling {}", target.label);
                        self.dial_hash(&endpoint, target.identity_hash)?;
                    }
                    None => println!("Unknown command: {other}"),
                },
            }
            self.telephone.tick();
        }
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn handle_hardware_keypad_event(
        &mut self,
        endpoint: &TelephonyEndpoint,
        event: KeypadEvent,
    ) -> Result<(), String> {
        self.handle_hardware_keypad_event_at(endpoint, event, Instant::now())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn handle_hardware_keypad_event_at(
        &mut self,
        endpoint: &TelephonyEndpoint,
        event: KeypadEvent,
        now: Instant,
    ) -> Result<(), String> {
        self.hardware_last_event = now;
        let action =
            self.hardware_ui
                .handle_keypad_event(event, self.telephone.state(), &self.config);
        self.apply_hardware_action(endpoint, action)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn apply_hardware_action(
        &mut self,
        endpoint: &TelephonyEndpoint,
        action: HardwareAction,
    ) -> Result<(), String> {
        match action {
            HardwareAction::None => Ok(()),
            HardwareAction::Answer => {
                if self.answer_current() {
                    println!("Call answered");
                } else {
                    println!("No incoming call to answer");
                }
                Ok(())
            }
            HardwareAction::Reject | HardwareAction::Hangup => {
                self.hangup_current();
                Ok(())
            }
            HardwareAction::Dial(identity_hash) => self.dial_hash(endpoint, identity_hash),
        }
    }

    fn ensure_network(&mut self, endpoint: &TelephonyEndpoint) -> Result<(), String> {
        if self.node.is_some() {
            return Ok(());
        }
        let (callbacks, events) = lxst::telephony_callback_channel();
        let node = RnsNode::from_config(self.rnsconfig.as_deref(), callbacks)
            .map_err(|e| e.to_string())?;
        endpoint
            .register(&node, &self.identity)
            .map_err(|e| e.to_string())?;
        self.network_events = Some(events);
        self.node = Some(Arc::new(node));
        Ok(())
    }

    fn announce(&mut self, endpoint: &TelephonyEndpoint) -> Result<(), String> {
        self.ensure_network(endpoint)?;
        endpoint
            .announce(
                self.node.as_ref().expect("node is initialized"),
                &self.identity,
            )
            .map_err(|e| e.to_string())?;
        println!("Announce sent");
        Ok(())
    }

    fn dial_hash(
        &mut self,
        endpoint: &TelephonyEndpoint,
        identity_hash: [u8; 16],
    ) -> Result<(), String> {
        if self.telephone.is_busy() {
            println!("Telephone is busy");
            return Ok(());
        }

        self.ensure_network(endpoint)?;
        let node = self.node.as_ref().expect("node is initialized");
        let dest_hash = telephony_dest_hash(identity_hash);
        println!("Requesting path to {}", hex(&dest_hash.0));
        if !request_path_until(
            node,
            dest_hash,
            Duration::from_secs(10),
            Duration::from_millis(250),
        )
        .map_err(|e| e.to_string())?
        {
            return Err(format!("no path to {}", hex(&dest_hash.0)));
        }

        let announced = recall_telephony_identity(node, identity_hash)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no recalled identity for {}", hex(&identity_hash)))?;
        let link_id = create_telephony_link(node, &announced).map_err(|e| e.to_string())?;
        if self.telephone.begin_outgoing_call(identity_hash) {
            self.active_link = Some(link_id);
            self.last_dialed = Some(identity_hash);
            println!(
                "Calling {} on link {}...",
                pretty_hash(&identity_hash),
                pretty_hash(&link_id)
            );
        } else {
            let _ = node.teardown_link(link_id);
            println!("Telephone is busy");
        }
        Ok(())
    }

    fn answer_current(&mut self) -> bool {
        if !self.telephone.answer() {
            return false;
        }

        self.send_signal(Signal::Code(SignalCode::Connecting));
        let _ = self.telephone.establish();
        self.send_signal(Signal::Code(SignalCode::Established));
        if let Some(link_id) = self.active_link {
            self.start_call_audio(link_id);
        }
        true
    }

    fn became_available(&mut self) {
        self.became_available_at(Instant::now());
    }

    fn became_available_at(&mut self, now: Instant) {
        self.hardware_last_event = now;
        self.hardware_ui.became_available();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn poll_hardware_idle(&mut self, now: Instant) -> bool {
        let idle_for = now.saturating_duration_since(self.hardware_last_event);
        self.hardware_ui
            .sleep_if_idle(idle_for, self.telephone.state(), self.telephone.is_busy())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn poll_hardware_events(
        &mut self,
        endpoint: &TelephonyEndpoint,
        now: Instant,
    ) -> Result<usize, String> {
        if self.hardware_poller.is_none() && self.hardware_events.is_none() {
            return Ok(0);
        }

        let Some(events) = self.hardware_events.take() else {
            return Ok(0);
        };
        let mut pending = Vec::new();
        while let Ok(event) = events.try_recv() {
            pending.push(event);
        }
        self.hardware_events = Some(events);

        let handled = pending.len();
        for event in pending {
            self.handle_hardware_keypad_event_at(endpoint, event, now)?;
        }
        Ok(handled)
    }

    fn hangup_current(&mut self) {
        if self.telephone.state() == CallState::Ringing {
            self.send_signal(Signal::Code(SignalCode::Rejected));
        }
        self.stop_ringer();
        self.stop_call_audio();
        if let (Some(node), Some(link_id)) = (self.node.as_ref(), self.active_link.take()) {
            let _ = node.teardown_link(link_id);
        }
        self.telephone.hangup();
        self.became_available();
        println!("Call ended");
    }

    fn poll_network_events(&mut self) {
        let Some(events) = self.network_events.take() else {
            return;
        };
        while let Ok(event) = events.try_recv() {
            self.handle_network_event(event);
        }
        self.network_events = Some(events);
    }

    fn handle_network_event(&mut self, event: TelephonyNetworkEvent) {
        match event {
            TelephonyNetworkEvent::Announce(announced) => {
                println!("Heard {}", announced.identity_hash);
            }
            TelephonyNetworkEvent::PathUpdated { dest_hash, hops } => {
                println!("Path to {dest_hash} is {hops} hop(s)");
            }
            TelephonyNetworkEvent::LocalDelivery { .. } => {}
            TelephonyNetworkEvent::LinkEstablished {
                link_id,
                dest_hash,
                is_initiator,
                ..
            } => {
                self.active_link = Some(link_id.0);
                if is_initiator {
                    let _ = self.telephone.establish();
                    self.send_signal(Signal::Code(SignalCode::Established));
                    self.start_call_audio(link_id.0);
                    println!("Link {link_id} established to {dest_hash}");
                } else {
                    println!("Incoming link {link_id} from {dest_hash}");
                }
            }
            TelephonyNetworkEvent::LinkClosed { link_id, .. } => {
                if self.active_link == Some(link_id.0) {
                    self.active_link = None;
                    self.stop_ringer();
                    self.stop_call_audio();
                    if self.telephone.state() != CallState::Available {
                        self.telephone.hangup();
                    }
                    self.became_available();
                    println!("Link {link_id} closed");
                }
            }
            TelephonyNetworkEvent::RemoteIdentified {
                identity_hash,
                link_id,
                ..
            } => {
                if self.telephone.state() == CallState::Available {
                    self.active_link = Some(link_id.0);
                    if self.telephone.begin_incoming_call(identity_hash.0) {
                        self.send_signal(Signal::Code(SignalCode::Ringing));
                        self.start_ringer();
                        println!("Incoming call from {identity_hash}");
                    } else {
                        self.send_signal(Signal::Code(SignalCode::Busy));
                        self.active_link = None;
                        println!("Rejected incoming call from {identity_hash}");
                    }
                }
            }
            TelephonyNetworkEvent::LinkData { data, .. } => {
                if let Some(audio) = &self.active_audio {
                    let _ = audio.handle_link_data(&data);
                }
                if let Ok(packet) = LxstPacket::decode(&data) {
                    for signal in packet.signals {
                        self.telephone.apply_signal(signal);
                        match signal {
                            Signal::Code(SignalCode::Established) => {
                                if self.telephone.state() == CallState::Established {
                                    if let Some(link_id) = self.active_link {
                                        self.start_call_audio(link_id);
                                    }
                                }
                            }
                            Signal::Code(SignalCode::Busy | SignalCode::Rejected) => {
                                if self.telephone.state() == CallState::Available {
                                    self.stop_ringer();
                                    self.stop_call_audio();
                                    self.active_link = None;
                                    self.became_available();
                                }
                            }
                            _ => {
                                if self.telephone.state() != CallState::Ringing {
                                    self.stop_ringer();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(test)]
    fn start_ringer(&mut self) {
        if !self.test_ringer_running {
            self.test_ringer_running = true;
            self.test_ringer_starts += 1;
        }
    }

    #[cfg(not(test))]
    fn start_ringer(&mut self) {
        if self.active_ringer.is_some() {
            return;
        }
        let Some(path) = self.config.ringtone_path.clone() else {
            return;
        };
        if !path.is_file() {
            return;
        }
        match RingerAudio::start(path, self.config.audio_devices.ringer.clone()) {
            Ok(ringer) => self.active_ringer = Some(ringer),
            Err(err) => println!("Ringer unavailable: {err}"),
        }
    }

    #[cfg(test)]
    fn stop_ringer(&mut self) {
        if self.test_ringer_running {
            self.test_ringer_running = false;
            self.test_ringer_stops += 1;
        }
    }

    #[cfg(not(test))]
    fn stop_ringer(&mut self) {
        if let Some(mut ringer) = self.active_ringer.take() {
            ringer.stop();
        }
    }

    #[cfg(test)]
    fn start_call_audio(&mut self, link_id: [u8; 16]) {
        self.stop_ringer();
        if !self.test_call_audio_running {
            self.test_call_audio_running = true;
            self.test_started_audio.push(link_id);
        }
    }

    #[cfg(not(test))]
    fn start_call_audio(&mut self, link_id: [u8; 16]) {
        self.stop_ringer();
        if self.active_audio.is_some() {
            return;
        }
        let Some(node) = self.node.clone() else {
            return;
        };
        match CallAudio::start(
            node,
            link_id,
            self.telephone.active_profile(),
            self.config.audio_devices.clone(),
        ) {
            Ok(audio) => {
                self.active_audio = Some(audio);
                println!("Audio transport started");
            }
            Err(err) => println!("Audio transport unavailable: {err}"),
        }
    }

    #[cfg(test)]
    fn stop_call_audio(&mut self) {
        if self.test_call_audio_running {
            self.test_call_audio_running = false;
            self.test_call_audio_stops += 1;
        }
    }

    #[cfg(not(test))]
    fn stop_call_audio(&mut self) {
        if let Some(mut audio) = self.active_audio.take() {
            audio.stop();
        }
    }

    #[cfg(test)]
    fn send_signal(&mut self, signal: Signal) {
        let Some(link_id) = self.active_link else {
            return;
        };
        self.test_sent_signals.push((link_id, signal));
    }

    #[cfg(not(test))]
    fn send_signal(&mut self, signal: Signal) {
        let (Some(node), Some(link_id)) = (self.node.clone(), self.active_link) else {
            return;
        };
        let sender = LxstLinkSender::new(node, link_id);
        let _ = sender.send_signal(signal);
    }

    fn print_phonebook(&self) {
        print!("{}", phonebook_menu(&self.config));
    }
}

#[cfg_attr(test, allow(dead_code))]
struct RingerAudio {
    stop_tx: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

#[cfg_attr(test, allow(dead_code))]
impl RingerAudio {
    fn start(path: PathBuf, preferred_device: Option<String>) -> Result<Self, String> {
        let source = OpusFileSource::open(&path, 60, true).map_err(|e| e.to_string())?;
        let sink = CpalOutputSink::new(CpalOutputConfig {
            preferred_device,
            ..CpalOutputConfig::default()
        })
        .map_err(|e| e.to_string())?;
        let mut player = SourcePlayer::new(source, sink);
        player.start().map_err(|e| e.to_string())?;

        let (stop_tx, stop_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            while stop_rx.try_recv().is_err() {
                if let Err(err) = player.process_next() {
                    eprintln!("rnphone ringer: {err}");
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let _ = player.stop();
        });

        Ok(Self {
            stop_tx,
            worker: Some(worker),
        })
    }

    fn stop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct CallAudio {
    link_source: Arc<Mutex<LinkSource>>,
    stop_tx: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

#[cfg_attr(test, allow(dead_code))]
impl CallAudio {
    fn start(
        node: Arc<RnsNode>,
        link_id: [u8; 16],
        profile: CallProfile,
        devices: AudioDeviceConfig,
    ) -> Result<Self, String> {
        let codec_profile = profile.codec_profile();
        let frame_ms = profile.frame_duration().as_millis();

        let mut input = CpalInputSource::new(CpalInputConfig {
            preferred_device: devices.microphone.clone(),
            target_frame_ms: frame_ms,
            codec_profile: Some(codec_profile),
            skip: Duration::from_millis(75),
            ease_in: Duration::from_millis(225),
            ..CpalInputConfig::default()
        })
        .map_err(|e| e.to_string())?;
        input.add_filter(BandPass::new(250.0, 8_500.0).map_err(|e| e.to_string())?);
        input.add_filter(Agc::new(-15.0, 12.0));

        let mut output = CpalOutputSink::new(CpalOutputConfig {
            preferred_device: devices.speaker.clone(),
            ..CpalOutputConfig::default()
        })
        .map_err(|e| e.to_string())?;
        output.start().map_err(|e| e.to_string())?;

        let transmit_codec = CodecFactory::create(CodecSelection::Profile(codec_profile));
        let receive_codec = CodecFactory::create(CodecSelection::Profile(codec_profile));
        let link_source = Arc::new(Mutex::new(LinkSource::new(receive_codec, 8_000, 1)));
        link_source.lock().map_err(|e| e.to_string())?.start();

        let sender = LxstLinkSender::new(node, link_id);
        let packetizer = Packetizer::new(sender);
        let (stop_tx, stop_rx) = mpsc::channel();
        let worker_source = Arc::clone(&link_source);
        let worker = thread::spawn(move || {
            if let Err(err) = run_call_audio_loop(
                input,
                transmit_codec,
                packetizer,
                worker_source,
                output,
                stop_rx,
            ) {
                eprintln!("rnphone audio: {err}");
            }
        });

        Ok(Self {
            link_source,
            stop_tx,
            worker: Some(worker),
        })
    }

    fn handle_link_data(&self, data: &[u8]) -> Result<(), String> {
        self.link_source
            .lock()
            .map_err(|e| e.to_string())?
            .handle_packet_bytes(data)
            .map_err(|e| e.to_string())
    }

    fn stop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for CallAudio {
    fn drop(&mut self) {
        self.stop();
    }
}

fn transmit_audio_frame<S, C, K>(
    source: &mut S,
    codec: &mut C,
    sink: &mut K,
) -> Result<bool, String>
where
    S: AudioSource + ?Sized,
    C: AudioCodec + ?Sized,
    K: AudioSink + ?Sized,
{
    if !sink.can_receive() {
        return Ok(false);
    }

    let Some(frame) = source.next_frame().map_err(|e| e.to_string())? else {
        return Ok(false);
    };

    let encoded = EncodedAudioFrame {
        codec: codec.kind(),
        samplerate: frame.samplerate(),
        channels: frame.channels(),
        payload: codec.encode(&frame).map_err(|e| e.to_string())?,
    };
    sink.handle_frame(encoded).map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg_attr(test, allow(dead_code))]
fn run_call_audio_loop(
    mut input: CpalInputSource,
    mut transmit_codec: Box<dyn AudioCodec>,
    mut packetizer: Packetizer<LxstLinkSender>,
    link_source: Arc<Mutex<LinkSource>>,
    mut output: CpalOutputSink,
    stop_rx: mpsc::Receiver<()>,
) -> Result<(), String> {
    input.start();
    while stop_rx.try_recv().is_err() {
        transmit_audio_frame(&mut input, transmit_codec.as_mut(), &mut packetizer)?;

        {
            let mut source = link_source.lock().map_err(|e| e.to_string())?;
            while let Some(frame) = source.next_frame().map_err(|e| e.to_string())? {
                if output.can_receive() {
                    output.handle_frame(frame).map_err(|e| e.to_string())?;
                } else {
                    break;
                }
            }
        }

        thread::sleep(Duration::from_millis(5));
    }
    input.stop();
    let _ = output.stop();
    Ok(())
}

#[derive(Debug, Clone)]
struct RnphoneConfig {
    phonebook: HashMap<String, PhonebookEntry>,
    phonebook_order: Vec<String>,
    allowed_callers: lxst::CallerPolicy,
    allow_phonebook_callers: bool,
    blocked_callers: HashSet<[u8; 16]>,
    audio_devices: AudioDeviceConfig,
    hardware: HardwareConfig,
    ringtone_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AudioDeviceConfig {
    speaker: Option<String>,
    microphone: Option<String>,
    ringer: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct HardwareConfig {
    keypad: Option<String>,
    display: Option<String>,
    keypad_hook_pin: Option<u8>,
    amp_mute_pin: Option<u8>,
    amp_mute_level: Option<PinLevel>,
}

impl HardwareConfig {
    fn validate(&self) -> Result<(), String> {
        self.keypad_matrix()?;
        self.display_enabled()?;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn keypad_matrix(&self) -> Result<Option<MatrixKeypad>, String> {
        let Some(driver) = self.keypad.as_deref() else {
            return Ok(None);
        };

        let mut keypad = match driver {
            "gpio_4x4" => MatrixKeypad::gpio_4x4(),
            "gpio_5x5" => MatrixKeypad::gpio_5x5(),
            _ => return Err(format!("unknown keypad driver {driver}")),
        };
        if self.keypad_hook_pin.is_some() {
            keypad = keypad.with_hook();
        }
        Ok(Some(keypad))
    }

    #[cfg(feature = "gpio-rpi")]
    fn keypad_pins(&self) -> Result<Option<(&'static [u8], &'static [u8])>, String> {
        let Some(driver) = self.keypad.as_deref() else {
            return Ok(None);
        };

        match driver {
            "gpio_4x4" => Ok(Some((
                &MatrixKeypad::GPIO_4X4_ROW_PINS,
                &MatrixKeypad::GPIO_4X4_COL_PINS,
            ))),
            "gpio_5x5" => Ok(Some((
                &MatrixKeypad::GPIO_5X5_ROW_PINS,
                &MatrixKeypad::GPIO_5X5_COL_PINS,
            ))),
            _ => Err(format!("unknown keypad driver {driver}")),
        }
    }

    fn display_enabled(&self) -> Result<bool, String> {
        match self.display.as_deref() {
            None => Ok(false),
            Some("i2c_lcd1602") => Ok(true),
            Some(driver) => Err(format!("unknown display driver {driver}")),
        }
    }
}

fn start_hardware_keypad(
    config: &HardwareConfig,
) -> Result<
    (
        Option<mpsc::Receiver<KeypadEvent>>,
        Option<MatrixKeypadPoller>,
    ),
    String,
> {
    let Some(keypad) = config.keypad_matrix()? else {
        return Ok((None, None));
    };

    let (tx, rx) = mpsc::channel();

    #[cfg(feature = "gpio-rpi")]
    {
        let Some((row_pins, col_pins)) = config.keypad_pins()? else {
            return Ok((None, None));
        };
        let backend = RpiMatrixKeypadBackend::new(row_pins, col_pins, config.keypad_hook_pin)
            .map_err(|e| format!("gpio keypad unavailable: {e}"))?;
        let scanner = MatrixKeypadScanner::new(keypad, backend);
        let poller = MatrixKeypadPoller::start(
            scanner,
            Duration::from_millis(MatrixKeypad::SCAN_INTERVAL_MS),
            move |event| {
                let _ = tx.send(event);
            },
        );
        return Ok((Some(rx), Some(poller)));
    }

    #[cfg(not(feature = "gpio-rpi"))]
    {
        let scanner = MatrixKeypadScanner::new(
            keypad,
            NoopKeypadBackend {
                hook_enabled: config.keypad_hook_pin.is_some(),
            },
        );
        let poller = MatrixKeypadPoller::start(
            scanner,
            Duration::from_millis(MatrixKeypad::SCAN_INTERVAL_MS),
            move |event| {
                let _ = tx.send(event);
            },
        );
        Ok((Some(rx), Some(poller)))
    }
}

#[cfg(not(feature = "gpio-rpi"))]
#[derive(Debug, Clone, Copy)]
struct NoopKeypadBackend {
    hook_enabled: bool,
}

#[cfg(not(feature = "gpio-rpi"))]
impl MatrixKeypadBackend for NoopKeypadBackend {
    fn read_col(&mut self, _row: usize, _col: usize) -> bool {
        false
    }

    fn hook_on(&mut self) -> Option<bool> {
        self.hook_enabled.then_some(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinLevel {
    Low,
    High,
}

impl Default for RnphoneConfig {
    fn default() -> Self {
        Self {
            phonebook: HashMap::new(),
            phonebook_order: Vec::new(),
            allowed_callers: lxst::CallerPolicy::All,
            allow_phonebook_callers: false,
            blocked_callers: HashSet::new(),
            audio_devices: AudioDeviceConfig::default(),
            hardware: HardwareConfig::default(),
            ringtone_path: None,
        }
    }
}

impl RnphoneConfig {
    fn parse(input: &str) -> Result<Self, String> {
        let mut config = RnphoneConfig::default();
        let mut section = String::new();
        for raw_line in input.lines() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_lowercase();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match section.as_str() {
                "telephone" if key == "allowed_callers" => {
                    if value.eq_ignore_ascii_case("phonebook") {
                        config.allow_phonebook_callers = true;
                        config.allowed_callers = lxst::CallerPolicy::List(HashSet::new());
                    } else {
                        config.allowed_callers = parse_allowed_callers(value)?;
                    }
                }
                "telephone" if key == "blocked_callers" => {
                    for item in split_list(value) {
                        config.blocked_callers.insert(parse_hash(item)?);
                    }
                }
                "telephone" if key == "ringtone" => {
                    config.ringtone_path = non_empty_config_value(value).map(PathBuf::from);
                }
                "telephone" if key == "speaker" => {
                    config.audio_devices.speaker = non_empty_config_value(value);
                }
                "telephone" if key == "microphone" => {
                    config.audio_devices.microphone = non_empty_config_value(value);
                }
                "telephone" if key == "ringer" => {
                    config.audio_devices.ringer = non_empty_config_value(value);
                }
                "hardware" if key == "keypad" => {
                    config.hardware.keypad = non_empty_config_value(value);
                }
                "hardware" if key == "display" => {
                    config.hardware.display = non_empty_config_value(value);
                }
                "hardware" if key == "keypad_hook_pin" => {
                    config.hardware.keypad_hook_pin = parse_optional_u8(value)?;
                }
                "hardware" if key == "amp_mute_pin" => {
                    config.hardware.amp_mute_pin = parse_optional_u8(value)?;
                }
                "hardware" if key == "amp_mute_level" => {
                    config.hardware.amp_mute_level = parse_pin_level(value)?;
                }
                "phonebook" => {
                    let parts = split_list(value);
                    if let Some(identity_hash) = parts.first() {
                        let alias = parts
                            .get(1)
                            .map(|s| s.chars().filter(|c| c.is_ascii_digit()).collect::<String>())
                            .filter(|s| !s.is_empty());
                        if !config.phonebook.contains_key(key) {
                            config.phonebook_order.push(key.to_string());
                        }
                        config.phonebook.insert(
                            key.to_string(),
                            PhonebookEntry {
                                identity_hash: parse_hash(identity_hash)?,
                                alias,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        if config.allow_phonebook_callers {
            config.allowed_callers = lxst::CallerPolicy::List(
                config.phonebook.values().map(|e| e.identity_hash).collect(),
            );
        }
        Ok(config)
    }

    fn resolve_paths(&mut self, config_dir: &Path) {
        if let Some(path) = &self.ringtone_path {
            if path.is_relative() {
                self.ringtone_path = Some(config_dir.join(path));
            }
        }
    }

    fn finalize_for_identity(&mut self, own_hash: &[u8; 16]) {
        self.phonebook
            .retain(|_, entry| &entry.identity_hash != own_hash);
        self.phonebook_order
            .retain(|name| self.phonebook.contains_key(name));
        self.blocked_callers.remove(own_hash);
        match &mut self.allowed_callers {
            lxst::CallerPolicy::List(allowed) => {
                allowed.remove(own_hash);
                if self.allow_phonebook_callers {
                    *allowed = self
                        .phonebook
                        .values()
                        .map(|entry| entry.identity_hash)
                        .collect();
                }
            }
            lxst::CallerPolicy::All | lxst::CallerPolicy::None => {}
        }
    }

    fn resolve_dial_target(&self, input: &str) -> Option<DialTarget> {
        self.phonebook_order.iter().find_map(|name| {
            let entry = self.phonebook.get(name)?;
            let alias_matches = entry.alias.as_deref() == Some(input);
            let name_matches = name.eq_ignore_ascii_case(input);
            if alias_matches || name_matches {
                Some(DialTarget {
                    label: match &entry.alias {
                        Some(alias) => format!("{name} ({alias})"),
                        None => name.clone(),
                    },
                    identity_hash: entry.identity_hash,
                })
            } else {
                None
            }
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn resolve_dial_alias_name(&self, input: &str) -> Option<&str> {
        self.phonebook_order.iter().find_map(|name| {
            let entry = self.phonebook.get(name)?;
            (entry.alias.as_deref() == Some(input)).then_some(name.as_str())
        })
    }
}

#[derive(Debug, Clone)]
struct PhonebookEntry {
    identity_hash: [u8; 16],
    alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DialTarget {
    label: String,
    identity_hash: [u8; 16],
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HardwareMode {
    Idle,
    Dial,
    Sleep,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum HardwareAction {
    None,
    Answer,
    Reject,
    Hangup,
    Dial([u8; 16]),
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct HardwareUi {
    mode: HardwareMode,
    input: String,
    display: Option<Lcd1602Buffer>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl HardwareUi {
    fn new(display_enabled: bool) -> Self {
        Self {
            mode: HardwareMode::Idle,
            input: String::new(),
            display: display_enabled.then(Lcd1602Buffer::new),
        }
    }

    fn from_config(config: &HardwareConfig) -> Result<Self, String> {
        config.validate()?;
        Ok(Self::new(config.display_enabled()?))
    }

    fn became_available(&mut self) {
        if let Some(display) = &mut self.display {
            display.clear();
            display.print("Telephone Ready", 0, 0);
            display.print("", 0, 1);
        }
        self.input.clear();
        self.mode = HardwareMode::Idle;
    }

    fn sleep(&mut self) {
        self.mode = HardwareMode::Sleep;
        if let Some(display) = &mut self.display {
            display.sleep();
        }
    }

    fn sleep_if_idle(
        &mut self,
        idle_for: Duration,
        call_state: CallState,
        telephone_busy: bool,
    ) -> bool {
        if call_state == CallState::Available
            && self.mode == HardwareMode::Idle
            && !telephone_busy
            && idle_for >= HW_SLEEP_TIMEOUT
        {
            self.sleep();
            true
        } else {
            false
        }
    }

    fn handle_keypad_event(
        &mut self,
        event: KeypadEvent,
        call_state: CallState,
        config: &RnphoneConfig,
    ) -> HardwareAction {
        if self.mode == HardwareMode::Sleep {
            self.became_available();
        }

        match call_state {
            CallState::Ringing => {
                if is_key_down(event, 'D') || is_hook_up(event) {
                    HardwareAction::Answer
                } else if is_key_down(event, 'C') {
                    HardwareAction::Reject
                } else {
                    HardwareAction::None
                }
            }
            CallState::Calling | CallState::Connecting | CallState::Established => {
                if is_key_down(event, 'D') || is_hook_down(event) {
                    HardwareAction::Hangup
                } else {
                    HardwareAction::None
                }
            }
            CallState::Available if self.mode == HardwareMode::Idle => {
                if is_key_down(event, 'A') {
                    self.input.clear();
                    self.mode = HardwareMode::Dial;
                    self.update_display(config);
                } else if let Some(digit) = down_digit(event) {
                    self.input.push(digit);
                    self.mode = HardwareMode::Dial;
                    self.update_display(config);
                }
                HardwareAction::None
            }
            CallState::Available if self.mode == HardwareMode::Dial => {
                let mut dial_event = false;
                if event.transition == KeyTransition::Down {
                    match event.key {
                        Key::Char(ch) if ch.is_ascii_digit() => self.input.push(ch),
                        Key::Char('A') => self.became_available(),
                        Key::Char('B') => {
                            self.input.pop();
                        }
                        Key::Char('C') => self.input.clear(),
                        Key::Char('D') => dial_event = true,
                        _ => {}
                    }
                }
                if is_hook_up(event) {
                    dial_event = true;
                }

                if dial_event {
                    if let Some(target) = config.resolve_dial_target(&self.input) {
                        self.input.clear();
                        self.mode = HardwareMode::Idle;
                        return HardwareAction::Dial(target.identity_hash);
                    }
                }

                self.update_display(config);
                HardwareAction::None
            }
            _ => HardwareAction::None,
        }
    }

    fn update_display(&mut self, config: &RnphoneConfig) {
        if self.mode != HardwareMode::Dial {
            return;
        }

        let Some(display) = &mut self.display else {
            return;
        };
        let lookup_name = if self.input.is_empty() {
            "Enter number".to_string()
        } else {
            config
                .resolve_dial_alias_name(&self.input)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "Unknown".to_string())
        };

        display.print(&self.input, 0, 0);
        display.print(&lookup_name, 0, 1);
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn down_digit(event: KeypadEvent) -> Option<char> {
    match event {
        KeypadEvent {
            key: Key::Char(ch),
            transition: KeyTransition::Down,
        } if ch.is_ascii_digit() => Some(ch),
        _ => None,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn is_key_down(event: KeypadEvent, key: char) -> bool {
    event.key == Key::Char(key) && event.transition == KeyTransition::Down
}

#[cfg_attr(not(test), allow(dead_code))]
fn is_hook_down(event: KeypadEvent) -> bool {
    event.key == Key::Hook && event.transition == KeyTransition::Down
}

#[cfg_attr(not(test), allow(dead_code))]
fn is_hook_up(event: KeypadEvent) -> bool {
    event.key == Key::Hook && event.transition == KeyTransition::Up
}

fn parse_allowed_callers(value: &str) -> Result<lxst::CallerPolicy, String> {
    match value.to_ascii_lowercase().as_str() {
        "all" => Ok(lxst::CallerPolicy::All),
        "none" => Ok(lxst::CallerPolicy::None),
        _ => {
            let mut allowed = HashSet::new();
            for item in split_list(value) {
                allowed.insert(parse_hash(item)?);
            }
            Ok(lxst::CallerPolicy::List(allowed))
        }
    }
}

fn non_empty_config_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_optional_u8(value: &str) -> Result<Option<u8>, String> {
    let value = value.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        value
            .parse::<u8>()
            .map(Some)
            .map_err(|e| format!("invalid pin value {value}: {e}"))
    }
}

fn parse_pin_level(value: &str) -> Result<Option<PinLevel>, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" => Ok(None),
        "low" | "0" => Ok(Some(PinLevel::Low)),
        "high" | "1" => Ok(Some(PinLevel::High)),
        other => Err(format!("invalid pin level {other}")),
    }
}

fn split_list(value: &str) -> Vec<&str> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect()
}

fn parse_hash(value: &str) -> Result<[u8; 16], String> {
    if value.len() != 32 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("invalid identity hash {value}"));
    }
    let mut bytes = [0u8; 16];
    decode_hex_into(value, &mut bytes)?;
    Ok(bytes)
}

fn decode_hex_into(value: &str, out: &mut [u8]) -> Result<(), String> {
    if value.len() != out.len() * 2 {
        return Err(format!("invalid hex length {}", value.len()));
    }
    for (idx, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[idx * 2..idx * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn pretty_hash(bytes: &[u8]) -> String {
    format!("<{}>", hex(bytes))
}

fn identity_status(identity_hash: &[u8]) -> String {
    format!(
        "Identity hash of this telephone: {}\n",
        pretty_hash(identity_hash)
    )
}

fn destination_status(destination_hash: &[u8]) -> String {
    format!(
        "Destination hash of this telephone: {}\n",
        pretty_hash(destination_hash)
    )
}

fn phonebook_menu(config: &RnphoneConfig) -> String {
    if config.phonebook_order.is_empty() {
        return "\nNo entries in phonebook\n".to_string();
    }

    let max_name_len = config
        .phonebook_order
        .iter()
        .map(|name| name.len())
        .max()
        .unwrap_or(0);
    let max_alias_len = config
        .phonebook_order
        .iter()
        .filter_map(|name| config.phonebook.get(name))
        .filter_map(|entry| entry.alias.as_ref().map(String::len))
        .max()
        .unwrap_or(0);
    let max_index_len = config.phonebook_order.len().to_string().len();
    let alias_width = max_alias_len.max(max_index_len);

    let mut output = String::from("\nPhonebook\n");
    for (index, name) in config.phonebook_order.iter().enumerate() {
        let Some(entry) = config.phonebook.get(name) else {
            continue;
        };
        let alias = entry
            .alias
            .clone()
            .unwrap_or_else(|| (index + 1).to_string());
        output.push_str(&format!(
            "  {alias:>alias_width$} {name:<max_name_len$} : {}\n",
            pretty_hash(&entry.identity_hash)
        ));
    }
    output
}

fn default_config_dir() -> PathBuf {
    if Path::new("/etc/rnphone/config").exists() {
        return PathBuf::from("/etc/rnphone");
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let xdg = home.join(".config/rnphone/config");
    if xdg.exists() {
        home.join(".config/rnphone")
    } else {
        home.join(".rnphone")
    }
}

fn print_help() {
    println!("Reticulum Telephone Utility");
    println!("  -l, --list-devices      list available audio devices");
    println!("      --config PATH       path to rnphone config directory");
    println!("      --rnsconfig PATH    path to Reticulum config directory");
    println!("  -s, --service           run as a service");
    println!("      --systemd           print example systemd unit");
    println!("      --version           print version");
    println!("  -v                      increase verbosity");
}

fn print_help_menu() {
    println!("Available commands");
    println!("  phonebook : Open the phonebook");
    println!("  redial    : Call the last called identity again");
    println!("  identity  : Display the identity hash");
    println!("  desthash  : Display the destination hash");
    println!("  announce  : Send an announce");
    println!("  quit      : Exit the program");
    println!("  help      : This help menu");
}

fn systemd_unit() -> String {
    let user = env::var("USER").unwrap_or_else(|_| "USERNAME".to_string());
    format!(
        "# This systemd unit allows installing rnphone as a system service on Linux-based devices\n[Unit]\nDescription=Reticulum Telephone Service\nAfter=sound.target\n\n[Service]\nExecStartPre=/bin/sleep 30\nType=simple\nEnvironment=\"DISPLAY=:0\"\nEnvironment=\"XAUTHORITY=/home/{user}/.Xauthority\"\nEnvironment=\"XDG_RUNTIME_DIR=/run/user/1000\"\nRestart=always\nRestartSec=5\nUser={user}\nExecStart=/home/{user}/.local/bin/rnphone --service -vvv\n\n[Install]\nWantedBy=graphical.target\n"
    )
}

const DEFAULT_SOUND_ASSETS: &[(&str, &[u8])] = &[
    ("ringer.opus", include_bytes!("../assets/ringer.opus")),
    ("soft.opus", include_bytes!("../assets/soft.opus")),
];

fn install_default_sound_assets(config_dir: &Path) -> Result<(), String> {
    for (filename, bytes) in DEFAULT_SOUND_ASSETS {
        let path = config_dir.join(filename);
        if !path.exists() {
            fs::write(path, bytes).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

const DEFAULT_CONFIG: &str = r#"# This is an example rnphone config file.

[telephone]
    ringtone = ringer.opus
    # speaker = device name
    # microphone = device name
    # ringer = device name
    # allowed_callers = all
    # blocked_callers = f3e8c3359b39d36f3baff0a616a73d3e

[phonebook]
    # Mary = f3e8c3359b39d36f3baff0a616a73d3e
    # Rudy = 5d2d14619dfa0ff06278c17347c14331, 241

[hardware]
    # keypad = gpio_4x4
    # display = i2c_lcd1602
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_phonebook_alias() {
        let config =
            RnphoneConfig::parse("[phonebook]\nMary = f3e8c3359b39d36f3baff0a616a73d3e, 123\n")
                .unwrap();
        assert_eq!(config.phonebook["Mary"].alias.as_deref(), Some("123"));
    }

    #[test]
    fn parses_and_resolves_ringtone_path() {
        let mut config = RnphoneConfig::parse("[telephone]\nringtone = soft.opus\n").unwrap();
        assert_eq!(config.ringtone_path, Some(PathBuf::from("soft.opus")));

        config.resolve_paths(Path::new("/tmp/rnphone-test"));
        assert_eq!(
            config.ringtone_path,
            Some(PathBuf::from("/tmp/rnphone-test/soft.opus"))
        );
    }

    #[test]
    fn preserves_absolute_ringtone_path() {
        let mut config = RnphoneConfig::parse("[telephone]\nringtone = /opt/ring.opus\n").unwrap();
        config.resolve_paths(Path::new("/tmp/rnphone-test"));
        assert_eq!(config.ringtone_path, Some(PathBuf::from("/opt/ring.opus")));
    }

    #[test]
    fn parses_audio_device_names() {
        let config = RnphoneConfig::parse(
            "[telephone]\nspeaker = Living Room Output\nmicrophone = Desk Mic\nringer = Bell Speaker\n",
        )
        .unwrap();

        assert_eq!(
            config.audio_devices.speaker.as_deref(),
            Some("Living Room Output")
        );
        assert_eq!(config.audio_devices.microphone.as_deref(), Some("Desk Mic"));
        assert_eq!(config.audio_devices.ringer.as_deref(), Some("Bell Speaker"));
    }

    #[test]
    fn parses_hardware_config() {
        let config = RnphoneConfig::parse(
            "[hardware]\nkeypad = gpio_4x4\ndisplay = i2c_lcd1602\nkeypad_hook_pin = 5\namp_mute_pin = 25\namp_mute_level = high\n",
        )
        .unwrap();

        assert_eq!(config.hardware.keypad.as_deref(), Some("gpio_4x4"));
        assert_eq!(config.hardware.display.as_deref(), Some("i2c_lcd1602"));
        assert_eq!(config.hardware.keypad_hook_pin, Some(5));
        assert_eq!(config.hardware.amp_mute_pin, Some(25));
        assert_eq!(config.hardware.amp_mute_level, Some(PinLevel::High));
    }

    #[test]
    fn hardware_config_builds_supported_keypad_and_display_drivers() {
        let config = RnphoneConfig::parse(
            "[hardware]\nkeypad = gpio_5x5\ndisplay = i2c_lcd1602\nkeypad_hook_pin = 11\n",
        )
        .unwrap();

        let keypad = config.hardware.keypad_matrix().unwrap().unwrap();
        assert_eq!(keypad.rows(), 5);
        assert_eq!(keypad.cols(), 5);
        assert_eq!(keypad.key_at(4, 4), Some(Key::Char('K')));
        assert!(keypad.is_up(Key::Hook));

        let ui = HardwareUi::from_config(&config.hardware).unwrap();
        assert!(ui.display.is_some());
    }

    #[test]
    fn hardware_config_rejects_unknown_drivers() {
        let config = RnphoneConfig::parse("[hardware]\nkeypad = spi_keypad\n").unwrap();
        let err = config.hardware.validate().unwrap_err();
        assert_eq!(err, "unknown keypad driver spi_keypad");

        let config = RnphoneConfig::parse("[hardware]\ndisplay = oled12864\n").unwrap();
        let err = HardwareUi::from_config(&config.hardware).unwrap_err();
        assert_eq!(err, "unknown display driver oled12864");
    }

    #[test]
    fn rejects_invalid_hardware_pin_level() {
        let err = RnphoneConfig::parse("[hardware]\namp_mute_level = floating\n").unwrap_err();
        assert!(err.contains("invalid pin level"));
    }

    #[test]
    fn hardware_ready_display_matches_upstream() {
        let mut ui = HardwareUi::new(true);

        ui.became_available();

        assert_eq!(ui.mode, HardwareMode::Idle);
        assert_eq!(ui.input, "");
        assert_eq!(display_row(&ui, 0), "Telephone Ready ");
        assert_eq!(display_row(&ui, 1), "                ");
    }

    #[test]
    fn hardware_keypad_digits_enter_dial_mode_and_lookup_aliases() {
        let config = phonebook_config();
        let mut ui = HardwareUi::new(true);
        ui.became_available();

        assert_eq!(
            ui.handle_keypad_event(key_down('1'), CallState::Available, &config),
            HardwareAction::None
        );
        assert_eq!(ui.mode, HardwareMode::Dial);
        assert_eq!(ui.input, "1");
        assert_eq!(display_row(&ui, 0), "1               ");
        assert_eq!(display_row(&ui, 1), "Unknown         ");

        ui.handle_keypad_event(key_down('2'), CallState::Available, &config);

        assert_eq!(ui.input, "12");
        assert_eq!(display_row(&ui, 0), "12              ");
        assert_eq!(display_row(&ui, 1), "Mary            ");
    }

    #[test]
    fn hardware_dial_mode_supports_backspace_clear_and_cancel() {
        let config = RnphoneConfig::default();
        let mut ui = HardwareUi::new(true);
        ui.handle_keypad_event(key_down('1'), CallState::Available, &config);
        ui.handle_keypad_event(key_down('2'), CallState::Available, &config);
        ui.handle_keypad_event(key_down('3'), CallState::Available, &config);

        ui.handle_keypad_event(key_down('B'), CallState::Available, &config);
        assert_eq!(ui.input, "12");
        assert_eq!(display_row(&ui, 0), "12              ");

        ui.handle_keypad_event(key_down('C'), CallState::Available, &config);
        assert_eq!(ui.input, "");
        assert_eq!(display_row(&ui, 1), "Enter number    ");

        ui.handle_keypad_event(key_down('A'), CallState::Available, &config);
        assert_eq!(ui.mode, HardwareMode::Idle);
        assert_eq!(display_row(&ui, 0), "Telephone Ready ");
        assert_eq!(display_row(&ui, 1), "                ");
    }

    #[test]
    fn hardware_dial_key_dials_matching_alias_and_resets_input() {
        let config = phonebook_config();
        let expected_hash = parse_hash("f3e8c3359b39d36f3baff0a616a73d3e").unwrap();
        let mut ui = HardwareUi::new(true);
        ui.handle_keypad_event(key_down('1'), CallState::Available, &config);
        ui.handle_keypad_event(key_down('2'), CallState::Available, &config);

        let action = ui.handle_keypad_event(key_down('D'), CallState::Available, &config);

        assert_eq!(action, HardwareAction::Dial(expected_hash));
        assert_eq!(ui.mode, HardwareMode::Idle);
        assert_eq!(ui.input, "");
    }

    #[test]
    fn hardware_hook_up_dials_matching_alias() {
        let config = phonebook_config();
        let expected_hash = parse_hash("f3e8c3359b39d36f3baff0a616a73d3e").unwrap();
        let mut ui = HardwareUi::new(false);
        ui.handle_keypad_event(key_down('1'), CallState::Available, &config);
        ui.handle_keypad_event(key_down('2'), CallState::Available, &config);

        let action = ui.handle_keypad_event(hook_up(), CallState::Available, &config);

        assert_eq!(action, HardwareAction::Dial(expected_hash));
        assert_eq!(ui.mode, HardwareMode::Idle);
    }

    #[test]
    fn hardware_ringing_keys_answer_or_reject_like_upstream() {
        let config = RnphoneConfig::default();
        let mut ui = HardwareUi::new(false);

        assert_eq!(
            ui.handle_keypad_event(key_down('D'), CallState::Ringing, &config),
            HardwareAction::Answer
        );
        assert_eq!(
            ui.handle_keypad_event(hook_up(), CallState::Ringing, &config),
            HardwareAction::Answer
        );
        assert_eq!(
            ui.handle_keypad_event(key_down('C'), CallState::Ringing, &config),
            HardwareAction::Reject
        );
    }

    #[test]
    fn hardware_active_call_keys_hang_up_like_upstream() {
        let config = RnphoneConfig::default();
        let mut ui = HardwareUi::new(false);

        for state in [
            CallState::Calling,
            CallState::Connecting,
            CallState::Established,
        ] {
            assert_eq!(
                ui.handle_keypad_event(key_down('D'), state, &config),
                HardwareAction::Hangup
            );
            assert_eq!(
                ui.handle_keypad_event(hook_down(), state, &config),
                HardwareAction::Hangup
            );
        }
    }

    #[test]
    fn hardware_sleeping_display_wakes_and_processes_same_key_event() {
        let config = RnphoneConfig::default();
        let mut ui = HardwareUi::new(true);
        ui.became_available();
        ui.sleep();
        assert!(ui.display.as_ref().unwrap().is_sleeping());

        ui.handle_keypad_event(key_down('5'), CallState::Available, &config);

        assert_eq!(ui.mode, HardwareMode::Dial);
        assert_eq!(ui.input, "5");
        assert!(!ui.display.as_ref().unwrap().is_sleeping());
        assert_eq!(display_row(&ui, 0), "5               ");
        assert_eq!(display_row(&ui, 1), "Unknown         ");
    }

    #[test]
    fn hardware_idle_display_sleeps_after_upstream_timeout() {
        let mut ui = HardwareUi::new(true);
        ui.became_available();

        assert!(ui.sleep_if_idle(HW_SLEEP_TIMEOUT, CallState::Available, false));

        assert_eq!(ui.mode, HardwareMode::Sleep);
        assert!(ui.display.as_ref().unwrap().is_sleeping());
        assert_eq!(display_row(&ui, 0), "                ");
        assert_eq!(display_row(&ui, 1), "                ");
    }

    #[test]
    fn hardware_idle_sleep_is_guarded_by_state_mode_busy_and_timeout() {
        let mut ui = HardwareUi::new(true);
        ui.became_available();
        assert!(!ui.sleep_if_idle(
            HW_SLEEP_TIMEOUT - Duration::from_millis(1),
            CallState::Available,
            false
        ));
        assert_eq!(ui.mode, HardwareMode::Idle);

        ui.handle_keypad_event(
            key_down('1'),
            CallState::Available,
            &RnphoneConfig::default(),
        );
        assert!(!ui.sleep_if_idle(HW_SLEEP_TIMEOUT, CallState::Available, false));
        assert_eq!(ui.mode, HardwareMode::Dial);

        ui.became_available();
        assert!(!ui.sleep_if_idle(HW_SLEEP_TIMEOUT, CallState::Ringing, false));
        assert_eq!(ui.mode, HardwareMode::Idle);

        assert!(!ui.sleep_if_idle(HW_SLEEP_TIMEOUT, CallState::Available, true));
        assert_eq!(ui.mode, HardwareMode::Idle);
    }

    #[test]
    fn empty_audio_device_names_are_ignored() {
        let config =
            RnphoneConfig::parse("[telephone]\nspeaker =    \nmicrophone =\nringer =     \n")
                .unwrap();

        assert_eq!(config.audio_devices, AudioDeviceConfig::default());
    }

    #[test]
    fn parses_long_verbose_flag() {
        let args = Args::parse(vec!["--verbose".to_string(), "-vv".to_string()]).unwrap();
        assert_eq!(args.verbose, 3);
    }

    #[test]
    fn parses_allowed_list() {
        let config = RnphoneConfig::parse(
            "[telephone]\nallowed_callers = f3e8c3359b39d36f3baff0a616a73d3e\n",
        )
        .unwrap();
        match config.allowed_callers {
            lxst::CallerPolicy::List(list) => assert_eq!(list.len(), 1),
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn phonebook_policy_allows_only_phonebook_entries() {
        let config = RnphoneConfig::parse(
            "[telephone]\nallowed_callers = phonebook\n\
             [phonebook]\nMary = f3e8c3359b39d36f3baff0a616a73d3e, 123\n",
        )
        .unwrap();
        match config.allowed_callers {
            lxst::CallerPolicy::List(list) => {
                assert!(list.contains(&parse_hash("f3e8c3359b39d36f3baff0a616a73d3e").unwrap()));
                assert!(!list.contains(&parse_hash("5d2d14619dfa0ff06278c17347c14331").unwrap()));
            }
            _ => panic!("expected phonebook caller list"),
        }
    }

    #[test]
    fn finalization_removes_own_identity_from_phonebook_policy() {
        let own = parse_hash("f3e8c3359b39d36f3baff0a616a73d3e").unwrap();
        let mut config = RnphoneConfig::parse(
            "[telephone]\nallowed_callers = phonebook\nblocked_callers = f3e8c3359b39d36f3baff0a616a73d3e\n\
             [phonebook]\nMary = f3e8c3359b39d36f3baff0a616a73d3e, 123\n\
             Rudy = 5d2d14619dfa0ff06278c17347c14331, 241\n",
        )
        .unwrap();
        config.finalize_for_identity(&own);
        assert!(!config.phonebook.contains_key("Mary"));
        assert!(!config.blocked_callers.contains(&own));
        match config.allowed_callers {
            lxst::CallerPolicy::List(list) => {
                assert_eq!(list.len(), 1);
                assert!(list.contains(&parse_hash("5d2d14619dfa0ff06278c17347c14331").unwrap()));
            }
            _ => panic!("expected phonebook caller list"),
        }
    }

    #[test]
    fn resolves_phonebook_name_and_numeric_alias() {
        let config =
            RnphoneConfig::parse("[phonebook]\nMary = f3e8c3359b39d36f3baff0a616a73d3e, A1B2\n")
                .unwrap();
        let by_name = config.resolve_dial_target("mary").unwrap();
        let by_alias = config.resolve_dial_target("12").unwrap();
        assert_eq!(by_name.identity_hash, by_alias.identity_hash);
        assert_eq!(by_alias.label, "Mary (12)");
    }

    #[test]
    fn formats_identity_and_destination_status_like_upstream() {
        let hash = parse_hash("f3e8c3359b39d36f3baff0a616a73d3e").unwrap();

        assert_eq!(
            identity_status(&hash),
            "Identity hash of this telephone: <f3e8c3359b39d36f3baff0a616a73d3e>\n"
        );
        assert_eq!(
            destination_status(&hash),
            "Destination hash of this telephone: <f3e8c3359b39d36f3baff0a616a73d3e>\n"
        );
    }

    #[test]
    fn formats_empty_phonebook_like_upstream() {
        let config = RnphoneConfig::default();

        assert_eq!(phonebook_menu(&config), "\nNo entries in phonebook\n");
    }

    #[test]
    fn formats_phonebook_in_config_order_with_aligned_aliases() {
        let config = RnphoneConfig::parse(
            "[phonebook]\nMary = f3e8c3359b39d36f3baff0a616a73d3e, A1B2\nAlexander = 5d2d14619dfa0ff06278c17347c14331\n",
        )
        .unwrap();

        assert_eq!(
            phonebook_menu(&config),
            "\nPhonebook\n  12 Mary      : <f3e8c3359b39d36f3baff0a616a73d3e>\n   2 Alexander : <5d2d14619dfa0ff06278c17347c14331>\n"
        );
    }

    #[test]
    fn finalization_removes_own_identity_from_phonebook_order() {
        let own = parse_hash("f3e8c3359b39d36f3baff0a616a73d3e").unwrap();
        let mut config = RnphoneConfig::parse(
            "[phonebook]\nMary = f3e8c3359b39d36f3baff0a616a73d3e, 123\nRudy = 5d2d14619dfa0ff06278c17347c14331, 241\n",
        )
        .unwrap();

        config.finalize_for_identity(&own);

        assert_eq!(config.phonebook_order, vec!["Rudy"]);
        assert_eq!(
            phonebook_menu(&config),
            "\nPhonebook\n  241 Rudy : <5d2d14619dfa0ff06278c17347c14331>\n"
        );
    }

    #[test]
    fn installs_default_sound_assets_when_missing() {
        let dir = temp_config_dir("install-assets");
        fs::create_dir_all(&dir).unwrap();

        install_default_sound_assets(&dir).unwrap();

        assert!(fs::read(dir.join("ringer.opus"))
            .unwrap()
            .starts_with(b"OggS"));
        assert!(fs::read(dir.join("soft.opus"))
            .unwrap()
            .starts_with(b"OggS"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keeps_existing_sound_assets() {
        let dir = temp_config_dir("keep-assets");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ringer.opus"), b"custom").unwrap();

        install_default_sound_assets(&dir).unwrap();

        assert_eq!(fs::read(dir.join("ringer.opus")).unwrap(), b"custom");
        assert!(fs::read(dir.join("soft.opus"))
            .unwrap()
            .starts_with(b"OggS"));

        fs::remove_dir_all(dir).unwrap();
    }

    fn test_app_with_config(config: RnphoneConfig) -> App {
        let identity = Identity::from_private_key(&[0x42; 64]);
        let telephone_config = TelephoneConfig {
            allowed_callers: config.allowed_callers.clone(),
            blocked_callers: config.blocked_callers.clone(),
            ..TelephoneConfig::default()
        };
        let (telephone, _events) = Telephone::new(telephone_config);
        let hardware_ui = HardwareUi::from_config(&config.hardware).unwrap();
        let hardware_last_event = Instant::now();
        App {
            rnsconfig: None,
            config,
            identity,
            telephone,
            service: false,
            node: None,
            network_events: None,
            active_link: None,
            active_audio: None,
            active_ringer: None,
            last_dialed: None,
            hardware_ui,
            hardware_last_event,
            hardware_events: None,
            hardware_poller: None,
            test_sent_signals: Vec::new(),
            test_started_audio: Vec::new(),
            test_call_audio_running: false,
            test_ringer_running: false,
            test_ringer_starts: 0,
            test_ringer_stops: 0,
            test_call_audio_stops: 0,
        }
    }

    fn test_app() -> App {
        test_app_with_config(RnphoneConfig::default())
    }

    fn phonebook_config() -> RnphoneConfig {
        RnphoneConfig::parse("[phonebook]\nMary = f3e8c3359b39d36f3baff0a616a73d3e, 12\n").unwrap()
    }

    fn display_config() -> RnphoneConfig {
        RnphoneConfig::parse("[hardware]\ndisplay = i2c_lcd1602\n").unwrap()
    }

    fn key_down(key: char) -> KeypadEvent {
        KeypadEvent {
            key: Key::Char(key),
            transition: KeyTransition::Down,
        }
    }

    fn hook_down() -> KeypadEvent {
        KeypadEvent {
            key: Key::Hook,
            transition: KeyTransition::Down,
        }
    }

    fn hook_up() -> KeypadEvent {
        KeypadEvent {
            key: Key::Hook,
            transition: KeyTransition::Up,
        }
    }

    fn display_row(ui: &HardwareUi, row: usize) -> &str {
        ui.display.as_ref().unwrap().row(row).unwrap()
    }

    fn link_id(byte: u8) -> rns_net::LinkId {
        rns_net::LinkId([byte; 16])
    }

    fn dest_hash(byte: u8) -> rns_net::DestHash {
        rns_net::DestHash([byte; 16])
    }

    fn identity_hash(byte: u8) -> rns_net::IdentityHash {
        rns_net::IdentityHash([byte; 16])
    }

    fn public_key(byte: u8) -> [u8; 64] {
        [byte; 64]
    }

    fn established_link_event(
        link_id: rns_net::LinkId,
        is_initiator: bool,
    ) -> TelephonyNetworkEvent {
        TelephonyNetworkEvent::LinkEstablished {
            link_id,
            dest_hash: dest_hash(0xD0),
            rtt: 0.125,
            is_initiator,
        }
    }

    fn remote_identified_event(
        link_id: rns_net::LinkId,
        identity_hash: rns_net::IdentityHash,
    ) -> TelephonyNetworkEvent {
        TelephonyNetworkEvent::RemoteIdentified {
            link_id,
            identity_hash,
            public_key: public_key(0xA5),
        }
    }

    fn link_data_signal(signal: Signal) -> TelephonyNetworkEvent {
        TelephonyNetworkEvent::LinkData {
            link_id: link_id(0x77),
            context: 0,
            data: LxstPacket::signalling(signal).encode().unwrap(),
        }
    }

    #[test]
    fn hardware_keypad_answer_routes_through_app_call_flow() {
        let mut app = test_app();
        let endpoint = TelephonyEndpoint::new(&app.identity);
        let link_id = link_id(0x20);
        app.active_link = Some(link_id.0);
        assert!(app.telephone.begin_incoming_call([0x21; 16]));

        app.handle_hardware_keypad_event(&endpoint, key_down('D'))
            .unwrap();

        assert_eq!(app.telephone.state(), CallState::Established);
        assert_eq!(
            app.test_sent_signals,
            vec![
                (link_id.0, Signal::Code(SignalCode::Connecting)),
                (link_id.0, Signal::Code(SignalCode::Established)),
            ]
        );
        assert_eq!(app.test_started_audio, vec![link_id.0]);
        assert!(app.test_call_audio_running);
    }

    #[test]
    fn hardware_keypad_reject_routes_through_app_teardown() {
        let mut app = test_app();
        let endpoint = TelephonyEndpoint::new(&app.identity);
        let link_id = link_id(0x22);
        app.active_link = Some(link_id.0);
        assert!(app.telephone.begin_incoming_call([0x23; 16]));

        app.handle_hardware_keypad_event(&endpoint, key_down('C'))
            .unwrap();

        assert_eq!(app.telephone.state(), CallState::Available);
        assert_eq!(app.active_link, None);
        assert_eq!(
            app.test_sent_signals,
            vec![(link_id.0, Signal::Code(SignalCode::Rejected))]
        );
    }

    #[test]
    fn hardware_hook_down_hangs_up_active_call() {
        let mut app = test_app();
        let endpoint = TelephonyEndpoint::new(&app.identity);
        let link_id = link_id(0x24);
        app.active_link = Some(link_id.0);
        assert!(app.telephone.begin_outgoing_call([0x25; 16]));
        assert!(app.telephone.establish());
        app.test_call_audio_running = true;

        app.handle_hardware_keypad_event(&endpoint, hook_down())
            .unwrap();

        assert_eq!(app.telephone.state(), CallState::Available);
        assert_eq!(app.active_link, None);
        assert!(!app.test_call_audio_running);
        assert_eq!(app.test_call_audio_stops, 1);
        assert!(app.test_sent_signals.is_empty());
    }

    #[test]
    fn hardware_display_returns_to_ready_on_call_teardown() {
        let mut app = test_app_with_config(display_config());
        let endpoint = TelephonyEndpoint::new(&app.identity);
        let link_id = link_id(0x26);
        app.handle_hardware_keypad_event(&endpoint, key_down('4'))
            .unwrap();
        assert_eq!(display_row(&app.hardware_ui, 0), "4               ");

        app.active_link = Some(link_id.0);
        assert!(app.telephone.begin_outgoing_call([0x27; 16]));
        assert!(app.telephone.establish());
        app.hangup_current();

        assert_eq!(app.telephone.state(), CallState::Available);
        assert_eq!(display_row(&app.hardware_ui, 0), "Telephone Ready ");
        assert_eq!(display_row(&app.hardware_ui, 1), "                ");
    }

    #[test]
    fn app_hardware_idle_poll_sleeps_display_after_last_event_timeout() {
        let mut app = test_app_with_config(display_config());
        let start = Instant::now();
        app.became_available_at(start);

        assert!(!app.poll_hardware_idle(start + HW_SLEEP_TIMEOUT - Duration::from_millis(1)));
        assert_eq!(app.hardware_ui.mode, HardwareMode::Idle);

        assert!(app.poll_hardware_idle(start + HW_SLEEP_TIMEOUT));
        assert_eq!(app.hardware_ui.mode, HardwareMode::Sleep);
        assert!(app.hardware_ui.display.as_ref().unwrap().is_sleeping());
    }

    #[test]
    fn app_hardware_keypad_event_resets_idle_sleep_timer() {
        let mut app = test_app_with_config(display_config());
        let endpoint = TelephonyEndpoint::new(&app.identity);
        let start = Instant::now();
        app.became_available_at(start);

        app.handle_hardware_keypad_event_at(&endpoint, hook_down(), start + Duration::from_secs(8))
            .unwrap();

        assert_eq!(app.hardware_ui.mode, HardwareMode::Idle);
        assert!(!app.hardware_ui.display.as_ref().unwrap().is_sleeping());
        assert!(!app.poll_hardware_idle(start + Duration::from_secs(20)));
    }

    #[test]
    fn app_polls_hardware_keypad_events_from_channel() {
        let mut app = test_app_with_config(display_config());
        let endpoint = TelephonyEndpoint::new(&app.identity);
        let (tx, rx) = mpsc::channel();
        app.hardware_events = Some(rx);
        tx.send(key_down('7')).unwrap();

        let handled = app.poll_hardware_events(&endpoint, Instant::now()).unwrap();

        assert_eq!(handled, 1);
        assert_eq!(app.hardware_ui.mode, HardwareMode::Dial);
        assert_eq!(app.hardware_ui.input, "7");
        assert_eq!(display_row(&app.hardware_ui, 0), "7               ");
    }

    #[cfg(not(feature = "gpio-rpi"))]
    #[test]
    fn configured_keypad_starts_host_noop_event_channel() {
        let config =
            RnphoneConfig::parse("[hardware]\nkeypad = gpio_4x4\nkeypad_hook_pin = 5\n").unwrap();

        let (events, poller) = start_hardware_keypad(&config.hardware).unwrap();

        assert!(events
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_millis(50))
            .is_err());
        assert!(poller.is_some());
    }

    #[test]
    fn incoming_remote_identity_rings_and_tracks_link() {
        let mut app = test_app();
        let link_id = link_id(0x11);
        let caller = identity_hash(0x22);

        app.handle_network_event(remote_identified_event(link_id, caller));

        assert_eq!(app.telephone.state(), CallState::Ringing);
        assert_eq!(app.active_link, Some(link_id.0));
        assert_eq!(
            app.test_sent_signals,
            vec![(link_id.0, Signal::Code(SignalCode::Ringing))]
        );
        assert!(app.test_ringer_running);
        assert_eq!(app.test_ringer_starts, 1);
    }

    #[test]
    fn inbound_link_can_establish_before_remote_identity() {
        let mut app = test_app();
        let link_id = link_id(0x33);
        let caller = identity_hash(0x44);

        app.handle_network_event(established_link_event(link_id, false));
        assert_eq!(app.active_link, Some(link_id.0));
        assert_eq!(app.telephone.state(), CallState::Available);
        assert!(app.test_sent_signals.is_empty());

        app.handle_network_event(remote_identified_event(link_id, caller));
        assert_eq!(app.active_link, Some(link_id.0));
        assert_eq!(app.telephone.state(), CallState::Ringing);
        assert_eq!(
            app.test_sent_signals,
            vec![(link_id.0, Signal::Code(SignalCode::Ringing))]
        );
    }

    #[test]
    fn blocked_incoming_identity_sends_busy_on_identified_link() {
        let blocked = [0x55; 16];
        let mut config = RnphoneConfig::default();
        config.blocked_callers.insert(blocked);
        let mut app = test_app_with_config(config);
        let link_id = link_id(0x56);

        app.handle_network_event(remote_identified_event(
            link_id,
            rns_net::IdentityHash(blocked),
        ));

        assert_eq!(app.telephone.state(), CallState::Available);
        assert_eq!(app.active_link, None);
        assert_eq!(
            app.test_sent_signals,
            vec![(link_id.0, Signal::Code(SignalCode::Busy))]
        );
        assert_eq!(app.test_ringer_starts, 0);
    }

    #[test]
    fn outgoing_link_established_sends_established_and_starts_audio() {
        let mut app = test_app();
        let remote = [0x66; 16];
        let link_id = link_id(0x67);
        assert!(app.telephone.begin_outgoing_call(remote));

        app.handle_network_event(established_link_event(link_id, true));

        assert_eq!(app.active_link, Some(link_id.0));
        assert_eq!(app.telephone.state(), CallState::Established);
        assert_eq!(
            app.test_sent_signals,
            vec![(link_id.0, Signal::Code(SignalCode::Established))]
        );
        assert_eq!(app.test_started_audio, vec![link_id.0]);
        assert!(app.test_call_audio_running);
    }

    #[test]
    fn incoming_established_signal_before_answer_does_not_start_audio() {
        let mut app = test_app();
        let link_id = link_id(0x70);
        app.handle_network_event(remote_identified_event(link_id, identity_hash(0x71)));

        app.handle_network_event(link_data_signal(Signal::Code(SignalCode::Established)));

        assert_eq!(app.telephone.state(), CallState::Ringing);
        assert!(app.test_started_audio.is_empty());
        assert!(app.test_ringer_running);
    }

    #[test]
    fn outgoing_established_signal_starts_audio_once_call_is_established() {
        let mut app = test_app();
        let remote = [0x72; 16];
        let link_id = link_id(0x73);
        assert!(app.telephone.begin_outgoing_call(remote));
        app.active_link = Some(link_id.0);

        app.handle_network_event(link_data_signal(Signal::Code(SignalCode::Established)));
        app.handle_network_event(link_data_signal(Signal::Code(SignalCode::Established)));

        assert_eq!(app.telephone.state(), CallState::Established);
        assert_eq!(app.test_started_audio, vec![link_id.0]);
        assert!(app.test_call_audio_running);
    }

    #[test]
    fn remote_busy_signal_cleans_active_link_and_audio() {
        let mut app = test_app();
        let remote = [0x80; 16];
        let link_id = link_id(0x81);
        assert!(app.telephone.begin_outgoing_call(remote));
        app.active_link = Some(link_id.0);
        app.test_call_audio_running = true;

        app.handle_network_event(link_data_signal(Signal::Code(SignalCode::Busy)));

        assert_eq!(app.telephone.state(), CallState::Available);
        assert_eq!(app.active_link, None);
        assert!(!app.test_call_audio_running);
        assert_eq!(app.test_call_audio_stops, 1);
    }

    #[test]
    fn remote_rejected_signal_cleans_active_link_and_ringer() {
        let mut app = test_app();
        let remote = [0x82; 16];
        let link_id = link_id(0x83);
        assert!(app.telephone.begin_outgoing_call(remote));
        app.active_link = Some(link_id.0);
        app.test_ringer_running = true;

        app.handle_network_event(link_data_signal(Signal::Code(SignalCode::Rejected)));

        assert_eq!(app.telephone.state(), CallState::Available);
        assert_eq!(app.active_link, None);
        assert!(!app.test_ringer_running);
        assert_eq!(app.test_ringer_stops, 1);
    }

    #[test]
    fn closed_non_active_link_does_not_touch_current_call() {
        let mut app = test_app();
        let active = link_id(0x90);
        app.active_link = Some(active.0);
        assert!(app.telephone.begin_outgoing_call([0x91; 16]));
        app.test_call_audio_running = true;
        app.test_ringer_running = true;

        app.handle_network_event(TelephonyNetworkEvent::LinkClosed {
            link_id: link_id(0x92),
            reason: None,
        });

        assert_eq!(app.active_link, Some(active.0));
        assert_eq!(app.telephone.state(), CallState::Calling);
        assert!(app.test_call_audio_running);
        assert!(app.test_ringer_running);
        assert_eq!(app.test_call_audio_stops, 0);
        assert_eq!(app.test_ringer_stops, 0);
    }

    #[test]
    fn closed_active_link_cleans_call_state_and_side_effects() {
        let mut app = test_app();
        let active = link_id(0x94);
        app.active_link = Some(active.0);
        assert!(app.telephone.begin_outgoing_call([0x95; 16]));
        app.test_call_audio_running = true;
        app.test_ringer_running = true;

        app.handle_network_event(TelephonyNetworkEvent::LinkClosed {
            link_id: active,
            reason: None,
        });

        assert_eq!(app.active_link, None);
        assert_eq!(app.telephone.state(), CallState::Available);
        assert!(!app.test_call_audio_running);
        assert!(!app.test_ringer_running);
        assert_eq!(app.test_call_audio_stops, 1);
        assert_eq!(app.test_ringer_stops, 1);
    }

    #[test]
    fn transmit_audio_frame_obeys_sink_backpressure() {
        let mut source =
            FakeInputSource::with_frame(lxst::AudioFrame::new(8_000, 1, vec![0.0, 0.25]).unwrap());
        source.start();
        let mut codec = lxst::RawCodec::default();
        let mut sink = FakeTransmitSink {
            can_receive: false,
            frames: Vec::new(),
        };

        let transmitted = transmit_audio_frame(&mut source, &mut codec, &mut sink).unwrap();

        assert!(!transmitted);
        assert_eq!(source.pulls, 0);
        assert!(sink.frames.is_empty());
    }

    #[test]
    fn transmit_audio_frame_encodes_when_sink_can_receive() {
        let mut source =
            FakeInputSource::with_frame(lxst::AudioFrame::new(8_000, 1, vec![0.0, 0.25]).unwrap());
        source.start();
        let mut codec = lxst::RawCodec::default();
        let mut sink = FakeTransmitSink {
            can_receive: true,
            frames: Vec::new(),
        };

        let transmitted = transmit_audio_frame(&mut source, &mut codec, &mut sink).unwrap();

        assert!(transmitted);
        assert_eq!(source.pulls, 1);
        assert_eq!(sink.frames.len(), 1);
        assert_eq!(sink.frames[0].codec, lxst::core::CodecKind::Raw);
    }

    struct FakeInputSource {
        frames: std::collections::VecDeque<lxst::AudioFrame>,
        running: bool,
        pulls: usize,
    }

    impl FakeInputSource {
        fn with_frame(frame: lxst::AudioFrame) -> Self {
            Self {
                frames: std::collections::VecDeque::from([frame]),
                running: false,
                pulls: 0,
            }
        }
    }

    impl AudioSource for FakeInputSource {
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
            8_000
        }

        fn channels(&self) -> u8 {
            1
        }

        fn next_frame(&mut self) -> Result<Option<lxst::AudioFrame>, lxst::PipelineError> {
            self.pulls += 1;
            Ok(self.frames.pop_front())
        }
    }

    struct FakeTransmitSink {
        can_receive: bool,
        frames: Vec<EncodedAudioFrame>,
    }

    impl AudioSink for FakeTransmitSink {
        fn can_receive(&self) -> bool {
            self.can_receive
        }

        fn handle_frame(&mut self, frame: EncodedAudioFrame) -> Result<(), lxst::PipelineError> {
            self.frames.push(frame);
            Ok(())
        }
    }

    fn temp_config_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!(
            "lxst-rs-rnphone-{name}-{}-{nanos}",
            std::process::id()
        ))
    }
}
