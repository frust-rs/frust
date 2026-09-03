//! Ports beUI's `message-bubble` agent-interface part.
//!
//! **Source:** `components/agents/message-bubble.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `message-bubble`: *"A focused conversational surface with visual tones,
//! independent alignment, grouped messages, expandable content, and interactive
//! link or button support."*
//!
//! Upstream ships four exports from one file; three of them arrive here:
//!
//! | upstream export | here |
//! |---|---|
//! | `MessageBubble` + `MessageBubbleContent` | [`message_bubble`] — one widget; the wrapper/content split is a React styling seam, not two behaviours |
//! | `MessageBubbleGroup` | [`message_bubble_group`] |
//! | `MessageBubbleCollapsible` | **not ported** — see *Degradations* |
//!
//! # The surface is painted, not a child
//!
//! Upstream layers an `absolute inset-0 -z-10` span behind the content so the
//! background can scale independently of the text (`scale: 0.92 → 1` on the
//! surface, a plain opacity fade on the content). This port paints that surface
//! itself, from the same two lanes, rather than mounting a second child — a
//! widget that paints its own background does not need a pod to hold one.
//!
//! # Geometry
//!
//! | class | here |
//! |---|---|
//! | `rounded-2xl` | [`style::RADIUS_2XL`] |
//! | `px-3.5 py-2.5` | [`BUBBLE_PADDING_X`] / [`BUBBLE_PADDING_Y`] |
//! | `min-w-9` | [`BUBBLE_MIN_WIDTH`] |
//! | `max-w-[82%]` | [`BUBBLE_MAX_WIDTH_FRACTION`] |
//! | group `gap-1.5` / `gap-3` | [`MessageBubbleSpacing::Compact`] / [`MessageBubbleSpacing::Default`] |
//!
//! # Entrance
//!
//! `animateIn` plays two overlapping ramps, both ported:
//!
//! * the surface — `scale: 0.92 → 1` on `BUBBLE_POP`, plus a 120 ms opacity fade;
//! * the content — a 120 ms opacity fade delayed 40 ms.
//!
//! Both are [`Lane`]s, retargeted on the first paint (a build carries no clock)
//! and snapped outright under `reduce_motion`.
//!
//! # Degradations against upstream
//!
//! - **`BUBBLE_POP` is substituted.** Upstream authors a per-component
//!   `{ stiffness: 520, damping: 27, mass: 0.52 }`, which is not one of
//!   [`crate::tokens::motion`]'s six. `SPRING_PRESS` (500/30/0.6) is the nearest
//!   catalog spring and drives the pop instead.
//! - **The content reveal's 40 ms delay is folded into its ramp.** [`Lane`] has
//!   no delay seam, so the content fades over
//!   [`BUBBLE_CONTENT_MS`] + [`BUBBLE_CONTENT_DELAY_MS`] from the same instant
//!   rather than starting 40 ms late. The visible ordering is preserved
//!   (surface first, text after) with a softer boundary.
//! - **No `MessageBubbleCollapsible`.** Upstream's show-more disclosure is a
//!   `line-clamp-N` box plus a `mask-image` gradient fade. The framework's text
//!   stack exposes neither a line cap over an arbitrary child subtree nor a mask
//!   brush, so a port would be a different component wearing the same name.
//! - **No `render` slot / interactive bubble.** Upstream's `render={<button/>}`
//!   turns the bubble into a pressable. A bubble here is a passive surface; a
//!   pressable one composes [`crate::components::button`] inside it.
//! - **`layout="size"` is not ported.** Upstream animates the surface between
//!   sizes when its content grows (Motion's shared-layout). A growing bubble
//!   here relayouts in one step.

use std::time::Duration;

use frust::Theme;
use frust::authoring::{
    Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, View, Widget,
    any, build_child, rebuild_child, rebuild_children, route_event, route_event_single,
    teardown_child, visit_children,
};

use crate::motion::Ramp;
use crate::press::{Lane, stroke_outline};
use crate::style::{self, scale_alpha, with_alpha};
use crate::tokens::motion::{EASE_OUT, SPRING_PRESS};
use crate::tokens::{BEUI_LIGHT, BeuiPalette};

