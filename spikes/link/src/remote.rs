//! Real-network runs for the remote plan's stage 0 (docs/remote-tests.md,
//! R-A): pair with a daemon on another machine, then follow a session there
//! under one set of liveness rules while prompts stream, logging every
//! transition and ending with a summary.
//!
//! `spike-link remote pair --state DIR --addr HOST:PORT --code CODE`
//! `spike-link remote follow --state DIR --addrs A,B --session S
//!     [--rules spike|plan] [--secs N] [--prompt-every S]`

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::json;
use tokio::sync::{Mutex, mpsc};

use crate::client::{self, Note, Rules, Tally};
use crate::identity::Identity;

pub async fn run(args: &[String]) -> Result<()> {
    let get = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let state = PathBuf::from(get("--state").context("--state DIR")?);
    std::fs::create_dir_all(&state)?;
    let me = Identity::load(&state, "client")?;
    match args.first().map(String::as_str) {
        Some("pair") => {
            let addr: SocketAddr = get("--addr").context("--addr")?.parse()?;
            let code = get("--code").context("--code")?;
            let mut o = client::open(addr, &me).await?;
            if !o.trusted {
                client::pair(&mut o.ch, &code)
                    .await
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            std::fs::write(state.join("peer-key"), hex::encode(&o.ch.remote))?;
            println!("paired with {} ({})", o.name, o.id);
            Ok(())
        }
        Some("follow") => {
            let addrs: Vec<SocketAddr> = get("--addrs")
                .context("--addrs A,B")?
                .split(',')
                .map(|a| a.parse())
                .collect::<Result<_, _>>()?;
            let mut rules = match get("--rules").as_deref() {
                Some("plan") => Rules::plan(),
                Some("spike") | None => Rules::spike(),
                Some(r) => bail!("unknown rules {r}"),
            };
            if let Some(h) = get("--home-every").and_then(|s| s.parse().ok()) {
                rules.home_every = Some(Duration::from_secs(h));
            }
            let secs = get("--secs").and_then(|s| s.parse().ok()).unwrap_or(60);
            let every = get("--prompt-every")
                .and_then(|s| s.parse().ok())
                .unwrap_or(10);
            let session = get("--session").unwrap_or_else(|| format!("r-{}", std::process::id()));
            follow(&state, me, addrs, rules, session, secs, every).await
        }
        _ => bail!("remote pair|follow"),
    }
}

fn peer_key(state: &Path) -> Result<Vec<u8>> {
    Ok(hex::decode(
        std::fs::read_to_string(state.join("peer-key"))
            .context("not paired: run `remote pair` first")?
            .trim(),
    )?)
}

async fn follow(
    state: &Path,
    me: Identity,
    addrs: Vec<SocketAddr>,
    rules: Rules,
    session: String,
    secs: u64,
    every: u64,
) -> Result<()> {
    let key = peer_key(state)?;
    let tally = Arc::new(Mutex::new(Tally::default()));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let resolve: client::Resolve = {
        let addrs = addrs.clone();
        Arc::new(move || addrs.clone())
    };
    let name = rules.name;
    let f = client::follow_with(
        rules,
        me.clone(),
        key.clone(),
        resolve,
        session.clone(),
        0,
        tally.clone(),
        tx,
    );

    // Prompts on their own connections, each with a fresh id, retried until
    // acknowledged, as the client core's outbox does.
    let sent = Arc::new(Mutex::new((0u64, 0u64)));
    let prompter = {
        let (me, key, addrs, session, sent) = (
            me.clone(),
            key.clone(),
            addrs.clone(),
            session.clone(),
            sent.clone(),
        );
        tokio::spawn(async move {
            let mut n = 0u64;
            loop {
                n += 1;
                let id = format!("{session}-{n}");
                loop {
                    match client::prompt(&me, &key, &addrs, &id, &session, "go").await {
                        Ok(_) => {
                            sent.lock().await.0 += 1;
                            break;
                        }
                        Err(_) => {
                            sent.lock().await.1 += 1;
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(every)).await;
            }
        })
    };

    let start = Instant::now();
    let t = || start.elapsed().as_secs_f64();
    let end = start + Duration::from_secs(secs);
    let mut online: Option<Instant> = None;
    let mut down_since: Option<Instant> = Some(start);
    let mut down_total = Duration::ZERO;
    let mut down_max = Duration::ZERO;
    let mut downs = 0u64;
    let mut last_frame = Instant::now();
    let mut gap_max = Duration::ZERO;
    let mut slow = 0u64;
    let mut events = 0u64;
    let mut rtt_max = 0u64;
    loop {
        let note = tokio::select! {
            n = rx.recv() => n,
            _ = tokio::time::sleep_until(end.into()) => break,
        };
        let Some(note) = note else { break };
        let now = Instant::now();
        let gap = now - last_frame;
        if online.is_some() {
            gap_max = gap_max.max(gap);
            // What a person would see as "Reconnecting…" under the plan.
            if gap >= Duration::from_secs(3) {
                slow += 1;
            }
        }
        last_frame = now;
        match note {
            Note::Up { addr, .. } => {
                if let Some(d) = down_since.take() {
                    let gone = now - d;
                    down_total += gone;
                    down_max = down_max.max(gone);
                }
                online = Some(now);
                println!("{}", json!({"t": t(), "up": addr.to_string()}));
            }
            // A swap moves to a link that is already up, so it isn't a drop.
            Note::Down { reason } if client::SWAP_REASONS.contains(&reason.as_str()) => {
                println!("{}", json!({"t": t(), "swap": reason}));
            }
            Note::Down { reason } => {
                if online.take().is_some() {
                    downs += 1;
                    down_since = Some(now);
                }
                println!("{}", json!({"t": t(), "down": reason}));
            }
            Note::Event(_) => events += 1,
            Note::Rtt(r) => rtt_max = rtt_max.max(r),
            Note::Bye(r) => println!("{}", json!({"t": t(), "bye": r})),
            Note::Denied(r) => println!("{}", json!({"t": t(), "denied": r})),
            Note::Incompatible { proto, min } => {
                println!("{}", json!({"t": t(), "incompatible": [proto, min]}))
            }
            Note::Health(_) => {}
        }
    }
    if let Some(d) = down_since {
        let gone = Instant::now() - d;
        down_total += gone;
        down_max = down_max.max(gone);
    }
    f.abort();
    prompter.abort();
    let tl = tally.lock().await.clone();
    let (acked, retries) = *sent.lock().await;
    println!(
        "{}",
        json!({
            "summary": {
                "rules": name,
                "secs": secs,
                "events": events,
                "gaps": tl.gaps,
                "dups": tl.dups,
                "drops": downs,
                "reconnects": tl.reconnects,
                "swaps": tl.swaps,
                "dials": tl.dials,
                "down_total_s": down_total.as_secs_f64(),
                "down_max_s": down_max.as_secs_f64(),
                "max_frame_gap_s": gap_max.as_secs_f64(),
                "slow_periods_3s": slow,
                "rtt_max_ms": rtt_max,
                "prompts_acked": acked,
                "prompt_retries": retries,
            }
        })
    );
    Ok(())
}
