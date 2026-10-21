//! Devices on other machines ([remote-plan.md], stage 1): an encrypted
//! WebSocket on every address, filtered by route and trust.
//!
//! Each connection is Noise XX between device keys ([`dent_link`]). The
//! daemon then decides trust ([decision 0061]): paired with a code, or the
//! same Tailscale account on a Tailscale route, or not at all, when only
//! pairing and pings are allowed. A trusted connection carries the same
//! requests and events as the local socket, one JSON line per data
//! message, with control messages (pings, goodbyes, refusals) on their own
//! lane between them.
//!
//! [remote-plan.md]: ../../../docs/remote-plan.md
//! [decision 0061]: ../../../docs/decisions/0061-trust.md

pub mod addrs;
pub mod identity;
pub mod tailnet;
pub mod trust;

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use dent_link::{
    Access, Control, Goodbye, Hello, Keys, Opened, Refusal, Responder, Transport, Trust as Verdict,
};
use dent_protocol::{PROTOCOL, PROTOCOL_MIN};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;

use crate::daemon::Daemon;
use addrs::Route;
use identity::Identity;
use trust::{Trust, Window};

/// The default port, kept across restarts so learned routes stay good.
pub const PORT: u16 = 7480;
/// From accepting a socket to knowing who it is and whether it's trusted.
const HANDSHAKE: Duration = Duration::from_secs(5);
/// A connection is dropped after this long without a frame. Twice the
/// client's own 15 s, so the client decides when a link is dead
/// ([decision 0062], [spike 12]).
///
/// [decision 0062]: ../../../docs/decisions/0062-liveness-and-retry.md
/// [spike 12]: ../../../docs/spikes/12-link-under-real-networks.md
const IDLE: Duration = Duration::from_secs(30);
/// Connections not yet trusted, per address, and handshakes in progress in
/// all.
const PREAUTH_PER_ADDR: usize = 8;
const PREAUTH_ALL: usize = 64;
/// Untrusted connections waiting to pair, in all. Separate from handshakes
/// in progress, so strangers lingering here can't keep trusted devices from
/// finishing theirs.
const UNTRUSTED_ALL: usize = 32;
/// The longest an untrusted connection stays, pings or not: as long as a
/// code lasts.
const UNTRUSTED: Duration = trust::CODE_TTL;
/// Longest wait to hand one frame to a remote client before it's dropped,
/// as for the local socket.
const WRITE_LIMIT: Duration = Duration::from_secs(30);
/// Control messages waiting to go to one remote client. A client sending
/// pings faster than it reads pongs loses pongs, not memory.
const CTL_QUEUE: usize = 64;
/// How often trust through the Tailscale account is checked again on a
/// live connection ([decision 0061]).
const RECHECK: Duration = Duration::from_secs(300);
/// Lines waiting to go to one remote client.
const OUT_QUEUE: usize = 256;
/// How long Tailscale's view of this machine is reused.
const TAILSCALE_TTL: Duration = Duration::from_secs(30);

/// This machine's network side: identity, trust and pairing.
pub struct Net {
    pub identity: Option<Identity>,
    /// Why there's no identity, when there isn't.
    pub problem: Option<String>,
    /// The event log's epoch ([decision 0059]).
    ///
    /// [decision 0059]: ../../../docs/decisions/0059-machine-identity.md
    pub epoch: String,
    pub port: u16,
    trust: Mutex<Trust>,
    window: Mutex<Window>,
    lan: AtomicBool,
    tailscale: tokio::sync::Mutex<Option<(Option<tailnet::SelfNode>, Instant)>>,
    /// Device keys trusted through Tailscale lately, so a LAN attempt from
    /// one can be told "trusted on Tailscale" rather than "pair".
    via_tailscale: Mutex<HashSet<Vec<u8>>>,
    /// Device ids removed while connected: their connections end.
    removed: broadcast::Sender<String>,
    /// Goodbyes for every remote connection, as the daemon goes away.
    pub bye: broadcast::Sender<Goodbye>,
    preauth: Mutex<HashMap<IpAddr, usize>>,
    preauth_all: Arc<Semaphore>,
    untrusted: Arc<Semaphore>,
}

