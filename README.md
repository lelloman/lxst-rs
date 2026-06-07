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

The workspace now contains a tested transport-neutral wire layer, Raw codec support, deterministic audio/DSP primitives, Reticulum link helper APIs, an event-driven telephony state API, and an `rnphone` terminal utility scaffold. It is not yet a complete live voice client. The remaining major gaps are:

- libopus-backed Opus encode/decode
- libcodec2 FFI for 700C/1600/3200 telephony profiles
- CPAL-backed Linux live audio input/output
- full Reticulum callback orchestration for live call setup and frame exchange
- Docker/live-network interoperability tests against Python LXST

The Python project is early alpha and explicitly API-unstable, so this port
tracks behavior and wire format deliberately instead of copying incidental
Python APIs.

## Development Checks

```bash
cargo fmt --check
bash scripts/lint-host.sh
cargo test --workspace
```
