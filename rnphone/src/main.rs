use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use lxst::network::{
    create_telephony_link, recall_telephony_identity, request_path_until, telephony_dest_hash,
    LinkSource, LxstLinkSender, Packetizer, TelephonyEndpoint,
};
use lxst::{
    Agc, AudioCodec, AudioSink, AudioSource, BandPass, CallProfile, CallState, CodecFactory,
    CodecSelection, CpalInputConfig, CpalInputSource, CpalOutputConfig, CpalOutputSink,
    EncodedAudioFrame, LxstPacket, Signal, SignalCode, Telephone, TelephoneConfig,
    TelephonyNetworkEvent,
};
use rns_crypto::identity::Identity;
use rns_crypto::OsRng;
use rns_net::storage::{load_identity, save_identity};
use rns_net::RnsNode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
                "-v" => parsed.verbose = parsed.verbose.saturating_add(1),
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

struct App {
    config_dir: PathBuf,
    rnsconfig: Option<PathBuf>,
    config: RnphoneConfig,
    identity: Identity,
    telephone: Telephone,
    service: bool,
    node: Option<Arc<RnsNode>>,
    network_events: Option<mpsc::Receiver<TelephonyNetworkEvent>>,
    active_link: Option<[u8; 16]>,
    active_audio: Option<CallAudio>,
    last_dialed: Option<[u8; 16]>,
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
        config.finalize_for_identity(identity.hash());

        let telephone_config = TelephoneConfig {
            allowed_callers: config.allowed_callers.clone(),
            blocked_callers: config.blocked_callers.clone(),
            ..TelephoneConfig::default()
        };
        let (telephone, _events) = Telephone::new(telephone_config);

        Ok(Self {
            config_dir,
            rnsconfig,
            config,
            identity,
            telephone,
            service,
            node: None,
            network_events: None,
            active_link: None,
            active_audio: None,
            last_dialed: None,
        })
    }

    fn start(&mut self) -> Result<(), String> {
        let endpoint = TelephonyEndpoint::new(&self.identity);
        println!("Reticulum Telephone Utility is ready");
        println!("  Identity hash: {}", hex(self.identity.hash()));
        println!("  Destination hash: {}", hex(&endpoint.destination.hash.0));
        println!("  Config: {}", self.config_dir.display());

        if self.service {
            self.ensure_network(&endpoint)?;
            self.announce(&endpoint)?;
            println!("Service mode running");
            loop {
                self.poll_network_events();
                self.telephone.tick();
                thread::sleep(Duration::from_millis(100));
            }
        }

        println!("Enter an identity hash to stage a call, or ? for help");
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
            match line {
                "?" | "h" | "help" => print_help_menu(),
                "q" | "quit" | "exit" => break,
                "i" | "identity" => println!("Identity hash: {}", hex(self.identity.hash())),
                "d" | "desthash" => {
                    println!("Destination hash: {}", hex(&endpoint.destination.hash.0))
                }
                "p" | "phonebook" => self.print_phonebook(),
                "a" | "announce" => self.announce(&endpoint)?,
                "r" | "redial" => match self.last_dialed {
                    Some(hash) => self.dial_hash(&endpoint, hash)?,
                    None => println!("Redial requires a completed call target"),
                },
                "answer" => {
                    if self.telephone.answer() {
                        self.send_signal(Signal::Code(SignalCode::Connecting));
                        let _ = self.telephone.establish();
                        self.send_signal(Signal::Code(SignalCode::Established));
                        if let Some(link_id) = self.active_link {
                            self.start_call_audio(link_id);
                        }
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
        println!("Announced {}", hex(&endpoint.destination.hash.0));
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
            println!("Dialing {} on link {}", hex(&identity_hash), hex(&link_id));
        } else {
            let _ = node.teardown_link(link_id);
            println!("Telephone is busy");
        }
        Ok(())
    }

    fn hangup_current(&mut self) {
        if self.telephone.state() == CallState::Ringing {
            self.send_signal(Signal::Code(SignalCode::Rejected));
        }
        self.stop_call_audio();
        if let (Some(node), Some(link_id)) = (self.node.as_ref(), self.active_link.take()) {
            let _ = node.teardown_link(link_id);
        }
        self.telephone.hangup();
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
                    self.stop_call_audio();
                    if self.telephone.state() != CallState::Available {
                        self.telephone.hangup();
                    }
                    println!("Link {link_id} closed");
                }
            }
            TelephonyNetworkEvent::RemoteIdentified {
                identity_hash,
                link_id,
                ..
            } => {
                if self.telephone.state() == CallState::Available {
                    if self.telephone.begin_incoming_call(identity_hash.0) {
                        self.active_link = Some(link_id.0);
                        self.send_signal(Signal::Code(SignalCode::Ringing));
                        println!("Incoming call from {identity_hash}");
                    } else {
                        self.send_signal(Signal::Code(SignalCode::Busy));
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
                        if signal == lxst::Signal::Code(SignalCode::Established) {
                            if let Some(link_id) = self.active_link {
                                let _ = self.telephone.establish();
                                self.start_call_audio(link_id);
                            }
                        }
                    }
                }
            }
        }
    }

    fn start_call_audio(&mut self, link_id: [u8; 16]) {
        if self.active_audio.is_some() {
            return;
        }
        let Some(node) = self.node.clone() else {
            return;
        };
        match CallAudio::start(node, link_id, self.telephone.active_profile()) {
            Ok(audio) => {
                self.active_audio = Some(audio);
                println!("Audio transport started");
            }
            Err(err) => println!("Audio transport unavailable: {err}"),
        }
    }

    fn stop_call_audio(&mut self) {
        if let Some(mut audio) = self.active_audio.take() {
            audio.stop();
        }
    }

    fn send_signal(&self, signal: Signal) {
        let (Some(node), Some(link_id)) = (self.node.clone(), self.active_link) else {
            return;
        };
        let sender = LxstLinkSender::new(node, link_id);
        let _ = sender.send_signal(signal);
    }

    fn print_phonebook(&self) {
        if self.config.phonebook.is_empty() {
            println!("No entries in phonebook");
            return;
        }
        println!("Phonebook");
        for (name, entry) in &self.config.phonebook {
            match &entry.alias {
                Some(alias) => println!("  {alias} {name}: {}", hex(&entry.identity_hash)),
                None => println!("  {name}: {}", hex(&entry.identity_hash)),
            }
        }
    }
}

