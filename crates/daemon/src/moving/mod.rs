//! Moving a session to another machine ([decision 0064]): the source's
//! states and fence (`state`), the git snapshot (`git`), packing (`pack`)
//! and arriving (`accept`), and here the requests a carrier makes of each
//! side. Every request is answered so it can be asked again: a carrier
//! stopped at any point starts over with the same move id.
//!
//! [decision 0064]: ../../../../docs/decisions/0064-moving-sessions.md

pub mod accept;
pub mod git;
pub mod pack;
pub mod state;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use dent_protocol::{Body, GitRemote, MoveManifest, Reply, b64};

use crate::daemon::Daemon;
use crate::session::Op;
use state::Step;

/// A session's move as kept, if it has one.
pub fn current(store: &crate::store::Store, session: &str) -> Option<state::Move> {
    serde_json::from_str(&store.get(&move_key(session)).ok()??).ok()
}

/// Refuse what the move fences, for what's done outside the session's own
/// task (forking it, a side question).
pub fn check(store: &crate::store::Store, session: &str) -> anyhow::Result<()> {
    match state::fence(current(store, session).as_ref()) {
        Some(why) => anyhow::bail!(why),
        None => Ok(()),
    }
}

/// Where a session's move is kept in the store.
pub fn move_key(session: &str) -> String {
    format!("move/{session}")
}

/// A move that ended (called off, failed, kept here): its id isn't used
/// again, as its package is gone and the session has moved on since.
pub fn ended_key(id: &str) -> String {
    format!("move-ended/{id}")
}

/// One move's work at a time, whichever carrier asks: packing, writing and
/// accepting the same id never overlap. (Keyed `clone:URL`, one clone of a
/// repository at a time.)
async fn lock_move(id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    type Locks = std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>;
    static LOCKS: std::sync::LazyLock<std::sync::Mutex<Locks>> =
        std::sync::LazyLock::new(Default::default);
    let m = LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(id.to_string())
        .or_default()
        .clone();
    m.lock_owned().await
}

/// The ignored files a project cloned for a move took from the machine
/// the session left, by the clone's folder.
pub fn carried_key(project: &str) -> String {
    format!("move-carried/{project}")
}

/// What a session that arrived here tells its agent first: where it came
/// from. Kept until said.
pub fn arrived_key(session: &str) -> String {
    format!("moved/{session}")
}

/// The most a part is read or written in one go.
pub const CHUNK: u32 = 512 * 1024;

fn scratch() -> PathBuf {
    crate::paths::home().join("dent/scratch")
}

/// The local repository with one of `remotes`, from the projects this
/// machine knows: its main checkout.
async fn local_repo(d: &Daemon, remotes: &[GitRemote]) -> Option<PathBuf> {
    let projects = crate::server::project_list(d).await.ok()?;
    let remotes = remotes.to_vec();
    tokio::task::spawn_blocking(move || {
        projects.into_iter().find_map(|p| {
            let path = PathBuf::from(&p.path);
            let here = git::remotes(&path);
            here.iter()
                .any(|h| remotes.iter().any(|r| accept::same_remote(&h.url, &r.url)))
                .then_some(path)
        })
    })
    .await
    .ok()
    .flatten()
}

impl Daemon {
    /// `BringProject`: clone a project here for a session moving in, by
    /// itself; asked again, how it's going. One here already (cloned
    /// before a restart, or another way) is said at once.
    pub async fn bring_project(
        self: &Arc<Self>,
        id: &str,
        bring: Option<dent_protocol::Bring>,
    ) -> Result<Reply> {
        if !pack::valid_id(id) {
            bail!("{id:?} isn't a move id");
        }
        let done = |path: PathBuf| Reply::ProjectMade {
            path: path.display().to_string(),
            warning: None,
        };
        {
            let mut jobs = crate::daemon::lock(&self.clones);
            match jobs.get(id) {
                Some(None) => return Ok(Reply::Cloning),
                // Done, and still there: said again. Gone since, it's
                // cloned afresh.
                Some(Some(Ok(r))) if matches!(r, Reply::ProjectMade { path, .. } if Path::new(path).exists()) =>
                {
                    return Ok(r.clone());
                }
                Some(Some(Ok(_))) => {
                    jobs.remove(id);
                }
                // Failed: said once, and tried afresh when asked again.
                Some(Some(Err(why))) => {
                    let why = why.clone();
                    jobs.remove(id);
                    bail!("{why}");
                }
                None => {}
            }
        }
        let Some(bring) = bring else {
            bail!("no such clone here");
        };
        if let Some(root) = local_repo(self, &bring.remotes).await {
            return Ok(done(root));
        }
        crate::daemon::lock(&self.clones).insert(id.to_string(), None);
        let (d, id) = (self.clone(), id.to_string());
        tokio::spawn(async move {
            let got = d.clone_for_move(&id, bring).await;
            crate::daemon::lock(&d.clones).insert(id, Some(got));
        });
        Ok(Reply::Cloning)
    }

