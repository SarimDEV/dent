//! The dent command-line client for scripts and agents.
//!
//! Remote machines ([remote-plan.md], stage 1):
//! - `dent machine add HOST:PORT [--code CODE]` pairs with a machine (no
//!   code is needed for one on your own Tailscale account) and saves it.
//! - `dent machines` lists saved machines; `dent machine remove NAME`
//!   forgets one.
//! - `dent watch [NAME...] [--secs N]` connects to saved machines and
//!   follows them, printing every change of state, route and catch-up.
//! - `dent ask NAME TEXT [--file PATH]... [--cwd DIR]` starts a session on
//!   a saved machine (in its scratch folder unless `--cwd`), its files
//!   carried along, and prints the new session's id. With `--bring DIR`,
//!   the project at DIR on this machine is cloned there first, with its
//!   ignored environment files (`.env`), and the session works in it.
//!
//! - `dent call NAME JSON` sends one request (as JSON, `{"op": …}`) to a
//!   saved machine and prints its reply, for scripts and for looking into
//!   what a machine says.
//! - `dent get NAME SESSION PATH [OUT]` copies a file a session names from
//!   its machine (by what the session wrote: `~`, relative, absolute), in
//!   ranges a few at a time, and says how fast it came.
//! - `dent wake NAME` sends a sleeping machine a Wake-on-LAN packet, from
//!   the same network, to the cards its last welcome named.
//!
//! Its own key and saved machines live in the client state folder
//! (`DENT_CLIENT_STATE`, else `client/` beside the daemon's state).
//!
//! [remote-plan.md]: ../../docs/remote-plan.md

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use dent_client_core::machine::{Machine, Note, State};
use dent_protocol::{Envelope, Out, PROTOCOL, Reply, Request, encode};
use dent_remote::host;
use dent_remote::store::{Saved, device_keys, device_name, load, save};
use tokio::sync::mpsc;

mod moving;

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn hello_line(n: u64) -> Vec<u8> {
    let mut l = encode(
        n,
        &Request::Hello {
            proto: PROTOCOL,
            client: "dent cli".into(),
        },
    );
    l.pop();
    l
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("machine") => match args.get(1).map(String::as_str) {
            Some("add") => add(&args[2..]).await,
            Some("remove") => remove(args.get(2).context("dent machine remove NAME")?),
            _ => bail!("dent machine add HOST:PORT [--code CODE] | remove NAME"),
        },
        Some("machines") => {
            for m in load() {
                println!("{}  {}  {}", m.name, m.id, m.routes.join(", "));
            }
            Ok(())
        }
        Some("watch") => watch(&args[1..]).await,
        Some("ask") => ask(&args[1..]).await,
        Some("call") => call(&args[1..]).await,
        Some("move") => moving::run(&args[1..]).await,
        Some("get") => get(&args[1..]).await,
        Some("wake") => {
            let name = args.get(1).context("dent wake NAME")?;
            let m = saved(name)?;
            if m.wake.is_empty() {
                bail!("{name} hasn't said how to wake it: connect to it once while it's awake");
            }
            let n = dent_remote::wake::wake(&m.wake)?;
            println!("sent {n} wake-up packets to {name}");
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: dent machine add HOST:PORT [--code CODE] | dent machines | dent machine remove NAME | dent watch [NAME...] [--secs N] | dent ask NAME TEXT [--file PATH]... [--cwd DIR | --bring DIR] | dent wake NAME | dent call NAME JSON | dent get NAME SESSION PATH [OUT] | dent move NAME SESSION --to NAME [--id ID] [--clone]"
            );
            Ok(())
        }
    }
}

