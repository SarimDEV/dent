//! The dent desktop app: the session inbox, the chat and the viewer, drawn
//! with GPUI from what this machine's daemon reports.
//!
//! `cargo run -p dent-desktop -- [--dark] [--size WxH] [--reduce-motion] [--background]`
//! starts the daemon beside it when none is running.

mod anim;
mod app;
mod assets;
mod bridge;
mod chat;
mod context;
mod data;
mod diagram;
mod ds;
mod highlight;
mod live;
mod markdown;
mod math;
mod notify;
mod palette;
mod popout;
mod prefs;
mod select;
mod shell;
mod sidebar;
mod theme;
mod ui;
mod viewer;
mod walk;
mod web;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::base::input::{InputEditorStyle, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use app::{
    CloseSession, Dent, Escape, InboxClose, InboxDown, InboxOpen, InboxUp, NewSession,
    NextNeedsYou, Search, SelectNext, SelectPrev, ShowChanges, ShowFirst, ShowTerminal, SideChat,
    SteerSend, StopSession, Tab, ToastAction, TogglePanel, UndoClose,
};
use ds::{Typed, UI, radius, size, space};
use theme::Theme;

pub static PROCESS_START: OnceLock<Instant> = OnceLock::new();
static REDUCE_MOTION: AtomicBool = AtomicBool::new(false);
/// Opened without taking focus: walkthroughs and measurements.
pub static BACKGROUND: AtomicBool = AtomicBool::new(false);
pub static WINDOW: OnceLock<(f32, f32, f32, f32)> = OnceLock::new();

/// A fresh id for a message, so a resend is recognised.
pub fn message_id() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("d{t:x}-{}-{n}", std::process::id())
}

/// Where sessions with no project work.
pub fn scratch_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("dent/scratch")
}

pub fn reduce_motion() -> bool {
    REDUCE_MOTION.load(Ordering::Relaxed)
}

pub fn set_reduce_motion(on: bool) {
    REDUCE_MOTION.store(on, Ordering::Relaxed);
}

pub fn window_rect() -> (f32, f32, f32, f32) {
    WINDOW.get().copied().unwrap_or((40., 40., 1440., 860.))
}

/// Inputs take the page's colours: ink text, quiet placeholder, ink caret.
pub fn style_input(input: &Entity<TextareaState>, t: &Theme, cx: &mut App) {
    let style = InputEditorStyle {
        foreground: t.ink,
        muted_foreground: t.faint,
        background: t.raised.opacity(0.),
        border: t.hairline.opacity(0.),
        selection: t.selection(),
        caret: t.ink,
        ..Default::default()
    };
    input.update(cx, |i, _| i.set_editor_style(style));
}