    async fn clone_for_move(
        self: &Arc<Self>,
        id: &str,
        mut b: dent_protocol::Bring,
    ) -> Result<Reply, String> {
        // Two sessions of one repository moving in at once clone it once:
        // the second, its turn come, finds the first's checkout.
        let url = b
            .remotes
            .first()
            .map(|r| accept::remote_key(&r.url))
            .unwrap_or_default();
        let _turn = lock_move(&format!("clone:{url}")).await;
        if let Some(root) = local_repo(self, &b.remotes).await {
            return Ok(Reply::ProjectMade {
                path: root.display().to_string(),
                warning: None,
            });
        }
        // Tests hold clones open, so two overlap.
        if std::env::var_os("DENT_TEST_SLOW_CLONE").is_some() {
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        }
        // One folder per move, kept from its first try: a try cut short
        // (a restart) is made again there, not beside it as `-2`.
        let at = format!("move-clone/{id}");
        let kept = self.store().get(&at).ok().flatten();
        let dest = match kept {
            Some(p) => PathBuf::from(p),
            None => {
                let p = crate::projects::free_in_root(&crate::projects::slug(&b.name))
                    .map_err(|e| format!("{e:#}"))?;
                self.store()
                    .put(&at, &p.display().to_string())
                    .map_err(|e| format!("{e:#}"))?;
                p
            }
        };
        let staging = crate::bring::staging(&self.paths.state, &format!("move-{id}"));
        let files = std::mem::take(&mut b.files);
        let carried: Vec<String> = files.iter().map(|f| f.name.clone()).collect();
        // Cloned on its default branch: the moving session brings its own,
        // and lands in a worktree on it.
        let session_branch = b.branch.take();
        let (to, st) = (dest.clone(), staging.clone());
        let got = tokio::task::spawn_blocking(move || {
            crate::bring::stage(&st, &files)?;
            let got = crate::bring::clone_into(&to, &b, &st)?;
            // Out on the session's branch, the checkout lets go of it, so
            // the session lands on its own with what it has that the
            // remote doesn't. Any other branch stays as cloned.
            if session_branch.is_some() && crate::workspace::branch(&to) == session_branch {
                crate::workspace::run(&to, &["checkout", "-q", "--detach"])?;
            }
            Ok(got)
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        let _ = std::fs::remove_dir_all(&staging);
        // A clone that failed leaves nothing: not even the empty folder
        // kept for it (a retry makes it again).
        // Its name forgotten with it, so a retry claims a folder anew and
        // never clones over one another has taken since.
        if got.is_err() {
            let _ = std::fs::remove_dir(&dest);
            let _ = self.store().del(&at);
        }
        let (_, notes) = got?;
        let path = dest.display().to_string();
        crate::server::remember_project(self, &path).map_err(|e| format!("{e:#}"))?;
        // What it took from the machine the session left, for the
        // session's worktree (as `.env` files are).
        let _ = self.store().put(
            &carried_key(&path),
            &serde_json::to_string(&carried).unwrap_or_default(),
        );
        let _ = self.store().del(&at);
        Ok(Reply::ProjectMade {
            path,
            warning: (!notes.is_empty()).then(|| notes.join("; ")),
        })
    }

    /// `MoveHave`: whether the repository is here, and what it has.
    pub async fn move_have(self: &Arc<Self>, remotes: &[GitRemote]) -> Reply {
        match local_repo(self, remotes).await {
            Some(root) => Reply::MoveHave {
                found: true,
                tips: tokio::task::spawn_blocking(move || git::tips(&root))
                    .await
                    .unwrap_or_default(),
            },
            None => Reply::MoveHave {
                found: false,
                tips: Vec::new(),
            },
        }
    }

    /// `MovePack`: start the move (or find where it stands), and once the
    /// session is quiet and fenced, pack it.
    pub async fn move_pack(
        self: &Arc<Self>,
        session: &str,
        id: &str,
        to: &str,
        to_machine: &str,
        have: Option<Vec<String>>,
    ) -> Result<Reply> {
        if !pack::valid_id(id) {
            bail!("{id:?} isn't a move id");
        }
        let Some(have) = have else {
            let row = self.store().session(session)?.context("no such session")?;
            let (remotes, root, branch) = tokio::task::spawn_blocking(move || {
                // dent's scratch folder moves as no repository, as packing
                // treats it, even where it sits inside a checkout.
                let cwd = Path::new(&row.cwd);
                if cwd.starts_with(scratch()) {
                    return (Vec::new(), String::new(), None);
                }
                // Named by its main checkout, where its ignored files are,
                // as the app names a session's project.
                git::repo_of(cwd)
                    .map(|(root, _)| {
                        let main = crate::workspace::main_checkout(&root).unwrap_or(root.clone());
                        let branch = crate::workspace::branch(cwd);
                        (git::remotes(&root), main.display().to_string(), branch)
                    })
                    .unwrap_or_default()
            })
            .await?;
            return Ok(Reply::MoveRemotes {
                remotes,
                root,
                branch,
            });
        };
        self.command(
            session,
            Op::Move(Step::Start {
                id: id.into(),
                to: to.into(),
                to_machine: to_machine.into(),
                busy: false,
            }),
        )
        .await?;
        let m = current(&self.store(), session).context("the move ended")?;
        if m.state == dent_protocol::MoveState::Pending {
            return Ok(Reply::MoveWaiting);
        }
        // Gone already, by this move: what it was packed as, so a carrier
        // that lost the answer finishes the same way.
        if m.state == dent_protocol::MoveState::Departed {
            if m.id == id
                && let Ok(manifest) = pack::read_manifest(&pack::dir(&self.paths.state, id))
            {
                return Ok(Reply::MovePacked {
                    manifest: Box::new(manifest),
                });
            }
            bail!("it moved to {} already", m.to);
        }
        let _one = lock_move(id).await;
        let row = self.store().session(session)?.context("no such session")?;
        let events = self.store().session_events(session)?;
        let driver = crate::agent::named(Some(&row.agent))?;
        let (state, machine, name, id2) = (
            self.paths.state.clone(),
            self.net.id(),
            self.machine.clone(),
            id.to_string(),
        );
        let manifest = tokio::task::spawn_blocking(move || {
            pack::pack(
                &pack::Session {
                    state: &state,
                    machine: &machine,
                    machine_name: &name,
                    row: &row,
                    driver,
                    events: &events,
                    scratch: &scratch(),
                },
                &id2,
                &have,
            )
        })
        .await?;
        match manifest {
            Ok(manifest) => Ok(Reply::MovePacked {
                manifest: Box::new(manifest),
            }),
            // Nothing was sent: it works here again, saying why.
            Err(why) => {
                let _ = self
                    .command(
                        session,
                        Op::Move(Step::NotAccepted {
                            id: id.into(),
                            why: why.clone(),
                        }),
                    )
                    .await;
                bail!("{why}")
            }
        }
    }

    /// `MoveRead`: a ranged read of a packed part. The first read marks the
    /// move as maybe sent: from now on only Continue here undoes it.
    pub async fn move_read(
        self: &Arc<Self>,
        id: &str,
        part: &str,
        offset: u64,
        len: u32,
    ) -> Result<Reply> {
        if !pack::valid_id(id) {
            bail!("{id:?} isn't a move id");
        }
        let dir = pack::dir(&self.paths.state, id);
        let m = pack::read_manifest(&dir).context("no packed move by that id")?;
        if !m.parts.iter().any(|p| p.name == part) {
            bail!("{part:?} isn't part of this move");
        }
        // Only while it's going ahead: called off, or kept here, nothing of
        // it leaves (or the other machine would hold a second live copy).
        let going = current(&self.store(), &m.from_session).is_some_and(|c| {
            c.id == id && matches!(c.state, dent_protocol::MoveState::Departing { .. })
        });
        if !going {
            bail!("this move isn't going ahead any more");
        }
        if current(&self.store(), &m.from_session)
            .is_some_and(|c| c.state == dent_protocol::MoveState::Departing { sent: false })
        {
            self.command(&m.from_session, Op::Move(Step::Sending { id: id.into() }))
                .await?;
        }
        let path = dir.join(part);
        let len = len.min(CHUNK) as u64;
        let (bytes, total) =
            tokio::task::spawn_blocking(move || -> std::io::Result<(Vec<u8>, u64)> {
                use std::io::{Read, Seek, SeekFrom};
                let mut f = std::fs::File::open(&path)?;
                let total = f.metadata()?.len();
                f.seek(SeekFrom::Start(offset.min(total)))?;
                let mut buf = Vec::new();
                f.take(len).read_to_end(&mut buf)?;
                Ok((buf, total))
            })
            .await??;
        Ok(Reply::MoveBytes {
            offset,
            total,
            bytes_b64: b64::encode(&bytes),
        })
    }

    /// `MoveIn`: a move arriving, or arrived.
    pub fn move_in(&self, m: &MoveManifest) -> Result<Reply> {
        if let Some(session) = accept::accepted(&self.paths.state, &m.id) {
            return Ok(Reply::Created { session });
        }
        let have = accept::begin(&self.paths.state, m).map_err(anyhow::Error::msg)?;
        Ok(Reply::MoveParts { have })
    }

    pub async fn move_write(
        &self,
        id: &str,
        part: &str,
        offset: u64,
        bytes_b64: &str,
    ) -> Result<Reply> {
        if !pack::valid_id(id) {
            bail!("{id:?} isn't a move id");
        }
        let data = b64::decode(bytes_b64).context("not base64")?;
        let _one = lock_move(id).await;
        let (state, id, part) = (self.paths.state.clone(), id.to_string(), part.to_string());
        let have =
            tokio::task::spawn_blocking(move || accept::write(&state, &id, &part, offset, &data))
                .await?
                .map_err(anyhow::Error::msg)?;
        Ok(Reply::MoveWrote { have })
    }

    /// `MoveEnded`, on the machine it left: arrived (departed), or not
    /// accepted (it works here again).
    pub async fn move_ended(
        self: &Arc<Self>,
        session: &str,
        id: &str,
        to_session: Option<String>,
        not_accepted: Option<String>,
    ) -> Result<()> {
        let step = match (to_session, not_accepted) {
            (Some(to_session), _) => Step::Arrived {
                id: id.into(),
                to_session,
            },
            (None, Some(why)) => Step::NotAccepted { id: id.into(), why },
            (None, None) => bail!("say how the move ended"),
        };
        self.command(session, Op::Move(step)).await
    }

    pub async fn move_stop(
        self: &Arc<Self>,
        session: &str,
        id: &str,
        continue_here: bool,
    ) -> Result<()> {
        let step = if continue_here {
            Step::ContinueHere { id: id.into() }
        } else {
            Step::Cancel { id: id.into() }
        };
        self.command(session, Op::Move(step)).await
    }

    /// `MoveAccept`: make the session here from what arrived.
    pub async fn move_accept(self: &Arc<Self>, id: &str) -> Result<Reply> {
        if !pack::valid_id(id) {
            bail!("{id:?} isn't a move id");
        }
        let _one = lock_move(id).await;
        let state = self.paths.state.clone();
        if let Some(session) = accept::accepted(&state, id) {
            return Ok(Reply::Created { session });
        }
        // Said not accepted once (and so working again where it was): the
        // same every time it's asked.
        if let Ok(why) = std::fs::read_to_string(accept::inbox(&state, id).join("refused")) {
            return Ok(Reply::MoveNotAccepted { why });
        }
        let m = accept::manifest(&state, id).map_err(anyhow::Error::msg)?;
        let session = derived_session(&m);
        // Made already by an accept cut short after the import: it's this
        // move's, so it's said as accepted now.
        if self.store().session(&session)?.is_some() {
            mark_accepted(&state, id, &session)?;
            return Ok(Reply::Created { session });
        }
        let not = |why: String| -> Result<Reply> {
            let _ = std::fs::write(accept::inbox(&state, id).join("refused"), &why);
            Ok(Reply::MoveNotAccepted { why })
        };
        // A part still to come, or one damaged on the way (sent again): not
        // a refusal; the carrier carries on.
        let (s2, m2) = (state.clone(), m.clone());
        if let Err(why) = tokio::task::spawn_blocking(move || accept::verify(&s2, &m2)).await? {
            bail!("{why}");
        }
        let Ok(driver) = crate::agent::named(Some(&m.agent)) else {
            return not(format!("{} isn't an agent this machine has", m.agent));
        };
        // Where it lands.
        let parts = accept::inbox(&state, id).join("parts");
        let (cwd, applied) = match &m.repo {
            None => (scratch(), None),
            Some(r) => {
                let Some(root) = local_repo(self, &r.remotes).await else {
                    return not("this machine doesn't have the project; bring it here first".into());
                };
                let name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let leaf = if r.branch.is_empty() {
                    "moved".to_string()
                } else {
                    r.branch.replace('/', "-")
                };
                let new = crate::workspace::worktrees_root()
                    .join(&name)
                    .join(landing_name(&leaf, id));
                // One an accept cut short made is this move's, wherever it
                // was put (an older daemon named it otherwise): made again.
                let landing = accept::inbox(&state, id).join("landing");
                if let Some(made) = std::fs::read_to_string(&landing)
                    .ok()
                    .map(PathBuf::from)
                    // Only ever one of dent's own folders.
                    .filter(|p| p.starts_with(crate::workspace::worktrees_root()) && p.exists())
                {
                    let root2 = root.clone();
                    tokio::task::spawn_blocking(move || {
                        let _ = crate::workspace::one_at_a_time(&root2, || {
                            crate::workspace::run(
                                &root2,
                                &["worktree", "remove", "--force", &made.display().to_string()],
                            )
                        });
                        let _ = std::fs::remove_dir_all(&made);
                    })
                    .await?;
                }
                if let Some(p) = new.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::write(&landing, new.display().to_string())?;
                let carried: Vec<String> = self
                    .store()
                    .get(&carried_key(&root.display().to_string()))
                    .ok()
                    .flatten()
                    .and_then(|v| serde_json::from_str(&v).ok())
                    .unwrap_or_default();
                let (r2, id2, root2, new2, bundle) = (
                    r.clone(),
                    id.to_string(),
                    root.clone(),
                    new.clone(),
                    parts.join("git.bundle"),
                );
                let applied = tokio::task::spawn_blocking(move || {
                    let a = git::apply(&root2, &r2, &id2, &bundle, &new2)?;
                    // Its settings and secrets here (`.env`), as a new
                    // worktree gets: the checkout's, which a project
                    // cloned for it took from the machine it left, with
                    // whatever else that clone took.
                    crate::workspace::copy_env(&root2, &new2);
                    crate::workspace::copy_files(&root2, &new2, &carried);
                    Ok::<_, String>(a)
                })
                .await?;
                match applied {
                    Ok(a) => (
                        if r.subdir.is_empty() {
                            new
                        } else {
                            new.join(&r.subdir)
                        },
                        Some(a),
                    ),
                    Err(why) => return not(why),
                }
            }
        };
        let cwd_s = cwd.display().to_string();
        // Tests stand in for a daemon dying here: between the worktree and
        // the import, the hardest place to pick up from.
        if std::env::var_os("DENT_TEST_DIE_IN_ACCEPT").is_some() {
            if let Some(a) = applied {
                a.keep();
            }
            std::process::exit(9);
        }
        // The agent's conversation, under a new id of its own.
        let agent_to = derived_uuid(&format!("{}/{}", m.from_machine, m.id));
        let files = state.join("attachments").join(&session);
        // Without the transcript (it couldn't be found there), there's
        // nothing to put in place: the first message there finds no
        // conversation, and starts one given what was said, as text.
        let carried = m.parts.iter().any(|p| p.name == "transcript.jsonl");
        let placed = {
            let (parts, from, to, cwd, files) = (
                parts.clone(),
                m.agent_session.clone(),
                agent_to.clone(),
                cwd_s.clone(),
                files.clone(),
            );
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                if !from.is_empty() && carried {
                    driver
                        .place_conversation(&parts, &from, &to, &cwd)
                        .map_err(|e| format!("{e:#}"))?;
                }
                // What dent saved for it.
                let from_files = parts.join("files");
                for f in pack::files_under(&from_files) {
                    let rel = f.strip_prefix(&from_files).map_err(|e| e.to_string())?;
                    let to = files.join(rel);
                    if let Some(p) = to.parent() {
                        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
                    }
                    std::fs::copy(&f, &to).map_err(|e| e.to_string())?;
                }
                Ok(())
            })
            .await?
        };
        if let Err(why) = placed {
            return not(why);
        }
        // dent's record of it, as this machine's.
        let events = match history(&parts, &m, &files) {
            Ok(e) => e,
            Err(why) => return not(why),
        };
        let mut bodies = vec![(
            crate::daemon::now_ms(),
            Body::Created {
                agent: m.agent.clone(),
                cwd: cwd_s.clone(),
                model: m.model.clone(),
                permission: m.permission,
                title: m.title.clone(),
            },
        )];
        bodies.extend(events);
        let now = crate::daemon::now_ms();
        bodies.push((
            now,
            Body::Agent {
                agent_session: (!m.agent_session.is_empty()).then(|| agent_to.clone()),
                version: None,
            },
        ));
        // On a branch of its own here: said, to you and to the agent.
        let instead = applied.as_ref().and_then(|a| a.instead()).cloned();
        let from = if m.from_name.is_empty() {
            &m.from_machine
        } else {
            &m.from_name
        };
        bodies.push((
            now,
            Body::Notice {
                text: match &instead {
                    Some((name, why)) => {
                        format!("Moved here from {from}, onto a new branch, {name}: {why}")
                    }
                    None => format!("Moved here from {from}"),
                },
            },
        ));
        bodies.push((
            now,
            Body::Status {
                status: dent_protocol::Status::Done,
            },
        ));
        let arrived = serde_json::json!({
            "from_machine": m.from_machine,
            "from_cwd": m.cwd,
            "to_cwd": cwd_s,
            "branch": instead.as_ref().map(|(name, why)| {
                format!("Its branch here is {name}, not {}: {why}.", m.repo.as_ref().map_or("", |r| r.branch.as_str()))
            }),
        })
        .to_string();
        let kv = vec![
            (arrived_key(&session), arrived),
            (crate::daemon::titled_key(&session), "1".to_string()),
        ];
        self.import(&session, &bodies, &kv)?;
        // In: the worktree is the session's now, whatever happens next.
        if let Some(a) = applied {
            a.keep();
        }
        mark_accepted(&state, id, &session)?;
        // Taken: what arrived is in place now, only its record is kept.
        let _ = std::fs::remove_dir_all(&parts);
        let row = self
            .store()
            .session(&session)?
            .context("session row missing")?;
        let cwd = row.cwd.clone();
        crate::session::start(self.clone(), row, Vec::new());
        // Where it works in git here, said now: clients name it by its
        // project, not its new worktree's folder.
        self.note_workspace(&session, &cwd).await;
        Ok(Reply::Created { session })
    }
}