async fn add(args: &[String]) -> Result<()> {
    let typed = args.first().context("dent machine add HOST[:PORT]")?;
    // Not an address at all: said at once, not tried for half a minute.
    let Some(addr) = dent_client_core::machine::address(typed) else {
        bail!(
            "{typed:?} isn't an address: give a host, and a port if it isn't 7480, like 192.168.1.20 or mini.tail1234.ts.net:7480"
        );
    };
    let code = flag(args, "--code");
    let keys = device_keys()?;
    let m = Machine::new(&device_name(), keys, None, vec![addr.clone()]);
    let (ask_tx, ask_rx) = mpsc::unbounded_channel();
    let (note_tx, mut notes) = mpsc::unbounded_channel();
    let task = tokio::spawn(host::run(m, ask_rx, note_tx));
    let deadline = tokio::time::sleep(Duration::from_secs(30));
    tokio::pin!(deadline);
    let mut key: Option<Vec<u8>> = None;
    let result = loop {
        let n = tokio::select! {
            n = notes.recv() => n,
            _ = &mut deadline => break Err(anyhow::anyhow!("gave up after 30 s")),
        };
        let Some(n) = n else {
            break Err(anyhow::anyhow!("stopped"));
        };
        match n {
            Note::State(State::NeedsCode { pairing_open }) => match &code {
                Some(c) => {
                    let _ = ask_tx.send(host::Ask::Pair(c.clone()));
                }
                None => {
                    break Err(anyhow::anyhow!(
                        "{addr} needs a pairing code{}. Run `dent-daemon pair` there and add again with --code",
                        if pairing_open {
                            " (one is showing there)"
                        } else {
                            ""
                        }
                    ));
                }
            },
            Note::PairFailed(r) => {
                break Err(anyhow::anyhow!(
                    "{}",
                    dent_client_core::machine::refusal_words(&r)
                ));
            }
            Note::Paired { key: k } => key = Some(k),
            Note::Up { trust, key: k, .. } => {
                key.get_or_insert(k);
                println!("connected ({trust:?})");
                let _ = ask_tx.send(host::Ask::Send(hello_line(1)));
            }
            Note::Data { line, .. } => {
                let Some(env) = dent_protocol::read_out(&line) else {
                    continue;
                };
                if let Out::Reply {
                    reply:
                        Reply::Welcome {
                            machine,
                            id,
                            addresses,
                            ..
                        },
                } = env.body
                {
                    break Ok((machine, id, addresses));
                }
            }
            Note::State(State::Removed) => {
                break Err(anyhow::anyhow!(
                    "this device was removed from {addr}; pair again there with `dent-daemon pair` and --code"
                ));
            }
            Note::State(State::Incompatible { daemon, client }) => {
                let ranges = format!(
                    "it speaks protocols {}–{}, this dent {}–{}",
                    daemon.0, daemon.1, client.0, client.1
                );
                break Err(
                    if dent_client_core::machine::machine_is_older(daemon, client) {
                        anyhow::anyhow!(
                            "{addr} runs an older dent ({ranges}): on it, build the latest and run `dent-daemon update` with the build's folder"
                        )
                    } else {
                        anyhow::anyhow!(
                            "this dent is older than {addr}'s ({ranges}): build the latest dent here"
                        )
                    },
                );
            }
            Note::State(State::TrustedElsewhere { routes }) => {
                break Err(anyhow::anyhow!(
                    "{addr} trusts this device only over {}: add it by that address instead",
                    routes.join(", ")
                ));
            }
            Note::State(State::Unreachable { why }) => eprintln!("  {why}; retrying"),
            _ => {}
        }
    };
    let _ = ask_tx.send(host::Ask::Stop);
    let _ = task.await;
    let (name, id, addresses) = result?;
    // The key the actor learned (paired) or saw (account trust).
    let key = key.unwrap_or_default();
    dent_remote::store::upsert(&Saved {
        name: name.clone(),
        id: id.clone(),
        key: hex::encode(&key),
        routes: vec![addr],
        learned: addresses,
        wake: Vec::new(),
    })?;
    println!("added {name} ({id})");
    Ok(())
}

