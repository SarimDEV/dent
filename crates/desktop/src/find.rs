//! ⌘F: find in what you're reading: the chat, or the document, changes
//! or side chat in the viewer if that's where you last clicked or
//! scrolled. Every match on screen is tinted and the current one stands
//! out; Return and ⇧Return (or ⌘G and ⇧⌘G) step through them, scrolling
//! to each, and Esc puts the bar away.

use crate::ui::FocusField;
use std::hash::{Hash, Hasher};

use gpui_kit::base::input::Textarea;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::app::{Dent, Tab};
use crate::data::{Item, Kind, Line, Session};
use crate::ds::{LABEL, Typed, UI, motion, radius, size, space};
use crate::select::Area;
use crate::ui::{Arrive, From, Press, Tipped, floating, icon, icon_button, probe, tip};
use crate::viewer::Row;

/// Where ⌘F looks, within the open session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Chat,
    /// A Markdown document in the viewer.
    Doc(&'static str),
    /// The file shown in Changes.
    Changes,
    Aside,
    /// Your terminal: its scrollback and screen.
    Terminal,
    /// A web page or dev server: the page finds and counts for itself.
    Page(&'static str),
}

/// The tint on every match, and the stronger one on the current match. On
/// Carbon the current one stays light enough for the ink on it to read
/// (about 5.5:1, where 70% was 2.8:1); its edge (`edge`) marks it too.
pub fn tints(t: &crate::theme::Theme) -> (Hsla, Hsla) {
    (
        t.waiting.0.opacity(if t.dark { 0.22 } else { 0.30 }),
        t.waiting.0.opacity(if t.dark { 0.42 } else { 0.62 }),
    )
}

/// The current match's 1 px edge, in the full signal.
pub fn edge(t: &crate::theme::Theme) -> Hsla {
    t.waiting.0
}

/// Frames to look for a match to bring into view before giving up.
const REVEAL_FRAMES: u8 = 6;

/// What the bar found, kept in step with what it searches as that grows.
pub struct Find {
    pub session: String,
    pub scope: Scope,
    /// Each match: its item, and which match it is within that item.
    pub hits: Vec<(usize, usize)>,
    /// The current match.
    pub at: usize,
    /// What the hits were counted against: the query and a digest of the
    /// text, so a reply streaming in is searched too.
    sig: (String, u64),
    /// A match to bring into view once it's next drawn: how many more
    /// frames to look for it (it may not be laid out on the first).
    reveal: u8,
    /// In the terminal: where each match is.
    pub cells: Vec<alacritty_terminal::term::search::Match>,
    /// In a page: how many matches it last said it has.
    pub page_count: usize,
    /// In the chat: how many matches each item draws, and each item's count
    /// by its words, so only what changed is counted again.
    chat: std::rc::Rc<Vec<usize>>,
    memo: std::collections::HashMap<u64, usize>,
}

/// The words a chat item draws as text, a piece at a time: your message
/// and a plan (folded as they're shown), a reply as rendered, a note. Folded
/// work, cards and waiting messages aren't searched.
pub fn item_words(
    item: &Item,
    open: bool,
    parse: impl FnOnce(&str) -> std::rc::Rc<Vec<crate::markdown::Block>>,
) -> Vec<String> {
    match item {
        Item::User { text, .. } if crate::data::command_name(text).is_some() => Vec::new(),
        Item::User { text, .. } => {
            let (_, body) = crate::data::attachment_lines(text);
            if body.is_empty() {
                return Vec::new();
            }
            vec![crate::chat::user_shown(&body, open).0]
        }
        Item::Text(src) => {
            let mut out = Vec::new();
            crate::markdown::drawn(&parse(src), &mut out);
            out
        }
        // A long plan only as far as it shows until it's expanded.
        Item::Plan { plan, .. } => {
            let mut out = Vec::new();
            crate::markdown::drawn(&parse(&crate::chat::plan_shown(plan, open).0), &mut out);
            out
        }
        Item::Note { text, .. } => vec![text.clone()],
        _ => Vec::new(),
    }
}

impl Dent {
    /// What the viewer shows that ⌘F can search, if it's showing.
    pub fn viewer_scope(&self, s: &Session) -> Option<Scope> {
        if !(self.panel_open && self.shows_panel(s)) {
            return None;
        }
        match self.tab(s) {
            Tab::Art(id) if self.art(id).is_some_and(|a| a.kind == Kind::Doc) => {
                Some(Scope::Doc(id))
            }
            Tab::Changes if !s.diff.is_empty() => Some(Scope::Changes),
            Tab::Aside => Some(Scope::Aside),
            Tab::Terminal if self.shells.contains_key(&s.id) => Some(Scope::Terminal),
            Tab::Art(id)
                if self
                    .art(id)
                    .is_some_and(|a| matches!(a.kind, Kind::Html | Kind::Server)) =>
            {
                Some(Scope::Page(id))
            }
            _ => None,
        }
    }

    /// The selection area a scope's text is drawn in.
    pub fn scope_area(&self, s: &Session, scope: Scope) -> Area {
        crate::select::area(&match scope {
            Scope::Chat => format!("chat/{}", s.id),
            Scope::Doc(id) => format!("doc/{id}"),
            Scope::Changes => format!(
                "diff/{}/{}",
                s.id,
                s.diff
                    .get(self.diff_selected(s))
                    .map_or("", |f| f.path.as_str())
            ),
            Scope::Aside => format!("aside/{}", s.id),
            // Not drawn by dent's text: nothing is tinted by it.
            Scope::Terminal => format!("term/{}", s.id),
            Scope::Page(id) => format!("page/{id}"),
        })
    }

    /// ⌘F: open the bar on what you were last reading, or take the
    /// keyboard back to it with its text selected to type over.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.current_rc() else {
            return;
        };
        // The terminal or a page with the keyboard; else where you last
        // clicked or scrolled.
        let keys_in = |v: &Scope| match v {
            Scope::Terminal => self.term_focus.is_focused(window),
            Scope::Page(_) => self
                .preview
                .as_ref()
                .is_some_and(|p| p.borrow().has_focus()),
            _ => false,
        };
        let scope = self
            .viewer_scope(&s)
            .filter(|v| keys_in(v) || crate::select::last_area() == Some(self.scope_area(&s, *v)))
            .unwrap_or(Scope::Chat);
        if let Some(p) = &self.preview {
            p.borrow().release_focus();
        }
        if self.find.as_ref().is_some_and(|f| f.scope != scope) {
            self.unmark_page();
        }
        if self
            .find
            .as_ref()
            .is_none_or(|f| f.session != s.id || f.scope != scope)
        {
            self.find = Some(Find {
                session: s.id.clone(),
                scope,
                hits: Vec::new(),
                at: 0,
                sig: (String::new(), 0),
                reveal: 0,
                cells: Vec::new(),
                page_count: 0,
                chat: Default::default(),
                memo: Default::default(),
            });
        }
        self.find_input.update(cx, |f, cx| {
            let n = f.value().len();
            f.set_selected_range(0..n, cx);
            f.focus_field(window, cx);
        });
        self.refresh_find(true, cx);
        cx.notify();
    }

    /// Put the bar away; what it searched keeps its place and the composer
    /// gets the keyboard.
    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.unmark_page();
        let Some(f) = self.find.take() else {
            return false;
        };
        crate::select::set_find(None);
        // Back to your terminal if that's what it searched.
        if f.scope == Scope::Terminal {
            window.focus(&self.term_focus, cx);
        } else {
            self.composer.update(cx, |c, cx| c.focus_field(window, cx));
        }
        cx.notify();
        true
    }

    /// Each match of `query` in `scope`, item by item.
    fn find_hits(&self, s: &Session, scope: Scope, query: &str) -> Vec<(usize, usize)> {
        let each = |counts: Vec<(usize, usize)>| -> Vec<(usize, usize)> {
            counts
                .into_iter()
                .flat_map(|(ix, n)| (0..n).map(move |k| (ix, k)))
                .collect()
        };
        if query.is_empty() {
            return Vec::new();
        }
        match scope {
            // Counted by `chat_counts`, from what each item draws.
            Scope::Chat => each(
                self.find
                    .as_ref()
                    .map(|f| f.chat.iter().copied().enumerate().collect())
                    .unwrap_or_default(),
            ),
            // Rows of the diff, drawn or not: the text of each line.
            Scope::Changes => {
                let Some(file) = s.diff.get(self.diff_selected(s)) else {
                    return Vec::new();
                };
                let (rows, _) = self.diff_rows(file);
                each(
                    rows.iter()
                        .enumerate()
                        .map(|(ix, r)| match r {
                            Row::Line(i) => {
                                let l = &file.lines[*i];
                                let n = if l.kind == Line::Hunk || l.text.is_empty() {
                                    0
                                } else {
                                    crate::select::matches(&l.text, query).len()
                                };
                                (ix, n)
                            }
                            _ => (ix, 0),
                        })
                        .collect(),
                )
            }
            // What it last said it found, a match at a time.
            Scope::Page(_) => {
                let n = self.find.as_ref().map_or(0, |f| f.page_count);
                (0..n).map(|k| (k, 0)).collect()
            }
            Scope::Terminal => {
                let n = self.find.as_ref().map_or(0, |f| f.cells.len());
                (0..n).map(|k| (k, 0)).collect()
            }
            // Drawn whole: what was laid out.
            Scope::Doc(_) | Scope::Aside => each(
                crate::select::counts(self.scope_area(s, scope), query)
                    .into_iter()
                    .map(|(ix, n)| (ix as usize, n))
                    .collect(),
            ),
        }
    }

    /// Count the matches again if the query or what's searched changed.
    /// With `jump`, the current match is the first one at or below what's
    /// on screen, and it's scrolled to.
    pub fn refresh_find(&mut self, jump: bool, cx: &mut Context<Self>) {
        let Some(f) = &self.find else { return };
        let Some(s) = self.session(&f.session).cloned() else {
            self.find = None;
            crate::select::set_find(None);
            return;
        };
        // The pane it searched went away: another tab, the viewer hidden.
        if f.scope != Scope::Chat && self.viewer_scope(&s) != Some(f.scope) {
            self.unmark_page();
            self.find = None;
            crate::select::set_find(None);
            return;
        }
        let scope = f.scope;
        let query = self.find_input.read(cx).value().to_string();
        // A digest of what's searched, cheap to take each frame: a list's
        // length and its last item's, or for what's drawn whole, the
        // matches themselves.
        let mut h = std::hash::DefaultHasher::new();
        let mut counted = None;
        match scope {
            // Every item's words by their length (a reply above waiting
            // messages still streams), and whether a long message is open.
            Scope::Chat => {
                s.items.len().hash(&mut h);
                for i in s.items.iter() {
                    match i {
                        Item::User { id, text, .. } => {
                            (text.len(), self.expanded.contains(id)).hash(&mut h)
                        }
                        Item::Text(t) | Item::Plan { plan: t, .. } | Item::Note { text: t, .. } => {
                            t.len().hash(&mut h)
                        }
                        _ => 0.hash(&mut h),
                    }
                }
            }
            Scope::Changes => {
                self.diff_selected(&s).hash(&mut h);
                s.diff
                    .get(self.diff_selected(&s))
                    .map(|f| (&f.path, f.lines.len()))
                    .hash(&mut h);
                self.review.len().hash(&mut h);
                self.commenting.hash(&mut h);
            }
            Scope::Doc(_) | Scope::Aside => {
                let hits = self.find_hits(&s, scope, &query);
                hits.hash(&mut h);
                counted = Some(hits);
            }
            Scope::Page(_) => f.page_count.hash(&mut h),
            // Searched again when something new is printed.
            Scope::Terminal => {
                self.shells.get(&s.id).map(|sh| sh.find_sig()).hash(&mut h);
            }
        }
        let sig = (query, h.finish());
        if sig != f.sig {
            if scope == Scope::Chat {
                self.chat_counts(&s, &sig.0);
            }
            if scope == Scope::Terminal {
                let cells = self
                    .shells
                    .get(&s.id)
                    .map_or(Vec::new(), |sh| sh.find_all(&sig.0));
                if let Some(f) = self.find.as_mut() {
                    f.cells = cells;
                }
            }
            let hits = counted.unwrap_or_else(|| self.find_hits(&s, scope, &sig.0));
            // The first item you can see, at least partly.
            let top = match scope {
                Scope::Chat => {
                    let view = self.transcript.viewport_bounds();
                    (0..s.items.len())
                        .find(|ix| {
                            self.transcript
                                .bounds_for_item(*ix)
                                .is_some_and(|b| b.bottom() > view.top() + px(space::S6))
                        })
                        .unwrap_or_else(|| self.transcript.logical_scroll_top().item_ix)
                }
                Scope::Changes => self.diff_list.logical_scroll_top().item_ix,
                // The first match at or below the top of the screen.
                Scope::Terminal => self.shells.get(&s.id).map_or(0, |sh| {
                    let top = -(sh.term.lock().grid().display_offset() as i32);
                    let cells = &self.find.as_ref().expect("find").cells;
                    cells
                        .iter()
                        .position(|m| m.start().line.0 >= top)
                        .unwrap_or(0)
                }),
                Scope::Doc(_) | Scope::Aside | Scope::Page(_) => 0,
            };
            let f = self.find.as_mut().expect("find");
            let query_changed = f.sig.0 != sig.0;
            f.hits = hits;
            f.sig = sig;
            if query_changed || jump {
                // From where you're reading, so typing doesn't throw you
                // back to the start of something long.
                f.at = f.hits.iter().position(|h| h.0 >= top).unwrap_or(0);
                self.reveal_find();
            } else {
                f.at = f.at.min(f.hits.len().saturating_sub(1));
            }
        }
        self.paint_find(cx);
    }

    /// Count the matches of `query` in each chat item's drawn words, again
    /// only for items whose words changed.
    fn chat_counts(&mut self, s: &Session, query: &str) {
        let Some(mut f) = self.find.take() else {
            return;
        };
        if f.sig.0 != query {
            f.memo.clear();
        }
        // The reply streaming in is drawn from its stream, not the cache.
        let live = self.births.get(&s.id).map(|(ix, _)| *ix);
        if f.memo.len() > 2 * s.items.len() + 64 {
            f.memo.clear();
        }
        let counts = s
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let open = match item {
                    Item::User { id, .. } => self.expanded.contains(id),
                    Item::Plan { id, .. } => self.expanded.contains(&crate::chat::plan_key(id)),
                    _ => false,
                };
                let mut h = std::hash::DefaultHasher::new();
                match item {
                    Item::User { text, .. } => (0u8, text, open).hash(&mut h),
                    Item::Text(t) => (1u8, t).hash(&mut h),
                    Item::Plan { plan, .. } => (2u8, plan, open).hash(&mut h),
                    Item::Note { text, .. } => (3u8, text).hash(&mut h),
                    _ => return 0,
                }
                let key = h.finish();
                let md = &mut self.md;
                *f.memo.entry(key).or_insert_with(|| {
                    item_words(item, open, |src| {
                        if live == Some(ix) {
                            md.live(&s.id, src)
                        } else {
                            md.get(src)
                        }
                    })
                    .iter()
                    .map(|w| crate::select::matches(w, query).len())
                    .sum()
                })
            })
            .collect();
        f.chat = std::rc::Rc::new(counts);
        self.find = Some(f);
    }

    /// Step to the next match, or the previous, round the ends.
    pub fn step_find(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            self.open_find(window, cx);
            return;
        }
        let Some(f) = self.find.as_mut() else { return };
        let n = f.hits.len();
        if n == 0 {
            return;
        }
        f.at = if back {
            (f.at + n - 1) % n
        } else {
            (f.at + 1) % n
        };
        self.reveal_find();
        self.paint_find(cx);
        cx.notify();
    }

    fn reveal_find(&mut self) {
        let Some(f) = self.find.as_mut() else { return };
        // A page counts as it looks: it's asked even for nothing found yet.
        if let Scope::Page(_) = f.scope
            && let Some(p) = &self.preview
        {
            let (tint, strong) = tints(&self.t);
            p.borrow().find(&f.sig.0, f.at, tint, strong);
            return;
        }
        let Some(&(ix, _)) = f.hits.get(f.at) else {
            return;
        };
        match f.scope {
            // Its message first, then the match within it once that's
            // laid out: a long reply's later matches are in view too.
            Scope::Chat => {
                self.transcript.scroll_to_reveal_item(ix);
                f.reveal = REVEAL_FRAMES;
            }
            Scope::Changes => self.diff_list.scroll_to_reveal_item(ix),
            Scope::Terminal => {
                if let (Some(m), Some(sh)) = (f.cells.get(ix), self.shells.get(&f.session)) {
                    sh.show_match(m);
                }
            }
            Scope::Page(_) => {}
            // Found where it was laid out, once it's drawn.
            Scope::Doc(_) | Scope::Aside => f.reveal = REVEAL_FRAMES,
        }
    }

    /// Bring a match in the chat or the viewer into view, as laid out last
    /// frame; if it wasn't laid out yet, look again next frame, a few times.
    pub fn reveal_found(&mut self, s: &Session, window: &mut Window, cx: &App) {
        let Some(f) = self
            .find
            .as_ref()
            .filter(|f| f.reveal > 0 && f.session == s.id)
        else {
            return;
        };
        let Some(&(ix, k)) = f.hits.get(f.at) else {
            return;
        };
        let area = self.scope_area(s, f.scope);
        let needle = self.find_input.read(cx).value().to_string();
        let found = crate::select::reveal(area, ix as u32, k, &needle, window);
        if let Some(f) = self.find.as_mut() {
            f.reveal = if found { 0 } else { f.reveal - 1 };
        }
        if found {
            window.refresh();
        } else {
            window.request_animation_frame();
        }
    }

    /// Take a page's marks off it, when find leaves it.
    pub fn unmark_page(&self) {
        if let Some(Find {
            scope: Scope::Page(_),
            ..
        }) = &self.find
            && let Some(p) = &self.preview
        {
            let (tint, strong) = tints(&self.t);
            p.borrow().find("", 0, tint, strong);
        }
    }

    /// What a page said it found.
    pub fn page_found(&mut self, n: usize) {
        if let Some(f) = self
            .find
            .as_mut()
            .filter(|f| matches!(f.scope, Scope::Page(_)))
        {
            f.page_count = n;
        }
    }

    fn paint_find(&self, cx: &App) {
        let t = self.t;
        let (tint, strong) = tints(&t);
        let target = self.find.as_ref().and_then(|f| {
            let s = self.session(&f.session)?;
            Some((f, self.scope_area(s, f.scope)))
        });
        crate::select::set_find(target.map(|(f, area)| crate::select::Find {
            area,
            needle: self.find_input.read(cx).value().to_string(),
            current: f.hits.get(f.at).map(|&(ix, k)| (ix as u32, k)),
            tint,
            strong,
            edge: edge(&t),
            limits: (f.scope == Scope::Chat).then(|| f.chat.clone()),
        }));
    }

    /// The bar, over the top right corner of what it searches: `here`.
    pub fn find_bar(&mut self, here: Scope, cx: &mut Context<Self>) -> Option<AnyElement> {
        let f = self.find.as_ref()?;
        if Some(&f.session) != self.selected.as_ref() || f.scope != here {
            return None;
        }
        // Below a document's toolbar.
        let top = match here {
            Scope::Doc(_) => size::TOOLBAR + space::S2,
            _ => space::S2,
        };
        let t = self.t;
        let typed = !self.find_input.read(cx).value().is_empty();
        let n = f.hits.len();
        let count = if !typed {
            String::new()
        } else if n == 0 {
            "No matches".into()
        } else {
            format!("{} of {n}", f.at + 1)
        };
        let step = |id: &'static str, name: &'static str, says: &'static str, keys, back: bool| {
            icon_button(id, name, &t)
                .size(px(size::CONTROL_SM))
                .when(n == 0, |d| d.opacity(0.45).cursor_not_allowed())
                .with_tip(tip(says, Some(keys), t))
                .on_press(cx.listener(move |this, _, window, cx| this.step_find(back, window, cx)))
        };
        Some(
            // In a narrow pane the field gives way first.
            div()
                .absolute()
                .top(px(top))
                .left(px(space::S4))
                .right(px(space::S4))
                .flex()
                .justify_end()
                .child(
                    floating(&t)
                        .id("find-bar")
                        .occlude()
                        .min_w_0()
                        .rounded(px(radius::LG))
                        .h(px(size::CONTROL + space::S2))
                        .pl(px(space::S3))
                        .pr(px(space::S1))
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(icon("search", 13., t.quiet))
                        .child(
                            div()
                                .relative()
                                .w(px(200.))
                                .min_w(px(64.))
                                .flex_shrink(1.)
                                .ty(UI)
                                .text_color(t.ink)
                                .child(Textarea::new(&self.find_input))
                                .child(probe(&self.probes, "find-field")),
                        )
                        .child(
                            div()
                                .min_w(px(72.))
                                .flex()
                                .justify_end()
                                .ty(LABEL)
                                .text_color(if typed && n == 0 { t.needs.0 } else { t.quiet })
                                .child(count),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .child(step(
                                    "find-prev",
                                    "chevron-up",
                                    "Previous match",
                                    "⇧ ⌘ G",
                                    true,
                                ))
                                .child(step(
                                    "find-next",
                                    "chevron-down",
                                    "Next match",
                                    "⌘ G",
                                    false,
                                )),
                        )
                        .child(
                            icon_button("find-close", "x", &t)
                                .size(px(size::CONTROL_SM))
                                .with_tip(tip("Close", Some("Esc"), t))
                                .on_press(cx.listener(|this, _, window, cx| {
                                    this.close_find(window, cx);
                                })),
                        )
                        .child(probe(&self.probes, "find-bar"))
                        .arrive("find-arrive", From::Above, motion::FAST),
                )
                .into_any_element(),
        )
    }
}

