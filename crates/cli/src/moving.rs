//! `dent move MACHINE SESSION --to MACHINE [--id ID]`: carry a session from
//! one machine to another ([decision 0064]). This device is trusted by
//! both; the machines never talk to each other. Every step asks each side
//! where it stands, so the same command (with the same id) picks up after
//! any interruption, on either machine, the link or here.
//!
//! [decision 0064]: ../../../docs/decisions/0064-moving-sessions.md

use std::time::Duration;

use anyhow::{Context, Result, bail};
use dent_protocol::{Out, Reply, Request};
use dent_remote::remote::{Event, Remote};
use tokio::sync::mpsc;

/// One machine to ask things of, an answer at a time.
struct Asker {
    name: String,
    remote: Remote,
    rx: mpsc::UnboundedReceiver<Event>,
    next: u64,
}

impl Asker {
    fn open(name: &str) -> Result<Self> {
        let saved = super::saved(name)?;
        let (tx, rx) = mpsc::unbounded_channel();
        let remote = dent_remote::remote::spawn_asking(
            &tokio::runtime::Handle::current(),
            dent_remote::store::device_keys()?,
            "dent cli",
            saved,
            move |e| {
                let _ = tx.send(e);
            },
        );
        Ok(Asker {
            name: name.to_string(),
            remote,
            rx,
            next: 100,
        })
    }

    /// Ask, and wait up to `secs` for the answer.
    async fn ask(&mut self, req: Request, secs: u64) -> Result<Reply> {
        self.next += 1;
        let n = self.next;
        self.remote.send(n, req);
        let wait = tokio::time::sleep(Duration::from_secs(secs));
        tokio::pin!(wait);
        loop {
            tokio::select! {
                e = self.rx.recv() => match e {
                    Some(Event::Out { req, out }) if req == n => match out {
                        Out::Reply { reply } => return Ok(reply),
                        Out::Error { message } => bail!("{}: {message}", self.name),
                        other => bail!("{}: unexpected answer {other:?}", self.name),
                    },
                    Some(_) => {}
                    None => bail!("the link to {} ended", self.name),
                },
                _ = &mut wait => bail!("{} didn't answer in {secs} s", self.name),
            }
        }
    }
}

pub async fn run(args: &[String]) -> Result<()> {
    let usage = "dent move MACHINE SESSION --to MACHINE [--id ID]";
    let to_name = super::flag(args, "--to").context(usage)?;
    let id = super::flag(args, "--id").unwrap_or_else(new_id);
    let plain: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && (*i == 0 || !args[i - 1].starts_with("--")))
        .map(|(_, a)| a)
        .collect();
    let [from_name, session] = plain[..] else {
        bail!("{usage}")
    };
    eprintln!(
        "move {id}: {session} on {from_name} to {to_name} (run again with --id {id} to pick up)"
    );
    let mut from = Asker::open(from_name)?;
    let mut to = Asker::open(&to_name)?;
    // Named as this device knows it, which is what the chat on the machine
    // it leaves says it went to.
    let target = super::saved(&to_name)?.name;
    let pack = |have: Option<Vec<String>>| Request::MovePack {
        session: session.to_string(),
        id: id.clone(),
        to: target.clone(),
        have,
    };
    // What the other machine has of the repository, for the bundle to
    // leave out.
    let Reply::MoveRemotes { remotes } = from.ask(pack(None), 30).await? else {
        bail!("{from_name} didn't say the session's remotes");
    };
    let in_repo = !remotes.is_empty();
    let have = match to.ask(Request::MoveHave { remotes }, 60).await? {
        // Said before anything's packed or fenced: nothing to undo.
        Reply::MoveHave { found: false, .. } if in_repo => bail!(
            "{to_name} doesn't have this project; bring it there first, then move the session"
        ),
        Reply::MoveHave { tips, .. } => tips,
        other => bail!("{to_name}: unexpected answer {other:?}"),
    };
    // Packed once the turn (and work left running) ends.
    let manifest = loop {
        match from.ask(pack(Some(have.clone())), 600).await? {
            Reply::MovePacked { manifest } => break manifest,
            Reply::MoveWaiting => {
                eprintln!("  waiting for the turn on {from_name} to end…");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            other => bail!("{from_name}: unexpected answer {other:?}"),
        }
    };
    let total: u64 = manifest.parts.iter().map(|p| p.bytes).sum();
    eprintln!(
        "  packed: {} parts, {} KB",
        manifest.parts.len(),
        total.div_ceil(1024)
    );
    let arrived = match to
        .ask(
            Request::MoveIn {
                manifest: manifest.clone(),
            },
            60,
        )
        .await?
    {
        Reply::Created { session } => Some(session),
        Reply::MoveParts { have } => {
            for p in &manifest.parts {
                let mut at = have
                    .iter()
                    .find(|(n, _)| *n == p.name)
                    .map_or(0, |(_, n)| *n);
                while at < p.bytes {
                    let Reply::MoveBytes {
                        bytes_b64, offset, ..
                    } = from
                        .ask(
                            Request::MoveRead {
                                id: id.clone(),
                                part: p.name.clone(),
                                offset: at,
                                len: 512 * 1024,
                            },
                            120,
                        )
                        .await?
                    else {
                        bail!("{from_name} didn't send {}", p.name);
                    };
                    let Reply::MoveWrote { have } = to
                        .ask(
                            Request::MoveWrite {
                                id: id.clone(),
                                part: p.name.clone(),
                                offset,
                                bytes_b64,
                            },
                            120,
                        )
                        .await?
                    else {
                        bail!("{to_name} didn't take {}", p.name);
                    };
                    if have <= at {
                        bail!("{to_name} stopped taking {} at {at} bytes", p.name);
                    }
                    at = have;
                }
            }
            None
        }
        other => bail!("{to_name}: unexpected answer {other:?}"),
    };
    let outcome = match arrived {
        Some(s) => Ok(s),
        None => match to.ask(Request::MoveAccept { id: id.clone() }, 600).await? {
            Reply::Created { session } => Ok(session),
            Reply::MoveNotAccepted { why } => Err(why),
            other => bail!("{to_name}: unexpected answer {other:?}"),
        },
    };
    // The machine it left hears how it ended.
    let (to_session, not_accepted) = match &outcome {
        Ok(s) => (Some(unqualified(s)), None),
        Err(why) => (None, Some(why.clone())),
    };
    from.ask(
        Request::MoveEnded {
            session: session.to_string(),
            id: id.clone(),
            to_session,
            not_accepted,
        },
        60,
    )
    .await?;
    from.remote.stop();
    to.remote.stop();
    match outcome {
        Ok(s) => {
            println!("moved: {to_name} {}", unqualified(&s));
            Ok(())
        }
        Err(why) => bail!("{to_name} didn't take it: {why}; it works on {from_name} again"),
    }
}

/// A session's id as its own machine names it (a client may prefix the
/// machine).
fn unqualified(s: &str) -> String {
    s.rsplit_once('/').map_or(s, |(_, id)| id).to_string()
}

fn new_id() -> String {
    let mut b = [0u8; 6];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut b);
    }
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!(
        "m{t:x}-{}",
        b.iter().map(|x| format!("{x:02x}")).collect::<String>()
    )
}
