//! A fake agent: writes known output at a set pace and echoes stdin, so the
//! holder can be checked byte for byte without spending any usage.

use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

#[derive(Default, Clone)]
pub struct Opts {
    pub id: String,
    pub lines: usize,
    pub every_us: u64,
    /// Bytes per line, newline included.
    pub size: usize,
    /// Write this many raw bytes (every value, no final newline) instead.
    pub binary: usize,
    /// Each line carries the time it was written, for latency.
    pub stamp: bool,
    /// Answer each stdin line, and exit when stdin ends.
    pub echo: bool,
    pub ignore_term: bool,
    /// Start a long `sleep` in the same group, as an agent's tool would.
    pub child: bool,
    /// Leave a `sleep` holding stdout open after exiting.
    pub linger: bool,
    pub exit: i32,
}

pub fn line(id: &str, i: usize, size: usize) -> Vec<u8> {
    let mut l = format!("{id} {i:08} ").into_bytes();
    let fill = b'a' + (i % 26) as u8;
    while l.len() + 1 < size {
        l.push(fill);
    }
    l.push(b'\n');
    l
}

pub fn expected(id: &str, lines: usize, size: usize) -> Vec<u8> {
    (0..lines).flat_map(|i| line(id, i, size)).collect()
}

pub fn binary(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

pub fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

pub fn run(o: Opts) -> i32 {
    if o.ignore_term {
        // SAFETY: setting a signal to ignored has no preconditions.
        unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
    }
    if o.child
        && let Ok(c) = std::process::Command::new("sleep")
            .arg("1000")
            .stdout(std::process::Stdio::null())
            .spawn()
    {
        eprintln!("child {}", c.id());
    }
    let mut out = std::io::stdout().lock();
    let echo = o.echo.then(|| {
        std::thread::spawn(|| {
            let mut stdin = std::io::stdin().lock();
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match stdin.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                let text = String::from_utf8_lossy(&buf);
                let head: String = text.trim_end().chars().take(40).collect();
                let mut o = std::io::stdout().lock();
                let _ = writeln!(o, "got {}: {head}", buf.len());
                let _ = o.flush();
            }
        })
    });
    if o.binary > 0 {
        for chunk in binary(o.binary).chunks(64 * 1024) {
            if out.write_all(chunk).is_err() {
                return 141;
            }
        }
    }
    let start = Instant::now();
    for i in 0..o.lines {
        let l = if o.stamp {
            format!("{} {i}\n", now_us()).into_bytes()
        } else {
            line(&o.id, i, o.size)
        };
        if out.write_all(&l).is_err() {
            return 141;
        }
        if o.every_us > 0 {
            let _ = out.flush();
            // Pace against the start, so slow writes don't add up.
            let due = start + Duration::from_micros(o.every_us * (i as u64 + 1));
            if let Some(d) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
        }
    }
    let _ = out.flush();
    drop(out);
    if let Some(e) = echo {
        let _ = e.join();
    }
    if o.linger {
        let _ = std::process::Command::new("sleep").arg("30").spawn();
    }
    o.exit
}
