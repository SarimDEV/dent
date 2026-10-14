//! Clients on this machine connect over a Unix socket that only this user
//! can open. Each line is one [`Request`]; replies, events and deltas go
//! back the same way.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use dent_protocol::{BATCH_BYTES, Envelope, Out, PROTOCOL, Reply, Request, b64, encode};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

use crate::daemon::{Daemon, Live};
use crate::session::Op;

/// Files the viewer may open are capped at this size.
const FILE_CAP: u64 = 32 * 1024 * 1024;

pub async fn serve(d: Arc<Daemon>) -> Result<()> {
    let sock = d.paths.socket();
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("bind {}", sock.display()))?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    loop {
        let (stream, _) = listener.accept().await?;
        let d = d.clone();
        tokio::spawn(async move {
            let _ = conn(d, stream).await;
        });
    }
}

async fn conn(d: Arc<Daemon>, stream: UnixStream) -> Result<()> {
    let (r, mut w) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if w.write_all(&line).await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(r).lines();
    let mut hello = false;
    let mut sub: Option<tokio::task::JoinHandle<()>> = None;
    while let Some(line) = lines.next_line().await? {
        let env: Envelope<Request> = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(e) => {
                let _ = tx.send(encode(
                    0,
                    &Out::Error {
                        message: format!("bad request: {e}"),
                    },
                ));
                continue;
            }
        };
        let req = env.req;
        if !hello && !matches!(env.body, Request::Hello { .. }) {
            let _ = tx.send(encode(
                req,
                &Out::Error {
                    message: "hello first".into(),
                },
            ));
            continue;
        }
        let out = match env.body {
            Request::Hello { proto, .. } => {
                if proto != PROTOCOL {
                    Err(anyhow::anyhow!(
                        "this daemon speaks protocol {PROTOCOL}, the app {proto}; update the older one"
                    ))
                } else {
                    hello = true;
                    Ok(Reply::Welcome {
                        proto: PROTOCOL,
                        version: d.version.clone(),
                        machine: d.machine.clone(),
                        home: crate::paths::home().display().to_string(),
                    })
                }
            }
            Request::Subscribe { after } => {
                if let Some(old) = sub.take() {
                    old.abort();
                }
                let rx = d.live.subscribe();
                sub = Some(tokio::spawn(subscribe(d.clone(), after, rx, tx.clone())));
                Ok(Reply::Ok)
            }
            other => handle(&d, other).await,
        };
        let msg = match out {
            Ok(reply) => Out::Reply { reply },
            Err(e) => Out::Error {
                message: format!("{e:#}"),
            },
        };
        if tx.send(encode(req, &msg)).is_err() {
            break;
        }
    }
    if let Some(s) = sub {
        s.abort();
    }
    Ok(())
}

