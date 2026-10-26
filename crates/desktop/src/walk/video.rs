//! Another machine's videos played in place, against the VM (made there
//! by `scripts/video-fixtures.sh`): each starts, seeks, and fetches only
//! what's watched; a restart of the machine's daemon mid-play is played
//! through. Not in `checks`: it needs the VM. Timings are printed.

use super::*;

use std::cell::RefCell;
use std::time::Instant;

/// The videos, in order, and their sizes.
const FILES: [(&str, u64); 5] = [
    ("fast.mp4", 121_042_188),
    ("big.mp4", 605_787_698),
    ("4k.mp4", 113_126_838),
    ("clip.mov", 20_381_795),
    ("clip.webm", 2_438_948),
];

thread_local! {
    static SINCE: RefCell<Option<Instant>> = const { RefCell::new(None) };
    static AT: RefCell<f64> = const { RefCell::new(0.) };
    static LAST_ASK: RefCell<Option<Instant>> = const { RefCell::new(None) };
    /// Bytes of the video now playing fetched so far.
    static FETCHED: RefCell<u64> = const { RefCell::new(0) };
    /// Which of [`FILES`] is playing, from the first.
    static NOW: RefCell<Option<usize>> = const { RefCell::new(None) };
}

/// Bytes of the video now playing fetched from its machine so far.
fn fetched() -> u64 {
    FETCHED.with(|f| {
        let mut f = f.borrow_mut();
        *f += crate::stream::take_fetched(now().0);
        *f
    })
}

/// The video being played.
fn now() -> (&'static str, u64) {
    FILES[NOW.with(|n| n.borrow().unwrap_or(0))]
}

/// The player's state, as the page last said it, asking again; only the
/// video now open counts, never the page before it.
fn player(d: &Dent) -> Option<serde_json::Value> {
    // Asked five times a second at most: asking is work for the page too.
    let due = LAST_ASK.with(|l| {
        let mut l = l.borrow_mut();
        let due = l.is_none_or(|t: Instant| t.elapsed().as_millis() >= 200);
        if due {
            *l = Some(Instant::now());
        }
        due
    });
    if let Some(p) = &d.preview
        && due
    {
        p.borrow().script(
            "(()=>{const v=document.querySelector('video');window.ipc.postMessage(JSON.stringify({t:'probe',msg:v?JSON.stringify({src:v.currentSrc,rs:v.readyState,t:v.currentTime,d:v.duration,p:v.paused,s:v.seeking,e:v.error?v.error.code:0}):'none'}))})()",
        );
    }
    let v: serde_json::Value = serde_json::from_str(d.page_probe.as_deref()?).ok()?;
    let (file, _) = now();
    v["src"]
        .as_str()
        .is_some_and(|s| s.ends_with(file))
        .then_some(v)
}

fn num(v: &serde_json::Value, k: &str) -> f64 {
    v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.)
}

fn start_clock() {
    SINCE.with(|s| *s.borrow_mut() = Some(Instant::now()));
}

fn took() -> u128 {
    SINCE.with(|s| s.borrow().map_or(0, |t| t.elapsed().as_millis()))
}

fn far_session(d: &Dent) -> Option<String> {
    d.sessions
        .iter()
        .find(|s| !dent_client_core::fleet::split(&s.id).0.is_empty())
        .map(|s| s.id.clone())
}

/// Run `js` against the video in the viewer.
fn on_video(d: &Dent, js: &str) {
    if let Some(p) = &d.preview {
        p.borrow().script(&format!(
            "(()=>{{const v=document.querySelector('video');{js}}})()"
        ));
    }
}

