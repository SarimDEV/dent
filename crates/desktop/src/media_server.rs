//! Another machine's video and sound, played from a small web server on
//! this Mac (127.0.0.1 only), so the player fetches them as it would any
//! video: a few large ranges, read on its own threads. Through the page's
//! own scheme every one of a player's many small reads would wait on the
//! window's thread, which is busy drawing whenever an agent works.
//!
//! Each file has a token no one can guess; its bytes come over dent's own
//! link a block at a time ([`crate::stream`]), kept in memory, and are
//! written on as they come. A player that stops reading (a seek, the
//! viewer closed) hangs up, and nothing more is fetched for it.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;

use crate::stream::{self, AHEAD, BLOCK};

/// How long a player waits on its machine before hearing it won't come.
const PATIENCE: Duration = Duration::from_secs(30);

static PORT: OnceLock<Option<u16>> = OnceLock::new();

/// The server's port, started the first time it's wanted; `None` if it
/// can't listen (the page falls back to the app's own scheme).
pub fn port() -> Option<u16> {
    *PORT.get_or_init(|| {
        let l = TcpListener::bind(("127.0.0.1", 0)).ok()?;
        let port = l.local_addr().ok()?.port();
        std::thread::Builder::new()
            .name("dent-media".into())
            .spawn(move || {
                for c in l.incoming().flatten() {
                    let _ = std::thread::Builder::new()
                        .name("dent-media-conn".into())
                        .spawn(move || serve(c));
                }
            })
            .ok()?;
        Some(port)
    })
}

/// The address a page plays `token`'s file from, ending in its name so the
/// player can tell its kind.
pub fn url(port: u16, token: &str, name: &str) -> String {
    let name: String = name
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("http://127.0.0.1:{port}/m/{token}/{name}")
}

/// One request, as much of it as matters here.
struct Ask {
    head: bool,
    token: Option<String>,
    range: Option<String>,
}

fn read_ask(r: &mut impl BufRead) -> Option<Ask> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut range = None;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).ok()? == 0 {
            return None;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':')
            && k.eq_ignore_ascii_case("range")
        {
            range = Some(v.trim().to_string());
        }
    }
    let token = path
        .strip_prefix("/m/")
        .and_then(|p| p.split('/').next())
        .filter(|t| t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_string);
    Some(Ask {
        head: method == "HEAD",
        token,
        range,
    })
}

fn plain(w: &mut TcpStream, status: &str) -> std::io::Result<()> {
    write!(
        w,
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nCache-Control: no-store\r\n\r\n"
    )
}

/// Answer a connection's requests in turn, until the player hangs up.
fn serve(c: TcpStream) {
    let _ = c.set_nodelay(true);
    // A small send buffer: what's fetched is paced by what the player
    // reads, not by how much the socket would hold.
    {
        use std::os::fd::AsRawFd;
        let size: libc::c_int = 128 * 1024;
        // SAFETY: a valid socket, and a pointer to an int of its size.
        unsafe {
            libc::setsockopt(
                c.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&size as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }
    }
    let Ok(mut w) = c.try_clone() else { return };
    let mut r = BufReader::new(c);
    while let Some(ask) = read_ask(&mut r) {
        if answer(&mut w, ask).is_err() {
            return;
        }
    }
}

fn answer(w: &mut TcpStream, ask: Ask) -> std::io::Result<()> {
    let Some(token) = ask.token else {
        return plain(w, "404 Not Found");
    };
    let (file, total, base, ty) = match stream::wait_file(&token, PATIENCE) {
        Ok(f) => f,
        Err(_) => return plain(w, "404 Not Found"),
    };
    let ranged = ask.range.is_some();
    let (start, end) = match ask.range.as_deref() {
        Some(r) => match crate::web::byte_range(r, total) {
            Some(se) => se,
            None => {
                return write!(
                    w,
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\n\r\n"
                );
            }
        },
        None => (0, total.saturating_sub(1)),
    };
    let len = if total == 0 { 0 } else { end - start + 1 };
    let status = if ranged {
        "206 Partial Content"
    } else {
        "200 OK"
    };
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ty}\r\nAccept-Ranges: bytes\r\nContent-Length: {len}\r\nCache-Control: no-store\r\n"
    );
    if ranged {
        head.push_str(&format!("Content-Range: bytes {start}-{end}/{total}\r\n"));
    }
    head.push_str("\r\n");
    // The first bytes before the head, so one that can't come is a 404,
    // not a cut-off answer. Read ahead only as far as the player keeps
    // reading on: one probing the container's headers costs a block.
    let ahead = |sent: u64| (1 + sent / (2 * BLOCK)).min(AHEAD);
    let first = if ask.head || len == 0 {
        Vec::new()
    } else {
        match stream::wait_bytes(
            &file,
            total,
            start,
            end.min(start + BLOCK - 1),
            &base,
            ahead(0),
            PATIENCE,
        ) {
            Ok(b) => b,
            Err(_) => return plain(w, "404 Not Found"),
        }
    };
    w.write_all(head.as_bytes())?;
    if ask.head || len == 0 {
        return w.flush();
    }
    w.write_all(&first)?;
    let mut at = start + first.len() as u64;
    // On as it comes, a block at a time; a player that hangs up (the write
    // fails) or moves to another file ends it.
    while at <= end {
        if !stream::is_current(&token) {
            return Err(std::io::Error::other("another file is shown"));
        }
        let upto = end.min(at + BLOCK - 1);
        let b = stream::wait_bytes(&file, total, at, upto, &base, ahead(at - start), PATIENCE)
            .map_err(std::io::Error::other)?;
        w.write_all(&b)?;
        at += b.len() as u64;
    }
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_address_keeps_its_name_safe() {
        assert_eq!(
            url(5000, "ab", "my clip \"1\".mp4"),
            "http://127.0.0.1:5000/m/ab/my%20clip%20%221%22.mp4"
        );
    }

    #[test]
    fn only_a_tokens_files_are_asked_for() {
        let ask = |req: &str| read_ask(&mut std::io::Cursor::new(req.as_bytes().to_vec())).unwrap();
        let good = "0123456789abcdef0123456789abcdef";
        let a = ask(&format!(
            "GET /m/{good}/clip.mov HTTP/1.1\r\nrange: bytes=0-7\r\n\r\n"
        ));
        assert_eq!(a.token.as_deref(), Some(good));
        assert_eq!(a.range.as_deref(), Some("bytes=0-7"));
        assert!(!a.head);
        for path in [
            "/m/../../etc/passwd",
            "/m/short/x",
            "/etc/passwd",
            "/m/0123456789abcdef0123456789abcdeg/x",
        ] {
            assert!(
                ask(&format!("GET {path} HTTP/1.1\r\n\r\n")).token.is_none(),
                "{path}"
            );
        }
        assert!(ask(&format!("HEAD /m/{good}/x HTTP/1.1\r\n\r\n")).head);
        // Cut off before its headers end: nothing to answer.
        assert!(
            read_ask(&mut std::io::Cursor::new(
                b"GET /m/x HTTP/1.1\r\nRange: b".to_vec()
            ))
            .is_none()
        );
    }
}
