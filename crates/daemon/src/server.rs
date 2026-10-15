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
                        build: d.build.clone(),
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
        Request::Restart => {
            eprintln!("dent-daemon: restarting for a newer build");
            // Reply first, then go; the client starts the new daemon.
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                std::process::exit(0);
            });
            Reply::Ok
        }
        Request::Create {
            cwd,
            model,
            permission,
            effort,
            id,
            text,
            attachments,
        } => Reply::Created {
            session: d
                .create(crate::daemon::NewSession {
                    cwd,
                    model,
                    permission,
                    effort,
                    id,
                    text,
                    attachments,
                })
                .await?,
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
        Request::Fork { session, before } => Reply::Created {
            session: d.fork(&session, before.as_deref()).await?,
        },
        Request::Aside { session, id, text } => {
            d.aside(&session, &id, &text).await?;
            Reply::Ok
        }
        Request::PromoteAside { session, id } => Reply::Created {
            session: d.promote_aside(&session, &id).await?,
        },
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
        Request::Act {
            session,
            command,
            act,
        } => {
            d.command(&session, Op::Act { command, act }).await?;
            Reply::Ok
        }
        Request::SetEffort { session, effort } => {
            d.command(&session, Op::SetEffort(effort)).await?;
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
        Request::Projects => Reply::Projects {
            projects: project_list(d).await?,
        },
        Request::Folders { partial } => {
            let (parent, folders) =
                tokio::task::spawn_blocking(move || crate::projects::folders(&partial)).await??;
            Reply::Folders { parent, folders }
        }
        Request::AddProject { path, create } => {
            let dir = std::path::PathBuf::from(&path);
            if !dir.is_absolute() {
                bail!("{path} isn't a full path");
            }
            if create {
                std::fs::create_dir_all(&dir)?;
            } else if !dir.is_dir() {
                bail!("{path} isn't a folder");
            }
            remember_project(d, &path)?;
            Reply::Projects {
                projects: project_list(d).await?,
            }
        }
        Request::HideProject { path, hidden } => {
            let mut set = hidden_projects(d)?;
            if hidden {
                set.insert(path);
            } else {
                set.remove(&path);
            }
            d.store().put(HIDDEN, &serde_json::to_string(&set)?)?;
            Reply::Projects {
                projects: project_list(d).await?,
            }
        }
        Request::NewProject { name } => {
            let (dir, warning) =
                tokio::task::spawn_blocking(move || crate::projects::create(&name)).await??;
            let path = dir.display().to_string();
            remember_project(d, &path)?;
            Reply::ProjectMade { path, warning }
        }
        Request::CloneProject { url } => {
            let dir = tokio::task::spawn_blocking(move || crate::projects::clone(&url)).await??;
            let path = dir.display().to_string();
            remember_project(d, &path)?;
            Reply::ProjectMade {
                path,
                warning: None,
            }
        }
        Request::Models => Reply::Models { models: d.models() },
        Request::Recent => {
            let owned: std::collections::HashSet<String> = d
                .store()
                .sessions()?
                .into_iter()
                .filter_map(|r| r.agent_session)
                .collect();
            let sessions =
                tokio::task::spawn_blocking(move || crate::terminal::recent(&owned, 12)).await?;
            Reply::Recent { sessions }
        }
        Request::Adopt { agent_session } => Reply::Created {
            session: d.adopt(&agent_session).await?,
        },
        Request::Usage => {
            let (windows, at) = d.usage().await;
            Reply::Usage { windows, at }
        }
        Request::Commands { session } => Reply::Commands {
            commands: d.commands(session.as_deref()),
        },
        Request::Changes { session } => {
            let cwd = cwd_of(d, &session)?;
            let files =
                tokio::task::spawn_blocking(move || crate::git::changes(Path::new(&cwd))).await?;
            Reply::Changes { files }
        }
        Request::Files { session } => {
            let cwd = cwd_of(d, &session)?;
            let paths =
                tokio::task::spawn_blocking(move || crate::git::files(Path::new(&cwd), 20_000))
                    .await?;
            Reply::Files { paths }
        }
        Request::FilesIn { dir } => {
            let dir = std::path::PathBuf::from(dir);
            if !dir.is_absolute() || !dir.is_dir() {
                bail!("{} is not a folder", dir.display());
            }
            let paths =
                tokio::task::spawn_blocking(move || crate::git::files(&dir, 20_000)).await?;
            Reply::Files { paths }
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

/// Where the folders added here are kept, with when; and the ones taken
/// off the list.
const ADDED: &str = "projects.added";
const HIDDEN: &str = "projects.hidden";

fn added_projects(d: &Daemon) -> Result<std::collections::HashMap<String, u64>> {
    Ok(d.store()
        .get(ADDED)?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default())
}

fn hidden_projects(d: &Daemon) -> Result<std::collections::HashSet<String>> {
    Ok(d.store()
        .get(HIDDEN)?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default())
}

/// Keep a folder on the list, newest, and back on it if it was taken off.
fn remember_project(d: &Daemon, path: &str) -> Result<()> {
    let mut added = added_projects(d)?;
    added.insert(path.to_string(), crate::daemon::now_ms());
    let mut hidden = hidden_projects(d)?;
    hidden.remove(path);
    let store = d.store();
    store.put(ADDED, &serde_json::to_string(&added)?)?;
    store.put(HIDDEN, &serde_json::to_string(&hidden)?)?;
    Ok(())
}

async fn project_list(d: &Daemon) -> Result<Vec<dent_protocol::Project>> {
    let recent: Vec<(String, u64)> = d
        .store()
        .sessions()?
        .into_iter()
        .map(|r| (r.cwd, r.updated))
        .collect();
    let (added, hidden) = (added_projects(d)?, hidden_projects(d)?);
    Ok(
        tokio::task::spawn_blocking(move || crate::projects::list(&recent, &added, &hidden))
            .await?,
    )
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
        for (session, activity) in d.activities() {
            let _ = tx.send(encode(
                0,
                &Out::Activity {
                    session,
                    activity: Some(activity),
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
                        Live::Models(models) => Out::Models {
                            models: models.clone(),
                        },
                        Live::Delta {
                            session,
                            text,
                            reset,
                        } => Out::Delta {
                            session: session.clone(),
                            text: text.clone(),
                            reset: *reset,
                        },
                        Live::Activity { session, activity } => Out::Activity {
                            session: session.clone(),
                            activity: activity.clone(),
                        },
                        Live::Commands { session, commands } => Out::Commands {
                            session: session.clone(),
                            commands: commands.clone(),
                        },
                        Live::Out(out) => out.clone(),
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