/// The next of [`FILES`]: opened, played, seeked most of the way in, each
/// timed; the bytes fetched stay a small part of it.
fn one() -> Vec<(f32, Act)> {
    vec![
        (
            0.2,
            Call(|d, _, cx| {
                NOW.with(|n| {
                    let mut n = n.borrow_mut();
                    *n = Some(n.map_or(0, |i| i + 1));
                });
                let (file, _) = now();
                d.far_served = 0;
                d.far_bytes = 0;
                FETCHED.with(|f| *f.borrow_mut() = 0);
                let _ = crate::stream::take_fetched(file);
                d.page_probe = None;
                start_clock();
                d.open_file(&format!("~/vt/{file}"), cx);
            }),
        ),
        (
            0.1,
            Until(
                "it opens: the player knows how long it is",
                |d, _| player(d).is_some_and(|v| num(&v, "rs") >= 1. && num(&v, "d") > 0.),
                20.,
            ),
        ),
        (
            0.0,
            Call(|d, _, _| {
                let (file, _) = now();
                eprintln!(
                    "video {file}: open in {} ms, {} KB",
                    took(),
                    fetched() / 1024
                );
                start_clock();
                on_video(d, "v.muted=true;v.play()");
            }),
        ),
        (
            0.1,
            Until(
                "pressed play, it plays",
                |d, _| player(d).is_some_and(|v| num(&v, "t") > 0.05 && num(&v, "rs") >= 3.),
                // 4K at 30 Mbit/s over a slower link buffers first.
                30.,
            ),
        ),
        (
            0.0,
            Call(|_, _, _| {
                let (file, _) = now();
                eprintln!(
                    "video {file}: playing {} ms after play, {} KB",
                    took(),
                    fetched() / 1024
                );
            }),
        ),
        (
            0.1,
            Until(
                "and plays on",
                |d, _| {
                    player(d)
                        .is_some_and(|v| num(&v, "t") > 1.0 && !v["p"].as_bool().unwrap_or(true))
                },
                15.,
            ),
        ),
        (
            0.0,
            Call(|d, _, _| {
                start_clock();
                on_video(d, "v.currentTime=v.duration*0.8");
            }),
        ),
        (
            0.1,
            Until(
                "a seek most of the way in plays from there",
                |d, _| {
                    player(d).is_some_and(|v| {
                        num(&v, "t") >= num(&v, "d") * 0.79
                            && num(&v, "rs") >= 3.
                            && !v["s"].as_bool().unwrap_or(true)
                    })
                },
                20.,
            ),
        ),
        (
            0.0,
            Call(|_, _, _| {
                let (file, size) = now();
                eprintln!(
                    "video {file}: seek to 80% in {} ms; {} MB of {} MB fetched in all",
                    took(),
                    fetched() >> 20,
                    size >> 20
                );
            }),
        ),
        (
            0.0,
            Until(
                "a long video never comes whole: only what was watched",
                |_, _| fetched() < now().1 / 2 || now().1 < (64 << 20),
                0.5,
            ),
        ),
    ]
}

