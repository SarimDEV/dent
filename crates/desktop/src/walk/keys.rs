use super::*;

pub(super) fn noted(d: &Dent, start: &str) -> bool {
    current(d).is_some_and(|s| {
        s.items
            .iter()
            .any(|i| matches!(i, Item::Note { text, .. } if text.starts_with(start)))
    })
}

/// Approvals and plans answer to keys on their card, and only there.
pub(super) fn answer_keys() -> Vec<(f32, Act)> {
    fn waiting(d: &Dent) -> bool {
        has(d, |i| matches!(i, Item::Approval { .. })) && status_is(d, Status::Needs)
    }
    let mut v = new_session("please run it");
    v.extend([
        (0.1, Until("an approval waits", |d, _| waiting(d), 10.)),
        (0.2, Keys("1")),
        (
            0.3,
            Until(
                "a 1 typed in the reply box is just a 1: the approval still waits",
                |d, cx| waiting(d) && composer_is(d, cx, "1"),
                1.,
            ),
        ),
        (0.1, Type(Field::Composer, "")),
        (0.1, Key("escape")),
        (
            0.1,
            Until(
                "Esc from the empty box takes the keyboard to the card",
                |d, _| d.ask_keyed,
                1.,
            ),
        ),
        (0.3, Shot("approval-keys")),
        (0.8, Key("1")),
        (
            0.1,
            Until(
                "1 on the card allows once: no lasting rule, no note",
                |d, _| done_saying(d, "Ran it") && !noted(d, "Allowed"),
                10.,
            ),
        ),
        (
            0.1,
            Until(
                "the keyboard goes back to the reply box",
                |d, _| !d.ask_keyed,
                1.,
            ),
        ),
        (0.2, Type(Field::Composer, "run it again")),
        (0.1, Key("enter")),
        (0.1, Until("another approval", |d, _| waiting(d), 10.)),
        (0.1, Key("escape")),
        (0.2, Key("2")),
        (
            0.1,
            Until(
                "2 allows for this session",
                |d, _| {
                    status_is(d, Status::Done)
                        && noted(d, "Allowed echo hello > hello.txt for this session")
                },
                10.,
            ),
        ),
    ]);
    v.extend(new_session("run it, please"));
    v.extend([
        (0.1, Until("a third approval", |d, _| waiting(d), 10.)),
        (0.1, Key("escape")),
        (0.2, Key("3")),
        (
            0.1,
            Until(
                "3 allows it in this project from now on",
                |d, _| {
                    status_is(d, Status::Done)
                        && noted(d, "Allowed echo hello > hello.txt in this project")
                        && std::fs::read_to_string(in_project(".claude/settings.local.json"))
                            .is_ok_and(|s| s.contains("Bash(echo hello > hello.txt)"))
                },
                10.,
            ),
        ),
    ]);
    v.extend(new_session("run it once more"));
    v.extend([
        (
            0.1,
            Until(
                "and a later session runs it without asking",
                |d, _| done_saying(d, "Ran it") && !has(d, |i| matches!(i, Item::Approval { .. })),
                10.,
            ),
        ),
        // Leave the project as it was, so later checks are asked again.
        (
            0.1,
            Call(|_, _, _| {
                let _ = std::fs::remove_file(in_project(".claude/settings.local.json"));
            }),
        ),
    ]);
    v.extend(new_session("write keep.md please"));
    v.extend([
        (0.1, Until("a write waits", |d, _| waiting(d), 10.)),
        (0.1, Key("escape")),
        (0.2, Key("3")),
        (
            0.3,
            Until(
                "with no rule to save, 3 does nothing and says why on hover",
                |d, _| waiting(d) && drawn(d, "allow-project").is_some(),
                1.,
            ),
        ),
        (0.2, Shot("allow-project-off")),
        (0.8, Key("1")),
        (
            0.1,
            Until("it's written", |d, _| status_is(d, Status::Done), 10.),
        ),
    ]);
    v.extend(new_session("run it, please"));
    v.extend([
        (0.1, Until("a fourth approval", |d, _| waiting(d), 10.)),
        (
            0.1,
            Until(
                "a new session's reply box has the keyboard",
                |d, _| d.composer_keyed,
                1.,
            ),
        ),
        (0.1, Key("escape")),
        (
            0.1,
            Until("and Esc takes it to the card", |d, _| d.ask_keyed, 1.),
        ),
        (0.2, Key("4")),
        (
            0.1,
            Until(
                "4 denies",
                |d, _| status_is(d, Status::Done) && noted(d, "Denied"),
                10.,
            ),
        ),
    ]);
    v.extend(new_session("present a plan"));
    v.extend([
        (
            0.1,
            Until(
                "a plan waits",
                |d, _| has(d, |i| matches!(i, Item::Plan { .. })),
                10.,
            ),
        ),
        (0.1, Key("escape")),
        (0.2, Key("enter")),
        (
            0.1,
            Until(
                "Return on its card starts building",
                |d, _| done_saying(d, "Building it now") && noted(d, "Approved the plan"),
                10.,
            ),
        ),
    ]);
    // A long plan folds to its start, its actions still in view.
    v.extend(new_session("present a long plan"));
    v.extend([
        (
            0.1,
            Until(
                "a long plan waits",
                |d, _| has(d, |i| matches!(i, Item::Plan { .. })),
                10.,
            ),
        ),
        (0.2, Call(to_end)),
        (0.4, Shot("plan-long")),
        (0.1, Key("escape")),
        (0.2, Key("enter")),
        (
            0.1,
            Until(
                "and starts building like a short one",
                |d, _| done_saying(d, "Building it now"),
                10.,
            ),
        ),
    ]);
    v
}
