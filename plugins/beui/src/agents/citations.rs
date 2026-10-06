//! Ports beUI's `citations` agent-interface part.
//!
//! **Source:** `components/agents/citations.tsx` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! | upstream | here |
//! |---|---|
//! | `<Citation>` marker `min-w-4 rounded-md bg-muted/60 px-1 py-0.5 text-[10px] font-semibold` | [`citations`], [`CITATION_CHIP_HEIGHT`], [`CITATION_CHIP_PADDING_X`] |
//! | `<CitationStack>` overlapping favicons | the chip row, numbered rather than favicon-stacked (see the degradations) |
//! | `<Citations>` trigger `BookOpenText` + `Sources` + count + chevron | [`citation_list`]'s header |
//! | `<CitationList>` rows, `AnimatePresence` `y: 6 → 0` on `SPRING_LAYOUT` | the row reveal plus [`CITATION_CASCADE_RISE`] |
//! | `<CitationRow>` favicon, title, domain, index badge, external-link mark | [`citation_preview`] and the list rows |
//! | `AgentDisclosure` reveal | the list's reveal lane |
//!
//! # The hover preview, and the seam it is hosted on
//!
//! Upstream's marker is an `<a href="#…">` that jumps to the reference row.
//! An agent transcript on a desktop wants the reference *previewed* under the
//! pointer instead, so this port makes the chip row an **anchor publisher** for
//! the catalog's [anchored overlay host](crate::overlay::anchored):
//!
//! 1. the app holds an [`OverlayAnchor`] and a [`CitationHover`] and hands both
//!    to [`CitationsView::anchor`] / [`CitationsView::hover`];
//! 2. the chip row writes the hovered chip's **window-space** rect into the
//!    anchor and its index into the latch the moment a `Move` discovers the
//!    hover, and re-confirms both every `paint` — which is also the pass that
//!    *clears* them, since `PaintCtx::is_hovered` is the only authority that
//!    notices a pointer leaving without another move;
//! 3. the app mounts `overlay::anchored(citation_preview(…))` against that same
//!    anchor, open while the latch holds an index.
//!
//! The latch is the same non-reactive `Rc<Cell<_>>` shape [`OverlayAnchor`]
//! itself uses and the catalog's tooltip already reads a hover decision
//! through: written during a paint, read during the pass that follows it, so a
//! preview is at worst one frame behind and never permanently stale.
//! [`CitationsView::on_hover_change`] reports the same transition to app state,
//! but **best-effort and from the event pass** — a widget can only reach state
//! from an event, and a pointer that leaves without another move is only
//! noticed at paint time. It is a notification; the latch is the mechanism.
//!
//! # Degradations against the web original
//!
//! - **No favicons.** `useFavicon` fetches `https://<domain>/favicon.ico` (with
//!   a Google fallback) and renders an `<img>`. A plugin-tier widget has no
//!   network and `PaintScene::draw_image` wants already-decoded pixels the app
//!   would have to supply, so every row draws the globe glyph upstream itself
//!   falls back to. `CitationStack`'s overlapping-favicon cluster is dropped
//!   with it; the chip row is numbered instead, which is what the inline
//!   markers are anyway.
//! - **No anchor navigation.** Upstream's marker is a fragment link into the
//!   reference list. There is no document to scroll here, so a press reports
//!   [`CitationsView::on_activate`] and the app decides what to do with it
//!   (open the URL, scroll its own list).
//! - **No layout animation between rows.** Upstream's list is
//!   `AnimatePresence mode="popLayout"` with `layout="position"`, so a removed
//!   row slides its neighbours up. Here a row list change re-lays out
//!   immediately and only the *entrance* cascade is staged.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedArgCallback,
    EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View, Widget,
    erase_callback_arg,
};
use frust::{FrameTime, Theme};

use crate::agents::code_block::{code_palette, code_style, draw_chevron};
use crate::agents::tool_approval::{WrappedRun, prose_style, strong_style};
use crate::motion::{Ramp, Stagger};
use crate::overlay::OverlayAnchor;
use crate::press::{Lane, draw_focus_ring, inside, is_activation_key, presses};
use crate::style::{self, with_alpha};
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::BeuiTokens;
use crate::tokens::motion::{EASE_OUT, SPRING_PANEL};

/// A numbered chip's height, in logical px (`text-[10px] py-0.5`).
pub const CITATION_CHIP_HEIGHT: f64 = 16.0;

/// A numbered chip's minimum width, in logical px (`min-w-4`).
pub const CITATION_CHIP_MIN_WIDTH: f64 = 16.0;

/// A chip's horizontal padding, in logical px (`px-1`).
pub const CITATION_CHIP_PADDING_X: f64 = 4.0;

/// The gap between chips, in logical px (`mx-0.5` on both sides).
pub const CITATION_CHIP_GAP: f64 = 4.0;

/// The gap between wrapped chip rows, in logical px.
pub const CITATION_CHIP_ROW_GAP: f64 = 4.0;

/// A chip's corner radius (`rounded-md`).
pub const CITATION_CHIP_RADIUS: f64 = style::RADIUS_MD;

/// The chip's type size, in logical px (`text-[10px]`).
pub const CITATION_CHIP_SIZE: f64 = 10.0;

/// Alpha the chip's rest fill is painted at (`bg-muted/60`).
pub const CITATION_CHIP_FILL_ALPHA: f32 = 0.6;

/// How far apart consecutive chips or rows start arriving.
pub const CITATION_CASCADE_STEP: Duration = Duration::from_millis(28);

/// How long one chip's or row's own entrance takes.
pub const CITATION_CASCADE_ITEM: Duration = Duration::from_millis(200);

/// How far a chip or row rises into place, in logical px (`y: 6 → 0`).
pub const CITATION_CASCADE_RISE: f64 = 6.0;

/// The list header's height, in logical px (`min-h-8`).
pub const CITATION_HEADER_HEIGHT: f64 = 32.0;

/// One reference row's height, in logical px.
pub const CITATION_ROW_HEIGHT: f64 = 32.0;

/// The gap between reference rows, in logical px (`gap-0.5`).
pub const CITATION_ROW_GAP: f64 = 2.0;

/// A reference row's leading glyph box, in logical px (`size-5`).
pub const CITATION_GLYPH_BOX: f64 = 20.0;

/// The index badge's box on a reference row, in logical px (`size-5`).
pub const CITATION_BADGE_BOX: f64 = 20.0;

/// The preview panel's padding, in logical px.
pub const CITATION_PREVIEW_PADDING: f64 = 12.0;

/// The preview panel's corner radius (`rounded-xl`).
pub const CITATION_PREVIEW_RADIUS: f64 = style::RADIUS_XL;

/// The preview panel's width, in logical px — a fixed column, since the
/// anchored host places it against a chip rather than a container.
pub const CITATION_PREVIEW_WIDTH: f64 = 260.0;

/// The ramp the reference list's disclosure plays, in both directions.
pub const CITATION_REVEAL: Ramp = Ramp::spring(SPRING_PANEL);

/// The type-scale role a numbered marker — a chip, an index badge, the list's
/// count — takes its family from at layout.
const BADGE_ROLE: ThemeTextType = ThemeTextType::LabelSmall;

/// The type-scale role a citation's title takes its family from at layout, in
/// the preview and on a reference row.
const TITLE_ROLE: ThemeTextType = ThemeTextType::TitleSmall;

/// The type-scale role a citation's display domain takes its family from at
/// layout.
const DOMAIN_ROLE: ThemeTextType = ThemeTextType::BodySmall;

/// The type-scale role the reference list's header word takes its family from
/// at layout — the disclosure's control label.
const HEADER_ROLE: ThemeTextType = ThemeTextType::LabelLarge;

/// The cascade chips and reference rows arrive on.
pub fn citation_cascade() -> Stagger {
    Stagger::eased(CITATION_CASCADE_STEP, CITATION_CASCADE_ITEM, EASE_OUT)
}

