//! The holder's wire format. A frame is one line of JSON, followed by `n`
//! raw bytes when `n` is set (stdin to write, or output being sent). Ops are
//! only ever added: the daemon checks `proto` before using a newer one.

use std::io::{BufRead, Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// This holder build's protocol, and the oldest daemon protocol it accepts.
pub const PROTO: u32 = 1;
pub const PROTO_MIN: u32 = 1;

/// Longest header line, and the most bytes one frame may carry. A list of
/// running processes carries every one's command line in its header, and an
/// agent's runs to kilobytes (its prompt and tool config): 64 KB was gone at
/// about 45 agents, which lost them all when the daemon restarted. Writers
/// were never bounded, so a holder already running needs no change.
pub const MAX_HEADER: usize = 16 * 1024 * 1024;
pub const MAX_PAYLOAD: usize = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Envelope<T> {
    /// Echoed on the reply; 0 for output the holder sends unasked.
    #[serde(default)]
    pub req: u64,
    /// Bytes that follow the line.
    #[serde(default)]
    pub n: usize,
    #[serde(flatten)]
    pub body: T,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Req {
    /// Opens the connection and takes over from any earlier daemon.
    Hello {
        proto: u32,
    },
    Spawn {
        id: String,
        argv: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        env: Vec<(String, String)>,
    },
    /// Write the payload to the process's stdin, whole or not at all.
    Write {
        id: String,
    },
    CloseStdin {
        id: String,
    },
    /// Send output from these offsets on, then live, then `exit`.
    Attach {
        id: String,
        out: u64,
        err: u64,
    },
    /// SIGTERM to the process group, SIGKILL after `grace_ms`.
    Stop {
        id: String,
        grace_ms: u64,
    },
    /// Forget an exited process and delete its output.
    Release {
        id: String,
    },
    List,
    /// Start nothing new and exit once every process has ended, so a
    /// newer holder can take over.
    ExitWhenIdle,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Msg {
    Hello {
        proto: u32,
        min: u32,
        version: String,
        pid: u32,
    },
    Ok,
    Spawned {
        pid: u32,
    },
    Procs {
        procs: Vec<Proc>,
    },
    Error {
        msg: String,
    },
    /// Output at byte `off` of stdout (or stderr when `err`).
    Data {
        id: String,
        err: bool,
        off: u64,
    },
    /// Sent after the last byte of output.
    Exit {
        id: String,
        code: Option<i32>,
        signal: Option<i32>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Proc {
    pub id: String,
    pub pid: u32,
    pub argv: Vec<String>,
    /// Milliseconds since the Unix epoch.
    pub started: u64,
    pub out: u64,
    pub err: u64,
    pub exited: bool,
}

pub fn write_frame<T: Serialize>(
    w: &mut impl Write,
    req: u64,
    body: T,
    payload: &[u8],
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(&Envelope {
        req,
        n: payload.len(),
        body,
    })?;
    line.push(b'\n');
    // One write per frame where possible, so frames never interleave.
    line.extend_from_slice(payload);
    w.write_all(&line)?;
    w.flush()
}

/// Read one frame, or `None` at a clean end. A frame cut short is an error,
/// so a half-sent write is never acted on.
pub fn read_frame<T: DeserializeOwned>(
    r: &mut impl BufRead,
) -> std::io::Result<Option<(Envelope<T>, Vec<u8>)>> {
    let mut line = Vec::new();
    let got = r.take(MAX_HEADER as u64 + 1).read_until(b'\n', &mut line)?;
    if got == 0 {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        return Err(bad("header cut short or too long"));
    }
    let env: Envelope<T> = serde_json::from_slice(&line).map_err(|e| bad(&e.to_string()))?;
    if env.n > MAX_PAYLOAD {
        return Err(bad("payload too large"));
    }
    let mut payload = vec![0; env.n];
    r.read_exact(&mut payload)?;
    Ok(Some((env, payload)))
}

fn bad(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_process_list_fits_in_one_frame() {
        let procs: Vec<Proc> = (0..200)
            .map(|i| Proc {
                id: format!("s{i}-1"),
                pid: i,
                argv: vec![
                    "claude".into(),
                    "--append-system-prompt".into(),
                    "x".repeat(4096),
                ],
                started: 0,
                out: 0,
                err: 0,
                exited: false,
            })
            .collect();
        let mut buf = Vec::new();
        write_frame(&mut buf, 1, Msg::Procs { procs }, b"").unwrap();
        assert!(buf.len() > 64 * 1024);
        let mut r = std::io::BufReader::new(&buf[..]);
        let (env, _) = read_frame::<Msg>(&mut r).unwrap().unwrap();
        assert!(matches!(env.body, Msg::Procs { procs } if procs.len() == 200));
    }
}
