//! Ports beUI's `preview-rail` component.
//!
//! **Source:** `components/motion/preview-rail.tsx` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! | upstream | here |
//! |---|---|
//! | rail `w-12` / `h-12`, `itemSize = 24` | [`PREVIEW_RAIL_THICKNESS`], [`PREVIEW_RAIL_ITEM_SIZE`] |
//! | tick `h-0.5 w-12 origin-left` / `h-12 w-0.5 origin-bottom` | [`PREVIEW_TICK_THICKNESS`] and the per-orientation origin |
//! | `scale = 1 / 0.68 / 0.44 / 0.25` by index distance | [`PREVIEW_RAIL_SCALES`] |
//! | tick transition `SPRING_LAYOUT` | the scales spring together on [`SPRING_LAYOUT`] |
//! | tick `text-muted-foreground`, highlighted `text-foreground` | the resolved tick inks |
//! | card `rounded-2xl border border-border bg-card p-4 shadow-sm` | [`PREVIEW_CARD_RADIUS`], [`PREVIEW_CARD_PADDING`] |
//! | title `font-medium text-card-foreground`, body `mt-1 text-sm text-muted-foreground` | the card title run and its content child |
//! | card `{opacity:0, y:4} → {opacity:1, y:0}`, `0.18s EASE_OUT` | [`PREVIEW_CARD_TIMING`], [`PREVIEW_CARD_RISE`] |
//! | vertical card at `left-16`, horizontal card `bottom-12 w-72` | [`PREVIEW_CARD_GAP`], [`PREVIEW_CARD_WIDTH_HORIZONTAL`] |
//!
//! # Premise correction: it is a tick rail, not a rail of cards
//!
//! The porting card describes this slug as "a horizontal rail of preview cards
//! with hover-expand focus and spring layout shifts of neighbors". Upstream is
//! something else and the port follows upstream: a rail of **hairline ticks**
//! (one per section, `24px` apart, `48px` long) with **one** floating preview
//! card for whichever tick is currently pointed at. The "neighbour" motion is a
//! *scale* falloff on the ticks themselves — the pointed tick draws at full
//! length, its immediate neighbours at `0.68`, then `0.44`, then `0.25` — and
//! nothing shifts position at all. Vertical is upstream's default orientation;
//! horizontal is its documented alternative and is ported too.
//!
//! # The falloff is a table, not a curve
//!
//! [`PREVIEW_RAIL_SCALES`] is upstream's literal ladder rather than a function:
//! the source writes `distance === 1 ? 0.68 : distance === 2 ? 0.44 : 0.25`, and
//! a smooth curve fitted through those four numbers would not reproduce the flat
//! `0.25` floor every further tick sits on. What *is* animated is the move
//! between two ladders: when the pointed tick changes, every tick springs from
//! the length it was displaying to its new one on
//! [`SPRING_LAYOUT`](crate::tokens::motion::SPRING_LAYOUT), which is the
//! transition upstream puts on each tick.
//!
//! # Degradations against the web original
//!
//! - **No blur in the card's entrance.** Upstream animates
//!   `filter: blur(6px) → blur(0)` alongside the fade and rise. `PaintScene`
//!   publishes no blur filter, so the fade and the rise carry the entrance.
//! - **No exit for the outgoing card.** Upstream's `AnimatePresence mode="wait"`
//!   plays a 0.12s exit before the next card enters. Here a change of pointed
//!   tick swaps the content and replays the entrance; keeping the outgoing card
//!   mounted would mean laying out two preview children at once for 120ms.
//! - **The tap/pin dance is simplified.** Upstream separates hover, a
//!   touch-tap "pin", keyboard focus and an outside-tap dismiss, because a
//!   finger cannot hover and a link must not be followed by the tap that reveals
//!   its card. Here a press pins the card and a second press on the same tick
//!   un-pins it, with hover taking precedence over a pin while the pointer is on
//!   the rail. There are no `href` items in this port, so the half of upstream's
//!   logic that exists to defer a navigation has nothing to defer.
//! - **No `children` slot.** Upstream's rail can wrap the page content it
//!   indexes; here the rail plus its card is the whole widget and a host lays it
//!   beside whatever it indexes.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerButton,
    PointerEvent, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View, Widget,
    any, build_child, erase_callback_arg, rebuild_children, route_event_single, teardown_child,
    text::{FontWeight, TextContext, TextLayout, TextStyle},
    visit_children,
};
use frust::{ChildKey, FrameTime, Theme};

use crate::motion::Ramp;
use crate::style;
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT};

/// A tick's full length, in logical px (`w-12` / `h-12`).
pub const PREVIEW_RAIL_THICKNESS: f64 = 48.0;

/// The pitch between two ticks, in logical px (the `itemSize` prop's default).
pub const PREVIEW_RAIL_ITEM_SIZE: f64 = 24.0;

/// A tick's thickness, in logical px (`h-0.5` / `w-0.5`).
pub const PREVIEW_TICK_THICKNESS: f64 = 2.0;

/// The tick-length ladder by index distance from the pointed tick: `0` away,
/// `1` away, `2` away, and everything further out.
pub const PREVIEW_RAIL_SCALES: [f64; 4] = [1.0, 0.68, 0.44, 0.25];