pub(super) fn video_live() -> Vec<(f32, Act)> {
    let mut v = vec![
        (
            0.1,
            Until(
                "the VM is online",
                |d, _| d.machines.iter().any(|m| m.rtt.is_some()),
                30.,
            ),
        ),
        (
            0.1,
            Until(
                "a session of its is here",
                |d, _| far_session(d).is_some(),
                15.,
            ),
        ),
        (
            0.1,
            Call(|d, w, cx| {
                let id = far_session(d);
                d.select(id, w, cx);
            }),
        ),
    ];
    NOW.with(|n| *n.borrow_mut() = None);
    for _ in FILES {
        v.extend(one());
    }
    // The machine's daemon restarted while it plays: it plays through.
    v.extend([
        (
            0.2,
            Call(|d, _, cx| {
                NOW.with(|n| *n.borrow_mut() = Some(1));
                d.page_probe = None;
                d.open_file("~/vt/big.mp4", cx);
            }),
        ),
        (
            0.1,
            Until("the long one is up again", |d, _| player(d).is_some_and(|v| num(&v, "rs") >= 1.), 20.),
        ),
        (
            0.0,
            Call(|d, _, _| {
                if let Some(p) = &d.preview {
                    p.borrow().script("(()=>{const v=document.querySelector('video');v.muted=true;v.currentTime=60;v.play()})()");
                }
            }),
        ),
        (
            2.0,
            Call(|d, _, _| {
                AT.with(|a| *a.borrow_mut() = player(d).map_or(0., |v| num(&v, "t")));
                start_clock();
                let _ = std::process::Command::new("/bin/sh")
                    .args([
                        "-c",
                        "LIMA_HOME=/Users/sarimdev/.lima /opt/homebrew/bin/limactl shell dent-linux -- systemctl --user restart dent > /tmp/video-restart.log 2>&1; echo exit $? >> /tmp/video-restart.log",
                    ])
                    .spawn();
            }),
        ),
        (
            0.5,
            Until(
                "its daemon really restarted",
                |_, _| {
                    std::fs::read_to_string("/tmp/video-restart.log")
                        .is_ok_and(|s| s.contains("exit 0"))
                },
                30.,
            ),
        ),
        (
            0.5,
            Until(
                "and the video plays on past it",
                |d, _| {
                    let was = AT.with(|a| *a.borrow());
                    player(d).is_some_and(|v| num(&v, "t") > was + 20. && !v["p"].as_bool().unwrap_or(true))
                },
                60.,
            ),
        ),
        (
            0.0,
            Call(|_, _, _| eprintln!("video big.mp4: played 20 s past a daemon restart, {} ms after it", took())),
        ),
        // Played from what was here, that proves little: its machine back,
        // somewhere never fetched plays too.
        (
            0.5,
            Until(
                "its machine is back online",
                |d, _| d.machines.iter().any(|m| m.rtt.is_some()),
                60.,
            ),
        ),
        (
            0.0,
            Call(|d, _, _| {
                start_clock();
                FETCHED.with(|f| *f.borrow_mut() = 0);
                let _ = crate::stream::take_fetched("big.mp4");
                on_video(d, "v.currentTime=v.duration*0.45");
            }),
        ),
        (
            0.3,
            Until(
                "and a part never fetched plays, fetched after the restart",
                |d, _| {
                    fetched() > 0
                        && player(d).is_some_and(|v| {
                            num(&v, "t") >= num(&v, "d") * 0.44 && num(&v, "rs") >= 3. && !v["s"].as_bool().unwrap_or(true)
                        })
                },
                60.,
            ),
        ),
        (
            0.0,
            Call(|_, _, _| eprintln!("video big.mp4: after the restart, a new part plays in {} ms, {} KB fetched", took(), fetched() / 1024)),
        ),
        (1.0, Shot("video-live")),
    ]);
    // Rewritten on its machine while not playing: opened again, it's the
    // new version that plays, never the old one kept in memory.
    fn vm(cmd: &str) {
        let _ = std::process::Command::new("/opt/homebrew/bin/limactl")
            .env("LIMA_HOME", "/Users/sarimdev/.lima")
            .args(["shell", "dent-linux", "--", "bash", "-lc", cmd])
            .status();
    }
    fn length(d: &Dent) -> Option<f64> {
        local_player(d, "rw.webm")?;
        serde_json::from_str::<serde_json::Value>(d.page_probe.as_deref()?)
            .ok()?
            .get("d")?
            .as_f64()
    }
    v.extend([
        (
            0.2,
            Call(|d, _, cx| {
                vm("cp ~/vt/rw-a.webm ~/vt/rw.webm");
                d.page_probe = None;
                d.open_file("~/vt/rw.webm", cx);
            }),
        ),
        (
            0.2,
            Until(
                "a 20-second video opens",
                |d, _| length(d).is_some_and(|l| l > 15.),
                20.,
            ),
        ),
        (
            0.2,
            Call(|d, _, cx| {
                d.page_probe = None;
                d.open_file("~/vt/fast.mp4", cx);
                vm("cp ~/vt/rw-b.webm ~/vt/rw.webm");
            }),
        ),
        // Long enough that what was known of it is looked at again.
        (
            7.0,
            Call(|d, _, cx| {
                d.page_probe = None;
                d.open_file("~/vt/rw.webm", cx);
            }),
        ),
        (
            0.2,
            Until(
                "rewritten there, opened again: the new 8-second one plays",
                |d, _| length(d).is_some_and(|l| l > 6. && l < 10.),
                30.,
            ),
        ),
    ]);
    v
}