impl Net {
    pub fn new(paths: &crate::paths::Paths, store: &crate::store::Store) -> Self {
        let (identity, problem) = match identity::load(&paths.state) {
            Ok(i) => (Some(i), None),
            Err(p) => (None, Some(p.to_string())),
        };
        let epoch = match store.get("log_epoch") {
            Ok(Some(e)) => e,
            Ok(None) => {
                let e = crate::daemon::new_id();
                let _ = store.put("log_epoch", &e);
                e
            }
            // Unreadable just now: an epoch for this run only, never one
            // written over the log's own.
            Err(_) => crate::daemon::new_id(),
        };
        let port = std::env::var("DENT_REMOTE_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(PORT);
        let lan = std::env::var("DENT_REMOTE_LAN").is_ok_and(|v| v == "1")
            || store
                .get("remote.lan")
                .ok()
                .flatten()
                .is_some_and(|v| v == "on");
        Self {
            identity,
            problem,
            epoch,
            port,
            trust: Mutex::new(Trust::load(&paths.state)),
            window: Mutex::new(Window::default()),
            lan: AtomicBool::new(lan),
            tailscale: tokio::sync::Mutex::new(None),
            via_tailscale: Mutex::new(HashSet::new()),
            removed: broadcast::channel(16).0,
            bye: broadcast::channel(4).0,
            preauth: Mutex::new(HashMap::new()),
            preauth_all: Arc::new(Semaphore::new(PREAUTH_ALL)),
            untrusted: Arc::new(Semaphore::new(UNTRUSTED_ALL)),
        }
    }

    pub fn id(&self) -> String {
        self.identity
            .as_ref()
            .map(|i| i.keys.id())
            .unwrap_or_default()
    }

    pub fn open_pairing(&self) -> Result<(String, u64)> {
        lock(&self.window).open()
    }

    pub fn devices(&self) -> Vec<dent_protocol::Device> {
        lock(&self.trust)
            .list()
            .into_iter()
            .map(|(id, p)| dent_protocol::Device {
                id,
                name: p.name,
                how: "paired".into(),
                access: p.access,
                added: p.added,
                seen: p.seen,
            })
            .collect()
    }

    pub fn remove_device(&self, id: &str) -> Result<bool> {
        let had = lock(&self.trust).remove(id)?;
        let _ = self.removed.send(id.to_string());
        Ok(had)
    }

    /// This machine as Tailscale sees it, cached briefly.
    pub async fn tailscale(&self) -> Option<tailnet::SelfNode> {
        let mut c = self.tailscale.lock().await;
        if let Some((node, at)) = c.as_ref()
            && at.elapsed() < TAILSCALE_TTL
        {
            return node.clone();
        }
        let node = tailnet::status().await.ok();
        *c = Some((node.clone(), Instant::now()));
        node
    }

    /// Where devices can reach this machine, best first: LAN addresses
    /// (when LAN connections are on), then Tailscale's name and addresses.
    pub async fn addresses(&self) -> Vec<String> {
        let mut out = Vec::new();
        let fmt = |ip: IpAddr, port: u16| match ip {
            IpAddr::V4(v) => format!("{v}:{port}"),
            IpAddr::V6(v) => format!("[{v}]:{port}"),
        };
        if self.lan.load(Ordering::Relaxed) {
            out.extend(
                addrs::lan_addresses()
                    .into_iter()
                    .map(|ip| fmt(ip, self.port)),
            );
        }
        if let Some(ts) = self.tailscale().await {
            if !ts.name.is_empty() {
                out.push(format!("{}:{}", ts.name, self.port));
            }
            out.extend(ts.addresses.iter().map(|ip| fmt(*ip, self.port)));
        }
        out
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Listens for remote devices until the daemon stops. Without a usable
/// identity it says why and leaves remote access off; local clients are
/// unaffected.
pub async fn serve(d: Arc<Daemon>) -> Result<()> {
    let Some(identity) = d.net.identity.as_ref() else {
        eprintln!(
            "dent-daemon: remote access off: {}",
            d.net.problem.as_deref().unwrap_or("no identity")
        );
        return Ok(());
    };
    if d.net.port == 0 {
        return Ok(());
    }
    let keys = Arc::new(identity.keys.clone());
    // Every address, then filtered by route ([remote-plan.md]): binding an
    // interface address at startup fails when that interface (Tailscale)
    // comes up later ([spike 14]).
    let listener = listen(d.net.port).with_context(|| format!("listen on port {}", d.net.port))?;
    eprintln!(
        "dent-daemon: remote access on port {} as {}",
        d.net.port,
        identity.keys.id()
    );
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(a) => a,
            // Out of descriptors, or a connection reset before it was
            // taken: wait a moment rather than stop listening for good.
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let ip = addrs::unmap(peer.ip());
        // Caps on handshakes in progress, per address and in all, so a
        // flood of silent connections can't crowd anyone out.
        let Ok(permit) = d.net.preauth_all.clone().try_acquire_owned() else {
            continue;
        };
        {
            let mut p = lock(&d.net.preauth);
            let n = p.entry(ip).or_insert(0);
            if *n >= PREAUTH_PER_ADDR {
                continue;
            }
            *n += 1;
        }
        let (d, keys) = (d.clone(), keys.clone());
        tokio::spawn(async move {
            let _ = conn(d, keys, stream, peer, permit).await;
        });
    }
}

/// A listener on every address, IPv6 and IPv4 alike. Dual-stack is asked
/// for rather than assumed: some systems make IPv6 sockets IPv6-only by
/// default, which would leave IPv4 devices out. Without IPv6, IPv4 alone.
fn listen(port: u16) -> std::io::Result<TcpListener> {
    use std::os::fd::AsRawFd;
    let dual = (|| {
        let s = tokio::net::TcpSocket::new_v6()?;
        s.set_reuseaddr(true)?;
        let off: libc::c_int = 0;
        // SAFETY: a valid socket, and an int option passed with its size.
        let r = unsafe {
            libc::setsockopt(
                s.as_raw_fd(),
                libc::IPPROTO_IPV6,
                libc::IPV6_V6ONLY,
                (&off as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        s.bind(SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)))?;
        s.listen(1024)
    })();
    match dual {
        Ok(l) => Ok(l),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Err(e),
        Err(_) => {
            let s = tokio::net::TcpSocket::new_v4()?;
            s.set_reuseaddr(true)?;
            s.bind(SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, port)))?;
            s.listen(1024)
        }
    }
}

/// One connection's place under the caps: a handshake in progress (in all)
/// until [`Slot::handshaken`], and a connection not yet trusted (per
/// address) until [`Slot::release`]. Whatever is still held is given back
/// when dropped.
struct Slot<'a> {
    net: &'a Net,
    ip: IpAddr,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    counted: bool,
}

