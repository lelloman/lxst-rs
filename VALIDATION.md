# Validation

This file defines parity validation targets, commands, required devices, expected output, and pass/fail criteria. Result-producing scripts write JSON and Markdown summaries under `validation/results/`; those artifacts are intentionally ignored by git unless a release process explicitly captures them.

## Required Targets

| Target | Status | Required device/access | Command | Pass criteria |
| --- | --- | --- | --- | --- |
| Linux CPAL input/output | Harness ready, not captured | Linux host with microphone and speaker selected as defaults | `RUN_CPAL_PROBE=1 bash scripts/validation/cpal-probe.sh`; `cargo run -p lxst --example file_recorder -- validation/results/linux-cpal.opus`; `cargo run -p lxst --example file_player -- validation/results/linux-cpal.opus` | JSON status is `passed`, devices are listed, a 3 second recording is audible on playback, and result files are attached. |
| macOS CPAL input/output | Not captured | macOS host with microphone permission granted | Same commands as Linux on macOS | Same as Linux, with macOS artifact names. |
| Windows CPAL input/output | Not captured | Windows host with working default input/output | Same commands as Linux on Windows shell | Same as Linux, with Windows artifact names. |
| Android build/runtime audio probe | Not captured | Android SDK/NDK device or emulator with audio route | Build the Android target used by the release workflow, then run the CPAL probe equivalent on device | Build succeeds, runtime can enumerate/open audio route, and JSON/Markdown result is captured. |
| Raspberry Pi GPIO keypad | Not captured | Raspberry Pi with supported 4x4 or 5x5 keypad wired to documented GPIO pins | `cargo run -p lxst --example keypad_scan_dump -- 0 0`; `cargo test -p lxst --features gpio-rpi --test hardware_tests keypad_scanner_polls_backend_into_key_events` | Host scan dump shows expected map; hardware run records expected key transitions for each physical key. |
| Raspberry Pi I2C LCD1602 | Not captured | Raspberry Pi with LCD1602 on I2C bus 1 address `0x27` unless configured otherwise | `cargo run -p lxst --example lcd1602_render -- "LXST validation" "LCD1602 OK"`; `cargo test -p lxst --features gpio-rpi --test hardware_tests i2c_lcd1602_initializes_using_python_driver_sequence` | Host render shows two padded 16-char rows; hardware display renders both lines clearly and result artifact includes photo/log. |

Rows remain incomplete until result artifacts from the named platform or hardware are captured.

## Live Validation Scripts

Run from the repository root:

```sh
bash scripts/validation/rnphone-two-node-smoke.sh
bash scripts/validation/python-rust-signalling-smoke.sh
bash scripts/validation/packet-media-roundtrip.sh
bash scripts/validation/cpal-probe.sh
```

`rnphone-two-node-smoke.sh` requires `RUN_LIVE_RNS=1` to perform live Reticulum work. Without it, the script emits a skipped JSON/Markdown result.

`cpal-probe.sh` requires `RUN_CPAL_PROBE=1` to touch host audio. Without it, the script emits a skipped JSON/Markdown result.

`python-rust-signalling-smoke.sh` and `packet-media-roundtrip.sh` use `UPSTREAM_LXST=/home/lelloman/lxst` by default and fail if the upstream checkout is missing.

## Audio Commands

List devices:

```sh
cargo run -p rnphone -- --list-devices
```

Record Opus from the default input. Press Enter after 3 seconds:

```sh
cargo run -p lxst --example file_recorder -- validation/results/cpal-recording.opus
```

Play Opus to the default output:

```sh
cargo run -p lxst --example file_player -- validation/results/cpal-recording.opus
```

rnphone ringer output is validated by configuring `[telephone] ringer = <device substring>` and placing a call into the service, or by using the default ringtone assets created in the rnphone config directory during an incoming-call smoke.

## Hardware Commands

Keypad host map/dump:

```sh
cargo run -p lxst --example keypad_scan_dump -- 0 0
cargo run -p lxst --example keypad_scan_dump -- 0 0 hook
```

LCD host render:

```sh
cargo run -p lxst --example lcd1602_render -- "LXST validation" "LCD1602 OK"
```

Raspberry Pi build checks:

```sh
cargo check -p lxst --features gpio-rpi --examples
cargo check -p rnphone --features gpio-rpi
```

## Dependency Notes

- Opus support depends on the Rust `opus` crate and platform libopus availability as resolved by Cargo/build tooling.
- Codec2 700C decode/encode requires a runtime-loadable system `libcodec2`; other Codec2 modes use the pure Rust backend.
- CPAL uses the host backend selected by the platform: ALSA/JACK/Pulse/PipeWire availability affects Linux behavior, CoreAudio affects macOS, and WASAPI affects Windows.
- Raspberry Pi GPIO and I2C validation requires the `gpio-rpi` feature and access to `/dev/gpiomem`/GPIO plus `/dev/i2c-*` as appropriate.
- Android validation requires the project-specific Android target setup plus device runtime permissions for audio.
