//! `dent move MACHINE SESSION --to MACHINE [--id ID] [--clone]`: carry a
//! session from one machine to another ([decision 0064]). With `--clone`,
//! a machine without the project clones it first, with the environment
//! files (`.env`) the app would tick. This device is trusted by
//! both; the machines never talk to each other. Every step asks each side
//! where it stands, so the same command (with the same id) picks up after
//! any interruption, on either machine, the link or here.
//!
//! [decision 0064]: ../../../docs/decisions/0064-moving-sessions.md

use std::time::Duration;

use anyhow::{Context, Result, bail};
use dent_client_core::carry::{Carrier, CloneThere, Ended, Phase, Side, Step};
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
    let usage = "dent move MACHINE SESSION --to MACHINE [--id ID] [--clone]";
    let to_name = super::flag(args, "--to").context(usage)?;
    let id = super::flag(args, "--id").unwrap_or_else(new_id);
    let clone = args.iter().any(|a| a == "--clone");
    // `--to` and `--id` take a value; `--clone` doesn't.
    let plain: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            !a.starts_with("--") && (*i == 0 || !matches!(args[i - 1].as_str(), "--to" | "--id"))
        })
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
    let mut carrier = Carrier::new(session, &id, &target);
    // What cloning takes is asked before anyone knows whether it's
    // needed: what stops a clone doesn't stop a move that needs none.
    if clone {
        match clone_there(&mut from, session).await {
            Ok(c) => carrier = carrier.clone_there(c),
            Err(why) => eprintln!("  can't clone it there if needed: {why:#}"),
        }
    }
    let mut step = carrier.start();
    let mut said = None;
    let outcome = loop {
        let (side, req) = match step {
            Step::Done(r) => break r,
            Step::Ask(side, req) => (side, req),
            Step::Wait(ms, side, req) => {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                (side, req)
            }
        };
        let secs = patience(&req);
        let asker = match side {
            Side::From => &mut from,
            Side::To => &mut to,
        };
        let answer = asker.ask(req, secs).await.map_err(|e| format!("{e:#}"));
        step = carrier.answer(side, answer);
        // Each phase said once.
        let now = match carrier.phase() {
            Phase::Waiting => Some(format!("waiting for the turn on {from_name} to end…")),
            Phase::Sending { total, .. } => Some(format!("sending {} KB", total.div_ceil(1024))),
            Phase::Cloning => Some(format!("cloning it on {to_name}…")),
            Phase::Arriving => Some(format!("arriving on {to_name}…")),
            _ => None,
        };
        if now.is_some() && now != said {
            eprintln!("  {}", now.clone().unwrap_or_default());
            said = now;
        }
    };
    from.remote.stop();
    to.remote.stop();
    match outcome {
        Ok(s) => {
            println!("moved: {to_name} {s}");
            Ok(())
        }
        Err(Ended::NoProject) => bail!(
            "{to_name} doesn't have this project: move it with --clone to clone it there first"
        ),
        Err(Ended::NotAccepted(why)) => {
            bail!("{to_name} didn't take it: {why}; it works on {from_name} again")
        }
        Err(Ended::Stopped { side, why }) => {
            let name = match side {
                Side::From => from_name.as_str(),
                Side::To => to_name.as_str(),
            };
            // A move that ended can't be picked up: it's moved again anew.
            if why.contains("that move ended") {
                bail!("{name}: {why}")
            }
            bail!("{name}: {why} (run again with --id {id} to pick up)")
        }
    }
}

/// What cloning the session's project takes, asked of its machine: where
/// it is, its remotes, and its environment files, ticked as the app ticks
/// them. Asked whether or not the other machine has it; it clones only if
/// not.
async fn clone_there(from: &mut Asker, session: &str) -> Result<CloneThere> {
    let Reply::MoveRemotes { root, branch, .. } = from
        .ask(
            Request::MovePack {
                session: session.to_string(),
                id: "probe".into(),
                to: String::new(),
                to_machine: String::new(),
                have: None,
            },
            60,
        )
        .await?
    else {
        bail!("{}: unexpected answer", from.name);
    };
    if root.is_empty() {
        bail!(
            "{}: the session isn't in a Git repository there (or that dent is too old to say)",
            from.name
        );
    }
    let Reply::Pack { pack } = from.ask(Request::Pack { dir: root.clone() }, 120).await? else {
        bail!("{}: unexpected answer", from.name);
    };
    let pack = pack.with_context(|| format!("{root} isn't a Git repository"))?;
    if pack.remotes.is_empty() {
        bail!("{root} has no remote another machine can clone from: push it somewhere first");
    }
    let ticked: Vec<&dent_protocol::Ignored> = pack.ignored.iter().filter(|i| i.carry).collect();
    let bytes: u64 = ticked.iter().map(|i| i.size).sum();
    if bytes > dent_protocol::CARRIED_MAX as u64 {
        bail!(
            "its environment files come to {} KB, more than the {} KB that can go along",
            bytes.div_ceil(1024),
            dent_protocol::CARRIED_MAX / 1024
        );
    }
    for i in &ticked {
        eprintln!("  bringing {} if it's cloned", i.path);
    }
    Ok(CloneThere {
        dir: root,
        paths: ticked.iter().map(|i| i.path.clone()).collect(),
        bring: dent_protocol::Bring {
            name: pack.name,
            remotes: pack.remotes,
            branch,
            files: Vec::new(),
        },
    })
}

/// How long an answer may take: packing a big repository, or making the
/// session there, takes a while; a chunk or a question doesn't.
fn patience(req: &Request) -> u64 {
    match req {
        Request::MovePack { have: Some(_), .. } | Request::MoveAccept { .. } => 600,
        Request::MoveRead { .. } | Request::MoveWrite { .. } => 120,
        _ => 60,
    }
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
