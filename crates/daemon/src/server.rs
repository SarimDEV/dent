//! Clients on this machine connect over a Unix socket that only this user
//! can open. Each line is one [`Request`]; replies, events and deltas go
//! back the same way.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use dent_protocol::{
    BATCH_BYTES, Body, Envelope, Out, PROTOCOL, PROTOCOL_MIN, Reply, Request, b64, encode,
};
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

/// The most files one `Stat` asks about.
const STAT_MAX: usize = 256;

/// Why a file can't be read, in words, saying where it looked.
fn said(s: &dent_protocol::FileStat) -> String {
    use dent_protocol::FileStat as F;
    match s {
        F::Missing { checked } => format!("{checked} isn't there"),
        F::Refused { checked } => format!("{checked} isn't shared"),
        F::Unreadable { checked, error } => format!("{checked} can't be read: {error}"),
        F::Dir { path } => format!("{path} is a folder"),
        F::File { path, .. } => path.clone(),
    }
}

/// The socket local clients connect to, bound (an old one left by a
/// daemon that's gone is replaced).
pub fn bind(paths: &crate::paths::Paths) -> Result<UnixListener> {
    let sock = paths.socket();
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("bind {}", sock.display()))?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

pub async fn serve(d: Arc<Daemon>, listener: UnixListener) -> Result<()> {
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
    let (lines_tx, lines_rx) = mpsc::channel::<String>(64);
    tokio::spawn(async move {
        let mut lines = BufReader::new(r).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if lines_tx.send(line).await.is_err() {
                break;
            }
        }
    });
    // Over the local socket, file bytes share the one lane.
    let bulk = tx.clone();
    session(d, lines_rx, tx, bulk).await
}

/// File reads at once across every client, and per client: they run beside
/// the request loop, so a slow disk never holds up an approval or a send.
static FILE_READS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(16);
const FILE_READS_EACH: usize = 4;
/// File reads one client may have waiting.
const READS_QUEUED: usize = 64;

/// Whether `r` only reads files: run beside the loop, its reply on the
/// bulk lane.
fn reads_files(r: &Request) -> bool {
    matches!(
        r,
        Request::Stat { .. }
            | Request::Read { .. }
            | Request::ReadFile { .. }
            | Request::ReadFiles { .. }
            | Request::Pack { .. }
    )
}

