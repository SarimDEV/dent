use super::*;

/// Still frames in the current theme and size, after the checks' sessions.
pub(super) fn stills() -> Vec<(f32, Act)> {
    let mut v = checks();
    v.retain(|(_, a)| !matches!(a, Idle(..)));
    v.extend([
        (0.3, Key("cmd-n")),
        (0.3, Keys("Fix the footer on small screens")),
        (0.8, Shot("new-session")),
        (0.3, Key("escape")),
        (0.2, Key("cmd-k")),
        (0.3, Keys("redis")),
        (
            0.1,
            Until(
                "search finds words inside a conversation",
                |d, cx| {
                    d.palette_results(cx)
                        .iter()
                        .any(|id| d.session(id).is_some_and(|s| s.title.contains("cache")))
                },
                3.,
            ),
        ),
        (0.6, Shot("search")),
        (0.2, Key("escape")),
    ]);
    v
}
