use super::*;

/// How many sessions stream at once.
const MANY: usize = 20;

/// Many agents at once, where agent apps most often struggle: twenty
/// sessions each streaming a long mixed reply together, switched between
/// while they stream. Run with DENT_WALK_RELEASE=1 DENT_FRAME_TIMES=1 to
/// time drawing; the app's memory is said before and after.
pub(super) fn load() -> Vec<(f32, Act)> {
    let mut v = vec![(0.1, Call(say_memory))];
    for n in 0..MANY {
        v.extend(new_session(leak(format!("stream bench 90 for load {n}"))));
    }
    v.extend([
        (
            0.1,
            Until("all twenty stream", |d, _| busy(d) >= MANY / 2, 30.),
        ),
        (0.3, Shot("load-streaming")),
        (0.1, Call(say_memory)),
    ]);
    // Through the sessions while they stream, as someone checking on them:
    // every one opened in turn, oldest first.
    for _ in 0..MANY {
        v.push((0.3, Call(open_next)));
    }
    v.extend([
        (0.2, Shot("load-switched")),
        (
            0.1,
            Until(
                "every one finishes, whole",
                |d, _| {
                    d.sessions.len() >= MANY && d.sessions.iter().all(|s| s.status == Status::Done)
                },
                300.,
            ),
        ),
        (0.1, Call(say_memory)),
        (0.5, Shot("load-done")),
        (
            0.5,
            Idle("with twenty finished sessions, the window rests", 3., 1.0),
        ),
    ]);
    v
}

fn busy(d: &Dent) -> usize {
    d.sessions
        .iter()
        .filter(|s| s.status == Status::Working)
        .count()
}

/// The app's resident memory, said in the walk's output.
fn say_memory(d: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok();
    let kib: u64 = out
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0);
    eprintln!(
        "load: {} sessions, {} working, app memory {} MB",
        d.sessions.len(),
        busy(d),
        kib / 1024
    );
}

/// Opens the session after the one showing, by when each began.
fn open_next(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>) {
    let ids: Vec<String> = d.sessions.iter().map(|s| s.id.clone()).collect();
    let at = current(d)
        .and_then(|c| ids.iter().position(|id| *id == c.id))
        .unwrap_or(0);
    let next = ids.get((at + 1) % ids.len().max(1)).cloned();
    d.select(next, w, cx);
}
