use super::*;

/// The chat's small things, for stills: a tool's picture, thinking and
/// call times, and the sidebar's marks (unread, a draft).
pub(super) fn polish() -> Vec<(f32, Act)> {
    let mut v = new_session("first notes");
    v.push((
        0.1,
        Until("the first session answers", |d, _| answered(d), 20.),
    ));
    v.extend(new_session("take a screenshot of the page please"));
    v.extend([
        (
            0.1,
            Until("the screenshot turn ends", |d, _| answered(d), 30.),
        ),
        (
            0.1,
            Until(
                "the tool's picture shows at a size you can see",
                |d, _| {
                    drawn(d, "tool-picture")
                        .is_some_and(|b| b.size.width > px(200.) && b.size.height > px(100.))
                },
                3.,
            ),
        ),
        (0.6, Shot("screenshot")),
        (0.2, Call(open_every_group)),
        (0.3, Call(to_tools)),
        (0.6, Shot("screenshot-open")),
        (
            0.2,
            Type(Field::Composer, "half a thought about the footer"),
        ),
    ]);
    v.extend(new_session("second notes"));
    v.extend([
        (0.1, Call(|d, w, cx| select_titled(d, w, cx, "first notes"))),
        (
            0.1,
            Until(
                "the second session answers while another is open",
                |d, _| {
                    d.sessions
                        .iter()
                        .any(|s| s.title.contains("second notes") && s.status == Status::Done)
                },
                20.,
            ),
        ),
        (0.8, Shot("sidebar")),
        // ⌘-click two more rows: three picked, to close or pin together.
        (
            0.2,
            Call(|d, _, cx| {
                for words in ["second notes", "landing page"] {
                    if let Some(id) = d
                        .sessions
                        .iter()
                        .find(|s| s.title.contains(words))
                        .map(|s| s.id.clone())
                    {
                        d.pick(&id, false, cx);
                    }
                }
            }),
        ),
        (
            0.1,
            Until("three rows are picked", |d, _| d.picked.len() == 3, 2.),
        ),
        (0.6, Shot("picked")),
        (0.2, Key("escape")),
        (
            0.1,
            Until("Esc lets them go", |d, _| d.picked.is_empty(), 2.),
        ),
    ]);
    v
}

pub(super) fn pictures() -> Vec<(f32, Act)> {
    let mut v = new_session("screenshot the page at every size");
    v.extend([
        (
            0.1,
            Until("the screenshots turn ends", |d, _| answered(d), 30.),
        ),
        (
            0.1,
            Until(
                "the group holds all six pictures",
                |d, _| tool_picture_count(d) == 6,
                3.,
            ),
        ),
        (0.2, Call(to_tools)),
        (0.6, Shot("pictures")),
        (0.2, Call(open_every_group)),
        (0.2, Call(to_tools)),
        (0.6, Shot("pictures-open")),
        (0.2, Call(|d, _, cx| open_tool_picture(d, 2, cx))),
        (
            0.1,
            Until("a tile opens large", |d, _| d.lightbox.is_some(), 3.),
        ),
        (0.8, Shot("pictures-viewer")),
        // Onto a picture the "+3" tile stands for.
        (0.2, Key("right")),
        (0.2, Key("right")),
        (0.2, Key("right")),
        (0.5, Shot("pictures-viewer-last")),
        (0.2, Key("escape")),
        (
            0.1,
            Until("Esc puts it back", |d, _| d.lightbox.is_none(), 3.),
        ),
    ]);
    v
}

/// How many pictures the open session's first tool group brought back.
pub(super) fn tool_picture_count(d: &Dent) -> usize {
    current(d)
        .and_then(|s| {
            s.items.iter().find_map(|i| match i {
                Item::Tools { pictures, .. } => Some(pictures.len()),
                _ => None,
            })
        })
        .unwrap_or(0)
}

/// Open the `at`th picture of the first tool group large, as a click does.
pub(super) fn open_tool_picture(d: &mut Dent, at: usize, cx: &mut Context<Dent>) {
    let Some(s) = current(d) else { return };
    let Some((ix, paths)) = s.items.iter().enumerate().find_map(|(ix, i)| match i {
        Item::Tools { pictures, .. } => Some((ix, pictures.clone())),
        _ => None,
    }) else {
        return;
    };
    let set = std::rc::Rc::new(
        paths
            .iter()
            .enumerate()
            .map(|(j, p)| crate::lightbox::Shot {
                key: SharedString::from(format!("tool-pic-{}-{ix}-{j}", s.id)),
                path: std::path::PathBuf::from(p),
            })
            .collect(),
    );
    crate::lightbox::ask(set, at);
    cx.notify();
}

/// Scroll the conversation so its first tool group starts at the top.
pub(super) fn to_tools(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    let at = current(d).and_then(|s| s.items.iter().position(|i| matches!(i, Item::Tools { .. })));
    if let Some(ix) = at {
        d.transcript.scroll_to(ListOffset {
            item_ix: ix.saturating_sub(1),
            offset_in_item: px(0.),
        });
        cx.notify();
    }
}

/// Open every tool group and lone call in the open session.
pub(super) fn open_every_group(d: &mut Dent, _: &mut Window, cx: &mut Context<Dent>) {
    let Some(s) = d.current() else { return };
    let mut groups = Vec::new();
    let mut calls = Vec::new();
    for i in s.items.iter() {
        if let Item::Tools { lines, .. } = i {
            if let Some(l) = lines.first() {
                groups.push(l.id.clone());
            }
            calls.extend(lines.iter().map(|l| l.id.clone()));
        }
    }
    d.open_tools.extend(groups);
    d.open_calls.extend(calls);
    cx.notify();
}

/// Open the session whose title holds `words`.
pub(super) fn select_titled(d: &mut Dent, w: &mut Window, cx: &mut Context<Dent>, words: &str) {
    let id = d
        .sessions
        .iter()
        .find(|s| s.title.contains(words))
        .map(|s| s.id.clone());
    if id.is_some() {
        d.select(id, w, cx);
    }
}
