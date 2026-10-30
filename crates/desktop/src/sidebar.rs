//! The sidebar is the inbox: what needs you, then what's done and waiting to
//! be steered or closed, then working sessions, lighter, at the bottom.

use crate::ui::Tipped;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ui::Press;

use crate::ui::RevealOnHover;

use crate::anim;
use crate::app::Dent;
use crate::data::Session;
use crate::ds::{LABEL, NUMERAL, Typed, UI, motion, radius, size, space};
use crate::theme::{Status, status_square_at};
use crate::ui::{icon, icon_button, kbd, probe, section_label, tip};

pub const WIDTH: f32 = 288.;
pub const MIN_WIDTH: f32 = 220.;
pub const MAX_WIDTH: f32 = 440.;

impl Dent {
    pub fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.t;
        let [needs, done, working, paused] = self.shown_sections();
        // Hold these places while the pointer is here; let go when it leaves.
        self.inbox_hold = self.sidebar_hovered.then(|| {
            [&needs, &done, &working, &paused]
                .iter()
                .enumerate()
                .flat_map(|(sec, list)| {
                    list.iter()
                        .enumerate()
                        .map(move |(ix, s)| (s.id.clone(), (sec, ix)))
                })
                .collect()
        });
        let closed = self.closed();
        let pinned = self.pinned_sessions();
        // Caught up means nothing needs you and nothing's waiting to be
        // read, pinned or not.
        let clear = self.needs().is_empty() && self.done().is_empty();
        let new_selected = self.selected.is_none();

        // Only the rows on screen are laid out, however many sessions there are.
        let rows = self.inbox_rows(&pinned, [&needs, &done, &working, &paused], &closed, clear);
        self.sync_inbox(&rows);
        // A selection moved by the keyboard stays in view.
        if let Some(id) = self.reveal_row.take()
            && let Some(ix) = rows
                .iter()
                .position(|r| matches!(r, InboxRow::Session(s, _) if s.id == id))
        {
            self.inbox_list.scroll_to_reveal_item(ix);
        }
        let rows = std::rc::Rc::new(rows);
        let body = list(
            self.inbox_list.clone(),
            cx.processor(move |this, ix: usize, _window, cx| {
                // The list sizes each row to its width only through a wrapper.
                let row = match rows.get(ix) {
                    Some(r) => this.inbox_row(r, cx),
                    None => div().into_any_element(),
                };
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(row)
                    .into_any_element()
            }),
        )
        .flex_1()
        .min_h_0()
        .pb(px(space::S4));