/// The gap between the rail and the preview card, in logical px (`left-16`
/// against a `w-12` rail, and `bottom-12` in the horizontal arrangement).
pub const PREVIEW_CARD_GAP: f64 = 16.0;

/// The card's largest width in the vertical arrangement, in logical px
/// (`max-w-sm`).
pub const PREVIEW_CARD_MAX_WIDTH: f64 = 384.0;

/// The card's width in the horizontal arrangement, in logical px (`w-72`).
pub const PREVIEW_CARD_WIDTH_HORIZONTAL: f64 = 288.0;

/// The card's padding, in logical px (`p-4`).
pub const PREVIEW_CARD_PADDING: f64 = 16.0;

/// The gap between the card's title and its body, in logical px (`mt-1`).
pub const PREVIEW_CARD_TITLE_GAP: f64 = 4.0;

/// The card's corner radius, in logical px (`rounded-2xl`).
pub const PREVIEW_CARD_RADIUS: f64 = style::RADIUS_2XL;

/// How far below its resting place the card starts, in logical px
/// (`initial={{ y: 4 }}`).
pub const PREVIEW_CARD_RISE: f64 = 4.0;

/// The card's entrance (`{ duration: 0.18, ease: EASE_OUT }`).
pub const PREVIEW_CARD_TIMING: Ramp = Ramp::eased(Duration::from_millis(180), EASE_OUT);

/// Which way the rail runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewRailOrientation {
    /// `orientation="vertical"`: the rail runs down the left edge and the card
    /// sits to its right. Upstream's default.
    #[default]
    Vertical,
    /// `orientation="horizontal"`: the rail runs along the bottom and the card
    /// sits above it.
    Horizontal,
}

/// The tick length at `distance` index-steps from the pointed tick, as a
/// fraction of [`PREVIEW_RAIL_THICKNESS`].
///
/// Upstream's literal ladder — see the [module docs](self) on why it is not a
/// curve. Anything three or more steps out sits on the flat floor.
pub fn preview_rail_scale(distance: usize) -> f64 {
    PREVIEW_RAIL_SCALES[distance.min(PREVIEW_RAIL_SCALES.len() - 1)]
}

/// A cached text run whose colour is applied at paint time.
struct Run {
    content: String,
    layout: Option<TextLayout>,
    shaped: Option<TextStyle>,
}

impl Run {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped: None,
        }
    }

    fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, |l| l.size())
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.shaped.as_ref() == Some(style)
        {
            return cached.size();
        }
        let laid = ctx
            .text_context::<TextContext>()
            .layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped = Some(style.clone());
        size
    }

    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for mut run in layout.to_scene_runs(origin) {
                run.brush = Brush::Solid(color);
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// One rail entry: the section's label (the card's title and the tick's
/// accessible name) and the body its preview card shows.
pub struct PreviewRailItem<State: 'static> {
    label: String,
    preview: AnyView<State>,
}

/// Create a rail entry titled `label` whose card shows `preview`.
pub fn preview_rail_item<State: 'static, V: View<State>>(
    label: impl Into<String>,
    preview: V,
) -> PreviewRailItem<State> {
    PreviewRailItem {
        label: label.into(),
        preview: any(preview),
    }
}

/// A view-held selection callback, erased on build.
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative beUI preview rail. See the [module docs](self).
pub struct PreviewRailView<State: 'static> {
    items: Vec<PreviewRailItem<State>>,
    active: usize,
    orientation: PreviewRailOrientation,
    highlight_active: bool,
    on_select: OnSelect<State>,
}

/// Create a preview rail over `items` whose current section is `active`,
/// reporting a chosen index through `on_select` — a **controlled** component.
pub fn preview_rail<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<PreviewRailItem<State>>,
    active: usize,
    on_select: F,
) -> PreviewRailView<State> {
    PreviewRailView {
        items,
        active,
        orientation: PreviewRailOrientation::default(),
        highlight_active: false,
        on_select: Rc::new(on_select),
    }
}

impl<State: 'static> PreviewRailView<State> {
    /// Run the rail vertically or horizontally.
    pub fn orientation(mut self, orientation: PreviewRailOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Draw the active section's tick at full length even with no pointer on the
    /// rail (upstream's `highlightActive`).
    pub fn highlight_active(mut self, highlight: bool) -> Self {
        self.highlight_active = highlight;
        self
    }
}

/// The resolved rail palette.
struct RailColors {
    /// The pointed tick (`text-foreground`).
    tick_active: Color,
    /// Every other tick (`text-muted-foreground`).
    tick: Color,
    /// The card's fill (`bg-card`).
    card: Color,
    /// The card's hairline (`border-border`).
    border: Color,
    /// The card's title (`text-card-foreground`).
    title: Color,
}

/// Resolve the palette, falling back to the vendored **light** table unthemed.
fn resolve_colors(theme: Option<&Theme>) -> RailColors {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            RailColors {
                tick_active: s.on_surface,
                tick: s.on_surface_variant,
                card: s.surface_container,
                border: s.outline_variant,
                title: s.on_surface,
            }
        }
        None => {
            let p = crate::BEUI_LIGHT;
            RailColors {
                tick_active: p.foreground,
                tick: p.muted_foreground,
                card: p.card,
                border: p.border,
                title: p.card_foreground,
            }
        }
    }
}

