//! Remote machines for dent clients on tokio ([remote-plan.md], stage 1):
//! [`host`] does the I/O the connection actor in `dent-client-core` asks
//! for, [`remote`] runs a daemon session over it, and [`store`] keeps this
//! device's key and the machines it saved, and [`history`] each machine's
//! event log, kept here.
//!
//! [remote-plan.md]: ../../docs/remote-plan.md

pub mod history;
pub mod host;
pub mod remote;
pub mod store;
pub mod wake;

pub use remote::{Event, Remote, spawn};
pub use store::Saved;
