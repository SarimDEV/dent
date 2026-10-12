//! The client core for iOS. UniFFI wants objects shared across threads, so
//! the link sits behind a mutex; every call is short and never blocks on
//! I/O, because the core does none. Swift owns the sockets and the timer
//! (see ios/Dent/Core.swift), which keeps async and cancellation on the
//! Swift side where the app's lifecycle lives.

use std::sync::{Arc, Mutex, MutexGuard};

use spike_core::link::Stats;
use spike_core::push::Notice;
use spike_core::{Block, Io, Keys, Link, Note, Status};

uniffi::setup_scaffolding!();

#[derive(Debug, uniffi::Error)]
pub enum CoreError {
    Invalid { why: String },
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            CoreError::Invalid { why } => f.write_str(why),
        }
    }
}

impl std::error::Error for CoreError {}

fn invalid(why: impl ToString) -> CoreError {
    CoreError::Invalid {
        why: why.to_string(),
    }
}

#[derive(uniffi::Object)]
pub struct CoreLink {
    inner: Mutex<Link>,
}

impl CoreLink {
    fn link(&self) -> MutexGuard<'_, Link> {
        // A panic inside the core poisons the lock; the state is still
        // consistent enough to keep reading notes from.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[uniffi::export]
impl CoreLink {
    /// `keys` from `generate_keys`; `known` is the machine's key (hex) once
    /// paired.
    #[uniffi::constructor]
    pub fn new(
        name: String,
        keys: Vec<u8>,
        known: Option<String>,
        addrs: Vec<String>,
    ) -> Result<Arc<Self>, CoreError> {
        let keys = Keys::from_bytes(&keys).ok_or_else(|| invalid("keys must be 64 bytes"))?;
        let known = known.map(hex::decode).transpose().map_err(invalid)?;
        Ok(Arc::new(Self {
            inner: Mutex::new(Link::new(&name, keys, known, addrs)),
        }))
    }

    pub fn start(&self, now: u64) {
        self.link().start(now)
    }
    pub fn stop(&self) {
        self.link().stop()
    }
    pub fn set_addrs(&self, addrs: Vec<String>) {
        self.link().set_addrs(addrs)
    }
    pub fn pair(&self, code: String, now: u64) {
        self.link().pair(&code, now)
    }
    pub fn follow(&self, session: String) {
        self.link().follow(&session)
    }
    pub fn prompt(&self, session: String, text: String) -> String {
        self.link().prompt(&session, &text)
    }
    pub fn set_push(&self, handle: String, key_hex: String) {
        self.link().set_push(&handle, &key_hex)
    }
    pub fn bulk(&self, session: String, n: u64) {
        self.link().bulk(&session, n)
    }
    pub fn resend(&self, session: String) {
        self.link().resend(&session)
    }
    pub fn opened(&self, conn: u64, now: u64) {
        self.link().opened(conn, now)
    }
    pub fn received(&self, conn: u64, data: Vec<u8>, now: u64) {
        self.link().received(conn, &data, now)
    }
    pub fn closed(&self, conn: u64, reason: String, now: u64) {
        self.link().closed(conn, &reason, now)
    }
    pub fn tick(&self, now: u64) {
        self.link().tick(now)
    }
    pub fn take_io(&self) -> Vec<Io> {
        self.link().take_io()
    }
    pub fn take_notes(&self) -> Vec<Note> {
        self.link().take_notes()
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.link().next_deadline()
    }
    pub fn status(&self) -> Status {
        self.link().status()
    }
    pub fn known_key(&self) -> Option<String> {
        self.link().known_key()
    }
    pub fn stats(&self) -> Stats {
        self.link().stats()
    }
}

/// A new device key pair, 64 bytes, for the Keychain.
#[uniffi::export]
pub fn generate_keys() -> Vec<u8> {
    Keys::generate().to_bytes()
}

#[uniffi::export]
pub fn device_id(keys: Vec<u8>) -> String {
    Keys::from_bytes(&keys).map(|k| k.id()).unwrap_or_default()
}

/// A key for a phone's notifications, given to each machine it pairs with.
#[uniffi::export]
pub fn new_push_key() -> Vec<u8> {
    spike_core::push::new_key()
}

/// Opens a sealed notice (the extension uses CryptoKit; this is for checks).
#[uniffi::export]
pub fn open_notice(key: Vec<u8>, sealed: Vec<u8>) -> Result<Notice, CoreError> {
    spike_core::push::open(&key, &sealed).map_err(invalid)
}

/// Parse a whole text into blocks (benchmarks).
#[uniffi::export]
pub fn parse_markdown(text: String) -> Vec<Block> {
    spike_core::blocks::parse(&text)
}
