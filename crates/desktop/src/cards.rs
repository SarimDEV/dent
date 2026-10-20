//! What the agent reports besides replies, drawn by kind rather than by
//! command ([decision 0046]): a command's answer, a meter, limits, the
//! line where the conversation was compacted, and what the working line
//! says while the agent compacts or retries.
//!
//! [decision 0046]: ../../../docs/decisions/0046-commands-by-type.md

use gpui_kit::prelude::*;
use gpui_kit::*;

use dent_protocol::{Act, Activity, Bar, CommandOutcome, Field, Mark, Part, Row, Section, View};

use crate::app::Dent;
use crate::data::Session;
use crate::ds::{CODE, LABEL, Typed, UI, motion, radius, space};
use crate::markdown;
use crate::theme::Theme;
use crate::ui::{self, Arrive, From, Press, Tipped, ghost, icon, probe, tip_owned};

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A token count the way people say it: 940, 18.5k, 152k, 1M.
pub fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..10_000 => trim(format!("{:.1}k", n as f64 / 1e3)),
        10_000..1_000_000 => format!("{}k", (n as f64 / 1e3).round() as u64),
        _ => trim(format!("{:.1}M", n as f64 / 1e6)),
    }
}

/// `18.0k` reads as `18k`.
fn trim(s: String) -> String {
    s.replace(".0k", "k").replace(".0M", "M")
}

/// What the working line says for an activity: the lead, and what follows
/// the dot. `None` when the work goes on as usual and only the detail says
/// what it is.
pub fn activity_line(a: &Activity, now_ms: u64) -> Option<(String, String)> {
    Some(match a {
        Activity::Preparing { .. } | Activity::Thinking { .. } => return None,
        Activity::SettingUp { since, what } => {
            let secs = now_ms.saturating_sub(*since) / 1000;
            (
                if secs >= 1 {
                    format!("Setting up for {}", crate::chat::duration(secs))
                } else {
                    "Setting up".into()
                },
                format!("{what} in the new worktree"),
            )
        }
        Activity::Compacting { since } => {
            let secs = now_ms.saturating_sub(*since) / 1000;
            (
                if secs >= 1 {
                    format!("Compacting for {}", crate::chat::duration(secs))
                } else {
                    "Compacting".into()
                },
                "Summarizing the conversation so far".into(),
            )
        }
        Activity::Retrying {
            attempt,
            max,
            at,
            error,
        } => {
            let left = at.saturating_sub(now_ms).div_ceil(1000);
            let lead = if left == 0 {
                "Retrying now".to_string()
            } else {
                format!("Retrying in {}", crate::chat::duration(left))
            };
            let count = if *max > 0 {
                format!("attempt {attempt} of {max}")
            } else {
                format!("attempt {attempt}")
            };
            (lead, format!("{error} · {count}"))
        }
    })
}

/// A command as it was run: a glyph, then what was typed in the code
/// face. A sent command looks like this until its answer comes, so the
/// answer lands under it rather than replacing a message.
pub fn command_line(typed: &str, glyph: &str, color: Hsla, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(space::S2))
        .child(
            div()
                .w(px(space::S4))
                .flex_none()
                .flex()
                .justify_center()
                .child(icon(glyph, 13., color)),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .ty(CODE)
                .text_color(t.quiet)
                .child(typed.to_string()),
        )
}

/// The parts of a meter, darkest first, in the ink: a share of the whole
/// reads by its weight, not by a colour to look up.
fn tone(i: usize, t: &Theme) -> Hsla {
    const STEPS: [f32; 6] = [0.86, 0.62, 0.42, 0.28, 0.2, 0.14];
    t.ink.opacity(STEPS[i.min(STEPS.len() - 1)])
}

