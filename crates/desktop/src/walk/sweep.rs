use super::*;

/// Every session the daemon has, opened one by one: for checking the app
/// against real transcripts loaded with `dent-daemon check-transcripts --into`.
pub(super) fn sweep() -> Vec<(f32, Act)> {
    vec![
        (
            0.5,
            Until(
                "every session's history arrives",
                |d, _| d.core.live && d.sessions.len() > 1,
                120.,
            ),
        ),
        (0.5, Sweep),
    ]
}

// ---- Helpers for checks ----