fn remove(name: &str) -> Result<()> {
    let gone = saved(name)?;
    let mut all = load();
    all.retain(|s| s.id != gone.id);
    save(&all)?;
    println!("forgot {} ({})", gone.name, gone.id);
    Ok(())
}

/// The saved machine `name` names: its id, or its name when only one
/// machine has it. Two of one name are never guessed between.
fn saved(name: &str) -> Result<Saved> {
    let all = load();
    if let Some(m) = all.iter().find(|s| s.id == name) {
        return Ok(m.clone());
    }
    let named: Vec<&Saved> = all.iter().filter(|s| s.name == name).collect();
    match named.as_slice() {
        [] => bail!("no saved machine called {name}"),
        [one] => Ok((*one).clone()),
        many => bail!(
            "{} machines are called {name}: use its id ({})",
            many.len(),
            many.iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

async fn ask(args: &[String]) -> Result<()> {
    let usage = "dent ask NAME TEXT [--file PATH]... [--cwd DIR | --bring DIR]";
    let mut plain = Vec::new();
    let mut files = Vec::new();
    let mut cwd = String::new();
    let mut from = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--file" => files.push(it.next().context(usage)?.clone()),
            "--cwd" => cwd = it.next().context(usage)?.clone(),
            "--bring" => from = Some(it.next().context(usage)?.clone()),
            _ => plain.push(a.clone()),
        }
    }
    let [name, text] = plain.as_slice() else {
        bail!(usage);
    };
    let saved = saved(name)?;
    if from.is_some() && !cwd.is_empty() {
        bail!("--bring makes the folder it works in: leave out --cwd");
    }
    let bring = match from {
        Some(dir) => Some(pack(&dir)?),
        None => None,
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let remote = dent_remote::remote::spawn(
        &tokio::runtime::Handle::current(),
        device_keys()?,
        "dent cli",
        saved,
        move |e| {
            let _ = tx.send(e);
        },
    );
    const REQ: u64 = 10;
    remote.send(
        REQ,
        Request::Create {
            agent: None,
            cwd,
            worktree: None,
            model: None,
            permission: Default::default(),
            effort: None,
            id: format!("cli-{}", std::process::id()),
            text: text.clone(),
            attachments: files,
            files: Vec::new(),
            bring,
        },
    );
    let wait = tokio::time::sleep(Duration::from_secs(60));
    tokio::pin!(wait);
    loop {
        tokio::select! {
            e = rx.recv() => match e {
                Some(dent_remote::remote::Event::Out { req: REQ, out }) => {
                    remote.stop();
                    match out {
                        Out::Reply { reply: Reply::Created { session } } => {
                            println!("{session}");
                            return Ok(());
                        }
                        Out::Error { message } => bail!(message),
                        o => bail!("unexpected answer: {o:?}"),
                    }
                }
                Some(dent_remote::remote::Event::State(s)) => eprintln!("{name}: {s:?}"),
                // What its welcome said (routes, how to wake it), kept.
                Some(dent_remote::remote::Event::Saved(s)) => {
                    let _ = dent_remote::store::upsert(&s);
                }
                Some(_) => {}
                None => bail!("the connection ended"),
            },
            _ = &mut wait => bail!("{name} didn't answer within a minute"),
        }
    }
}

/// What another machine needs to bring the project at `dir` there, asked
/// of this machine's daemon: its remotes and branch, and its ignored
/// environment files.
fn pack(dir: &str) -> Result<dent_protocol::Bring> {
    let dir = std::fs::canonicalize(dir)
        .with_context(|| format!("{dir} isn't a folder here"))?
        .display()
        .to_string();
    let Reply::Pack { pack } = local(Request::Pack { dir: dir.clone() })? else {
        bail!("unexpected answer from this machine's daemon");
    };
    let pack = pack.with_context(|| format!("{dir} isn't in a Git repository"))?;
    if pack.remotes.is_empty() {
        bail!("{dir} has no remote another machine can clone from");
    }
    if let Some(missing) = pack.missing() {
        eprintln!("note: {missing}");
    }
    let paths: Vec<String> = pack
        .ignored
        .iter()
        .filter(|i| i.carry)
        .map(|i| i.path.clone())
        .collect();
    let files = if paths.is_empty() {
        Vec::new()
    } else {
        for p in &paths {
            eprintln!("bringing {p}");
        }
        match local(Request::ReadFiles { dir, paths })? {
            Reply::Carried { files } => files,
            _ => bail!("unexpected answer from this machine's daemon"),
        }
    };
    Ok(dent_protocol::Bring {
        name: pack.name,
        remotes: pack.remotes,
        branch: pack.branch,
        files,
    })
}

/// One request to this machine's daemon, over its socket.
fn local(r: Request) -> Result<Reply> {
    use std::io::{BufRead, Write};
    let sock = dent_protocol::daemon_socket(&dent_remote::store::daemon_state());
    let mut s = std::os::unix::net::UnixStream::connect(&sock)
        .with_context(|| format!("dent isn't running on this machine ({})", sock.display()))?;
    let mut hello = hello_line(1);
    hello.push(b'\n');
    s.write_all(&hello)?;
    s.write_all(&encode(2, &r))?;
    for line in std::io::BufReader::new(s.try_clone()?).lines() {
        let env: Envelope<Out> = serde_json::from_str(&line?)?;
        // A refused hello is why, not the "hello first" that follows it.
        if let (1, Out::Error { message }) = (env.req, &env.body) {
            bail!(
                "this machine's daemon won't talk to this dent: {message}. One older than this dent \
                 is replaced when the dent app opens; if this dent is the older, build the latest"
            );
        }
        if env.req != 2 {
            continue;
        }
        return match env.body {
            Out::Reply { reply } => Ok(reply),
            Out::Error { message } => bail!(message),
            o => bail!("unexpected answer: {o:?}"),
        };
    }
    bail!("this machine's daemon hung up")
}

/// The next answer to a request, past the feed's events.
async fn next_reply(
    rx: &mut mpsc::UnboundedReceiver<dent_remote::remote::Event>,
) -> Result<(u64, Out)> {
    // A minute with no answer, however much the link says meanwhile
    // (tries, failures): the machine isn't answering, and that's said.
    let by = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut last = None;
    loop {
        match tokio::time::timeout_at(by, rx.recv()).await {
            Ok(Some(dent_remote::remote::Event::Out { req, out })) if req != 0 => {
                return Ok((req, out));
            }
            Ok(Some(dent_remote::remote::Event::State(s))) => last = Some(s),
            Ok(Some(_)) => {}
            Ok(None) => bail!("the connection ended"),
            Err(_) => match last {
                Some(dent_client_core::machine::State::Unreachable { why }) => {
                    bail!("no answer within a minute: {why}")
                }
                _ => bail!("no answer within a minute"),
            },
        }
    }
}

async fn get(args: &[String]) -> Result<()> {
    let (name, session, path, dest) = match args {
        [n, s, p] => (n, s, p, p.rsplit('/').next().unwrap_or(p).to_string()),
        [n, s, p, o] => (n, s, p, o.clone()),
        _ => bail!("dent get NAME SESSION PATH [OUT]"),
    };
    let saved = saved(name)?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let remote = dent_remote::remote::spawn_asking(
        &tokio::runtime::Handle::current(),
        device_keys()?,
        "dent cli",
        saved,
        move |e| {
            let _ = tx.send(e);
        },
    );
    let file = dent_protocol::FileRef {
        session: session.clone(),
        path: path.clone(),
        base: None,
    };
    remote.send(
        9,
        Request::Stat {
            files: vec![file.clone()],
        },
    );
    let (_, out) = next_reply(&mut rx).await?;
    let (real, stamp) = match out {
        Out::Reply {
            reply: Reply::Stats { stats },
        } => match stats.into_iter().next() {
            Some(dent_protocol::FileStat::File { path, stamp, .. }) => (path, stamp),
            other => bail!("{other:?}"),
        },
        Out::Error { message } => bail!(message),
        o => bail!("unexpected answer: {o:?}"),
    };
    let total = stamp.size;
    let mut bytes = vec![0u8; total as usize];
    let step = dent_protocol::READ_MAX as u64;
    let mut next = 0u64;
    let mut got = 0u64;
    let mut inflight = std::collections::HashMap::new();
    let mut n = 10u64;
    let started = Instant::now();
    while got < total {
        while inflight.len() < 4 && next < total {
            remote.send(
                n,
                Request::Read {
                    file: file.clone(),
                    offset: next,
                    len: dent_protocol::READ_MAX,
                    expect: Some(stamp),
                },
            );
            inflight.insert(n, next);
            n += 1;
            next += step;
        }
        let (req, out) = next_reply(&mut rx).await?;
        let Some(at) = inflight.remove(&req) else {
            continue;
        };
        match out {
            Out::Reply {
                reply: Reply::Bytes { bytes_b64, .. },
            } => {
                let b = dent_protocol::b64::decode(&bytes_b64).context("bad bytes")?;
                bytes[at as usize..at as usize + b.len()].copy_from_slice(&b);
                got += b.len() as u64;
            }
            Out::Reply {
                reply: Reply::FileGone { changed, .. },
            } => bail!(if changed {
                "it changed while it came"
            } else {
                "it's gone"
            }),
            o => bail!("unexpected answer: {o:?}"),
        }
    }
    remote.stop();
    std::fs::write(&dest, &bytes)?;
    let secs = started.elapsed().as_secs_f64();
    eprintln!(
        "{real}: {} bytes in {secs:.2} s ({:.1} MB/s) to {dest}",
        total,
        total as f64 / secs / 1e6
    );
    Ok(())
}

async fn call(args: &[String]) -> Result<()> {
    let [name, json] = args else {
        bail!("dent call NAME JSON");
    };
    let req: Request = serde_json::from_str(json).context("not a request")?;
    let saved = saved(name)?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let remote = dent_remote::remote::spawn_asking(
        &tokio::runtime::Handle::current(),
        device_keys()?,
        "dent cli",
        saved,
        move |e| {
            let _ = tx.send(e);
        },
    );
    const REQ: u64 = 10;
    let started = Instant::now();
    remote.send(REQ, req);
    let wait = tokio::time::sleep(Duration::from_secs(60));
    tokio::pin!(wait);
    loop {
        tokio::select! {
            e = rx.recv() => match e {
                Some(dent_remote::remote::Event::Out { req: REQ, out }) => {
                    remote.stop();
                    eprintln!("answered in {:?}", started.elapsed());
                    println!("{}", serde_json::to_string(&out)?);
                    return Ok(());
                }
                Some(_) => {}
                None => bail!("the connection ended"),
            },
            _ = &mut wait => bail!("{name} didn't answer within a minute"),
        }
    }
}

async fn watch(args: &[String]) -> Result<()> {
    let secs: Option<u64> = flag(args, "--secs")
        .map(|s| s.parse())
        .transpose()
        .context("--secs takes a number of seconds")?;
    let names: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && (*i == 0 || args[i - 1] != "--secs"))
        .map(|(_, a)| a)
        .collect();
    let all: Vec<Saved> = load()
        .into_iter()
        .filter(|s| names.is_empty() || names.iter().any(|n| **n == s.name || **n == s.id))
        .collect();
    if all.is_empty() {
        bail!("no saved machines to watch; add one with `dent machine add`");
    }
    let keys = device_keys()?;
    let start = Instant::now();
    let (note_tx, mut notes) = mpsc::unbounded_channel::<(usize, Note)>();
    let mut asks = Vec::new();
    for (i, s) in all.iter().enumerate() {
        let m = Machine::new(&device_name(), keys.clone(), s.known_key(), s.all_routes());
        let (ask_tx, ask_rx) = mpsc::unbounded_channel();
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(host::run(m, ask_rx, tx));
        let note_tx = note_tx.clone();
        tokio::spawn(async move {
            while let Some(n) = rx.recv().await {
                if note_tx.send((i, n)).is_err() {
                    break;
                }
            }
        });
        asks.push(ask_tx);
    }
    drop(note_tx);
    #[derive(Default)]
    struct Tally {
        events: u64,
        last: u64,
        gaps: u64,
        ups: u64,
        epoch: Option<String>,
    }
    let mut tally: Vec<Tally> = all.iter().map(|_| Tally::default()).collect();
    let end = secs.map(|s| tokio::time::Instant::now() + Duration::from_secs(s));
    loop {
        let item = match end {
            Some(e) => tokio::select! {
                n = notes.recv() => n,
                _ = tokio::time::sleep_until(e) => None,
                _ = tokio::signal::ctrl_c() => None,
            },
            None => tokio::select! {
                n = notes.recv() => n,
                _ = tokio::signal::ctrl_c() => None,
            },
        };
        let Some((i, n)) = item else { break };
        let t = start.elapsed().as_secs_f64();
        let name = &all[i].name;
        match n {
            Note::State(s) => println!("{t:7.2}s  {name}: {s:?}"),
            Note::Up {
                generation,
                addr,
                trust,
                ..
            } => {
                tally[i].ups += 1;
                println!("{t:7.2}s  {name}: up via {addr} (link {generation}, {trust:?})");
                let _ = asks[i].send(host::Ask::Send(hello_line(1)));
                let mut sub = encode(
                    2,
                    &Request::Subscribe {
                        after: tally[i].last,
                        epoch: tally[i].epoch.clone(),
                    },
                );
                sub.pop();
                let _ = asks[i].send(host::Ask::Send(sub));
            }
            Note::Data { line, .. } => {
                let Some(env) = dent_protocol::read_out(&line) else {
                    continue;
                };
                match env.body {
                    Out::Reply {
                        reply: Reply::Welcome { epoch, version, .. },
                    } => {
                        // The epoch goes with the cursor: a new one is taken
                        // only from the resync that resets it, or the next
                        // link would resume an old cursor in the new log.
                        match &tally[i].epoch {
                            None => tally[i].epoch = Some(epoch),
                            Some(e) if *e != epoch => {
                                println!("{t:7.2}s  {name}: log epoch changed")
                            }
                            Some(_) => {}
                        }
                        println!("{t:7.2}s  {name}: dent {version}");
                    }
                    Out::Events { events } => {
                        for e in events {
                            if e.seq > tally[i].last + 1 && tally[i].last > 0 {
                                tally[i].gaps += 1;
                                println!("{t:7.2}s  {name}: gap, {} to {}", tally[i].last, e.seq);
                            }
                            tally[i].last = tally[i].last.max(e.seq);
                            tally[i].events += 1;
                        }
                    }
                    Out::CaughtUp { seq } => println!("{t:7.2}s  {name}: caught up at {seq}"),
                    Out::Resync { epoch } => {
                        tally[i].last = 0;
                        tally[i].epoch = Some(epoch);
                        println!("{t:7.2}s  {name}: resync");
                    }
                    _ => {}
                }
            }
            Note::DifferentMachine { addr } => {
                println!("{t:7.2}s  {name}: a different machine answered at {addr}")
            }
            Note::Rtt { .. } | Note::Paired { .. } | Note::PairFailed(_) => {}
        }
    }
    for a in &asks {
        let _ = a.send(host::Ask::Stop);
    }
    for (s, t) in all.iter().zip(&tally) {
        println!(
            "{}: links {}, events {}, last seq {}, gaps {}",
            s.name, t.ups, t.events, t.last, t.gaps
        );
    }
    Ok(())
}