/// The card title style (`font-medium`).
fn title_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    });
    TextStyle {
        family,
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_BASE as f32, Color::BLACK)
    }
}

/// Whether `p` carries a button that may begin a press.
fn presses(p: &PointerEvent) -> bool {
    p.button == PointerButton::Primary
}

/// One retained entry.
struct Entry {
    title: Run,
    label: String,
    /// The tick length fraction currently displayed.
    scale: f64,
}

impl<State: 'static> View<State> for PreviewRailView<State> {
    type Element = PreviewRailWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PreviewRailWidget {
        let mut widget = PreviewRailWidget {
            entries: self
                .items
                .iter()
                .map(|item| Entry {
                    title: Run::new(item.label.clone()),
                    label: item.label.clone(),
                    scale: preview_rail_scale(usize::MAX),
                })
                .collect(),
            previews: self
                .items
                .iter()
                .map(|item| build_child(&item.preview, ctx))
                .collect(),
            active: self.active,
            orientation: self.orientation,
            highlight_active: self.highlight_active,
            hovered: None,
            pinned: None,
            captured: None,
            card_started: None,
            card_playing: false,
            scales_from: None,
            scales_started: None,
            size: Size::ZERO,
            card_rect: Rect::ZERO,
            on_select: erase_callback_arg(&self.on_select),
        };
        widget.settle_scales();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PreviewRailWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = erase_callback_arg(&self.on_select);
        let mut flags = ChangeFlags::NONE;

        if prev.items.len() != self.items.len() {
            element.entries = self
                .items
                .iter()
                .map(|item| Entry {
                    title: Run::new(item.label.clone()),
                    label: item.label.clone(),
                    scale: preview_rail_scale(usize::MAX),
                })
                .collect();
            element.hovered = None;
            element.pinned = None;
            element.captured = None;
            element.settle_scales();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (entry, item) in element.entries.iter_mut().zip(self.items.iter()) {
                if entry.label != item.label {
                    entry.title = Run::new(item.label.clone());
                    entry.label = item.label.clone();
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }

        if prev.active != self.active
            || prev.orientation != self.orientation
            || prev.highlight_active != self.highlight_active
        {
            element.active = self.active;
            element.orientation = self.orientation;
            element.highlight_active = self.highlight_active;
            element.retarget_scales();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags |= rebuild_children(
            &prev.items,
            &self.items,
            &mut element.previews,
            ctx,
            |item: &PreviewRailItem<State>| &item.preview,
            |_| None::<ChildKey>,
        );
        flags
    }

    fn teardown(&self, element: &mut PreviewRailWidget, ctx: &mut BuildCtx<'_>) {
        for (item, pod) in self.items.iter().zip(element.previews.iter_mut()) {
            teardown_child(&item.preview, pod, ctx);
        }
    }
}

/// The retained widget for a [`PreviewRailView`].
pub struct PreviewRailWidget {
    entries: Vec<Entry>,
    /// One preview body per entry; only the displayed one is laid out or
    /// painted.
    previews: Vec<ChildPod>,
    active: usize,
    orientation: PreviewRailOrientation,
    highlight_active: bool,
    /// The tick under the pointer, self-corrected from `PaintCtx::is_hovered`.
    hovered: Option<usize>,
    /// The tick a press pinned open.
    pinned: Option<usize>,
    /// The tick a `Down` armed.
    captured: Option<usize>,
    /// The frame the card's entrance was first painted at.
    card_started: Option<FrameTime>,
    /// Whether a card entrance is staged.
    card_playing: bool,
    /// The tick lengths the current spring started from.
    scales_from: Option<Vec<f64>>,
    /// The frame that spring was first painted at.
    scales_started: Option<FrameTime>,
    /// The size layout resolved.
    size: Size,
    /// The card's box layout resolved.
    card_rect: Rect,
    on_select: frust::authoring::ErasedArgCallback<usize>,
}

impl PreviewRailWidget {
    /// The tick whose card is shown: the pointer's, else a pin, else nothing.
    fn displayed(&self) -> Option<usize> {
        self.hovered.or(self.pinned)
    }

    /// The tick drawn at full length: the displayed one, else the active one
    /// when `highlight_active` asks for it.
    fn highlighted(&self) -> Option<usize> {
        self.displayed().or_else(|| {
            self.highlight_active
                .then_some(self.active)
                .filter(|index| *index < self.entries.len())
        })
    }

    /// Entry `index`'s target tick length.
    fn target_scale(&self, index: usize) -> f64 {
        match self.highlighted() {
            Some(highlighted) => preview_rail_scale(highlighted.abs_diff(index)),
            // Nothing pointed at: every tick sits on the ladder's floor, which
            // is what `displayedIndex < 0` produces upstream.
            None => preview_rail_scale(usize::MAX),
        }
    }

    /// Land every tick on its target without a spring.
    fn settle_scales(&mut self) {
        for index in 0..self.entries.len() {
            self.entries[index].scale = self.target_scale(index);
        }
        self.scales_from = None;
        self.scales_started = None;
    }

    /// Start a spring from the lengths currently displayed toward the new
    /// ladder, unless nothing changed.
    fn retarget_scales(&mut self) {
        let unchanged = (0..self.entries.len())
            .all(|index| self.entries[index].scale == self.target_scale(index));
        if unchanged {
            return;
        }
        self.scales_from = Some(self.entries.iter().map(|e| e.scale).collect());
        self.scales_started = None;
    }

    /// Set the displayed tick, staging the card's entrance when it changes.
    fn set_displayed(&mut self, hovered: Option<usize>, pinned: Option<usize>) -> bool {
        let before = self.displayed();
        self.hovered = hovered;
        self.pinned = pinned;
        if self.displayed() == before {
            return false;
        }
        self.retarget_scales();
        if self.displayed().is_some() {
            self.card_playing = true;
            self.card_started = None;
        }
        true
    }

    /// The rail band's box.
    fn rail_rect(&self) -> Rect {
        let span = self.entries.len() as f64 * PREVIEW_RAIL_ITEM_SIZE;
        match self.orientation {
            PreviewRailOrientation::Vertical => {
                Rect::from_origin_size(Point::ZERO, Size::new(PREVIEW_RAIL_THICKNESS, span))
            }
            PreviewRailOrientation::Horizontal => Rect::from_origin_size(
                Point::new(0.0, (self.size.height - PREVIEW_RAIL_THICKNESS).max(0.0)),
                Size::new(span, PREVIEW_RAIL_THICKNESS),
            ),
        }
    }

    /// Entry `index`'s pointer target — upstream's `h-6 w-12` (vertical) or
    /// `h-12 w-6` (horizontal) button box.
    fn item_rect(&self, index: usize) -> Option<Rect> {
        if index >= self.entries.len() {
            return None;
        }
        let along = index as f64 * PREVIEW_RAIL_ITEM_SIZE;
        let rail = self.rail_rect();
        Some(match self.orientation {
            PreviewRailOrientation::Vertical => Rect::from_origin_size(
                Point::new(0.0, along),
                Size::new(PREVIEW_RAIL_THICKNESS, PREVIEW_RAIL_ITEM_SIZE),
            ),
            PreviewRailOrientation::Horizontal => Rect::from_origin_size(
                Point::new(along, rail.y0),
                Size::new(PREVIEW_RAIL_ITEM_SIZE, PREVIEW_RAIL_THICKNESS),
            ),
        })
    }

    /// Entry `index`'s painted tick, at its current length and with its
    /// orientation's growth origin (left, or bottom).
    fn tick_rect(&self, index: usize) -> Option<Rect> {
        let item = self.item_rect(index)?;
        let scale = self.entries[index].scale.clamp(0.0, 1.0);
        let length = PREVIEW_RAIL_THICKNESS * scale;
        Some(match self.orientation {
            PreviewRailOrientation::Vertical => Rect::from_origin_size(
                Point::new(
                    item.x0,
                    item.y0 + (PREVIEW_RAIL_ITEM_SIZE - PREVIEW_TICK_THICKNESS) / 2.0,
                ),
                Size::new(length, PREVIEW_TICK_THICKNESS),
            ),
            PreviewRailOrientation::Horizontal => Rect::from_origin_size(
                Point::new(
                    item.x0 + (PREVIEW_RAIL_ITEM_SIZE - PREVIEW_TICK_THICKNESS) / 2.0,
                    item.y1 - length,
                ),
                Size::new(PREVIEW_TICK_THICKNESS, length),
            ),
        })
    }

    /// The tick under a widget-local `pos`, if any.
    fn hit_item(&self, pos: Point) -> Option<usize> {
        (0..self.entries.len())
            .find(|index| self.item_rect(*index).is_some_and(|r| r.contains(pos)))
    }

    /// Advance the tick lengths to `now`, returning whether they are still
    /// moving.
    fn advance_scales(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        if reduce_motion {
            self.settle_scales();
            return false;
        }
        let Some(from) = self.scales_from.clone() else {
            self.settle_scales();
            return false;
        };
        let ramp = Ramp::spring(SPRING_LAYOUT);
        let started = *self.scales_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if ramp.is_settled(elapsed) {
            self.settle_scales();
            return false;
        }
        let t = ramp.progress(elapsed);
        for index in 0..self.entries.len() {
            let start = from.get(index).copied().unwrap_or(0.0);
            let target = self.target_scale(index);
            self.entries[index].scale = start + (target - start) * t;
        }
        true
    }

    /// Advance the card's entrance to `now`, returning `(alpha, rise, running)`.
    fn advance_card(&mut self, now: FrameTime, reduce_motion: bool) -> (f32, f64, bool) {
        if !self.card_playing {
            return (1.0, 0.0, false);
        }
        if reduce_motion {
            self.card_playing = false;
            self.card_started = None;
            return (1.0, 0.0, false);
        }
        let started = *self.card_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if PREVIEW_CARD_TIMING.is_settled(elapsed) {
            self.card_playing = false;
            self.card_started = None;
            return (1.0, 0.0, false);
        }
        let t = PREVIEW_CARD_TIMING.progress_clamped(elapsed);
        (t as f32, (1.0 - t) * PREVIEW_CARD_RISE, true)
    }
}

impl Widget for PreviewRailWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let style = title_style(theme);
        for entry in &mut self.entries {
            entry.title.layout(ctx, &style);
        }

        let displayed = self.displayed();
        let rail_span = self.entries.len() as f64 * PREVIEW_RAIL_ITEM_SIZE;

        // Only the displayed preview is measured; the rest keep whatever
        // geometry they last had and are never painted.
        let (card_width, card_origin_along) = match self.orientation {
            PreviewRailOrientation::Vertical => {
                let x = PREVIEW_RAIL_THICKNESS + PREVIEW_CARD_GAP;
                let available = (bc.max().width - x).max(0.0);
                (available.min(PREVIEW_CARD_MAX_WIDTH), x)
            }
            PreviewRailOrientation::Horizontal => {
                (PREVIEW_CARD_WIDTH_HORIZONTAL.min(bc.max().width), 0.0)
            }
        };

        let mut card_size = Size::ZERO;
        if let Some(index) = displayed {
            let inner = (card_width - PREVIEW_CARD_PADDING * 2.0).max(0.0);
            let body = match self.previews.get_mut(index) {
                Some(pod) => pod.layout_child(
                    ctx,
                    &BoxConstraints::new(Size::ZERO, Size::new(inner, bc.max().height)),
                ),
                None => Size::ZERO,
            };
            let title = self.entries[index].title.size();
            card_size = Size::new(
                card_width,
                PREVIEW_CARD_PADDING * 2.0
                    + title.height
                    + if body.height > 0.0 {
                        PREVIEW_CARD_TITLE_GAP + body.height
                    } else {
                        0.0
                    },
            );
        }

        let size = match self.orientation {
            PreviewRailOrientation::Vertical => Size::new(
                card_origin_along + card_width,
                rail_span.max(card_size.height),
            ),
            PreviewRailOrientation::Horizontal => Size::new(
                rail_span.max(card_size.width),
                PREVIEW_RAIL_THICKNESS
                    + if card_size.height > 0.0 {
                        card_size.height + PREVIEW_CARD_GAP
                    } else {
                        0.0
                    },
            ),
        };
        self.size = bc.constrain(size);

        // The card is centred on its own tick and clamped inside the widget.
        self.card_rect = Rect::ZERO;
        if let Some(index) = displayed
            && let Some(item) = self.item_rect(index)
        {
            let origin = match self.orientation {
                PreviewRailOrientation::Vertical => Point::new(
                    card_origin_along,
                    (item.center().y - card_size.height / 2.0)
                        .clamp(0.0, (self.size.height - card_size.height).max(0.0)),
                ),
                PreviewRailOrientation::Horizontal => Point::new(
                    (item.center().x - card_size.width / 2.0)
                        .clamp(0.0, (self.size.width - card_size.width).max(0.0)),
                    0.0,
                ),
            };
            self.card_rect = Rect::from_origin_size(origin, card_size);
            if let Some(pod) = self.previews.get_mut(index) {
                let title = self.entries[index].title.size();
                pod.set_origin(Point::new(
                    origin.x + PREVIEW_CARD_PADDING,
                    origin.y + PREVIEW_CARD_PADDING + title.height + PREVIEW_CARD_TITLE_GAP,
                ));
            }
        }

        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() && self.hovered.is_some() {
            let pinned = self.pinned;
            self.set_displayed(None, pinned);
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let origin = ctx.origin();

        let scales_running = self.advance_scales(ctx.frame_time(), reduce_motion);
        let (card_alpha, card_rise, card_running) =
            self.advance_card(ctx.frame_time(), reduce_motion);
        let highlighted = self.highlighted();

        for index in 0..self.entries.len() {
            let Some(tick) = self.tick_rect(index) else {
                continue;
            };
            scene.fill_rect(
                Point::new(origin.x + tick.x0, origin.y + tick.y0),
                Size::new(tick.width(), tick.height()),
                if highlighted == Some(index) {
                    colors.tick_active
                } else {
                    colors.tick
                },
            );
        }

        // The card: fill, hairline, title, then its body child.
        if let Some(index) = self.displayed()
            && self.card_rect.width() > 0.0
        {
            let card_origin = Point::new(
                origin.x + self.card_rect.x0,
                origin.y + self.card_rect.y0 + card_rise,
            );
            let card_size = Size::new(self.card_rect.width(), self.card_rect.height());
            if card_running {
                scene.push_layer(card_origin, card_size, card_alpha);
            }
            scene.fill_rounded_rect(card_origin, card_size, PREVIEW_CARD_RADIUS, colors.card);
            let outline = RoundedRect::from_rect(
                Rect::from_origin_size(Point::ORIGIN, card_size).inset(-style::BORDER_WIDTH / 2.0),
                PREVIEW_CARD_RADIUS,
            );
            scene.stroke_path(
                card_origin,
                &Shape::to_path(&outline, style::PATH_TOLERANCE),
                style::BORDER_WIDTH,
                &Brush::Solid(colors.border),
            );
            self.entries[index].title.paint(
                Point::new(
                    card_origin.x + PREVIEW_CARD_PADDING,
                    card_origin.y + PREVIEW_CARD_PADDING,
                ),
                colors.title,
                scene,
            );
            if let Some(pod) = self.previews.get_mut(index) {
                if card_rise != 0.0 {
                    scene.push_transform(Affine::translate((0.0, card_rise)));
                    pod.paint_child(ctx, scene);
                    scene.pop_transform();
                } else {
                    pod.paint_child(ctx, scene);
                }
            }
            if card_running {
                scene.pop_layer();
            }
        }

        // The tick lengths move within their own band and the card entrance is
        // paint-only, but the card's *presence* changes the widget's size, which
        // `set_displayed` already flagged through a redraw-and-relayout pass.
        if scales_running || card_running {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in &mut self.previews {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        // The displayed card is routed first; upstream's card is
        // `pointer-events-none`, but a frust preview body may be interactive and
        // the container ordering rule wants the child asked first either way.
        if let Some(index) = self.displayed()
            && let Some(pod) = self.previews.get_mut(index)
            && route_event_single(pod, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }

        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(index) = self.hit_item(p.position) else {
                    return EventResult::Ignored;
                };
                self.captured = Some(index);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.captured.is_some() {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                    return EventResult::Handled;
                }
                let over = self.hit_item(p.position);
                if over.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                let pinned = self.pinned;
                if self.set_displayed(over, pinned) {
                    // The card's presence changes the reported size, so this is
                    // a relayout, not only a repaint.
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Up => {
                let Some(armed) = self.captured.take() else {
                    return EventResult::Ignored;
                };
                if self.hit_item(p.position) == Some(armed) {
                    // A press pins the card so a finger, which cannot hover, can
                    // still read it; a second press on the same tick un-pins.
                    let pinned = if self.pinned == Some(armed) {
                        None
                    } else {
                        Some(armed)
                    };
                    let hovered = self.hovered;
                    self.set_displayed(hovered, pinned);
                    (self.on_select)(ctx, armed);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.captured.take().is_none() {
                    return EventResult::Ignored;
                }
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let active = self.active;
        ctx.push_container(
            Role::Navigation,
            |_| {},
            |ctx| {
                for (index, entry) in self.entries.iter().enumerate() {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(entry.label.as_str());
                        node.set_selected(index == active);
                        node.add_action(Action::Click);
                    });
                }
            },
        );
        // Only the card input can reach is published — the input-parity
        // carve-out for a container that gates input.
        if let Some(pod) = self.displayed().and_then(|index| self.previews.get(index)) {
            pod.semantics_child(ctx);
        }
    }

    visit_children!(previews);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::BezPath;
    use frust::text;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        layers: Vec<f32>,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, _t: Affine) {}
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct Picked {
        last: Option<usize>,
        count: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn items() -> Vec<PreviewRailItem<Picked>> {
        vec![
            preview_rail_item("Intro", text("What this is.")),
            preview_rail_item("Install", text("How to add it.")),
            preview_rail_item("Usage", text("How to drive it.")),
            preview_rail_item("Tokens", text("What it reads.")),
            preview_rail_item("Motion", text("How it moves.")),
            preview_rail_item("Notes", text("Everything else.")),
        ]
    }

    fn view(active: usize, orientation: PreviewRailOrientation) -> PreviewRailView<Picked> {
        preview_rail::<Picked, _>(items(), active, |s: &mut Picked, i: usize| {
            s.last = Some(i);
            s.count += 1;
        })
        .orientation(orientation)
    }

    fn build(active: usize, orientation: PreviewRailOrientation) -> PreviewRailWidget {
        let mut counter = 0u64;
        View::<Picked>::build(&view(active, orientation), &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut PreviewRailWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(600.0, 600.0)),
        )
    }

    fn laid_out(active: usize, orientation: PreviewRailOrientation) -> (PreviewRailWidget, Size) {
        let mut w = build(active, orientation);
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut PreviewRailWidget,
        size: Size,
        theme: Option<&Theme>,
        ms: f64,
    ) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut PreviewRailWidget, size: Size, event: &InputEvent, state: &mut Picked) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    /// The ladder is upstream's literal one, with a flat floor past two steps.
    #[test]
    fn the_scale_ladder_is_upstreams_own() {
        assert_eq!(preview_rail_scale(0), 1.0);
        assert_eq!(preview_rail_scale(1), 0.68);
        assert_eq!(preview_rail_scale(2), 0.44);
        assert_eq!(preview_rail_scale(3), 0.25);
        assert_eq!(preview_rail_scale(50), 0.25, "the floor is flat, not zero");
        assert_eq!(preview_rail_scale(usize::MAX), 0.25);
    }

    /// A real `RenderRoot`, the only way to get an actual hover link on the
    /// widget's path: `PaintCtx::for_test` reports none, so a direct paint would
    /// drop the pointed tick before the ladder could be read.
    struct Harness {
        root: frust_core::RenderRoot<Picked, PreviewRailView<Picked>>,
        state: Picked,
        tcx: TextContext,
    }

    const WINDOW: Size = Size::new(600.0, 600.0);

    impl Harness {
        fn new(active: usize, orientation: PreviewRailOrientation, theme: Option<Theme>) -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Picked::default(),
                tcx: TextContext::new(),
            };
            if let Some(theme) = theme {
                h.root.set_theme(Box::new(theme));
            }
            let mut logic = move |_s: &mut Picked| view(active, orientation);
            h.root.rebuild(&mut logic, &mut h.state);
            h.root.layout_with_text(WINDOW, &mut h.tcx as &mut dyn Any);
            h
        }

        fn dispatch(&mut self, event: &InputEvent) {
            self.root.event(&mut self.state, event);
        }

        /// Relayout (the card's presence changes the reported size) and paint.
        fn paint(&mut self, ms: f64) -> Recorder {
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            rec
        }

        /// Paint past the ladder spring's settle time.
        fn settled_paint(&mut self, from_ms: f64) -> Recorder {
            self.paint(from_ms);
            self.paint(from_ms + 4_000.0)
        }
    }

    /// The vertical rail's pointer target for tick `index`, from constants
    /// alone — the rail is pinned to the widget's top-left corner.
    fn vertical_target(index: usize) -> Point {
        Point::new(
            PREVIEW_RAIL_THICKNESS / 2.0,
            index as f64 * PREVIEW_RAIL_ITEM_SIZE + PREVIEW_RAIL_ITEM_SIZE / 2.0,
        )
    }

    /// The painted tick rects, in rail order.
    fn ticks(rec: &Recorder) -> Vec<Rect> {
        rec.rects
            .iter()
            .filter(|(_, s, _)| {
                s.height == PREVIEW_TICK_THICKNESS || s.width == PREVIEW_TICK_THICKNESS
            })
            .map(|(o, s, _)| Rect::from_origin_size(*o, *s))
            .collect()
    }

    /// With nothing pointed at, every tick sits on the floor; pointing at one
    /// puts it at full length and grades its neighbours outward.
    #[test]
    fn pointing_at_a_tick_grades_its_neighbours() {
        let mut h = Harness::new(0, PreviewRailOrientation::Vertical, None);
        let resting = ticks(&h.paint(0.0));
        assert_eq!(resting.len(), 6);
        assert!(
            resting
                .iter()
                .all(|t| t.width() == PREVIEW_RAIL_THICKNESS * 0.25),
            "an unpointed rail is all floor: {resting:?}"
        );

        h.dispatch(&pointer(PointerPhase::Move, vertical_target(2)));
        let lengths: Vec<f64> = ticks(&h.settled_paint(0.0))
            .iter()
            .map(|t| t.width() / PREVIEW_RAIL_THICKNESS)
            .collect();
        assert_eq!(lengths, vec![0.44, 0.68, 1.0, 0.68, 0.44, 0.25]);
    }

    /// The move between two ladders is sprung, not a jump: mid-flight the newly
    /// pointed tick is between the two lengths.
    #[test]
    fn the_ladder_springs_between_two_pointed_ticks() {
        let mut h = Harness::new(0, PreviewRailOrientation::Vertical, None);
        h.paint(0.0);
        h.dispatch(&pointer(PointerPhase::Move, vertical_target(0)));
        assert_eq!(
            ticks(&h.settled_paint(0.0))[0].width(),
            PREVIEW_RAIL_THICKNESS
        );

        h.dispatch(&pointer(PointerPhase::Move, vertical_target(2)));
        h.paint(5_000.0);
        let mid = ticks(&h.paint(5_050.0))[2].width() / PREVIEW_RAIL_THICKNESS;
        assert!(mid > 0.44 && mid < 1.0, "not mid-flight: {mid}");
        let landed = ticks(&h.settled_paint(5_050.0))[2].width();
        assert_eq!(landed, PREVIEW_RAIL_THICKNESS);
    }

    /// The tick grows from the correct edge in each orientation — left in the
    /// vertical rail, bottom in the horizontal one.
    #[test]
    fn a_tick_grows_from_its_orientations_own_origin() {
        let mut h = Harness::new(0, PreviewRailOrientation::Vertical, None);
        h.paint(0.0);
        h.dispatch(&pointer(PointerPhase::Move, vertical_target(1)));
        let bars = ticks(&h.settled_paint(0.0));
        assert!(
            bars.iter().all(|t| t.x0 == bars[0].x0),
            "vertical ticks share a left edge: {bars:?}"
        );
        assert!(bars[1].width() > bars[5].width());
        assert_eq!(bars[1].height(), PREVIEW_TICK_THICKNESS);

        // Horizontal: with no card yet, the rail is the whole widget, so the
        // target is one tick pitch along it.
        let mut h = Harness::new(0, PreviewRailOrientation::Horizontal, None);
        h.paint(0.0);
        h.dispatch(&pointer(
            PointerPhase::Move,
            Point::new(PREVIEW_RAIL_ITEM_SIZE * 1.5, PREVIEW_RAIL_THICKNESS / 2.0),
        ));
        let bars = ticks(&h.settled_paint(0.0));
        assert!(
            bars.iter().all(|t| t.y1 == bars[0].y1),
            "horizontal ticks share a bottom edge: {bars:?}"
        );
        assert!(bars[1].height() > bars[5].height());
        assert_eq!(bars[1].width(), PREVIEW_TICK_THICKNESS);
    }

    /// The card appears only for the pointed tick, carries its title, and fades
    /// and rises into place.
    #[test]
    fn the_card_appears_for_the_pointed_tick_and_fades_in() {
        let mut h = Harness::new(0, PreviewRailOrientation::Vertical, None);
        let resting = h.paint(0.0);
        assert!(resting.rrects.is_empty(), "no card with nothing pointed at");
        assert!(resting.inks.is_empty());

        h.dispatch(&pointer(PointerPhase::Move, vertical_target(3)));
        let rec = h.paint(100.0);
        assert_eq!(rec.rrects.len(), 1, "exactly one card");
        assert_eq!(rec.rrects[0].2, PREVIEW_CARD_RADIUS);
        assert_eq!(rec.layers.len(), 1, "composited while it enters");
        assert!(rec.layers[0] < 1.0, "and it starts transparent");
        // Its own title, plus the preview child's text.
        assert_eq!(rec.inks.len(), 2);

        let settled = h.paint(400.0);
        assert!(settled.layers.is_empty(), "a settled card needs no layer");
        assert_eq!(settled.rrects.len(), 1, "and the card is still there");
    }

    /// `reduce_motion` lands the ladder and the card entrance at once.
    #[test]
    fn reduce_motion_lands_the_ladder_and_the_card() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut h = Harness::new(0, PreviewRailOrientation::Vertical, Some(theme));
        h.paint(0.0);
        h.dispatch(&pointer(PointerPhase::Move, vertical_target(1)));

        let rec = h.paint(100.0);
        assert_eq!(
            ticks(&rec)[1].width(),
            PREVIEW_RAIL_THICKNESS,
            "the ladder lands at once"
        );
        assert!(rec.layers.is_empty(), "and the card arrives whole");
    }

    /// The vertical card sits to the right of the rail and is centred on its own
    /// tick; the horizontal one sits above the rail, centred on it.
    #[test]
    fn the_card_is_placed_beside_its_tick() {
        let (mut w, size) = laid_out(0, PreviewRailOrientation::Vertical);
        let mut state = Picked::default();
        let at = w.item_rect(3).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Move, at), &mut state);
        layout(&mut w);
        assert_eq!(
            w.card_rect.x0,
            PREVIEW_RAIL_THICKNESS + PREVIEW_CARD_GAP,
            "the vertical card clears the rail"
        );
        let tick = w.item_rect(3).unwrap().center().y;
        assert!(
            (w.card_rect.center().y - tick).abs() < 1.0 || w.card_rect.y0 == 0.0,
            "the card is centred on its tick, or clamped to the top"
        );

        let (mut w, _) = laid_out(0, PreviewRailOrientation::Horizontal);
        let size = layout(&mut w);
        let at = w.item_rect(3).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Move, at), &mut state);
        layout(&mut w);
        assert_eq!(w.card_rect.y0, 0.0, "the horizontal card sits on top");
        assert_eq!(w.card_rect.width(), PREVIEW_CARD_WIDTH_HORIZONTAL);
    }

    /// Leaving the rail collapses the ladder and drops the card, and the
    /// paint-time hover read is what catches a pointer that left silently.
    #[test]
    fn leaving_the_rail_drops_the_card() {
        let (mut w, size) = laid_out(0, PreviewRailOrientation::Vertical);
        let mut state = Picked::default();
        let at = w.item_rect(2).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Move, at), &mut state);
        assert_eq!(w.displayed(), Some(2));

        // `PaintCtx::for_test` reports no hover link, which is exactly the
        // silent leave the tracker contract expects a paint to catch.
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.displayed(), None);
        layout(&mut w);
        let (rec, _) = paint_at(&mut w, size, None, 5_000.0);
        assert!(rec.rrects.is_empty(), "the card went with the pointer");
        assert!(w.entries.iter().all(|e| e.scale == 0.25));
    }

    /// A press reports its index and pins the card open; pressing the same tick
    /// again un-pins it.
    #[test]
    fn a_press_reports_its_index_and_pins_the_card() {
        let (mut w, size) = laid_out(0, PreviewRailOrientation::Vertical);
        let mut state = Picked::default();
        let at = w.item_rect(4).unwrap().center();

        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.last, Some(4));
        assert_eq!(w.pinned, Some(4));
        // A pin survives the paint-time hover read, which is the whole point of
        // having one: a finger cannot hover.
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.displayed(), Some(4));

        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(w.pinned, None, "a second press un-pins");
        assert_eq!(state.count, 2);
    }

    /// `highlight_active` draws the active tick at full length with no pointer
    /// on the rail, and does not summon a card.
    #[test]
    fn highlight_active_lights_the_active_tick_without_a_card() {
        let mut counter = 0u64;
        let mut w = View::<Picked>::build(
            &view(2, PreviewRailOrientation::Vertical).highlight_active(true),
            &mut BuildCtx::new(&mut counter),
        );
        let size = layout(&mut w);
        let (rec, _) = paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.entries[2].scale, 1.0);
        assert_eq!(w.entries[1].scale, 0.68);
        assert!(rec.rrects.is_empty(), "a highlight is not a card");
        assert_eq!(w.displayed(), None);
    }
}