async fn handle(d: &Arc<Daemon>, req: Request) -> Result<Reply> {
    Ok(match req {
        Request::Hello { .. } | Request::Subscribe { .. } => Reply::Ok,
        Request::Create {
            cwd,
            model,
            permission,
            id,
            text,
            attachments,
        } => Reply::Created {
            session: d.create(cwd, model, permission, id, text, attachments).await?,
        },
        Request::Send {
            session,
            id,
            text,
            steer,
            attachments,
        } => {
            d.command(
                &session,
                Op::Send {
                    id,
                    text,
                    steer,
                    attachments,
                },
            )
            .await?;
            Reply::Ok
        }
        Request::Rewind { session, to, files } => {
            d.command(&session, Op::Rewind { to, files }).await?;
            Reply::Ok
        }
        Request::Unqueue { session, id } => {
            d.command(&session, Op::Unqueue { id }).await?;
            Reply::Ok
        }
        Request::Interrupt { session } => {
            d.command(&session, Op::Interrupt).await?;
            Reply::Ok
        }
        Request::Answer {
            session,
            id,
            answer,
        } => {
            d.command(&session, Op::Answer { id, answer }).await?;
            Reply::Ok
        }
        Request::SetModel { session, model } => {
            d.command(&session, Op::SetModel(model)).await?;
            Reply::Ok
        }
        Request::SetPermission {
            session,
            permission,
        } => {
            d.command(&session, Op::SetPermission(permission)).await?;
            Reply::Ok
        }
        Request::Rename { session, title } => {
            d.command(&session, Op::Rename(title)).await?;
            Reply::Ok
        }
        Request::Close { session } => {
            d.command(&session, Op::Close).await?;
            Reply::Ok
        }
        Request::Reopen { session } => {
            d.command(&session, Op::Reopen).await?;
            Reply::Ok
        }
        Request::Projects => {
            let recent: Vec<(String, u64)> = d
                .store()
                .sessions()?
                .into_iter()
                .map(|r| (r.cwd, r.updated))
                .collect();
            let projects =
                tokio::task::spawn_blocking(move || crate::projects::list(&recent)).await?;
            Reply::Projects { projects }
        }
        Request::Models => Reply::Models { models: d.models() },
        Request::Commands { session } => Reply::Commands {
            commands: d.commands(session.as_deref()),
        },
        Request::Changes { session } => {
            let cwd = cwd_of(d, &session)?;
            let files =
                tokio::task::spawn_blocking(move || crate::git::changes(Path::new(&cwd))).await?;
            Reply::Changes { files }
        }
        Request::ReadFile { session, path } => {
            let cwd = cwd_of(d, &session)?;
            let bytes =
                tokio::task::spawn_blocking(move || read_inside(Path::new(&cwd), &path)).await??;
            Reply::File {
                bytes_b64: b64::encode(&bytes),
            }
        }
    })
}

fn cwd_of(d: &Daemon, session: &str) -> Result<String> {
    Ok(d.store()
        .session(session)?
        .with_context(|| format!("no session {session}"))?
        .cwd)
}

/// Read a file, refusing anything outside the session's directory.
fn read_inside(cwd: &Path, path: &str) -> Result<Vec<u8>> {
    let root = cwd.canonicalize()?;
    let full = root.join(path).canonicalize()?;
    if !full.starts_with(&root) {
        bail!("{path} is outside the session's folder");
    }
    if std::fs::metadata(&full)?.len() > FILE_CAP {
        bail!("{path} is too big to show");
    }
    Ok(std::fs::read(full)?)
}

/// Catch up from the log, then stream live. Live events that arrive during
/// catch-up are skipped by sequence number; a client that falls too far
/// behind the live feed catches up from the log again.
async fn subscribe(
    d: Arc<Daemon>,
    mut after: u64,
    mut rx: broadcast::Receiver<Arc<Live>>,
    tx: mpsc::UnboundedSender<Vec<u8>>,
) {
    loop {
        loop {
            let batch = match d.store().after(after, BATCH_BYTES) {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx.send(encode(
                        0,
                        &Out::Error {
                            message: format!("{e:#}"),
                        },
                    ));
                    return;
                }
            };
            let Some(last) = batch.last() else { break };
            after = last.seq;
            if tx.send(encode(0, &Out::Events { events: batch })).is_err() {
                return;
            }
        }
        if tx.send(encode(0, &Out::CaughtUp { seq: after })).is_err() {
            return;
        }
        for (session, text) in d.partials() {
            let _ = tx.send(encode(
                0,
                &Out::Delta {
                    session,
                    text,
                    reset: true,
                },
            ));
        }
        loop {
            match rx.recv().await {
                Ok(live) => {
                    let msg = match &*live {
                        Live::Event(e) if e.seq <= after => continue,
                        Live::Event(e) => {
                            after = e.seq;
                            Out::Events {
                                events: vec![e.clone()],
                            }
                        }
                        Live::Delta {
                            session,
                            text,
                            reset,
                        } => Out::Delta {
                            session: session.clone(),
                            text: text.clone(),
                            reset: *reset,
                        },
                    };
                    if tx.send(encode(0, &msg)).is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => break,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}
