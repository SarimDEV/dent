//! Tailscale's LocalAPI, for trust by account ([decision 0061],
//! [spike 10]). The CLI is never run on a connection path: it can take
//! seconds, and a killed one has been seen relaunching the app.
//!
//! Linux: tailscaled's Unix socket, open to any local user. macOS
//! (standalone app): `127.0.0.1:<port>`, port from `/Library/Tailscale/ipnport`
//! and the token from `sameuserproof-<port>`, which only admin users can
//! read. Anywhere else, or when either can't be read, account trust is
//! simply unavailable and pairing is offered instead.
//!
//! [decision 0061]: ../../../docs/decisions/0061-trust.md
//! [spike 10]: ../../../docs/spikes/10-tailscale-trust.md

use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Every call is bounded, so a stuck tailscaled never holds a connection up.
const LIMIT: Duration = Duration::from_secs(2);
/// Largest answer read.
const MAX_BODY: u64 = 4 << 20;

/// Tailscale answered, but not with 200.
#[derive(Debug)]
struct Status(String);

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "tailscale answered {}", self.0)
    }
}

impl std::error::Error for Status {}

/// What Tailscale says about this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfNode {
    pub user: u64,
    pub addresses: Vec<IpAddr>,
    /// The MagicDNS name, without the trailing dot.
    pub name: String,
    pub tagged: bool,
}

/// What Tailscale says about a peer address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub user: u64,
    pub tagged: bool,
    pub name: String,
}

#[derive(Deserialize)]
struct StatusJson {
    #[serde(rename = "Self")]
    me: Option<NodeJson>,
}

#[derive(Deserialize)]
struct NodeJson {
    #[serde(rename = "UserID", default)]
    user_id: u64,
    #[serde(rename = "TailscaleIPs", default)]
    ips: Vec<String>,
    #[serde(rename = "DNSName", default)]
    dns: String,
    #[serde(rename = "Tags", default)]
    tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct WhoisJson {
    #[serde(rename = "Node")]
    node: WhoisNode,
}

#[derive(Deserialize)]
struct WhoisNode {
    #[serde(rename = "User", default)]
    user: u64,
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "Tags", default)]
    tags: Option<Vec<String>>,
}

pub async fn status() -> Result<SelfNode> {
    // Without peers: only this node is needed, and a large tailnet's full
    // status is big.
    let body = get("/localapi/v0/status?peers=false").await?;
    let s: StatusJson = serde_json::from_slice(&body).context("status json")?;
    let me = s.me.context("no Self in status")?;
    Ok(SelfNode {
        user: me.user_id,
        addresses: me.ips.iter().filter_map(|a| a.parse().ok()).collect(),
        name: me.dns.trim_end_matches('.').to_string(),
        tagged: me.tags.is_some_and(|t| !t.is_empty()),
    })
}

/// Who owns a peer address. `None` when Tailscale doesn't know it: a
/// loopback forward, a LAN address, anything not a tailnet peer.
pub async fn whois(ip: IpAddr) -> Result<Option<Peer>> {
    // Percent-encoded: `[`, `]` and `:` in a query.
    let addr = match ip {
        IpAddr::V4(v) => format!("{v}%3A1"),
        IpAddr::V6(v) => format!("%5B{}%5D%3A1", v.to_string().replace(':', "%3A")),
    };
    let body = match get(&format!("/localapi/v0/whois?addr={addr}")).await {
        Ok(b) => b,
        Err(e) if e.downcast_ref::<Status>().is_some_and(|s| s.0 == "404") => return Ok(None),
        Err(e) => return Err(e),
    };
    let w: WhoisJson = serde_json::from_slice(&body).context("whois json")?;
    Ok(Some(Peer {
        user: w.node.user,
        tagged: w.node.tags.is_some_and(|t| !t.is_empty()),
        name: w.node.name.trim_end_matches('.').to_string(),
    }))
}

/// Whether `peer` gets trust by account: a user-owned node (never tags)
/// owned by the same user as this machine's own node ([decision 0061]).
pub fn same_account(me: &SelfNode, peer: &Peer) -> bool {
    !me.tagged && !peer.tagged && me.user != 0 && peer.user == me.user
}

async fn get(path: &str) -> Result<Vec<u8>> {
    tokio::time::timeout(LIMIT, get_inner(path))
        .await
        .context("tailscale didn't answer")?
}

async fn get_inner(path: &str) -> Result<Vec<u8>> {
    let (stream, mut body) = request(path).await?;
    // HTTP/1.0: the answer ends when tailscaled closes the connection.
    let left = MAX_BODY.saturating_sub(body.len() as u64);
    stream.take(left).read_to_end(&mut body).await?;
    Ok(body)
}

/// Bit 12, `NotifyPeerChanges`: peers added, changed (online or not
/// included) or removed. Changes to this node and its login come to every
/// watcher. Versions before it ignore the bit and send the whole network
/// map on each change instead.
const WATCH_PEERS: u64 = 1 << 12;

