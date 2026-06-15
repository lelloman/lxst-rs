# rnphone

`rnphone` is the Rust LXST Reticulum telephone utility. It uses the same
configuration shape as upstream Python rnphone and stores its local identity,
default sounds, and config in the selected rnphone config directory.

## Commands

```bash
cargo run -p rnphone -- --help
cargo run -p rnphone -- --version
cargo run -p rnphone -- --list-devices
cargo run -p rnphone -- --config ~/.rnphone --rnsconfig ~/.reticulum
cargo run -p rnphone -- --service -vvv
cargo run -p rnphone -- --systemd
```

`--config` selects the rnphone config directory. `--rnsconfig` selects the
Reticulum config directory. If `--config` is omitted, rnphone uses
`/etc/rnphone` when present, then `~/.config/rnphone` when it already contains a
config file, and otherwise `~/.rnphone`.

`--list-devices` prints CPAL input and output device names. The `speaker`,
`microphone`, and `ringer` config values are fuzzy-matched against this list.

`--service` runs without the interactive prompt. `--systemd` prints a Linux
unit template using `rnphone --service -vvv`.

## Config

On first run, rnphone creates a default `config` file and installs bundled
`ringer.opus` and `soft.opus` assets if they are missing.

```ini
[telephone]
    ringtone = ringer.opus
    # speaker = device name
    # microphone = device name
    # ringer = device name
    # allowed_callers = all
    # allowed_callers = none
    # allowed_callers = phonebook
    # blocked_callers = f3e8c3359b39d36f3baff0a616a73d3e

[phonebook]
    # Mary = f3e8c3359b39d36f3baff0a616a73d3e
    # Rudy = 5d2d14619dfa0ff06278c17347c14331, 241

[hardware]
    # keypad = gpio_4x4
    # display = i2c_lcd1602
```

Phonebook entries can include an optional numeric alias after the identity hash.
Aliases can be dialed from supported keypad hardware.

## Interactive Prompt

When not running as a service, the prompt accepts:

```text
phonebook
redial
identity
desthash
announce
quit
help
```

Entering an identity hash dials it directly. Entering a phonebook name or alias
dials the matching phonebook entry.