impl Slot<'_> {
    fn handshaken(&mut self) {
        self.permit = None;
    }

    fn release(&mut self) {
        self.permit = None;
        if std::mem::take(&mut self.counted) {
            let mut p = lock(&self.net.preauth);
            if let Some(n) = p.get_mut(&self.ip) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    p.remove(&self.ip);
                }
            }
        }
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

type Ws = tokio_tungstenite::WebSocketStream<TcpStream>;

fn ws_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    // One WebSocket message is one Noise message, so nothing bigger is valid.
    tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_frame_size(Some(65535 + 64))
        .max_message_size(Some(65535 + 64))
}

async fn recv_bin(ws: &mut Ws) -> Result<Vec<u8>> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Binary(b))) => return Ok(b.to_vec()),
            Some(Ok(Message::Close(_))) | None => bail!("closed"),
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(e.into()),
        }
    }
}

/// Seals and sends one control message. Before trust only: afterwards the
/// writer task is the only sealer.
async fn send_control(ws: &mut Ws, t: &mut Transport, c: &Control) -> Result<()> {
    let f = t.seal_control(c)?;
    tokio::time::timeout(WRITE_LIMIT, ws.send(Message::Binary(f.into())))
        .await
        .context("the device isn't reading")??;
    Ok(())
}

