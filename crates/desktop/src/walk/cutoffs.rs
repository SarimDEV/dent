use super::*;

/// What a session says when something other than you cut its turn off:
/// dent stopping an agent for its memory, and a turn the holder's death
/// cut off carrying on by itself. Run with the limits low, as walk.sh's
/// `cutoffs` does.
pub(super) fn cutoffs() -> Vec<(f32, Act)> {
    let mut v = new_session("eat memory please");
    v.extend([
        (
            0.1,
            Until(
                "an agent past the memory limit is stopped, and the turn says why",
                |d, _| noted(d, "past this machine's limit"),
                30.,
            ),
        ),
        (0.5, Shot("cutoff-memory")),
    ]);
    v.extend(new_session("work quietly please"));
    v.extend([
        (0.1, Until("it works", |d, _| status_is(d, Status::Working), 10.)),
        (0.5, Call(kill_holder)),
        (
            0.1,
            Until(
                "cut off by the holder going, it carries on by itself",
                |d, _| {
                    current(d).is_some_and(|s| {
                        s.items.iter().any(|i| {
                            matches!(i, Item::User { text, .. } if text.starts_with("You were cut off"))
                        })
                    })
                },
                20.,
            ),
        ),
        (0.8, Shot("cutoff-carried-on")),
    ]);
    v
}

/// A note in the open session (an error or a notice) saying `needle`.
fn noted(d: &Dent, needle: &str) -> bool {
    current(d).is_some_and(|s| {
        s.items
            .iter()
            .any(|i| matches!(i, Item::Note { text, .. } if text.contains(needle)))
    })
}

/// Kills this walk's holder, found from its own state folder: never
/// another daemon's.
fn kill_holder(_: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    let Ok(state) = std::env::var("DENT_STATE") else {
        return;
    };
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in std::path::Path::new(&state).as_os_str().as_encoded_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let tmp = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let lock = tmp.join(format!("dent-{h:016x}")).join("holder.lock");
    if let Some(pid) = std::fs::read_to_string(lock)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .filter(|p| *p > 1)
    {
        // SAFETY: kill has no memory preconditions.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}
