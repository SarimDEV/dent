//! Text you can select. Every run of text that should be copyable is one of
//! these: it joins the window's single selection (the layer the root
//! mounts), so a drag can run from a reply into a code block and ⌘C copies
//! all of it. Double-click takes a word, triple-click a paragraph.
//!
//! Links stay clickable: a press and release on the same link opens it, a
//! drag that starts on one selects instead.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::{
    TextSelection, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun,
    TextSelectionScopeId,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;

/// The chat and the viewer each select on their own: a drag in one never
/// reaches into the other. The window keeps one scope active (the UI
/// library's root resets it every frame), so the area you're not in is
/// parked in a scope of its own instead. Which area a text belongs to is
/// decided by where it sits, the same line the pointer is judged by.
#[derive(Clone, Copy, PartialEq)]
enum Area {
    Chat,
    Viewer,
}

thread_local! {
    /// Where the text of the area you're not in waits.
    static PARKED: TextSelectionScopeId = TextSelectionScopeId::new();
    static ACTIVE: Cell<Area> = const { Cell::new(Area::Chat) };
    /// (frame, next): text is numbered in the order it's laid out each frame,
    /// top to bottom, so a copy comes out in reading order.
    static ORDER: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    /// What the next text made copies with before it, set by the block
    /// laying it out (a list marker, a quote's `> `).
    static LEAD: RefCell<Option<SharedString>> = const { RefCell::new(None) };
    static VIEWER_LEFT: Cell<Option<f32>> = const { Cell::new(None) };
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

/// The selection as text, shaped like what was selected: paragraphs apart,
/// list items with their markers, cells side by side, maths as TeX.
pub fn copied(window: &mut Window, cx: &mut App) -> String {
    shape(&TextSelection::selected_text(window, cx))
}

/// The pieces as selected, one per line, made into the copy.
fn shape(joined: &str) -> String {
    let mut out = joined.replace(&format!("\n{SAME_LINE}"), "");
    out.retain(|c| c != SAME_LINE);
    out.trim_start_matches('\n').trim_end().to_string()
}

/// Where the viewer starts this frame, if it's open. Text to its right
/// selects with the viewer, the rest with the chat; the pointer switches
/// between them by the same line.
pub fn set_viewer_left(x: Option<f32>) {
    VIEWER_LEFT.with(|l| l.set(x));
}

fn in_viewer(x: f32) -> bool {
    VIEWER_LEFT.with(Cell::get).is_some_and(|left| x >= left)
}

/// Text in the area being selected joins the active scope; the rest is
/// parked, so a drag can't reach it.
fn scope_at(x: f32) -> TextSelectionScopeId {
    let area = if in_viewer(x) { Area::Viewer } else { Area::Chat };
    if area == ACTIVE.with(Cell::get) {
        TextSelectionScopeId::default()
    } else {
        PARKED.with(|p| *p)
    }
}

/// The area under the pointer selects: the viewer's text or the chat's. It
/// switches as the pointer moves between them, but never while something is
/// selected, so a selection can't be lost on the way to ⌘C; a press in the
/// other area switches then, and the drag picks up the text it reaches
/// from the next frame on.
pub fn hover(at: Point<Pixels>, pressed: bool, window: &mut Window, cx: &mut App) {
    let area = if in_viewer(at.x.as_f32()) {
        Area::Viewer
    } else {
        Area::Chat
    };
    if ACTIVE.with(Cell::get) == area || (!pressed && TextSelection::has_selection(window, cx)) {
        return;
    }
    ACTIVE.with(|a| a.set(area));
    window.refresh();
}

fn next_order() -> u64 {
    let frame = crate::ui::FRAME.load(std::sync::atomic::Ordering::Relaxed);
    ORDER.with(|o| {
        let (f, n) = o.get();
        let n = if f == frame { n + 1 } else { 0 };
        o.set((frame, n));
        n
    })
}

pub struct Text {
    id: ElementId,
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
    Text {
        id: id.into(),
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
    Text {
        id: id.into(),
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

/// Kept across frames: the selection handle, the link a press began on, and
/// the redraw that keeps the highlight under the pointer while it drags.
#[derive(Clone)]
pub struct Kept {
    handle: TextSelectionHandle,
    /// What a copy reads: the text, its lead and swaps, and the part
    /// selected when it was last painted.
    copy: Rc<RefCell<Copy>>,
    pressed: Rc<Cell<Option<usize>>>,
    _redraw: Rc<Subscription>,
}

#[derive(Default)]
struct Copy {
    text: SharedString,
    lead: SharedString,
    swaps: Rc<Vec<(Range<usize>, String)>>,
    range: Option<Range<usize>>,
}

impl Copy {
    fn text(&self) -> String {
        let Some(range) = self.range.clone().filter(|r| !r.is_empty()) else {
            return String::new();
        };
        let mut out = String::new();
        if range.start == 0 {
            out.push_str(&self.lead);
        }
        let mut at = range.start;
        for (r, with) in self.swaps.iter() {
            if r.end <= at || r.start >= range.end {
                continue;
            }
            out.push_str(&self.text[at..r.start.max(at)]);
            out.push_str(with);
            at = r.end.min(range.end).max(at);
        }
        out.push_str(&self.text[at..range.end]);
        out
    }
}

impl IntoElement for Text {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Text {
    type RequestLayoutState = Kept;
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
    ) -> (LayoutId, Kept) {
        let kept = window.with_element_state(
            id.expect("selectable text has an id"),
            |kept: Option<Kept>, window| {
                let kept = kept.unwrap_or_else(|| {
                    let handle = TextSelectionHandle::new(self.text.to_string(), cx);
                    let redraw = handle.refresh_window_on_change(window, cx);
                    let copy: Rc<RefCell<Copy>> = Rc::default();
                    handle.copy_with(
                        {
                            let copy = copy.clone();
                            move |_| copy.borrow().text()
                        },
                        cx,
                    );
                    Kept {
                        handle,
                        copy,
                        pressed: Rc::default(),
                        _redraw: Rc::new(redraw),
                    }
                });
                {
                    let mut c = kept.copy.borrow_mut();
                    c.text = self.text.clone();
                    c.lead = self.lead.clone();
                    c.swaps = self.swaps.clone();
                }
                (kept.clone(), kept)
            },
        );
        let (layout, ()) = self.styled.request_layout(id, inspector, window, cx);
        (layout, kept)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        kept: &mut Kept,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.styled
            .prepaint(id, inspector, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_document_order(next_order())
            // The viewer and the chat each select on their own.
            .with_scope(scope_at(bounds.origin.x.as_f32()))
            .with_text_bounds(vec![bounds])
            .with_rendered_element(&kept.handle, window, cx);
        kept.handle.register(registration, window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        kept: &mut Kept,
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled.layout().clone();
        let projection = kept.handle.update_runs(
            &[TextSelectionRun::new(
                self.text.clone(),
                layout.clone(),
                bounds,
            )],
            cx,
        );
        kept.copy.borrow_mut().range = projection.ranges().first().cloned().flatten();
        for range in projection.ranges().iter().flatten() {
            paint_range(&layout, range.clone(), self.color, window);
        }
        self.styled
            .paint(id, inspector, bounds, &mut (), &mut (), window, cx);

        let link_at = {
            let links = self.links.clone();
            let layout = layout.clone();
            move |p: Point<Pixels>| {
                let ix = layout.index_for_position(p).ok()?;
                links.iter().position(|(r, _)| r.contains(&ix))
            }
        };
        let over_link = hitbox.is_hovered(window)
            && !self.links.is_empty()
            && link_at(window.mouse_position()).is_some();
        window.set_cursor_style(
            if over_link {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            },
            hitbox,
        );
        if self.links.is_empty() {
            return;
        }
        window.on_mouse_event({
            let (hitbox, pressed, link_at) =
                (hitbox.clone(), kept.pressed.clone(), link_at.clone());
            move |e: &MouseDownEvent, phase, window, _| {
                if phase.bubble() && e.button == MouseButton::Left && hitbox.is_hovered(window) {
                    pressed.set(link_at(e.position));
                }
            }
        });
        window.on_mouse_event({
            let (hitbox, pressed, links) =
                (hitbox.clone(), kept.pressed.clone(), self.links.clone());
            move |e: &MouseUpEvent, phase, window, cx| {
                if !phase.bubble() || e.button != MouseButton::Left {
                    return;
                }
                let down = pressed.take();
                if !hitbox.is_hovered(window) || TextSelection::has_selection(window, cx) {
                    return;
                }
                if let Some(ix) = down.filter(|ix| link_at(e.position) == Some(*ix))
                    && let Some((_, url)) = links.get(ix)
                {
                    open_link(url, window, cx);
                }
            }
        });
    }
}

thread_local! {
    /// A file a link asked dent to show, for the app to open on its next draw.
    static OPEN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Follow a link: web addresses in the browser; a file dent can show (a
/// document, page or picture) in the viewer; any other file in its app.
pub fn open_link(url: &str, window: &mut Window, cx: &mut App) {
    if url.starts_with('/') {
        if crate::data::Kind::of(url).is_some() {
            OPEN.with(|o| *o.borrow_mut() = Some(url.to_string()));
            window.refresh();
        } else if std::path::Path::new(url).exists() {
            cx.open_url(&format!("file://{url}"));
        }
    } else if !url.starts_with('#') {
        cx.open_url(url);
    }
}

/// The file a link asked to show, once.
pub fn take_open() -> Option<String> {
    OPEN.with(|o| o.borrow_mut().take())
}

/// The highlight behind `range`: one band per line it covers.
fn paint_range(layout: &TextLayout, range: Range<usize>, color: Hsla, window: &mut Window) {
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
        window.paint_quad(fill(band, color));
    }
}

#[cfg(test)]
mod tests {
    use super::{Copy, SAME_LINE, shape};
    use std::ops::Range;
    use std::rc::Rc;

    fn piece(text: &str, lead: &str, range: Range<usize>) -> String {
        Copy {
            text: text.to_string().into(),
            lead: lead.to_string().into(),
            swaps: Rc::default(),
            range: Some(range),
        }
        .text()
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
        let c = Copy {
            text: "a mmmm b".into(),
            lead: "".into(),
            swaps: Rc::new(vec![(2..6, "$x^2$".into())]),
            range: Some(0..8),
        };
        assert_eq!(c.text(), "a $x^2$ b");
        // Part of a formula selected copies all of it.
        let part = Copy {
            range: Some(4..8),
            ..c
        };
        assert_eq!(part.text(), "$x^2$ b");
    }
}
