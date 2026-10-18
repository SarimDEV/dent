use super::*;

/// Something that went wrong can be tried again, unless a used-up limit
/// means it would only fail again.
pub(super) fn failures() -> Vec<(f32, Act)> {
    let mut v = new_session("crash halfway please");
    v.extend([
        (
            0.1,
            Until(
                "when Claude Code stops unexpectedly, the note offers Try again",
                |d, _| {
                    has(d, |i| matches!(i, Item::Note { kind: crate::data::NoteKind::Error, text, .. } if text.contains("stopped unexpectedly")))
                        && (drawn(d, "retry-1").is_some() || drawn(d, "retry-2").is_some())
                },
                15.,
            ),
        ),
    ]);
    v.extend(new_session("hit the limit now"));
    v.extend([
        (
            0.1,
            Until(
                "a used-up limit says which and when it resets, and Try again waits for it",
                |d, _| {
                    !current(d).is_some_and(|s| s.running())
                        && d.limit_hit().is_some_and(|(w, _)| w == "5-hour limit")
                        && has(d, |i| {
                            matches!(
                                i,
                                Item::Note {
                                    kind: crate::data::NoteKind::Error,
                                    ..
                                }
                            )
                        })
                },
                10.,
            ),
        ),
        (0.4, Shot("limit")),
        (0.8, Wait),
    ]);
    v
}
