//! Spike 07: the client core with no I/O.
//!
//! Everything a client needs to talk to a daemon (the encrypted channel,
//! pairing, reconnecting, following sessions, turning replies into Markdown
//! blocks) lives here as plain state machines. The host owns the sockets and
//! the clock: it opens the WebSocket the core asks for, hands it the bytes
//! that arrive and calls `tick` with the time. So the same code runs in the
//! desktop app, behind UniFFI on iOS and behind wasm-bindgen in a browser.
//!
//! Nothing here reads the clock, the network or the disk. `SystemTime::now`
//! panics on `wasm32-unknown-unknown`, so time always comes in as an
//! argument.

#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

pub mod blocks;
pub mod chat;
pub mod keys;
pub mod link;
pub mod noise;
pub mod pair;
pub mod proto;
pub mod push;
pub mod stream;

pub use blocks::{Block, BlockKind, Span};
pub use chat::{MessageUpdate, Role, Turn};
pub use keys::Keys;
pub use link::{Io, Link, Note, PairError, Stats, Status};