/// One grounded source (`CitationItem`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Citation {
    /// The source's stable identity.
    pub id: String,
    /// Its title.
    pub title: String,
    /// Its display domain, when it has one.
    pub domain: Option<String>,
    /// Its URL, when it has one.
    pub url: Option<String>,
}

/// Build a citation identified by `id`.
pub fn citation(id: impl Into<String>, title: impl Into<String>) -> Citation {
    Citation {
        id: id.into(),
        title: title.into(),
        domain: None,
        url: None,
    }
}

impl Citation {
    /// Give the citation its display domain.
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Give the citation its URL.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}

/// The shared latch a chip row writes the hovered citation's index into.
///
/// **Not reactive** — see the [module docs](self) for the write-at-paint,
/// read-next-pass contract it shares with [`OverlayAnchor`].
#[derive(Clone, Debug, Default)]
pub struct CitationHover(Rc<Cell<Option<usize>>>);

impl CitationHover {
    /// A fresh latch, holding no hover.
    pub fn new() -> Self {
        Self::default()
    }

    /// The index of the citation the pointer is over, if any.
    pub fn hovered(&self) -> Option<usize> {
        self.0.get()
    }

    /// Overwrite the latch — for a caller driving the preview itself (a
    /// keyboard cursor, a programmatic highlight).
    pub fn set(&self, hovered: Option<usize>) {
        self.0.set(hovered);
    }
}

/// A view-held one-argument callback, erased on build.
type OnArg<State, A> = Rc<dyn Fn(&mut State, A)>;

// ---- the inline chip row ---------------------------------------------------

/// A declarative row of numbered citation chips. See the [module docs](self).
pub struct CitationsView<State: 'static> {
    citations: Vec<Citation>,
    anchor: Option<OverlayAnchor>,
    hover: Option<CitationHover>,
    on_activate: Option<OnArg<State, usize>>,
    on_hover_change: Option<OnArg<State, Option<usize>>>,
}

/// Create the inline chip row for `citations`, numbered from 1 in order.
///
/// Chain [`CitationsView::anchor`] and [`CitationsView::hover`] to drive an
/// anchored preview panel — see the [module docs](self).
pub fn citations<State: 'static>(citations: impl Into<Vec<Citation>>) -> CitationsView<State> {
    CitationsView {
        citations: citations.into(),
        anchor: None,
        hover: None,
        on_activate: None,
        on_hover_change: None,
    }
}

impl<State: 'static> CitationsView<State> {
    /// Publish the hovered chip's window rect into `anchor`, so an anchored
    /// overlay can place its preview against it.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.anchor = Some(anchor.clone());
        self
    }

    /// Publish the hovered chip's index into `hover`.
    pub fn hover(mut self, hover: &CitationHover) -> Self {
        self.hover = Some(hover.clone());
        self
    }

    /// Report a press on a chip, by index. Upstream navigates to the reference
    /// row; here the app decides — see the [module docs](self).
    pub fn on_activate<F: Fn(&mut State, usize) + 'static>(mut self, on_activate: F) -> Self {
        self.on_activate = Some(Rc::new(on_activate));
        self
    }

    /// Report a hover change, best-effort — see the [module docs](self).
    pub fn on_hover_change<F: Fn(&mut State, Option<usize>) + 'static>(
        mut self,
        on_change: F,
    ) -> Self {
        self.on_hover_change = Some(Rc::new(on_change));
        self
    }
}

/// The retained widget for a [`CitationsView`].
pub struct CitationsWidget {
    citations: Vec<Citation>,
    chips: Vec<LabelRun>,
    /// Each chip's box, resolved by the last layout pass.
    boxes: Vec<Rect>,
    width: f64,
    height: f64,
    /// The frame the entrance cascade started on, while one runs.
    entrance: Option<FrameTime>,
    /// Raised when `paint` self-corrected the hover away, so the next event
    /// pass can report it.
    hover_lost: bool,
    focused: usize,
    hovered: Option<usize>,
    captured: Option<usize>,
    anchor: Option<OverlayAnchor>,
    hover: Option<CitationHover>,
    on_activate: Option<ErasedArgCallback<usize>>,
    on_hover_change: Option<ErasedArgCallback<Option<usize>>>,
}

impl<State: 'static> View<State> for CitationsView<State> {
    type Element = CitationsWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CitationsWidget {
        CitationsWidget {
            chips: (1..=self.citations.len())
                .map(|n| LabelRun::new(n.to_string()))
                .collect(),
            citations: self.citations.clone(),
            boxes: Vec::new(),
            width: 0.0,
            height: 0.0,
            entrance: None,
            hover_lost: false,
            focused: 0,
            hovered: None,
            captured: None,
            anchor: self.anchor.clone(),
            hover: self.hover.clone(),
            on_activate: self.on_activate.as_ref().map(erase_callback_arg),
            on_hover_change: self.on_hover_change.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut CitationsWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_activate = self.on_activate.as_ref().map(erase_callback_arg);
        element.on_hover_change = self.on_hover_change.as_ref().map(erase_callback_arg);
        element.anchor = self.anchor.clone();
        element.hover = self.hover.clone();
        if element.citations == self.citations {
            return ChangeFlags::NONE;
        }
        if element.citations.len() != self.citations.len() {
            element.chips = (1..=self.citations.len())
                .map(|n| LabelRun::new(n.to_string()))
                .collect();
            // A new set of chips arrives with a fresh cascade.
            element.entrance = None;
            element.captured = None;
            element.hovered = None;
            element.focused = 0;
        }
        element.citations = self.citations.clone();
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }
}

impl CitationsWidget {
    /// Report the hover transition to app state, if anything is listening and
    /// it actually changed, and publish it to the shared latch and anchor.
    fn report_hover(&mut self, ctx: &mut EventCtx, next: Option<usize>) {
        if self.hovered == next {
            return;
        }
        self.hovered = next;
        self.publish(next, ctx.origin());
        if let Some(on_change) = self.on_hover_change.as_mut() {
            on_change(ctx, next);
        }
        ctx.request_redraw();
    }

    /// Write `hovered` into the shared latch, and that chip's window-space rect
    /// into the anchor an overlay places its preview against.
    fn publish(&self, hovered: Option<usize>, origin: Point) {
        if let Some(hover) = &self.hover {
            hover.set(hovered);
        }
        if let Some(anchor) = &self.anchor
            && let Some(rect) = hovered.and_then(|index| self.boxes.get(index))
        {
            anchor.set(Rect::from_origin_size(
                origin + rect.origin().to_vec2(),
                rect.size(),
            ));
        }
    }

    /// The chip under a widget-local `pos`, if any.
    fn hit(&self, pos: Point) -> Option<usize> {
        self.boxes.iter().position(|rect| rect.contains(pos))
    }
}

/// Paint the globe glyph a source with no favicon falls back to (`Globe2`).
pub(crate) fn draw_globe(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let inset = box_size * 0.14;
    let rect = Rect::new(inset, inset, box_size - inset, box_size - inset);
    let circle = RoundedRect::from_rect(rect, (box_size - inset * 2.0) / 2.0);
    scene.stroke_path(
        origin,
        &Shape::to_path(&circle, style::PATH_TOLERANCE),
        1.2,
        &Brush::Solid(color),
    );
    let mut lines = BezPath::new();
    lines.move_to(Point::new(inset, box_size / 2.0));
    lines.line_to(Point::new(box_size - inset, box_size / 2.0));
    lines.move_to(Point::new(box_size / 2.0, inset));
    lines.line_to(Point::new(box_size * 0.68, box_size / 2.0));
    lines.line_to(Point::new(box_size / 2.0, box_size - inset));
    lines.line_to(Point::new(box_size * 0.32, box_size / 2.0));
    lines.close_path();
    scene.stroke_path(origin, &lines, 1.0, &Brush::Solid(color));
}

