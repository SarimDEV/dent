//! A host for the core over tokio: what the desktop app's I/O layer does,
//! and what Swift and JavaScript do on the phone and in the browser.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use spike_core::chat::Mirror;
use spike_core::{Io, Keys, Link, Note, Status};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

enum Ev {
    Opened,
    Frame(Vec<u8>),
    Closed(String),
}

enum Cmd {
    Send(Vec<u8>),
    Close,
}

pub struct Host {
    pub link: Link,
    socks: HashMap<u64, mpsc::UnboundedSender<Cmd>>,
    tx: mpsc::UnboundedSender<(u64, Ev)>,
    rx: mpsc::UnboundedReceiver<(u64, Ev)>,
    pub mirror: Mirror,
    /// Every note, with when it was taken.
    pub log: Vec<(Instant, Note)>,
    /// Every connection the core asked for, with when.
    pub connects: Vec<(Instant, String)>,
    last_drain: Instant,
    /// How often the UI takes notes (a frame).
    pub frame: Duration,
    /// Time spent inside the core taking notes (parsing, building updates).
    pub drain_time: Duration,
}

impl Host {
    pub fn new(keys: Keys, known: Option<Vec<u8>>, addrs: Vec<String>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            link: Link::new("harness", keys, known, addrs),
            socks: HashMap::new(),
            tx,
            rx,
            mirror: Mirror::default(),
            log: Vec::new(),
            connects: Vec::new(),
            last_drain: Instant::now(),
            frame: Duration::from_millis(16),
            drain_time: Duration::ZERO,
        }
    }

    fn do_io(&mut self) {
        for io in self.link.take_io() {
            match io {
                Io::Connect { conn, addr } => {
                    self.connects.push((Instant::now(), addr.clone()));
                    let (ctx, crx) = mpsc::unbounded_channel();
                    self.socks.insert(conn, ctx);
                    tokio::spawn(socket(conn, addr, crx, self.tx.clone()));
                }
                Io::Send { conn, data } => {
                    if let Some(s) = self.socks.get(&conn) {
                        let _ = s.send(Cmd::Send(data));
                    }
                }
                Io::Close { conn } => {
                    if let Some(s) = self.socks.remove(&conn) {
                        let _ = s.send(Cmd::Close);
                    }
                }
            }
        }
    }

    fn drain(&mut self) {
        let t = Instant::now();
        let notes = self.link.take_notes();
        self.drain_time += t.elapsed();
        let at = Instant::now();
        for n in notes {
            if let Note::Message { update } = &n {
                self.mirror.apply(update);
            }
            self.log.push((at, n));
        }
        self.last_drain = at;
    }

    /// Run until `done` holds (checked after each drain) or `limit` passes.
    pub async fn run_until(
        &mut self,
        limit: Duration,
        mut done: impl FnMut(&Host) -> bool,
    ) -> bool {
        let end = Instant::now() + limit;
        loop {
            self.do_io();
            if self.last_drain.elapsed() >= self.frame {
                self.drain();
                if done(self) {
                    return true;
                }
            }
            if Instant::now() >= end {
                self.drain();
                return done(self);
            }
            let now = now_ms();
            let wake = self
                .link
                .next_deadline()
                .map_or(250, |d| d.saturating_sub(now).min(250));
            let frame_left = self.frame.saturating_sub(self.last_drain.elapsed());
            let wait = Duration::from_millis(wake).min(frame_left.max(Duration::from_millis(1)));
            match tokio::time::timeout(wait, self.rx.recv()).await {
                Ok(Some((conn, ev))) => {
                    self.apply(conn, ev);
                    // Take whatever else has arrived before drawing, as a
                    // UI does between frames.
                    while let Ok((conn, ev)) = self.rx.try_recv() {
                        self.apply(conn, ev);
                    }
                }
                _ => self.link.tick(now_ms()),
            }
        }
    }

    fn apply(&mut self, conn: u64, ev: Ev) {
        let now = now_ms();
        match ev {
            Ev::Opened => self.link.opened(conn, now),
            Ev::Frame(b) => self.link.received(conn, &b, now),
            Ev::Closed(why) => {
                self.socks.remove(&conn);
                self.link.closed(conn, &why, now)
            }
        }
        self.link.tick(now);
    }

    pub fn status(&self) -> Status {
        self.link.status()
    }

    pub fn saw(&self, f: impl Fn(&Note) -> bool) -> Option<Instant> {
        self.log.iter().find(|(_, n)| f(n)).map(|(t, _)| *t)
    }

    /// Close every socket, as an app going away does.
    pub fn shutdown(&mut self) {
        self.link.stop();
        self.do_io();
        for (_, s) in self.socks.drain() {
            let _ = s.send(Cmd::Close);
        }
    }
}

async fn socket(
    conn: u64,
    addr: String,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    tx: mpsc::UnboundedSender<(u64, Ev)>,
) {
    let url = format!("ws://{addr}/");
    let ws = tokio::select! {
        r = tokio_tungstenite::connect_async(url) => r,
        // Closed by the core (timed out, or another address won).
        _ = wait_close(&mut cmds) => return,
    };
    let ws = match ws {
        Ok((ws, _)) => ws,
        Err(e) => {
            let _ = tx.send((conn, Ev::Closed(e.to_string())));
            return;
        }
    };
    let _ = tx.send((conn, Ev::Opened));
    let (mut w, mut r) = ws.split();
    loop {
        tokio::select! {
            c = cmds.recv() => match c {
                Some(Cmd::Send(b)) => {
                    if w.send(Message::Binary(b.into())).await.is_err() {
                        let _ = tx.send((conn, Ev::Closed("send failed".into())));
                        return;
                    }
                }
                Some(Cmd::Close) | None => {
                    let _ = w.close().await;
                    return;
                }
            },
            m = r.next() => match m {
                Some(Ok(Message::Binary(b))) => {
                    let _ = tx.send((conn, Ev::Frame(b.to_vec())));
                }
                Some(Ok(Message::Close(_))) | None => {
                    let _ = tx.send((conn, Ev::Closed("closed".into())));
                    return;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    let _ = tx.send((conn, Ev::Closed(e.to_string())));
                    return;
                }
            },
        }
    }
}

async fn wait_close(cmds: &mut mpsc::UnboundedReceiver<Cmd>) {
    loop {
        match cmds.recv().await {
            Some(Cmd::Close) | None => return,
            // Nothing is sent before the socket opens.
            Some(Cmd::Send(_)) => {}
        }
    }
}