/// Horizontal padding inside the bubble, in logical px (`px-3.5`).
pub const BUBBLE_PADDING_X: f64 = 14.0;

/// Vertical padding inside the bubble, in logical px (`py-2.5`).
pub const BUBBLE_PADDING_Y: f64 = 10.0;

/// The bubble's smallest width, in logical px (`min-w-9`) — what keeps a
/// one-character reply from collapsing to a sliver.
pub const BUBBLE_MIN_WIDTH: f64 = 36.0;

/// The largest fraction of the row a bubble may occupy (`max-w-[82%]`).
pub const BUBBLE_MAX_WIDTH_FRACTION: f64 = 0.82;

/// The surface's entrance scale floor (`scale: 0.92`).
pub const BUBBLE_POP_SCALE: f64 = 0.92;

/// How long the content's opacity fade takes, in ms
/// (`BUBBLE_CONTENT_REVEAL.duration = 0.12`).
pub const BUBBLE_CONTENT_MS: u64 = 120;

/// Upstream's content-reveal delay, in ms (`BUBBLE_CONTENT_REVEAL.delay =
/// 0.04`). Folded into the content lane's own ramp — see the [module
/// docs](self)' *Degradations*.
pub const BUBBLE_CONTENT_DELAY_MS: u64 = 40;

/// Alpha of the `tint` variant's fill (`bg-primary/10`).
const TINT_ALPHA: f32 = 0.1;

/// Alpha of the `danger` variant's fill (`bg-destructive/10`).
const DANGER_ALPHA: f32 = 0.1;

/// Alpha multiplier on the `outline` variant's hairline (`border-border/70`).
const OUTLINE_BORDER_ALPHA: f32 = 0.7;

/// Unthemed fallback palette — every colour this component resolves has a
/// beUI-light constant behind it, so an untethered bubble still paints beUI's
/// own colours rather than the framework's neutral floor.
const FALLBACK: BeuiPalette = BEUI_LIGHT;

/// The bubble's visual tone — upstream's `MessageBubbleVariant`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageBubbleVariant {
    /// `bg-foreground text-background` — the inverted surface a sent message
    /// usually wears.
    Solid,
    /// `bg-muted` — the default.
    #[default]
    Soft,
    /// `bg-primary/10` — a wash of the ink colour.
    Tint,
    /// `border border-border/70 bg-background` — an outlined surface.
    Outline,
    /// No surface at all (`rounded-none px-0 py-0 w-full max-w-none`) — the
    /// shape a long assistant answer takes when it should read as page text.
    Ghost,
    /// `bg-destructive/10 text-destructive`.
    Danger,
}

impl MessageBubbleVariant {
    /// Every variant, in the order upstream's union declares them — the seam a
    /// catalog page or a test enumerates the set through.
    pub const ALL: [MessageBubbleVariant; 6] = [
        MessageBubbleVariant::Solid,
        MessageBubbleVariant::Soft,
        MessageBubbleVariant::Tint,
        MessageBubbleVariant::Outline,
        MessageBubbleVariant::Ghost,
        MessageBubbleVariant::Danger,
    ];

    /// Whether this variant paints a surface at all. Only
    /// [`Ghost`](MessageBubbleVariant::Ghost) does not — and it also drops the
    /// padding and the width cap.
    pub const fn has_surface(self) -> bool {
        !matches!(self, MessageBubbleVariant::Ghost)
    }
}

/// Which side of the row the bubble sits on — upstream's `MessageBubbleAlign`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageBubbleAlign {
    /// `items-start` — the leading edge (an assistant reply).
    #[default]
    Start,
    /// `items-end` — the trailing edge (a sent message).
    End,
}

/// A [`message_bubble_group`]'s row gap — upstream's `spacing` prop, the
/// grouped-consecutive spacing rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageBubbleSpacing {
    /// `gap-1.5` — consecutive bubbles from the same sender.
    #[default]
    Compact,
    /// `gap-3` — a change of sender.
    Default,
}

