//! What a machine is and how it's doing, read from the OS. macOS first, with
//! Linux fallbacks; anything unreadable is left empty rather than guessed.

use std::process::Command;

use crate::proto::{Agent, Battery, Health, Info, now_ms};

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn sysctl(key: &str) -> Option<String> {
    run("sysctl", &["-n", key])
}

/// Read once at startup. Agent versions come from `--version`, which never
/// talks to the provider.
pub fn info() -> Info {
    let mac = cfg!(target_os = "macos");
    let agents = [("Claude Code", "claude"), ("Codex", "codex")]
        .into_iter()
        .filter_map(|(name, bin)| {
            let out = run(bin, &["--version"])?;
            // "2.1.296 (Claude Code)" or "codex-cli 0.162.1".
            let version = out
                .split_whitespace()
                .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
                .map(str::to_string);
            Some(Agent {
                name: name.into(),
                version,
            })
        })
        .collect();
    Info {
        hostname: run("hostname", &["-s"]).unwrap_or_default(),
        os: if mac { "macOS" } else { std::env::consts::OS }.into(),
        os_version: if mac {
            run("sw_vers", &["-productVersion"])
        } else {
            run("uname", &["-r"])
        }
        .unwrap_or_default(),
        arch: std::env::consts::ARCH.into(),
        model: if mac {
            sysctl("machdep.cpu.brand_string")
        } else {
            None
        }
        .unwrap_or_default(),
        cores: std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(0),
        dent_version: env!("CARGO_PKG_VERSION").into(),
        agents,
        macs: macs(),
        timezone: std::fs::read_link("/etc/localtime")
            .ok()
            .and_then(|p| {
                let s = p.to_string_lossy().to_string();
                s.split("zoneinfo/").nth(1).map(str::to_string)
            })
            .unwrap_or_default(),
    }
}

/// Hardware addresses of interfaces that carry an IPv4 address: the ones
/// another machine reaches this one through.
fn macs() -> Vec<String> {
    let Some(out) = run("ifconfig", &[]) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let (mut ether, mut inet) = (None::<String>, false);
    let mut flush = |ether: &mut Option<String>, inet: &mut bool| {
        if let Some(e) = ether.take()
            && *inet
            && !found.contains(&e)
        {
            found.push(e);
        }
        *inet = false;
    };
    for line in out.lines() {
        if !line.starts_with(['\t', ' ']) {
            flush(&mut ether, &mut inet);
        }
        let l = line.trim();
        if let Some(e) = l.strip_prefix("ether ") {
            ether = Some(e.trim().to_string());
        }
        if l.starts_with("inet ") && !l.starts_with("inet 127.") {
            inet = true;
        }
    }
    flush(&mut ether, &mut inet);
    found
}

/// Measured every few seconds while someone watches.
pub fn health(running: u32) -> Health {
    let load = sysctl("vm.loadavg")
        .and_then(|s| {
            s.trim_matches(|c| c == '{' || c == '}' || c == ' ')
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .or_else(|| {
            std::fs::read_to_string("/proc/loadavg")
                .ok()?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .unwrap_or(0.);
    let mem_total: u64 = sysctl("hw.memsize")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // Used = active + wired + compressed, as Activity Monitor counts it.
    let mem_used = run("vm_stat", &[])
        .map(|s| {
            let page = s
                .lines()
                .next()
                .and_then(|l| l.split("page size of ").nth(1))
                .and_then(|l| l.split_whitespace().next()?.parse::<u64>().ok())
                .unwrap_or(16384);
            let pages = |key: &str| {
                s.lines()
                    .find(|l| l.starts_with(key))
                    .and_then(|l| l.split(':').nth(1))
                    .and_then(|v| v.trim().trim_end_matches('.').parse::<u64>().ok())
                    .unwrap_or(0)
            };
            (pages("Pages active")
                + pages("Pages wired down")
                + pages("Pages occupied by compressor"))
                * page
        })
        .unwrap_or(0);
    let (disk_total, disk_free) = run("df", &["-k", "/"])
        .and_then(|s| {
            let f: Vec<u64> = s
                .lines()
                .nth(1)?
                .split_whitespace()
                .skip(1)
                .take(3)
                .filter_map(|v| v.parse().ok())
                .collect();
            Some((*f.first()? * 1024, *f.get(2)? * 1024))
        })
        .unwrap_or((0, 0));
    let battery = run("pmset", &["-g", "batt"]).and_then(|s| {
        let line = s.lines().find(|l| l.contains("InternalBattery"))?;
        let percent = line
            .split('\t')
            .nth(1)?
            .split('%')
            .next()?
            .trim()
            .parse()
            .ok()?;
        Some(Battery {
            percent,
            charging: line.contains("; charging") || line.contains("; charged"),
            on_battery: s.contains("'Battery Power'"),
        })
    });
    let uptime_s = sysctl("kern.boottime")
        .and_then(|s| {
            s.split("sec = ")
                .nth(1)?
                .split(',')
                .next()?
                .parse::<u64>()
                .ok()
        })
        .map(|boot| (now_ms() / 1000).saturating_sub(boot))
        .unwrap_or(0);
    Health {
        at: now_ms(),
        load,
        mem_used,
        mem_total,
        disk_free,
        disk_total,
        battery,
        running,
        uptime_s,
    }
}

/// A Wake-on-LAN magic packet for `mac` ("aa:bb:cc:dd:ee:ff").
pub fn magic_packet(mac: &str) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = mac
        .split([':', '-'])
        .map(|h| u8::from_str_radix(h, 16).ok())
        .collect::<Option<_>>()?;
    if bytes.len() != 6 {
        return None;
    }
    let mut p = vec![0xFF; 6];
    for _ in 0..16 {
        p.extend_from_slice(&bytes);
    }
    Some(p)
}

/// Send magic packets for each address to `targets` (broadcast addresses).
pub fn wake(macs: &[String], targets: &[std::net::SocketAddr]) -> usize {
    let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") else {
        return 0;
    };
    let _ = sock.set_broadcast(true);
    let mut sent = 0;
    for mac in macs {
        let Some(p) = magic_packet(mac) else { continue };
        for t in targets {
            if sock.send_to(&p, t).is_ok() {
                sent += 1;
            }
        }
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_packet_shape() {
        let p = magic_packet("ba:cf:2b:12:48:06").unwrap();
        assert_eq!(p.len(), 102);
        assert_eq!(&p[..6], &[0xFF; 6]);
        assert_eq!(&p[6..12], &[0xba, 0xcf, 0x2b, 0x12, 0x48, 0x06]);
        assert_eq!(&p[96..], &p[6..12]);
        assert!(magic_packet("nope").is_none());
    }

    #[test]
    fn reads_this_machine() {
        let i = info();
        assert!(!i.os.is_empty() && i.cores > 0);
        let h = health(0);
        assert!(h.at > 0);
    }
}
