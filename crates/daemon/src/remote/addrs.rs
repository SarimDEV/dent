//! Which addresses to tell devices about, and which route a connection
//! came in on.
//!
//! Advertised addresses are hints a device learns routes from
//! ([remote-plan.md], connection list): never loopback or link-local, never
//! bridges, containers, VMs or VPN tunnels (Tailscale is reported
//! separately, from Tailscale itself), never 198.18.0.0/15.
//!
//! [remote-plan.md]: ../../../docs/remote-plan.md

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// How a connection reached this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// From this machine itself, or a forward on it (Tailscale Serve, an
    /// SSH tunnel). Never trusted by account.
    Loopback,
    /// A Tailscale address, as Tailscale itself confirms.
    Tailscale,
    /// A private LAN address.
    Lan,
    /// Anything else (a public address, another VPN).
    Other,
}

/// Classifies `ip`, given this machine's Tailscale addresses.
pub fn route(ip: IpAddr, tailscale_peer: bool) -> Route {
    let ip = unmap(ip);
    if ip.is_loopback() {
        return Route::Loopback;
    }
    if tailscale_peer {
        return Route::Tailscale;
    }
    if is_private(ip) {
        return Route::Lan;
    }
    Route::Other
}

/// An IPv4 address carried in IPv6 (dual-stack sockets) as plain IPv4.
pub fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// 100.64.0.0/10, which Tailscale uses but so do other VPNs and carrier NAT:
/// only a hint until Tailscale confirms.
pub fn in_cgnat(ip: IpAddr) -> bool {
    matches!(unmap(ip), IpAddr::V4(v) if v.octets()[0] == 100 && (64..128).contains(&v.octets()[1]))
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private(),
        // Unique local fc00::/7.
        IpAddr::V6(v) => (v.segments()[0] & 0xfe00) == 0xfc00,
    }
}

fn advertisable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            !(v.is_loopback()
                || v.is_link_local()
                || v.is_unspecified()
                || in_cgnat(ip)
                // 198.18.0.0/15: benchmarking, and some VPNs' tunnels.
                || (v.octets()[0] == 198 && (v.octets()[1] & 0xfe) == 18))
                && v.is_private()
        }
        IpAddr::V6(v) => !v.is_loopback() && is_private(IpAddr::V6(v)) && !is_tailscale_v6(v),
    }
}

fn is_tailscale_v6(v: Ipv6Addr) -> bool {
    // fd7a:115c:a1e0::/48
    let s = v.segments();
    s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
}

/// Interface names that never carry the LAN for other devices.
fn virtual_interface(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "lo",
        "utun",
        "ipsec",
        "ppp",
        "tun",
        "tap",
        "wg",
        "docker",
        "br-",
        "veth",
        "virbr",
        "vmnet",
        "vboxnet",
        "vEthernet",
        "podman",
        "cni",
        "flannel",
        "cali",
        "lxcbr",
        "lxdbr",
        "tailscale",
        "awdl",
        "llw",
        "anpi",
        "gif",
        "stf",
        "ap1",
    ];
    // bridgeN on macOS is the Thunderbolt or VM bridge; br0 and vmbr0 on
    // Linux are host bridges that carry the LAN, so they stay.
    (name.starts_with("bridge") && name != "br0") || PREFIXES.iter().any(|p| name.starts_with(p))
}

/// LAN addresses of this machine, best first: interfaces in the order the
/// OS lists them (the primary one first on macOS and Linux), IPv4 first.
pub fn lan_addresses() -> Vec<IpAddr> {
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for (name, ip) in interfaces() {
        if virtual_interface(&name) || !advertisable(ip) {
            continue;
        }
        let list = match ip {
            IpAddr::V4(_) => &mut v4,
            IpAddr::V6(_) => &mut v6,
        };
        if !list.contains(&ip) {
            list.push(ip);
        }
    }
    v4.extend(v6);
    v4
}

/// Every interface address, through getifaddrs.
fn interfaces() -> Vec<(String, IpAddr)> {
    let mut out = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we free below; we only
    // read fields while it's alive.
    unsafe {
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut cur = head;
        while !cur.is_null() {
            let ifa = &*cur;
            let up = (ifa.ifa_flags & libc::IFF_UP as libc::c_uint) != 0;
            if up && !ifa.ifa_addr.is_null() {
                let name = std::ffi::CStr::from_ptr(ifa.ifa_name)
                    .to_string_lossy()
                    .into_owned();
                let family = i32::from((*ifa.ifa_addr).sa_family);
                if family == libc::AF_INET {
                    let sa = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    out.push((
                        name,
                        IpAddr::V4(Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr))),
                    ));
                } else if family == libc::AF_INET6 {
                    let sa = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                    out.push((name, IpAddr::V6(Ipv6Addr::from(sa.sin6_addr.s6_addr))));
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(head);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn routes() {
        assert_eq!(route(ip("127.0.0.1"), false), Route::Loopback);
        assert_eq!(route(ip("::ffff:127.0.0.1"), false), Route::Loopback);
        assert_eq!(
            route(ip("127.0.0.1"), true),
            Route::Loopback,
            "a forward is never Tailscale"
        );
        assert_eq!(route(ip("100.70.46.115"), true), Route::Tailscale);
        assert_eq!(
            route(ip("100.85.0.1"), false),
            Route::Other,
            "100.x without Tailscale's word"
        );
        assert_eq!(route(ip("192.168.64.2"), false), Route::Lan);
        assert_eq!(route(ip("::ffff:192.168.1.5"), false), Route::Lan);
        assert_eq!(route(ip("8.8.8.8"), false), Route::Other);
    }

    #[test]
    fn what_gets_advertised() {
        assert!(advertisable(ip("192.168.0.54")));
        assert!(advertisable(ip("10.0.0.3")));
        assert!(!advertisable(ip("127.0.0.1")));
        assert!(!advertisable(ip("169.254.1.1")));
        assert!(!advertisable(ip("100.74.27.91")));
        assert!(!advertisable(ip("198.18.0.1")));
        assert!(!advertisable(ip("8.8.8.8")));
        assert!(
            !advertisable(ip("fd7a:115c:a1e0::1")),
            "Tailscale's own IPv6 is reported separately"
        );
        assert!(virtual_interface("utun3"));
        assert!(virtual_interface("docker0"));
        assert!(virtual_interface("bridge100"));
        assert!(!virtual_interface("br0"));
        assert!(!virtual_interface("en0"));
        assert!(!virtual_interface("eth0"));
    }

    #[test]
    fn lists_something_sane() {
        for a in lan_addresses() {
            assert!(advertisable(a));
        }
    }
}
