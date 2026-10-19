use super::*;

fn zoom(d: &Dent) -> f32 {
    d.lightbox_zoom().map_or(0., |(z, _)| z)
}

fn pan(d: &Dent) -> (f32, f32) {
    d.lightbox_zoom().map_or((0., 0.), |(_, p)| p)
}

thread_local! {
    /// The text size before ⌘− was pressed over the picture.
    static TEXT: std::cell::Cell<f32> = const { std::cell::Cell::new(0.) };
}

/// Pictures zoom and pan large; `#3` links to the session's repository's
/// pull request and shows what it is when the pointer rests on it.
pub(super) fn zoom_links() -> Vec<(f32, Act)> {
    let mut v = new_session("screenshot the hero");
    v.extend([
        (
            0.1,
            Until("the screenshot turn ends", |d, _| answered(d), 30.),
        ),
        (0.2, Call(|d, _, cx| open_tool_picture(d, 0, cx))),
        (
            0.1,
            Until(
                "the picture opens large, fitted",
                |d, _| zoom(d) == 1. && drawn(d, "lightbox-picture").is_some(),
                3.,
            ),
        ),
        (0.6, Shot("zoom-fit")),
        (0.1, Key("=")),
        (
            0.1,
            Until(
                "= zooms in a step, eased",
                |d, _| (zoom(d) - 1.5).abs() < 0.01,
                2.,
            ),
        ),
        (0.3, Shot("zoom-step")),
        // Larger than the window, so there's more of it to see.
        (0.1, Key("=")),
        (0.1, Key("=")),
        (0.1, Until("each step zooms on", |d, _| zoom(d) > 3., 2.)),
        (0.2, Press("lightbox-picture", 0.5, 0.5)),
        (0.1, MoveBy(-160., -90.)),
        (0.1, Release),
        (
            0.1,
            Until(
                "a drag moves it, held inside its edges",
                |d, _| pan(d).0 < -1. && pan(d).1 < -1.,
                2.,
            ),
        ),
        (0.4, Shot("zoom-dragged")),
        (0.1, Key("0")),
        (
            0.1,
            Until(
                "0 fits it again, centred",
                |d, _| zoom(d) == 1. && pan(d) == (0., 0.),
                2.,
            ),
        ),
        (0.2, DoubleClick("lightbox-picture", 0.3, 0.3)),
        (
            0.1,
            Until(
                "a double-click zooms to twice its own size",
                |d, _| (zoom(d) - 2.).abs() < 0.01,
                2.,
            ),
        ),
        (0.4, Shot("zoom-double-click")),
        (0.0, Call(|_, _, _| TEXT.with(|t| t.set(crate::ds::zoom())))),
        (0.1, Key("cmd--")),
        (
            0.1,
            Until(
                "⌘− zooms the picture out, not the text",
                |d, _| zoom(d) < 1.99 && TEXT.with(|t| t.get()) == crate::ds::zoom(),
                2.,
            ),
        ),
        (0.1, Key("escape")),
        (
            0.1,
            Until("Esc puts it back", |d, _| d.lightbox.is_none(), 3.),
        ),
        // The project's repository is on GitHub.
        (
            0.1,
            Call(|_, _, _| {
                git_in_project(&[
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/SarimDEV/dent.git",
                ])
            }),
        ),
    ]);
    v.extend(new_session("link the pull request"));
    v.extend([
        (
            0.1,
            Until(
                "#3 and SarimDEV/dent#2 link to GitHub; issue#9 and code don't",
                |d, _| {
                    let links = crate::select::drawn_links();
                    done_saying(d, "aren't references")
                        && links
                            .iter()
                            .any(|l| l == "https://github.com/SarimDEV/dent/issues/3")
                        && links
                            .iter()
                            .any(|l| l == "https://github.com/SarimDEV/dent/issues/2")
                        && !links
                            .iter()
                            .any(|l| l.ends_with("/issues/9") || l.ends_with("/issues/4"))
                },
                12.,
            ),
        ),
        (0.3, HoverLink("https://github.com/SarimDEV/dent/issues/3")),
        (
            0.1,
            Until(
                "resting on it shows its card",
                |d, _| drawn(d, "github-card").is_some(),
                3.,
            ),
        ),
        (
            0.1,
            Until(
                "the card says what GitHub says it is",
                |d, _| {
                    d.github
                        .get(&("SarimDEV/dent".to_string(), 3))
                        .is_some_and(|(l, _)| matches!(l, crate::github::Look::Found(_)))
                },
                12.,
            ),
        ),
    ]);
    both_themes(&mut v, "github-card");
    v.extend([
        // The pointer stays put while the page changes under it.
        (0.1, Key("cmd-n")),
        (
            0.1,
            Until(
                "a card whose link is gone goes too",
                |d, _| crate::select::hovered_link().is_none() && drawn(d, "github-card").is_none(),
                3.,
            ),
        ),
    ]);
    v
}
