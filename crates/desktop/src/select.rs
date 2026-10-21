//! Text you can select, and the one selection dent keeps.
//!
//! dent owns its selection rather than leaving it to the text on screen.
//! Every piece of selectable text has a [`Key`]: the area it belongs to (a
//! session's chat, a document, a diff) and its place in that area's reading
//! order (message, then piece within it). The selection is two spots by key,
//! so it holds however the chat scrolls: a drag can run from the third
//! message to the thirtieth, past the edge with the chat scrolling under
//! it, and ⌘C copies every message between, drawn or not.
//!
//! A drag stays in the area it began in, so the chat and the viewer never
//! select into each other. Double-click takes a word, triple-click a piece
//! (a paragraph, a line of code), Shift-click extends.
//!
//! Links stay clickable: a press and release on the same link opens it, a
//! drag that starts on one selects instead.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;

/// A region that selects on its own: a session's chat, a document.
pub type Area = u64;

/// The area named `name`, like `chat/<session>` or `doc/<path>`.
pub fn area(name: &str) -> Area {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish()
}

/// Where a piece of text sits: its area, then its place in reading order.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Key {
    area: Area,
    path: Vec<u32>,
}

/// A place in the text: a piece and a byte offset in it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Spot {
    key: Key,
    at: usize,
}

#[derive(Clone)]
struct Sel {
    anchor: Spot,
    head: Spot,
}

impl Sel {
    fn bounds(&self) -> (&Spot, &Spot) {
        if self.anchor <= self.head {
            (&self.anchor, &self.head)
        } else {
            (&self.head, &self.anchor)
        }
    }

    fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The part of the piece `key` (of `len` bytes) that's selected.
    fn range_in(&self, key: &Key, len: usize) -> Option<Range<usize>> {
        let (start, end) = self.bounds();
        if *key < start.key || *key > end.key {
            return None;
        }
        let s = if *key == start.key { start.at } else { 0 };
        let e = if *key == end.key { end.at } else { len };
        (s < e).then(|| s.min(len)..e.min(len))
    }
}

/// What a piece copies as.
#[derive(Clone, Default)]
struct Piece {
    text: SharedString,
    lead: SharedString,
    swaps: Rc<Vec<(Range<usize>, String)>>,
}

impl Piece {
    fn copy(&self, range: Range<usize>) -> String {
        // The offsets were taken from the text as it was when selected; if
        // it has changed since, they may fall inside a character.
        let text = self.text.as_str();
        let range = text.floor_char_boundary(range.start)..text.floor_char_boundary(range.end);
        if range.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        if range.start == 0 {
            out.push_str(&self.lead);
        }
        let mut at = range.start;
        for (r, with) in self.swaps.iter() {
            if r.end <= at || r.start >= range.end {
                continue;
            }
            let upto = text.floor_char_boundary(r.start.max(at));
            out.push_str(&text[at.min(upto)..upto]);
            out.push_str(with);
            at = text.floor_char_boundary(r.end.min(range.end).max(at));
        }
        out.push_str(&text[at.min(range.end)..range.end]);
        out
    }
}

/// A piece as laid out this frame: where it is, for pointing at it.
struct Placed {
    key: Key,
    win: WindowId,
    bounds: Bounds<Pixels>,
    layout: TextLayout,
    hitbox: Hitbox,
    links: Rc<Vec<(Range<usize>, String)>>,
}

/// How to scroll an area: `scroll(dy)` moves its content up by `dy`.
type Scroll = Rc<dyn Fn(f32)>;

/// An area as laid out this frame, and how to scroll it.
struct Region {
    area: Area,
    win: WindowId,
    bounds: Bounds<Pixels>,
    hitbox: Hitbox,
    scroll: Scroll,
}

struct Drag {
    area: Area,
    win: WindowId,
    down: Point<Pixels>,
    at: Point<Pixels>,
    /// The link the press began on, opened if it's let go on it unmoved.
    link: Option<(Key, usize)>,
    moved: bool,
    ticking: bool,
}

/// Where new pieces are numbered: an area, a path in it, the next number.
struct Here {
    area: Area,
    path: Vec<u32>,
    next: u32,
    /// The scope of a whole item, whose piece count is kept.
    item: bool,
}

#[derive(Default)]
struct State {
    sel: Option<Sel>,
    drag: Option<Drag>,
    placed: Vec<Placed>,
    regions: Vec<Region>,
    /// What every piece ever laid out copies as, by area and path: a copy
    /// reaches messages scrolled out of sight.
    pieces: HashMap<Area, BTreeMap<Vec<u32>, Piece>>,
    /// How many pieces each item had when last laid out; any past that are
    /// left from an older version of it.
    counts: HashMap<(Area, u32), u32>,
    /// Which places in each item hold rows made later (see [`slot`]); rows
    /// under any other place are left from an older version of it.
    slots: HashMap<(Area, u32), Vec<u32>>,
    here: Vec<Here>,
    /// For areas drawn as a list: how many items, and each one's text, for
    /// items never drawn (a long chat never scrolled through).
    sources: HashMap<Area, Source>,
    /// A control took this press: it doesn't start a selection.
    claimed: bool,
    /// The keyboard's editing keys (⌘A, ⌘C) have gone to a text field
    /// since the selection was made: by a press elsewhere, the field
    /// taking focus, or typing in it.
    yielded: bool,
    /// The area last pressed or scrolled in: what ⌘F searches.
    last: Option<Area>,
    /// What ⌘F looks for, and how many matches each item has painted so
    /// far this frame, so the current one can be told apart.
    find: Option<Find>,
    found: HashMap<u32, usize>,
}

/// ⌘F in one area: every match of `needle` is tinted, and the `n`th match
/// in item `current.0` stands out.
#[derive(Clone)]
pub struct Find {
    pub area: Area,
    pub needle: String,
    pub current: Option<(u32, usize)>,
    pub tint: Hsla,
    pub strong: Hsla,
    /// The current match's 1 px edge, so it stands out by shape too.
    pub edge: Hsla,
    /// How many matches each item may tint, by item, when they were counted
    /// from what the item draws rather than from what was laid out (a long
    /// chat): what isn't counted isn't tinted.
    pub limits: Option<std::rc::Rc<Vec<usize>>>,
}

/// About a line of text, to keep a match clear of the edge.
const LINE: f32 = 24.;

