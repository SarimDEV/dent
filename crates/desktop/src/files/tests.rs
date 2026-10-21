//! The file layer's rules, without a network: each test is a way showing
//! another machine's file has gone wrong elsewhere (decision 0068).

use super::*;
use dent_protocol::b64;

fn setup() -> PathBuf {
    let d = std::env::temp_dir().join(format!("dent-files-cache-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    // SAFETY: every test sets the same value.
    unsafe { std::env::set_var("DENT_FILES_CACHE", &d) };
    FILES.with(|f| *f.borrow_mut() = Files::default());
    d
}

fn far(path: &str) -> Ask {
    Ask::new("vm/s1", path, "/home/ada/app")
}

fn stamp(n: i64, size: u64) -> Stamp {
    Stamp {
        dev: 1,
        ino: 2,
        size,
        mtime_ns: n,
    }
}

fn file(path: &str, st: Stamp) -> FileStat {
    FileStat::File {
        path: path.into(),
        stamp: st,
        width: None,
        height: None,
    }
}

/// Answer what was asked like a machine holding `content` at `real`.
fn serve(real: &str, st: Stamp, content: &[u8]) {
    for _ in 0..1000 {
        let sends = drain();
        if sends.is_empty() {
            return;
        }
        for (_, s) in sends {
            match s {
                Send::Stat { asks, .. } => {
                    let n = asks.len();
                    stats_came(asks, vec![file(real, st); n]);
                }
                Send::Read { key, offset, .. } => {
                    let end = (offset as usize + READ_MAX as usize).min(content.len());
                    let chunk = content[offset as usize..end].to_vec();
                    bytes_came(key, offset, st, content.len() as u64, Some(chunk));
                }
            }
        }
    }
    panic!("never settled");
}

#[test]
fn another_machines_file_is_never_looked_for_here() {
    setup();
    // This path exists on this Mac; another machine's same path isn't it.
    let here = std::env::temp_dir().join(format!("dent-same-path-{}.png", std::process::id()));
    std::fs::write(&here, b"this mac's picture").unwrap();
    let a = far(&here.display().to_string());
    set_online("vm", true, Some(4));
    assert_eq!(picture(&a), Look::Loading);
    let _ = std::fs::remove_file(&here);
}

#[test]
fn a_picture_is_asked_then_copied_whole_in_ranges() {
    setup();
    set_online("vm", true, Some(4));
    let a = far("shot.png");
    assert_eq!(picture(&a), Look::Loading);
    let content: Vec<u8> = (0..(READ_MAX as usize * 2 + 10)).map(|i| i as u8).collect();
    let st = stamp(1, content.len() as u64);
    serve("/home/ada/app/shot.png", st, &content);
    let Look::Here(p) = picture(&a) else {
        panic!("{:?}", picture(&a))
    };
    assert_eq!(std::fs::read(&p).unwrap(), content);
    // Kept under its own name, so it reads as itself.
    assert_eq!(p.file_name().unwrap(), "shot.png");
    // Asked again (an event names it): the same version isn't fetched twice.
    changed("vm", Some(&["shot.png".into()]));
    let sends = drain();
    assert!(sends.iter().all(|(_, s)| matches!(s, Send::Stat { .. })));
}

#[test]
fn a_rewritten_file_never_shows_its_old_bytes() {
    setup();
    set_online("vm", true, Some(4));
    let a = far("chart.png");
    picture(&a);
    serve("/home/ada/app/chart.png", stamp(1, 3), b"old");
    assert_eq!(std::fs::read(picture(&a).shown().unwrap()).unwrap(), b"old");
    // The agent rewrites it, same size, same second to the eye.
    changed("vm", Some(&["chart.png".into()]));
    serve("/home/ada/app/chart.png", stamp(2, 3), b"new");
    assert_eq!(std::fs::read(picture(&a).shown().unwrap()).unwrap(), b"new");
}

#[test]
fn a_file_that_changes_mid_copy_starts_again() {
    setup();
    set_online("vm", true, Some(4));
    let a = far("big.bin");
    let content = vec![1u8; READ_MAX as usize * 3];
    picture(&a);
    let st = stamp(1, content.len() as u64);
    // Its stat, and a first range.
    for (_, s) in drain() {
        if let Send::Stat { asks, .. } = s {
            stats_came(asks, vec![file("/home/ada/app/big.bin", st)]);
        }
    }
    for (_, s) in drain() {
        if let Send::Read { key, .. } = s {
            read_gone(key, file("/home/ada/app/big.bin", stamp(2, 5)), true);
        }
    }
    // Asked again, it's the new version that's copied.
    let fresh = b"fresh".to_vec();
    serve("/home/ada/app/big.bin", stamp(2, 5), &fresh);
    assert_eq!(std::fs::read(picture(&a).shown().unwrap()).unwrap(), fresh);
}

#[test]
fn not_there_says_where_and_isnt_kept_for_good() {
    setup();
    set_online("vm", true, Some(4));
    let a = far("gone.png");
    picture(&a);
    for (_, s) in drain() {
        if let Send::Stat { asks, .. } = s {
            stats_came(
                asks,
                vec![FileStat::Missing {
                    checked: "/home/ada/app/gone.png".into(),
                }],
            );
        }
    }
    match picture(&a) {
        Look::Gone(g) => {
            assert_eq!(g.why, Why::Missing);
            assert_eq!(g.said("vm"), "Not on vm · /home/ada/app/gone.png");
        }
        l => panic!("{l:?}"),
    }
    // The agent makes it: the next event naming it asks again.
    changed("vm", Some(&["gone.png".into()]));
    serve("/home/ada/app/gone.png", stamp(1, 2), b"ok");
    assert!(matches!(picture(&a), Look::Here(_)));
}

#[test]
fn away_shows_whats_kept_marked_and_nothing_else() {
    setup();
    set_online("vm", true, Some(4));
    let a = far("kept.png");
    picture(&a);
    serve("/home/ada/app/kept.png", stamp(1, 2), b"ok");
    set_online("vm", false, None);
    assert!(matches!(picture(&a), Look::Stale(_)));
    let b = far("never.png");
    assert!(matches!(
        picture(&b),
        Look::Gone(Gone { why: Why::Away, .. })
    ));
    // Nothing is sent to a machine that's away.
    assert!(drain().is_empty());
    // Back: asked again.
    set_online("vm", true, Some(4));
    assert!(drain().iter().any(|(_, s)| matches!(s, Send::Stat { .. })));
}

#[test]
fn a_kept_copy_draws_at_once_after_a_relaunch() {
    setup();
    // A machine of its own: tests run side by side, and each machine has
    // one index file.
    set_online("vm-relaunch", true, Some(4));
    let a = Ask::new("vm-relaunch/s1", "again.png", "/home/ada/app");
    picture(&a);
    serve("/home/ada/app/again.png", stamp(1, 2), b"hi");
    drain(); // the index is written
    // A new run: nothing in memory, the machine not yet back.
    FILES.with(|f| *f.borrow_mut() = Files::default());
    assert!(matches!(picture(&a), Look::Stale(_)));
}

#[test]
fn an_older_daemon_says_update() {
    setup();
    set_online("vm", true, Some(3));
    match picture(&far("x.png")) {
        Look::Gone(g) => assert_eq!(g.said("vm"), "Update dent on vm to see its files"),
        l => panic!("{l:?}"),
    }
}

#[test]
fn too_big_to_show_says_so() {
    setup();
    set_online("vm", true, Some(4));
    let a = far("huge.mov");
    picture(&a);
    for (_, s) in drain() {
        if let Send::Stat { asks, .. } = s {
            stats_came(
                asks,
                vec![file("/home/ada/app/huge.mov", stamp(1, WHOLE_MAX + 1))],
            );
        }
    }
    assert!(matches!(
        picture(&a),
        Look::Gone(Gone {
            why: Why::TooBig(_),
            ..
        })
    ));
    assert!(drain().iter().all(|(_, s)| !matches!(s, Send::Read { .. })));
}

#[test]
fn only_a_few_reads_at_once_per_machine() {
    setup();
    set_online("vm", true, Some(4));
    let asks: Vec<Ask> = (0..10).map(|i| far(&format!("p{i}.png"))).collect();
    for a in &asks {
        picture(a);
    }
    for (_, s) in drain() {
        if let Send::Stat { asks, .. } = s {
            let stats = asks
                .iter()
                .map(|a| file(&format!("/r/{}", a.path), stamp(1, 10)))
                .collect();
            stats_came(asks, stats);
        }
    }
    let reads = drain()
        .into_iter()
        .filter(|(_, s)| matches!(s, Send::Read { .. }))
        .count();
    assert_eq!(reads, WINDOW);
}

#[test]
fn this_macs_files_are_read_where_they_are() {
    setup();
    let d = std::env::temp_dir().join(format!("dent-near-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("pic.png"), b"x").unwrap();
    let a = Ask::new("s1", "pic.png", &d.display().to_string());
    match picture(&a) {
        Look::Here(p) => assert_eq!(p, d.join("pic.png").canonicalize().unwrap()),
        l => panic!("{l:?}"),
    }
    assert!(drain().is_empty(), "nothing asked of any machine");
    let _ = b64::encode(b"");
}

#[test]
fn drawing_never_reads_a_sessions_file_from_this_disk_unmarked() {
    // Decision 0068: a session's files go through this layer, so another
    // machine's are never read from this Mac's disk. Code that reads the
    // disk where a session's file could be says why it's this Mac's.
    let sources = [
        ("chat.rs", include_str!("../chat.rs")),
        ("markdown.rs", include_str!("../markdown.rs")),
        ("bridge.rs", include_str!("../bridge.rs")),
        ("viewer.rs", include_str!("../viewer.rs")),
        ("lightbox.rs", include_str!("../lightbox.rs")),
        ("cards.rs", include_str!("../cards.rs")),
        ("context.rs", include_str!("../context.rs")),
        ("select.rs", include_str!("../select.rs")),
        ("web.rs", include_str!("../web.rs")),
        ("popout.rs", include_str!("../popout.rs")),
        ("app.rs", include_str!("../app.rs")),
    ];
    let reads = [
        "is_file(",
        "std::fs::read(",
        "fs::metadata(",
        ".exists()",
        "read_to_string(",
        "open::open(",
        "reveal_path(",
    ];
    let mut bad = Vec::new();
    for (name, src) in sources {
        let lines: Vec<&str> = src.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            let code = l.trim_start();
            if code.starts_with("//") || !reads.iter().any(|r| l.contains(r)) {
                continue;
            }
            // On the line, or the one before or after (rustfmt moves a
            // trailing comment down when the line breaks).
            let marked = l.contains("// this Mac's")
                || i.checked_sub(1)
                    .is_some_and(|j| lines[j].contains("// this Mac's"))
                || lines
                    .get(i + 1)
                    .is_some_and(|n| n.trim_start().starts_with("// this Mac's"));
            if !marked {
                bad.push(format!("{name}:{}: {}", i + 1, code));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "read through crate::files, or mark why it's this Mac's:\n{}",
        bad.join("\n")
    );
}

#[test]
fn two_ways_of_naming_one_file_make_one_copy_never_a_broken_one() {
    setup();
    set_online("vm-two", true, Some(4));
    // `shot.png` and its full path are one file there.
    let a = Ask::new("vm-two/s1", "shot.png", "/home/ada/app");
    let b = Ask::new("vm-two/s1", "/home/ada/app/shot.png", "/home/ada/app");
    let content: Vec<u8> = (0..(READ_MAX as usize * 3))
        .map(|i| (i % 251) as u8)
        .collect();
    let st = stamp(7, content.len() as u64);
    picture(&a);
    // A's copy gets going first.
    for (_, s) in drain() {
        if let Send::Stat { asks, .. } = s {
            let n = asks.len();
            stats_came(asks, vec![file("/home/ada/app/shot.png", st); n]);
        }
    }
    let first: Vec<(PathBuf, u64)> = drain()
        .into_iter()
        .filter_map(|(_, s)| match s {
            Send::Read { key, offset, .. } => Some((key, offset)),
            _ => None,
        })
        .collect();
    assert_eq!(first.len(), 1);
    let (key, _) = first[0].clone();
    bytes_came(
        key.clone(),
        0,
        st,
        content.len() as u64,
        Some(content[..READ_MAX as usize].to_vec()),
    );
    // Then B asks for the same file, part-way through.
    picture(&b);
    serve("/home/ada/app/shot.png", st, &content);
    let (Look::Here(pa), Look::Here(pb)) = (picture(&a), picture(&b)) else {
        panic!("{:?} {:?}", picture(&a), picture(&b))
    };
    assert_eq!(pa, pb, "one copy");
    assert_eq!(std::fs::read(pa).unwrap(), content, "and it's whole");
}

#[test]
fn a_reply_sent_twice_after_a_reconnect_is_taken_once() {
    setup();
    set_online("vm-dup", true, Some(4));
    let a = Ask::new("vm-dup/s1", "d.bin", "/w");
    let content = vec![3u8; READ_MAX as usize * 2];
    let st = stamp(1, content.len() as u64);
    picture(&a);
    for (_, s) in drain() {
        if let Send::Stat { asks, .. } = s {
            stats_came(asks, vec![file("/w/d.bin", st)]);
        }
    }
    let key = drain()
        .into_iter()
        .find_map(|(_, s)| match s {
            Send::Read { key, .. } => Some(key),
            _ => None,
        })
        .unwrap();
    let first = content[..READ_MAX as usize].to_vec();
    bytes_came(
        key.clone(),
        0,
        st,
        content.len() as u64,
        Some(first.clone()),
    );
    // The same range again (resent by the link): ignored, no stall.
    bytes_came(key.clone(), 0, st, content.len() as u64, Some(first));
    serve("/w/d.bin", st, &content);
    assert_eq!(
        std::fs::read(picture(&a).shown().unwrap()).unwrap(),
        content
    );
}

#[test]
fn a_question_never_answered_is_asked_again() {
    setup();
    set_online("vm-lost", true, Some(4));
    let a = Ask::new("vm-lost/s1", "q.png", "/w");
    picture(&a);
    assert!(drain().iter().any(|(_, s)| matches!(s, Send::Stat { .. })));
    // Its link went for good: no answer. In a while it's asked again.
    FILES.with(|f| {
        for e in f.borrow_mut().far.values_mut() {
            e.asked_at = Some(Instant::now() - PATIENCE - Duration::from_secs(1));
        }
    });
    assert!(drain().iter().any(|(_, s)| matches!(s, Send::Stat { .. })));
}

#[test]
fn a_failure_waits_on_a_clock_not_on_every_frame() {
    setup();
    set_online("vm-fail", true, Some(4));
    let a = Ask::new("vm-fail/s1", "f.png", "/w");
    picture(&a);
    let asks = drain()
        .into_iter()
        .find_map(|(_, s)| match s {
            Send::Stat { asks, .. } => Some(asks),
            _ => None,
        })
        .unwrap();
    stats_failed(asks, "something broke");
    drain();
    // Nothing to send each frame; something to look at again in a while.
    assert!(!busy());
    assert!(waiting());
}
