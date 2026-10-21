//! The dent command-line client for scripts and agents.
//!
//! Remote machines ([remote-plan.md], stage 1):
//! - `dent machine add HOST:PORT [--code CODE]` pairs with a machine (no
//!   code is needed for one on your own Tailscale account) and saves it.
//! - `dent machines` lists saved machines; `dent machine remove NAME`
//!   forgets one.
//! - `dent watch [NAME...] [--secs N]` connects to saved machines and
//!   follows them, printing every change of state, route and catch-up.
//!
//! Its own key and saved machines live in the client state folder
//! (`DENT_CLIENT_STATE`, else beside the daemon's state, in `client/`).
//!
//! [remote-plan.md]: ../../docs/remote-plan.md

mod host;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use dent_client_core::machine::{Machine, Note, State};
use dent_link::Keys;
use dent_protocol::{Envelope, Out, PROTOCOL, Reply, Request, encode};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Saved {
    name: String,
    id: String,
    /// The machine's public key, hex.
    key: String,
    /// Routes to try, best first: the ones added by hand, then those the
    /// machine reported.
    routes: Vec<String>,
    #[serde(default)]
    learned: Vec<String>,
}

fn state_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("DENT_CLIENT_STATE") {
        return PathBuf::from(p);
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/dent/client")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"))
            .join("dent/client")
    }
}

fn device_keys() -> Result<Keys> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    let p = dir.join("device.key");
    if let Ok(t) = std::fs::read_to_string(&p) {
        return Keys::from_hex(&t).context("the device key is damaged");
    }
    let k = Keys::generate()?;
    write_private(&p, k.to_hex().as_bytes())?;
    Ok(k)
}

fn write_private(path: &std::path::Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn load() -> Vec<Saved> {
    std::fs::read_to_string(state_dir().join("machines.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(all: &[Saved]) -> Result<()> {
    std::fs::create_dir_all(state_dir())?;
    write_private(
        &state_dir().join("machines.json"),
        serde_json::to_string_pretty(all)?.as_bytes(),
    )
}

fn device_name() -> String {
    std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "dent cli".into())
}

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
        _ => {
            eprintln!(
                "usage: dent machine add HOST:PORT [--code CODE] | dent machines | dent machine remove NAME | dent watch [NAME...] [--secs N]"
            );
            Ok(())
        }
    }
}

async fn add(args: &[String]) -> Result<()> {
    let addr = args.first().context("dent machine add HOST:PORT")?.clone();
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
            Note::Up { trust, .. } => {
                println!("connected ({trust:?})");
                let _ = ask_tx.send(host::Ask::Send(hello_line(1)));
            }
            Note::Data { line, .. } => {
                let Ok(env) = serde_json::from_slice::<Envelope<Out>>(&line) else {
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
            Note::State(
                s @ (State::Removed | State::Incompatible { .. } | State::TrustedElsewhere { .. }),
            ) => break Err(anyhow::anyhow!("{s:?}")),
            Note::State(State::Unreachable { why }) => eprintln!("  {why}; retrying"),
            _ => {}
        }
    };
    let _ = ask_tx.send(host::Ask::Stop);
    let _ = task.await;
    let (name, id, addresses) = result?;
    // The key the actor learned (paired) or saw (account trust).
    let key = key.unwrap_or_default();
    let mut all = load();
    all.retain(|s| s.id != id);
    all.push(Saved {
        name: name.clone(),
        id: id.clone(),
        key: hex::encode(&key),
        routes: vec![addr],
        learned: addresses,
    });
    save(&all)?;
    println!("added {name} ({id})");
    Ok(())
}

fn remove(name: &str) -> Result<()> {
    let mut all = load();
    let before = all.len();
    all.retain(|s| s.name != name && s.id != name);
    if all.len() == before {
        bail!("no machine called {name}");
    }
    save(&all)?;
    println!("forgot {name}");
    Ok(())
}

async fn watch(args: &[String]) -> Result<()> {
    let secs: Option<u64> = flag(args, "--secs").and_then(|s| s.parse().ok());
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
        let known = hex::decode(&s.key).ok().filter(|k| !k.is_empty());
        let mut routes = s.routes.clone();
        for r in &s.learned {
            if !routes.contains(r) {
                routes.push(r.clone());
            }
        }
        let m = Machine::new(&device_name(), keys.clone(), known, routes);
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
                let Ok(env) = serde_json::from_slice::<Envelope<Out>>(&line) else {
                    continue;
                };
                match env.body {
                    Out::Reply {
                        reply: Reply::Welcome { epoch, version, .. },
                    } => {
                        if tally[i].epoch.as_ref().is_none_or(|e| *e != epoch)
                            && tally[i].epoch.is_some()
                        {
                            println!("{t:7.2}s  {name}: log epoch changed");
                        }
                        tally[i].epoch = Some(epoch);
                        println!("{t:7.2}s  {name}: dent {version}");
                    }
                    Out::Events { events } => {
                        for e in events {
                            if e.seq > tally[i].last + 1 && tally[i].last > 0 {
                                tally[i].gaps += 1;
                            }
                            tally[i].last = tally[i].last.max(e.seq);
                            tally[i].events += 1;
                        }
                    }
                    Out::CaughtUp { seq } => println!("{t:7.2}s  {name}: caught up at {seq}"),
                    Out::Resync { .. } => {
                        tally[i].last = 0;
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
