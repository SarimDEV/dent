//! Adversarial tests (break-b): files carried inside a message.

use super::*;
use dent_protocol::CARRIED_MAX;

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("dent-bb-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn send_with(paths: Vec<String>) -> Request {
    Request::Send {
        session: "s1".into(),
        id: "m1".into(),
        text: "look".into(),
        steer: false,
        attachments: paths,
        files: Vec::new(),
    }
}

fn carried(r: &Request) -> &Vec<dent_protocol::Carried> {
    match r {
        Request::Send { files, .. } => files,
        _ => unreachable!(),
    }
}

/// The limit is "up to 40 MB in all": exactly 40 MB across two files goes,
/// whole; one byte more is refused with a reason the user can act on.
#[test]
fn exactly_forty_megabytes_goes_and_one_byte_more_is_refused() {
    let d = dir("limit");
    let (a, b, c) = (d.join("a.bin"), d.join("b.bin"), d.join("c.bin"));
    std::fs::write(&a, vec![7u8; CARRIED_MAX - 10]).unwrap();
    std::fs::write(&b, vec![9u8; 10]).unwrap();
    std::fs::write(&c, b"x").unwrap();
    let known = HashSet::new();
    let mut ok = send_with(vec![a.display().to_string(), b.display().to_string()]);
    carry(&mut ok, &known).expect("exactly 40 MB is allowed");
    let got: usize = carried(&ok)
        .iter()
        .map(|f| dent_protocol::b64::decode(&f.bytes_b64).unwrap().len())
        .sum();
    assert_eq!(got, CARRIED_MAX, "every byte arrives");
    let mut over = send_with(vec![
        a.display().to_string(),
        b.display().to_string(),
        c.display().to_string(),
    ]);
    let e = carry(&mut over, &known).unwrap_err();
    assert!(e.contains("40 MB"), "{e}");
    std::fs::remove_dir_all(&d).unwrap();
}

/// An attachment that isn't a regular file (a named pipe nobody writes to)
/// must fail the message, not block forever: `ready_up` is awaited inside
/// the machine's actor loop, so a block stalls every request and event for
/// that machine.
#[test]
fn an_attachment_that_never_ends_doesnt_stall_the_machine() {
    let d = dir("fifo");
    let fifo = d.join("pipe");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let pending = Arc::new(Mutex::new(Pending::default()));
    pending
        .lock()
        .unwrap()
        .0
        .insert(1, (send_with(vec![fifo.display().to_string()]), false));
    let theirs = Arc::new(Mutex::new(HashSet::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    let p = pending.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let _ = tx.send(rt.block_on(ready_up(&p, &theirs, PROTOCOL_FOR_TEST, "vm", 1)));
    });
    let got = rx.recv_timeout(std::time::Duration::from_secs(5));
    // Unblock the stuck reader so the thread can end.
    let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
    let _ = std::fs::remove_dir_all(&d);
    match got {
        Ok(r) => assert!(r.is_err(), "a pipe isn't a file to send: {r:?}"),
        Err(_) => {
            panic!("ready_up blocked for 5 s on a named pipe: the machine's link actor would hang")
        }
    }
}

const PROTOCOL_FOR_TEST: u32 = 99;

/// Once read in, a message is never read again: the file changed (or went)
/// after it was carried, and a resend on a new link still sends what was
/// read the first time, so the daemon writes the same thing once.
#[test]
fn a_resend_after_reconnect_doesnt_read_the_file_again() {
    let d = dir("resend");
    let f = d.join("shot.png");
    std::fs::write(&f, b"first").unwrap();
    let pending = Arc::new(Mutex::new(Pending::default()));
    pending
        .lock()
        .unwrap()
        .0
        .insert(5, (send_with(vec![f.display().to_string()]), false));
    let theirs = Arc::new(Mutex::new(HashSet::new()));
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(ready_up(&pending, &theirs, PROTOCOL_FOR_TEST, "vm", 5))
        .unwrap();
    assert!(pending.lock().unwrap().go(5).is_some());
    // The link drops; the file is edited, then deleted.
    std::fs::write(&f, b"second").unwrap();
    std::fs::remove_file(&f).unwrap();
    rt.block_on(ready_up(&pending, &theirs, PROTOCOL_FOR_TEST, "vm", 5))
        .expect("a resend reads nothing");
    let line = pending.lock().unwrap().resend(5).unwrap();
    let text = String::from_utf8(line).unwrap();
    assert!(
        text.contains(&dent_protocol::b64::encode(b"first")),
        "{text}"
    );
    assert!(
        !text.contains(&f.display().to_string()),
        "no local path goes"
    );
    std::fs::remove_dir_all(&d).unwrap();
}