async fn conn(
    d: Arc<Daemon>,
    keys: Arc<Keys>,
    stream: TcpStream,
    peer: SocketAddr,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
    let ip = addrs::unmap(peer.ip());
    let mut slot = Slot {
        net: &d.net,
        ip,
        permit: Some(permit),
        counted: true,
    };
    let _ = stream.set_nodelay(true);
    let me = Hello {
        name: d.machine.clone(),
        proto_min: PROTOCOL_MIN,
        proto: PROTOCOL,
        probe: false,
    };
    let setup = tokio::time::timeout(HANDSHAKE, async {
        let mut ws = tokio_tungstenite::accept_async_with_config(stream, Some(ws_config())).await?;
        let m1 = recv_bin(&mut ws).await?;
        let (res, m2) = Responder::accept(&keys, &m1, &me)?;
        ws.send(Message::Binary(m2.into())).await?;
        let m3 = recv_bin(&mut ws).await?;
        let (t, hello) = res.finish(&m3)?;
        anyhow::Ok((ws, t, hello))
    })
    .await
    .context("handshake took too long")??;
    let (mut ws, mut t, hello) = setup;

    if me.agree(&hello).is_none() {
        let _ = send_control(
            &mut ws,
            &mut t,
            &Control::Refuse(Refusal::Incompatible {
                daemon: (PROTOCOL_MIN, PROTOCOL),
                client: (hello.proto_min, hello.proto),
            }),
        )
        .await;
        return Ok(());
    }

    // Which route, as Tailscale itself confirms: a 100.x address alone
    // proves nothing ([decision 0061]).
    let ts_peer = if !ip.is_loopback() {
        tailnet::whois(ip).await.ok().flatten()
    } else {
        None
    };
    let me_ts = match ts_peer {
        Some(_) => d.net.tailscale().await,
        None => None,
    };
    // This machine's own Tailscale address is this machine, like loopback:
    // any local process can connect from it, and it's no peer.
    let from_self = me_ts.as_ref().is_some_and(|m| m.addresses.contains(&ip));
    let route = if from_self {
        Route::Loopback
    } else {
        addrs::route(ip, ts_peer.is_some())
    };
    let paired = lock(&d.net.trust).get(&t.remote).cloned();
    // With LAN access off, an unpaired device gets in only over Tailscale or
    // loopback. "Other" is held to the same rule: a LAN device reaching us
    // by its global or link-local IPv6 address lands there, and so does the
    // open internet.
    if matches!(route, Route::Lan | Route::Other)
        && !d.net.lan.load(Ordering::Relaxed)
        && paired.is_none()
    {
        let _ = send_control(&mut ws, &mut t, &Control::Refuse(Refusal::LanOff)).await;
        return Ok(());
    }

    let pairing_open = lock(&d.net.window).is_open();
    let mut verdict = match &paired {
        Some(p) => Verdict::Paired {
            access: access(&p.access),
        },
        None => Verdict::Untrusted { pairing_open },
    };
    if matches!(verdict, Verdict::Untrusted { .. })
        && route == Route::Tailscale
        && let (Some(me_ts), Some(p)) = (me_ts.as_ref(), ts_peer.as_ref())
        && tailnet::same_account(me_ts, p)
    {
        verdict = Verdict::Tailscale {
            access: Access::Full,
        };
        lock(&d.net.via_tailscale).insert(t.remote.clone());
    }
    // Trusted through the Tailscale account but reached another way: say
    // so, unless a code is showing here, when it may pair to use this route
    // too ([decision 0061]).
    let seen_on_tailscale = lock(&d.net.via_tailscale).contains(&t.remote);
    if matches!(verdict, Verdict::Untrusted { .. })
        && route != Route::Tailscale
        && seen_on_tailscale
        && !pairing_open
    {
        let _ = send_control(
            &mut ws,
            &mut t,
            &Control::Refuse(Refusal::TrustedOn {
                routes: vec!["tailscale".into()],
            }),
        )
        .await;
        return Ok(());
    }
    // The handshake is over; from here an untrusted connection counts
    // against its own cap, not the one trusted devices handshake under.
    slot.handshaken();
    let lingering = match verdict {
        Verdict::Untrusted { .. } if !hello.probe => {
            match d.net.untrusted.clone().try_acquire_owned() {
                Ok(p) => Some(p),
                Err(_) => {
                    let _ = send_control(
                        &mut ws,
                        &mut t,
                        &Control::Refuse(Refusal::Busy { retry_ms: 5000 }),
                    )
                    .await;
                    return Ok(());
                }
            }
        }
        _ => None,
    };
    send_control(&mut ws, &mut t, &Control::Trust(verdict.clone())).await?;
    if hello.probe {
        // Discovery: who's there, and nothing else.
        return Ok(());
    }
    if let Verdict::Untrusted { .. } = verdict {
        let paired = tokio::time::timeout(UNTRUSTED, pairing(&d, &mut ws, &mut t, &hello))
            .await
            .context("untrusted for too long")??;
        if !paired {
            return Ok(());
        }
    } else {
        lock(&d.net.trust).seen(&t.remote);
    }
    drop(lingering);
    slot.release();
    t.limit = dent_link::LARGE;
    let id = dent_link::fingerprint(&t.remote);
    let recheck = matches!(verdict, Verdict::Tailscale { .. }).then_some(ip);
    trusted(d.clone(), ws, t, id, recheck).await
}