/// Calls `changed` once for each notice from Tailscale that something
/// changed: a peer, this node, its login. Notices aren't read, only
/// counted: their shape changes between versions, and one can be a large
/// network map. Called once as it starts, too, for whatever changed while
/// nobody watched. Returns when the stream ends; `Err` when it never
/// started.
pub async fn watch(mut changed: impl FnMut()) -> Result<()> {
    let path = format!("/localapi/v0/watch-ipn-bus?mask={WATCH_PEERS}");
    let (mut stream, first) = tokio::time::timeout(LIMIT, request(&path))
        .await
        .context("tailscale didn't answer")??;
    changed();
    // Each notice is one line of JSON.
    let mut notices = |b: &[u8]| {
        for _ in b.iter().filter(|c| **c == b'\n') {
            changed();
        }
    };
    notices(&first);
    let mut buf = vec![0u8; 64 << 10];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return Ok(()),
            Ok(n) => notices(&buf[..n]),
        }
    }
}

/// A LocalAPI request with its answer's headers read: the stream, and any
/// of the body read with them. An answer other than 200 is an error.
async fn request(path: &str) -> Result<(Box<dyn Stream>, Vec<u8>)> {
    let (mut stream, auth): (Box<dyn Stream>, Option<String>) = open().await?;
    let mut req = format!(
        "GET {path} HTTP/1.0\r\nHost: local-tailscaled.sock\r\nSec-Tailscale: localapi\r\n"
    );
    if let Some(token) = auth {
        req.push_str(&format!(
            "Authorization: Basic {}\r\n",
            dent_protocol::b64::encode(format!(":{token}").as_bytes())
        ));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).await?;
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    let split = loop {
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if raw.len() > 64 << 10 {
            bail!("tailscale's headers never ended");
        }
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            bail!("no end of headers");
        }
        raw.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&raw[..split]);
    let status = head.split_whitespace().nth(1).unwrap_or("");
    if status != "200" {
        return Err(Status(status.to_string()).into());
    }
    let body = raw.split_off(split + 4);
    Ok((stream, body))
}

trait Stream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Stream for T {}

async fn open() -> Result<(Box<dyn Stream>, Option<String>)> {
    if let Ok(port) = std::env::var("DENT_TAILSCALE_LOCALAPI") {
        // Tests point this at a stand-in: "host:port[,token]".
        let (addr, token) = match port.split_once(',') {
            Some((a, t)) => (a.to_string(), Some(t.to_string())),
            None => (port, None),
        };
        return Ok((Box::new(tokio::net::TcpStream::connect(addr).await?), token));
    }
    if cfg!(target_os = "macos") {
        let port = std::fs::read_link("/Library/Tailscale/ipnport")
            .context("no standalone Tailscale here")?
            .display()
            .to_string();
        let token = std::fs::read_to_string(format!("/Library/Tailscale/sameuserproof-{port}"))
            .context("can't read Tailscale's token (only admin users can)")?;
        let s = tokio::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>()?)).await?;
        return Ok((Box::new(s), Some(token.trim().to_string())));
    }
    for sock in [
        "/var/run/tailscale/tailscaled.sock",
        "/run/tailscale/tailscaled.sock",
    ] {
        if let Ok(s) = tokio::net::UnixStream::connect(sock).await {
            return Ok((Box::new(s), None));
        }
    }
    bail!("no tailscaled socket")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me(user: u64, tagged: bool) -> SelfNode {
        SelfNode {
            user,
            addresses: vec![],
            name: "studio".into(),
            tagged,
        }
    }

    fn peer(user: u64, tagged: bool) -> Peer {
        Peer {
            user,
            tagged,
            name: "p".into(),
        }
    }

    #[test]
    fn account_trust_needs_one_real_user_and_no_tags() {
        assert!(same_account(&me(7, false), &peer(7, false)));
        assert!(
            !same_account(&me(7, false), &peer(8, false)),
            "another account"
        );
        assert!(!same_account(&me(7, false), &peer(7, true)), "tagged peer");
        assert!(
            !same_account(&me(7, true), &peer(7, true)),
            "two tagged nodes"
        );
        assert!(
            !same_account(&me(0, false), &peer(0, false)),
            "no user known"
        );
    }

    #[test]
    fn parses_tailscale_shapes() {
        let s: StatusJson = serde_json::from_str(
            r#"{"Self":{"UserID":4132,"TailscaleIPs":["100.70.46.115","fd7a:115c:a1e0::1"],"DNSName":"dent-linux.tail1.ts.net.","Tags":null}}"#,
        )
        .unwrap();
        let n = s.me.unwrap();
        assert_eq!(n.user_id, 4132);
        assert_eq!(n.ips.len(), 2);
        let w: WhoisJson =
            serde_json::from_str(r#"{"Node":{"User":4132,"Name":"mac.tail1.ts.net.","Tags":["tag:ci"]},"UserProfile":{}}"#)
                .unwrap();
        assert!(w.node.tags.is_some_and(|t| !t.is_empty()));
    }
}