/// Paint the external-link glyph a linked row carries (`ExternalLink`).
fn draw_external(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let unit = box_size / 16.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(unit * 9.0, unit * 3.0));
    path.line_to(Point::new(unit * 13.0, unit * 3.0));
    path.line_to(Point::new(unit * 13.0, unit * 7.0));
    path.move_to(Point::new(unit * 13.0, unit * 3.0));
    path.line_to(Point::new(unit * 7.5, unit * 8.5));
    path.move_to(Point::new(unit * 11.0, unit * 10.0));
    path.line_to(Point::new(unit * 11.0, unit * 13.0));
    path.line_to(Point::new(unit * 3.0, unit * 13.0));
    path.line_to(Point::new(unit * 3.0, unit * 5.0));
    path.line_to(Point::new(unit * 6.0, unit * 5.0));
    scene.stroke_path(origin, &path, 1.1, &Brush::Solid(color));
}

/// Paint the open-book glyph the reference list's header carries
/// (`BookOpenText`).
fn draw_book(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let unit = box_size / 16.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(unit * 2.0, unit * 4.0));
    path.line_to(Point::new(unit * 7.0, unit * 4.0));
    path.line_to(Point::new(unit * 8.0, unit * 5.5));
    path.line_to(Point::new(unit * 9.0, unit * 4.0));
    path.line_to(Point::new(unit * 14.0, unit * 4.0));
    path.line_to(Point::new(unit * 14.0, unit * 12.5));
    path.line_to(Point::new(unit * 9.0, unit * 12.5));
    path.line_to(Point::new(unit * 8.0, unit * 11.0));
    path.line_to(Point::new(unit * 7.0, unit * 12.5));
    path.line_to(Point::new(unit * 2.0, unit * 12.5));
    path.close_path();
    scene.stroke_path(origin, &path, 1.2, &Brush::Solid(color));
}

impl Widget for CitationsWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.width = bc.max().width;
        let style = strong_style(CITATION_CHIP_SIZE);
        self.boxes.clear();
        let mut x = 0.0;
        let mut y = 0.0;
        for chip in &mut self.chips {
            let measured = chip.layout_themed(ctx, &style, BADGE_ROLE);
            let width =
                (measured.width + CITATION_CHIP_PADDING_X * 2.0).max(CITATION_CHIP_MIN_WIDTH);
            if x > 0.0 && x + width > self.width {
                x = 0.0;
                y += CITATION_CHIP_HEIGHT + CITATION_CHIP_ROW_GAP;
            }
            self.boxes.push(Rect::from_origin_size(
                Point::new(x, y),
                Size::new(width, CITATION_CHIP_HEIGHT),
            ));
            x += width + CITATION_CHIP_GAP;
        }
        self.height = if self.chips.is_empty() {
            0.0
        } else {
            y + CITATION_CHIP_HEIGHT
        };
        bc.constrain(Size::new(self.width, self.height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative and is the only thing that
        // notices a pointer that left without another move; the transition is
        // recorded for the next event pass, which is the only pass that can
        // report it to app state.
        if !ctx.is_hovered() && self.hovered.is_some() {
            self.hovered = None;
            self.hover_lost = true;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let palette = code_palette(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let ring = with_alpha(
            BeuiTokens::resolve_ring(None, theme),
            style::FOCUS_RING_OPACITY,
        );
        let now = ctx.frame_time();
        let origin = ctx.origin();

        let cascade = if reduce_motion {
            citation_cascade().collapsed()
        } else {
            citation_cascade()
        };
        let started = *self.entrance.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        let count = self.chips.len();

        for (index, chip) in self.chips.iter().enumerate() {
            let Some(rect) = self.boxes.get(index) else {
                continue;
            };
            let arrived = if reduce_motion {
                1.0
            } else {
                cascade.progress_clamped(elapsed, index, count)
            };
            let at = Point::new(
                origin.x + rect.x0,
                origin.y + rect.y0 + (1.0 - arrived) * CITATION_CASCADE_RISE,
            );
            let hovered = self.hovered == Some(index);
            if arrived < 1.0 {
                scene.push_layer(at, rect.size(), arrived as f32);
            }
            scene.fill_rounded_rect(
                at,
                rect.size(),
                CITATION_CHIP_RADIUS,
                with_alpha(palette.surface, CITATION_CHIP_FILL_ALPHA),
            );
            let measured = chip.size();
            chip.paint(
                Point::new(
                    at.x + (rect.width() - measured.width) / 2.0,
                    at.y + (CITATION_CHIP_HEIGHT - measured.height) / 2.0,
                ),
                if hovered {
                    palette.plain
                } else {
                    palette.comment
                },
                scene,
            );
            if arrived < 1.0 {
                scene.pop_layer();
            }
            if self.focused == index && ctx.has_focus() {
                draw_focus_ring(scene, at, rect.size(), CITATION_CHIP_RADIUS, 0.0, ring);
            }
        }

        // Re-confirm the published hover: a trigger that moved under a settled
        // pointer needs its anchor re-read, and a pointer that left without
        // another move is only noticed here.
        self.publish(self.hovered, origin);

        if !reduce_motion && !cascade.is_settled(elapsed, count) {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            return EventResult::Ignored;
        }
        // Drain the paint-noticed hover loss: the pointer left without another
        // move, so this is the first pass that can say so.
        if self.hover_lost {
            self.hover_lost = false;
            if let Some(on_change) = self.on_hover_change.as_mut() {
                on_change(ctx, None);
            }
        }
        match event {
            InputEvent::Key(key) => {
                let step = match &key.key {
                    Key::Named(NamedKey::ArrowRight) => Some(1),
                    Key::Named(NamedKey::ArrowLeft) => Some(-1),
                    _ => None,
                };
                if let Some(step) = step {
                    if self.chips.is_empty() {
                        return EventResult::Ignored;
                    }
                    let len = self.chips.len() as isize;
                    self.focused = (self.focused as isize + step).rem_euclid(len) as usize;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if is_activation_key(key) && self.focused < self.chips.len() {
                    let index = self.focused;
                    if let Some(on_activate) = self.on_activate.as_mut() {
                        on_activate(ctx, index);
                    }
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) || !inside(p.position, ctx.size()) {
                        return EventResult::Ignored;
                    }
                    let Some(index) = self.hit(p.position) else {
                        return EventResult::Ignored;
                    };
                    self.captured = Some(index);
                    self.focused = index;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if self.captured.is_some() {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                        return EventResult::Handled;
                    }
                    let over = self.hit(p.position);
                    if over.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    self.report_hover(ctx, over);
                    EventResult::Ignored
                }
                PointerPhase::Up => {
                    let Some(armed) = self.captured.take() else {
                        return EventResult::Ignored;
                    };
                    if self.hit(p.position) == Some(armed)
                        && let Some(on_activate) = self.on_activate.as_mut()
                    {
                        on_activate(ctx, armed);
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.captured.take().is_none() {
                        return EventResult::Ignored;
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for (index, item) in self.citations.iter().enumerate() {
            ctx.push_node(Role::Button, |node| {
                node.set_label(format!("Citation {}: {}", index + 1, item.title).as_str());
                node.add_action(Action::Click);
            });
        }
    }
}

// ---- the anchored preview panel --------------------------------------------

/// A declarative preview panel for one citation — the content an anchored
/// overlay hosts. See the [module docs](self).
pub struct CitationPreviewView {
    citation: Citation,
    index: usize,
}

/// Create the preview panel for `citation`, numbered `index` (1-based).
pub fn citation_preview(citation: Citation, index: usize) -> CitationPreviewView {
    CitationPreviewView { citation, index }
}

/// The retained widget for a [`CitationPreviewView`].
pub struct CitationPreviewWidget {
    citation: Citation,
    index: LabelRun,
    title: WrappedRun,
    domain: Option<LabelRun>,
    url: Option<WrappedRun>,
    title_height: f64,
    url_height: f64,
}

impl<State: 'static> View<State> for CitationPreviewView {
    type Element = CitationPreviewWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CitationPreviewWidget {
        CitationPreviewWidget {
            index: LabelRun::new(self.index.to_string()),
            title: WrappedRun::new(self.citation.title.clone()),
            domain: self.citation.domain.as_deref().map(LabelRun::new),
            url: self.citation.url.as_deref().map(WrappedRun::new),
            citation: self.citation.clone(),
            title_height: 0.0,
            url_height: 0.0,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut CitationPreviewWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if element.citation == self.citation && element.index.content() == self.index.to_string() {
            return ChangeFlags::NONE;
        }
        element.index = LabelRun::new(self.index.to_string());
        element.title = WrappedRun::new(self.citation.title.clone());
        element.domain = self.citation.domain.as_deref().map(LabelRun::new);
        element.url = self.citation.url.as_deref().map(WrappedRun::new);
        element.citation = self.citation.clone();
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }
}

impl Widget for CitationPreviewWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = bc.max().width.min(CITATION_PREVIEW_WIDTH);
        let inner =
            (width - CITATION_PREVIEW_PADDING * 2.0 - CITATION_GLYPH_BOX - style::GAP_MD).max(0.0);
        let title = themed_style(
            strong_style(style::TEXT_SM),
            TITLE_ROLE,
            Theme::from_layout_ctx(ctx),
        );
        self.title_height = self.title.layout(ctx, &title, inner).height;
        if let Some(domain) = &mut self.domain {
            domain.layout_themed(ctx, &prose_style(style::TEXT_XS), DOMAIN_ROLE);
        }
        self.url_height = match &mut self.url {
            // Explicit: the URL is set in the mono stack, and `TypeScale` has
            // no monospace role to take it from.
            Some(url) => {
                url.layout(ctx, &code_style(CITATION_CHIP_SIZE), inner)
                    .height
            }
            None => 0.0,
        };
        self.index
            .layout_themed(ctx, &strong_style(CITATION_CHIP_SIZE), BADGE_ROLE);

        let mut height = CITATION_PREVIEW_PADDING * 2.0 + self.title_height;
        if self.domain.is_some() {
            height += style::spacing(0.5) + CITATION_CHIP_SIZE + 4.0;
        }
        if self.url_height > 0.0 {
            height += style::spacing(1.0) + self.url_height;
        }
        bc.constrain(Size::new(width, height.max(CITATION_ROW_HEIGHT)))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let palette = code_palette(theme);
        let chrome = crate::components::popover::resolve_panel(theme);
        let origin = ctx.origin();
        let size = ctx.size();

        crate::components::popover::paint_panel(
            scene,
            origin,
            Rect::from_origin_size(Point::ORIGIN, size),
            CITATION_PREVIEW_RADIUS,
            chrome,
        );

        let glyph_at = Point::new(
            origin.x + CITATION_PREVIEW_PADDING,
            origin.y + CITATION_PREVIEW_PADDING,
        );
        draw_globe(scene, glyph_at, CITATION_GLYPH_BOX, chrome.dim_ink);

        let column_x = glyph_at.x + CITATION_GLYPH_BOX + style::GAP_MD;
        let mut y = origin.y + CITATION_PREVIEW_PADDING;
        self.title.paint(Point::new(column_x, y), chrome.ink, scene);
        y += self.title_height;

        if let Some(domain) = &self.domain {
            y += style::spacing(0.5);
            domain.paint(Point::new(column_x, y), chrome.dim_ink, scene);
            y += CITATION_CHIP_SIZE + 4.0;
        }
        if let Some(url) = &self.url {
            y += style::spacing(1.0);
            url.paint(Point::new(column_x, y), palette.function, scene);
        }

        // The numbered badge, mirroring the chip the panel is anchored to.
        let badge = Size::new(CITATION_BADGE_BOX, CITATION_BADGE_BOX);
        let badge_at = Point::new(
            origin.x + size.width - CITATION_PREVIEW_PADDING - CITATION_BADGE_BOX,
            origin.y + CITATION_PREVIEW_PADDING,
        );
        scene.fill_rounded_rect(
            badge_at,
            badge,
            CITATION_CHIP_RADIUS,
            with_alpha(chrome.ink, 0.05),
        );
        let measured = self.index.size();
        self.index.paint(
            Point::new(
                badge_at.x + (CITATION_BADGE_BOX - measured.width) / 2.0,
                badge_at.y + (CITATION_BADGE_BOX - measured.height) / 2.0,
            ),
            chrome.dim_ink,
            scene,
        );
        if self.citation.url.is_some() {
            draw_external(
                scene,
                Point::new(
                    badge_at.x - style::ICON_SIZE - style::GAP_SM,
                    badge_at.y + (CITATION_BADGE_BOX - style::ICON_SIZE) / 2.0,
                ),
                style::ICON_SIZE,
                chrome.dim_ink,
            );
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Tooltip, |node| {
            node.set_label(self.citation.title.as_str());
        });
    }
}

// ---- the collapsible reference list ----------------------------------------

/// A declarative collapsible reference list — upstream's `Citations` wrapper
/// over its `CitationList`. See the [module docs](self).
pub struct CitationListView<State: 'static> {
    citations: Vec<Citation>,
    title: String,
    open: bool,
    on_open_change: Option<OnArg<State, bool>>,
    on_activate: Option<OnArg<State, usize>>,
}

/// Create the collapsible reference list for `citations`, closed by default
/// (upstream's `defaultOpen = false`).
pub fn citation_list<State: 'static>(
    citations: impl Into<Vec<Citation>>,
) -> CitationListView<State> {
    CitationListView {
        citations: citations.into(),
        title: "Sources".to_owned(),
        open: false,
        on_open_change: None,
        on_activate: None,
    }
}

impl<State: 'static> CitationListView<State> {
    /// The header's word (`title`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Whether the reference rows are disclosed (`open`).
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Report the disclosure a press on the header asks for.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_change: F) -> Self {
        self.on_open_change = Some(Rc::new(on_change));
        self
    }

    /// Report a press on a reference row, by index.
    pub fn on_activate<F: Fn(&mut State, usize) + 'static>(mut self, on_activate: F) -> Self {
        self.on_activate = Some(Rc::new(on_activate));
        self
    }
}