/// Show (or with `None`, stop showing) what ⌘F found.
pub fn set_find(find: Option<Find>) {
    with(|s| s.find = find.filter(|f| !f.needle.is_empty()));
}

/// Where `needle` occurs in `hay`, ignoring case, as byte ranges in `hay`.
pub fn matches(hay: &str, needle: &str) -> Vec<Range<usize>> {
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = hay.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let mut j = 0;
        let mut k = i;
        // Compare a char at a time, lowercased; a char that lowercases to
        // several (rare) simply doesn't match across them.
        while j < needle.len() && k < chars.len() {
            let mut low = chars[k].1.to_lowercase();
            if low.len() != 1 || low.next() != Some(needle[j]) {
                break;
            }
            j += 1;
            k += 1;
        }
        if j == needle.len() {
            let end = chars.get(k).map_or(hay.len(), |c| c.0);
            out.push(chars[i].0..end);
            i = k;
        } else {
            i += 1;
        }
    }
    out
}

type Source = (usize, Rc<dyn Fn(usize) -> Option<String>>);

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// What the next text made copies with before it, set by the block
    /// laying it out (a list marker, a quote's `> `).
    static LEAD: RefCell<Option<SharedString>> = const { RefCell::new(None) };
}

/// Where the link under `p` points, if one is.
pub fn link_at(p: Point<Pixels>, window: &Window) -> Option<String> {
    let win = window.window_handle().window_id();
    let (spot, link) = hit(p, win, window)?;
    let ix = link?;
    with(|s| {
        s.placed
            .iter()
            .rev()
            .find(|x| x.key == spot.key)
            .and_then(|x| x.links.get(ix).map(|(_, u)| u.clone()))
    })
}

/// The GitHub link the pointer rests on: where it points, where it's drawn (the
/// line of it under the pointer), and since when.
#[derive(Clone, Debug)]
pub struct Hovered {
    pub url: String,
    pub at: Bounds<Pixels>,
    pub since: std::time::Instant,
}

thread_local! {
    static HOVERED: RefCell<Option<Hovered>> = const { RefCell::new(None) };
}

/// The link under the pointer now, if it rests on one.
pub fn hovered_link() -> Option<Hovered> {
    HOVERED.with(|h| h.borrow().clone())
}

/// Forget the link under the pointer (a scroll moves it, a press acts on
/// it), redrawing if there was one.
fn unhover(window: &mut Window) {
    if HOVERED.with(|h| h.borrow_mut().take()).is_some() {
        window.refresh();
    }
}

/// Forget the link under the pointer if this frame no longer draws it
/// there: the session changed under a resting pointer, or it scrolled
/// away. Called while painting what the hover shows, after the text.
pub fn check_hover(window: &mut Window) {
    let Some(h) = hovered_link() else { return };
    let win = window.window_handle().window_id();
    let p = h.at.center();
    let still = with(|s| {
        s.placed.iter().any(|x| {
            x.win == win
                && x.bounds.contains(&p)
                && x.layout
                    .index_for_position(p)
                    .ok()
                    .is_some_and(|at| x.links.iter().any(|(r, u)| r.contains(&at) && *u == h.url))
        })
    });
    if !still {
        unhover(window);
    }
}

/// Note the link under `p`, redrawing when it changes.
fn hover(p: Point<Pixels>, window: &mut Window) {
    let win = window.window_handle().window_id();
    let now = with(|s| {
        let placed = s
            .placed
            .iter()
            .rev()
            .find(|x| x.win == win && x.bounds.contains(&p) && x.hitbox.is_hovered(window))?;
        // Only right over its letters, not the nearest to the pointer.
        let at = placed.layout.index_for_position(p).ok()?;
        // Only GitHub's issues and pull requests show anything on a rest;
        // other links aren't followed, so the window isn't redrawn for them.
        let (r, url) = placed
            .links
            .iter()
            .find(|(r, u)| r.contains(&at) && crate::github::issue(u).is_some())?;
        let line = placed.layout.line_height();
        let here = placed.layout.position_for_index(at)?;
        let start = placed.layout.position_for_index(r.start)?;
        let end = placed.layout.position_for_index(r.end).unwrap_or(here);
        // One line: the whole link. Wrapped: the part of the line under
        // the pointer.
        let at = if start.y == end.y {
            Bounds::from_corners(start, point(end.x, start.y + line))
        } else {
            Bounds::new(point(p.x, here.y), size(px(1.), line))
        };
        Some((url.clone(), at))
    });
    let changed = HOVERED.with(|h| {
        let mut h = h.borrow_mut();
        match (&*h, now) {
            (Some(o), Some((url, at))) if o.url == url && o.at == at => false,
            (None, None) => false,
            (_, now) => {
                *h = now.map(|(url, at)| Hovered {
                    url,
                    at,
                    since: std::time::Instant::now(),
                });
                true
            }
        }
    });
    if changed {
        window.refresh();
    }
}

/// Where the first link drawn now to an address starting `prefix` is, for
/// walkthroughs to rest the pointer on.
pub fn link_bounds(prefix: &str) -> Option<Bounds<Pixels>> {
    with(|s| {
        s.placed.iter().find_map(|p| {
            let (r, _) = p.links.iter().find(|(_, u)| u.starts_with(prefix))?;
            let a = p.layout.position_for_index(r.start)?;
            let b = p.layout.position_for_index(r.end)?;
            let line = p.layout.line_height();
            Some(Bounds::from_corners(
                a,
                point(b.x.max(a.x + px(4.)), a.y + line),
            ))
        })
    })
}