/// A meter: the bar in parts, with where it compacts on its own, then what
/// each part is. Parts loaded only when needed don't count against it and
/// are listed apart.
pub fn meter(used: u64, max: u64, compacts_at: Option<u64>, parts: &[Part], t: &Theme) -> Div {
    let max = max.max(1);
    let counted: Vec<&Part> = parts.iter().filter(|p| !p.deferred).collect();
    let later: Vec<&Part> = parts.iter().filter(|p| p.deferred).collect();
    let pct = used as f64 / max as f64 * 100.;
    let share = |n: u64| (n as f32 / max as f32).clamp(0., 1.);
    let bar = div()
        .relative()
        .h(px(6.))
        .w_full()
        .rounded(px(3.))
        .overflow_hidden()
        .bg(t.hairline.opacity(0.7))
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                // A hair between parts, so each reads as its own.
                .gap(px(1.))
                .children(if counted.is_empty() {
                    vec![div().h_full().w(relative(share(used))).bg(tone(0, t))]
                } else {
                    counted
                        .iter()
                        .enumerate()
                        .map(|(i, p)| div().h_full().w(relative(share(p.tokens))).bg(tone(i, t)))
                        .collect()
                }),
        )
        .children(compacts_at.filter(|c| *c < max).map(|c| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(share(c)))
                .w(px(1.))
                .bg(t.ink.opacity(0.55))
        }));
    let row = |swatch: Option<Hsla>, name: String, n: u64| {
        div()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .ty(UI)
            .child(
                div()
                    .size(px(8.))
                    .flex_none()
                    .rounded(px(2.))
                    .when_some(swatch, |d, c| d.bg(c))
                    .when(swatch.is_none(), |d| d.border_1().border_color(t.faint)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t.quiet)
                    .child(name),
            )
            .child(div().flex_none().text_color(t.ink).child(tokens(n)))
    };
    div()
        .flex()
        .flex_col()
        .gap(px(space::S2))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(space::S2))
                .child(div().ty(UI).text_color(t.ink).child(format!(
                    "{} of {} tokens",
                    tokens(used),
                    tokens(max)
                )))
                .child(div().ty(LABEL).text_color(t.quiet).child(if pct < 10. {
                    format!("{pct:.1}%")
                } else {
                    format!("{pct:.0}%")
                })),
        )
        .child(bar)
        .children(compacts_at.map(|c| {
            div().ty(LABEL).text_color(t.quiet).child(if c > used {
                format!(
                    "Compacts itself at {} · {} to go",
                    tokens(c),
                    tokens(c - used)
                )
            } else {
                format!("Compacts itself at {}", tokens(c))
            })
        }))
        .when(!counted.is_empty(), |d| {
            d.child(
                div()
                    .pt(px(space::S1))
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .children(
                        counted
                            .iter()
                            .enumerate()
                            .map(|(i, p)| row(Some(tone(i, t)), p.name.clone(), p.tokens)),
                    ),
            )
        })
        .when(!later.is_empty(), |d| {
            d.child(
                div()
                    .pt(px(space::S1))
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(
                        div()
                            .ty(LABEL)
                            .text_color(t.faint)
                            .child("Loaded only when needed"),
                    )
                    .children(later.iter().map(|p| row(None, p.name.clone(), p.tokens))),
            )
        })
}

/// Limits, each a bar with how much is used and when it resets.
pub fn limits(bars: &[Bar], t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(space::S3))
        .children(bars.iter().map(|b| {
            let used = b.used.clamp(0., 1.) as f32;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            let resets = (b.resets_at > now).then(|| {
                format!(
                    "Resets in {}",
                    crate::data::span((b.resets_at - now) as u64)
                )
            });
            div()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(space::S2))
                        .ty(UI)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(t.ink)
                                .child(b.label.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(if used >= 0.9 { t.needs.0 } else { t.quiet })
                                .child(format!("{:.0}% used", used * 100.)),
                        ),
                )
                .child(
                    div()
                        .h(px(6.))
                        .w_full()
                        .rounded(px(3.))
                        .overflow_hidden()
                        .bg(t.hairline.opacity(0.7))
                        .child(div().h_full().w(relative(used)).bg(if used >= 0.9 {
                            t.needs.0
                        } else {
                            tone(0, t)
                        })),
                )
                .children(resets.map(|r| div().ty(LABEL).text_color(t.faint).child(r)))
        }))
}

