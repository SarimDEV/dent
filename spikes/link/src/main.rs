//! Spike 05: devices find, pair with and stream from each other directly,
//! over the LAN or Tailscale, with no server in between.
//!
//! `cargo run -p spike-link -- test` runs every scenario against a second
//! daemon on this machine, standing in for another device.

mod channel;
mod client;
mod daemon;
mod discovery;
mod harness;
mod identity;
mod pair;
mod proto;
mod proxy;
mod tailnet;

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
            })
            .await
        }
        Some("test") => harness::run().await,
        Some("tailscale") => {
            use tailnet::Tailnet;
            let t = tailnet::Cli;
            println!("me: {:?}", t.me());
            println!("peers: {:?}", t.peers());
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: spike-link daemon --state DIR [--name N] [--lan ADDR] [--tailnet ADDR] | test | tailscale"
            );
            Ok(())
        }
    }
}