impl MessageBubbleSpacing {
    /// The gap this spacing resolves to, in logical px.
    pub const fn gap(self) -> f64 {
        match self {
            MessageBubbleSpacing::Compact => style::SPACING_UNIT * 1.5,
            MessageBubbleSpacing::Default => style::SPACING_UNIT * 3.0,
        }
    }
}

/// The resolved paint of one bubble surface.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BubblePaint {
    /// The surface fill, or `None` for a variant that paints none.
    fill: Option<Color>,
    /// The hairline, or `None` when the variant has no border.
    border: Option<Color>,
}

impl BubblePaint {
    /// Resolve `variant` against `theme`, falling back to beUI light.
    fn resolve(variant: MessageBubbleVariant, theme: Option<&Theme>) -> Self {
        let (ink, muted, background, border, destructive) = match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                (
                    scheme.primary,
                    scheme.surface_container_highest,
                    scheme.surface,
                    scheme.outline_variant,
                    scheme.error,
                )
            }
            None => (
                FALLBACK.primary,
                FALLBACK.muted,
                FALLBACK.background,
                FALLBACK.border,
                FALLBACK.destructive,
            ),
        };
        match variant {
            MessageBubbleVariant::Solid => BubblePaint {
                fill: Some(ink),
                border: None,
            },
            MessageBubbleVariant::Soft => BubblePaint {
                fill: Some(muted),
                border: None,
            },
            MessageBubbleVariant::Tint => BubblePaint {
                fill: Some(with_alpha(ink, TINT_ALPHA)),
                border: None,
            },
            MessageBubbleVariant::Outline => BubblePaint {
                fill: Some(background),
                border: Some(scale_alpha(border, OUTLINE_BORDER_ALPHA)),
            },
            MessageBubbleVariant::Ghost => BubblePaint {
                fill: None,
                border: None,
            },
            MessageBubbleVariant::Danger => BubblePaint {
                fill: Some(with_alpha(destructive, DANGER_ALPHA)),
                border: None,
            },
        }
    }
}

/// A declarative beUI message bubble wrapping `content`. See the [module
/// docs](self).
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::agents::message_bubble::{
///     MessageBubbleAlign, MessageBubbleVariant, message_bubble,
/// };
///
/// let sent = message_bubble::<(), _>(text("ship it"))
///     .variant(MessageBubbleVariant::Solid)
///     .align(MessageBubbleAlign::End)
///     .animate_in(true);
/// ```
pub struct MessageBubbleView<State: 'static> {
    content: AnyView<State>,
    variant: MessageBubbleVariant,
    align: MessageBubbleAlign,
    animate_in: bool,
}

/// Create a bubble around `content`, [`Soft`](MessageBubbleVariant::Soft) and
/// [`Start`](MessageBubbleAlign::Start)-aligned, with no entrance.
///
/// `animateIn` defaults to `false` upstream too: a transcript that mounts a
/// page of history should not play a page of entrances.
pub fn message_bubble<State: 'static, V: View<State>>(content: V) -> MessageBubbleView<State> {
    MessageBubbleView {
        content: any(content),
        variant: MessageBubbleVariant::default(),
        align: MessageBubbleAlign::default(),
        animate_in: false,
    }
}

impl<State: 'static> MessageBubbleView<State> {
    /// Paint `variant`'s surface instead of [`Soft`](MessageBubbleVariant::Soft).
    pub fn variant(mut self, variant: MessageBubbleVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Sit on `align`'s side of the row.
    pub fn align(mut self, align: MessageBubbleAlign) -> Self {
        self.align = align;
        self
    }

    /// Play the entrance once when this bubble mounts (`animateIn`).
    pub fn animate_in(mut self, animate_in: bool) -> Self {
        self.animate_in = animate_in;
        self
    }
}

/// The retained widget for a [`MessageBubbleView`].
pub struct MessageBubbleWidget {
    content: ChildPod,
    variant: MessageBubbleVariant,
    align: MessageBubbleAlign,
    /// Whether this bubble owes an entrance — armed at build, staged onto the
    /// lanes by the first paint (a build carries no clock).
    animate_in: bool,
    /// `0.0` un-entered .. `1.0` settled: the surface's scale and opacity.
    surface: Lane,
    /// `0.0` un-entered .. `1.0` settled: the content's opacity.
    content_reveal: Lane,
    /// Whether the entrance has been staged onto the two lanes yet.
    staged: bool,
    /// The bubble's own box (the surface), resolved by layout.
    bubble: Size,
    /// Where the bubble's leading edge sits inside the row, resolved by layout.
    bubble_x: f64,
}

