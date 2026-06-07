# lxst-rs

Rust port of LXST, the Lightweight Extensible Signal Transport protocol and
real-time audio toolkit for Reticulum.

This repository starts from the Python LXST public mirror at:

```text
https://github.com/markqvist/lxst
```

Initial local source checkout reviewed for this scaffold:

```text
/home/lelloman/lxst
LXST version: 0.4.5
HEAD: 1194c90
```

## Scope

The port should separate protocol/domain concerns from application plumbing:

- `lxst-core`: frame metadata, codec/profile identifiers, signalling values, and
  transport-neutral protocol types.
- `lxst`: higher-level API surface that will eventually connect `lxst-core` to
  Reticulum networking, audio pipelines, codecs, and telephony primitives.

The Python project is early alpha and explicitly API-unstable, so this port
should track behavior and wire format deliberately instead of copying incidental
Python APIs.