fn access(s: &str) -> Access {
    match s {
        "full" => Access::Full,
        _ => Access::Unknown,
    }
}

/// One pairing exchange in flight. Dropped without an outcome (the
/// connection went away), it frees the window without counting a guess.
struct Exchange<'a> {
    net: &'a Net,
    ticket: Option<trust::Ticket>,
}

impl Exchange<'_> {
    fn wrong(mut self) -> Refusal {
        match self.ticket.take() {
            Some(t) => lock(&self.net.window).wrong(t),
            None => Refusal::PairClosed,
        }
    }

    fn succeeded(mut self) -> bool {
        self.ticket
            .take()
            .is_some_and(|t| lock(&self.net.window).succeeded(t))
    }
}

impl Drop for Exchange<'_> {
    fn drop(&mut self) {
        if let Some(t) = self.ticket.take() {
            lock(&self.net.window).abandon(t);
        }
    }
}

/// Longest device name kept from a pairing.
const NAME_MAX: usize = 64;

/// An untrusted device may pair while a code shows. Returns whether it's
/// now trusted. Only a completed exchange with a wrong code counts as a
/// guess; a dropped connection doesn't.
async fn pairing(d: &Daemon, ws: &mut Ws, t: &mut Transport, hello: &Hello) -> Result<bool> {
    loop {
        let frame = tokio::time::timeout(HANDSHAKE * 6, recv_bin(ws))
            .await
            .context("idle while untrusted")??;
        let Some(Opened::Control(c)) = t.open(&frame)? else {
            // Data from an untrusted device is ignored.
            continue;
        };
        match c {
            Control::Ping { t: n } => send_control(ws, t, &Control::Pong { t: n }).await?,
            Control::Pair { spake } => {
                let begun = lock(&d.net.window).begin();
                let (code, ticket) = match begun {
                    Ok(c) => c,
                    Err(r) => {
                        send_control(ws, t, &Control::Refuse(r)).await?;
                        continue;
                    }
                };
                let exchange = Exchange {
                    net: &d.net,
                    ticket: Some(ticket),
                };
                let mut p = dent_link::pair::Pairing::start(&code);
                let out = p.outbound.clone();
                let key = p.finish(&hex::decode(&spake).context("pairing message")?)?;
                send_control(
                    ws,
                    t,
                    &Control::Pair {
                        spake: hex::encode(out),
                    },
                )
                .await?;
                let frame = tokio::time::timeout(HANDSHAKE, recv_bin(ws))
                    .await
                    .context("pairing stalled")??;
                let mac = match t.open(&frame)? {
                    Some(Opened::Control(Control::PairConfirm { mac })) => mac,
                    _ => bail!("expected the pairing confirmation"),
                };
                let ok = hex::decode(&mac).is_ok_and(|m| {
                    dent_link::pair::verify(&key, &t.hash, dent_link::pair::Role::Client, &m)
                });
                if !ok {
                    let r = exchange.wrong();
                    send_control(ws, t, &Control::Refuse(r)).await?;
                    continue;
                }
                // The right code, but it may have been replaced or expired
                // while the exchange ran.
                if !exchange.succeeded() {
                    send_control(ws, t, &Control::Refuse(Refusal::PairClosed)).await?;
                    continue;
                }
                let name: String = hello.name.trim().chars().take(NAME_MAX).collect();
                let name = if name.is_empty() {
                    "a device".into()
                } else {
                    name
                };
                lock(&d.net.trust).add(&t.remote, &name)?;
                let mine = dent_link::pair::confirm(&key, &t.hash, dent_link::pair::Role::Daemon);
                send_control(
                    ws,
                    t,
                    &Control::PairConfirm {
                        mac: hex::encode(mine),
                    },
                )
                .await?;
                send_control(
                    ws,
                    t,
                    &Control::Paired {
                        access: Access::Full,
                    },
                )
                .await?;
                return Ok(true);
            }
            _ => {}
        }
    }
}

