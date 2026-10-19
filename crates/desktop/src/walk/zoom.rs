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
    /// The zoom each frame drew, by frame and when, while a move is watched.
    static SEEN: std::cell::RefCell<Vec<(u64, Instant, f32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn watch(_: &mut Dent, _: &mut Window, _: &mut Context<Dent>) {
    SEEN.with(|s| s.borrow_mut().clear());
}

/// Note this frame's zoom, once a frame.
fn seen(d: &Dent) -> Vec<(u64, Instant, f32)> {
    SEEN.with(|s| {
        let mut s = s.borrow_mut();
        if s.last().is_none_or(|(f, ..)| *f != d.frames) {
            s.push((d.frames, Instant::now(), zoom(d)));
        }
        s.clone()
    })
}

/// An eased move to `to` that ran over many frames, every frame onward
/// from the last, and none a jump: no frame's step more than twice and a
/// half its neighbours' (a jump stands out from the frames around it,
/// where uneven frame times don't).
fn smooth_to(d: &Dent, to: f32) -> bool {
    let s = seen(d);
    if (zoom(d) - to).abs() > 1e-4 || s.len() < 2 {
        return false;
    }
    let sign = (to / s[0].2).ln().signum();
    let steps: Vec<f32> = s
        .windows(2)
        .map(|w| (w[1].2 / w[0].2).ln() * sign)
        .filter(|x| *x != 0.)
        .collect();
    let spike = |i: usize| {
        let near = [i.checked_sub(1), Some(i + 1)]
            .into_iter()
            .flatten()
            .filter_map(|j| steps.get(j))
            .fold(0f32, |a, b| a.max(*b));
        steps[i] > near * 2.5 + 1e-3
    };
    let ok = steps.len() >= 5 && steps.iter().all(|x| *x > 0.) && !(0..steps.len()).any(spike);
    if !ok {
        let secs = s.last().unwrap().1.duration_since(s[0].1).as_secs_f32();
        eprintln!(
            "      zoom by frame ({:.0} fps): {:?}",
            s.len() as f32 / secs.max(1e-3),
            s.iter().map(|x| x.2).collect::<Vec<_>>()
        );
    }
    ok
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
        (0.1, Call(watch)),
        (0.0, Key("=")),
        (
            0.0,
            Until(
                "= eases in a step over many frames, never jumping or turning back",
                |d, _| smooth_to(d, 1.5),
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
        // Fingers: a pinch follows them at once; past fitting it stretches,
        // and settles back once they lift.
        (0.2, Pinch("lightbox-picture", 3.)),
        (
            0.0,
            Until(
                "a pinch follows the fingers, with no easing behind them",
                |d, _| (zoom(d) - 3.).abs() < 0.01,
                1.,
            ),
        ),
        (0.2, Call(watch)),
        (0.0, Pinch("lightbox-picture", 0.15)),
        (
            0.0,
            Until(
                "pinched past fitting it gives, then settles back smoothly",
                |d, _| {
                    let s = seen(d);
                    let least = s.iter().map(|x| x.2).fold(f32::MAX, f32::min);
                    least < 0.95 && least > 0.5 && smooth_to(d, 1.)
                },
                2.,
            ),
        ),
        (0.2, ZoomScroll("lightbox-picture", 200.)),
        (
            0.0,
            Until(
                "⌘-scrolling a trackpad zooms with the fingers: 200 pt, e×",
                |d, _| (zoom(d) - std::f32::consts::E).abs() < 0.02,
                1.,
            ),
        ),
        (0.1, Key("0")),
        (0.1, Until("fitted again", |d, _| zoom(d) == 1., 2.)),
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