struct CallAudio {
    link_source: Arc<Mutex<LinkSource>>,
    stop_tx: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl CallAudio {
    fn start(node: Arc<RnsNode>, link_id: [u8; 16], profile: CallProfile) -> Result<Self, String> {
        let codec_profile = profile.codec_profile();
        let frame_ms = profile.frame_duration().as_millis();

        let mut input = CpalInputSource::new(CpalInputConfig {
            target_frame_ms: frame_ms,
            codec_profile: Some(codec_profile),
            skip: Duration::from_millis(75),
            ease_in: Duration::from_millis(225),
            ..CpalInputConfig::default()
        })
        .map_err(|e| e.to_string())?;
        input.add_filter(BandPass::new(250.0, 8_500.0).map_err(|e| e.to_string())?);
        input.add_filter(Agc::new(-15.0, 12.0));

        let mut output =
            CpalOutputSink::new(CpalOutputConfig::default()).map_err(|e| e.to_string())?;
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
    allowed_callers: lxst::CallerPolicy,
    allow_phonebook_callers: bool,
    blocked_callers: HashSet<[u8; 16]>,
}

impl Default for RnphoneConfig {
    fn default() -> Self {
        Self {
            phonebook: HashMap::new(),
            allowed_callers: lxst::CallerPolicy::All,
            allow_phonebook_callers: false,
            blocked_callers: HashSet::new(),
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
                "phonebook" => {
                    let parts = split_list(value);
                    if let Some(identity_hash) = parts.first() {
                        let alias = parts
                            .get(1)
                            .map(|s| s.chars().filter(|c| c.is_ascii_digit()).collect::<String>())
                            .filter(|s| !s.is_empty());
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

    fn finalize_for_identity(&mut self, own_hash: &[u8; 16]) {
        self.phonebook
            .retain(|_, entry| &entry.identity_hash != own_hash);
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
        self.phonebook.iter().find_map(|(name, entry)| {
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
        "# This systemd unit allows installing rnphone as a system service on Linux-based devices\n[Unit]\nDescription=Reticulum Telephone Service\nAfter=sound.target\n\n[Service]\nExecStartPre=/bin/sleep 30\nType=simple\nRestart=always\nRestartSec=5\nUser={user}\nExecStart=/usr/local/bin/rnphone --service -vvv\n\n[Install]\nWantedBy=graphical.target\n"
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
