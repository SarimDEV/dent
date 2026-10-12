//! Finding daemons on the local network with Bonjour (mDNS/DNS-SD).

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

pub const SERVICE: &str = "_dent._tcp.local.";

#[derive(Debug, Clone)]
pub struct Found {
    pub name: String,
    pub id: String,
    pub addrs: Vec<SocketAddr>,
}

/// Announce a daemon. Keep the returned handle alive to stay announced.
pub fn advertise(name: &str, id: &str, port: u16) -> Result<ServiceDaemon> {
    let mdns = ServiceDaemon::new()?;
    // The machine's network addresses, not loopback: what another machine
    // on the LAN can actually reach.
    let ips: Vec<IpAddr> = if_addrs::get_if_addrs()?
        .into_iter()
        .filter(|i| !i.is_loopback() && i.ip().is_ipv4())
        .map(|i| i.ip())
        .collect();
    let info = ServiceInfo::new(
        SERVICE,
        &format!("{name}-{id}"),
        &format!("dent-{id}.local."),
        &ips[..],
        port,
        &[("id", id), ("name", name)][..],
    )?;
    mdns.register(info)?;
    Ok(mdns)
}

/// Daemons announcing themselves, collected for up to `wait`, or until
/// `want` (a daemon id) shows up.
pub fn browse(wait: Duration, want: Option<&str>) -> Result<Vec<Found>> {
    let mdns = ServiceDaemon::new()?;
    let rx = mdns.browse(SERVICE)?;
    let deadline = Instant::now() + wait;
    let mut found: Vec<Found> = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = rx.recv_timeout(left) else {
            break;
        };
        if let ServiceEvent::ServiceResolved(info) = event {
            let id = info
                .get_property_val_str("id")
                .unwrap_or_default()
                .to_string();
            let name = info
                .get_property_val_str("name")
                .unwrap_or_default()
                .to_string();
            let mut addrs: Vec<SocketAddr> = info
                .get_addresses_v4()
                .into_iter()
                .map(|ip| SocketAddr::new(IpAddr::V4(ip), info.get_port()))
                .collect();
            addrs.sort();
            found.retain(|f| f.id != id);
            let hit = want == Some(id.as_str());
            found.push(Found { name, id, addrs });
            if hit {
                break;
            }
        }
    }
    let _ = mdns.shutdown();
    Ok(found)
}
