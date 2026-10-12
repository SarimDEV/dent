//! The client core in the browser (decision 0024: a separate wasm-bindgen
//! facade over the same core). JavaScript owns the WebSocket and the timer;
//! notes cross as one JSON string per call, which is cheaper than building
//! JS objects field by field from Rust. Bytes cross as `Uint8Array`.

use js_sys::{Array, Object, Reflect, Uint8Array};
use spike_core::{Io, Keys, Link};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WebLink {
    inner: Link,
}

fn set(o: &Object, k: &str, v: &JsValue) {
    let _ = Reflect::set(o, &JsValue::from_str(k), v);
}

#[wasm_bindgen]
impl WebLink {
    #[wasm_bindgen(constructor)]
    pub fn new(
        name: &str,
        keys: &[u8],
        known: Option<String>,
        addrs: Vec<String>,
    ) -> Result<WebLink, JsError> {
        let keys = Keys::from_bytes(keys).ok_or_else(|| JsError::new("keys must be 64 bytes"))?;
        let known = known
            .map(hex::decode)
            .transpose()
            .map_err(|e| JsError::new(&e.to_string()))?;
        Ok(Self {
            inner: Link::new(name, keys, known, addrs),
        })
    }

    pub fn start(&mut self, now: f64) {
        self.inner.start(now as u64)
    }
    pub fn stop(&mut self) {
        self.inner.stop()
    }
    pub fn pair(&mut self, code: &str, now: f64) {
        self.inner.pair(code, now as u64)
    }
    pub fn follow(&mut self, session: &str) {
        self.inner.follow(session)
    }
    pub fn prompt(&mut self, session: &str, text: &str) -> String {
        self.inner.prompt(session, text)
    }
    pub fn set_push(&mut self, handle: &str, key_hex: &str) {
        self.inner.set_push(handle, key_hex)
    }
    pub fn bulk(&mut self, session: &str, n: f64) {
        self.inner.bulk(session, n as u64)
    }
    pub fn resend(&mut self, session: &str) {
        self.inner.resend(session)
    }
    pub fn opened(&mut self, conn: f64, now: f64) {
        self.inner.opened(conn as u64, now as u64)
    }
    pub fn received(&mut self, conn: f64, data: &[u8], now: f64) {
        self.inner.received(conn as u64, data, now as u64)
    }
    pub fn closed(&mut self, conn: f64, reason: &str, now: f64) {
        self.inner.closed(conn as u64, reason, now as u64)
    }
    pub fn tick(&mut self, now: f64) {
        self.inner.tick(now as u64)
    }

    /// `[{type: "connect", conn, addr} | {type: "send", conn, data} |
    /// {type: "close", conn}]`
    pub fn take_io(&mut self) -> Array {
        let out = Array::new();
        for io in self.inner.take_io() {
            let o = Object::new();
            match io {
                Io::Connect { conn, addr } => {
                    set(&o, "type", &"connect".into());
                    set(&o, "conn", &(conn as f64).into());
                    set(&o, "addr", &addr.into());
                }
                Io::Send { conn, data } => {
                    set(&o, "type", &"send".into());
                    set(&o, "conn", &(conn as f64).into());
                    set(&o, "data", &Uint8Array::from(&data[..]).into());
                }
                Io::Close { conn } => {
                    set(&o, "type", &"close".into());
                    set(&o, "conn", &(conn as f64).into());
                }
            }
            out.push(&o);
        }
        out
    }

    /// Notes as a JSON array.
    pub fn take_notes(&mut self) -> String {
        serde_json::to_string(&self.inner.take_notes()).unwrap_or_else(|_| "[]".into())
    }

    pub fn next_deadline(&self) -> Option<f64> {
        self.inner.next_deadline().map(|d| d as f64)
    }

    pub fn known_key(&self) -> Option<String> {
        self.inner.known_key()
    }

    /// Counters as JSON.
    pub fn stats(&self) -> String {
        serde_json::to_string(&self.inner.stats()).unwrap_or_default()
    }
}

#[wasm_bindgen]
pub fn generate_keys() -> Vec<u8> {
    Keys::generate().to_bytes()
}

#[wasm_bindgen]
pub fn device_id(keys: &[u8]) -> String {
    Keys::from_bytes(keys).map(|k| k.id()).unwrap_or_default()
}

/// Parse a whole text into blocks, as JSON (benchmarks).
#[wasm_bindgen]
pub fn parse_markdown(text: &str) -> String {
    serde_json::to_string(&spike_core::blocks::parse(text)).unwrap_or_default()
}