impl MessageBubbleWidget {
    /// The tone this bubble paints.
    pub fn variant(&self) -> MessageBubbleVariant {
        self.variant
    }

    /// Which side of the row it sits on.
    pub fn align(&self) -> MessageBubbleAlign {
        self.align
    }

    /// The surface's box, as of the last layout — what upstream's
    /// `max-w-[82%]`/`min-w-9` pair resolved to.
    pub fn bubble_size(&self) -> Size {
        self.bubble
    }

    /// The surface's leading edge inside the row, as of the last layout.
    pub fn bubble_x(&self) -> f64 {
        self.bubble_x
    }

    /// How far through its entrance the surface is; `1.0` for a bubble that
    /// never had one.
    pub fn surface_progress(&self) -> f64 {
        self.surface.value()
    }

    /// How far through its entrance the content is.
    pub fn content_progress(&self) -> f64 {
        self.content_reveal.value()
    }

    /// The inner padding this variant carries — `Ghost` has none.
    fn padding(&self) -> (f64, f64) {
        if self.variant.has_surface() {
            (BUBBLE_PADDING_X, BUBBLE_PADDING_Y)
        } else {
            (0.0, 0.0)
        }
    }

    /// Stage the entrance onto the lanes the first time a clock is available.
    /// A bubble built without `animateIn` — or under `reduce_motion` — snaps
    /// both lanes home instead, so nothing plays.
    fn stage(&mut self, reduce_motion: bool) {
        if self.staged {
            return;
        }
        self.staged = true;
        self.surface.retarget(1.0);
        self.content_reveal.retarget(1.0);
        if !self.animate_in || reduce_motion {
            self.surface.snap();
            self.content_reveal.snap();
        }
    }
}

