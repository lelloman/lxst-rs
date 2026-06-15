use std::env;

use lxst::{CallProfile, CodecProfile};

#[derive(Debug, Clone, Copy)]
struct Simulation {
    link_speed: f64,
    audio_slot_ms: f64,
    codec_rate: f64,
    signalling_bytes: usize,
    token_overhead: usize,
}

impl Default for Simulation {
    fn default() -> Self {
        Self {
            link_speed: 10_000.0,
            audio_slot_ms: 353.0,
            codec_rate: 6_000.0,
            signalling_bytes: 0,
            token_overhead: 48,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let simulation = parse_args()?;
    print_profile_table();
    simulate(simulation);
    Ok(())
}

fn parse_args() -> Result<Simulation, Box<dyn std::error::Error>> {
    let mut simulation = Simulation::default();
    let mut args = env::args().skip(1);
    if let Some(value) = args.next() {
        simulation.link_speed = value.parse()?;
    }
    if let Some(value) = args.next() {
        simulation.audio_slot_ms = value.parse()?;
    }
    if let Some(value) = args.next() {
        simulation.codec_rate = value.parse()?;
    }
    if let Some(value) = args.next() {
        simulation.signalling_bytes = value.parse()?;
    }
    if let Some(value) = args.next() {
        simulation.token_overhead = value.parse()?;
    }
    Ok(simulation)
}

fn print_profile_table() {
    println!("===== LXST Call Profiles ===\n");
    for profile in CallProfile::available_profiles() {
        let codec = profile.codec_profile();
        println!(
            "  {:22} {:>4} ms  {:>7} bps  {:?}",
            profile.name(),
            profile.frame_duration().as_millis(),
            codec_rate(codec),
            codec
        );
    }
}

fn simulate(simulation: Simulation) {
    let packets_per_second = 1000.0 / simulation.audio_slot_ms;
    let audio_len = (simulation.codec_rate / packets_per_second / 8.0).ceil() as usize;
    let packing_overhead = msgpack_payload_overhead(simulation.signalling_bytes, audio_len);
    let payload_len = audio_len + packing_overhead;

    let phy_overhead = 1usize;
    let rns_overhead = 19usize;
    let transport_overhead = phy_overhead + rns_overhead;
    let block_size = 16usize;
    let required_blocks = (payload_len + 1).div_ceil(block_size);
    let encrypted_payload_len = required_blocks * block_size;
    let block_headroom = encrypted_payload_len.saturating_sub(payload_len + 1);
    let packet_len =
        phy_overhead + rns_overhead + encrypted_payload_len + simulation.token_overhead;
    let per_byte_latency_ms = 1000.0 / (simulation.link_speed / 8.0);
    let packet_airtime = packet_len as f64 * per_byte_latency_ms;
    let airtime_pct = (packet_airtime / simulation.audio_slot_ms) * 100.0;
    let concurrent_calls = (100.0 / airtime_pct).floor() as usize;
    let encrypted_padding = encrypted_payload_len.saturating_sub(payload_len);

    println!("\n===== Simulation Parameters ===\n");
    println!("  Packing method       : msgpack");
    println!(
        "  Sampling delay       : {:.0} ms",
        simulation.audio_slot_ms
    );
    println!("  Codec bitrate        : {:.0} bps", simulation.codec_rate);
    println!("  Audio data           : {audio_len} bytes");
    println!("  Packing overhead     : {packing_overhead} bytes");
    println!("  Payload length       : {payload_len} bytes");
    println!("  AES blocks needed    : {required_blocks}");
    println!("  Encrypted payload    : {encrypted_payload_len} bytes");
    println!(
        "  Token overhead       : {} bytes",
        simulation.token_overhead
    );
    println!(
        "  Transport overhead   : {transport_overhead} bytes ({rns_overhead} from RNS, {phy_overhead} from PHY)"
    );
    println!("  On-air length        : {packet_len} bytes");
    println!("  Packet airtime       : {:.2} ms", packet_airtime);
    println!(
        "  Transport bitrate    : {}",
        format_speed((packet_len as f64 * 8.0) / (simulation.audio_slot_ms / 1000.0))
    );

    println!(
        "\n===== Results for {} Link Speed ===\n",
        format_speed(simulation.link_speed)
    );
    println!(
        "  Final latency        : {:.1} ms",
        simulation.audio_slot_ms + packet_airtime
    );
    println!(
        "    Recording latency  : contributes {:.0} ms",
        simulation.audio_slot_ms
    );
    println!(
        "    Packet transport   : contributes {:.1} ms",
        packet_airtime
    );
    println!(
        "      Payload          : contributes {:.1} ms",
        encrypted_payload_len as f64 * per_byte_latency_ms
    );
    println!(
        "        Audio data     : contributes {:.1} ms",
        audio_len as f64 * per_byte_latency_ms
    );
    println!(
        "        Packing format : contributes {:.1} ms",
        packing_overhead as f64 * per_byte_latency_ms
    );
    println!(
        "        Encryption     : contributes {:.1} ms {}",
        encrypted_padding as f64 * per_byte_latency_ms,
        if encrypted_padding == 1 {
            "(optimal)"
        } else {
            "(sub-optimal)"
        }
    );
    println!(
        "      Token overhead   : contributes {:.1} ms",
        simulation.token_overhead as f64 * per_byte_latency_ms
    );
    println!(
        "      RNS+PHY overhead : contributes {:.1} ms",
        transport_overhead as f64 * per_byte_latency_ms
    );
    println!();
    println!(
        "  Half-duplex airtime  : {:.2}% of link capacity",
        airtime_pct
    );
    println!("    Concurrent calls   : {concurrent_calls}");
    println!(
        "  Full-duplex airtime  : {:.2}% of link capacity",
        airtime_pct * 2.0
    );
    println!("    Concurrent calls   : {}", concurrent_calls / 2);

    if block_headroom != 0 {
        println!(
            "\n  Unaligned AES block: each packet could fit {block_headroom} bytes of additional audio data"
        );
    }
}

fn msgpack_payload_overhead(signalling_bytes: usize, audio_len: usize) -> usize {
    1 + msgpack_bin_header_len(signalling_bytes)
        + signalling_bytes
        + msgpack_bin_header_len(audio_len)
}

fn msgpack_bin_header_len(len: usize) -> usize {
    if len <= u8::MAX as usize {
        2
    } else if len <= u16::MAX as usize {
        3
    } else {
        5
    }
}

fn codec_rate(profile: CodecProfile) -> u32 {
    match profile {
        CodecProfile::Codec2_700C => 700,
        CodecProfile::Codec2_1200 => 1_200,
        CodecProfile::Codec2_1300 => 1_300,
        CodecProfile::Codec2_1400 => 1_400,
        CodecProfile::Codec2_1600 => 1_600,
        CodecProfile::Codec2_2400 => 2_400,
        CodecProfile::Codec2_3200 => 3_200,
        other => other.info().bitrate_ceiling,
    }
}

fn format_speed(bits_per_second: f64) -> String {
    if bits_per_second >= 1_000_000.0 {
        format!("{:.1} Mbps", bits_per_second / 1_000_000.0)
    } else if bits_per_second >= 1_000.0 {
        format!("{:.1} Kbps", bits_per_second / 1_000.0)
    } else {
        format!("{bits_per_second:.0} bps")
    }
}
