//! Connections, sync, cache, search and the Markdown block tree. No I/O and
//! no runtime: the host feeds it what the daemon sent and does the I/O it
//! asks for, so it binds to iOS and WebAssembly ([spike 07]).
//!
//! [`Sessions`] folds the daemon's events into what every client shows: a
//! timeline per session (messages, replies, tool calls grouped together,
//! questions, notes, files the agent wrote) and the inbox status.
//!
//! [spike 07]: ../../docs/spikes/07-client-core.md

pub mod carry;
pub mod fleet;
pub mod inflight;
pub mod machine;
pub mod timeline;

pub use timeline::{
    Aside, Branch, Call, Entry, Feed, Made, Note, SessionView, Sessions, Version, pr_news,
};