        let t2 = self.t;
        div()
            .w(px(self.sidebar_w))
            .flex_none()
            .h_full()
            .relative()
            .child(probe(&self.probes, "sidebar"))
            // Its right edge resizes it, as the viewer's left edge does.
            .child(
                div()
                    .id("sidebar-resize")
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(px(6.))
                    .cursor_col_resize()
                    .hover(|d| d.bg(t2.hairline))
                    .when(self.dragging_sidebar, |d| d.bg(t2.hairline))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if e.click_count == 2 {
                                // Double-click puts it back to its usual width.
                                this.sidebar_w = WIDTH;
                            } else {
                                this.dragging_sidebar = true;
                            }
                            cx.notify();
                        }),
                    ),
            )
            .key_context("Inbox")
            .track_focus(&self.inbox_focus)
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.hairline)
            .child(
                // The traffic lights sit in this bar; the rest of it drags the window.
                div()
                    .h(px(size::HEADER))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(space::S1))
                    .pl(px(80.))
                    .pr(px(space::S2))
                    .on_mouse_down(MouseButton::Left, |e, window, _| {
                        if e.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .child(
                        icon_button("search", "search", &t)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_press(
                                cx.listener(|this, _, window, cx| this.open_palette(window, cx)),
                            )
                            .with_tip(tip("Search sessions", Some("⌘ K"), t)),
                    )
                    .child(
                        icon_button("new-session", "square-pen", &t)
                            .relative()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .when(new_selected, |d| d.bg(t.selected()))
                            .on_press(
                                cx.listener(|this, _, window, cx| this.select(None, window, cx)),
                            )
                            .with_tip(tip("New session", Some("⌘ N"), t))
                            .child(probe(&self.probes, "new-session")),
                    ),
            )
            .child(body)
    }

    /// The sidebar's rows, top to bottom.
    fn inbox_rows(
        &self,
        pinned: &[std::rc::Rc<Session>],
        [needs, done, working, paused]: [&[std::rc::Rc<Session>]; 4],
        closed: &[std::rc::Rc<Session>],
        clear: bool,
    ) -> Vec<InboxRow> {
        let mut rows = Vec::new();
        if clear {
            rows.push(InboxRow::Empty);
        }
        for (label, list, quiet) in [
            ("Pinned", pinned, false),
            ("Needs you", needs, false),
            ("Done", done, false),
            ("Working", working, true),
            // Its machine is away; each row says which and why.
            ("Paused", paused, true),
        ] {
            if list.is_empty() {
                continue;
            }
            rows.push(InboxRow::Label(label, list.len()));
            rows.extend(list.iter().map(|s| InboxRow::Session(s.clone(), quiet)));
        }
        // Closed sessions are archived, never gone: one row opens them.
        if !closed.is_empty() {
            rows.push(InboxRow::Closed(closed.len()));
            if self.show_closed {
                rows.extend(closed.iter().map(|s| InboxRow::Session(s.clone(), false)));
            }
        }
        rows
    }

    /// Keep the list's measurements in step: only rows that changed, or
    /// moved, are measured again.
    fn sync_inbox(&mut self, rows: &[InboxRow]) {
        let sigs: Vec<u64> = rows.iter().map(|r| r.sig(self)).collect();
        let old = &self.inbox_sigs;
        if old.is_empty() || sigs.is_empty() {
            self.inbox_list.reset(sigs.len());
        } else if *old != sigs {
            let first = old
                .iter()
                .zip(&sigs)
                .position(|(a, b)| a != b)
                .unwrap_or(old.len().min(sigs.len()));
            let same_tail = old[first..]
                .iter()
                .rev()
                .zip(sigs[first..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            self.inbox_list
                .splice(first..old.len() - same_tail, sigs.len() - same_tail - first);
        }
        self.inbox_sigs = sigs;
    }

    fn inbox_row(&mut self, r: &InboxRow, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        match r {
            InboxRow::Empty => self.inbox_empty(),
            InboxRow::Label(label, n) => section_label(label, Some(*n), &t)
                .h(px(size::ROW))
                .pb(px(space::S1))
                .px(px(space::S4))
                .mt(px(space::S3))
                .into_any_element(),
            InboxRow::Session(s, quiet) => self.row(s, *quiet, cx),
            InboxRow::Closed(n) => {
                let open = self.show_closed;
                div()
                    .id("closed-toggle")
                    .relative()
                    .mt(px(space::S3))
                    .h(px(size::ROW))
                    .px(px(space::S4))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover()))
                    .active(|s| s.bg(t.press()))
                    .on_press(cx.listener(|this, _, _, cx| {
                        this.show_closed = !this.show_closed;
                        cx.notify();
                    }))
                    .child(crate::ui::disclosure("fold", open, 12., t.faint))
                    .child(div().ty(LABEL).text_color(t.quiet).child("CLOSED"))
                    .child(div().ty(LABEL).text_color(t.faint).child(n.to_string()))
                    .child(probe(&self.probes, "closed-toggle"))
                    .into_any_element()
            }
        }
    }

    /// Nothing at all yet reads differently from nothing to do.
    fn inbox_empty(&self) -> AnyElement {
        let t = self.t;
        let fresh = self.sessions.is_empty();
        // Nothing yet may only mean nothing has arrived yet.
        let down = self.connection.as_ref().err().cloned();
        let starting = down.as_deref() == Some("Starting dent…");
        let (head, line): (&str, String) = if fresh && down.is_some() && !starting {
            ("Can't reach dent's service", down.unwrap_or_default())
        } else if fresh && (down.is_some() || !self.core.live) {
            ("Loading sessions…", String::new())
        } else if fresh && !self.recent.is_empty() {
            (
                "Nothing in dent yet",
                "Start a task, or pick up one from the terminal.".into(),
            )
        } else if fresh {
            ("No sessions yet", "Type a task to start one.".into())
        } else {
            ("All caught up", "Nothing needs you.".into())
        };
        div()
            .mx(px(space::S2))
            .mt(px(space::S2))
            .px(px(space::S2))
            .py(px(space::S3))
            .flex()
            .gap(px(space::S3))
            .child(div().pt(px(4.)).child(icon(
                if fresh {
                    "message-square"
                } else {
                    "check-check"
                },
                16.,
                t.quiet,
            )))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .ty(UI)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.ink)
                            .child(head),
                    )
                    .when(!line.is_empty(), |d| {
                        d.child(div().ty(UI).text_color(t.quiet).child(line))
                    }),
            )
            .into_any_element()
    }

    fn row(&mut self, s: &Session, quiet: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = self.t;
        let open = self.selected.as_deref() == Some(s.id.as_str());
        // Picked with Shift or ⌘, a row looks selected too.
        let selected = open || self.picked.contains(&s.id);
        let closable = Self::can_close(s);
        // A row that just finished pops its square once; a row that just
        // arrived rises into place.
        let pop = self
            .finished_at
            .get(&s.id)
            .map(|at| {
                if crate::reduce_motion() {
                    1.
                } else {
                    0.6 + 0.4 * anim::pop(at.elapsed().as_secs_f32())
                }
            })
            .unwrap_or(1.);
        let arrive = self
            .arrived_at
            .get(&s.id)
            .map(|at| anim::ease_out(anim::progress(*at, motion::BASE)))
            .unwrap_or(1.);
        let id = s.id.clone();
        let (close_id, reopen_id) = (s.id.clone(), s.id.clone());
        let mark: AnyElement = if s.closed {
            icon("archive", 12., t.faint).into_any_element()
        } else {
            // Retrying waits, as the working line says.
            let status = if matches!(s.activity, Some(dent_protocol::Activity::Retrying { .. })) {
                Status::Waiting
            } else {
                s.status
            };
            status_square_at(status, &t, pop).into_any_element()
        };
        let title_ink = if quiet { t.quiet } else { t.ink };
        // One line each: an error or a thought can run over several, and a
        // row stays its height.
        let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let detail = if s.closed {
            format!("Closed · {}", s.project)
        } else {
            format!("{} · {}", s.project, one_line(&s.detail))
        };
        // What it finished or asked since you last looked: a dot, until you
        // open it. The open row is being looked at.
        let unread = s.unread() && !open && !s.closed;
        // Words or files left in its reply box, unsent: the row says so in
        // place of what it's doing, so they aren't forgotten.
        let draft = (!open)
            .then(|| self.drafts.get(&s.id))
            .flatten()
            .filter(|d| !d.is_empty())
            .map(|d| {
                let words = d.text.split_whitespace().collect::<Vec<_>>().join(" ");
                if words.is_empty() {
                    let n = d.atts.len();
                    format!("{n} file{}", if n == 1 { "" } else { "s" })
                } else {
                    words
                }
            });
        // The close (or reopen) button shows on whichever row is under the
        // pointer right now: after a close, the next row slides in with its
        // own button rather than opening on the next click.
        let group = SharedString::from(format!("row-g-{}", s.id));
        let age = match self.working_since.get(&s.id) {
            Some(since) if s.status == Status::Working => {
                crate::chat::duration(since.elapsed().as_secs())
            }
            _ => crate::data::ago(s.updated),
        };
        let button: Option<AnyElement> = if closable {
            Some(
                div()
                    .id(SharedString::from(format!("close-{id}")))
                    .size(px(size::CONTROL_SM + 2.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .hover(|d| d.bg(t.hover()))
                    .active(|d| d.bg(t.press()))
                    .on_press(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close(&close_id, window, cx);
                    }))
                    .with_tip(tip("Close", None, t))
                    .child(icon("archive", 13., t.quiet))
                    .into_any_element(),
            )
        } else if s.closed {
            Some(
                div()
                    .id(SharedString::from(format!("reopen-{id}")))
                    .size(px(size::CONTROL_SM + 2.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .hover(|d| d.bg(t.hover()))
                    .active(|d| d.bg(t.press()))
                    .on_press(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.reopen(&reopen_id, cx);
                    }))
                    .with_tip(tip("Reopen", None, t))
                    .child(icon("archive-restore", 13., t.quiet))
                    .into_any_element(),
            )
        } else {
            None
        };
        let has_button = button.is_some();
        let trailing: AnyElement = div()
            .relative()
            .h(px(20.))
            .flex()
            .items_center()
            .justify_end()
            .gap(px(space::S1H))
            .when(unread, |d| {
                d.child(
                    div()
                        .id(SharedString::from(format!("unread-{id}")))
                        .size(px(UNREAD_DOT))
                        .rounded_full()
                        .bg(t.ink)
                        .when(has_button, |d| d.hide_on_hover(group.clone()))
                        .with_tip(tip("New since you looked", None, t)),
                )
            })
            .child(
                div()
                    .ty(NUMERAL)
                    .text_color(t.faint)
                    .when(has_button, |d| d.hide_on_hover(group.clone()))
                    .child(age),
            )
            .children(button.map(|b| {
                div()
                    .absolute()
                    .right(px(-4.))
                    .top(px(-2.))
                    .reveal_on_hover(group.clone())
                    .child(b)
            }))
            .into_any_element();
        div()
            .id(SharedString::from(format!("row-{}", s.id)))
            .group(group)
            .relative()
            .mx(px(space::S2))
            .px(px(space::S2))
            .py(px(space::S2))
            .rounded(px(radius::MD))
            // Keyboard focus in the inbox: a 2 px ink ring on the selected row.
            .when(open && self.inbox_focused, |d| {
                d.border_2()
                    .border_color(t.ink)
                    .px(px(space::S2 - 2.))
                    .py(px(space::S2 - 2.))
            })
            .flex()
            .gap(px(space::S1H + 2.))
            .cursor_pointer()
            .opacity(arrive * if quiet && !selected { 0.72 } else { 1. })
            .top(px((1. - arrive) * motion::RISE))
            .when(selected, |d| d.bg(t.selected()))
            .when(!selected, |d| {
                d.hover(|d| d.bg(t.hover())).active(|d| d.bg(t.press()))
            })
            .on_press(cx.listener(move |this, e: &ClickEvent, window, cx| {
                // Shift picks a run of rows, ⌘ one more; a plain click opens.
                let m = e.modifiers();
                if m.shift || m.platform {
                    this.pick(&id, m.shift, cx);
                } else {
                    this.picked.clear();
                    this.select(Some(id.clone()), window, cx)
                }
            }))
            .on_mouse_down(MouseButton::Right, {
                let (sid, cwd, closed) = (s.id.clone(), s.cwd.clone(), s.closed);
                let (can_fork, can_restart) = (s.caps.fork, s.caps.restart);
                let resume = s.resume_command();
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    use crate::context::Do;
                    // On one of several picked rows: what they can do together.
                    if this.picked.len() > 1 && this.picked.contains(&sid) {
                        let n = this.picked.len();
                        let all_pinned = this.picked.iter().all(|id| this.pinned.contains(id));
                        let items = vec![
                            (format!("Close {n} sessions"), Do::ClosePicked),
                            (
                                if all_pinned {
                                    format!("Unpin {n} sessions")
                                } else {
                                    format!("Pin {n} sessions to top")
                                },
                                Do::PinPicked,
                            ),
                        ];
                        this.open_context(e.position, items, cx);
                        return;
                    }
                    let mut items = vec![
                        ("Rename", Do::Rename(sid.clone())),
                        ("Regenerate Title", Do::Retitle(sid.clone())),
                    ];
                    // Only what its agent can do.
                    if can_fork {
                        items.push(("Fork", Do::Fork(sid.clone())));
                    }
                    if !closed && can_restart {
                        items.push(("Restart Agent", Do::Restart(sid.clone())));
                    }
                    if !closed {
                        let pinned = this.pinned.contains(&sid);
                        items.push((
                            if pinned { "Unpin" } else { "Pin to top" },
                            Do::Pin(sid.clone()),
                        ));
                    }
                    if closed {
                        items.push(("Reopen", Do::Reopen(sid.clone())));
                    } else if closable {
                        items.push(("Close", Do::CloseSession(sid.clone())));
                    }
                    if !cwd.is_empty() {
                        items.push(("", Do::Separator));
                        items.push(("Copy folder path", Do::CopyPath(cwd.clone())));
                        items.push(("Show folder in Finder", Do::Reveal(cwd.clone())));
                    }
                    // Carry it on in a terminal of your own.
                    if let Some(cmd) = &resume {
                        items.push(("Copy Resume Command", Do::Copy(cmd.clone())));
                    }
                    this.open_context(e.position, items, cx);
                })
            })
            .child(div().h(px(20.)).flex().items_center().child(mark))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .id(SharedString::from(format!("row-title-{}", s.id)))
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .ty(UI)
                                    .text_color(title_ink)
                                    .when(!quiet, |d| d.font_weight(FontWeight::MEDIUM))
                                    // A title too long for the row reads in full on hover.
                                    .when(s.title.chars().count() > 30, |d| {
                                        d.with_tip(crate::ui::tip_owned(s.title.clone(), t))
                                    })
                                    .child(one_line(&s.title)),
                            )
                            .child(div().h(px(20.)).flex().items_center().child(trailing)),
                    )
                    .child(match draft {
                        Some(words) => div().truncate().ty(UI).text_color(t.quiet).child(
                            StyledText::new(format!("Draft · {words}")).with_highlights([(
                                0..5,
                                HighlightStyle {
                                    color: Some(t.ink),
                                    font_weight: Some(FontWeight::MEDIUM),
                                    ..Default::default()
                                },
                            )]),
                        ),
                        // With more than one machine, which one it runs on.
                        None if self.has_machines() && !s.closed => {
                            let owner = dent_client_core::fleet::split(&s.id).0;
                            let shape = self
                                .shapes()
                                .get(owner)
                                .copied()
                                .unwrap_or(crate::glyph::Shape::Circle);
                            // Long names give way, ending in "…", rather than
                            // run past the row.
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::S1H))
                                .min_w_0()
                                .overflow_hidden()
                                .ty(UI)
                                .text_color(t.quiet)
                                .child(
                                    div()
                                        .min_w(px(24.))
                                        .flex_shrink(1.)
                                        .truncate()
                                        .child(format!("{} ·", s.project)),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .child(crate::glyph::glyph(shape, t.quiet, 9.)),
                                )
                                .child(
                                    div().flex_none().child(format!(
                                        "{} ·",
                                        crate::data::machine_short(&s.machine)
                                    )),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_shrink(4.)
                                        .truncate()
                                        .child(one_line(&s.detail)),
                                )
                        }
                        None => div().truncate().ty(UI).text_color(t.quiet).child(detail),
                    }),
            )
            .child(probe(&self.probes, format!("row-{}", s.id)))
            .into_any_element()
    }

    /// One agent's usage in the status bar: its mark, then each window
    /// used. Nothing when it hasn't said.
    fn usage_group(&self, agent: &str) -> Option<AnyElement> {
        let t = self.t;
        let limits = self.limits(agent);
        let window = |name: &str| {
            limits
                .iter()
                .find(|w| w.name == name)
                .map(|w| format!("{:.0}%", w.used * 100.))
        };
        let (five, week) = (window("five_hour"), window("seven_day"));
        if five.is_none() && week.is_none() {
            return None;
        }
        let both = five.is_some() && week.is_some();
        // When each window starts over, for the tooltip.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let resets = |name: &str| {
            limits
                .iter()
                .find(|w| w.name == name && w.resets_at > now)
                .map(|w| crate::data::span((w.resets_at - now) as u64))
        };
        let who = crate::bridge::agent_info(&self.agents, agent).short;
        let usage_tip = match (resets("five_hour"), resets("seven_day")) {
            (Some(a), Some(b)) => {
                format!("{who} usage. The 5-hour window resets in {a}, the week in {b}.")
            }
            (Some(a), None) => format!("{who} usage. The 5-hour window resets in {a}."),
            (None, Some(b)) => format!("{who} usage. The week resets in {b}."),
            (None, None) => format!("{who} usage across every way you use your account"),
        };
        let (mark, color) = crate::agents::mark(agent);
        // The default agent's is `usage`, as walkthroughs know it.
        let id = if self.agent_id(None) == agent {
            "usage".to_string()
        } else {
            format!("usage-{agent}")
        };
        Some(
            div()
                .id(SharedString::from(id))
                .flex()
                .items_center()
                .gap(px(space::S1H))
                .with_tip(crate::ui::tip_owned(usage_tip, t))
                .child(icon(mark, 12., color.unwrap_or(t.quiet)))
                .when_some(five, |d, v| {
                    d.child(div().text_color(t.ink).child(v))
                        .child("of 5-hour limit")
                })
                .when(both, |d| d.child(div().text_color(t.faint).child("·")))
                .when_some(week, |d, v| {
                    d.child(div().text_color(t.ink).child(v))
                        .child("of weekly limit")
                })
                .into_any_element(),
        )
    }

    pub fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.t;
        let needs = self.needs().len();
        // Each agent's usage, the default's first.
        let usage: Vec<AnyElement> = self
            .agent_ids()
            .into_iter()
            .filter_map(|agent| self.usage_group(&agent))
            .collect();
        let machine = crate::data::machine_shown(&self.machine);
        // With other machines: how many, and only the ones that need a look.
        let away: Vec<String> = self
            .added_machines()
            .filter_map(|m| {
                use crate::machines_page::Standing;
                let word = match Standing::of(m) {
                    Standing::Online | Standing::Connecting => return None,
                    Standing::Asleep => "asleep",
                    Standing::Updating => "updating",
                    Standing::NeedsCode => "needs a code",
                    _ => "can't reach",
                };
                // A long name shortened in its middle, so what's become of
                // it still shows.
                Some(format!("{} {word}", crate::data::machine_short(m.name())))
            })
            .collect();
        let (link, link_tip) = match &self.connection {
            Ok(()) if self.has_machines() => {
                let mut parts = away;
                parts.push(format!("{} machines", self.added_machines().count() + 1));
                (parts.join(" · "), "Settings › Machines".to_string())
            }
            Ok(()) => (
                format!("Sessions run on {machine}"),
                "They keep going when dent closes".to_string(),
            ),
            Err(why) => (why.clone(), "dent's service isn't answering".to_string()),
        };
        let down = self.connection.is_err();
        let (updates, updating) = (self.updates_ready(), self.updating.is_some());
        div()
            .h(px(size::STATUS))
            .flex_none()
            .px(px(space::S4))
            .flex()
            .items_center()
            .gap(px(space::S4))
            .border_t_1()
            .border_color(t.hairline)
            .ty(LABEL)
            .text_color(t.quiet)
            .children(usage)
            .child(div().flex_1())
            .child(
                div()
                    .id("machine")
                    .flex()
                    .items_center()
                    .gap(px(space::S1H))
                    // A long machine name gives way rather than push the
                    // bar past the window.
                    .min_w_0()
                    .flex_shrink(1.)
                    .when(down, |d| d.text_color(t.ink))
                    .with_tip(crate::ui::tip_owned(link_tip, t))
                    .child(div().min_w_0().truncate().child(link)),
            )
            // A newer dent ready somewhere: one press runs it everywhere.
            .when(updates > 0 || updating, |d| {
                d.child(
                    div()
                        .id("update-ready")
                        .relative()
                        .flex()
                        .flex_none()
                        .items_center()
                        .px(px(space::S1H))
                        .h(px(size::CONTROL_SM))
                        .rounded(px(radius::SM))
                        .text_color(t.ink)
                        .when(!updating, |d| {
                            d.cursor_pointer()
                                .hover(|d| d.bg(t.hover()))
                                .on_press(cx.listener(|this, _, _, cx| this.update_all(cx)))
                        })
                        .with_tip(crate::ui::tip_owned(
                            if updating {
                                "Sessions keep running".to_string()
                            } else {
                                "A newer dent is ready · sessions keep running".to_string()
                            },
                            t,
                        ))
                        .child(if updating { "Updating…" } else { "Update" })
                        .child(crate::ui::probe(&self.probes, "update-ready")),
                )
            })
            .when(needs > 0, |d| {
                d.child(
                    div()
                        .id("needs-count")
                        .flex()
                        .items_center()
                        .gap(px(space::S1H))
                        .px(px(space::S1H))
                        .h(px(size::CONTROL_SM))
                        .rounded(px(radius::SM))
                        .cursor_pointer()
                        .hover(|d| d.bg(t.hover()))
                        .on_press(
                            cx.listener(|this, _, window, cx| this.next_needs_you(window, cx)),
                        )
                        .child(status_square_at(Status::Needs, &t, 1.))
                        .child(if needs == 1 {
                            "1 needs you".to_string()
                        } else {
                            format!("{needs} need you")
                        })
                        .child(kbd("⌘ J", &t)),
                )
            })
    }
}

/// One row of the sidebar.
pub enum InboxRow {
    Empty,
    Label(&'static str, usize),
    Session(std::rc::Rc<Session>, bool),
    Closed(usize),
}

impl InboxRow {
    /// Anything that changes the row's height, or which row it is.
    fn sig(&self, d: &Dent) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match self {
            InboxRow::Empty => (0u8, d.sessions.is_empty(), d.recent.is_empty()).hash(&mut h),
            InboxRow::Label(l, n) => (1u8, l, n).hash(&mut h),
            InboxRow::Session(s, quiet) => {
                (2u8, &s.id, quiet, &s.title, &s.detail, s.status as u8).hash(&mut h)
            }
            InboxRow::Closed(n) => (3u8, n, d.show_closed).hash(&mut h),
        }
        h.finish()
    }
}

/// The unread mark beside a row's time.
const UNREAD_DOT: f32 = 7.;
