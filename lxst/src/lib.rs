//! High-level LXST Rust API.

pub use lxst_core as core;

pub use lxst_core::{CallProfile, CodecKind, CodecProfile, FrameDuration, Signal, SignalCode};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("operation is not implemented yet")]
    NotImplemented,
}