/// One retained reference row.
struct RowRuns {
    title: LabelRun,
    domain: Option<LabelRun>,
    index: LabelRun,
    linked: bool,
}

/// Which affordance the roving cursor sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListTarget {
    /// The `Sources` header, which toggles the disclosure.
    Header,
    /// One reference row.
    Row(usize),
}

/// The retained widget for a [`CitationListView`].
pub struct CitationListWidget {
    citations: Vec<Citation>,
    rows: Vec<RowRuns>,
    title: LabelRun,
    title_text: String,
    count: LabelRun,
    open: bool,
    /// The disclosure lane.
    reveal: Lane,
    /// The frame the row cascade started on, while one runs.
    cascade_from: Option<FrameTime>,
    width: f64,
    focused: ListTarget,
    hovered: Option<ListTarget>,
    captured: Option<ListTarget>,
    on_open_change: Option<ErasedArgCallback<bool>>,
    on_activate: Option<ErasedArgCallback<usize>>,
}

/// Build the shape-cache carriers for every reference row.
fn build_rows(citations: &[Citation]) -> Vec<RowRuns> {
    citations
        .iter()
        .enumerate()
        .map(|(index, item)| RowRuns {
            title: LabelRun::new(item.title.clone()),
            domain: item.domain.as_deref().map(LabelRun::new),
            index: LabelRun::new((index + 1).to_string()),
            linked: item.url.is_some(),
        })
        .collect()
}

