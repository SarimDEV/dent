//! Spike 05: devices find, pair with and stream from each other directly,
//! over the LAN or Tailscale, with no server in between.
//!
//! `cargo run -p spike-link -- test` runs every scenario against a second
//! daemon on this machine, standing in for another device.

use spike_link::{daemon, harness, tailnet};

use std::path::PathBuf;

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    match args.first().map(String::as_str) {
        Some("daemon") => {
            let tailnet_impl: Box<dyn tailnet::Tailnet> =
                match (get("--sim-me"), get("--sim-owner")) {
                    (Some(me), Some(peer_owner)) => Box::new(tailnet::Simulated { me, peer_owner }),
                    _ => Box::new(tailnet::Cli),
                };
            daemon::run(daemon::Config {
                name: get("--name").unwrap_or_else(|| "daemon".into()),
                state: PathBuf::from(get("--state").context("--state DIR")?),
                lan: get("--lan")
                    .unwrap_or_else(|| "0.0.0.0:7480".into())
                    .parse()?,
                tailnet: get("--tailnet").map(|a| a.parse()).transpose()?,
                tailnet_impl,
                advertise: !args.iter().any(|a| a == "--no-mdns"),
                proto: match get("--proto").and_then(|p| p.parse().ok()) {
                    Some(p) => (p, p),
                    None => (spike_link::proto::PROTO, spike_link::proto::PROTO_MIN),
                },
                pair_ttl: std::time::Duration::from_secs(
                    get("--pair-ttl")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(600),
                ),
                agent_ms: get("--agent-ms").and_then(|s| s.parse().ok()).unwrap_or(30),
                version: get("--version"),
            })
            .await
        }
        // Started by the daemon: the session holder, and the fake agent it
        // runs.
        Some("holder") => {
            let dir = PathBuf::from(get("--dir").context("--dir DIR")?);
            tokio::task::spawn_blocking(move || {
                spike_holder::holder::serve(spike_holder::holder::Options::new(dir))
            })
            .await?
        }
        Some("agent") => daemon::fake_agent(
            &get("--name").unwrap_or_default(),
            get("--ms").and_then(|s| s.parse().ok()).unwrap_or(30),
        ),
        Some("test") => match args.get(1).map(String::as_str) {
            Some("edges") => spike_link::edges::run().await,
            Some("links") => harness::run().await,
            _ => {
                harness::run().await?;
                spike_link::edges::run().await
            }
        },
        Some("tailscale") => {
            use tailnet::Tailnet;
            let t = tailnet::Cli;
            println!("me: {:?}", t.me());
            println!("peers: {:?}", t.peers());
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: spike-link daemon --state DIR [--name N] [--lan ADDR] [--tailnet ADDR] [--proto N] [--pair-ttl S] [--agent-ms MS] [--version V] | test [links|edges] | tailscale"
            );
            Ok(())
        }
    }
}
