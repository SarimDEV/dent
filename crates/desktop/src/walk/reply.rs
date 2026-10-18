use super::*;

/// When the session's latest turn ended, in ms.
pub(super) fn last_end(d: &Dent) -> u64 {
    current(d).map_or(0, |s| {
        s.items
            .iter()
            .rev()
            .find_map(|i| match i {
                Item::TurnEnd { at, .. } => Some(*at),
                _ => None,
            })
            .unwrap_or(0)
    })
}

pub(super) fn note_end(d: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    MARK.with(|m| m.set(last_end(d) as usize));
}

/// Under a finished reply: its time, Copy, and Try again.
pub(super) fn reply_actions() -> Vec<(f32, Act)> {
    let mut v = vec![(0., Call(save_clipboard))];
    v.extend(new_session("check the parser"));
    v.extend([
        (0.1, Until("it answers", |d, _| answered(d), 20.)),
        (
            0.2,
            Until(
                "the latest reply shows Copy and Try again",
                |d, _| drawn(d, "copy-reply-1").is_some() && drawn(d, "try-again").is_some(),
                2.,
            ),
        ),
        (0.3, Shot("reply-actions")),
        (0.8, Click("copy-reply-1")),
        (
            0.1,
            Until(
                "Copy takes the whole reply, as Markdown",
                |_, cx| copied(cx).starts_with("Here's what I'd do about **check the parser**"),
                1.,
            ),
        ),
        (0.1, Call(note_end)),
        (0.2, Click("try-again")),
        (
            0.1,
            Until(
                "Try again, with no files changed, sends the message again in its place",
                |d, _| answered(d) && mine(d) == 1 && last_end(d) as usize > mark(),
                20.,
            ),
        ),
    ]);
    v.extend(new_session("write redo.md please"));
    v.extend([
        (
            0.1,
            Until(
                "asks to write",
                |d, _| has(d, |i| matches!(i, Item::Approval { .. })),
                10.,
            ),
        ),
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "the file is written",
                |d, _| status_is(d, Status::Done) && in_project("redo.md").exists(),
                10.,
            ),
        ),
        (0.3, Click("try-again")),
        (
            0.1,
            Until(
                "when the turn changed files, Try again asks about them first",
                |d, _| {
                    drawn(d, "try-again-files").is_some() && drawn(d, "try-again-keep").is_some()
                },
                2.,
            ),
        ),
        (0.3, Shot("try-again-files")),
        (0.8, Click("try-again-files")),
        (
            0.1,
            Until(
                "undoing them takes the file away, and the message goes again",
                |d, _| {
                    !in_project("redo.md").exists()
                        && has(d, |i| matches!(i, Item::Approval { .. }))
                        && mine(d) == 1
                },
                20.,
            ),
        ),
        (0.2, Click("allow")),
        (
            0.1,
            Until(
                "and it's written again",
                |d, _| status_is(d, Status::Done) && in_project("redo.md").exists(),
                10.,
            ),
        ),
        (0.1, Call(restore_clipboard)),
    ]);
    v
}
