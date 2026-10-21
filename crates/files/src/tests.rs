//! Each test is a way showing a file across machines has gone wrong
//! (T3 Code's history, decision 0068): the wrong home, the wrong folder,
//! a link out of bounds, a pipe that hangs, a rewrite that keeps old bytes.

use super::*;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("dent-files-{name}-{}", std::process::id()))
        .canonicalize()
        .unwrap_or_else(|_| {
            let d = std::env::temp_dir().join(format!("dent-files-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&d).unwrap();
            d.canonicalize().unwrap()
        });
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn home_is_the_machines_own_and_relative_is_the_folder_given() {
    let (base, home) = (Path::new("/work/app"), Path::new("/home/ada"));
    let r = |w: &str| resolve(w, base, home);
    assert_eq!(r("~/x.png").unwrap(), Path::new("/home/ada/x.png"));
    assert_eq!(r("~").unwrap(), home);
    assert_eq!(r("out/x.png").unwrap(), Path::new("/work/app/out/x.png"));
    assert_eq!(r("/abs/x.png").unwrap(), Path::new("/abs/x.png"));
    assert_eq!(r("file:///a%20b/c.png").unwrap(), Path::new("/a b/c.png"));
    assert_eq!(
        r("file://localhost/a/c.png").unwrap(),
        Path::new("/a/c.png")
    );
    // This machine by its own name is this machine.
    let me = String::from_utf8(
        std::process::Command::new("hostname")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let me = me.trim();
    assert_eq!(
        r(&format!("file://{me}/a/c.png")).unwrap(),
        Path::new("/a/c.png")
    );
    let short = me.split('.').next().unwrap().to_uppercase();
    assert_eq!(
        r(&format!("file://{short}.local/a")).unwrap(),
        Path::new("/a")
    );
    // Another computer's file is no file here, one of the same short name
    // on another network too.
    assert_eq!(r("file://elsewhere-not-this-one/a/c.png"), None);
    assert_eq!(r(&format!("file://{short}.other.lan/a")), None);
    // `~name` is that user's home, as a shell reads it; no such user, and
    // it's a name like any other.
    let root = if cfg!(target_os = "macos") {
        "/var/root"
    } else {
        "/root"
    };
    assert_eq!(r("~root/x").unwrap(), Path::new(root).join("x"));
    assert_eq!(r("~root").unwrap(), Path::new(root));
    assert_eq!(
        r("~nobody-dent-test/x").unwrap(),
        Path::new("/work/app/~nobody-dent-test/x")
    );
}

#[test]
fn a_hash_in_a_name_wins_and_a_fragment_falls_back() {
    let d = dir("hash");
    std::fs::write(d.join("a#1.png"), b"x").unwrap();
    std::fs::write(d.join("b.md"), b"y").unwrap();
    let p = Policy::default();
    assert!(matches!(stat("a#1.png", &d, &d, &p), FileStat::File { .. }));
    match stat("b.md#heading", &d, &d, &p) {
        FileStat::File { path, .. } => assert!(path.ends_with("/b.md"), "{path}"),
        s => panic!("{s:?}"),
    }
    match stat("nope.md#x", &d, &d, &p) {
        FileStat::Missing { checked } => assert!(checked.ends_with("nope.md#x")),
        s => panic!("{s:?}"),
    }
}

#[test]
fn every_answer_says_where_it_looked() {
    let d = dir("where");
    std::fs::create_dir_all(d.join("sub")).unwrap();
    let p = Policy::default();
    match stat("gone.png", &d, &d, &p) {
        FileStat::Missing { checked } => {
            assert_eq!(checked, d.join("gone.png").display().to_string())
        }
        s => panic!("{s:?}"),
    }
    assert!(matches!(stat("sub", &d, &d, &p), FileStat::Dir { .. }));
}

#[test]
fn the_daemons_own_secrets_are_refused_even_through_a_link() {
    let d = dir("policy");
    let state = d.join("state");
    std::fs::create_dir_all(state.join("attachments/s1")).unwrap();
    std::fs::write(state.join("identity.key"), b"secret").unwrap();
    std::fs::write(state.join("attachments/s1/pic.png"), b"pic").unwrap();
    std::os::unix::fs::symlink(state.join("identity.key"), d.join("innocent.txt")).unwrap();
    let p = Policy {
        deny: vec![state.clone()],
        allow: vec![state.join("attachments")],
    };
    assert!(matches!(
        stat("state/identity.key", &d, &d, &p),
        FileStat::Refused { .. }
    ));
    assert!(matches!(
        stat("innocent.txt", &d, &d, &p),
        FileStat::Refused { .. }
    ));
    assert!(matches!(
        stat("state/attachments/s1/pic.png", &d, &d, &p),
        FileStat::File { .. }
    ));
    assert!(read("innocent.txt", &d, &d, &p, 0, 10, None).is_err());
}

#[test]
fn a_link_inside_is_followed_to_the_real_file() {
    let d = dir("link");
    std::fs::write(d.join("real.txt"), b"hello").unwrap();
    std::os::unix::fs::symlink(d.join("real.txt"), d.join("link.txt")).unwrap();
    match stat("link.txt", &d, &d, &Policy::default()) {
        FileStat::File { path, .. } => assert!(path.ends_with("/real.txt")),
        s => panic!("{s:?}"),
    }
}

#[test]
fn a_pipe_is_unreadable_and_never_hangs() {
    let d = dir("fifo");
    let f = d.join("pipe");
    let c = std::ffi::CString::new(f.display().to_string()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let p = Policy::default();
    assert!(matches!(
        stat("pipe", &d, &d, &p),
        FileStat::Unreadable { .. }
    ));
    assert!(read("pipe", &d, &d, &p, 0, 10, None).is_err());
}

#[test]
fn a_rewrite_is_a_new_version_even_at_the_same_size() {
    let d = dir("stamp");
    let f = d.join("chart.png");
    std::fs::write(&f, b"aaaa").unwrap();
    let p = Policy::default();
    let first = match stat("chart.png", &d, &d, &p) {
        FileStat::File { stamp, .. } => stamp,
        s => panic!("{s:?}"),
    };
    std::thread::sleep(std::time::Duration::from_millis(5));
    std::fs::write(&f, b"bbbb").unwrap();
    // A read pinned to the old version says it changed, never mixes.
    match read("chart.png", &d, &d, &p, 0, 4, Some(first)) {
        Err((FileStat::File { stamp, .. }, true)) => assert_ne!(stamp, first),
        r => panic!("{r:?}"),
    }
    // Replaced by a rename (as editors and agents save): changed too.
    let now = match stat("chart.png", &d, &d, &p) {
        FileStat::File { stamp, .. } => stamp,
        s => panic!("{s:?}"),
    };
    std::fs::write(d.join("tmp"), b"cccc").unwrap();
    std::fs::rename(d.join("tmp"), &f).unwrap();
    assert!(matches!(
        read("chart.png", &d, &d, &p, 0, 4, Some(now)),
        Err((_, true))
    ));
}

#[test]
fn ranges_are_capped_and_end_at_the_end() {
    let d = dir("range");
    let data: Vec<u8> = (0..(READ_MAX as usize + 1000)).map(|i| i as u8).collect();
    std::fs::write(d.join("big.bin"), &data).unwrap();
    let p = Policy::default();
    let (stamp, total, b) = read("big.bin", &d, &d, &p, 0, u32::MAX, None).unwrap();
    assert_eq!(total, data.len() as u64);
    assert_eq!(b.len(), READ_MAX as usize);
    let (_, _, tail) = read(
        "big.bin",
        &d,
        &d,
        &p,
        READ_MAX as u64,
        READ_MAX,
        Some(stamp),
    )
    .unwrap();
    assert_eq!(tail, &data[READ_MAX as usize..]);
    let (_, _, past) = read("big.bin", &d, &d, &p, total + 5, 10, Some(stamp)).unwrap();
    assert!(past.is_empty());
}

#[test]
fn picture_sizes_from_headers() {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(640u32.to_be_bytes());
    png.extend(480u32.to_be_bytes());
    assert_eq!(picture_size(&png), Some((640, 480)));
    let mut gif = b"GIF89a".to_vec();
    gif.extend(320u16.to_le_bytes());
    gif.extend(200u16.to_le_bytes());
    assert_eq!(picture_size(&gif), Some((320, 200)));
    // JPEG: SOI, an APP0 segment, then SOF0 with height 100, width 300.
    let jpg = [
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x64,
        0x01, 0x2c, 0x03,
    ];
    assert_eq!(picture_size(&jpg), Some((300, 100)));
    let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
    webp.extend([0u8; 8]);
    webp.extend([0x7f, 0x02, 0x00, 0xdf, 0x01, 0x00]);
    assert_eq!(picture_size(&webp), Some((640, 480)));
    assert_eq!(picture_size(b"plain text"), None);
}

#[test]
fn file_addresses_at_their_edges() {
    let (base, home) = (Path::new("/work/app"), Path::new("/home/ada"));
    let r = |w: &str| resolve(w, base, home);
    assert_eq!(r("file://").unwrap(), Path::new("/"));
    assert_eq!(r("file:///").unwrap(), Path::new("/"));
    assert_eq!(r("file://localhost").unwrap(), Path::new("/"));
    assert_eq!(r("file://LOCALHOST/x").unwrap(), Path::new("/x"));
    assert_eq!(r("  file:///x  ").unwrap(), Path::new("/x"));
    // A port is still another computer's name.
    assert_eq!(r("file://host:80/x"), None);
    assert_eq!(r("file://localhost:80/x"), None);
    // Percent-encoding.
    assert_eq!(r("file:///a%2Fb").unwrap(), Path::new("/a/b"));
    assert_eq!(r("file:///a%zzb").unwrap(), Path::new("/a%zzb"));
    assert_eq!(r("file:///a%").unwrap(), Path::new("/a%"));
    assert_eq!(r("file:///a%2").unwrap(), Path::new("/a%2"));
    assert_eq!(r("file:///%E2%9C%93").unwrap(), Path::new("/✓"));
    // Not an address: written as is, `%20` and all.
    assert_eq!(r("/a%20b").unwrap(), Path::new("/a%20b"));
    // `..` in an address isn't resolved away here (the policy sees the
    // real path).
    assert_eq!(r("file:///a/../b").unwrap(), Path::new("/a/../b"));
}

#[test]
fn a_decoded_nul_is_no_file_and_no_panic() {
    let d = dir("nul");
    let p = Policy::default();
    assert_eq!(
        resolve("file:///a%00b", &d, &d).unwrap(),
        Path::new("/a\0b")
    );
    let s = stat("file:///a%00b", &d, &d, &p);
    assert!(
        !matches!(s, FileStat::File { .. } | FileStat::Dir { .. }),
        "{s:?}"
    );
    let s = stat("a\0b", &d, &d, &p);
    assert!(
        !matches!(s, FileStat::File { .. } | FileStat::Dir { .. }),
        "{s:?}"
    );
}

#[test]
fn a_nul_is_missing_not_unreadable() {
    // A name no file can have is no file, as a missing one is.
    let d = dir("nul2");
    let s = stat("a\0b.png", &d, &d, &Policy::default());
    assert!(matches!(s, FileStat::Missing { .. }), "{s:?}");
}

#[test]
fn the_scheme_is_case_insensitive() {
    // RFC 3986: schemes are case-insensitive.
    let (base, home) = (Path::new("/work/app"), Path::new("/home/ada"));
    assert_eq!(resolve("FILE:///x", base, home).unwrap(), Path::new("/x"));
}

#[test]
fn this_machine_by_its_loopback_address_is_this_machine() {
    // `file://127.0.0.1/x` and `file://[::1]/x` can only name this machine.
    let (base, home) = (Path::new("/work/app"), Path::new("/home/ada"));
    assert_eq!(
        resolve("file://127.0.0.1/x", base, home).as_deref(),
        Some(Path::new("/x"))
    );
    assert_eq!(
        resolve("file://[::1]/x", base, home).as_deref(),
        Some(Path::new("/x"))
    );
}

#[test]
fn tildes_at_their_edges() {
    let (base, home) = (Path::new("/work/app"), Path::new("/home/ada"));
    let r = |w: &str| resolve(w, base, home).unwrap();
    let root = if cfg!(target_os = "macos") {
        "/var/root"
    } else {
        "/root"
    };
    assert_eq!(r("~/"), home);
    assert_eq!(r("~root/"), Path::new(root));
    assert_eq!(r("~root//x"), Path::new(root).join("x"));
    // Not a user name: a name like any other, taken from the folder.
    assert_eq!(r("~../etc"), Path::new("/work/app/~../etc"));
    assert_eq!(r("~./x"), Path::new("/work/app/~./x"));
    assert_eq!(r("~a b/x"), Path::new("/work/app/~a b/x"));
    assert_eq!(r("~a%20b/x"), Path::new("/work/app/~a%20b/x"));
    let long = format!("~{}", "a".repeat(300));
    assert_eq!(r(&long), Path::new("/work/app").join(&long));
    assert_eq!(
        r(&format!("{long}/x")),
        Path::new("/work/app").join(&long).join("x")
    );
    let huge = format!("~{}", "b".repeat(100_000));
    assert_eq!(r(&huge), Path::new("/work/app").join(&huge));
    // As an address: `file:///~root` is a folder named `~root` at the top,
    // not root's home.
    assert_eq!(r("file:///~root"), Path::new("/~root"));
}

#[test]
fn candidates_at_their_edges() {
    let (base, home) = (Path::new("/w"), Path::new("/h"));
    let c = |w: &str| candidates(w, base, home);
    assert_eq!(c("a.md#h"), [Path::new("/w/a.md#h"), Path::new("/w/a.md")]);
    assert_eq!(
        c("a.md?q=1#h"),
        [Path::new("/w/a.md?q=1#h"), Path::new("/w/a.md")]
    );
    assert_eq!(c("#only"), [Path::new("/w/#only")]);
    assert_eq!(c("?only"), [Path::new("/w/?only")]);
    assert_eq!(c("a.md#"), [Path::new("/w/a.md#"), Path::new("/w/a.md")]);
    assert_eq!(c("~/a#b"), [Path::new("/h/a#b"), Path::new("/h/a")]);
    // Another computer's address: nothing here, its fragment cut or not.
    assert!(c("file://elsewhere-not-this-one/a#b").is_empty());
    // An address whose fragment comes before the host ends: the cut is
    // taken from the whole string.
    assert_eq!(c("file://#x"), [Path::new("/")]);
    assert_eq!(c(""), [Path::new("/w")]);
}

#[test]
fn another_computers_file_is_missing_as_written() {
    let d = dir("away");
    let p = Policy::default();
    for w in [
        "file://elsewhere-not-this-one/a/c.png",
        "  file://elsewhere-not-this-one/a/c.png#x  ",
        "file://host:80/x",
        "file://elsewhere-not-this-one",
    ] {
        match stat(w, &d, &d, &p) {
            FileStat::Missing { checked } => assert_eq!(checked, w.trim()),
            s => panic!("{w}: {s:?}"),
        }
        match stat_plain(w, &d, &d, &p) {
            FileStat::Missing { checked } => assert_eq!(checked, w.trim()),
            s => panic!("{w}: {s:?}"),
        }
    }
}

#[test]
fn a_real_file_with_a_hash_and_odd_characters() {
    let d = dir("odd");
    let p = Policy::default();
    std::fs::write(d.join("take #1 ✓.png"), b"x").unwrap();
    std::fs::write(d.join("a?b.md"), b"x").unwrap();
    std::fs::write(d.join("pct%20.md"), b"x").unwrap();
    assert!(matches!(
        stat("take #1 ✓.png", &d, &d, &p),
        FileStat::File { .. }
    ));
    assert!(matches!(stat("a?b.md", &d, &d, &p), FileStat::File { .. }));
    assert!(matches!(
        stat("pct%20.md", &d, &d, &p),
        FileStat::File { .. }
    ));
    // As an address, `%23` is the `#`.
    let addr = format!("file://{}/take%20%231%20%E2%9C%93.png", d.display());
    assert!(
        matches!(stat(&addr, &d, &d, &p), FileStat::File { .. }),
        "{addr}"
    );
    // `#1` read as a fragment when the whole isn't there: `take ` is not.
    match stat("take #2 ✓.png", &d, &d, &p) {
        FileStat::Missing { checked } => assert!(checked.ends_with("take #2 ✓.png")),
        s => panic!("{s:?}"),
    }
}

#[test]
fn very_long_paths_dont_panic() {
    let d = dir("long");
    let p = Policy::default();
    let long = "x".repeat(5000);
    let s = stat(&long, &d, &d, &p);
    assert!(!matches!(s, FileStat::File { .. }), "{s:?}");
    let deep = "a/".repeat(3000);
    let s = stat(&deep, &d, &d, &p);
    assert!(!matches!(s, FileStat::File { .. }), "{s:?}");
}

#[test]
fn a_long_fragment_still_falls_back_to_the_file() {
    // A link to a long heading: as written, the name is too long for the
    // disk; without the fragment it's there.
    let d = dir("longfrag");
    std::fs::write(d.join("notes.md"), b"x").unwrap();
    let w = format!("notes.md#{}", "a-very-long-heading-".repeat(15));
    match stat(&w, &d, &d, &Policy::default()) {
        FileStat::File { path, .. } => assert!(path.ends_with("/notes.md"), "{path}"),
        s => panic!("{s:?}"),
    }
}

#[test]
fn a_doubled_slash_after_a_tilde_stays_home() {
    // `~//x` is home's `x` to a shell; joined as is, `/x` replaces home.
    let (base, home) = (Path::new("/work/app"), Path::new("/home/ada"));
    assert_eq!(
        resolve("~//x", base, home).unwrap(),
        Path::new("/home/ada/x")
    );
}