/// The same videos from this Mac's own disk, through the same scheme: the
/// floor what's played from another machine is measured against.
pub(super) fn video_local() -> Vec<(f32, Act)> {
    let mut v = vec![
        (0.3, Key("cmd-n")),
        (0.2, Type(Field::Starter, "say: a local session")),
        (0.2, Click("start")),
        (
            0.1,
            Until(
                "a session of this Mac's is open",
                |d, _| d.current().is_some(),
                10.,
            ),
        ),
    ];
    v.extend([
        (
            0.3,
            Call(|d, _, cx| {
                d.page_probe = None;
                NOW.with(|n| *n.borrow_mut() = Some(0));
                start_clock();
                d.open_file("/tmp/fast-local.mp4", cx);
            }),
        ),
        (
            0.1,
            Until(
                "this Mac's opens",
                |d, _| local_player(d, "fast-local.mp4").is_some_and(|v| num(&v, "rs") >= 1.),
                20.,
            ),
        ),
        (
            0.0,
            Call(|_, _, _| eprintln!("video local fast.mp4: open in {} ms", took())),
        ),
        (
            0.3,
            Call(|d, _, cx| {
                d.page_probe = None;
                start_clock();
                d.open_file("/tmp/clip-local.mov", cx);
            }),
        ),
        (
            0.1,
            Until(
                "this Mac's mov opens",
                |d, _| local_player(d, "clip-local.mov").is_some_and(|v| num(&v, "rs") >= 1.),
                20.,
            ),
        ),
        (
            0.0,
            Call(|_, _, _| eprintln!("video local clip.mov: open in {} ms", took())),
        ),
    ]);
    v
}

fn local_player(d: &Dent, name: &str) -> Option<serde_json::Value> {
    if let Some(p) = &d.preview {
        p.borrow().script(
            "(()=>{const v=document.querySelector('video');window.ipc.postMessage(JSON.stringify({t:'probe',msg:v?JSON.stringify({src:v.currentSrc,rs:v.readyState,d:v.duration}):'none'}))})()",
        );
    }
    let v: serde_json::Value = serde_json::from_str(d.page_probe.as_deref()?).ok()?;
    v["src"]
        .as_str()
        .is_some_and(|s| s.ends_with(name))
        .then_some(v)
}

/// Prototype: the same video through the app's own scheme and from a
/// local web server, with the window's thread kept busy (as drawing a
/// working session does).
pub(super) fn video_proto() -> Vec<(f32, Act)> {
    fn busy_player(d: &Dent, name: &str) -> bool {
        std::thread::sleep(std::time::Duration::from_millis(40));
        local_player(d, name).is_some_and(|v| v["rs"].as_f64().unwrap_or(0.) >= 1.)
    }
    vec![
        (0.3, Key("cmd-n")),
        (0.2, Type(Field::Starter, "say: a local session")),
        (0.2, Click("start")),
        (
            0.1,
            Until(
                "a session of this Mac's is open",
                |d, _| d.current().is_some(),
                10.,
            ),
        ),
        (
            0.3,
            Call(|d, _, cx| {
                d.page_probe = None;
                start_clock();
                d.open_file("/tmp/clip-local.mov", cx);
            }),
        ),
        (
            0.0,
            Until(
                "through dent:// with the thread busy",
                |d, _| busy_player(d, "clip-local.mov"),
                30.,
            ),
        ),
        (
            0.0,
            Call(|_, _, _| eprintln!("video proto scheme+busy: open in {} ms", took())),
        ),
        (
            0.3,
            Call(|d, _, cx| {
                d.page_probe = None;
                start_clock();
                d.open_file("/tmp/lh.html", cx);
            }),
        ),
        (
            0.0,
            Until(
                "from 127.0.0.1 with the thread busy",
                |d, _| busy_player(d, "clip.mov"),
                30.,
            ),
        ),
        (
            0.0,
            Call(|_, _, _| eprintln!("video proto localhost+busy: open in {} ms", took())),
        ),
    ]
}
