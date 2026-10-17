//! The dent desktop app: the session inbox, the chat and the viewer, drawn
//! with GPUI from what this machine's daemon reports.
//!
//! `cargo run -p dent-desktop -- [--dark] [--size WxH] [--reduce-motion] [--background]`
//! starts the daemon beside it when none is running.

mod ambient;
mod anim;
mod ansi;
mod app;
mod assets;
mod bridge;
mod cards;
mod chat;
mod complete;
mod compose;
mod context;
mod data;
mod diagram;
mod draft;
mod ds;
mod find;
mod highlight;
mod keys;
mod lightbox;
mod live;
mod lru;
mod markdown;
mod math;
mod mdstream;
mod notify;
mod open;
mod palette;
mod picker;
mod popout;
mod prefs;
mod select;
mod shell;
mod sidebar;
mod switcher;
mod theme;
mod ui;
mod viewer;
mod walk;
mod web;

use crate::ui::Arrive;
use crate::ui::FocusField;
use crate::ui::Tipped;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::base::input::{InputEditorStyle, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ui::Press;

use app::{
    AttachFiles, ChatBottom, ChatPageDown, ChatPageUp, ChatTop, CloseSession, Dent, Escape,
    InboxClose, InboxDown, InboxOpen, InboxUp, NewSession, NextNeedsYou, Search, SelectNext,
    SelectPrev, ShowChanges, ShowFirst, ShowTerminal, SideChat, SteerSend, StopSession, Tab,
    ToastAction, TogglePanel, ToggleSidebar, UndoClose,
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

/// `--reduce-motion` holds it on whatever the system says.
static REDUCE_MOTION_FORCED: AtomicBool = AtomicBool::new(false);

/// Follow the system's Reduce Motion setting, for dent's own motion and
/// gpui's alike. Read at launch and whenever the window comes forward,
/// which is when a change in System Settings could have happened.
pub fn sync_reduce_motion(cx: &mut App) {
    #[cfg(target_os = "macos")]
    if !REDUCE_MOTION_FORCED.load(Ordering::Relaxed) {
        let on =
            objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        set_reduce_motion(on);
    }
    cx.set_reduce_motion(reduce_motion());
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
        crate::ui::set_menu_open(self.context.is_some());
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
        // A link asked to show a file.
        if let Some((path, line)) = select::take_open() {
            self.open_file_at(&path, line, cx);
        }
        // A file you opened in the viewer follows its changes on disk.
        if let Some(Tab::Art(a)) = self.current().map(|s| self.tab(s))
            && self.panel_open
        {
            self.refresh_opened(a);
        }
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
        if self.animating() || toast_arriving || walking || ambient::wants_frames(self) {
            window.request_animation_frame();
        }
        // The composer says what Enter will do in this session's state.
        let open = self.current().and_then(|s| {
            s.items.iter().rev().find_map(|i| match i {
                data::Item::Question { questions, .. } if questions.iter().any(|q| q.fixed) => {
                    Some("Pick one above")
                }
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
        // The side chat's box, a comment or your terminal put away with the
        // viewer (⌘\, another tab) gives the keyboard back to the prompt,
        // so typing never goes nowhere.
        let shown = self
            .current()
            .map(|s| self.tab(s))
            .filter(|_| self.panel_open);
        let field = self.field_focused(window, cx);
        if field == Some(app::Field::Aside) && shown != Some(Tab::Aside)
            || field == Some(app::Field::Comment) && shown != Some(Tab::Changes)
            || self.term_focus.is_focused(window) && shown != Some(Tab::Terminal)
        {
            let prompt = self.prompt().clone();
            prompt.update(cx, |c, cx| c.focus_field(window, cx));
        }
        self.inbox_focused = self.inbox_focus.is_focused(window);
        self.ask_keyed = self.ask_focus.is_focused(window);
        self.note_ask_shown();
        self.composer_keyed = self.composer.read(cx).focus_handle(cx).is_focused(window);
        self.take_resend(cx);
        self.take_restores(window, cx);
        self.prefs_tick(cx);
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
        self.sidebar_hidden = (self.sidebar_off && !self.sidebar_forced)
            || wants_panel && room < viewer::MIN_WIDTH && !self.sidebar_forced;
        // Folding away takes keyboard focus with it, to the window. So does
        // your terminal going out of sight (the viewer showing a file
        // instead): focus left on what isn't drawn reaches no shortcut.
        let shell_gone = self.term_focus.is_focused(window)
            && !(self.panel_open && self.current().is_some_and(|s| self.tab(s) == Tab::Terminal));
        if self.sidebar_hidden && self.inbox_focus.is_focused(window)
            || shell_gone
            || window.focused(cx).is_none()
        {
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
        self.sidebar_hovered = self.over_sidebar(window.mouse_position(), window);
        self.take_lightbox_request(window);
        // A card had the keyboard and its ask went: back to the reply box,
        // so typing carries on.
        if self.ask_focus.is_focused(window)
            && !self
                .selected
                .clone()
                .is_some_and(|id| self.open_ask(&id).is_some())
        {
            let composer = self.composer.clone();
            composer.update(cx, |c, cx| c.focus_field(window, cx));
        }
        // A menu's field gives the keyboard back to the prompt however the
        // menu closed: a choice, Esc, a click away.
        if self.menu.is_none() && self.menu_query.read(cx).focus_handle(cx).is_focused(window) {
            let prompt = self.prompt().clone();
            prompt.update(cx, |f, cx| f.focus_field(window, cx));
        }
        let sidebar = (!self.sidebar_hidden).then(|| self.sidebar(cx));
        let main = self.main_column(window, cx);
        let panel = self
            .current_rc()
            .filter(|s| self.panel_open && self.shows_panel(s))
            .map(|s| self.panel(&s, window, cx));
        let toast =
            self.toast.clone().map(|toast| {
                let label = match toast.action {
                    ToastAction::Undo(_)
                    | ToastAction::UnhideProject(_)
                    | ToastAction::Unremove { .. } => "Undo",
                    ToastAction::Resume(_) => "Resume",
                    ToastAction::Info => "OK",
                };
                div()
                    .absolute()
                    .w(px(self.sidebar_w - space::S4))
                    // Over the sidebar; with it folded, under the header,
                    // clear of the reply box and its send button.
                    .map(|d| {
                        if self.sidebar_hidden {
                            d.left(px(space::S4)).top(px(size::HEADER + space::S2))
                        } else {
                            d.left(px(space::S2)).bottom(px(size::STATUS + space::S2))
                        }
                    })
                    .child(
                        ui::floating(&t)
                            .id("toast")
                            // Over the sidebar: a press here is the toast's alone.
                            .occlude()
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
                                    ToastAction::UnhideProject(_) => "folder",
                                    ToastAction::Unremove { .. } => "x",
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
                                        d.with_tip(ui::tip_owned(toast.text.clone(), t))
                                    })
                                    .child(toast.text.clone()),
                            )
                            .when(toast.action.undoes(), |d| d.child(ui::kbd("⌘ Z", &t)))
                            .child(
                                ui::button("toast-action", label, false, &t)
                                    .relative()
                                    .border_0()
                                    .on_press(cx.listener(|this, _, window, cx| {
                                        this.toast_action(window, cx)
                                    }))
                                    .child(ui::probe(&self.probes, "toast-action")),
                            )
                            .arrive(
                                ElementId::Name(format!("toast-{:?}", toast.at).into()),
                                ui::From::Below,
                                ds::motion::BASE,
                            ),
                    )
            });
        let menu_open = self.menu.is_some();
        div()
            .id("root")
            .key_context("Dent")
            // A click anywhere in dent takes the keyboard back from the page.
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, _| {
                if let Some(p) = &this.preview {
                    p.borrow().release_focus();
                }
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
                if this.toast.as_ref().is_some_and(|t| t.action.undoes()) {
                    this.toast_action(window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &NextNeedsYou, window, cx| this.next_needs_you(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(
                cx.listener(|this, _: &ChatPageUp, _, cx| {
                    this.scroll_chat(app::ChatScroll::Up, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ChatPageDown, _, cx| {
                this.scroll_chat(app::ChatScroll::Down, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ChatTop, _, cx| this.scroll_chat(app::ChatScroll::Top, cx)),
            )
            .on_action(cx.listener(|this, _: &ChatBottom, _, cx| {
                this.scroll_chat(app::ChatScroll::Bottom, cx)
            }))
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
            .on_action(
                cx.listener(|this, _: &AttachFiles, window, cx| this.pick_attachments(window, cx)),
            )
            .on_action(cx.listener(|this, _: &StopSession, window, cx| {
                if let Some(id) = this.selected.clone() {
                    this.stop(&id, "⌘.", window, cx);
                }
            }))
            // A key let go: its next press is a fresh one (see `intercept`).
            // Esc reaches the app before a text field swallows it.
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                // A held shortcut acts once: holding ⌘W doesn't close every
                // session. Moving and deleting still repeat, and in a text
                // field so do undo, paste and the Control keys.
                let field = this.field_focused(window, cx).is_some();
                if e.is_held
                    && (k.modifiers.platform || k.modifiers.control)
                    && !matches!(
                        k.key.as_str(),
                        "left" | "right" | "up" | "down" | "backspace" | "delete"
                    )
                    && !(field
                        && (matches!(k.key.as_str(), "z" | "v")
                            || !k.modifiers.platform && k.key.len() == 1))
                {
                    cx.stop_propagation();
                    return;
                }
                // A waiting card's keys, never from a held key: gpui's own
                // flag is the one test for that, and it's known only here.
                if this.card_key(k, e.is_held, window, cx) {
                    cx.stop_propagation();
                    return;
                }
                // Esc acts once too: one held to stop a turn would otherwise
                // run on past it and throw the keyboard out of the reply box.
                if k.key == "escape" && !k.modifiers.modified() {
                    if !e.is_held {
                        window.dispatch_action(Box::new(Escape), cx);
                    }
                    cx.stop_propagation();
                }
            }))
            // Files dragged in from Finder: followed wherever they are, and
            // let go of (below) by where the pointer is, whatever was last
            // typed, since a drag from another app doesn't count as the
            // pointer moving.
            .on_drag_move(
                cx.listener(|this, e: &DragMoveEvent<ExternalPaths>, _, cx| {
                    this.dragging_files = Some(e.drag(cx).paths().to_vec());
                    if this.drag_at != Some(e.event.position) {
                        this.drag_at = Some(e.event.position);
                        cx.notify();
                    }
                }),
            )
            .child({
                let me = cx.entity().downgrade();
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let me = me.clone();
                        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture && cx.has_active_drag() {
                                let _ = me
                                    .update(cx, |this, cx| this.drop_files(e.position, window, cx));
                            }
                        });
                    },
                )
                .absolute()
                .size_0()
            })
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                // Esc backs out one thing at a time; with nothing open it moves
                // to the inbox, where arrows move and Enter opens.
                if this.switch_cancel(cx)
                    || std::mem::take(&mut this.keys_open)
                    || this.close_lightbox(cx)
                    || this.context.take().is_some()
                {
                } else if this.palette.is_some() {
                    this.close_palette(window, cx);
                } else if this.close_find(window, cx)
                    || this.project_back(window, cx)
                    || this.menu.take().is_some()
                    || std::mem::take(&mut this.meter_open)
                    || this.dismiss_completion(cx)
                    || select::clear(window)
                {
                } else if this.picking {
                    this.toggle_pick(cx);
                } else if this.renaming.is_some() || this.commenting.is_some() {
                    // Esc cancels a rename or comment; typing goes back to
                    // the reply box.
                    this.renaming = None;
                    this.commenting = None;
                    this.composer.update(cx, |c, cx| c.focus_field(window, cx));
                } else if this.field_focused(window, cx) == Some(app::Field::Composer)
                    && let Some(id) = this
                        .selected
                        .clone()
                        .filter(|id| this.stoppable(id) && this.open_ask(id).is_none())
                {
                    // Esc in the reply box stops a running turn, like ⌘.; a
                    // draft stays where it is. A waiting card still takes Esc.
                    this.stop(&id, "Esc", window, cx);
                } else if this
                    .field_focused(window, cx)
                    .is_some_and(|f| matches!(f, app::Field::Composer | app::Field::Starter))
                    && !this.prompt().read(cx).value().trim().is_empty()
                {
                    // A draft keeps the keyboard: Esc doesn't throw you out of
                    // it, in a session or a new one.
                } else if this.field_focused(window, cx) == Some(app::Field::Composer)
                    && this
                        .selected
                        .clone()
                        .is_some_and(|id| this.open_ask(&id).is_some())
                {
                    // With a question, approval or plan waiting, Esc from the
                    // empty box goes to its card, where keys answer it.
                    window.focus(&this.ask_focus, cx);
                } else {
                    this.focus_inbox(window, cx);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Search, window, cx| this.open_palette(window, cx)))
            .on_action(
                cx.listener(|this, _: &app::FindInSession, window, cx| this.open_find(window, cx)),
            )
            .on_action(cx.listener(|this, _: &app::TextBigger, _, cx| this.step_zoom(1, cx)))
            .on_action(cx.listener(|this, _: &app::TextSmaller, _, cx| this.step_zoom(-1, cx)))
            .on_action(cx.listener(|this, _: &app::TextActual, _, cx| this.step_zoom(0, cx)))
            .on_action(cx.listener(|this, _: &app::LookSystem, window, cx| {
                this.set_appearance(prefs::Appearance::System, window, cx)
            }))
            .on_action(cx.listener(|this, _: &app::LookLight, window, cx| {
                this.set_appearance(prefs::Appearance::Light, window, cx)
            }))
            .on_action(cx.listener(|this, _: &app::LookDark, window, cx| {
                this.set_appearance(prefs::Appearance::Dark, window, cx)
            }))
            .on_action(cx.listener(|this, _: &app::ShowKeys, _, cx| this.toggle_keys(cx)))
            .on_action(cx.listener(|this, _: &app::SwitchNext, _, cx| this.switch_step(false, cx)))
            .on_action(cx.listener(|this, _: &app::SwitchPrev, _, cx| this.switch_step(true, cx)))
            // Letting go of Ctrl opens what Ctrl+Tab lit.
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, window, cx| {
                if this.switcher.is_some() && !e.modifiers.control {
                    this.switch_finish(window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &app::FindNext, window, cx| {
                    this.step_find(false, window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &app::FindPrev, window, cx| this.step_find(true, window, cx)),
            )
            .on_action(|_: &app::Minimize, window, _| window.minimize_window())
            .on_action(|_: &app::FullScreen, window, _| window.toggle_fullscreen())
            .on_action(cx.listener(|this, a: &ambient::SetAmbient, _, cx| {
                ambient::choose(a.id.as_deref());
                this.prefs_changed();
                app::refresh_menus(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &app::NotifySound, _, cx| {
                use std::sync::atomic::Ordering::Relaxed;
                let on = notify::SOUND.load(Relaxed);
                notify::SOUND.store(!on, Relaxed);
                this.prefs_changed();
                app::refresh_menus(cx);
            }))
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
                    this.composer.update(cx, |c, cx| c.focus_field(window, cx))
                } else {
                    this.starter.update(cx, |c, cx| c.focus_field(window, cx))
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
                // Into or out of the sidebar: its rows hold their places, or
                // let go.
                if this.over_sidebar(e.position, window) != this.sidebar_hovered {
                    cx.notify();
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
            // First, so each frame starts its text afresh and its press runs
            // after every control's.
            .child(select::layer())
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
                        // Takes the press that dismisses it: nothing under it is clicked.
                        .occlude()
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
            .children(self.switcher_view(cx))
            .children(self.keys_view(cx))
            .children(self.context_view(window, cx))
            // Over everything, menus painted late included.
            .children(
                self.lightbox_view(window, cx)
                    .map(|l| deferred(l).with_priority(2)),
            )
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
            "--reduce-motion" => {
                REDUCE_MOTION.store(true, Ordering::Relaxed);
                REDUCE_MOTION_FORCED.store(true, Ordering::Relaxed);
            }
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
        sync_reduce_motion(cx);
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
            // A demo recording animates at full rate though it never has
            // focus (GPUI otherwise holds such a window to about 30 fps).
            inactive_frame_interval: if std::env::var_os("DENT_AMBIENT_DEMO").is_some() {
                None
            } else {
                WindowOptions::default().inactive_frame_interval
            },
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
                cx.observe_window_activation(window, |_, window, cx| {
                    if window.is_window_active() {
                        sync_reduce_motion(cx);
                    }
                })
                .detach();
                let mut d = Dent::new(dark, window, cx);
                d.walk = walk;
                d
            });
            let starter = view.read(cx).starter.clone();
            starter.update(cx, |c, cx| c.focus_field(window, cx));
            view
        })
        .expect("open window");
        // Look in eight times a second, but only draw when something on
        // screen changes: a timer's second, a page's message. (A working
        // square asks for every frame itself, through `animating`.)
        let weak = view.downgrade();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs_f32(1. / ds::motion::LOOK_FPS))
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