/// Packages of moves that ended a week ago or more go: kept that long so
/// a carrier asking again gets its answer.
pub fn sweep(state: &Path, store: &crate::store::Store) {
    let week = std::time::Duration::from_secs(7 * 24 * 3600);
    // Files waiting for a clone that never finished (a restart, or the
    // move called off): a day on, they go, secrets and all.
    for e in std::fs::read_dir(state.join("bring"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("move-"))
    {
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= std::time::Duration::from_secs(24 * 3600));
        if stale {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    for e in std::fs::read_dir(state.join("moves"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= week);
        let Ok(m) = pack::read_manifest(&e.path()) else {
            continue;
        };
        let live = current(store, &m.from_session).is_some_and(|c| {
            c.id == m.id
                && matches!(
                    c.state,
                    dent_protocol::MoveState::Pending | dent_protocol::MoveState::Departing { .. }
                )
        });
        if old && !live {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// The folder a move of branch `leaf` lands in: the same for every try
/// of one move, and told apart by the id's end, its random part (moves
/// begun within a minute or so share their start).
fn landing_name(leaf: &str, id: &str) -> String {
    let c: Vec<char> = id.chars().collect();
    let tail: String = c[c.len().saturating_sub(8)..].iter().collect();
    format!("{leaf}-{tail}")
}

/// The moved session's id here, the same for every try of one move.
fn derived_session(m: &MoveManifest) -> String {
    let h = fnv(&format!("{}/{}/{}", m.from_machine, m.from_session, m.id));
    format!("s{h:016x}m")
}

/// A version-4-shaped id from `seed`, the same each time.
fn derived_uuid(seed: &str) -> String {
    let (a, b) = (fnv(seed), fnv(&format!("{seed}#")));
    let h = format!("{a:016x}{b:016x}");
    format!(
        "{}-{}-4{}-a{}-{}",
        &h[..8],
        &h[8..12],
        &h[13..16],
        &h[17..20],
        &h[20..32]
    )
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn mark_accepted(state: &Path, id: &str, session: &str) -> Result<()> {
    let file = accept::inbox(state, id).join("accepted");
    let tmp = file.with_extension("part");
    std::fs::write(&tmp, session)?;
    std::fs::File::open(&tmp)?.sync_all()?;
    std::fs::rename(&tmp, &file)?;
    Ok(())
}

/// The events that came, as this machine's: what the session was made with
/// and which agent conversation it ran are said anew, and the files dent
/// saved for it are where they are here.
fn history(parts: &Path, m: &MoveManifest, files: &Path) -> Result<Vec<(u64, Body)>, String> {
    let text = std::fs::read_to_string(parts.join("events.jsonl")).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let to = files.display().to_string();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // The files dent saved for it are here now: each string naming one,
        // and only those (a whole folder name, not a prefix of another).
        if !m.files_from.is_empty() {
            rebase(&mut v, &m.files_from, &to);
        }
        #[derive(serde::Deserialize)]
        struct Line {
            at: u64,
            body: Body,
        }
        let Ok(l) = serde_json::from_value::<Line>(v) else {
            // A kind of event this daemon doesn't know (from a newer one):
            // left out rather than refused.
            continue;
        };
        // What's this machine's to say anew: what it was made with, the
        // agent's conversation, how it stands, its move, where it works in
        // git and its pull request; and no "carry on at" from there.
        if matches!(
            l.body,
            Body::Created { .. }
                | Body::Agent { .. }
                | Body::Status { .. }
                | Body::Move { .. }
                | Body::Resume { .. }
                | Body::Workspace { .. }
                | Body::PullRequest { .. }
        ) {
            continue;
        }
        out.push((l.at, l.body));
    }
    Ok(out)
}

/// Every string in `v` that is `from` or under it (`from/…`) made to start
/// with `to` instead.
fn rebase(v: &mut serde_json::Value, from: &str, to: &str) {
    match v {
        serde_json::Value::String(s) => {
            if s == from {
                *s = to.to_string();
            } else if let Some(rest) = s.strip_prefix(from).filter(|r| r.starts_with('/')) {
                *s = format!("{to}{rest}");
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(|x| rebase(x, from, to)),
        serde_json::Value::Object(o) => o.values_mut().for_each(|x| rebase(x, from, to)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    /// Two moves begun the same minute, of the same branch, land apart;
    /// one move tried again lands where it did.
    #[test]
    fn moves_begun_together_land_apart() {
        use dent_client_core::carry::new_id;
        let now = 1_761_200_000_000;
        let (a, b) = (
            new_id(now, 0x1111_2222_3333),
            new_id(now + 900, 0x4444_5555_6666),
        );
        assert_eq!(a[..8], b[..8]);
        assert_ne!(
            super::landing_name("main", &a),
            super::landing_name("main", &b)
        );
        assert_eq!(
            super::landing_name("main", &a),
            super::landing_name("main", &a)
        );
        assert_eq!(super::landing_name("moved", "mv1"), "moved-mv1");
    }

    #[test]
    fn saved_files_are_found_where_they_are_now_and_only_those() {
        let mut v = serde_json::json!({"at": 1, "body": {"kind": "user", "attachments":
            ["/s/attachments/s1/a.png", "/s/attachments/s12/b.png", "/s/attachments/s1"],
            "text": "see /s/attachments/s1/a.png"}});
        super::rebase(&mut v, "/s/attachments/s1", "/t/att/x\"q");
        assert_eq!(v["body"]["attachments"][0], "/t/att/x\"q/a.png");
        assert_eq!(v["body"]["attachments"][1], "/s/attachments/s12/b.png");
        assert_eq!(v["body"]["attachments"][2], "/t/att/x\"q");
        // Words that mention a path aren't paths.
        assert_eq!(v["body"]["text"], "see /s/attachments/s1/a.png");
    }
}
