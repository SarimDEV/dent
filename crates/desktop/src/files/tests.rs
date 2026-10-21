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
                Send::Read { ask, offset, .. } => {
                    let end = (offset as usize + READ_MAX as usize).min(content.len());
                    let chunk = content[offset as usize..end].to_vec();
                    bytes_came(ask, offset, st, content.len() as u64, Some(chunk));
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
        if let Send::Read { ask, .. } = s {
            read_gone(ask, file("/home/ada/app/big.bin", stamp(2, 5)), true);
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
