//! Pictures open large: click one in the chat (a screenshot you attached, a
//! picture in a reply, an attachment chip) and it zooms from where it sits to
//! the middle of the window as the backdrop darkens. Esc, Space or a click
//! outside zooms it back.
//!
//! Every picture that opens is drawn through `picture`, which notes where it
//! sits each frame, so the lift starts from the thumbnail itself.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::anim;
use crate::app::Dent;
use crate::ds::{LABEL, Typed, motion, radius, space};
use crate::ui::{Control, Press, icon, tip};

/// The lift takes a little longer than a small enter: the picture travels.
const OPEN_MS: f32 = motion::SLOW + 40.;
/// Exits are quicker than enters.
const CLOSE_MS: f32 = motion::BASE;

/// A picture open large.
pub struct Lightbox {
    path: PathBuf,
    /// Where it sat when clicked, in window coordinates.
    from: Bounds<Pixels>,
    /// Its size in pixels, (0, 0) if unreadable.
    size: (u32, u32),
    opened: Instant,
    closing: Option<Instant>,
}

thread_local! {
    /// Where each picture that can open was drawn last frame, by its id:
    /// the same file shown twice lifts from the one clicked.
    static PLACED: RefCell<HashMap<SharedString, Bounds<Pixels>>> = RefCell::default();
    /// A picture clicked this frame, opened by the next render.
    static ASKED: RefCell<Option<(SharedString, PathBuf)>> = const { RefCell::new(None) };
}