impl Dent {
    /// The context gauge beside the composer's chips; a click opens what
    /// fills it, with a way to compact now.
    pub fn context_meter(
        &self,
        s: &Session,
        used: u64,
        max: Option<u64>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let max = max.unwrap_or(200_000).max(1);
        let pct = used as f32 / max as f32 * 100.;
        let near = s.compacts_at.is_some_and(|c| used as f64 >= c as f64 * 0.9) || pct > 90.;
        let open = self.meter_open;
        // Working, or waiting on an answer; a pull request needing you
        // leaves the agent free.
        let busy = s.status == crate::theme::Status::Working
            || (s.status == crate::theme::Status::Needs
                && !s.pr.as_ref().is_some_and(|p| {
                    p.open() && crate::gitbar::pr_tone(p) == crate::gitbar::Tone::Trouble
                }));
        let compacting = matches!(s.activity, Some(Activity::Compacting { .. }));
        let sid = s.id.clone();
        let gauge = div()
            .id("context-gauge")
            .relative()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .flex_shrink(2.)
            .ty(LABEL)
            .text_color(if near { t.needs.0 } else { t.faint })
            .cursor_pointer()
            .hover(|d| d.text_color(t.ink))
            .on_press(cx.listener(|this, _, _, cx| {
                this.meter_open = !this.meter_open;
                this.menu = None;
                cx.notify();
            }))
            .child(if pct < 10. {
                format!("context {pct:.1}%")
            } else {
                format!("context {pct:.0}%")
            })
            .child(probe(&self.probes, "context-gauge"));
        let popover = open.then(|| {
            let who = &s.agent_short;
            let (label, off): (&str, Option<String>) = if compacting {
                ("Compacting…", Some(format!("{who} is compacting now")))
            } else if busy {
                (
                    "Compact now",
                    Some(format!("Wait for {who} to finish this turn")),
                )
            } else {
                ("Compact now", None)
            };
            let action = crate::ui::button("compact-now", label, false, &t)
                .relative()
                .map(|d| match off {
                    Some(why) => d
                        .opacity(0.5)
                        .cursor_not_allowed()
                        .with_tip(tip_owned(why, t)),
                    None => d
                        .with_tip(tip_owned(
                            "Summarize the conversation so far to free up room",
                            t,
                        ))
                        .on_press(cx.listener(move |this, _, window, cx| {
                            this.meter_open = false;
                            this.run_command(&sid, "/compact", window, cx);
                        })),
                })
                .child(probe(&self.probes, "compact-now"));
            let body = panel(&t)
                .id("context-panel")
                .w(px(300.))
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.meter_open = false;
                    cx.notify();
                }))
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .child(
                    div()
                        .ty(UI)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.ink)
                        .child("Context"),
                )
                .child(meter(used, max, s.compacts_at, &s.context_parts, &t))
                .child(div().pt(px(space::S1)).flex().justify_end().child(action));
            div().absolute().top_0().right_0().child(
                deferred(
                    anchored()
                        .anchor(Anchor::BottomRight)
                        .snap_to_window_with_margin(px(8.))
                        .child(div().mb(px(6.)).child(body.arrive(
                            "context-panel-in",
                            From::Below,
                            motion::FAST,
                        ))),
                )
                .with_priority(1),
            )
        });
        // Nearly full between turns: compacting first keeps the next
        // message from being the one that runs out of room.
        let first = (near && !busy && !compacting).then(|| {
            let sid = s.id.clone();
            crate::ui::ghost("compact-first", "Compact", &t)
                .relative()
                .text_color(t.needs.0)
                .with_tip(tip_owned(
                    "The context is nearly full: summarize the conversation before the next message",
                    t,
                ))
                .on_press(cx.listener(move |this, _, window, cx| {
                    this.run_command(&sid, "/compact", window, cx);
                }))
                .child(probe(&self.probes, "compact-first"))
        });
        div()
            .relative()
            .min_w_0()
            .mr(px(space::S1))
            .flex_shrink(2.)
            .flex()
            .items_center()
            .gap(px(space::S1))
            .children(first)
            .child(gauge)
            .children(popover)
            .into_any_element()
    }

    /// Where the conversation was compacted: a divider, with what it
    /// freed.
    pub fn compacted_line(
        &self,
        ix: usize,
        who: &str,
        auto: bool,
        before: u64,
        after: Option<u64>,
    ) -> AnyElement {
        let t = self.t;
        let what = if auto {
            "Compacted automatically"
        } else {
            "Compacted"
        };
        let label = match after {
            Some(a) if before > 0 => format!("{what} · {} → {} tokens", tokens(before), tokens(a)),
            _ if before > 0 => format!("{what} · from {} tokens", tokens(before)),
            _ => what.to_string(),
        };
        let rule = || div().flex_1().h(px(1.)).bg(t.hairline);
        div()
            .id(SharedString::from(format!("compacted-{ix}")))
            .relative()
            .py(px(space::S1))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .child(rule())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S1H))
                    .ty(LABEL)
                    .text_color(t.quiet)
                    .child(icon("fold-vertical", 12., t.quiet))
                    .child(label),
            )
            .child(rule())
            .with_tip(tip_owned(
                format!(
                    "{who} summarized the conversation so far to make room. \
                     Everything stays here to read; {who} works from the summary."
                ),
                t,
            ))
            .child(probe(&self.probes, format!("compacted-{ix}")))
            .into_any_element()
    }

    /// A command the agent ran itself: what was run, then its answer, in
    /// the quiet voice of something beside the conversation. An answer
    /// with a better shape (a meter, limits) shows that, its words a fold
    /// away; a long one folds after its first lines.
    #[allow(clippy::too_many_arguments)]
    pub fn command_card(
        &mut self,
        s: &Session,
        ix: usize,
        name: &str,
        args: &str,
        text: &str,
        outcome: &CommandOutcome,
        view: Option<&View>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let typed = format!("/{name} {args}").trim_end().to_string();
        let (glyph, glyph_color) = match outcome {
            CommandOutcome::Failed => ("circle-alert", t.needs.0),
            CommandOutcome::TerminalOnly | CommandOutcome::Unknown { .. } => ("ban", t.faint),
            CommandOutcome::Done => ("terminal", t.quiet),
        };
        let head = command_line(&typed, glyph, glyph_color, &t);
        // Every answer sits under the same rule, in the interface face:
        // something beside the conversation, not Claude talking.
        let rule = |inner: Div| {
            div()
                .flex()
                .flex_col()
                .gap(px(space::S2))
                .child(head)
                .child(
                    inner
                        .ml(px(space::S2 - 1.))
                        .pl(px(space::S3 + space::S1))
                        .border_l_2()
                        .border_color(t.hairline)
                        .ty(UI)
                        .text_color(t.quiet),
                )
                .into_any_element()
        };
        // Commands that can't run here say so in dent's words, and offer
        // what might have been meant.
        match outcome {
            CommandOutcome::TerminalOnly => {
                return rule(
                    div().child(format!("Works only in {} in the terminal.", s.agent_name)),
                );
            }
            CommandOutcome::Unknown { suggestion } => {
                let sid = s.id.clone();
                return rule(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S1))
                        .child(format!("{} has no command by that name.", s.agent_short))
                        .children(suggestion.clone().map(|sug| {
                            let label = format!("Use /{sug}");
                            ghost(SharedString::from(format!("suggest-{ix}")), label, &t)
                                .text_color(t.ink)
                                .relative()
                                .on_press(cx.listener(move |this, _, window, cx| {
                                    this.suggest_command(&sid, &sug, window, cx)
                                }))
                                .child(probe(&self.probes, format!("suggest-{ix}")))
                        })),
                );
            }
            _ => {}
        }
        let key = crate::data::Fold::Command(name, text).key();
        let open = self.is_open(&crate::data::Fold::Command(name, text));
        let long = text.lines().count() > 6 || text.len() > 600;
        // The view says it; the words are there for the detail. A sheet
        // says all of it.
        let sheet = matches!(view, Some(View::Sheet { .. }));
        let folds = (view.is_some() || long) && !sheet;
        let body: Option<AnyElement> = match view {
            Some(View::Meter {
                used,
                max,
                compacts_at,
                parts,
            }) => Some(
                // The agent's last full count says where it compacts.
                meter(*used, *max, compacts_at.or(s.compacts_at), parts, &t).into_any_element(),
            ),
            Some(View::Limits { bars }) => Some(limits(bars, &t).into_any_element()),
            Some(View::Sheet { sections }) => Some(self.sheet(s, ix, name, sections, cx)),
            None => None,
        };
        let words =
            (!sheet && !text.trim().is_empty() && (open || !folds || body.is_none())).then(|| {
                let src = if long && !open && body.is_none() {
                    // The first lines, then the fold.
                    text.lines().take(6).collect::<Vec<_>>().join("\n")
                } else {
                    text.to_string()
                };
                match shape(&src) {
                    // Plain lines read best as they are, selectable.
                    Shape::Plain => {
                        crate::select::text(SharedString::from(format!("command-{ix}")), src, &t)
                            .into_any_element()
                    }
                    Shape::Markdown | Shape::Columns => {
                        let blocks = self.markdown(&as_markdown(&src));
                        let mut r = markdown::R {
                            live: false,
                            counted: true,
                            md: self.mdv.clone(),
                            owner: cx.entity_id(),
                            window,
                            cx,
                            scope: format!("{}/{ix}/command", s.id),
                            bg: t.surface,
                            base: Some(std::path::PathBuf::from(&s.cwd)),
                        };
                        markdown::render_with(blocks.as_slice(), &t, false, true, &mut r)
                            .into_any_element()
                    }
                }
            });
        let fold_label = match (open, body.is_some()) {
            (true, _) => "Show less",
            (false, true) => "Show the full answer",
            (false, false) => "Show all",
        };
        rule(
            div()
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .children(body.map(|b| {
                    // A sheet's rows carry more than a meter's.
                    // A sheet fills the column, its edge on the composer's.
                    if matches!(view, Some(View::Sheet { .. })) {
                        div().child(b)
                    } else {
                        div().max_w(px(420.)).child(b)
                    }
                }))
                .children(words)
                .when(folds && !text.trim().is_empty(), |d| {
                    d.child(
                        div().flex().child(
                            ghost(SharedString::from(format!("fold-{ix}")), fold_label, &t)
                                .ml(px(-space::S2))
                                .relative()
                                .on_press(cx.listener(move |this, _, _, cx| {
                                    this.toggle_fold(&key);
                                    cx.notify();
                                }))
                                .child(probe(&self.probes, format!("fold-{ix}"))),
                        ),
                    )
                }),
        )
    }
}

