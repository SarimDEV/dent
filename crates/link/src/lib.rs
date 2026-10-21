//! The link between devices and daemons ([decision 0060]): a Noise XX
//! handshake between device keys, a framed transport with a control lane,
//! and pairing with a short code. No I/O: callers move the bytes.
//!
//! [decision 0060]: ../../docs/decisions/0060-remote-wire.md

pub mod control;
pub mod keys;
pub mod noise;
pub mod pair;

pub use control::{Access, Control, Goodbye, Refusal, Trust};
pub use keys::{Keys, fingerprint};
pub use noise::{Answered, Hello, Initiator, Opened, Responder, Transport, WrongKey};

/// Largest message before the other side is trusted: only the hello,
/// control and pairing travel then, and a stranger must not make us buffer
/// more.
pub const SMALL: usize = 256 << 10;
/// Largest message once trusted.
pub const LARGE: usize = 64 << 20;