/// Whether a device trusted through the Tailscale account still is: `false`
/// only when Tailscale says so, never when it can't be asked.
async fn still_same_account(net: &Net, ip: IpAddr) -> bool {
    let Ok(peer) = tailnet::whois(ip).await else {
        return true;
    };
    let Some(me) = net.tailscale().await else {
        return true;
    };
    peer.is_some_and(|p| tailnet::same_account(&me, &p))
}

/// A trusted connection: the same session as the local socket, framed.
/// `recheck` is the peer address when trust came from the Tailscale
/// account, which is checked again every [`RECHECK`].
async fn trusted(
    d: Arc<Daemon>,
    ws: Ws,
    t: Transport,
    id: String,
    recheck: Option<IpAddr>,
) -> Result<()> {
    let t = Arc::new(Mutex::new(t));
    let (mut sink, mut stream) = ws.split();
    let (lines_tx, lines_rx) = mpsc::channel::<String>(64);
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(OUT_QUEUE);
    let (ctl_tx, mut ctl_rx) = mpsc::channel::<Control>(CTL_QUEUE);
    let session = tokio::spawn(crate::server::session(d.clone(), lines_rx, out_tx));
    let mut removed = d.net.removed.subscribe();
    let mut bye = d.net.bye.subscribe();

    // The writer alone seals and sends, so frames go out in nonce order.
    // Control messages go first, and between the parts of a large message.
    // A goodbye or refusal is the last thing sent.
    let mut writer = {
        let t = t.clone();
        tokio::spawn(async move {
            let send = async |sink: &mut futures_util::stream::SplitSink<Ws, Message>,
                              f: Vec<u8>| {
                matches!(
                    tokio::time::timeout(WRITE_LIMIT, sink.send(Message::Binary(f.into()))).await,
                    Ok(Ok(()))
                )
            };
            // Sends one control message; whether to go on.
            let control = async |sink: &mut futures_util::stream::SplitSink<Ws, Message>,
                                 c: Control| {
                let end = matches!(c, Control::Goodbye(_) | Control::Refuse(_));
                let f = lock(&t).seal_control(&c);
                match f {
                    Ok(f) => send(sink, f).await && !end,
                    Err(_) => false,
                }
            };
            'run: loop {
                tokio::select! {
                    biased;
                    c = ctl_rx.recv() => {
                        let Some(c) = c else { break };
                        if !control(&mut sink, c).await {
                            break;
                        }
                    }
                    line = out_rx.recv() => {
                        let Some(mut line) = line else { break };
                        if line.last() == Some(&b'\n') {
                            line.pop();
                        }
                        let mut i = 0;
                        loop {
                            while let Ok(c) = ctl_rx.try_recv() {
                                if !control(&mut sink, c).await {
                                    break 'run;
                                }
                            }
                            let part = lock(&t).seal_part(&line, i);
                            let part = match part {
                                Ok(Some(p)) => p,
                                Ok(None) => break,
                                Err(_) => break 'run,
                            };
                            if !send(&mut sink, part).await {
                                break 'run;
                            }
                            i += 1;
                        }
                    }
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(1), sink.close()).await;
        })
    };
    let mut writer_done = false;
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + RECHECK, RECHECK);

    let result: Result<()> = async {
        loop {
            let frame = tokio::select! {
                f = tokio::time::timeout(IDLE, stream.next()) => match f {
                    Err(_) => bail!("silent for {} s", IDLE.as_secs()),
                    Ok(None) | Ok(Some(Ok(Message::Close(_)))) => return Ok(()),
                    Ok(Some(Ok(Message::Binary(b)))) => b,
                    Ok(Some(Ok(_))) => continue,
                    Ok(Some(Err(e))) => return Err(e.into()),
                },
                // The device stopped reading, or the socket failed.
                _ = &mut writer => {
                    writer_done = true;
                    return Ok(());
                }
                r = removed.recv() => {
                    // After missed removals, ask the list itself. Trust by
                    // account isn't on it, so only paired trust is asked.
                    let lagged = matches!(r, Err(broadcast::error::RecvError::Lagged(_)))
                        && recheck.is_none()
                        && !lock(&d.net.trust).has(&id);
                    if lagged || matches!(r, Ok(ref gone) if *gone == id) {
                        let _ = ctl_tx.try_send(Control::Refuse(Refusal::Removed));
                        return Ok(());
                    }
                    continue;
                }
                b = bye.recv() => {
                    if let Ok(g) = b {
                        let _ = ctl_tx.try_send(Control::Goodbye(g));
                    }
                    return Ok(());
                }
                _ = tick.tick(), if recheck.is_some() => {
                    if let Some(ip) = recheck
                        && !still_same_account(&d.net, ip).await
                    {
                        let _ = ctl_tx.try_send(Control::Refuse(Refusal::NotTrusted));
                        return Ok(());
                    }
                    continue;
                }
            };
            let opened = lock(&t).open(&frame);
            match opened {
                Err(e) => return Err(e.into()),
                Ok(None) => {}
                // Answered here, before any app work, so a busy daemon still
                // looks alive ([remote-plan.md]).
                Ok(Some(Opened::Control(Control::Ping { t: n }))) => {
                    let _ = ctl_tx.try_send(Control::Pong { t: n });
                }
                Ok(Some(Opened::Control(_))) => {}
                Ok(Some(Opened::Data(bytes))) => {
                    let Ok(line) = String::from_utf8(bytes) else {
                        continue;
                    };
                    if lines_tx.send(line).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
    .await;
    drop(lines_tx);
    drop(ctl_tx);
    // Let a final goodbye or refusal go out before closing.
    if !writer_done
        && tokio::time::timeout(Duration::from_millis(500), &mut writer)
            .await
            .is_err()
    {
        writer.abort();
    }
    session.abort();
    result
}
