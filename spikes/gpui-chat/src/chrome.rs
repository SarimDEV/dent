//! Block chrome for the specimen: tables capped by rows that scroll inside the
//! chat, copy buttons, a side panel for a whole table, and probes that record
//! where things were drawn so the scripted check can aim real input at them.

use std::cell::{Cell as Flag, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::*;
use pulldown_cmark::Alignment;

use super::{CELL_MAX, CELL_MIN_CAP, CELL_PROSE_MIN, Cell, Specimen};
use crate::ds::{LABEL, READING, Typed, radius, space};
use crate::theme::Theme;

/// Body rows a table shows before it scrolls inside the chat.
pub const TABLE_ROWS: usize = 10;
/// A table only caps when that hides a meaningful number of rows.
const TABLE_SLACK: usize = 3;
/// One single-line row: text, vertical padding and the rule below it.
const ROW_H: f32 = READING.line + 2. * space::S1 + 1.;
/// How long "Copied" stays up.
const COPIED_SECS: f32 = 1.2;

pub type Probes = Rc<RefCell<HashMap<String, Bounds<Pixels>>>>;
pub type CopyText = Rc<dyn Fn() -> String>;

pub fn text(s: String) -> CopyText {
    Rc::new(move || s.clone())
}

/// A table's rows and shared column widths, kept so the inline view, the
/// side panel and copying all read the same data.
pub struct TableData {
    pub head: Rc<[Cell]>,
    /// Shared per row, so a table growing by one row doesn't copy the rest.
    pub rows: Vec<Rc<[Cell]>>,
    pub widths: Vec<f32>,
    pub mins: Vec<f32>,
    pub aligns: Vec<Alignment>,
    pub total: f32,
    pub total_min: f32,
}

impl TableData {
    pub fn new(head: Rc<[Cell]>, rows: Vec<Rc<[Cell]>>, aligns: &[Alignment]) -> Self {
        let pad = 2. * space::S2;
        let all = || std::iter::once(&head).chain(&rows);
        let cols = all().map(|r| r.len()).max().unwrap_or(0);
        let mut widths = vec![0f32; cols];
        let mut mins = vec![0f32; cols];
        for row in all() {
            for (c, cell) in row.iter().enumerate() {
                widths[c] = widths[c].max(cell.natural.min(CELL_MAX) + pad);
                mins[c] = mins[c].max(cell.min.min(CELL_MIN_CAP) + pad);
            }
        }
        for c in 0..cols {
            mins[c] = mins[c].max(widths[c].min(CELL_PROSE_MIN + pad));
            widths[c] = widths[c].max(mins[c]);
        }
        Self {
            head,
            rows,
            total: widths.iter().sum::<f32>() + 2.,
            total_min: mins.iter().sum::<f32>() + 2.,
            widths,
            mins,
            aligns: aligns.to_vec(),
        }
    }

    pub fn capped(&self) -> bool {
        self.rows.len() > TABLE_ROWS + TABLE_SLACK
    }

    /// GitHub-flavoured Markdown, normalised from the cells.
    pub fn markdown(&self) -> String {
        let line = |cells: &[Cell]| {
            let parts: Vec<String> = cells
                .iter()
                .map(|c| c.inline.plain.replace('|', "\\|"))
                .collect();
            format!("| {} |", parts.join(" | "))
        };
        let rule: Vec<&str> = (0..self.widths.len())
            .map(|c| match self.aligns.get(c) {
                Some(Alignment::Left) => ":---",
                Some(Alignment::Center) => ":---:",
                Some(Alignment::Right) => "---:",
                _ => "---",
            })
            .collect();
        let mut out = vec![line(&self.head), format!("| {} |", rule.join(" | "))];
        out.extend(self.rows.iter().map(|r| line(r)));
        out.join("\n")
    }

    pub fn csv(&self) -> String {
        let field = |s: &str| {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_string()
            }
        };
        std::iter::once(&self.head)
            .chain(&self.rows)
            .map(|r| {
                r.iter()
                    .map(|c| field(&c.inline.plain))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The first body row whose text contains `needle`.
    pub fn find(&self, needle: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.iter().any(|c| c.inline.plain.contains(needle)))
    }
}

/// Room on the right of a scrolling table for its scrollbar.
pub const GUTTER: f32 = 10.;

/// One table row. Every row uses the same flex basis, grow and minimum per
/// column, so columns line up without a grid. `gutter` keeps the last column
/// clear of a scrollbar, on the header too so columns stay aligned.
pub fn row(
    data: &TableData,
    cells: &[Cell],
    head: bool,
    rule: bool,
    gutter: bool,
    t: &Theme,
) -> Div {
    let pad = 2. * space::S2;
    div()
        .flex()
        .w_full()
        .when(gutter, |d| d.pr(px(GUTTER)))
        .when(head, |d| {
            d.bg(t.ink.opacity(0.04)).font_weight(FontWeight::MEDIUM)
        })
        .when(rule, |d| d.border_b_1().border_color(t.hairline))
        .children(cells.iter().enumerate().map(|(c, cell)| {
            let w = data.widths.get(c).copied().unwrap_or(pad);
            div()
                .flex_basis(px(w))
                .flex_grow(w)
                .flex_shrink(1.)
                .min_w(px(data.mins.get(c).copied().unwrap_or(pad)))
                .px(px(space::S2))
                .py(px(space::S1))
                .map(|d| match data.aligns.get(c) {
                    Some(Alignment::Right) => d.text_right(),
                    Some(Alignment::Center) => d.text_center(),
                    _ => d,
                })
                .child(cell.inline.clone().element(t))
        }))
}

/// Records where an element was laid out, under `name`.
pub fn probe(probes: &Probes, name: String) -> impl IntoElement {
    let probes = probes.clone();
    canvas(
        move |bounds, _, _| {
            probes.borrow_mut().insert(name, bounds);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Scroll handoff for a list nested in the scrolling chat. GPUI lets every
/// scroller under the pointer take the wheel, so without this the table and
/// the chat would scroll together. In the capture phase we note whether the
/// list can still move in the wheel's direction; in the bubble phase, after
/// the list has scrolled, we stop the event there. At either end the event
/// carries on to the chat.
///
/// Must be painted before the list so its bubble handler runs after the list's.
fn handoff(state: ListState) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let state = state.clone();
            let inner = Rc::new(Flag::new(false));
            window.on_mouse_event(move |e: &ScrollWheelEvent, phase, _window, cx| {
                if !bounds.contains(&e.position) {
                    return;
                }
                match phase {
                    DispatchPhase::Capture => {
                        let d = e.delta.pixel_delta(px(20.));
                        // GPUI's list merges a frame's wheel deltas by sign, and
                        // 0.0 counts as positive: the zero-delta event that ends a
                        // trackpad gesture would cancel that frame's scrolling.
                        if d.x.as_f32() == 0. && d.y.as_f32() == 0. {
                            cx.stop_propagation();
                            return;
                        }
                        let dy = d.y.as_f32();
                        let top = -state.scroll_px_offset_for_scrollbar().y.as_f32();
                        let max = state.max_offset_for_scrollbar().y.as_f32();
                        inner.set(if dy < 0. {
                            top < max - 0.5
                        } else if dy > 0. {
                            top > 0.5
                        } else {
                            false
                        });
                    }
                    DispatchPhase::Bubble => {
                        if inner.get() {
                            cx.stop_propagation();
                        }
                    }
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A scrollbar that is always visible while there is more to see, so a
/// capped table reads as scrollable before anyone touches it.
fn scrollbar(state: ListState, t: &Theme) -> impl IntoElement {
    let ink = t.ink;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let view = bounds.size.height.as_f32();
            let max = state.max_offset_for_scrollbar().y.as_f32();
            if max <= 0.5 {
                return;
            }
            let top = -state.scroll_px_offset_for_scrollbar().y.as_f32();
            let thumb = (view * view / (view + max)).max(24.);
            let y = (top / max).clamp(0., 1.) * (view - thumb);
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(bounds.right() - px(7.), bounds.top() + px(y + 2.)),
                        size(px(4.), px(thumb - 4.)),
                    ),
                    ink.opacity(0.28),
                )
                .corner_radii(px(2.)),
            );
        },
    )
    .absolute()
    .top_0()
    .right_0()
    .w(px(10.))
    .h_full()
}

fn fade(top: bool, bg: Hsla) -> Div {
    let (a, b) = if top {
        (bg, bg.opacity(0.))
    } else {
        (bg.opacity(0.), bg)
    };
    div()
        .absolute()
        .left_0()
        .right_0()
        .h(px(28.))
        .when(top, |d| d.top_0())
        .when(!top, |d| d.bottom_0())
        .bg(linear_gradient(
            180.,
            linear_color_stop(a, 0.),
            linear_color_stop(b, 1.),
        ))
}

impl Specimen {
    fn list_for(&mut self, key: usize, n: usize) -> ListState {
        let state = self
            .table_lists
            .entry(key)
            .or_insert_with(|| ListState::new(0, ListAlignment::Top, px(ROW_H * 4.)))
            .clone();
        let have = state.item_count();
        if have != n {
            if have < n {
                // Rows streaming in append; the view stays where the reader left it.
                state.splice(have..have, n - have);
            } else {
                state.reset(n);
            }
            // The list only knows the heights of rows it has drawn. Without a
            // hint its scroll end sits short of the real one, and the wheel
            // would hand off to the chat with rows still hidden.
            let _ = state.clone().with_uniform_item_height(px(ROW_H));
        }
        state
    }

    /// A small text button that copies `text` and says so for a moment.
    /// `text` runs only when clicked, so a big table's Markdown and CSV are
    /// never built just to draw the button.
    pub fn copy_button(
        &mut self,
        key: String,
        label: &'static str,
        text: CopyText,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let copied = self
            .copied
            .as_ref()
            .is_some_and(|(k, at)| *k == key && at.elapsed().as_secs_f32() < COPIED_SECS);
        let probes = self.probes.clone();
        let id = key.clone();
        self.copy_texts.insert(key.clone(), text.clone());
        self.button(
            id.clone(),
            if copied { "Copied" } else { label },
            copied,
            &probes,
            cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text()));
                this.copied = Some((id.clone(), Instant::now()));
                cx.notify();
            }),
        )
    }

    pub fn button(
        &self,
        key: String,
        label: &'static str,
        active: bool,
        probes: &Probes,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let t = self.theme;
        div()
            .id(SharedString::from(key.clone()))
            .relative()
            .px(px(space::S2))
            .py(px(2.))
            .rounded(px(radius::SM))
            .ty(LABEL)
            .text_color(if active { t.ink } else { t.quiet })
            .when(active, |d| d.bg(t.ink.opacity(0.06)))
            .hover(|s| s.bg(t.ink.opacity(0.06)).text_color(t.ink))
            .cursor_pointer()
            .on_click(on_click)
            .child(label)
            .child(probe(probes, key))
            .into_any_element()
    }

    /// The table as it sits in the chat: header, body capped at
    /// `TABLE_ROWS` rows that scrolls inside, and a footer with the row
    /// count and actions.
    pub(super) fn table(
        &mut self,
        rows: Vec<Vec<Cell>>,
        aligns: &[Alignment],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut rows = rows.into_iter().map(Rc::from);
        let head = rows.next().unwrap_or_else(|| Rc::from(Vec::new()));
        let data = Rc::new(TableData::new(head, rows.collect(), aligns));
        self.table_view(data, cx)
    }

    pub(super) fn table_view(&mut self, data: Rc<TableData>, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let key = self.block_ix;
        self.tables.insert(key, data.clone());
        let n = data.rows.len();
        let body: AnyElement = if data.capped() {
            let state = self.list_for(key, n);
            let top = -state.scroll_px_offset_for_scrollbar().y.as_f32();
            let max = state.max_offset_for_scrollbar().y.as_f32();
            // Half a row peeks out at the bottom: a cut-off row says "more".
            let h = (TABLE_ROWS as f32 + 0.5) * ROW_H;
            let rows = data.clone();
            div()
                .relative()
                .h(px(h))
                .child(handoff(state.clone()))
                .child(
                    list(state.clone(), move |ix, _, _| {
                        row(
                            &rows,
                            &rows.rows[ix],
                            false,
                            ix + 1 < rows.rows.len(),
                            true,
                            &t,
                        )
                        .into_any_element()
                    })
                    .size_full(),
                )
                .when(top > 0.5, |d| d.child(fade(true, t.surface)))
                .when(top < max - 0.5, |d| d.child(fade(false, t.surface)))
                .child(scrollbar(state, &t))
                .child(probe(&self.probes, format!("table-body-{key}")))
                .into_any_element()
        } else {
            div()
                .children(
                    data.rows
                        .iter()
                        .enumerate()
                        .map(|(i, r)| row(&data, r, false, i + 1 < n, false, &t)),
                )
                .into_any_element()
        };
        let gutter = if data.capped() { GUTTER } else { 0. };
        let grid = div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(data.total_min + gutter))
            .max_w(px(data.total + gutter))
            .border_1()
            .border_color(t.hairline)
            .rounded(px(radius::MD))
            .overflow_hidden()
            .child(row(&data, &data.head, true, true, data.capped(), &t))
            .child(body);
        let scroller = self.hscroll(grid, t.surface);
        let footer = self.table_footer(key, &data, cx);
        // The footer spans the table, not the column, so its actions sit
        // under the table they act on.
        div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(2.))
            .w_full()
            .max_w(px(data.total + gutter))
            .child(scroller)
            .child(footer)
            .child(probe(&self.probes, format!("table-{key}")))
            .into_any_element()
    }

    fn table_footer(&mut self, key: usize, data: &Rc<TableData>, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let n = data.rows.len();
        let (a, b) = (data.clone(), data.clone());
        let md = self.copy_button(
            format!("copy-md-{key}"),
            "Copy",
            Rc::new(move || a.markdown()),
            cx,
        );
        let csv = self.copy_button(
            format!("copy-csv-{key}"),
            "CSV",
            Rc::new(move || b.csv()),
            cx,
        );
        let probes = self.probes.clone();
        let open = data.capped().then(|| {
            self.button(
                format!("open-{key}"),
                "Open",
                false,
                &probes,
                cx.listener(move |this, _, _, cx| {
                    this.panel = Some(key);
                    this.panel_list.reset(0);
                    cx.notify();
                }),
            )
        });
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .ty(LABEL)
                    .text_color(t.quiet)
                    .pl(px(space::S1))
                    .when(data.capped(), |d| d.child(format!("{n} rows"))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(space::S1))
                    .child(md)
                    .child(csv)
                    .children(open),
            )
    }

    /// The whole table in a panel over the right of the window, every row
    /// reachable and only the visible ones drawn.
    pub fn panel(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let key = self.panel?;
        let data = self.tables.get(&key)?.clone();
        let t = self.theme;
        let n = data.rows.len();
        if self.panel_list.item_count() != n {
            self.panel_list.reset(n);
            let _ = self.panel_list.clone().with_uniform_item_height(px(ROW_H));
        }
        let state = self.panel_list.clone();
        let w = window.viewport_size().width.as_f32().min(720.);
        let probes = self.probes.clone();
        let (a, b) = (data.clone(), data.clone());
        let md = self.copy_button(
            format!("panel-md-{key}"),
            "Copy",
            Rc::new(move || a.markdown()),
            cx,
        );
        let csv = self.copy_button(
            format!("panel-csv-{key}"),
            "CSV",
            Rc::new(move || b.csv()),
            cx,
        );
        let close = self.button(
            "panel-close".into(),
            "Close",
            false,
            &probes,
            cx.listener(|this, _, _, cx| {
                this.panel = None;
                cx.notify();
            }),
        );
        let rows = data.clone();
        let grid = div()
            .flex()
            .flex_col()
            .h_full()
            .w_full()
            .min_w(px(data.total_min))
            .max_w(px(data.total + GUTTER))
            .child(row(&data, &data.head, true, true, true, &t))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        list(state.clone(), move |ix, _, _| {
                            row(&rows, &rows.rows[ix], false, true, true, &t).into_any_element()
                        })
                        .size_full(),
                    )
                    .child(scrollbar(state, &t))
                    .child(probe(&probes, "panel-body".into())),
            );
        Some(
            div()
                .absolute()
                .top_0()
                .right_0()
                .h_full()
                .w(px(w))
                .flex()
                .flex_col()
                .bg(t.surface)
                .border_l_1()
                .border_color(t.hairline)
                .shadow_lg()
                // The chat behind the panel gets no pointer or wheel input.
                .occlude()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(space::S4))
                        .py(px(space::S2))
                        .border_b_1()
                        .border_color(t.hairline)
                        .child(
                            div()
                                .ty(LABEL)
                                .text_color(t.quiet)
                                .child(format!("Table · {n} rows")),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(space::S1))
                                .child(md)
                                .child(csv)
                                .child(close),
                        ),
                )
                .child(
                    div()
                        .id("panel-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_x_scroll()
                        .restrict_scroll_to_axis()
                        .ty(READING)
                        .child(grid),
                )
                .child(probe(&probes, "panel".into()))
                .into_any_element(),
        )
    }

    /// Scroll the chat to the first table row containing `needle`, and the
    /// table to that row. In-app search uses this to reach rows a capped
    /// table keeps out of view.
    pub fn reveal(&mut self, needle: &str) -> Option<(usize, usize)> {
        let mut keys: Vec<usize> = self.tables.keys().copied().collect();
        keys.sort();
        for key in keys {
            let data = self.tables[&key].clone();
            if let Some(ix) = data.find(needle) {
                self.scroll.scroll_to_item(key);
                if let Some(state) = self.table_lists.get(&key) {
                    state.scroll_to_reveal_item(ix);
                }
                return Some((key, ix));
            }
        }
        None
    }
}