/// How many rows a section shows before it folds.
const SHEET_ROWS: usize = 6;

/// What a busy action's tooltip says, naming what it's acting on.
fn doing(act: &Act) -> String {
    match act {
        Act::McpEnable {
            enabled: true,
            server,
        } => format!("Turning on {server}…"),
        Act::McpEnable {
            enabled: false,
            server,
        } => format!("Turning off {server}…"),
        Act::McpReconnect { server } => format!("Reconnecting {server}…"),
        Act::McpSignIn { server } => format!("Signing in to {server}…"),
        Act::Open { .. } => "Opening…".into(),
    }
}

/// The colour of a row's mark. Working rows have none: only what's off
/// or wants attention is marked.
fn mark_color(mark: Mark, t: &Theme) -> Option<Option<Hsla>> {
    match mark {
        Mark::Good => None,
        Mark::Off => Some(None),
        Mark::Waiting => Some(Some(t.waiting.0)),
        Mark::Attention | Mark::Bad => Some(Some(t.needs.0)),
    }
}

/// Smaller interface text, for what sits under a row's name, and its
/// line.
const SMALL: f32 = 12.;
const SMALL_LINE: f32 = 16.;

/// A line of interface text, to line marks and controls up with a name.
const LINE: f32 = crate::ds::size::LINE;