/// Where every link drawn now points, for walkthroughs.
pub fn drawn_links() -> Vec<String> {
    with(|s| {
        s.placed
            .iter()
            .flat_map(|p| p.links.iter().map(|(_, u)| u.clone()))
            .collect()
    })
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// Pieces of text copy one per line. A lead goes before a piece when the
/// selection takes in its start: `"\n"` (the default) leaves a blank line
/// above it, as between paragraphs; `""` starts it on the next line, as
/// between list items or lines of code; one starting with [`SAME_LINE`]
/// carries on the line before, as between table cells.
pub const SAME_LINE: char = '\u{1}';

/// Sets the lead of the next selectable text made.
pub fn lead_next(lead: impl Into<SharedString>) {
    LEAD.with(|l| *l.borrow_mut() = Some(lead.into()));
}

/// Clears a lead nothing took, so it can't reach a later block.
pub fn drop_lead() {
    LEAD.with(|l| l.borrow_mut().take());
}

/// The lead set for the next text, or a blank line.
pub fn take_lead() -> SharedString {
    LEAD.with(|l| l.borrow_mut().take())
        .unwrap_or_else(|| SharedString::new_static("\n"))
}

// ---------------------------------------------------------------------------
// Numbering: where each piece sits in its area's reading order.

/// Text made while this lives is item `ix` of `area`, numbered in the order
/// it's made. Make each item of a list inside one, so every piece keeps its
/// place whether or not the items around it are drawn.
#[must_use]
pub struct Scope(());

impl Drop for Scope {
    fn drop(&mut self) {
        with(|s| {
            if let Some(h) = s.here.pop()
                && h.item
                && let Some(&ix) = h.path.first()
            {
                s.counts.insert((h.area, ix), h.next);
            }
        });
    }
}

pub fn item(area: Area, ix: usize) -> Scope {
    with(|s| {
        s.slots.remove(&(area, ix as u32));
        s.here.push(Here {
            area,
            path: vec![ix as u32],
            next: 0,
            item: true,
        })
    });
    Scope(())
}

/// The chat item and area text made now belongs to (at layout).
pub fn here_item() -> Option<(Area, u32)> {
    with(|s| {
        let h = s.here.last()?;
        Some((h.area, *h.path.first()?))
    })
}

/// What ⌘F looks for in `area`, while it looks there.
pub fn find_needle(area: Area) -> Option<String> {
    with(|s| {
        s.find
            .as_ref()
            .filter(|f| f.area == area)
            .map(|f| f.needle.clone())
    })
}

/// The current ⌘F match in `area`: its item, and which match in it.
pub fn find_current(area: Area) -> Option<(u32, usize)> {
    with(|s| s.find.as_ref().filter(|f| f.area == area)?.current)
}

/// How many matches `item` has painted so far this frame, and setting it:
/// a long table's rows out of view count as if painted, so numbering stays
/// true (see `markdown::table_block`).
pub fn found_so_far(item: u32) -> usize {
    with(|s| s.found.get(&item).copied().unwrap_or(0))
}

pub fn set_found(item: u32, n: usize) {
    with(|s| {
        s.found.insert(item, n);
    })
}

thread_local! {
    /// ⌘F stepped to a match: a long table holding it out of view scrolls
    /// to it, once.
    static REVEAL_ROW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Ask a long table holding the current match out of view to bring it in.
pub fn reveal_row() {
    REVEAL_ROW.with(|r| r.set(true));
}

/// Take that request (a table holding the match does, once).
pub fn take_reveal_row() -> bool {
    REVEAL_ROW.with(|r| r.replace(false))
}

/// Runs `f` as it's painted, in paint order among the text around it;
/// takes no room.
pub fn at_paint(f: impl Fn(&mut Window) + 'static) -> AtPaint {
    AtPaint(Box::new(f))
}

pub struct AtPaint(Box<dyn Fn(&mut Window)>);

impl IntoElement for AtPaint {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for AtPaint {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (window.request_layout(Style::default(), [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        (self.0)(window)
    }
}

/// A place kept in the reading order for text made later: a list inside an
/// item draws its rows after the item is made.
#[derive(Clone)]
pub struct Slot(Option<(Area, Vec<u32>)>);

/// Keeps the next place in the current item for text made later.
pub fn slot() -> Slot {
    Slot(with(|s| {
        let h = s.here.last_mut()?;
        let mut path = h.path.clone();
        path.push(h.next);
        h.next += 1;
        let area = h.area;
        if let [ix, n] = path[..] {
            s.slots.entry((area, ix)).or_default().push(n);
        }
        Some((area, path))
    }))
}

impl Slot {
    /// Text made while this lives is row `ix` of the slot.
    pub fn enter(&self, ix: usize) -> Option<Scope> {
        let (area, path) = self.0.clone()?;
        with(|s| {
            let mut path = path;
            path.push(ix as u32);
            s.here.push(Here {
                area,
                path,
                next: 0,
                item: false,
            })
        });
        Some(Scope(()))
    }
}

/// The key for the next piece made. Text made outside any item is an area
/// of its own, named by its id.
fn next_key(id: &ElementId) -> Key {
    with(|s| match s.here.last_mut() {
        Some(h) => {
            let mut path = h.path.clone();
            path.push(h.next);
            h.next += 1;
            Key { area: h.area, path }
        }
        None => Key {
            area: area(&format!("loose/{id}")),
            path: vec![0, 0],
        },
    })
}

// ---------------------------------------------------------------------------
// The layer: pointing, dragging, and the highlight.

/// Mounted first in each window's root: starts each frame's layout afresh
/// and takes the presses, drags and releases that select.
pub fn layer() -> impl IntoElement {
    canvas(
        |_, window, _| {
            let win = window.window_handle().window_id();
            with(|s| {
                s.placed.retain(|p| p.win != win);
                s.regions.retain(|r| r.win != win);
                s.found.clear();
            });
        },
        |_, _, window, _| {
            // Registered first, these run last as the event bubbles: a
            // control under the pointer has had its say.
            window.on_mouse_event(|e: &ScrollWheelEvent, phase, window, _| {
                if phase == DispatchPhase::Capture {
                    unhover(window);
                    let win = window.window_handle().window_id();
                    if let Some(area) = region_at(e.position, win, window) {
                        with(|s| s.last = Some(area));
                    }
                }
            });
            window.on_mouse_event(|e: &MouseDownEvent, phase, window, cx| {
                // Every press yields the keys first; one that selects takes
                // them back. A field or control that keeps the press to
                // itself has still had the last word.
                match (e.button, phase) {
                    (MouseButton::Left, DispatchPhase::Capture) => {
                        unhover(window);
                        yield_keys();
                        let win = window.window_handle().window_id();
                        let area = region_at(e.position, win, window);
                        with(|s| s.last = area);
                    }
                    (MouseButton::Left, DispatchPhase::Bubble) => press(e, window, cx),
                    _ => {}
                }
            });
            window.on_mouse_event(|e: &MouseMoveEvent, phase, window, cx| {
                if phase.bubble() {
                    if e.pressed_button.is_none() {
                        hover(e.position, window);
                    } else {
                        unhover(window);
                    }
                    drag_to(
                        e.position,
                        e.pressed_button == Some(MouseButton::Left),
                        window,
                        cx,
                    );
                }
            });
            // Leaving the window leaves any link.
            window.on_mouse_event(|_: &MouseExitEvent, _, window, _| unhover(window));
            window.on_mouse_event(|e: &MouseUpEvent, phase, window, cx| {
                if phase.bubble() && e.button == MouseButton::Left {
                    release(e.position, window, cx);
                }
            });
        },
    )
    .absolute()
    .size_0()
}

/// Lays out the bounds of `area` this frame, and how to scroll it while a
/// drag runs past its edge: `scroll(dy)` moves the content up by `dy`.
pub fn region(area: Area, scroll: impl Fn(f32) + 'static) -> impl IntoElement {
    let scroll: Rc<dyn Fn(f32)> = Rc::new(scroll);
    canvas(
        move |bounds, window, _| {
            let win = window.window_handle().window_id();
            let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
            with(|s| {
                s.regions.push(Region {
                    area,
                    win,
                    bounds,
                    hitbox,
                    scroll: scroll.clone(),
                })
            });
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// `area` is a list of `len` items, and `text(ix)` is what item `ix`
/// copies as when it was never drawn.
pub fn source(area: Area, len: usize, text: impl Fn(usize) -> Option<String> + 'static) {
    with(|s| s.sources.insert(area, (len, Rc::new(text))));
}

/// Called by a control as it's pressed, so the press doesn't also select
/// the text under it.
pub fn claim() {
    with(|s| s.claimed = true);
}

/// The piece under `p`, and the offset in it nearest `p`.
fn hit(p: Point<Pixels>, win: WindowId, window: &Window) -> Option<(Spot, Option<usize>)> {
    with(|s| {
        // Last laid out is on top.
        let placed = s
            .placed
            .iter()
            .rev()
            .find(|x| x.win == win && x.bounds.contains(&p) && x.hitbox.is_hovered(window))?;
        let at = placed
            .layout
            .index_for_position(p)
            .unwrap_or_else(|closest| closest);
        let link = placed.links.iter().position(|(r, _)| r.contains(&at));
        Some((
            Spot {
                key: placed.key.clone(),
                at,
            },
            link,
        ))
    })
}

/// The area whose bounds hold `p`.
fn region_at(p: Point<Pixels>, win: WindowId, window: &Window) -> Option<Area> {
    with(|s| {
        s.regions
            .iter()
            .rev()
            // Not under a menu or anything else that covers it.
            .find(|r| r.win == win && r.bounds.contains(&p) && r.hitbox.is_hovered(window))
            .map(|r| r.area)
    })
}

/// The spot in `area` nearest `p`: in the piece under it; else, of the
/// pieces whose lines span its height, the nearest across; else the start
/// of the next piece down or the end of the last one up. Pieces of the
/// area drawn this frame are all it looks at; a point past the area's edge
/// is first brought inside it.
fn nearest(area: Area, p: Point<Pixels>, win: WindowId) -> Option<Spot> {
    with(|s| {
        let p = match s.regions.iter().find(|r| r.area == area && r.win == win) {
            Some(r) => point(
                p.x.clamp(r.bounds.left(), r.bounds.right() - px(1.)),
                p.y.clamp(r.bounds.top() + px(1.), r.bounds.bottom() - px(1.)),
            ),
            None => p,
        };
        let mine = || s.placed.iter().filter(|x| x.area_is(area, win));
        let at =
            |x: &Placed, q: Point<Pixels>| x.layout.index_for_position(q).unwrap_or_else(|c| c);
        // Pieces across whose height `p` falls: the nearest across.
        let level = mine()
            .filter(|x| x.bounds.top() <= p.y && p.y < x.bounds.bottom())
            .min_by(|a, b| {
                let d = |x: &Placed| {
                    if p.x < x.bounds.left() {
                        x.bounds.left() - p.x
                    } else if p.x > x.bounds.right() {
                        p.x - x.bounds.right()
                    } else {
                        px(0.)
                    }
                };
                d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some(x) = level {
            let q = point(p.x.clamp(x.bounds.left(), x.bounds.right()), p.y);
            return Some(Spot {
                key: x.key.clone(),
                at: at(x, q),
            });
        }
        // Between pieces: the start of the next one down, in reading order.
        let below = mine()
            .filter(|x| x.bounds.top() > p.y)
            .min_by(|a, b| a.key.cmp(&b.key));
        if let Some(x) = below {
            return Some(Spot {
                key: x.key.clone(),
                at: 0,
            });
        }
        let above = mine()
            .filter(|x| x.bounds.bottom() <= p.y)
            .max_by(|a, b| a.key.cmp(&b.key))?;
        Some(Spot {
            key: above.key.clone(),
            at: above.layout.len(),
        })
    })
}

impl Placed {
    fn area_is(&self, area: Area, win: WindowId) -> bool {
        self.key.area == area && self.win == win
    }
}

fn changed(window: &mut Window) {
    window.refresh();
}

fn press(e: &MouseDownEvent, window: &mut Window, _cx: &mut App) {
    let win = window.window_handle().window_id();
    let claimed = with(|s| std::mem::take(&mut s.claimed));
    let target = hit(e.position, win, window);
    // Shift extends what's selected, within its area.
    if e.modifiers.shift && !claimed {
        let area = with(|s| s.sel.as_ref().map(|sel| sel.anchor.key.area));
        if let Some(area) = area
            && let Some(head) = target
                .clone()
                .map(|(spot, _)| spot)
                .filter(|spot| spot.key.area == area)
                .or_else(|| nearest(area, e.position, win))
        {
            with(|s| {
                if let Some(sel) = s.sel.as_mut() {
                    sel.head = head;
                }
                s.yielded = false;
                s.drag = Some(Drag {
                    area,
                    win,
                    down: e.position,
                    at: e.position,
                    link: None,
                    moved: true,
                    ticking: false,
                });
            });
            changed(window);
            return;
        }
    }
    let had = with(|s| s.sel.take().is_some() | s.drag.take().is_some());
    if had {
        changed(window);
    }
    if claimed {
        return;
    }
    // On text, or in an area's margin: there, the nearest text.
    let (spot, link) = match target {
        Some(t) => t,
        None => {
            let Some(area) = region_at(e.position, win, window) else {
                return;
            };
            let Some(spot) = nearest(area, e.position, win) else {
                return;
            };
            (spot, None)
        }
    };
    let area = spot.key.area;
    let sel = match e.click_count {
        1 => Sel {
            anchor: spot.clone(),
            head: spot.clone(),
        },
        n => {
            let range = piece_text(&spot.key)
                .map(|text| {
                    if n == 2 {
                        word_at(&text, spot.at)
                    } else {
                        0..text.len()
                    }
                })
                .unwrap_or(spot.at..spot.at);
            Sel {
                anchor: Spot {
                    key: spot.key.clone(),
                    at: range.start,
                },
                head: Spot {
                    key: spot.key.clone(),
                    at: range.end,
                },
            }
        }
    };
    with(|s| {
        s.sel = Some(sel);
        s.yielded = false;
        s.drag = Some(Drag {
            area,
            win,
            down: e.position,
            at: e.position,
            link: link
                .filter(|_| e.click_count == 1)
                .map(|l| (spot.key.clone(), l)),
            moved: e.click_count > 1,
            ticking: false,
        });
    });
    changed(window);
}

/// The text of a piece, as last laid out.
fn piece_text(key: &Key) -> Option<SharedString> {
    with(|s| {
        s.pieces
            .get(&key.area)
            .and_then(|m| m.get(&key.path))
            .map(|p| p.text.clone())
    })
}

/// The word around `at`: a run of letters and digits, or of anything else
/// that isn't space; a space alone if that's what's there.
fn word_at(text: &str, at: usize) -> Range<usize> {
    let at = at.min(text.len());
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    // The character at `at`, or the one before at the end.
    let Some((i, c)) = text[at..]
        .char_indices()
        .next()
        .map(|(i, c)| (at + i, c))
        .or_else(|| text[..at].char_indices().last())
    else {
        return at..at;
    };
    let k = class(c);
    let start = text[..i]
        .char_indices()
        .rev()
        .take_while(|(_, c)| class(*c) == k)
        .last()
        .map_or(i, |(j, _)| j);
    let end = text[i..]
        .char_indices()
        .find(|(_, c)| class(*c) != k)
        .map_or(text.len(), |(j, _)| i + j);
    start..end
}

fn drag_to(p: Point<Pixels>, held: bool, window: &mut Window, cx: &mut App) {
    let Some((area, win, down, moved)) =
        with(|s| s.drag.as_ref().map(|d| (d.area, d.win, d.down, d.moved)))
    else {
        return;
    };
    if win != window.window_handle().window_id() {
        return;
    }
    // A release missed (outside the window): the drag is over.
    if !held {
        release(p, window, cx);
        return;
    }
    // A press that wobbles a pixel or two is still a click.
    if !moved && (p - down).magnitude() < 3. {
        return;
    }
    with(|s| {
        if let Some(d) = s.drag.as_mut() {
            d.at = p;
            d.moved = true;
        }
    });
    follow(area, p, win, window);
    // Past the area's top or bottom, it scrolls under the drag.
    let outside = with(|s| {
        s.regions
            .iter()
            .find(|r| r.area == area && r.win == win)
            .is_some_and(|r| p.y < r.bounds.top() || p.y >= r.bounds.bottom())
    });
    if outside {
        tick(window);
    }
}

/// Moves the head to the text nearest `p`.
fn follow(area: Area, p: Point<Pixels>, win: WindowId, window: &mut Window) {
    let head = hit(p, win, window)
        .map(|(spot, _)| spot)
        .filter(|spot| spot.key.area == area)
        .or_else(|| nearest(area, p, win));
    let Some(head) = head else {
        return;
    };
    let moved = with(|s| match s.sel.as_mut() {
        Some(sel) if sel.head != head => {
            sel.head = head;
            true
        }
        _ => false,
    });
    if moved {
        changed(window);
    }
}

/// Each frame while the pointer is past the area's edge, scroll toward it,
/// faster the further out it is, and carry the selection along.
fn tick(window: &mut Window) {
    let start = with(|s| match s.drag.as_mut() {
        Some(d) if !d.ticking => {
            d.ticking = true;
            true
        }
        _ => false,
    });
    if !start {
        return;
    }
    fn step(window: &mut Window) {
        window.on_next_frame(|window, _| {
            let go = with(|s| {
                let d = s.drag.as_ref()?;
                let r = s
                    .regions
                    .iter()
                    .find(|r| r.area == d.area && r.win == d.win)?;
                let over = if d.at.y < r.bounds.top() {
                    (d.at.y - r.bounds.top()).as_f32()
                } else if d.at.y >= r.bounds.bottom() {
                    (d.at.y - r.bounds.bottom()).as_f32() + 1.
                } else {
                    return None;
                };
                let speed = (over.abs() * 0.35 + 3.).min(48.) * over.signum();
                Some((r.scroll.clone(), speed, d.area, d.at, d.win))
            });
            match go {
                Some((scroll, speed, area, at, win)) => {
                    scroll(speed);
                    follow(area, at, win, window);
                    window.refresh();
                    step(window);
                }
                None => with(|s| {
                    if let Some(d) = s.drag.as_mut() {
                        d.ticking = false;
                    }
                }),
            }
        });
    }
    step(window);
}

fn release(p: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let Some(drag) = with(|s| s.drag.take()) else {
        return;
    };
    // An empty selection stays, as a caret: Shift-click extends from it,
    // and ⌘A selects the area it's in.
    let empty = with(|s| s.sel.as_ref().is_none_or(Sel::is_empty));
    if empty
        && !drag.moved
        && let Some((key, ix)) = drag.link
        && let Some((spot, Some(now))) = hit(p, drag.win, window)
        && spot.key == key
        && now == ix
    {
        let url = with(|s| {
            s.placed
                .iter()
                .find(|x| x.key == key)
                .and_then(|x| x.links.get(ix).map(|(_, u)| u.clone()))
        });
        if let Some(url) = url {
            open_link(&url, window, cx);
        }
    }
    changed(window);
}

// ---------------------------------------------------------------------------
// Asking about and changing the selection.

pub fn has_selection() -> bool {
    with(|s| s.sel.as_ref().is_some_and(|sel| !sel.is_empty()))
}

/// Clears the selection; whether there was one.
pub fn clear(window: &mut Window) -> bool {
    let had = with(|s| {
        s.drag = None;
        s.sel.take().is_some_and(|sel| !sel.is_empty())
    });
    if had {
        changed(window);
    }
    had
}

/// Selects all of `area`, first item to last, drawn or not.
pub fn select_all(area: Area, window: &mut Window) -> bool {
    let ends = with(|s| {
        let last = match s.sources.get(&area) {
            Some((len, _)) => (*len as u32).checked_sub(1)?,
            None => {
                let pieces = s.pieces.get(&area)?;
                *pieces.keys().next_back()?.first()?
            }
        };
        // From before the first item's first piece to past the last item.
        let key = |path| Key { area, path };
        Some((
            Spot {
                key: key(vec![0]),
                at: 0,
            },
            Spot {
                key: key(vec![last + 1]),
                at: 0,
            },
        ))
    });
    let Some((anchor, head)) = ends else {
        return false;
    };
    with(|s| {
        s.sel = Some(Sel { anchor, head });
        s.yielded = false;
    });
    changed(window);
    true
}

/// A text field has the keyboard's editing keys now: ⌘A and ⌘C are its
/// own, not the selection's, though the selection stays on screen.
pub fn yield_keys() {
    with(|s| s.yielded = true);
}

/// Whether the selection was the last thing pressed or made, so ⌘A and
/// ⌘C belong to it even while a text field keeps the keyboard.
pub fn holds_keys() -> bool {
    with(|s| s.sel.is_some() && !s.yielded)
}

/// The area last pressed or scrolled in, if it was in one.
pub fn last_area() -> Option<Area> {
    with(|s| s.last)
}

/// The pieces of `area` as last laid out, in reading order, with what
/// each says.
fn texts(s: &State, area: Area) -> Vec<(Vec<u32>, SharedString)> {
    s.pieces
        .get(&area)
        .into_iter()
        .flatten()
        .filter(|(path, _)| live(s, area, path))
        .map(|(path, p)| (path.clone(), p.text.clone()))
        .collect()
}

/// How many times `needle` is in each item of `area` that has any, in
/// reading order: what ⌘F counts in an area drawn whole (a document).
pub fn counts(area: Area, needle: &str) -> Vec<(u32, usize)> {
    let mut out: Vec<(u32, usize)> = Vec::new();
    with(|s| {
        for (path, text) in texts(s, area) {
            let Some(&item) = path.first() else { continue };
            let n = matches(&text, needle).len();
            if n == 0 {
                continue;
            }
            match out.last_mut() {
                Some((i, c)) if *i == item => *c += n,
                _ => out.push((item, n)),
            }
        }
    });
    out
}

/// Where the `k`th match of `needle` in `item` of `area` was laid out (its
/// top), the bounds of what's on screen there, and how to scroll it.
fn locate(
    area: Area,
    item: u32,
    k: usize,
    needle: &str,
    window: &Window,
) -> Option<(Pixels, Bounds<Pixels>, Scroll)> {
    let win = window.window_handle().window_id();
    with(|s| {
        let mut seen = 0;
        for (path, text) in texts(s, area) {
            if path.first() != Some(&item) {
                continue;
            }
            let hits = matches(&text, needle);
            if k < seen + hits.len() {
                let at = hits[k - seen].start;
                let placed = s
                    .placed
                    .iter()
                    .find(|x| x.win == win && x.key.area == area && x.key.path == path)?;
                let y = placed
                    .layout
                    .position_for_index(at)
                    .map_or(placed.bounds.top(), |p| p.y);
                let r = s.regions.iter().find(|r| r.area == area && r.win == win)?;
                return Some((y, r.bounds, r.scroll.clone()));
            }
            seen += hits.len();
        }
        None
    })
}

/// Whether the `k`th match of `needle` in `item` is on screen.
pub fn in_view(area: Area, item: u32, k: usize, needle: &str, window: &Window) -> bool {
    locate(area, item, k, needle, window)
        .is_some_and(|(y, view, _)| y >= view.top() && y + px(LINE) <= view.bottom())
}

/// Scroll `area` so the `k`th match of `needle` in `item` is in view, a
/// third of the way down, if it's out of it. Returns whether it was found.
pub fn reveal(area: Area, item: u32, k: usize, needle: &str, window: &Window) -> bool {
    let Some((y, view, scroll)) = locate(area, item, k, needle, window) else {
        return false;
    };
    if y < view.top() || y + px(LINE) > view.bottom() {
        scroll((y - (view.top() + view.size.height / 3.)).as_f32());
    }
    true
}

/// The area last pressed in, or selected in, if the last press was in one.
pub fn selected_area() -> Option<Area> {
    with(|s| s.sel.as_ref().map(|sel| sel.anchor.key.area))
}

/// Whether a piece at `path` is part of its item as last laid out, not
/// left from an older version of it.
fn live(s: &State, area: Area, path: &[u32]) -> bool {
    match (path.first(), path.get(1)) {
        (Some(ix), Some(n)) => {
            s.counts.get(&(area, *ix)).is_none_or(|c| n < c)
                && (path.len() <= 2 || s.slots.get(&(area, *ix)).is_some_and(|v| v.contains(n)))
        }
        _ => true,
    }
}

/// The selection as text, shaped like what was selected: paragraphs apart,
/// list items with their markers, cells side by side, maths as TeX. Messages
/// scrolled out of sight copy too.
pub fn copied() -> String {
    let joined = with(|s| {
        let sel = s.sel.as_ref().filter(|sel| !sel.is_empty())?;
        let (start, end) = sel.bounds();
        let area = start.key.area;
        let mut out: Vec<(Vec<u32>, String)> = Vec::new();
        if let Some(pieces) = s.pieces.get(&area) {
            for (path, piece) in pieces.range(start.key.path.clone()..=end.key.path.clone()) {
                if !live(s, area, path) {
                    continue;
                }
                let key = Key {
                    area,
                    path: path.clone(),
                };
                if let Some(r) = sel.range_in(&key, piece.text.len()) {
                    out.push((path.clone(), piece.copy(r)));
                }
            }
        }
        // Items wholly inside that were never drawn copy from their source;
        // so do those holding a list of their own (a long table), whose
        // rows out of sight were never drawn either.
        if let Some((len, text)) = s.sources.get(&area) {
            let first = start.key.path.first().copied().unwrap_or(0);
            let last = end.key.path.first().copied().unwrap_or(0);
            for ix in first..=last.min((*len as u32).saturating_sub(1)) {
                let inside = Key {
                    area,
                    path: vec![ix],
                } >= start.key
                    && Key {
                        area,
                        path: vec![ix + 1],
                    } <= end.key;
                let partly_drawn = !s.counts.contains_key(&(area, ix))
                    || s.slots.get(&(area, ix)).is_some_and(|v| !v.is_empty());
                if inside
                    && partly_drawn
                    && let Some(t) = text(ix as usize)
                {
                    out.retain(|(p, _)| p.first() != Some(&ix));
                    out.push((vec![ix], format!("\n{t}")));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Some(
            out.into_iter()
                .map(|(_, t)| t)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    });
    joined.map(|j| shape(&j)).unwrap_or_default()
}

/// The pieces as selected, one per line, made into the copy.
fn shape(joined: &str) -> String {
    let mut out = joined.replace(&format!("\n{SAME_LINE}"), "");
    out.retain(|c| c != SAME_LINE);
    out.trim_start_matches('\n').trim_end().to_string()
}

// ---------------------------------------------------------------------------
// The text element.

pub struct Text {
    id: ElementId,
    key: Key,
    text: SharedString,
    styled: StyledText,
    links: Rc<Vec<(Range<usize>, String)>>,
    color: Hsla,
    lead: SharedString,
    swaps: Rc<Vec<(Range<usize>, String)>>,
}

/// Plain text in the surrounding style.
pub fn text(id: impl Into<ElementId>, text: impl Into<SharedString>, t: &Theme) -> Text {
    let text = text.into();
    let id = id.into();
    Text {
        key: next_key(&id),
        id,
        styled: StyledText::new(text.clone()),
        text,
        links: Rc::default(),
        color: t.selection(),
        lead: take_lead(),
        swaps: Rc::default(),
    }
}

/// Text with its own runs: highlighted code, a reply's bold and links.
pub fn styled(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    runs: Vec<TextRun>,
    t: &Theme,
) -> Text {
    let text = text.into();
    let id = id.into();
    Text {
        key: next_key(&id),
        id,
        styled: StyledText::new(text.clone()).with_runs(runs),
        text,
        links: Rc::default(),
        color: t.selection(),
        lead: take_lead(),
        swaps: Rc::default(),
    }
}

impl Text {
    /// Byte ranges that open somewhere when clicked: a URL, or a path to
    /// open in its default app.
    pub fn links(mut self, links: Vec<(Range<usize>, String)>) -> Self {
        self.links = Rc::new(links);
        self
    }

    /// Links in plain text (no runs of its own) underlined as a reply's are,
    /// in the face it inherits.
    pub fn underline_links(mut self, t: &Theme) -> Self {
        let marks = self
            .links
            .iter()
            .map(|(r, _)| {
                (
                    r.clone(),
                    HighlightStyle {
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(t.quiet.opacity(0.6)),
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        self.styled = StyledText::new(self.text.clone()).with_highlights(marks);
        self
    }

    /// What copies before this text; see [`SAME_LINE`].
    pub fn lead(mut self, lead: impl Into<SharedString>) -> Self {
        self.lead = lead.into();
        self
    }

    /// Byte ranges drawn as something else, with the text they copy as:
    /// the stand-in under a formula copies as its TeX.
    pub fn swaps(mut self, swaps: Vec<(Range<usize>, String)>) -> Self {
        self.swaps = Rc::new(swaps);
        self
    }

    pub fn layout(&self) -> TextLayout {
        self.styled.layout().clone()
    }
}

impl IntoElement for Text {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Text {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // What it copies as, kept for when it's out of sight.
        with(|s| {
            s.pieces.entry(self.key.area).or_default().insert(
                self.key.path.clone(),
                Piece {
                    text: self.text.clone(),
                    lead: self.lead.clone(),
                    swaps: self.swaps.clone(),
                },
            );
        });
        self.styled.request_layout(id, inspector, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.styled
            .prepaint(id, inspector, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let win = window.window_handle().window_id();
        with(|s| {
            s.placed.push(Placed {
                key: self.key.clone(),
                win,
                bounds,
                layout: self.styled.layout().clone(),
                hitbox: hitbox.clone(),
                links: self.links.clone(),
            })
        });
        hitbox
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled.layout().clone();
        // What ⌘F found here: every match tinted, the current one strong.
        let marks = with(|s| {
            let f = s.find.as_ref().filter(|f| f.area == self.key.area)?;
            let item = *self.key.path.first()?;
            let limit = f
                .limits
                .as_ref()
                .map_or(usize::MAX, |l| l.get(item as usize).copied().unwrap_or(0));
            let seen = s.found.entry(item).or_default();
            let marks: Vec<(Range<usize>, Hsla, Option<Hsla>)> = matches(&self.text, &f.needle)
                .into_iter()
                .take(limit.saturating_sub(*seen))
                .map(|r| {
                    let on = f.current == Some((item, *seen));
                    *seen += 1;
                    if on {
                        (r, f.strong, Some(f.edge))
                    } else {
                        (r, f.tint, None)
                    }
                })
                .collect();
            Some(marks)
        });
        for (r, color, edge) in marks.into_iter().flatten() {
            paint_range(&layout, r, color, edge, window);
        }
        let range = with(|s| {
            s.sel
                .as_ref()
                .and_then(|sel| sel.range_in(&self.key, self.text.len()))
        });
        if let Some(range) = range {
            paint_range(&layout, range, self.color, None, window);
        }
        self.styled
            .paint(id, inspector, bounds, &mut (), &mut (), window, cx);
        let over_link = hitbox.is_hovered(window)
            && !self.links.is_empty()
            && layout
                .index_for_position(window.mouse_position())
                .ok()
                .is_some_and(|ix| self.links.iter().any(|(r, _)| r.contains(&ix)));
        window.set_cursor_style(
            if over_link {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            },
            hitbox,
        );
    }
}

thread_local! {
    /// A file a link asked dent to show, for the app to open on its next draw.
    static OPEN: RefCell<Option<(String, Option<u32>)>> = const { RefCell::new(None) };
}

/// Follow a link: web addresses in the browser; a file dent can show (a
/// document, page or picture) in the viewer; any other file in its app.
pub fn open_link(url: &str, window: &mut Window, cx: &mut App) {
    // A path, `~/…` too (whose home it is, the session's machine says).
    if url.starts_with('/') || url.starts_with("~/") {
        // The window decides where it shows, and says so when it can't.
        OPEN.with(|o| *o.borrow_mut() = Some(at_line(url)));
        window.refresh();
    } else if !url.starts_with('#') {
        cx.open_url(url);
    }
}

/// A path and the line it names, from `src/app.rs:42` or `src/app.rs:42:7`
/// (the column is let go). A path that really ends that way stays whole.
pub fn at_line(path: &str) -> (String, Option<u32>) {
    if std::path::Path::new(path).exists() {
        // this Mac's, if here: a name that ends in :12
        return (path.to_string(), None);
    }
    let mut rest = path;
    let mut nums = Vec::new();
    while let Some((head, n)) = rest.rsplit_once(':')
        && nums.len() < 2
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
    {
        nums.push(n);
        rest = head;
    }
    match nums.last().and_then(|n| n.parse().ok()) {
        Some(line) => (rest.to_string(), Some(line)),
        None => (path.to_string(), None),
    }
}

/// The file a link asked to show, and the line, once.
pub fn take_open() -> Option<(String, Option<u32>)> {
    OPEN.with(|o| o.borrow_mut().take())
}

/// The highlight behind `range`: one band per line it covers.
fn paint_range(
    layout: &TextLayout,
    range: Range<usize>,
    color: Hsla,
    edge: Option<Hsla>,
    window: &mut Window,
) {
    let (Some(start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return;
    };
    let (b, lh) = (layout.bounds(), layout.line_height());
    let mut bands = Vec::with_capacity(3);
    if start.y == end.y {
        bands.push(Bounds::from_corners(start, point(end.x, end.y + lh)));
    } else {
        bands.push(Bounds::from_corners(start, point(b.right(), start.y + lh)));
        if end.y > start.y + lh {
            bands.push(Bounds::from_corners(
                point(b.left(), start.y + lh),
                point(b.right(), end.y),
            ));
        }
        bands.push(Bounds::from_corners(
            point(b.left(), end.y),
            point(end.x, end.y + lh),
        ));
    }
    for band in bands {
        let q = fill(band, color);
        window.paint_quad(match edge {
            Some(e) => q.border_widths(px(1.)).border_color(e).corner_radii(px(2.)),
            None => q,
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn copy_never_cuts_a_character_after_the_text_changed() {
        // Offsets taken from "abcdef", the text since rewritten.
        let p = super::Piece {
            text: "日本語のテキスト".into(),
            ..Default::default()
        };
        assert_eq!(p.copy(1..5), "日");
        assert_eq!(p.copy(4..5), "");
    }

    #[test]
    fn a_link_names_a_line() {
        use super::at_line;
        assert_eq!(
            at_line("/nowhere/a.rs:42"),
            ("/nowhere/a.rs".into(), Some(42))
        );
        assert_eq!(
            at_line("/nowhere/a.rs:42:7"),
            ("/nowhere/a.rs".into(), Some(42))
        );
        assert_eq!(at_line("/nowhere/a.rs"), ("/nowhere/a.rs".into(), None));
        assert_eq!(at_line("/nowhere/a.rs:"), ("/nowhere/a.rs:".into(), None));
    }

    use super::{Key, Piece, SAME_LINE, Sel, Spot, shape, word_at};
    use std::ops::Range;
    use std::rc::Rc;

    fn piece(text: &str, lead: &str, range: Range<usize>) -> String {
        Piece {
            text: text.to_string().into(),
            lead: lead.to_string().into(),
            swaps: Rc::default(),
        }
        .copy(range)
    }

    fn spot(path: &[u32], at: usize) -> Spot {
        Spot {
            key: Key {
                area: 1,
                path: path.to_vec(),
            },
            at,
        }
    }

    #[test]
    fn pieces_join_like_what_was_selected() {
        let joined = [
            piece("Intro", "\n", 0..5),
            piece("one", "\n- ", 0..3),
            piece("two", "- ", 0..3),
            piece("a", "\n", 0..1),
            piece("b", &format!("{SAME_LINE}\t"), 0..1),
            piece("c", "", 0..1),
        ]
        .join("\n");
        assert_eq!(shape(&joined), "Intro\n\n- one\n- two\n\na\tb\nc");
    }

    #[test]
    fn a_lead_only_copies_when_the_start_is_selected() {
        assert_eq!(piece("hello", "\n- ", 2..5), "llo");
        assert_eq!(piece("hello", "\n- ", 0..2), "\n- he");
        assert_eq!(piece("hello", "", 0..0), "");
    }

    #[test]
    fn stand_ins_copy_as_what_they_stand_for() {
        let c = Piece {
            text: "a mmmm b".into(),
            lead: "".into(),
            swaps: Rc::new(vec![(2..6, "$x^2$".into())]),
        };
        assert_eq!(c.copy(0..8), "a $x^2$ b");
        // Part of a formula selected copies all of it.
        assert_eq!(c.copy(4..8), "$x^2$ b");
    }

    #[test]
    fn a_selection_spans_messages_in_reading_order() {
        // Dragged upward, from message 7 back to message 2.
        let sel = Sel {
            anchor: spot(&[7, 0], 4),
            head: spot(&[2, 1], 3),
        };
        let key = |p: &[u32]| spot(p, 0).key;
        assert_eq!(sel.range_in(&key(&[2, 0]), 10), None);
        assert_eq!(sel.range_in(&key(&[2, 1]), 10), Some(3..10));
        // Everything between, whole, including rows of a list in message 4.
        assert_eq!(sel.range_in(&key(&[4, 2, 9, 0]), 6), Some(0..6));
        assert_eq!(sel.range_in(&key(&[7, 0]), 10), Some(0..4));
        assert_eq!(sel.range_in(&key(&[7, 1]), 10), None);
        assert_eq!(sel.range_in(&key(&[8, 0]), 10), None);
    }

    #[test]
    fn words_are_runs_of_letters_or_of_marks() {
        let t = "let x_1 = foo(bar);";
        assert_eq!(&t[word_at(t, 5)], "x_1");
        assert_eq!(&t[word_at(t, 13)], "(");
        assert_eq!(&t[word_at(t, 3)], " ");
        assert_eq!(&t[word_at(t, t.len())], ");");
        assert_eq!(word_at("", 0), 0..0);
    }
}
