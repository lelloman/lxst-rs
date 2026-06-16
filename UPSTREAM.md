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

The Rust port now includes repository metadata/CI/licensing, LXST packet wire helpers, telephony profile/signalling constants, Raw/Opus/Codec2 codec support, deterministic audio/DSP primitives, CPAL audio I/O, Ogg Opus media, Reticulum link helper APIs, an event-driven telephony state machine, and an `rnphone` CLI with live call/audio/signalling wiring plus optional Raspberry Pi hardware hooks. Remaining upstream parity work is tracked through validation artifacts and future upstream-delta reviews rather than known missing core protocol pieces.

The current upstream baseline has been reviewed against the local checkout at
`/home/lelloman/lxst`. Applicable protocol, codec, media, telephony, rnphone,
and validation behavior has been ported or covered by deterministic fixtures
and tests. Python-only packaging, generated documentation, and live
platform-result artifacts are not vendored; live platform parity remains tied
to captured validation output.

A local helper path may be stored in `.local/lxst-upstream.path`. The `.local/`
directory is intentionally ignored, matching the sibling upstream-tracking
convention used by `rns-rs` and `lxmf-rs`.

When integrating future upstream changes, compare this baseline against the new
LXST upstream commit, review protocol constants, frame and signalling formats,
codec behavior, audio pipeline behavior, telephony call flow, bundled assets,
and utility changes, then port or explicitly defer each Rust-applicable item.

## Completed LXST Parity Closure Queue

The full parity closure pass reviewed LXST upstream through
`1194c9011fe6402edc7aebe7ffe9650ea3b1afee` and closed the remaining
deterministic Rust-applicable gaps with the following local commits:

- `b561d4b` `telephony: tighten rnphone call ownership`
  - added the public `Telephone::active_session()` snapshot API, tightened
    inbound/outbound link ownership, made terminal call cleanup idempotent,
    completed preferred-profile negotiation, recreated call audio on the new
    profile once, and added a testable rnphone call-flow output seam.
- `f1399bd` `interop: expand upstream packet fixtures`
  - expanded upstream packet, codec, Codec2, and Ogg Opus fixture coverage,
    including malformed/unknown/duplicate packet fields and non-byte-stable
    Opus decode-shape assertions.
- `23ca913` `validation: add pipeline graph harnesses`
  - added deterministic live-pipeline graph tests and validation scripts for
    rnphone two-node smoke, Python/Rust signalling, packet/media round trips,
    and local CPAL probing.
- `99f0267` `validation: document platform targets`
  - documented Linux, macOS, Windows, Android, Raspberry Pi GPIO keypad, and
    I2C LCD1602 validation commands, expected artifacts, and pass/fail
    criteria, plus validation-only examples for hardware/audio probes.
- `d3d0722` `ci: add validation checks`
  - extended CI and docs for workspace tests, formatting, host linting,
    example checks, cross-platform default-feature checks, and the Raspberry Pi
    GPIO feature check.

The detailed working audit is recorded in the intentionally untracked
`UPSTREAM_COMMIT_ANALYSIS.md` file.

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