/// `s` cut to about `max` characters in the middle: `2222…6666`.
fn elide_middle(s: &str, max: usize) -> String {
    let n = s.chars().count();
    let max = max.max(9);
    if n <= max {
        return s.to_string();
    }
    let head = (max - 1) / 2;
    let tail = max - 1 - head;
    let a: String = s.chars().take(head).collect();
    let b: String = s.chars().skip(n - tail).collect();
    format!("{a}…{b}")
}

/// `s` cut to about `max` characters from the start: `…/proj/site`.
fn elide_start(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max.max(8) {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - max.max(8) + 1).collect();
    format!("…{tail}")
}

impl Dent {
    /// A command's sheet: sections of rows. Every row is the same: a mark
    /// when something's off or needs you, the name, how it stands and
    /// more beneath, and at the end its controls (a switch for on and
    /// off, buttons for the rest) or, with none, its value.
    fn sheet(
        &mut self,
        s: &Session,
        ix: usize,
        command: &str,
        sections: &[Section],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let now = std::time::Instant::now();
        self.signing
            .retain(|_, since| now.duration_since(*since).as_secs() < 180);
        div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .children(sections.iter().enumerate().map(|(si, sec)| {
                let fold = crate::data::Fold::Sheet(&s.id, ix, si);
                let (fold_key, open) = (fold.key(), self.is_open(&fold));
                // A long section folds after its first few rows.
                let n = sec.rows.len();
                let (shown, folded): (Vec<usize>, Vec<usize>) = if open || n <= SHEET_ROWS + 1 {
                    ((0..n).collect(), vec![])
                } else {
                    ((0..SHEET_ROWS).collect(), (SHEET_ROWS..n).collect())
                };
                let rows: Vec<AnyElement> = shown
                    .iter()
                    .map(|&ri| self.sheet_row(s, ix, command, si, ri, &sec.rows[ri], cx))
                    .collect();
                let fold = (!folded.is_empty() || open).then(|| {
                    let label = if open {
                        "Show less".to_string()
                    } else {
                        format!("Show {} more", folded.len())
                    };
                    let gid = format!("sheet-more-{ix}-{si}");
                    let key = fold_key.clone();
                    ghost(SharedString::from(gid.clone()), label, &t)
                        .ml(px(-space::S2))
                        .relative()
                        .on_press(cx.listener(move |this, _, _, cx| {
                            this.toggle_fold(&key);
                            cx.notify();
                        }))
                        .child(probe(&self.probes, gid))
                        .into_any_element()
                });
                let body = div().flex().flex_col().children(rows);
                div()
                    .flex()
                    .flex_col()
                    // Space, not lines, between sections: the rule and the
                    // labels already make the structure.
                    // Rows carry S1H of their own above and below their
                    // words, so the gaps here make up the rest: S3 above
                    // a label, S1H to its note, S2 to the first row. A
                    // gap between sections stays under the one between
                    // sheets, so a sheet's edge is the bigger break.
                    .when(si > 0, |d| d.mt(px(space::S3 - space::S1H)))
                    .child(ui::section_label(&sec.title, None, &t))
                    .when(!sec.note.is_empty(), |d| {
                        d.child(
                            div()
                                .pt(px(space::S1H))
                                .text_size(px(SMALL))
                                .line_height(px(SMALL_LINE))
                                .text_color(t.quiet)
                                .child(sec.note.clone()),
                        )
                    })
                    .child(div().h(px(space::S2 - space::S1H)))
                    .child(body)
                    .children(fold.map(|f| div().pt(px(space::S1)).flex().child(f)))
            }))
            .into_any_element()
    }