/// A picture that opens large when clicked. `inner` is what's shown in place
/// (the thumbnail, sized by the caller); the wrapper notes where it sits.
pub fn picture(id: SharedString, path: &Path, inner: impl IntoElement) -> Stateful<Div> {
    let at = path.to_path_buf();
    let (key, mark) = (id.clone(), id.clone());
    div()
        .id(id)
        .relative()
        .cursor_pointer()
        // A press here opens the picture, never starts a text selection.
        .control()
        .on_press(move |_, window, _| {
            ASKED.with(|a| *a.borrow_mut() = Some((key.clone(), at.clone())));
            window.refresh();
        })
        .child(inner)
        .child(
            canvas(
                move |bounds, _, _| {
                    PLACED.with(|p| p.borrow_mut().insert(mark.clone(), bounds));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
}

/// A picture's size in pixels, read from the file's header.
fn pixel_size(path: &Path) -> (u32, u32) {
    image::ImageReader::open(path)
        .ok()
        .and_then(|r| r.with_guessed_format().ok())
        .and_then(|r| r.into_dimensions().ok())
        .unwrap_or((0, 0))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Dent {
    /// Open a picture clicked since the last frame, from where it sat.
    pub fn take_lightbox_request(&mut self, window: &Window) {
        let Some((key, path)) = ASKED.with(|a| a.borrow_mut().take()) else {
            return;
        };
        let from = PLACED
            .with(|p| p.borrow().get(&key).copied())
            .unwrap_or_else(|| {
                // Nowhere known: from a point in the middle.
                let c = window.viewport_size();
                Bounds::centered_at(point(c.width / 2., c.height / 2.), size(px(1.), px(1.)))
            });
        let size = pixel_size(&path);
        self.lightbox = Some(Lightbox {
            path,
            from,
            size,
            opened: Instant::now(),
            closing: None,
        });
    }

    /// Send it back to where it came from. True if one was open.
    pub fn close_lightbox(&mut self, cx: &mut Context<Self>) -> bool {
        match &mut self.lightbox {
            Some(l) if l.closing.is_none() => {
                // Leaving mid-lift starts from where it got to.
                let shown = 1. - anim::progress(l.opened, OPEN_MS);
                l.closing = Some(
                    Instant::now() - std::time::Duration::from_secs_f32(shown * CLOSE_MS / 1000.),
                );
                cx.notify();
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    pub fn lightbox_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let l = self.lightbox.as_ref()?;
        let t = self.t;
        let still = crate::reduce_motion();
        // How open it is, 0 at the thumbnail, 1 in the middle.
        let open = match l.closing {
            Some(c) => {
                let p = if still {
                    1.
                } else {
                    anim::progress(c, CLOSE_MS)
                };
                if p >= 1. {
                    self.lightbox = None;
                    cx.notify();
                    return None;
                }
                1. - anim::ease_out(p)
            }
            None if still => 1.,
            None => anim::ease_out(anim::progress(l.opened, OPEN_MS)),
        };
        let moving = open > 0. && open < 1.;
        // The backdrop leads the picture both ways: dark by three quarters
        // of the lift, and clear before the picture is home again.
        let cover = if l.closing.is_some() {
            ((open - 0.3) / 0.7).clamp(0., 1.)
        } else {
            (open / 0.75).min(1.)
        };
        if moving {
            window.request_animation_frame();
        }
        // The picture fits the window with room for its name below, never
        // larger than it is (in points, at the screen's scale).
        let vw = window.viewport_size();
        let (vw, vh) = (vw.width.as_f32(), vw.height.as_f32());
        let scale = window.scale_factor().max(1.);
        let (pw, ph) = match l.size {
            (0, 0) | (_, 0) | (0, _) => (
                l.from.size.width.as_f32().max(1.),
                l.from.size.height.as_f32().max(1.),
            ),
            (w, h) => (w as f32 / scale, h as f32 / scale),
        };
        // Room to breathe around it: the window shows through the dither.
        let room_w = (vw * 0.84).min(vw - 2. * space::S12).max(80.);
        let room_h = (vh * 0.78).min(vh - 2. * space::S12 - space::S8).max(80.);
        let k = (room_w / pw).min(room_h / ph).min(1.);
        let (tw, th) = (pw * k, ph * k);
        let (tx, ty) = ((vw - tw) / 2., (vh - th - space::S8) / 2.);
        let f = l.from;
        let x = lerp(f.origin.x.as_f32(), tx, open);
        let y = lerp(f.origin.y.as_f32(), ty, open);
        let w = lerp(f.size.width.as_f32(), tw, open);
        let h = lerp(f.size.height.as_f32(), th, open);
        let name = l
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let caption = match l.size {
            (0, 0) => name,
            (pw, ph) => format!("{name} · {pw} × {ph}"),
        };
        // The backdrop is dark in either theme, as photos are shown.
        let shade = gpui_kit::black().opacity(if t.dark { 0.78 } else { 0.66 });
        Some(
            div()
                .id("lightbox")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                // Takes every press: nothing under it is clicked.
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_lightbox(cx);
                    }),
                )
                // The backdrop fades with the zoom, leading it both ways.
                .child(
                    div()
                        .absolute()
                        .size_full()
                        .bg(shade.opacity(shade.a * cover)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(w))
                        .h(px(h))
                        .rounded(px(lerp(radius::MD, radius::LG, open)))
                        .overflow_hidden()
                        // A faint light edge, so a dark picture still reads
                        // against the dark backdrop; the shadow only once it
                        // has landed (shadows are never animated).
                        .border_1()
                        .border_color(gpui_kit::white().opacity(0.14 * open))
                        .when(!moving, |d| {
                            d.shadow(vec![BoxShadow {
                                color: hsla(0., 0., 0., 0.5),
                                offset: point(px(0.), px(16.)),
                                blur_radius: px(48.),
                                spread_radius: px(0.),
                                inset: false,
                            }])
                        })
                        // A press on the picture itself keeps it open.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            img(l.path.clone())
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(tx))
                        .top(px(ty + th + space::S3))
                        .w(px(tw))
                        .flex()
                        .justify_center()
                        .opacity(((open - 0.6) / 0.4).clamp(0., 1.))
                        // On its own band, clear of whatever shows through.
                        .child(
                            div()
                                .px(px(space::S2))
                                .py(px(space::HALF))
                                .rounded(px(radius::SM))
                                .bg(gpui_kit::black().opacity(0.6))
                                .ty(LABEL)
                                .text_color(gpui_kit::white().opacity(0.86))
                                .child(caption),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        // Below the header row, clear of its buttons.
                        .top(px(crate::ds::size::HEADER + space::S3))
                        .right(px(space::S4))
                        .opacity(open)
                        .child(
                            // Light on the dark backdrop, in either theme.
                            div()
                                .id("lightbox-close")
                                .size(px(32.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .bg(gpui_kit::white().opacity(0.12))
                                .hover(|d| d.bg(gpui_kit::white().opacity(0.2)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_press(cx.listener(|this, _, _, cx| {
                                    this.close_lightbox(cx);
                                }))
                                .tooltip(tip("Close", Some("Esc"), t))
                                .child(icon("x", 14., gpui_kit::white().opacity(0.9))),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's own `test` attribute would come with it.
    use super::pixel_size;

    #[test]
    fn reads_a_pictures_size() {
        let dir = std::env::temp_dir().join(format!("dent-lightbox-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.png");
        image::RgbImage::new(30, 20).save(&p).unwrap();
        assert_eq!(pixel_size(&p), (30, 20));
        assert_eq!(pixel_size(&dir.join("missing.png")), (0, 0));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