/// One client's requests in, its replies and events out: the same for the
/// local socket and for remote devices ([`crate::remote`]).
pub async fn session(
    d: Arc<Daemon>,
    mut lines: mpsc::Receiver<String>,
    tx: mpsc::Sender<Vec<u8>>,
    bulk: mpsc::Sender<Vec<u8>>,
) -> Result<()> {
    let mut hello = false;
    let mine = Arc::new(tokio::sync::Semaphore::new(FILE_READS_EACH));
    // Reads still running end with the session.
    let mut reads = tokio::task::JoinSet::new();
    // Ends with the session, even one aborted mid-request.
    let mut sub: Option<AbortOnDrop> = None;
    while let Some(line) = lines.recv().await {
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
        if reads_files(&env.body) {
            // A client can't queue reads without end: past a point it's told
            // to ask again, and the loop never waits on its reads (an
            // approval behind them would).
            while reads.try_join_next().is_some() {}
            if reads.len() >= READS_QUEUED {
                let _ = bulk.try_send(encode(
                    req,
                    &Out::Error {
                        message: "busy reading files: ask again shortly".into(),
                    },
                ));
                continue;
            }
            let (d, bulk, mine) = (d.clone(), bulk.clone(), mine.clone());
            let body = env.body;
            reads.spawn(async move {
                let msg = {
                    let (Ok(_each), Ok(_all)) = (mine.acquire().await, FILE_READS.acquire().await)
                    else {
                        return;
                    };
                    match handle(&d, body).await {
                        Ok(reply) => Out::Reply { reply },
                        Err(e) => Out::Error {
                            message: format!("{e:#}"),
                        },
                    }
                };
                // Sent with the permits let go: a slow link holds up only
                // its own replies, never other clients' reads.
                let _ = bulk.send(encode(req, &msg)).await;
            });
            // Finished ones let go as it goes.
            while reads.try_join_next().is_some() {}
            continue;
        }
        let out = match env.body {
            Request::Hello { proto, .. } => {
                // The client names its newest protocol; it talks this
                // daemon's when that's within its range.
                if proto < PROTOCOL_MIN {
                    Err(anyhow::anyhow!(
                        "this daemon speaks protocols {PROTOCOL_MIN} to {PROTOCOL}, the app {proto}; update the app"
                    ))
                } else {
                    hello = true;
                    Ok(Reply::Welcome {
                        proto: PROTOCOL,
                        version: d.version.clone(),
                        machine: d.machine.clone(),
                        home: crate::paths::home().display().to_string(),
                        build: d.build.clone(),
                        agents: crate::agent::DRIVERS
                            .iter()
                            .map(|a| crate::agent::brief(*a))
                            .collect(),
                        id: d.net.id(),
                        epoch: d.net.epoch.clone(),
                        addresses: d.net.addresses().await,
                        wake: crate::remote::addrs::wake_targets(),
                        managed: crate::launcher::get().is_some(),
                        release: crate::launcher::version().unwrap_or_default(),
                        update: crate::launcher::outcome(),
                    })
                }
            }
            Request::Subscribe { mut after, epoch } => {
                drop(sub.take());
                // A different epoch means this log isn't the one the
                // client's `after` counts in: start over, and say so.
                if epoch.as_ref().is_some_and(|e| *e != d.net.epoch) {
                    after = 0;
                    let _ = tx
                        .send(encode(
                            0,
                            &Out::Resync {
                                epoch: d.net.epoch.clone(),
                            },
                        ))
                        .await;
                }
                let rx = d.live.subscribe();
                sub = Some(AbortOnDrop(tokio::spawn(subscribe(
                    d.clone(),
                    after,
                    rx,
                    tx.clone(),
                ))));
                Ok(Reply::Ok)
            }
            Request::OpenPairing => d
                .net
                .open_pairing()
                .map(|(code, expires)| Reply::Pairing { code, expires }),
            Request::Pairing => Ok(match d.net.pairing_now() {
                Some((code, expires)) => Reply::Pairing { code, expires },
                None => Reply::Pairing {
                    code: String::new(),
                    expires: 0,
                },
            }),
            Request::Devices => Ok(Reply::Devices {
                devices: d.net.devices(),
            }),
            // One that isn't paired says so, rather than "removed".
            Request::RemoveDevice { id } => d.net.remove_device(&id).and_then(|had| {
                if had {
                    Ok(Reply::Ok)
                } else {
                    anyhow::bail!("no device {id} is paired with this machine")
                }
            }),
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
    Ok(())
}

/// A task that stops when this is dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn handle(d: &Arc<Daemon>, req: Request) -> Result<Reply> {
    Ok(match req {
        Request::Hello { .. }
        | Request::Subscribe { .. }
        | Request::OpenPairing
        | Request::Pairing
        | Request::Devices
        | Request::RemoveDevice { .. } => Reply::Ok,
        Request::Restart => {
            eprintln!("dent-daemon: restarting for a newer build");
            // Reply first, then go; the client starts the new daemon.
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                std::process::exit(0);
            });
            Reply::Ok
        }
        // The launcher trials it; this daemon is stopped if it's taken on.
        Request::Update { version } => {
            let Some(l) = crate::launcher::get() else {
                anyhow::bail!(
                    "this daemon isn't run by dent-launcher, so it can't update itself; install it with `dent-daemon install`"
                );
            };
            tokio::task::spawn_blocking(move || l.update(&version)).await??;
            Reply::Ok
        }
        Request::Create {
            agent,
            cwd,
            worktree,
            model,
            permission,
            effort,
            id,
            text,
            mut attachments,
            files,
            bring,
        } => Reply::Created {
            session: d
                .create(crate::daemon::NewSession {
                    attachments: {
                        attachments.extend(carry(&d.paths.state, &id, files).await?);
                        attachments
                    },
                    agent,
                    cwd,
                    worktree,
                    model,
                    permission,
                    effort,
                    id,
                    text,
                    bring,
                })
                .await?,
        },
        Request::Send {
            session,
            id,
            text,
            steer,
            mut attachments,
            files,
        } => {
            attachments.extend(carry(&d.paths.state, &id, files).await?);
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
        Request::Looked { session, key, look } => {
            d.command(&session, Op::Looked { key, look }).await?;
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
        Request::Pack { dir } => Reply::Pack {
            pack: tokio::task::spawn_blocking(move || crate::bring::pack(Path::new(&dir))).await?,
        },
        Request::ReadFiles { dir, paths } => Reply::Carried {
            files: tokio::task::spawn_blocking(move || {
                crate::bring::read_files(Path::new(&dir), &paths)
            })
            .await?
            .map_err(|e| anyhow::anyhow!(e))?,
        },
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
            let (summary, url) = d.git(d.agent_of(&session), &cwd, action, message).await?;
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
        Request::Agents { fresh } => Reply::Agents {
            agents: d.agents(fresh).await,
            default: crate::agent::default().id().into(),
        },
        Request::Machine => Reply::Machine {
            machine: d.machine().await,
        },
        Request::SetLan { on } => {
            d.set_lan(on)?;
            Reply::Machine {
                machine: d.machine().await,
            }
        }
        Request::SetLoginItem { on } => {
            if on && let Some(why) = crate::login::blocked() {
                bail!("{why}");
            }
            let state = d.paths.state.clone();
            tokio::task::spawn_blocking(move || crate::login::set(on, &state)).await??;
            Reply::Machine {
                machine: d.machine().await,
            }
        }
        Request::Models { agent } => Reply::Models {
            models: d.models(crate::agent::named(agent.as_deref())?.id()),
        },
        Request::Recent => {
            let owned: std::collections::HashSet<String> = d
                .store()
                .sessions()?
                .into_iter()
                .filter_map(|r| r.agent_session)
                .collect();
            // Every agent's, newest first.
            let sessions = tokio::task::spawn_blocking(move || {
                let mut all: Vec<_> = crate::agent::DRIVERS
                    .iter()
                    .filter(|d| d.caps().adopt)
                    .flat_map(|d| d.recent(&owned, 12))
                    .collect();
                all.sort_by_key(|r| std::cmp::Reverse(r.updated));
                all.truncate(12);
                all
            })
            .await?;
            Reply::Recent { sessions }
        }
        Request::Adopt {
            agent_session,
            agent,
        } => Reply::Created {
            session: d.adopt(agent.as_deref(), &agent_session).await?,
        },
        Request::Usage { agent } => {
            let (windows, at) = d.usage(crate::agent::named(agent.as_deref())?).await;
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
        // A session's file, by what the session wrote: wherever it is on this
        // machine, but never the daemon's own secrets (decision 0068).
        Request::ReadFile { session, path } => {
            let cwd = cwd_of(d, &session)?;
            let policy = d.file_policy();
            let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
                let home = crate::paths::home();
                let at = dent_files::resolve(&path, Path::new(&cwd), &home)
                    .ok_or_else(|| anyhow::anyhow!("{path} is on another computer"))?;
                let o = dent_files::open(&at, &policy).map_err(|s| anyhow::anyhow!(said(&s)))?;
                o.read_all(FILE_CAP)
                    .map_err(|_| anyhow::anyhow!("{path} is too big to show"))
            })
            .await??;
            Reply::File {
                bytes_b64: b64::encode(&bytes),
            }
        }
        Request::Stat { mut files } => {
            files.truncate(STAT_MAX);
            let mut bases = Vec::with_capacity(files.len());
            for f in &files {
                bases.push(match &f.base {
                    Some(b) => Some(b.clone()),
                    None => cwd_of(d, &f.session).ok(),
                });
            }
            let policy = d.file_policy();
            let stats = tokio::task::spawn_blocking(move || {
                let home = crate::paths::home();
                files
                    .iter()
                    .zip(bases)
                    .map(|(f, base)| match base {
                        Some(b) => dent_files::stat(&f.path, Path::new(&b), &home, &policy),
                        None => dent_protocol::FileStat::Unreadable {
                            checked: f.path.clone(),
                            error: format!("no session {}", f.session),
                        },
                    })
                    .collect()
            })
            .await?;
            Reply::Stats { stats }
        }
        Request::Read {
            file,
            offset,
            len,
            expect,
        } => {
            let base = match &file.base {
                Some(b) => b.clone(),
                None => cwd_of(d, &file.session)?,
            };
            let policy = d.file_policy();
            tokio::task::spawn_blocking(move || {
                let home = crate::paths::home();
                match dent_files::read(
                    &file.path,
                    Path::new(&base),
                    &home,
                    &policy,
                    offset,
                    len,
                    expect,
                ) {
                    Ok((stamp, total, bytes)) => Reply::Bytes {
                        stamp,
                        offset,
                        total,
                        bytes_b64: b64::encode(&bytes),
                    },
                    Err((stat, changed)) => Reply::FileGone { stat, changed },
                }
            })
            .await?
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
pub(crate) fn remember_project(d: &Daemon, path: &str) -> Result<()> {
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

/// Write the files message `id` brought with it ([`Request::Send`]'s
/// `files`) under the state folder, and their paths, to attach. Sent again,
/// the same message writes the same files over themselves.
async fn carry(state: &Path, id: &str, files: Vec<dent_protocol::Carried>) -> Result<Vec<String>> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let dir = state
        .join("attachments")
        .join("carried")
        .join(id_folder(id));
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)?;
        let mut paths = Vec::new();
        for (n, f) in files.into_iter().enumerate() {
            let bytes = b64::decode(&f.bytes_b64)
                .with_context(|| format!("{} didn't arrive whole", f.name))?;
            // Each file gets its place in the message ahead of its name, so
            // two of one name (from different folders, or differing only in
            // case on a disk that ignores it) never land on one file.
            let name = format!("{n}-{}", safe_name(&f.name));
            let path = dir.join(name);
            std::fs::write(&path, bytes)?;
            paths.push(path.display().to_string());
        }
        Ok(paths)
    })
    .await?
}

/// Message `id`'s own folder name: the whole id, so ids that differ
/// anywhere (`web/m1`, `cli/m1`) never share a folder; when it has
/// characters a folder name can't, they're replaced and a hash of the id
/// keeps it apart. The same id is always the same folder, so a resend
/// writes over itself.
fn id_folder(id: &str) -> String {
    let plain: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    if plain == id && !plain.is_empty() {
        return plain;
    }
    // FNV-1a: stable across builds, unlike the standard hasher.
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{plain}-{hash:016x}")
}

/// The last part of a name sent from elsewhere, as a file name here:
/// never a path out of its folder.
fn safe_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let last = match last.trim() {
        "" | "." | ".." => "file",
        s => s,
    };
    // Disks take names of up to 255 bytes: a longer one keeps its end (the
    // extension) and as much of its start as fits.
    if last.len() <= 200 {
        return last.to_string();
    }
    let ext = last
        .rfind('.')
        .filter(|&i| last.len() - i <= 16)
        .map_or("", |i| &last[i..]);
    let mut cut = 200 - ext.len();
    while !last.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{ext}", &last[..cut])
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
                        Live::Models(agent, models) => Out::Models {
                            models: models.clone(),
                            agent: agent.clone(),
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