impl Render for Dent {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_frame = Instant::now();
        // DENT_COUNT_FRAMES prints how many frames each 5 s, to find idle redraws.
        if std::env::var_os("DENT_COUNT_FRAMES").is_some() {
            thread_local!(static T: std::cell::Cell<Option<(Instant, u64)>> = const { std::cell::Cell::new(None) });
            T.with(|t| {
                let (at, n) = t.get().unwrap_or((Instant::now(), self.frames));
                if at.elapsed().as_secs() >= 5 {
                    eprintln!("frames: {} in 5s", self.frames - n);
                    t.set(Some((Instant::now(), self.frames)));
                } else if t.get().is_none() {
                    t.set(Some((at, n)));
                }
            });
        }
        if self.walk.as_ref().is_some_and(|w| w.resting.is_some()) {
            eprintln!(
                "      idle frame: active={} inbox={}",
                window.is_window_active(),
                self.inbox_focus.is_focused(window)
            );
        }
        self.frames += 1;
        ui::FRAME.store(self.frames, std::sync::atomic::Ordering::Relaxed);
        self.drain_page(window, cx);
        // A clicked notification opens its session.
        let clicked = notify::take_clicked();
        if let Some(id) = clicked.clone() {
            self.pending_select = Some(id);
        }
        // A session just created opens once the daemon has told us about it,
        // unless you've already started typing the next task.
        if let Some(id) = self.pending_select.clone()
            && self.session(&id).is_some()
        {
            self.pending_select = None;
            let typing_next =
                self.selected.is_none() && !self.starter.read(cx).value().trim().is_empty();
            if clicked.is_some() || !typing_next {
                self.select(Some(id), window, cx);
            }
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|t| t.at.elapsed() > Duration::from_secs(8))
        {
            self.toast = None;
        }
        let toast_arriving = self
            .toast
            .as_ref()
            .is_some_and(|t| t.at.elapsed() < Duration::from_millis(400));
        if self.walk.is_some() {
            let me = cx.entity();
            window.defer(cx, move |window, cx| walk::tick(me, window, cx));
        }
        let walking = self.walk.as_ref().is_some_and(|d| d.resting.is_none());
        if self.walk.as_ref().is_some_and(|w| w.resting.is_some())
            && (self.animating() || toast_arriving)
        {
            eprintln!(
                "      frame: animating={} toast={toast_arriving}",
                self.animating()
            );
        }
        if self.animating() || toast_arriving || walking {
            window.request_animation_frame();
        }
        // The composer says what Enter will do in this session's state.
        let open = self.current().and_then(|s| {
            s.items.iter().rev().find_map(|i| match i {
                data::Item::Question { .. } => Some("Type your own answer, or pick one above"),
                data::Item::Plan { .. } => Some("What to change in the plan (optional)"),
                data::Item::Approval { .. } => Some("Reason for denying (optional)"),
                _ => None,
            })
        });
        let want = match open {
            Some(w) => w,
            None if self
                .current()
                .is_some_and(|s| s.status == theme::Status::Working) =>
            {
                "Queue a message, or steer with ⌘ ↩"
            }
            None => "Reply",
        };
        if self.placeholder != want {
            self.placeholder = want;
            self.composer
                .update(cx, |c, cx| c.set_placeholder(want, window, cx));
        }
        self.inbox_focused = self.inbox_focus.is_focused(window);
        if self.menu.is_none() && !self.dragging && !self.dragging_sidebar && self.walk.is_none() {
            self.save_prefs();
        }
        let visible = self.preview_visible();
        if let Some(p) = &self.preview {
            p.borrow_mut().set_visible(visible);
            p.borrow().keep_keys_unless_pressed(window.mouse_position());
        }
        let t = self.t;
        // Lay out from the window: the chat keeps its minimum, the viewer
        // shrinks first, and only then does the sidebar fold away.
        let w = window.viewport_size().width.as_f32();
        self.window_h = window.viewport_size().height.as_f32();
        let wants_panel = self.panel_open && self.current().is_some_and(|s| self.shows_panel(s));
        if !wants_panel {
            self.sidebar_forced = false;
        }
        let room = w - self.sidebar_w - viewer::CHAT_MIN;
        self.sidebar_hidden = wants_panel && room < viewer::MIN_WIDTH && !self.sidebar_forced;
        // Folding away takes keyboard focus with it, to the window.
        if self.sidebar_hidden && self.inbox_focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        let side = if self.sidebar_hidden {
            0.
        } else {
            self.sidebar_w
        };
        self.panel_w = self
            .panel_width
            .min(w - side - viewer::CHAT_MIN)
            .max(viewer::MIN_WIDTH);
        self.chat_w = w - side - if wants_panel { self.panel_w } else { 0. };
        let sidebar = (!self.sidebar_hidden).then(|| self.sidebar(cx));
        let main = self.main_column(window, cx);
        let panel = self
            .current_rc()
            .filter(|s| self.panel_open && self.shows_panel(s))
            .map(|s| self.panel(&s, window, cx));
        // Where the viewer starts, for telling which side a press is on.
        self.panel_left = panel
            .as_ref()
            .map(|_| window.viewport_size().width.as_f32() - self.panel_w);
        let toast =
            self.toast.clone().map(|toast| {
                let label = match toast.action {
                    ToastAction::Undo(_) => "Undo",
                    ToastAction::Resume(_) => "Resume",
                    ToastAction::Info => "OK",
                };
                div()
                    .absolute()
                    .bottom(px(size::STATUS + space::S2))
                    .w(px(self.sidebar_w - space::S4))
                    .map(|d| {
                        if self.sidebar_hidden {
                            d.left(px(space::S4))
                        } else {
                            d.left(px(space::S2))
                        }
                    })
                    .child(
                        ui::floating(&t)
                            .rounded(px(radius::LG))
                            .pl(px(space::S3))
                            .pr(px(space::S1))
                            // A long message grows the toast to three lines,
                            // then ends in an ellipsis; its tooltip has it all.
                            .min_h(px(40.))
                            .py(px(space::S1))
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(ui::icon(
                                match toast.action {
                                    ToastAction::Undo(_) => "archive",
                                    ToastAction::Resume(_) => "square",
                                    ToastAction::Info => "info",
                                },
                                14.,
                                t.quiet,
                            ))
                            .child(
                                div()
                                    .id("toast-text")
                                    .flex_1()
                                    .min_w_0()
                                    .ty(UI)
                                    .text_color(t.ink)
                                    .line_clamp(3)
                                    .when(toast.text.len() > 90, |d| {
                                        d.tooltip(ui::tip_owned(toast.text.clone(), t))
                                    })
                                    .child(toast.text.clone()),
                            )
                            .when(matches!(toast.action, ToastAction::Undo(_)), |d| {
                                d.child(ui::kbd("⌘ Z", &t))
                            })
                            .child(
                                ui::button("toast-action", label, false, &t)
                                    .relative()
                                    .border_0()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toast_action(window, cx)
                                    }))
                                    .child(ui::probe(&self.probes, "toast-action")),
                            ),
                    )
            });
        let menu_open = self.menu.is_some();
        div()
            .id("root")
            .key_context("Dent")
            // A click anywhere in dent takes the keyboard back from the page.
            .capture_any_mouse_down(cx.listener(|this, e: &MouseDownEvent, window, cx| {
                if let Some(p) = &this.preview {
                    p.borrow().release_focus();
                }
                // A press outside the viewer: the chat's text selects.
                let in_viewer = this.panel_left.is_some_and(|x| e.position.x.as_f32() >= x);
                select::hover(in_viewer, true, window, cx);
            }))
            // Clicking a button, a chip or the chat keeps the keyboard in
            // the text field you were typing in, as on the Mac. Only another
            // field, or the inbox, takes it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // A rename or search ends when you click away, though.
                    if matches!(
                        this.field_focused(window, cx),
                        Some(
                            app::Field::Composer
                                | app::Field::Starter
                                | app::Field::Aside
                                | app::Field::Comment
                        )
                    ) {
                        window.prevent_default();
                    }
                }),
            )
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .text_color(t.ink)
            .ty(UI)
            .on_action(
                cx.listener(|this, _: &NewSession, window, cx| this.select(None, window, cx)),
            )
            .on_action(cx.listener(|this, _: &CloseSession, window, cx| {
                // After closing you're in the inbox, like mail: arrows move,
                // Enter opens, and ⌘Z undoes without a text field taking it.
                if let Some(id) = this.selected.clone()
                    && this.close(&id, window, cx)
                {
                    this.focus_inbox(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &UndoClose, window, cx| {
                if this
                    .toast
                    .as_ref()
                    .is_some_and(|t| matches!(t.action, ToastAction::Undo(_)))
                {
                    this.toast_action(window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &NextNeedsYou, window, cx| this.next_needs_you(window, cx)),
            )
            .on_action(cx.listener(|this, _: &TogglePanel, _, cx| {
                this.panel_open = !this.panel_open;
                this.end_pick();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowFirst, _, cx| {
                if let Some(a) = this.current().and_then(|s| s.arts.first().copied()) {
                    this.show_tab(Tab::Art(a), cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowChanges, _, cx| this.show_tab(Tab::Changes, cx)))
            .on_action(
                cx.listener(|this, _: &ShowTerminal, window, cx| this.open_shell(window, cx)),
            )
            // ⌘↩ steers from the reply box only; other boxes submit themselves.
            .on_action(cx.listener(|this, _: &SteerSend, window, cx| {
                if this.field_focused(window, cx) == Some(app::Field::Composer) {
                    this.send_with(true, window, cx)
                } else {
                    cx.propagate();
                }
            }))
            .on_action(
                cx.listener(|this, _: &SideChat, window, cx| this.open_side_chat(window, cx)),
            )
            .on_action(cx.listener(|this, _: &StopSession, window, cx| {
                if let Some(id) = this.selected.clone() {
                    this.stop(&id, window, cx);
                }
            }))
            // Esc reaches the app before a text field swallows it.
            .capture_key_down(cx.listener(|_, e: &KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                if k.key == "escape" && !k.modifiers.modified() {
                    window.dispatch_action(Box::new(Escape), cx);
                    cx.stop_propagation();
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.attach_paths(paths.paths(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                // Esc backs out one thing at a time; with nothing open it moves
                // to the inbox, where arrows move and Enter opens.
                if this.context.take().is_some() {
                } else if this.palette.is_some() {
                    this.close_palette(window, cx);
                } else if this.menu.take().is_some() {
                } else if gpui_kit::base::TextSelection::has_selection(window, cx) {
                    gpui_kit::base::TextSelection::clear(window, cx);
                } else if this.picking {
                    this.toggle_pick(cx);
                } else if this.renaming.is_some() || this.commenting.is_some() {
                    // Esc cancels a rename or comment; typing goes back to
                    // the reply box.
                    this.renaming = None;
                    this.commenting = None;
                    this.composer.update(cx, |c, cx| c.focus(window, cx));
                } else if this.field_focused(window, cx) == Some(app::Field::Composer)
                    && !this.composer.read(cx).value().trim().is_empty()
                {
                    // A draft keeps the keyboard: Esc doesn't throw you out of it.
                } else {
                    this.focus_inbox(window, cx);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Search, window, cx| this.open_palette(window, cx)))
            .on_action(|_: &app::Minimize, window, _| window.minimize_window())
            .on_action(|_: &app::Zoom, window, _| window.zoom_window())
            .on_action(cx.listener(|this, _: &SelectPrev, window, cx| {
                this.step_selection(-1, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| {
                this.step_selection(1, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &InboxUp, window, cx| {
                this.step_selection(-1, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &InboxDown, window, cx| {
                this.step_selection(1, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &InboxOpen, window, cx| {
                if this.selected.is_some() {
                    this.composer.update(cx, |c, cx| c.focus(window, cx))
                } else {
                    this.starter.update(cx, |c, cx| c.focus(window, cx))
                }
            }))
            .on_action(cx.listener(|this, _: &InboxClose, window, cx| {
                if let Some(id) = this.selected.clone()
                    && this.close(&id, window, cx)
                {
                    this.focus_inbox(window, cx);
                }
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                if e.pressed_button.is_none() {
                    let in_viewer = this.panel_left.is_some_and(|x| e.position.x.as_f32() >= x);
                    select::hover(in_viewer, false, window, cx);
                }
                if this.dragging_sidebar {
                    let w = window.viewport_size().width.as_f32();
                    // The chat keeps its minimum beside it.
                    let max = (w - viewer::CHAT_MIN).min(sidebar::MAX_WIDTH);
                    this.sidebar_w = e
                        .position
                        .x
                        .as_f32()
                        .clamp(sidebar::MIN_WIDTH, max.max(sidebar::MIN_WIDTH));
                    cx.notify();
                }
                if this.dragging {
                    let w = window.viewport_size().width.as_f32();
                    let side = if this.sidebar_hidden {
                        0.
                    } else {
                        this.sidebar_w
                    };
                    let max = (w - side - viewer::CHAT_MIN).max(viewer::MIN_WIDTH);
                    this.panel_width = (w - e.position.x.as_f32()).clamp(viewer::MIN_WIDTH, max);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.dragging || this.dragging_sidebar {
                        this.dragging = false;
                        this.dragging_sidebar = false;
                        cx.notify();
                    }
                }),
            )
            // A drag released outside the window still ends.
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.dragging || this.dragging_sidebar {
                        this.dragging = false;
                        this.dragging_sidebar = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .children(sidebar)
                    .child(main)
                    .children(panel),
            )
            .child(self.status_bar(cx))
            .children(toast)
            // Clicking anywhere outside an open menu closes it.
            .when(menu_open, |d| {
                d.child(
                    div()
                        .id("menu-scrim")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                cx.notify();
                            }),
                        ),
                )
            })
            .when(self.dragging || self.dragging_sidebar, |d| {
                d.cursor_col_resize()
            })
            .children(self.palette.map(|at| self.palette_view(at, window, cx)))
            .children(self.context_view(window, cx))
    }
}

/// The Book pairing, compiled in so the app needs nothing beside it (0032).
fn load_fonts(cx: &mut App) {
    let fonts: Vec<std::borrow::Cow<'static, [u8]>> = [
        &include_bytes!("../../../assets/fonts/CommitMono-400-Italic.ttf")[..],
        &include_bytes!("../../../assets/fonts/CommitMono-400-Regular.ttf")[..],
        &include_bytes!("../../../assets/fonts/CommitMono-700-Regular.ttf")[..],
        &include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf")[..],
        &include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf")[..],
        &include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf")[..],
        &include_bytes!("../../../assets/fonts/InstrumentSerif-Italic.ttf")[..],
        &include_bytes!("../../../assets/fonts/InstrumentSerif-Regular.ttf")[..],
        &include_bytes!("../../../assets/fonts/Literata-Italic.ttf")[..],
        &include_bytes!("../../../assets/fonts/Literata-Medium.ttf")[..],
        &include_bytes!("../../../assets/fonts/Literata-MediumItalic.ttf")[..],
        &include_bytes!("../../../assets/fonts/Literata-Regular.ttf")[..],
    ]
    .into_iter()
    .map(std::borrow::Cow::Borrowed)
    .collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("fonts: {e}");
    }
}

struct Opts {
    background: bool,
    walk: Option<String>,
    shots: Option<PathBuf>,
    /// Forced theme; `None` follows the system.
    dark: Option<bool>,
    size: (f32, f32),
}

fn opts() -> Opts {
    let mut o = Opts {
        background: false,
        walk: None,
        shots: None,
        dark: None,
        size: (1440., 860.),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dark" => o.dark = Some(true),
            "--light" => o.dark = Some(false),
            "--reduce-motion" => REDUCE_MOTION.store(true, Ordering::Relaxed),
            // Open behind whatever you're using, without taking focus.
            "--background" => o.background = true,
            "--walk" => o.walk = it.next(),
            "--shots" => o.shots = it.next().map(Into::into),
            "--size" => {
                if let Some((w, h)) = it.next().and_then(|v| {
                    let (w, h) = v.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                }) {
                    o.size = (w, h);
                }
            }
            other => eprintln!("ignoring unknown flag {other}"),
        }
    }
    o
}

fn main() {
    PROCESS_START.get_or_init(Instant::now);
    let o = opts();
    let _ = WINDOW.set((40., 40., o.size.0, o.size.1));

    let app = gpui_kit::application().with_assets(assets::Assets);
    // A background run starts as an accessory app on macOS: no Dock icon, and
    // macOS never brings it forward, so it leaves whatever you're doing alone.
    // Its windows draw as usual.
    #[cfg(target_os = "macos")]
    let app = if o.background || o.walk.is_some() {
        app.with_windowing(WindowingRequest::Headless)
    } else {
        app
    };
    // A background run keeps real timing: macOS would otherwise nap a hidden
    // app and slow its timers to a crawl.
    #[cfg(target_os = "macos")]
    if o.background || o.walk.is_some() {
        use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
        let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
            NSActivityOptions::UserInitiatedAllowingIdleSystemSleep
                | NSActivityOptions::LatencyCritical,
            &NSString::from_str("dent in the background"),
        );
        std::mem::forget(token);
    }
    app.run(move |cx| {
        gpui_kit::init(cx);
        notify::init();
        load_fonts(cx);
        math::load_fonts(cx);
        app::bind_keys(cx);
        app::set_menus(cx);
        let (x, y, w, h) = window_rect();
        let background = o.background || o.walk.is_some();
        BACKGROUND.store(background, Ordering::Relaxed);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(x), px(y)),
                size: size(px(w), px(h)),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some("dent".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.), px(16.))),
            }),
            // Narrower than the sidebar and the chat's minimum, and the chat
            // would break; the sidebar folds away before this.
            window_min_size: Some(size(px(720.), px(480.))),
            // Walkthroughs drive the window directly; they never need focus.
            focus: !background,
            ..Default::default()
        };
        let dark = o.dark;
        let walk = o.walk.clone().map(|n| walk::Walk::new(&n, o.shots.clone()));
        #[cfg(target_os = "linux")]
        if std::env::var_os("DENT_NO_GTK").is_none() {
            // WebKitGTK needs GTK running beside GPUI's own event loop.
            let _ = gtk::init();
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    while gtk::events_pending() {
                        gtk::main_iteration_do(false);
                    }
                }
            })
            .detach();
        }
        let (main_window, view) = gpui_kit::open_window(options, cx, move |window, cx| {
            let view = cx.new(|cx| {
                let mut d = Dent::new(dark, window, cx);
                d.walk = walk;
                d
            });
            let starter = view.read(cx).starter.clone();
            starter.update(cx, |c, cx| c.focus(window, cx));
            view
        })
        .expect("open window");
        // Look in eight times a second, but only draw when something on
        // screen changes: a working square's step, a timer's second, a
        // page's message.
        let weak = view.downgrade();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs_f32(1. / ds::motion::PIXEL_FPS))
                    .await;
                let Ok(due) = weak.update(cx, |this, _| this.tick_due()) else {
                    break;
                };
                if due && weak.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        // Pages in their own windows close without quitting dent.
        let main_id = main_window.window_id();
        cx.on_window_closed(move |cx, id| {
            if id == main_id {
                cx.quit();
            }
        })
        .detach();
        if !background {
            cx.activate(true);
        }
    });
}