impl<State: 'static> View<State> for MessageBubbleView<State> {
    type Element = MessageBubbleWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MessageBubbleWidget {
        // Both lanes rest where the entrance starts, and are retargeted on the
        // first paint; a bubble with no entrance rests at 1 and is snapped
        // there instead.
        let start = if self.animate_in { 0.0 } else { 1.0 };
        MessageBubbleWidget {
            content: build_child(&self.content, ctx),
            variant: self.variant,
            align: self.align,
            animate_in: self.animate_in,
            surface: Lane::at_rest(Ramp::spring(SPRING_PRESS), start),
            content_reveal: Lane::at_rest(
                Ramp::eased(
                    Duration::from_millis(BUBBLE_CONTENT_MS + BUBBLE_CONTENT_DELAY_MS),
                    EASE_OUT,
                ),
                start,
            ),
            staged: false,
            bubble: Size::ZERO,
            bubble_x: 0.0,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MessageBubbleWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        if element.variant != self.variant {
            // `Ghost` and the rest differ in padding, so a tone swap across
            // that boundary can move geometry.
            let geometry = element.variant.has_surface() != self.variant.has_surface();
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
            if geometry {
                flags |= ChangeFlags::LAYOUT;
            }
        }
        if element.align != self.align {
            element.align = self.align;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // `animateIn` is a mount-time decision: it is recorded so a bubble
        // built before its first paint still honours a change, but a bubble
        // that has already staged never replays (see `stage`).
        element.animate_in = self.animate_in;
        flags
    }

    fn teardown(&self, element: &mut MessageBubbleWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for MessageBubbleWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let row_width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let (pad_x, pad_y) = self.padding();

        // `Ghost` is `w-full max-w-none`; every other variant caps at 82% of
        // the row and floors at `min-w-9`.
        let cap = if self.variant.has_surface() {
            (row_width * BUBBLE_MAX_WIDTH_FRACTION).max(0.0)
        } else {
            row_width
        };
        let inner_max = (cap - pad_x * 2.0).max(0.0);
        let child_bc =
            BoxConstraints::new(Size::new(0.0, 0.0), Size::new(inner_max, f64::INFINITY));
        let content = self.content.layout_child(ctx, &child_bc);

        let bubble_width = if self.variant.has_surface() {
            (content.width + pad_x * 2.0)
                .max(BUBBLE_MIN_WIDTH)
                .min(cap.max(BUBBLE_MIN_WIDTH))
        } else {
            row_width
        };
        let bubble_height = content.height + pad_y * 2.0;
        self.bubble = Size::new(bubble_width, bubble_height);
        self.bubble_x = match self.align {
            MessageBubbleAlign::Start => 0.0,
            MessageBubbleAlign::End => (row_width - bubble_width).max(0.0),
        };
        self.content
            .set_origin(Point::new(self.bubble_x + pad_x, pad_y));
        bc.constrain(Size::new(row_width, bubble_height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (reduce_motion, paint) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                BubblePaint::resolve(self.variant, theme),
            )
        };
        self.stage(reduce_motion);

        let now = ctx.frame_time();
        let mut owes_frame = self.surface.advance(now);
        owes_frame |= self.content_reveal.advance(now);

        let origin = ctx.origin();
        let surface_progress = self.surface.value().clamp(0.0, 1.0);
        let content_alpha = self.content_reveal.value().clamp(0.0, 1.0);

        if let Some(fill) = paint.fill {
            // The surface scales about the bubble's own bottom corner on the
            // side it is aligned to (`origin-bottom-left`/`origin-bottom-right`).
            let scale = BUBBLE_POP_SCALE + (1.0 - BUBBLE_POP_SCALE) * self.surface.value();
            let pivot_x = origin.x
                + self.bubble_x
                + match self.align {
                    MessageBubbleAlign::Start => 0.0,
                    MessageBubbleAlign::End => self.bubble.width,
                };
            let pivot_y = origin.y + self.bubble.height;
            scene.push_transform(
                Affine::translate((pivot_x, pivot_y))
                    * Affine::scale(scale)
                    * Affine::translate((-pivot_x, -pivot_y)),
            );
            let radius =
                style::resolve_radius(style::RADIUS_2XL, self.bubble.width, self.bubble.height);
            let at = Point::new(origin.x + self.bubble_x, origin.y);
            scene.fill_rounded_rect(
                at,
                self.bubble,
                radius,
                scale_alpha(fill, surface_progress as f32),
            );
            if let Some(border) = paint.border {
                stroke_outline(
                    scene,
                    at,
                    self.bubble,
                    radius,
                    scale_alpha(border, surface_progress as f32),
                );
            }
            scene.pop_transform();
        }

        if content_alpha >= 1.0 {
            self.content.paint_child(ctx, scene);
        } else if content_alpha > 0.0 {
            scene.push_layer(origin, ctx.size(), content_alpha as f32);
            self.content.paint_child(ctx, scene);
            scene.pop_layer();
        }

        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A bubble is a passive surface; whatever it wraps need not be.
        route_event_single(&mut self.content, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(Role::Group, |_| {}, |ctx| self.content.semantics_child(ctx));
    }

    visit_children!(content);
}

// ---- The group -------------------------------------------------------------

/// A column of bubbles at one spacing — upstream's `MessageBubbleGroup`.
pub struct MessageBubbleGroupView<State: 'static> {
    bubbles: Vec<AnyView<State>>,
    spacing: MessageBubbleSpacing,
}

/// Stack `bubbles` in a column, [`Compact`](MessageBubbleSpacing::Compact)
/// apart — the spacing consecutive messages from one sender wear.
pub fn message_bubble_group<State: 'static>(
    bubbles: Vec<AnyView<State>>,
) -> MessageBubbleGroupView<State> {
    MessageBubbleGroupView {
        bubbles,
        spacing: MessageBubbleSpacing::default(),
    }
}

impl<State: 'static> MessageBubbleGroupView<State> {
    /// Space the column at `spacing` instead.
    pub fn spacing(mut self, spacing: MessageBubbleSpacing) -> Self {
        self.spacing = spacing;
        self
    }
}

