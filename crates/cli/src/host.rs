//! Runs [`Machine`] actors on tokio: dials the WebSockets they ask for,
//! carries frames both ways, and ticks them on time. One task per machine;
//! sockets report back through a channel tagged with the connection number,
//! so a late report from an old socket is simply ignored by the actor.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use dent_client_core::machine::{Io, Machine, Note};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

enum SockEvent {
    Opened(u64),
    Frame(u64, Vec<u8>),
    Closed(u64, String),
}

/// What a caller can ask of a running machine.
pub enum Ask {
    Pair(String),
    Send(Vec<u8>),
    Stop,
}

/// Drives `m` until told to stop, handing every note to `notes`.
pub async fn run(
    mut m: Machine,
    mut asks: mpsc::UnboundedReceiver<Ask>,
    notes: mpsc::UnboundedSender<Note>,
) {
    let start = Instant::now();
    let now = || start.elapsed().as_millis() as u64;
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<SockEvent>();
    let mut socks: HashMap<u64, mpsc::UnboundedSender<Option<Vec<u8>>>> = HashMap::new();
    m.connect(now());
    loop {
        for op in m.take_io() {
            match op {
                Io::Connect { conn, addr } => {
                    let (tx, rx) = mpsc::unbounded_channel();
                    socks.insert(conn, tx);
                    tokio::spawn(socket(conn, addr, rx, ev_tx.clone()));
                }
                Io::Send { conn, data } => {
                    if let Some(s) = socks.get(&conn) {
                        let _ = s.send(Some(data));
                    }
                }
                Io::Close { conn } => {
                    if let Some(s) = socks.remove(&conn) {
                        let _ = s.send(None);
                    }
                }
            }
        }
        for n in m.take_notes() {
            if notes.send(n).is_err() {
                return;
            }
        }
        let wait = m
            .next_deadline()
            .map(|d| d.saturating_sub(now()))
            .unwrap_or(60_000)
            .min(60_000);
        tokio::select! {
            ev = ev_rx.recv() => match ev {
                Some(SockEvent::Opened(c)) => m.opened(c, now()),
                Some(SockEvent::Frame(c, f)) => m.received(c, &f, now()),
                Some(SockEvent::Closed(c, why)) => {
                    socks.remove(&c);
                    m.closed(c, &why, now());
                }
                None => return,
            },
            a = asks.recv() => match a {
                Some(Ask::Pair(code)) => m.pair(&code, now()),
                Some(Ask::Send(line)) => { m.send(&line, now()); }
                Some(Ask::Stop) | None => {
                    m.disconnect();
                    for (_, s) in socks.drain() {
                        let _ = s.send(None);
                    }
                    return;
                }
            },
            _ = tokio::time::sleep(Duration::from_millis(wait)) => m.tick(now()),
        }
    }
}

/// One WebSocket: dials, then copies frames until either side closes.
async fn socket(
    conn: u64,
    addr: String,
    mut out: mpsc::UnboundedReceiver<Option<Vec<u8>>>,
    ev: mpsc::UnboundedSender<SockEvent>,
) {
    let url = format!("ws://{addr}/");
    let cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_frame_size(Some(65535 + 64))
        .max_message_size(Some(65535 + 64));
    // The actor times attempts out itself; this only bounds a dial the OS
    // would otherwise keep trying for a minute.
    let dial = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async_with_config(url, Some(cfg), true),
    )
    .await;
    let ws = match dial {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => {
            let _ = ev.send(SockEvent::Closed(conn, e.to_string()));
            return;
        }
        Err(_) => {
            let _ = ev.send(SockEvent::Closed(conn, "dial timed out".into()));
            return;
        }
    };
    if let Ok(s) = ws.get_ref().get_ref_tcp() {
        let _ = s.set_nodelay(true);
    }
    let _ = ev.send(SockEvent::Opened(conn));
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            o = out.recv() => match o {
                Some(Some(data)) => {
                    if sink.send(Message::Binary(data.into())).await.is_err() {
                        let _ = ev.send(SockEvent::Closed(conn, "send failed".into()));
                        return;
                    }
                }
                Some(None) | None => {
                    let _ = sink.close().await;
                    return;
                }
            },
            m = stream.next() => match m {
                Some(Ok(Message::Binary(b))) => {
                    let _ = ev.send(SockEvent::Frame(conn, b.to_vec()));
                }
                Some(Ok(Message::Close(_))) | None => {
                    let _ = ev.send(SockEvent::Closed(conn, "closed".into()));
                    return;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    let _ = ev.send(SockEvent::Closed(conn, e.to_string()));
                    return;
                }
            },
        }
    }
}

trait TcpRef {
    fn get_ref_tcp(&self) -> std::io::Result<&tokio::net::TcpStream>;
}

impl TcpRef for tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream> {
    fn get_ref_tcp(&self) -> std::io::Result<&tokio::net::TcpStream> {
        match self {
            tokio_tungstenite::MaybeTlsStream::Plain(s) => Ok(s),
            _ => Err(std::io::Error::other("not plain TCP")),
        }
    }
}