impl<State: 'static> View<State> for CitationListView<State> {
    type Element = CitationListWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CitationListWidget {
        CitationListWidget {
            rows: build_rows(&self.citations),
            count: LabelRun::new(self.citations.len().to_string()),
            citations: self.citations.clone(),
            title: LabelRun::new(self.title.clone()),
            title_text: self.title.clone(),
            open: self.open,
            reveal: Lane::at_rest(CITATION_REVEAL, if self.open { 1.0 } else { 0.0 }),
            cascade_from: None,
            width: 0.0,
            focused: ListTarget::Header,
            hovered: None,
            captured: None,
            on_open_change: self.on_open_change.as_ref().map(erase_callback_arg),
            on_activate: self.on_activate.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut CitationListWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_open_change = self.on_open_change.as_ref().map(erase_callback_arg);
        element.on_activate = self.on_activate.as_ref().map(erase_callback_arg);
        let mut flags = ChangeFlags::NONE;

        if element.citations != self.citations {
            element.rows = build_rows(&self.citations);
            element.count = LabelRun::new(self.citations.len().to_string());
            element.citations = self.citations.clone();
            element.captured = None;
            element.hovered = None;
            element.focused = ListTarget::Header;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.title_text != self.title {
            element.title = LabelRun::new(self.title.clone());
            element.title_text = self.title.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.open != self.open {
            element.open = self.open;
            if self.open {
                // An opening list re-runs its row cascade.
                element.cascade_from = None;
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Retargeted, never restarted: a rebuild re-passing the flag the lane is
        // already flying toward leaves its clock alone.
        element
            .reveal
            .retarget(if element.open { 1.0 } else { 0.0 });
        flags
    }
}

impl CitationListWidget {
    /// The rows' natural (fully-disclosed) height.
    fn rows_natural(&self) -> f64 {
        if self.rows.is_empty() {
            return 0.0;
        }
        self.rows.len() as f64 * CITATION_ROW_HEIGHT
            + CITATION_ROW_GAP * (self.rows.len() - 1) as f64
    }

    /// The rows' band height right now, tracking the reveal. An empty list has
    /// nothing to disclose, so it never opens a gap for it.
    fn band(&self) -> f64 {
        if self.rows.is_empty() {
            return 0.0;
        }
        (self.reveal.value().clamp(0.0, 1.0) * (style::GAP_SM + self.rows_natural())).max(0.0)
    }

    /// The header's box, in widget-local space.
    fn header_rect(&self) -> Rect {
        Rect::from_origin_size(Point::ORIGIN, Size::new(self.width, CITATION_HEADER_HEIGHT))
    }

    /// Row `index`'s box, in widget-local space.
    fn row_rect(&self, index: usize) -> Option<Rect> {
        if index >= self.rows.len() {
            return None;
        }
        let top = CITATION_HEADER_HEIGHT
            + style::GAP_SM
            + index as f64 * (CITATION_ROW_HEIGHT + CITATION_ROW_GAP);
        Some(Rect::from_origin_size(
            Point::new(0.0, top),
            Size::new(self.width, CITATION_ROW_HEIGHT),
        ))
    }

    /// The affordance under a widget-local `pos`, if any.
    fn hit(&self, pos: Point) -> Option<ListTarget> {
        if self.header_rect().contains(pos) {
            return Some(ListTarget::Header);
        }
        if self.reveal.value() <= 0.5 {
            return None;
        }
        let limit = CITATION_HEADER_HEIGHT + self.band();
        (0..self.rows.len())
            .find(|index| {
                self.row_rect(*index)
                    .is_some_and(|rect| rect.contains(pos) && pos.y < limit)
            })
            .map(ListTarget::Row)
    }

    /// The affordances the roving cursor can reach, in visual order.
    fn targets(&self) -> Vec<ListTarget> {
        let mut targets = vec![ListTarget::Header];
        if self.open {
            targets.extend((0..self.rows.len()).map(ListTarget::Row));
        }
        targets
    }

    /// The next affordance `step` places from the focused one, wrapping.
    fn step_target(&self, step: isize) -> Option<ListTarget> {
        let targets = self.targets();
        if targets.is_empty() {
            return None;
        }
        let at = targets.iter().position(|t| *t == self.focused).unwrap_or(0);
        let next = (at as isize + step).rem_euclid(targets.len() as isize) as usize;
        targets.get(next).copied()
    }

    /// Report the decision `target` asks for. Fires exactly one callback.
    fn activate(&mut self, ctx: &mut EventCtx, target: ListTarget) {
        match target {
            ListTarget::Header => {
                let next = !self.open;
                if let Some(on_change) = self.on_open_change.as_mut() {
                    on_change(ctx, next);
                }
            }
            ListTarget::Row(index) => {
                if index < self.rows.len()
                    && let Some(on_activate) = self.on_activate.as_mut()
                {
                    on_activate(ctx, index);
                }
            }
        }
    }
}

impl Widget for CitationListWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.width = bc.max().width;
        self.title
            .layout_themed(ctx, &strong_style(style::TEXT_SM), HEADER_ROLE);
        self.count
            .layout_themed(ctx, &strong_style(CITATION_CHIP_SIZE), BADGE_ROLE);
        for row in &mut self.rows {
            row.title
                .layout_themed(ctx, &strong_style(style::TEXT_SM), TITLE_ROLE);
            row.index
                .layout_themed(ctx, &strong_style(CITATION_CHIP_SIZE), BADGE_ROLE);
            if let Some(domain) = &mut row.domain {
                domain.layout_themed(ctx, &prose_style(style::TEXT_XS), DOMAIN_ROLE);
            }
        }
        bc.constrain(Size::new(self.width, CITATION_HEADER_HEIGHT + self.band()))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let palette = code_palette(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let ring = with_alpha(
            BeuiTokens::resolve_ring(None, theme),
            style::FOCUS_RING_OPACITY,
        );
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        let before = self.reveal.value();
        let moving = if reduce_motion {
            self.reveal.snap();
            self.cascade_from = None;
            false
        } else {
            let running = self.reveal.advance(now);
            if self.reveal.target() >= 1.0 && self.reveal.value() > 0.0 {
                self.cascade_from.get_or_insert(now);
            } else if self.reveal.target() <= 0.0 {
                self.cascade_from = None;
            }
            running
        };

        // ---- the header -------------------------------------------------------
        let header_hovered = self.hovered == Some(ListTarget::Header);
        let header_ink = if header_hovered {
            palette.plain
        } else {
            palette.comment
        };
        let mut x = origin.x;
        draw_book(
            scene,
            Point::new(
                x,
                origin.y + (CITATION_HEADER_HEIGHT - style::ICON_SIZE) / 2.0,
            ),
            style::ICON_SIZE,
            header_ink,
        );
        x += style::ICON_SIZE + style::GAP_MD;
        let title_size = self.title.size();
        self.title.paint(
            Point::new(
                x,
                origin.y + (CITATION_HEADER_HEIGHT - title_size.height) / 2.0,
            ),
            header_ink,
            scene,
        );
        x += title_size.width + style::GAP_MD;

        let count_size = self.count.size();
        let badge_width =
            (count_size.width + CITATION_CHIP_PADDING_X * 2.0).max(CITATION_CHIP_MIN_WIDTH);
        let badge_at = Point::new(
            x,
            origin.y + (CITATION_HEADER_HEIGHT - CITATION_CHIP_HEIGHT) / 2.0,
        );
        scene.fill_rounded_rect(
            badge_at,
            Size::new(badge_width, CITATION_CHIP_HEIGHT),
            CITATION_CHIP_HEIGHT / 2.0,
            with_alpha(palette.surface, CITATION_CHIP_FILL_ALPHA),
        );
        self.count.paint(
            Point::new(
                badge_at.x + (badge_width - count_size.width) / 2.0,
                badge_at.y + (CITATION_CHIP_HEIGHT - count_size.height) / 2.0,
            ),
            palette.comment,
            scene,
        );
        x += badge_width + style::GAP_SM;
        draw_chevron(
            scene,
            Point::new(
                x + style::ICON_SIZE / 2.0,
                origin.y + CITATION_HEADER_HEIGHT / 2.0,
            ),
            style::ICON_SIZE,
            self.reveal.value().clamp(0.0, 1.0) * std::f64::consts::PI,
            header_ink,
        );
        if self.focused == ListTarget::Header && ctx.has_focus() {
            draw_focus_ring(
                scene,
                origin,
                Size::new(x + style::ICON_SIZE, CITATION_HEADER_HEIGHT),
                style::RADIUS_LG,
                0.0,
                ring,
            );
        }

        // ---- the reference rows ----------------------------------------------
        let band = self.band();
        if band > 0.5 {
            let cascade = if reduce_motion {
                citation_cascade().collapsed()
            } else {
                citation_cascade()
            };
            let elapsed = self
                .cascade_from
                .map_or(Duration::ZERO, |from| now.saturating_sub(from));
            let count = self.rows.len();
            let top = origin.y + CITATION_HEADER_HEIGHT;
            scene.push_clip(Point::new(origin.x, top), Size::new(size.width, band));
            for (index, row) in self.rows.iter().enumerate() {
                let Some(rect) = self.row_rect(index) else {
                    continue;
                };
                let arrived = if reduce_motion || self.cascade_from.is_none() {
                    1.0
                } else {
                    cascade.progress_clamped(elapsed, index, count)
                };
                let at = Point::new(
                    origin.x + rect.x0,
                    origin.y + rect.y0 + (1.0 - arrived) * CITATION_CASCADE_RISE,
                );
                if at.y > top + band {
                    break;
                }
                let hovered = self.hovered == Some(ListTarget::Row(index));
                if arrived < 1.0 {
                    scene.push_layer(at, rect.size(), arrived as f32);
                }
                if hovered {
                    scene.fill_rounded_rect(
                        at,
                        rect.size(),
                        style::RADIUS_MD,
                        with_alpha(palette.plain, style::HOVER_WASH_ALPHA),
                    );
                }
                draw_globe(
                    scene,
                    Point::new(
                        at.x + style::GAP_SM,
                        at.y + (CITATION_ROW_HEIGHT - CITATION_GLYPH_BOX) / 2.0,
                    ),
                    CITATION_GLYPH_BOX,
                    palette.comment,
                );
                let mut column = at.x + style::GAP_SM + CITATION_GLYPH_BOX + style::GAP_MD;
                let title_size = row.title.size();
                row.title.paint(
                    Point::new(
                        column,
                        at.y + (CITATION_ROW_HEIGHT - title_size.height) / 2.0,
                    ),
                    if hovered {
                        palette.plain
                    } else {
                        palette.comment
                    },
                    scene,
                );
                column += title_size.width + style::GAP_MD;
                if let Some(domain) = &row.domain {
                    let measured = domain.size();
                    domain.paint(
                        Point::new(column, at.y + (CITATION_ROW_HEIGHT - measured.height) / 2.0),
                        with_alpha(palette.comment, 0.6),
                        scene,
                    );
                }

                let mut right = at.x + rect.width() - style::GAP_SM;
                if row.linked {
                    right -= style::ICON_SIZE;
                    draw_external(
                        scene,
                        Point::new(right, at.y + (CITATION_ROW_HEIGHT - style::ICON_SIZE) / 2.0),
                        style::ICON_SIZE,
                        with_alpha(palette.comment, 0.5),
                    );
                    right -= style::GAP_SM;
                }
                right -= CITATION_BADGE_BOX;
                let badge_at = Point::new(
                    right,
                    at.y + (CITATION_ROW_HEIGHT - CITATION_BADGE_BOX) / 2.0,
                );
                scene.fill_rounded_rect(
                    badge_at,
                    Size::new(CITATION_BADGE_BOX, CITATION_BADGE_BOX),
                    CITATION_CHIP_RADIUS,
                    with_alpha(palette.plain, 0.05),
                );
                let measured = row.index.size();
                row.index.paint(
                    Point::new(
                        badge_at.x + (CITATION_BADGE_BOX - measured.width) / 2.0,
                        badge_at.y + (CITATION_BADGE_BOX - measured.height) / 2.0,
                    ),
                    palette.comment,
                    scene,
                );
                if arrived < 1.0 {
                    scene.pop_layer();
                }
                if self.focused == ListTarget::Row(index) && ctx.has_focus() {
                    draw_focus_ring(scene, at, rect.size(), style::RADIUS_MD, 0.0, ring);
                }
            }
            scene.pop_clip();
            if !reduce_motion && self.cascade_from.is_some() && !cascade.is_settled(elapsed, count)
            {
                ctx.request_frame();
            }
        }

        // The band feeds the reported height, so a bare frame request would let
        // the disclosure freeze on the intra-frame layout skip. Ask for layout
        // while it moves, and once more on the frame the value changed — which
        // covers the landing frame and the `reduce_motion` snap.
        if moving || self.reveal.value() != before {
            ctx.request_layout();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Key(key) => {
                let step = match &key.key {
                    Key::Named(NamedKey::ArrowDown) => Some(1),
                    Key::Named(NamedKey::ArrowUp) => Some(-1),
                    _ => None,
                };
                if let Some(step) = step {
                    let Some(next) = self.step_target(step) else {
                        return EventResult::Ignored;
                    };
                    self.focused = next;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if is_activation_key(key) {
                    let target = self.focused;
                    if !self.targets().contains(&target) {
                        return EventResult::Ignored;
                    }
                    self.activate(ctx, target);
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) || !inside(p.position, ctx.size()) {
                        return EventResult::Ignored;
                    }
                    let Some(target) = self.hit(p.position) else {
                        return EventResult::Ignored;
                    };
                    self.captured = Some(target);
                    self.focused = target;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if self.captured.is_some() {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                        return EventResult::Handled;
                    }
                    let over = self.hit(p.position);
                    if over.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    EventResult::Ignored
                }
                PointerPhase::Up => {
                    let Some(armed) = self.captured.take() else {
                        return EventResult::Ignored;
                    };
                    if self.hit(p.position) == Some(armed) {
                        self.activate(ctx, armed);
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.captured.take().is_none() {
                        return EventResult::Ignored;
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = format!("{} ({})", self.title_text, self.citations.len());
        ctx.push_node(Role::Button, |node| {
            node.set_label(label.as_str());
            node.set_expanded(self.open);
            node.add_action(Action::Click);
        });
        if self.open {
            for (index, item) in self.citations.iter().enumerate() {
                ctx.push_node(Role::ListItem, |node| {
                    node.set_label(format!("{}. {}", index + 1, item.title).as_str());
                    node.add_action(Action::Click);
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Affine, CornerRadii, KeyEvent, Modifiers, PointerButton, PointerEvent};
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        clips: Vec<(Point, Size)>,
        layers: Vec<f32>,
        inks: Vec<Color>,
        transforms: Vec<Affine>,
        strokes: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, _r: f64, c: Color) {
            self.rounded.push((o, s, c));
        }
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, _r: CornerRadii, c: Color) {
            self.rounded.push((o, s, c));
        }
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s));
        }
        fn push_clip_rounded(&mut self, o: Point, s: Size, _r: f64) {
            self.clips.push((o, s));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _b: f64, _c: Color) {}
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn push_transform(&mut self, t: Affine) {
            self.transforms.push(t);
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct Picked {
        activated: Option<usize>,
        activations: u32,
        hover: Option<Option<usize>>,
        hover_calls: u32,
        open: Option<bool>,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn items() -> Vec<Citation> {
        vec![
            citation("rfc-9110", "HTTP Semantics")
                .domain("rfc-editor.org")
                .url("https://www.rfc-editor.org/rfc/rfc9110"),
            citation("wgpu", "wgpu documentation").domain("docs.rs"),
            citation("beui", "beUI component registry")
                .domain("beui.dev")
                .url("https://beui.dev"),
        ]
    }

    fn chips_view(anchor: &OverlayAnchor, hover: &CitationHover) -> CitationsView<Picked> {
        citations::<Picked>(items())
            .anchor(anchor)
            .hover(hover)
            .on_activate(|s: &mut Picked, index: usize| {
                s.activated = Some(index);
                s.activations += 1;
            })
            .on_hover_change(|s: &mut Picked, next: Option<usize>| {
                s.hover = Some(next);
                s.hover_calls += 1;
            })
    }

    fn build_chips(v: &CitationsView<Picked>) -> CitationsWidget {
        let mut counter = 0u64;
        View::<Picked>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout_of<W: Widget>(w: &mut W, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(width, 900.0)),
        )
    }

    fn paint_of<W: Widget>(
        w: &mut W,
        size: Size,
        theme: Option<&Theme>,
        ms: f64,
    ) -> (Recorder, bool, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame(), ctx.needs_layout())
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch<W: Widget>(w: &mut W, size: Size, event: &InputEvent, state: &mut Picked) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    fn key(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    /// The model builds up from `id` and `title`, with the two optional fields
    /// chained on.
    #[test]
    fn a_citation_carries_its_identity_title_domain_and_url() {
        let bare = citation("a", "A title");
        assert_eq!(bare.id, "a");
        assert_eq!(bare.title, "A title");
        assert_eq!(bare.domain, None);
        assert_eq!(bare.url, None);

        let full = bare.domain("example.com").url("https://example.com/x");
        assert_eq!(full.domain.as_deref(), Some("example.com"));
        assert_eq!(full.url.as_deref(), Some("https://example.com/x"));
        assert_eq!(Citation::default().title, "");
    }

    /// The chips lay out left to right, wrap when the row runs out, and keep a
    /// minimum width whatever the number measures.
    #[test]
    fn the_chips_lay_out_in_order_and_wrap() {
        let anchor = OverlayAnchor::new();
        let hover = CitationHover::new();
        let mut w = build_chips(&chips_view(&anchor, &hover));
        let size = layout_of(&mut w, 300.0);
        assert_eq!(w.boxes.len(), 3);
        assert!(w.boxes.windows(2).all(|pair| pair[0].x0 < pair[1].x0));
        assert!(w.boxes.iter().all(|r| r.width() >= CITATION_CHIP_MIN_WIDTH));
        assert_eq!(size.height, CITATION_CHIP_HEIGHT, "one row at 300px");

        // A width that fits one chip forces every later one onto its own row.
        let mut narrow = build_chips(&chips_view(&anchor, &hover));
        let size = layout_of(&mut narrow, CITATION_CHIP_MIN_WIDTH + 1.0);
        assert!(size.height > CITATION_CHIP_HEIGHT, "the row wrapped");
        assert!(narrow.boxes[1].y0 > narrow.boxes[0].y0);
        assert_eq!(narrow.boxes[1].x0, 0.0, "a wrapped chip restarts the row");
    }

    /// The chips cascade in: early frames composite partial alphas and ask for
    /// another frame, and the run settles.
    #[test]
    fn the_chips_cascade_in_and_settle() {
        let anchor = OverlayAnchor::new();
        let hover = CitationHover::new();
        let mut w = build_chips(&chips_view(&anchor, &hover));
        let size = layout_of(&mut w, 300.0);
        let (rec, needs_frame, _) = paint_of(&mut w, size, None, 0.0);
        assert!(needs_frame, "a running cascade owes another frame");
        assert!(
            rec.layers.iter().any(|a| *a < 1.0),
            "the chips start partway in: {:?}",
            rec.layers
        );

        let (rec, needs_frame, _) = paint_of(&mut w, size, None, 2_000.0);
        assert!(!needs_frame, "a settled cascade asks for nothing");
        assert!(rec.layers.is_empty(), "and composites nothing");
        assert_eq!(rec.rounded.len(), 3, "one chip fill each");
        assert_eq!(rec.inks.len(), 3, "one number each");
    }

    /// `reduce_motion` lands every chip on the frame it is first painted.
    #[test]
    fn reduce_motion_lands_the_cascade_immediately() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let anchor = OverlayAnchor::new();
        let hover = CitationHover::new();
        let mut w = build_chips(&chips_view(&anchor, &hover));
        let size = layout_of(&mut w, 300.0);
        let (rec, needs_frame, _) = paint_of(&mut w, size, Some(&theme), 0.0);
        assert!(!needs_frame, "nothing is owed under reduced motion");
        assert!(rec.layers.is_empty());
    }

    /// Hovering a chip reports the transition once, latches the index and
    /// publishes the chip's window rect for the anchored host to place against.
    #[test]
    fn hovering_a_chip_reports_it_latches_it_and_publishes_its_rect() {
        let anchor = OverlayAnchor::new();
        let hover = CitationHover::new();
        let mut w = build_chips(&chips_view(&anchor, &hover));
        let size = layout_of(&mut w, 300.0);
        paint_of(&mut w, size, None, 0.0);
        let mut state = Picked::default();
        assert_eq!(hover.hovered(), None);

        let target = w.boxes[1];
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, target.center()),
            &mut state,
        );
        assert_eq!(state.hover, Some(Some(1)));
        assert_eq!(state.hover_calls, 1);

        // A second move inside the same chip reports nothing new.
        dispatch(
            &mut w,
            size,
            &pointer(
                PointerPhase::Move,
                Point::new(target.x0 + 1.0, target.y0 + 1.0),
            ),
            &mut state,
        );
        assert_eq!(state.hover_calls, 1, "one report per transition");

        assert_eq!(hover.hovered(), Some(1), "the latch carries the index");
        assert_eq!(anchor.rect().width(), target.width());
        assert_eq!(anchor.rect().x0, target.x0, "in window space");

        // Moving off every chip reports the clear.
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, Point::new(1_000.0, 1_000.0)),
            &mut state,
        );
        assert_eq!(state.hover, Some(None));
        assert_eq!(state.hover_calls, 2);
        assert_eq!(hover.hovered(), None);
    }

    /// A pointer that leaves without another move is noticed at paint time and
    /// reported from the next event pass.
    #[test]
    fn a_hover_lost_at_paint_time_is_reported_on_the_next_pass() {
        let anchor = OverlayAnchor::new();
        let hover = CitationHover::new();
        let mut w = build_chips(&chips_view(&anchor, &hover));
        let size = layout_of(&mut w, 300.0);
        let mut state = Picked::default();
        let first = w.boxes[0].center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, first),
            &mut state,
        );
        assert_eq!(state.hover_calls, 1);

        assert_eq!(hover.hovered(), Some(0));
        // A paint with no hover link: the widget self-corrects and records it.
        paint_of(&mut w, size, None, 100.0);
        assert_eq!(w.hovered, None);
        assert!(w.hover_lost);
        assert_eq!(hover.hovered(), None, "the latch is authoritative at once");

        // The next event pass drains the record.
        dispatch(&mut w, size, &key(NamedKey::ArrowRight), &mut state);
        assert_eq!(state.hover, Some(None));
        assert_eq!(state.hover_calls, 2);
        assert!(!w.hover_lost, "and does not report it twice");
    }

    /// A press on a chip reports it once; a release off the armed chip reports
    /// nothing.
    #[test]
    fn pressing_a_chip_reports_it_once_and_up_inside_only() {
        let anchor = OverlayAnchor::new();
        let hover = CitationHover::new();
        let mut w = build_chips(&chips_view(&anchor, &hover));
        let size = layout_of(&mut w, 300.0);
        paint_of(&mut w, size, None, 0.0);
        let mut state = Picked::default();

        let at = w.boxes[2].center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.activated, Some(2));
        assert_eq!(state.activations, 1);

        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, Point::new(1_000.0, 1_000.0)),
            &mut state,
        );
        assert_eq!(state.activations, 1, "the release landed elsewhere");
    }

    /// Keyboard: the arrows rove the chips and the activation keys report the
    /// one under the cursor.
    #[test]
    fn the_arrows_rove_the_chips_and_enter_reports_one() {
        let anchor = OverlayAnchor::new();
        let hover = CitationHover::new();
        let mut w = build_chips(&chips_view(&anchor, &hover));
        let size = layout_of(&mut w, 300.0);
        paint_of(&mut w, size, None, 0.0);
        let mut state = Picked::default();
        dispatch(&mut w, size, &key(NamedKey::ArrowRight), &mut state);
        assert_eq!(w.focused, 1);
        dispatch(&mut w, size, &key(NamedKey::Enter), &mut state);
        assert_eq!(state.activated, Some(1));
        dispatch(&mut w, size, &key(NamedKey::ArrowLeft), &mut state);
        dispatch(&mut w, size, &key(NamedKey::ArrowLeft), &mut state);
        assert_eq!(w.focused, 2, "the cursor wraps");
    }

    /// The preview panel paints its panel chrome, the globe fallback, the
    /// title, the domain, the URL and the numbered badge.
    #[test]
    fn the_preview_paints_the_panel_the_globe_and_the_reference() {
        let mut w = <CitationPreviewView as View<Picked>>::build(
            &citation_preview(items()[0].clone(), 1),
            &mut BuildCtx::new(&mut 0u64),
        );
        let size = layout_of(&mut w, CITATION_PREVIEW_WIDTH);
        assert_eq!(size.width, CITATION_PREVIEW_WIDTH);
        assert!(size.height > CITATION_PREVIEW_PADDING * 2.0);
        let (rec, _, _) = paint_of(&mut w, size, None, 0.0);
        assert!(!rec.rounded.is_empty(), "the panel plus the badge");
        assert!(rec.strokes >= 3, "the hairline, the globe, the link mark");
        // title + domain + url (which may wrap to two rows) + badge number.
        let full_runs = rec.inks.len();
        assert!(full_runs >= 4, "painted {full_runs} runs");

        // A citation with no domain or URL drops both runs and the link mark.
        let mut bare = <CitationPreviewView as View<Picked>>::build(
            &citation_preview(citation("x", "Bare"), 9),
            &mut BuildCtx::new(&mut 0u64),
        );
        let size = layout_of(&mut bare, CITATION_PREVIEW_WIDTH);
        let (rec, _, _) = paint_of(&mut bare, size, None, 0.0);
        assert_eq!(rec.inks.len(), 2, "just the title and the badge");
        assert!(rec.inks.len() < full_runs);
    }

    /// The reference list discloses on its own lane, cascades its rows in and
    /// asks for relayout while the band moves.
    #[test]
    fn the_reference_list_discloses_and_cascades_its_rows() {
        let view = |open: bool| {
            citation_list::<Picked>(items())
                .open(open)
                .on_open_change(|s: &mut Picked, next: bool| s.open = Some(next))
                .on_activate(|s: &mut Picked, index: usize| {
                    s.activated = Some(index);
                    s.activations += 1;
                })
        };
        let mut w = View::<Picked>::build(&view(false), &mut BuildCtx::new(&mut 0u64));
        let size = layout_of(&mut w, 360.0);
        assert_eq!(size.height, CITATION_HEADER_HEIGHT, "closed is the header");
        paint_of(&mut w, size, None, 0.0);

        View::<Picked>::rebuild(
            &view(true),
            &view(false),
            &mut w,
            &mut BuildCtx::new(&mut 0u64),
        );
        let (_, _, needs_layout) = paint_of(&mut w, size, None, 100.0);
        assert!(needs_layout, "an opening list must ask for relayout");

        let mut staged = false;
        let mut tallest = CITATION_HEADER_HEIGHT;
        for step in 1..=60 {
            let (rec, _, _) = paint_of(&mut w, size, None, 100.0 + f64::from(step) * 20.0);
            staged |= rec.layers.iter().any(|a| *a > 0.0 && *a < 1.0);
            tallest = tallest.max(layout_of(&mut w, 360.0).height);
        }
        assert!(staged, "no row was ever partway in");
        assert!(tallest > CITATION_HEADER_HEIGHT, "the list never grew");
        assert_eq!(w.reveal.value(), 1.0);
        assert_eq!(
            layout_of(&mut w, 360.0).height,
            CITATION_HEADER_HEIGHT + style::GAP_SM + w.rows_natural()
        );
    }

    /// The list's header and rows each report their decision exactly once, and
    /// the widget never writes its own disclosure.
    #[test]
    fn the_list_triggers_report_once_and_write_nothing() {
        let view = |open: bool| {
            citation_list::<Picked>(items())
                .open(open)
                .on_open_change(|s: &mut Picked, next: bool| s.open = Some(next))
                .on_activate(|s: &mut Picked, index: usize| {
                    s.activated = Some(index);
                    s.activations += 1;
                })
        };
        let mut w = View::<Picked>::build(&view(true), &mut BuildCtx::new(&mut 0u64));
        let size = layout_of(&mut w, 360.0);
        for step in 0..=60 {
            paint_of(&mut w, size, None, f64::from(step) * 25.0);
        }
        let mut state = Picked::default();

        let header = w.header_rect().center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, header),
            &mut state,
        );
        dispatch(&mut w, size, &pointer(PointerPhase::Up, header), &mut state);
        assert_eq!(state.open, Some(false));
        assert!(w.open, "the widget never writes its own disclosure");

        let row = w.row_rect(1).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, row), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, row), &mut state);
        assert_eq!(state.activated, Some(1));
        assert_eq!(state.activations, 1);

        assert_eq!(w.targets().len(), 4, "the header plus three rows");
        // The press above left the cursor on row 1, so the arrow steps past it.
        dispatch(&mut w, size, &key(NamedKey::ArrowDown), &mut state);
        assert_eq!(w.focused, ListTarget::Row(2));
        dispatch(&mut w, size, &key(NamedKey::Enter), &mut state);
        assert_eq!(state.activated, Some(2));
        assert_eq!(state.activations, 2);
    }

    /// A closed list offers only its header, and an empty one measures nothing
    /// to disclose.
    #[test]
    fn a_closed_list_offers_only_its_header_and_an_empty_one_nothing() {
        let mut closed = View::<Picked>::build(
            &citation_list::<Picked>(items()),
            &mut BuildCtx::new(&mut 0u64),
        );
        let size = layout_of(&mut closed, 360.0);
        paint_of(&mut closed, size, None, 0.0);
        assert_eq!(closed.targets(), vec![ListTarget::Header]);
        assert_eq!(closed.band(), 0.0);

        let mut empty = View::<Picked>::build(
            &citation_list::<Picked>(Vec::new()).open(true),
            &mut BuildCtx::new(&mut 0u64),
        );
        let size = layout_of(&mut empty, 360.0);
        paint_of(&mut empty, size, None, 0.0);
        assert_eq!(empty.rows_natural(), 0.0);
        assert_eq!(size.height, CITATION_HEADER_HEIGHT);

        let mut no_chips =
            build_chips(&citations::<Picked>(Vec::new()).on_activate(|_: &mut Picked, _| {}));
        let size = layout_of(&mut no_chips, 300.0);
        assert_eq!(size.height, 0.0);
        paint_of(&mut no_chips, size, None, 0.0);
        assert_eq!(no_chips.hit(Point::ZERO), None);
    }

    // ---- Typeface: chips, preview and list follow the live theme -----------

    use crate::text::typeface_probe::{
        Face, Probe, assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(400.0, 600.0);

    /// Two sources with titles and domains, and no URL.
    fn probe_sources() -> Vec<Citation> {
        vec![
            citation("a", "Rust reference").domain("doc.rust-lang.org"),
            citation("b", "The Book").domain("rust-lang.org"),
        ]
    }

    /// The chip row, a preview and an opened reference list — every sans run
    /// this module shapes. The preview carries no URL: that run is mono on
    /// purpose, and `the_preview_url_stays_in_geist_mono` pins it.
    fn probe_view(_: &mut ()) -> frust::FlexView<()> {
        frust::column()
            .child(citations::<()>(probe_sources()))
            .child(citation_preview(probe_sources()[0].clone(), 1))
            .child(citation_list::<()>(probe_sources()).open(true))
    }

    #[test]
    fn citation_text_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the citation text", probe_view, PROBE_WINDOW);
    }

    #[test]
    fn citation_text_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the citation text", probe_view, PROBE_WINDOW);
    }

    /// The explicit site: under the beUI theme, whose every role is Geist, the
    /// preview's URL still paints in Geist Mono beside its Geist runs.
    #[test]
    fn the_preview_url_stays_in_geist_mono() {
        let logic = |_: &mut ()| {
            frust::column().child(citation_preview(
                citation("a", "Rust reference").url("https://doc.rust-lang.org"),
                1,
            ))
        };
        let faces = Probe::new(logic, PROBE_WINDOW, crate::theme()).frame();
        let mono = faces.iter().filter(|f| **f == Face::GeistMono).count();
        assert_eq!(mono, 1, "exactly the URL is mono: {faces:?}");
        assert!(
            faces.iter().all(|f| *f != Face::Other),
            "every run is a bundled face: {faces:?}"
        );
    }
}