/// Typing in the bar: count again and go to the nearest match.
pub fn query_changed(this: &mut Dent, cx: &mut Context<Dent>) {
    this.refresh_find(false, cx);
    cx.notify();
}

#[cfg(test)]
mod tests {
    use super::item_words;
    use crate::data::Item;

    fn words(item: &Item, open: bool) -> Vec<String> {
        let mut md = crate::mdstream::Cache::default();
        item_words(item, open, |src| md.get(src))
    }

    /// Find counts what's drawn: a reply's words without its Markdown, and
    /// a long message only as far as it's shown.
    #[test]
    fn counts_what_is_drawn() {
        let reply =
            Item::Text("Use **`cargo`** and [the docs](https://x.y).\n\n```sh\nls\n```".into());
        let w = words(&reply, false).join("\n");
        assert!(!w.contains("**") && !w.contains("https://x.y") && !w.contains("```"));
        assert!(w.contains("cargo") && w.contains("the docs") && w.contains("ls"));
        let long = format!("start {} needle", "word ".repeat(200));
        let user = Item::User {
            id: "u".into(),
            text: format!("› Attached · a.png\n{long}"),
            at: 0,
        };
        let n = |open| {
            words(&user, open)
                .iter()
                .map(|w| crate::select::matches(w, "needle").len())
                .sum::<usize>()
        };
        assert_eq!(n(false), 0, "folded away");
        assert_eq!(n(true), 1);
        assert!(!words(&user, true)[0].contains("Attached"));
    }

    /// A long table scrolls inside the chat and draws only what's in view:
    /// its rows past those shown at first aren't counted.
    #[test]
    fn a_long_table_counts_the_rows_it_shows() {
        let mut src = String::from("| n | word |\n|---|---|\n");
        for i in 0..30 {
            let w = if i == 2 || i == 25 { "foo" } else { "bar" };
            src.push_str(&format!("| {i} | {w} |\n"));
        }
        src.push_str("\nAnd foo after it.");
        let n: usize = words(&Item::Text(src), false)
            .iter()
            .map(|w| crate::select::matches(w, "foo").len())
            .sum();
        assert_eq!(n, 2, "row 2 and the paragraph; row 25 is out of view");
        // A short table counts every row.
        let short = "| a |\n|---|\n| foo |\n| foo |";
        let n: usize = words(&Item::Text(short.into()), false)
            .iter()
            .map(|w| crate::select::matches(w, "foo").len())
            .sum();
        assert_eq!(n, 2);
    }
}
