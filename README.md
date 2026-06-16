# lxst-rs

[![CI](https://github.com/lelloman/lxst-rs/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/lelloman/lxst-rs/actions/workflows/ci.yml?query=branch%3Amaster)
[![License](https://img.shields.io/badge/license-Reticulum-blue)](./LICENSE)

Rust port of LXST, the Lightweight Extensible Signal Transport protocol and
real-time audio toolkit for Reticulum.

LXST is a simple real-time streaming format and delivery protocol built on top
of Reticulum. The Python reference includes networking, codec, audio pipeline,
mixing, filtering, telephony, and utility components. This Rust repository
tracks the Python behavior and wire format while exposing Rust APIs for codecs,
media, live Reticulum link audio, and rnphone call handling.

The upstream Python baseline is recorded in [UPSTREAM.md](./UPSTREAM.md).

## Workspace Crates

| Crate | Description |
|-------|-------------|
| `lxst-core` | Frame metadata, codec/profile identifiers, signalling values, and transport-neutral protocol types. |
| `lxst` | High-level codec, audio/DSP, Reticulum bridge, and event-driven telephony API. |
| `rnphone` | Terminal telephone utility with Python-compatible config, Reticulum signalling, live call audio, ringer, and optional Raspberry Pi hardware hooks. |

## Current Status

The workspace contains a tested transport-neutral wire layer, Raw/Opus/Codec2 codec support, deterministic audio/DSP primitives, CPAL audio I/O, Ogg Opus file media, Reticulum link helper APIs, an event-driven telephony API, and an `rnphone` terminal utility with live call/audio/signalling wiring. Deterministic parity tests and validation harnesses are present; remaining parity work is mostly capturing live platform artifacts across Linux, macOS, Windows, Android, and Raspberry Pi hardware.

## Examples

```bash
cargo run -p lxst --example tone_generator
cargo run -p lxst --example filters
cargo run -p lxst --example mixer
cargo run -p lxst --example pipelines
cargo run -p lxst --example file_source -- input.opus output.opus
cargo run -p lxst --example call_calculator
cargo run -p lxst --example file_player -- path/to/audio.opus
cargo run -p lxst --example file_player -- path/to/audio.opus --loop
cargo run -p lxst --example file_recorder -- recording.opus
cargo run -p lxst --example keypad_scan_dump -- 0 0
cargo run -p lxst --example lcd1602_render -- "LXST validation" "LCD1602 OK"
```

The DSP, pipeline, keypad dump, and LCD render examples are deterministic and do not require audio hardware. The player example exits at EOF unless `--loop` is supplied, in which case Enter stops playback. The recorder example records from the default input device until Enter is pressed.

The Python project is early alpha and explicitly API-unstable, so this port
tracks behavior and wire format deliberately instead of copying incidental
Python APIs.

## rnphone

`rnphone` operation notes, config examples, and service commands are documented
in [rnphone/README.md](./rnphone/README.md).

## Validation

Validation targets, live smoke scripts, platform requirements, and pass/fail criteria are documented in [VALIDATION.md](./VALIDATION.md). Generated validation artifacts are written under `validation/results/` and are ignored by default.

## Development Checks

```bash
cargo fmt --check
bash scripts/lint-host.sh
cargo test --workspace
cargo check -p lxst --examples
cargo check -p rnphone --features gpio-rpi
```

Dependency notes: Opus uses libopus through the Rust `opus` crate, Codec2 700C requires a runtime-loadable system `libcodec2`, CPAL depends on the host audio backend, and Raspberry Pi GPIO/I2C support requires the `gpio-rpi` feature plus device access.
