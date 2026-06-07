# Upstream Tracking

This repository is a Rust implementation of the Python LXST project.

The current upstream reference baseline is:

- Project: LXST
- Repository: `https://github.com/markqvist/lxst`
- Local checkout used: `/home/lelloman/lxst`
- Version: `0.4.5`
- Branch: `origin/master`
- Commit: `1194c9011fe6402edc7aebe7ffe9650ea3b1afee`
- Describe: `0.4.4-6-g1194c90`
- Commit date: `2025-12-28 01:00:21 +0100`
- Subject: `Updated readme`

The first Rust porting pass implemented repository metadata/CI/licensing, LXST packet wire helpers, corrected telephony profile/signalling constants, Raw codec support, deterministic audio/DSP primitives, Reticulum link helper APIs, an event-driven telephony state skeleton, and an `rnphone` CLI scaffold. Opus, Codec2, CPAL live audio, and full live-call orchestration remain to be ported.

When integrating future upstream changes, compare this baseline against the new
LXST upstream commit, review protocol constants, frame and signalling formats,
codec behavior, audio pipeline behavior, telephony call flow, bundled assets,
and utility changes, then port or explicitly defer each Rust-applicable item.

## RNS Dependency Baseline

`lxst-rs` depends on published Rust Reticulum crates from crates.io:

- `rns-core` `=0.1.13`
- `rns-net` `=0.5.10`

The corresponding local `rns-rs` reference inspected during setup was:

- Repository: `git@github.com:lelloman/rns-rs.git`
- Branch used for integration context: `dev`
- Commit: `aa7fb1e3a239642e720d76db962dfb6b05a1e9fd`
- Describe: `rns-cli-v0.2.4-146-gaa7fb1e`
- Commit date: `2026-06-04 14:08:39 +0200`
- Subject: `Document upstream Reticulum daily check`

Treat upstream Python RNS dependency changes as dependency-parity review input,
not as direct Cargo version edits. When updating RNS integration, publish the
required `rns-rs` crates, update exact versions in `Cargo.toml`, run the
workspace checks, and record the new release/baseline here.
