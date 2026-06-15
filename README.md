# lxst-rs

[![CI](https://github.com/lelloman/lxst-rs/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/lelloman/lxst-rs/actions/workflows/ci.yml?query=branch%3Amaster)
[![License](https://img.shields.io/badge/license-Reticulum-blue)](./LICENSE)

Rust port of LXST, the Lightweight Extensible Signal Transport protocol and
real-time audio toolkit for Reticulum.

LXST is a simple real-time streaming format and delivery protocol built on top
of Reticulum. The Python reference includes networking, codec, audio pipeline,
mixing, filtering, telephony, and utility components. This Rust repository is
currently an early scaffold that captures a small transport-neutral protocol
surface and establishes the same tracking and maintenance conventions used by
the sibling `rns-rs` and `lxmf-rs` ports.

The upstream Python baseline is recorded in [UPSTREAM.md](./UPSTREAM.md).

## Workspace Crates

| Crate | Description |
|-------|-------------|
| `lxst-core` | Frame metadata, codec/profile identifiers, signalling values, and transport-neutral protocol types. |
| `lxst` | High-level codec, audio/DSP, Reticulum bridge, and event-driven telephony API. |
| `rnphone` | Terminal telephone utility scaffold with Python-compatible config and command surface. |

## Current Status

The workspace contains a tested transport-neutral wire layer, Raw/Opus/Codec2 codec support, deterministic audio/DSP primitives, CPAL audio I/O, Ogg Opus file media, Reticulum link helper APIs, an event-driven telephony API, and an `rnphone` terminal utility with live call/audio/signalling wiring. Remaining parity work is focused on deeper platform validation, longer live-network interoperability runs against Python LXST, final terminal polish, and examples/docs coverage.

## Examples

```bash
cargo run -p lxst --example tone_generator
cargo run -p lxst --example filters
cargo run -p lxst --example mixer
cargo run -p lxst --example pipelines
cargo run -p lxst --example file_player -- path/to/audio.opus
cargo run -p lxst --example file_player -- path/to/audio.opus --loop
cargo run -p lxst --example file_recorder -- recording.opus
```

The DSP and pipeline examples are deterministic and do not require audio hardware. The player example exits at EOF unless `--loop` is supplied, in which case Enter stops playback. The recorder example records from the default input device until Enter is pressed.

The Python project is early alpha and explicitly API-unstable, so this port
tracks behavior and wire format deliberately instead of copying incidental
Python APIs.

## Development Checks

```bash
cargo fmt --check
bash scripts/lint-host.sh
cargo test --workspace
```