    /// One row of a sheet.
    #[allow(clippy::too_many_arguments)]
    fn sheet_row(
        &mut self,
        s: &Session,
        ix: usize,
        command: &str,
        si: usize,
        ri: usize,
        r: &Row,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.t;
        let id = format!("sheet-{ix}-{si}-{ri}");
        let face = |d: Div, field: Field| match r.code == Some(field) {
            true => d.ty(CODE),
            false => d,
        };
        let waiting_browser = r.actions.iter().any(|a| {
            matches!(&a.act, Act::McpSignIn { server }
                if self.signing.contains_key(&(s.id.clone(), server.clone())))
        });
        let error = r.actions.iter().find_map(|a| {
            self.act_errors
                .get(&(s.id.clone(), command.to_string(), a.act.clone()))
                .cloned()
        });
        let controls = !r.actions.is_empty();
        let switch = r
            .actions
            .iter()
            .any(|a| matches!(a.act, Act::McpEnable { .. }));
        let buttons: Vec<AnyElement> = r
            .actions
            .iter()
            .enumerate()
            .map(|(ai, a)| {
                let busy = self
                    .acting
                    .iter()
                    .any(|(sid, c, act)| *sid == s.id && c == command && *act == a.act);
                let bid = format!("{id}-{ai}");
                let press = {
                    let (sid, cmd, act) = (s.id.clone(), command.to_string(), a.act.clone());
                    move |this: &mut Dent, cx: &mut Context<Dent>| {
                        this.act(&sid, &cmd, act.clone(), cx)
                    }
                };
                if let Act::McpEnable { enabled, server } = &a.act {
                    // A switch shows where it's going while it gets there.
                    let on = if busy { *enabled } else { !*enabled };
                    let el = div().id(SharedString::from(bid.clone()));
                    let el = if busy {
                        ui::off_owned(el, doing(&a.act), &t)
                    } else {
                        let what = if *enabled { "Turn on" } else { "Turn off" };
                        el.with_tip(tip_owned(format!("{what} {server}"), t))
                            .on_press(cx.listener(move |this, _, _, cx| press(this, cx)))
                    };
                    return div()
                        .relative()
                        .child(ui::switch(el, on, &t))
                        .child(probe(&self.probes, bid))
                        .into_any_element();
                }
                // Outlined, even for a fix: the row's mark carries the
                // urgency, and the same state always looks the same.
                let b =
                    ui::inner_button(SharedString::from(bid.clone()), a.label.clone(), false, &t)
                        .h(px(crate::ds::size::CONTROL_SM))
                        .px(px(space::S2))
                        .relative()
                        .child(probe(&self.probes, bid));
                if busy {
                    ui::off_owned(b, doing(&a.act), &t).into_any_element()
                } else {
                    b.on_press(cx.listener(move |this, _, _, cx| press(this, cx)))
                        .into_any_element()
                }
            })
            .collect();
        let status = if waiting_browser {
            "Finish signing in in your browser".to_string()
        } else if switch && r.mark == Some(Mark::Off) {
            // The switch says it's off.
            String::new()
        } else {
            r.value.clone()
        };
        let small = |text: String, field: Option<Field>| {
            let d = div()
                .text_size(px(SMALL))
                .line_height(px(SMALL_LINE))
                .text_color(t.quiet)
                .line_clamp(2)
                .child(text);
            match field {
                Some(f) => face(d, f),
                None => d,
            }
        };
        let words = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                face(div().min_w_0().truncate().text_color(t.ink), Field::Label)
                    .child(r.label.clone()),
            )
            // With controls at the end, how it stands goes beneath.
            .when(controls && !status.is_empty(), |d| {
                d.child(small(status.clone(), Some(Field::Value)))
            })
            .when(!r.detail.is_empty(), |d| {
                d.child(small(r.detail.clone(), Some(Field::Detail)))
            })
            // Signals are fills, never text: the dot carries the colour.
            .children(error.map(|e| {
                div()
                    .text_size(px(SMALL))
                    .line_height(px(SMALL_LINE))
                    .text_color(t.ink)
                    .line_clamp(2)
                    .child(e)
            }));
        let trailing = if controls {
            Some(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .children(buttons)
                    .into_any_element(),
            )
        } else if !status.is_empty() {
            let path = r.code == Some(Field::Value) && status.starts_with(['/', '~']);
            // A path keeps its end, which is the part that says where.
            // An id keeps its start and end, which are what people match.
            let room = ((self.chat_w * 0.45) / 7.8) as usize;
            let shown = if path {
                elide_start(&status, room)
            } else if r.code == Some(Field::Value) {
                elide_middle(&status, room)
            } else {
                status
            };
            Some(
                face(
                    div().flex_none().whitespace_nowrap().text_color(t.quiet),
                    Field::Value,
                )
                .child(shown)
                .into_any_element(),
            )
        } else {
            None
        };
        // Everything lines up on the name's line. The mark hangs in the
        // margin, so names start at the same place with or without one.
        let on_line = |d: AnyElement| div().h(px(LINE)).flex_none().flex().items_center().child(d);
        div()
            .id(SharedString::from(id))
            .relative()
            .flex()
            .items_start()
            .gap(px(space::S3))
            .py(px(space::S1H))
            .children(r.mark.and_then(|m| mark_color(m, &t)).map(|c| {
                div()
                    .absolute()
                    .left(px(-space::S3))
                    .top(px(space::S1H))
                    .h(px(LINE))
                    .flex()
                    .items_center()
                    .child(ui::dot(c, &t))
            }))
            .child(words)
            .children(trailing.map(on_line))
            .into_any_element()
    }

    /// Do what a sheet's row offers: open a file here, or ask the daemon.
    pub fn act(&mut self, session: &str, command: &str, act: Act, cx: &mut Context<Self>) {
        if let Act::Open { path } = &act {
            self.open_file(path, cx);
            return;
        }
        let key = (session.to_string(), command.to_string(), act.clone());
        if self.acting.contains(&key) {
            return;
        }
        self.act_errors.remove(&key);
        self.acting.push(key);
        self.live.send(dent_protocol::Request::Act {
            session: session.to_string(),
            command: command.to_string(),
            act,
        });
        cx.notify();
    }

    /// How something asked of a sheet went: open the page it gave, for the
    /// client that asked, or say what went wrong.
    pub fn acted(
        &mut self,
        session: &str,
        command: &str,
        act: &Act,
        url: Option<&str>,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let before = self.acting.len();
        self.acting
            .retain(|(s, c, a)| !(s == session && c == command && a == act));
        if self.acting.len() == before {
            // Someone else asked.
            return;
        }
        if let Some(url) = url {
            if let Act::McpSignIn { server } = act {
                self.signing.insert(
                    (session.to_string(), server.clone()),
                    std::time::Instant::now(),
                );
            }
            cx.open_url(url);
        }
        // What went wrong stays on the row it happened to.
        if let Some(e) = error {
            self.act_errors.insert(
                (session.to_string(), command.to_string(), act.clone()),
                e.to_string(),
            );
        }
        cx.notify();
    }
}

