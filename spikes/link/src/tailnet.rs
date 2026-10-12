//! What Tailscale knows: who owns the machine at an address, and which
//! machines are in the account. The real version asks the local Tailscale
//! app; the simulated one stands in when Tailscale is off or there is only
//! one machine to test with.

use std::net::IpAddr;
use std::process::Command;

pub trait Tailnet: Send + Sync {
    /// The Tailscale login that owns this device, if Tailscale is up.
    fn me(&self) -> Option<String>;
    /// The login owning the device that connected from `addr`.
    fn owner(&self, addr: IpAddr, port: u16) -> Option<String>;
    /// Addresses of the other devices in the account.
    fn peers(&self) -> Vec<(String, IpAddr)>;
}

/// The local Tailscale app, through its CLI.
pub struct Cli;

impl Cli {
    fn json(args: &[&str]) -> Option<serde_json::Value> {
        let out = Command::new("tailscale").args(args).output().ok()?;
        out.status
            .success()
            .then(|| serde_json::from_slice(&out.stdout).ok())
            .flatten()
    }
}

impl Tailnet for Cli {
    fn me(&self) -> Option<String> {
        let s = Self::json(&["status", "--json"])?;
        if s["BackendState"] != "Running" {
            return None;
        }
        let uid = s["Self"]["UserID"].to_string();
        s["User"][&uid]["LoginName"].as_str().map(str::to_string)
    }

    fn owner(&self, addr: IpAddr, port: u16) -> Option<String> {
        let w = Self::json(&["whois", "--json", &format!("{addr}:{port}")])?;
        w["UserProfile"]["LoginName"].as_str().map(str::to_string)
    }

    fn peers(&self) -> Vec<(String, IpAddr)> {
        let Some(s) = Self::json(&["status", "--json"]) else {
            return Vec::new();
        };
        s["Peer"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
            .filter_map(|p| {
                let ip = p["TailscaleIPs"][0].as_str()?.parse().ok()?;
                Some((p["HostName"].as_str()?.to_string(), ip))
            })
            .collect()
    }
}

/// A simulated account: this device's login, and the login every
/// connection arriving on the daemon's tailnet listener appears to come
/// from. Lets one Mac play both an own-account and a foreign machine.
pub struct Simulated {
    pub me: String,
    pub peer_owner: String,
}

impl Tailnet for Simulated {
    fn me(&self) -> Option<String> {
        Some(self.me.clone())
    }

    fn owner(&self, _addr: IpAddr, _port: u16) -> Option<String> {
        Some(self.peer_owner.clone())
    }

    fn peers(&self) -> Vec<(String, IpAddr)> {
        Vec::new()
    }
}