/// The retained widget for a [`MessageBubbleGroupView`].
pub struct MessageBubbleGroupWidget {
    bubbles: Vec<ChildPod>,
    spacing: MessageBubbleSpacing,
}

impl MessageBubbleGroupWidget {
    /// The gap between rows, in logical px.
    pub fn gap(&self) -> f64 {
        self.spacing.gap()
    }
}

impl<State: 'static> View<State> for MessageBubbleGroupView<State> {
    type Element = MessageBubbleGroupWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MessageBubbleGroupWidget {
        MessageBubbleGroupWidget {
            bubbles: self.bubbles.iter().map(|v| build_child(v, ctx)).collect(),
            spacing: self.spacing,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MessageBubbleGroupWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_children(
            &prev.bubbles,
            &self.bubbles,
            &mut element.bubbles,
            ctx,
            |v| v,
            |_| None,
        );
        if element.spacing != self.spacing {
            element.spacing = self.spacing;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut MessageBubbleGroupWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.bubbles.iter().zip(element.bubbles.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for MessageBubbleGroupWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let child_bc = BoxConstraints::new(Size::new(width, 0.0), Size::new(width, f64::INFINITY));
        let gap = self.spacing.gap();
        let mut y = 0.0_f64;
        for (index, pod) in self.bubbles.iter_mut().enumerate() {
            if index > 0 {
                y += gap;
            }
            let size = pod.layout_child(ctx, &child_bc);
            pod.set_origin(Point::new(0.0, y));
            y += size.height;
        }
        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.bubbles {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.bubbles, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Group,
            |_| {},
            |ctx| {
                for pod in &self.bubbles {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(bubbles);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::{FrameTime, text};
    use std::any::Any;

    /// The row every bubble test lays itself into.
    const ROW: Size = Size::new(400.0, 200.0);

    /// Records every rounded-rect fill, transform push and alpha layer.
    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        transforms: Vec<Affine>,
        layers: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
            self.rounded.push((origin, size, color));
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {}
    }

    fn laid_out(view: &MessageBubbleView<()>) -> MessageBubbleWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(ROW));
        widget
    }

    fn painted(
        widget: &mut MessageBubbleWidget,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, ROW, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame())
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// Every tone upstream's union declares is constructible, and only `Ghost`
    /// paints no surface.
    #[test]
    fn every_variant_is_constructible_and_only_ghost_paints_no_surface() {
        assert_eq!(MessageBubbleVariant::ALL.len(), 6);
        for variant in MessageBubbleVariant::ALL {
            let mut widget = laid_out(&message_bubble::<(), _>(text("hi")).variant(variant));
            let (rec, _) = painted(&mut widget, 0, None);
            assert_eq!(widget.variant(), variant);
            assert_eq!(
                rec.rounded.is_empty(),
                !variant.has_surface(),
                "{variant:?} surface presence"
            );
        }
    }

    /// `max-w-[82%]` and `min-w-9`: a long message caps at 82% of the row, a
    /// one-character one floors at 36px, and `Ghost` obeys neither.
    #[test]
    fn the_surface_caps_at_82_percent_and_floors_at_the_min_width() {
        let long = laid_out(&message_bubble::<(), _>(text(
            "a message long enough that it has to wrap and therefore reaches the width cap",
        )));
        assert!(
            long.bubble_size().width <= ROW.width * BUBBLE_MAX_WIDTH_FRACTION + 1e-6,
            "capped: {}",
            long.bubble_size().width
        );

        let tiny = laid_out(&message_bubble::<(), _>(text("k")));
        assert!(
            tiny.bubble_size().width >= BUBBLE_MIN_WIDTH,
            "floored: {}",
            tiny.bubble_size().width
        );

        let ghost =
            laid_out(&message_bubble::<(), _>(text("k")).variant(MessageBubbleVariant::Ghost));
        assert_eq!(
            ghost.bubble_size().width,
            ROW.width,
            "`Ghost` is `w-full max-w-none`"
        );
    }

    /// `items-end` puts the bubble's trailing edge on the row's trailing edge;
    /// `items-start` leaves it at the leading one.
    #[test]
    fn align_end_pushes_the_bubble_to_the_trailing_edge() {
        let start = laid_out(&message_bubble::<(), _>(text("hi")));
        assert_eq!(start.bubble_x(), 0.0);

        let end = laid_out(&message_bubble::<(), _>(text("hi")).align(MessageBubbleAlign::End));
        assert!(end.bubble_x() > 0.0);
        assert!(
            (end.bubble_x() + end.bubble_size().width - ROW.width).abs() < 1e-6,
            "flush with the trailing edge"
        );
    }

    /// A bubble built without `animateIn` is settled on its first paint and
    /// asks for no further frames — what keeps a page of history from playing
    /// a page of entrances.
    #[test]
    fn a_bubble_without_animate_in_paints_settled_and_owes_no_frame() {
        let mut widget = laid_out(&message_bubble::<(), _>(text("hi")));
        let (rec, needs_frame) = painted(&mut widget, 0, None);
        assert!(!needs_frame);
        assert_eq!(widget.surface_progress(), 1.0);
        assert_eq!(widget.content_progress(), 1.0);
        assert!(
            rec.layers.is_empty(),
            "settled content needs no alpha layer"
        );
    }

    /// `animateIn` runs both lanes from zero to settled, requesting frames the
    /// whole way and stopping once it lands.
    #[test]
    fn animate_in_runs_both_lanes_and_settles() {
        let mut widget = laid_out(&message_bubble::<(), _>(text("hi")).animate_in(true));
        let (_, needs_frame) = painted(&mut widget, 0, None);
        assert!(needs_frame, "the entrance owes frames");
        assert!(widget.surface_progress() < 1.0);

        let (rec, _) = painted(&mut widget, 40, None);
        let mid = widget.surface_progress();
        assert!(mid > 0.0 && mid < 1.0, "mid-flight: {mid}");
        assert!(
            rec.layers.iter().any(|alpha| *alpha < 1.0),
            "mid-entrance content composites through a partial layer"
        );
        assert!(
            !rec.transforms.is_empty(),
            "the surface scales about its own corner"
        );

        let (_, needs_frame) = painted(&mut widget, 5_000, None);
        assert!(!needs_frame, "a settled entrance stops asking for frames");
        assert_eq!(widget.surface_progress(), 1.0);
        assert_eq!(widget.content_progress(), 1.0);
    }

    /// `reduce_motion` collapses the entrance: the first paint is already
    /// settled and owes nothing.
    #[test]
    fn reduce_motion_lands_the_entrance_on_the_first_paint() {
        let theme = reduced();
        let mut widget = laid_out(&message_bubble::<(), _>(text("hi")).animate_in(true));
        let (_, needs_frame) = painted(&mut widget, 0, Some(&theme));
        assert!(!needs_frame);
        assert_eq!(widget.surface_progress(), 1.0);
        assert_eq!(widget.content_progress(), 1.0);
    }

    /// A redundant rebuild changes nothing, and a rebuild that flips
    /// `animateIn` on a bubble that has already painted does not replay its
    /// entrance.
    #[test]
    fn a_rebuild_never_replays_a_settled_entrance() {
        let view = message_bubble::<(), _>(text("hi"));
        let mut widget = laid_out(&view);
        painted(&mut widget, 0, None);

        let mut next_id = 0u64;
        let flags =
            View::<()>::rebuild(&view, &view, &mut widget, &mut BuildCtx::new(&mut next_id));
        assert_eq!(flags, ChangeFlags::NONE);

        let animated = message_bubble::<(), _>(text("hi")).animate_in(true);
        View::<()>::rebuild(
            &animated,
            &view,
            &mut widget,
            &mut BuildCtx::new(&mut next_id),
        );
        let (_, needs_frame) = painted(&mut widget, 1, None);
        assert!(!needs_frame, "a staged bubble does not re-enter");
        assert_eq!(widget.surface_progress(), 1.0);
    }

    /// A tone swap repaints; a swap that crosses the `Ghost` boundary also
    /// relayouts, because `Ghost` drops the padding.
    #[test]
    fn a_variant_swap_relayouts_only_across_the_ghost_boundary() {
        let soft = message_bubble::<(), _>(text("hi"));
        let tint = message_bubble::<(), _>(text("hi")).variant(MessageBubbleVariant::Tint);
        let ghost = message_bubble::<(), _>(text("hi")).variant(MessageBubbleVariant::Ghost);
        let mut widget = laid_out(&soft);
        let mut next_id = 0u64;

        let flags =
            View::<()>::rebuild(&tint, &soft, &mut widget, &mut BuildCtx::new(&mut next_id));
        assert!(flags.contains(ChangeFlags::PAINT));
        assert!(!flags.contains(ChangeFlags::LAYOUT));

        let flags =
            View::<()>::rebuild(&ghost, &tint, &mut widget, &mut BuildCtx::new(&mut next_id));
        assert!(flags.contains(ChangeFlags::LAYOUT));
    }

    /// The group's two spacings are upstream's `gap-1.5` / `gap-3`, and the
    /// column stacks its rows exactly that far apart.
    #[test]
    fn the_group_stacks_its_rows_at_the_declared_gap() {
        assert_eq!(MessageBubbleSpacing::Compact.gap(), 6.0);
        assert_eq!(MessageBubbleSpacing::Default.gap(), 12.0);

        for spacing in [MessageBubbleSpacing::Compact, MessageBubbleSpacing::Default] {
            let view = message_bubble_group::<()>(vec![
                any(message_bubble::<(), _>(text("one"))),
                any(message_bubble::<(), _>(text("two"))),
            ])
            .spacing(spacing);
            let mut next_id = 0u64;
            let mut widget = View::<()>::build(&view, &mut BuildCtx::new(&mut next_id));
            let mut text_ctx = TextContext::new();
            let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
            widget.layout(&mut layout, &BoxConstraints::loose(ROW));
            assert_eq!(widget.gap(), spacing.gap());

            let mut rows: Vec<(f64, f64)> = Vec::new();
            Widget::visit_children(&widget, &mut |pod| {
                rows.push((pod.origin().y, pod.size().height));
            });
            assert_eq!(rows.len(), 2);
            assert!(
                (rows[1].0 - (rows[0].0 + rows[0].1) - spacing.gap()).abs() < 1e-6,
                "second row sits one gap below the first: {rows:?}"
            );
        }
    }

    /// The tone table: every variant resolves its own treatment, and the
    /// unthemed fallback is beUI's own light palette rather than black.
    #[test]
    fn each_variant_resolves_its_own_surface_treatment() {
        let solid = BubblePaint::resolve(MessageBubbleVariant::Solid, None);
        let soft = BubblePaint::resolve(MessageBubbleVariant::Soft, None);
        let tint = BubblePaint::resolve(MessageBubbleVariant::Tint, None);
        let outline = BubblePaint::resolve(MessageBubbleVariant::Outline, None);
        let ghost = BubblePaint::resolve(MessageBubbleVariant::Ghost, None);
        let danger = BubblePaint::resolve(MessageBubbleVariant::Danger, None);

        assert_eq!(solid.fill, Some(FALLBACK.primary));
        assert_eq!(soft.fill, Some(FALLBACK.muted));
        assert_eq!(tint.fill, Some(with_alpha(FALLBACK.primary, TINT_ALPHA)));
        assert_eq!(outline.fill, Some(FALLBACK.background));
        assert!(outline.border.is_some(), "only `outline` has a hairline");
        assert_eq!(ghost.fill, None);
        assert_eq!(
            danger.fill,
            Some(with_alpha(FALLBACK.destructive, DANGER_ALPHA))
        );
        assert!(solid.border.is_none());

        // Themed, the same roles resolve through the scheme.
        let theme = crate::theme();
        let themed = BubblePaint::resolve(MessageBubbleVariant::Solid, Some(&theme));
        assert_eq!(themed.fill, Some(theme.scheme().primary));
    }

    /// The bubble publishes exactly one child pod — whatever it was handed.
    #[test]
    fn the_bubble_publishes_its_content_pod() {
        let widget = laid_out(&message_bubble::<(), _>(text("hi")));
        let mut seen = 0;
        Widget::visit_children(&widget, &mut |_| seen += 1);
        assert_eq!(seen, 1);
    }
}
