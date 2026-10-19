use super::*;

pub(super) fn pin_first(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    // The oldest session: the one furthest down.
    if let Some(id) = d.order().last().cloned() {
        MARK.with(|m| m.set(d.sessions.iter().position(|s| s.id == id).unwrap_or(0)));
        d.toggle_pin(&id, cx);
    }
}

pub(super) fn marked_id(d: &Dent) -> String {
    d.sessions
        .get(mark())
        .map(|s| s.id.clone())
        .unwrap_or_default()
}

/// Pinning keeps a session at the top; a file's Open offers Open with.
pub(super) fn pins() -> Vec<(f32, Act)> {
    let mut v = Vec::new();
    for t in ["first notes", "second notes"] {
        v.extend(new_session(t));
        v.push((
            0.1,
            Until(leak(format!("“{t}” answers")), |d, _| answered(d), 20.),
        ));
    }
    v.extend([
        (0.2, Call(pin_first)),
        (
            0.2,
            Until(
                "a pinned session moves to the top, under Pinned",
                |d, _| d.order().first() == Some(&marked_id(d)) && d.pinned.len() == 1,
                1.,
            ),
        ),
        (0.3, Shot("pinned")),
        (
            0.8,
            Call(|d, _, cx| {
                let id = marked_id(d);
                d.toggle_pin(&id, cx);
            }),
        ),
        (
            0.2,
            Until(
                "unpinned, it goes back to its section",
                |d, _| d.pinned.is_empty() && d.order().first() != Some(&marked_id(d)),
                1.,
            ),
        ),
    ]);
    v.extend(new_session("write about.md please"));
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
                "the file opens with Open and Open with beside it",
                |d, _| drawn(d, "open-doc").is_some() && drawn(d, "open-doc-with").is_some(),
                10.,
            ),
        ),
        (0.3, Shot("open-with")),
    ]);
    v
}
