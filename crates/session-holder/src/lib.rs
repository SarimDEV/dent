//! The session holder: a tiny process that owns agent processes, so they keep
//! running while the daemon restarts or upgrades ([decision 0004]).
//!
//! The daemon starts it on first use and talks to it over a Unix socket in a
//! private directory. [`wire`] is the whole protocol.
//!
//! [decision 0004]: ../../docs/decisions/0004-session-holder.md

pub mod holder;
pub mod wire;
