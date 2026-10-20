//! Clients on this machine connect over a Unix socket that only this user
//! can open. Each line is one [`Request`]; replies, events and deltas go
//! back the same way.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use dent_protocol::{BATCH_BYTES, Body, Envelope, Out, PROTOCOL, Reply, Request, b64, encode};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

use crate::daemon::{Daemon, Live};
use crate::session::Op;

/// Lines waiting to go to one client.
const OUT_QUEUE: usize = 256;

/// A client that hasn't taken a line in this long is dropped.
const WRITE_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

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
    // Bounded: a client that stops reading holds the subscription up, so
    // it falls behind the live feed and catches up from the log instead of
    // growing a queue here. One that takes too long over a write is let go.
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(OUT_QUEUE);
    tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            match tokio::time::timeout(WRITE_LIMIT, w.write_all(&line)).await {
                Ok(Ok(())) => {}
                _ => break,
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
                let _ = tx
                    .send(encode(
                        0,
                        &Out::Error {
                            message: format!("bad request: {e}"),
                        },
                    ))
                    .await;
                continue;
            }
        };
        let req = env.req;
        if !hello && !matches!(env.body, Request::Hello { .. }) {
            let _ = tx
                .send(encode(
                    req,
                    &Out::Error {
                        message: "hello first".into(),
                    },
                ))
                .await;
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
        if tx.send(encode(req, &msg)).await.is_err() {
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
            worktree,
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
                    worktree,
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
        Request::Fork {
            session,
            before,
            until,
            id,
        } => Reply::Created {
            session: match until {
                Some(seq) => d.fork_until(&session, seq, id.as_deref()).await?,
                None => d.fork(&session, before.as_deref(), id.as_deref()).await?,
            },
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
        Request::Requeue {
            session,
            id,
            before,
        } => {
            d.command(&session, Op::Requeue { id, before }).await?;
            Reply::Ok
        }
        Request::Interrupt { session, by } => {
            eprintln!(
                "dent-daemon: stop asked for {session} by {}",
                by.as_deref().unwrap_or("a client that didn't say")
            );
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
        Request::RestartAgent { session } => {
            d.command(&session, Op::Restart).await?;
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
            // Its worktree taken away since: back on its branch.
            let cwd = cwd_of(d, &session)?;
            if let (Some(branch), Some(root)) = d.workspace_of(&session)
                && !Path::new(&cwd).exists()
            {
                tokio::task::spawn_blocking(move || {
                    crate::workspace::restore(Path::new(&cwd), Path::new(&root), &branch)
                })
                .await?
                .map_err(|e| anyhow::anyhow!("couldn't bring its worktree back: {e}"))?;
            }
            d.command(&session, Op::Reopen).await?;
            Reply::Ok
        }
        Request::RemoveWorktree { session, force } => {
            let cwd = cwd_of(d, &session)?;
            // Only ever a linked worktree: never a project's main checkout.
            let root = crate::workspace::main_checkout(Path::new(&cwd))
                .map(|p| p.display().to_string())
                .or(d.workspace_of(&session).1)
                .context("the session doesn't work in a worktree")?;
            tokio::task::spawn_blocking(move || {
                crate::workspace::remove(Path::new(&cwd), Path::new(&root), force)
            })
            .await?
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            Reply::Ok
        }
        Request::Repo { dir } => Reply::Repo {
            repo: tokio::task::spawn_blocking(move || crate::workspace::repo(Path::new(&dir)))
                .await?,
        },
        Request::GitStatus { session } => {
            let cwd = cwd_of(d, &session)?;
            let gh = crate::ship::gh(&d.env().path);
            Reply::GitStatus {
                status: tokio::task::spawn_blocking(move || {
                    crate::ship::status(Path::new(&cwd), gh.as_deref())
                })
                .await?,
            }
        }
        Request::Git {
            session,
            action,
            message,
        } => {
            let cwd = cwd_of(d, &session)?;
            let (summary, url) = d.git(&cwd, action, message).await?;
            d.note_workspace(&session, &cwd).await;
            d.note_pr(&session, &cwd).await;
            Reply::GitDone { summary, url }
        }
        Request::PrBrief { session, about } => {
            let cwd = cwd_of(d, &session)?;
            let gh = crate::ship::gh(&d.env().path)
                .context("this needs the GitHub CLI (gh), signed in")?;
            let text = tokio::task::spawn_blocking(move || {
                let dir = Path::new(&cwd);
                let pr = crate::ship::pull_request(dir, &gh)?
                    .ok_or_else(|| "the branch has no pull request".to_string())?;
                match about {
                    dent_protocol::PrBrief::Checks => {
                        crate::ship::checks_brief(dir, &gh, pr.number)
                    }
                    dent_protocol::PrBrief::Review => {
                        crate::ship::review_brief(dir, &gh, pr.number)
                    }
                }
            })
            .await?
            .map_err(|e| anyhow::anyhow!(e))?;
            Reply::PrBrief { text }
        }
        Request::Retitle { session } => {
            let d = d.clone();
            tokio::spawn(async move {
                if let Err(e) = d.title_session(&session, true).await {
                    eprintln!("dent-daemon: {session}: couldn't title it: {e:#}");
                }
            });
            Reply::Ok
        }
        Request::Resume { session, at } => {
            d.command(&session, Op::Resume(at)).await?;
            Reply::Ok
        }
        Request::StopTask { session, task } => {
            d.command(&session, Op::StopTask(task)).await?;
            Reply::Ok
        }
        Request::KeepAwake { on } => {
            d.keep_awake(on);
            Reply::Ok
        }
        Request::Seen { session, upto } => {
            d.command(&session, Op::Seen(upto)).await?;
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
        Request::Changes { session, turn } => {
            let cwd = cwd_of(d, &session)?;
            let span = match &turn {
                Some(msg) => Some(turn_span(d, &session, msg)?),
                None => None,
            };
            let files = tokio::task::spawn_blocking(move || match span {
                Some((from, to)) => {
                    crate::git::changes_between(Path::new(&cwd), &from, to.as_deref())
                }
                None => crate::git::changes(Path::new(&cwd)),
            })
            .await?;
            Reply::Changes { files }
        }
        Request::Discard {
            session,
            path,
            hunk,
            turn,
        } => {
            let cwd = cwd_of(d, &session)?;
            let base = match &turn {
                Some(msg) => match turn_span(d, &session, msg)? {
                    (from, None) => Some(from),
                    (_, Some(_)) => {
                        bail!(
                            "a later turn has built on this one; discard from the latest, or from all changes"
                        )
                    }
                },
                None => None,
            };
            tokio::task::spawn_blocking(move || {
                crate::git::discard(
                    Path::new(&cwd),
                    base.as_deref(),
                    &path,
                    hunk.map(|h| h as usize),
                )
            })
            .await?
            .map_err(|e| anyhow::anyhow!(e))?;
            Reply::Ok
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

/// One turn's span: the snapshot from just before message `msg`, and the
/// one before the next message, if there is a next one (else it runs to
/// the working tree now).
fn turn_span(d: &Daemon, session: &str, msg: &str) -> Result<(String, Option<String>)> {
    let events = d.store().session_events(session)?;
    let mut checkpoints = events.iter().filter_map(|e| match &e.body {
        Body::Checkpoint { id, commit } => Some((id.clone(), commit.clone())),
        _ => None,
    });
    let from = checkpoints
        .find(|(id, _)| id == msg)
        .map(|(_, c)| c)
        .context("there's no snapshot of the files from before that turn")?;
    Ok((from, checkpoints.next().map(|(_, c)| c)))
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
    tx: mpsc::Sender<Vec<u8>>,
) {
    // Sessions this client holds a streaming reply for, and the count of
    // the last delta its snapshot held.
    let mut streaming = std::collections::HashSet::new();
    let mut snapshot: u64;
    loop {
        loop {
            let got = d.store().after(after, BATCH_BYTES);
            let (batch, upto) = match got {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx
                        .send(encode(
                            0,
                            &Out::Error {
                                message: format!("{e:#}"),
                            },
                        ))
                        .await;
                    return;
                }
            };
            // Rows this daemon can't read are passed over, not taken for
            // the end of the log.
            if upto <= after {
                break;
            }
            after = upto;
            if batch.is_empty() {
                continue;
            }
            if tx
                .send(encode(0, &Out::Events { events: batch }))
                .await
                .is_err()
            {
                return;
            }
        }
        if tx
            .send(encode(0, &Out::CaughtUp { seq: after }))
            .await
            .is_err()
        {
            return;
        }
        // The replies streaming now, whole. Deltas already queued for this
        // client are in the snapshot, so they're skipped by number; after
        // a lag, a reply that ended while this client wasn't listening is
        // dropped.
        let (now, n) = d.partials();
        for session in std::mem::take(&mut streaming) {
            if !now.iter().any(|(s, _)| *s == session) {
                let _ = tx
                    .send(encode(
                        0,
                        &Out::Delta {
                            session,
                            text: String::new(),
                            reset: true,
                        },
                    ))
                    .await;
            }
        }
        snapshot = n;
        for (session, text) in now {
            streaming.insert(session.clone());
            let _ = tx
                .send(encode(
                    0,
                    &Out::Delta {
                        session,
                        text,
                        reset: true,
                    },
                ))
                .await;
        }
        for (session, activity) in d.activities() {
            let _ = tx
                .send(encode(
                    0,
                    &Out::Activity {
                        session,
                        activity: Some(activity),
                    },
                ))
                .await;
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
                        Live::Delta { n, .. } if *n <= snapshot => continue,
                        Live::Delta {
                            session,
                            text,
                            reset,
                            ..
                        } => {
                            if *reset && text.is_empty() {
                                streaming.remove(session);
                            } else {
                                streaming.insert(session.clone());
                            }
                            Out::Delta {
                                session: session.clone(),
                                text: text.clone(),
                                reset: *reset,
                            }
                        }
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
                    if tx.send(encode(0, &msg)).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => break,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}