/// How a command's words are written.
#[derive(PartialEq, Debug)]
pub enum Shape {
    /// Headings, tables, lists: Markdown.
    Markdown,
    /// Columns lined up with spaces: kept in the code face.
    Columns,
    /// Plain lines.
    Plain,
}

pub fn shape(text: &str) -> Shape {
    let markdown = text.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with('#')
            || l.starts_with('|')
            || l.starts_with("- ")
            || l.contains("**")
            || l.contains('`')
    });
    if markdown {
        return Shape::Markdown;
    }
    if text.lines().filter(|l| l.trim().contains("   ")).count() >= 2 {
        return Shape::Columns;
    }
    Shape::Plain
}

/// A command's words as Markdown. Claude Code answers some commands in
/// Markdown (`/context`) and others in plain lines (`/autocompact`) or
/// columns lined up with spaces (`/skill-doctor`): plain lines keep their
/// breaks, columns keep their alignment in the code face.
pub fn as_markdown(text: &str) -> String {
    match shape(text) {
        Shape::Markdown => text.to_string(),
        Shape::Columns => format!("```text\n{}\n```", text.trim_end()),
        Shape::Plain => text
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("  \n"),
    }
}

/// A quiet rounded panel for a view that floats: the context meter.
pub fn panel(t: &Theme) -> Div {
    crate::ui::floating(t)
        .p(px(space::S4))
        .rounded(px(radius::XL))
}

#[cfg(test)]
mod tests {
    use super::{Activity, activity_line, tokens};

    #[test]
    fn token_counts_read_naturally() {
        assert_eq!(tokens(940), "940");
        assert_eq!(tokens(18_484), "18k");
        assert_eq!(tokens(1_500), "1.5k");
        assert_eq!(tokens(2_000), "2k");
        assert_eq!(tokens(152_340), "152k");
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(1_260_000), "1.3M");
    }

    #[test]
    fn retries_count_down() {
        let a = Activity::Retrying {
            attempt: 2,
            max: 10,
            at: 10_000,
            error: "Claude is overloaded".into(),
        };
        assert_eq!(
            activity_line(&a, 6_500),
            Some((
                "Retrying in 4s".into(),
                "Claude is overloaded · attempt 2 of 10".into()
            ))
        );
        assert_eq!(activity_line(&a, 10_500).unwrap().0, "Retrying now");
    }
}
