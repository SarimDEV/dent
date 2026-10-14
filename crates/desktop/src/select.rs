//! Text you can select. Every run of text that should be copyable is one of
//! these: it joins the window's single selection (the layer the root
//! mounts), so a drag can run from a reply into a code block and ⌘C copies
//! all of it. Double-click takes a word, triple-click a paragraph.
//!
//! Links stay clickable: a press and release on the same link opens it, a
//! drag that starts on one selects instead.

use std::cell::Cell;
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
/// reaches into the other. The viewer's subtree is marked with its scope.
#[derive(Clone, Copy, PartialEq)]
enum Area {
    Chat,
    Viewer,
}

thread_local! {
    pub static VIEWER_SCOPE: TextSelectionScopeId = TextSelectionScopeId::new();
    static ACTIVE: Cell<Area> = const { Cell::new(Area::Chat) };
    /// (frame, next): text is numbered in the order it's laid out each frame,
    /// top to bottom, so a copy comes out in reading order.
    static ORDER: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

pub fn viewer_scope() -> TextSelectionScopeId {
    VIEWER_SCOPE.with(|s| *s)
}

/// The area under the pointer selects: the viewer's text or the chat's. It
/// switches as the pointer moves between them, but never while something is
/// selected, so a selection can't be lost on the way to ⌘C; a press in the
/// other area switches then.
pub fn hover(viewer: bool, pressed: bool, window: &mut Window, cx: &mut App) {
    let area = if viewer { Area::Viewer } else { Area::Chat };
    if ACTIVE.with(Cell::get) == area || (!pressed && TextSelection::has_selection(window, cx)) {
        return;
    }
    ACTIVE.with(|a| a.set(area));
    let scope = if viewer {
        viewer_scope()
    } else {
        TextSelectionScopeId::default()
    };
    TextSelection::activate_scope(scope, window, cx);
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
    }
}

impl Text {
    /// Byte ranges that open somewhere when clicked: a URL, or a path to
    /// open in its default app.
    pub fn links(mut self, links: Vec<(Range<usize>, String)>) -> Self {
        self.links = Rc::new(links);
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
    pressed: Rc<Cell<Option<usize>>>,
    _redraw: Rc<Subscription>,
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
                    Kept {
                        handle,
                        pressed: Rc::default(),
                        _redraw: Rc::new(redraw),
                    }
                });
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
                    if url.starts_with('/') {
                        cx.open_url(&format!("file://{url}"));
                    } else {
                        cx.open_url(url);
                    }
                }
            }
        });
    }
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
