//! The daemon's side of the session holder: connect (starting the holder if
//! it isn't running), then spawn, write, attach, stop and release.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, oneshot};

use dent_session_holder::holder::socket;
use dent_session_holder::wire::{Envelope, MAX_HEADER, MAX_PAYLOAD, Msg, PROTO, Proc, Req};

pub enum Event {
    Data {
        id: String,
        err: bool,
        off: u64,
        bytes: Vec<u8>,
    },
    Exit {
        id: String,
        code: Option<i32>,
        signal: Option<i32>,
    },
    /// A newer daemon took over: this one should stop.
    Replaced,
    /// The holder went away (it crashed or was killed).
    Closed,
}

pub struct Holder {
    w: tokio::sync::Mutex<OwnedWriteHalf>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Msg>>>>,
    next: AtomicU64,
    pub proto: u32,
    pub version: String,
    pub pid: u32,
}

/// How to start a holder when none is running: the program and its
/// arguments, normally this same binary with a `holder` subcommand.
pub struct Start {
    pub argv: Vec<String>,
}

pub async fn connect(
    dir: &Path,
    start: Option<&Start>,
) -> Result<(Holder, mpsc::UnboundedReceiver<Event>)> {
    let sock = socket(dir);
    let stream = match UnixStream::connect(&sock).await {
        Ok(s) => s,
        Err(_) => {
            let Some(start) = start else {
                bail!("no holder at {}", sock.display())
            };
            launch(dir, start)?;
            let mut tries = 0;
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                match UnixStream::connect(&sock).await {
                    Ok(s) => break s,
                    Err(e) if tries > 300 => bail!("holder didn't start: {e}"),
                    Err(_) => tries += 1,
                }
            }
        }
    };
    let (r, w) = stream.into_split();
    let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Msg>>>> = Arc::default();
    let (tx, rx) = mpsc::unbounded_channel();
    {
        let pending = pending.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(r);
            loop {
                match read(&mut r).await {
                    Ok(Some((env, _))) if env.req != 0 => {
                        if let Some(tx) = pending.lock().ok().and_then(|mut p| p.remove(&env.req)) {
                            let _ = tx.send(env.body);
                        }
                    }
                    Ok(Some((env, bytes))) => {
                        let ev = match env.body {
                            Msg::Data { id, err, off } => Event::Data {
                                id,
                                err,
                                off,
                                bytes,
                            },
                            Msg::Exit { id, code, signal } => Event::Exit { id, code, signal },
                            Msg::Error { msg } if msg == "replaced" => Event::Replaced,
                            _ => continue,
                        };
                        if tx.send(ev).is_err() {
                            return;
                        }
                    }
                    Ok(None) | Err(_) => {
                        let _ = tx.send(Event::Closed);
                        if let Ok(mut p) = pending.lock() {
                            p.clear();
                        }
                        return;
                    }
                }
            }
        });
    }
    let mut h = Holder {
        w: tokio::sync::Mutex::new(w),
        pending,
        next: AtomicU64::new(1),
        proto: 0,
        version: String::new(),
        pid: 0,
    };
    match h.call(Req::Hello { proto: PROTO }, &[]).await? {
        Msg::Hello {
            proto,
            version,
            pid,
            ..
        } => {
            h.proto = proto;
            h.version = version;
            h.pid = pid;
        }
        other => bail!("unexpected hello reply {other:?}"),
    }
    Ok((h, rx))
}

/// Start a holder in the background. It leaves our process group itself;
/// a thread reaps it if it exits while we're still running.
fn launch(dir: &Path, start: &Start) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    // Logs rotate by size, one old file kept.
    let path = dir.join("holder.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
        let _ = std::fs::rename(&path, dir.join("holder.log.1"));
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("holder.log"))?;
    let Some((cmd, args)) = start.argv.split_first() else {
        bail!("empty holder command")
    };
    let mut child = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

async fn read(
    r: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Result<Option<(Envelope<Msg>, Vec<u8>)>> {
    let mut line = Vec::new();
    let got = (&mut *r)
        .take(MAX_HEADER as u64 + 1)
        .read_until(b'\n', &mut line)
        .await?;
    if got == 0 {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        bail!("header cut short");
    }
    let env: Envelope<Msg> = serde_json::from_slice(&line)?;
    if env.n > MAX_PAYLOAD {
        bail!("payload too large");
    }
    let mut bytes = vec![0; env.n];
    r.read_exact(&mut bytes).await?;
    Ok(Some((env, bytes)))
}

impl Holder {
    async fn call(&self, req: Req, payload: &[u8]) -> Result<Msg> {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        if let Ok(mut p) = self.pending.lock() {
            p.insert(n, tx);
        }
        let mut line = serde_json::to_vec(&Envelope {
            req: n,
            n: payload.len(),
            body: req,
        })?;
        line.push(b'\n');
        line.extend_from_slice(payload);
        let forget = || {
            if let Ok(mut p) = self.pending.lock() {
                p.remove(&n);
            }
        };
        if let Err(e) = self.w.lock().await.write_all(&line).await {
            forget();
            return Err(e.into());
        }
        match tokio::time::timeout(Duration::from_secs(10), rx).await {
            Ok(Ok(Msg::Error { msg })) => bail!("{msg}"),
            Ok(Ok(m)) => Ok(m),
            Ok(Err(_)) => bail!("holder closed"),
            Err(_) => {
                // A late answer has nowhere to go.
                forget();
                bail!("holder timed out")
            }
        }
    }

    pub async fn spawn(
        &self,
        id: &str,
        argv: Vec<String>,
        cwd: Option<PathBuf>,
        env: Vec<(String, String)>,
    ) -> Result<u32> {
        let req = Req::Spawn {
            id: id.into(),
            argv,
            cwd: cwd.map(|p| p.to_string_lossy().into_owned()),
            env,
        };
        match self.call(req, &[]).await? {
            Msg::Spawned { pid } => Ok(pid),
            other => bail!("unexpected {other:?}"),
        }
    }

    pub async fn write(&self, id: &str, bytes: &[u8]) -> Result<()> {
        self.call(Req::Write { id: id.into() }, bytes)
            .await
            .map(drop)
    }

    pub async fn close_stdin(&self, id: &str) -> Result<()> {
        self.call(Req::CloseStdin { id: id.into() }, &[])
            .await
            .map(drop)
    }

    pub async fn attach(&self, id: &str, out: u64, err: u64) -> Result<()> {
        let req = Req::Attach {
            id: id.into(),
            out,
            err,
        };
        self.call(req, &[]).await.map(drop)
    }

    pub async fn stop(&self, id: &str, grace: Duration) -> Result<()> {
        let req = Req::Stop {
            id: id.into(),
            grace_ms: grace.as_millis() as u64,
        };
        self.call(req, &[]).await.map(drop)
    }

    pub async fn release(&self, id: &str) -> Result<()> {
        self.call(Req::Release { id: id.into() }, &[])
            .await
            .map(drop)
    }

    pub async fn list(&self) -> Result<Vec<Proc>> {
        match self.call(Req::List, &[]).await? {
            Msg::Procs { procs } => Ok(procs),
            other => bail!("unexpected {other:?}"),
        }
    }
}
