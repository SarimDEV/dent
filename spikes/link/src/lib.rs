//! Spike 05 as a library: the pieces a client core and a daemon need to
//! find, pair with and stream from each other directly.

pub mod channel;
pub mod client;
pub mod daemon;
pub mod devices;
pub mod discovery;
pub mod harness;
pub mod identity;
pub mod pair;
pub mod proto;
pub mod proxy;
pub mod tailnet;
