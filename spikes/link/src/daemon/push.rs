//! Phone notifications through dent-push. A paired phone gives the daemon
//! its push handle and a key; when a reply finishes, the daemon seals a
//! short notice with that key and posts it to dent-push, which forwards it
//! to Apple without being able to read it.
//!
//! Whether notifications are going out is part of the machine's health, so
//! a phone over its limit, or dent-push being down, shows in the apps
//! instead of failing silently.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::identity::write_atomic;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Phone {
    handle: String,
    key: String,
}

struct Inner {
    url: Option<String>,
    path: PathBuf,
    phones: Mutex<BTreeMap<String, Phone>>,
    state: Mutex<Option<String>>,
}

#[derive(Clone)]
pub struct Push(Arc<Inner>);

impl Push {
    pub fn new(url: Option<String>, state: &std::path::Path) -> Self {
        let path = state.join("push.json");
        let phones = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self(Arc::new(Inner {
            url,
            path,
            phones: Mutex::new(phones),
            state: Mutex::new(None),
        }))
    }

    /// A paired device's handle and key. Replaces any it sent before.
    pub fn register(&self, device: &str, handle: String, key: String) {
        if hex::decode(&key).map(|k| k.len()) != Ok(32) || handle.len() > 4096 {
            eprintln!("push: ignoring a malformed registration from {device}");
            return;
        }
        self.change(|p| {
            p.insert(device.into(), Phone { handle, key });
        });
    }

    fn change(&self, f: impl FnOnce(&mut BTreeMap<String, Phone>)) {
        if let Ok(mut p) = self.0.phones.lock() {
            f(&mut p);
            if let Ok(json) = serde_json::to_vec_pretty(&*p) {
                let _ = write_atomic(&self.0.path, &json);
            }
        }
    }

    /// None while notifications go out; otherwise why not.
    pub fn state(&self) -> Option<String> {
        self.0.state.lock().ok().and_then(|s| s.clone())
    }

    fn set_state(&self, s: Option<String>) {
        if let Ok(mut st) = self.0.state.lock() {
            *st = s;
        }
    }

    /// Seal `notice` for every registered phone and send it, in the
    /// background.
    pub fn send(&self, kind: &str, collapse: &str, notice: spike_core::push::Notice) {
        let Some(url) = self.0.url.clone() else {
            return;
        };
        let phones: Vec<(String, Phone)> = self
            .0
            .phones
            .lock()
            .map(|p| p.iter().map(|(d, p)| (d.clone(), p.clone())).collect())
            .unwrap_or_default();
        let me = self.clone();
        let (kind, collapse) = (kind.to_string(), collapse.to_string());
        tokio::spawn(async move {
            for (device, phone) in phones {
                let Ok(key) = hex::decode(&phone.key) else {
                    continue;
                };
                let Ok(sealed) = spike_core::push::seal(&key, &notice) else {
                    continue;
                };
                let body = serde_json::json!({
                    "handle": phone.handle,
                    "kind": kind,
                    "payload": base64::engine::general_purpose::STANDARD.encode(sealed),
                    "collapse": collapse,
                });
                let r = tokio::time::timeout(
                    Duration::from_secs(5),
                    post(&url, "/send", body.to_string().as_bytes()),
                )
                .await;
                match r {
                    Ok(Ok((200, _))) => me.set_state(None),
                    Ok(Ok((429, body))) => {
                        let until = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v["until"].as_u64())
                            .unwrap_or(0);
                        me.set_state(Some(format!("limited:{until}")));
                    }
                    // The phone is gone (app removed, token expired).
                    Ok(Ok((410, _))) => me.change(|p| {
                        p.remove(&device);
                    }),
                    Ok(Ok((status, body))) => {
                        eprintln!("push: dent-push answered {status}: {body}");
                        me.set_state(Some("unavailable".into()));
                    }
                    Ok(Err(e)) => {
                        eprintln!("push: {e:#}");
                        me.set_state(Some("unavailable".into()));
                    }
                    Err(_) => me.set_state(Some("unavailable".into())),
                }
            }
        });
    }
}

/// The first readable line of a reply, without Markdown marks, for the
/// notification body.
pub fn summary(text: &str) -> String {
    let line = text
        .lines()
        .map(|l| l.trim_start_matches(['#', '>', '-', '*', ' ']).trim())
        .find(|l| !l.is_empty() && !l.starts_with("```"))
        .unwrap_or("");
    let plain: String = line.chars().filter(|c| !"*_`".contains(*c)).collect();
    match plain.char_indices().nth(120) {
        Some((i, _)) => format!("{}…", &plain[..i]),
        None => plain,
    }
}

/// A plain HTTP/1.1 POST, enough for dent-push running locally under
/// wrangler. The real daemon uses an HTTPS client.
async fn post(url: &str, path: &str, body: &[u8]) -> Result<(u16, String)> {
    let rest = url
        .strip_prefix("http://")
        .context("the spike only posts to http:// URLs")?;
    let (host, base) = match rest.split_once('/') {
        Some((h, p)) => (h, format!("/{}", p.trim_end_matches('/'))),
        None => (rest, String::new()),
    };
    let mut s = tokio::net::TcpStream::connect(host).await?;
    let head = format!(
        "POST {base}{path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).await?;
    s.write_all(body).await?;
    // Read the head, then exactly the body it announces: a server may keep
    // the connection open whatever we asked.
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head, mut rest) = loop {
        let n = s.read(&mut chunk).await?;
        anyhow::ensure!(n > 0, "closed before answering");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break (
                String::from_utf8_lossy(&buf[..i]).into_owned(),
                buf[i + 4..].to_vec(),
            );
        }
        anyhow::ensure!(buf.len() < 64 << 10, "head too large");
    };
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .context("bad status line")?;
    let lower = head.to_ascii_lowercase();
    let length = lower
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok());
    let chunked = lower.contains("transfer-encoding: chunked");
    loop {
        let done = match length {
            Some(len) => rest.len() >= len,
            None if chunked => rest.windows(5).any(|w| w == b"0\r\n\r\n"),
            None => false,
        };
        if done || rest.len() > 1 << 20 {
            break;
        }
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        rest.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&rest).into_owned();
    let body = if chunked { dechunk(&text) } else { text };
    Ok((status, body))
}

fn dechunk(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = s.split_once("\r\n") {
        let Ok(n) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if n == 0 || rest.len() < n {
            break;
        }
        out.push_str(&rest[..n]);
        s = rest[n..].trim_start_matches("\r\n");
    }
    out
}
