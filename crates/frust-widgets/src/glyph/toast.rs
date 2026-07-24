//! Glyph toast + host/queue (task 21, glyph-design-system): a status-dot +
//! message bubble ([`Toast`]), and [`ToastHost`] — the retained overlay
//! widget that plays a queue of toast requests one at a time with authored
//! enter/hold/exit motion and a 2.4s auto-dismiss.
//!
//! # Toast (the leaf)
//!
//! [`ToastView`]/[`ToastWidget`]: a status dot + message row painted on the
//! Glyph "overlay" surface role (`colors.surface_container_highest` — the
//! same `bg-overlay` token [`super::progress`]'s track uses; RESEARCH §1.1
//! doesn't name toast explicitly, but it *is* an overlay surface, so this
//! reuses the role rather than inventing a new one), `shape.medium` corner
//! radius (Glyph's canonical `--radius-md` = 10px — this task's own literal
//! "radius-md"), and the Glyph toast elevation recipe
//! (`elevation.level4.shadow(brightness)` — exactly `0 12px 32px` dark /
//! `rgba(...,.12)` light, RESEARCH §1.1b/§1.3's cited toast shadow). The dot
//! color follows [`ToastVariant`]: `Plain` (default) is the accent
//! (`colors.primary`); `Info`/`Success`/`Warning` resolve `StatusPalette`
//! (Glyph's success/warning/info extension, task 15); `Error` resolves
//! `colors.error` (a first-class `ColorScheme` role). Message color is
//! `colors.on_surface` — the same label-color simplification
//! `material::chips` documents (not the more precise
//! `on_surface_container_highest`, which `TextView` has no themed role for
//! yet).
//!
//! # ToastHost + queue policy
//!
//! [`ToastHostView`] takes a `Vec<String>` FIFO snapshot (the app's own
//! growing request log — see [`show`]) every rebuild; [`ToastHostWidget`]
//! retains a `next_index` cursor plus at most one [`ActiveToast`] — **v1
//! policy: exactly one visible toast at a time, queued FIFO**, never
//! stacked. `rebuild` is where all queue bookkeeping happens (building/
//! tearing down the active [`ChildPod`] needs a [`frust_core::BuildCtx`],
//! unavailable from `layout`/`paint`): a `Done` active toast is torn down and
//! `next_index` advances; if nothing is active and a next request exists, it
//! starts fresh. The host never mutates the *view-supplied* queue itself
//! (the controlled-component idiom `docs/CODE_STANDARDS.md` names) — it only
//! reads a fresh snapshot each rebuild and clamps its own cursor to it.
//!
//! **Known v1 limitation:** since the queue is the app's own ever-growing
//! `Vec`, a very long-running session accumulates every message ever shown.
//! An app that cares can periodically truncate the vec — `next_index` is
//! clamped to the (possibly shorter) new length on the following rebuild, so
//! this is always safe, just not automatic.
//!
//! # Anchoring (framework-side positioning)
//!
//! [`ToastHostWidget::layout`] fills the incoming max constraints (a
//! [`crate::Stack`] loosens its children, so the host spans the whole stack)
//! rather than sizing to the active toast's own content, and positions the
//! toast internally per [`ToastAnchor`] (`.anchor()` on [`ToastHostView`],
//! default [`ToastAnchor::BottomCenter`]) — apps no longer wrap the host in
//! their own `Align`. Margins are [`EDGE_MARGIN`] (10px) from the anchored
//! edges, additionally widened by [`frust_core::WindowInsets::padding`] on
//! those same edges (`ctx.window_insets()`, **not** `view_insets` — IME
//! avoidance stays out of scope for v1) so a bottom toast clears gesture-nav
//! insets with no app-side `SafeArea`. The host itself stays
//! **event-transparent**: it never overrides `Widget::event`, so the
//! default no-op (`EventResult::Ignored`) lets every pointer event — inside
//! or outside the toast's own (smaller) rect — pass through to whatever the
//! host overlays. Enter/exit motion follows the anchored edge: bottom
//! anchors rise from below (a positive slide offset settling to zero);
//! top anchors drop from above (the pre-existing negative-offset motion) —
//! see [`ToastAnchor::is_top`].
//!
//! # Timeline
//!
//! Enter: 220ms (`MotionScheme::durations.base`) with the Glyph spatial
//! easing (`Cubic(0.34, 1.35, 0.64, 1.0)` — an authored overshoot); a
//! vertical slide from [`ENTER_OFFSET_FRACTION`] (140%) of the toast's own
//! height above its resting position, plus a fade. Hold: exactly
//! [`HOLD_DURATION`] (2.4s — RESEARCH §1.4 pattern 08's authored
//! auto-dismiss value), advanced via **`FrameTime` differencing, never
//! `Instant::now()`** (`docs/CODE_STANDARDS.md`'s No-`Instant::now()` rule).
//! Exit: 150ms (`durations.fast`) with the Glyph exit easing
//! (`Cubic(0.4, 0.0, 1.0, 1.0)`), fading/sliding back out. `reduce_motion`
//! collapses both enter and exit to a single fast (120ms) linear crossfade
//! (no slide) — the Details' "toast becomes fast crossfade" rule, mirroring
//! `nav::transition::resolve_spec`'s reduce-motion collapse.
//!
//! All three durations/easings are **exact source values** (`MotionScheme::glyph`'s
//! doc comment; RESEARCH §1.4), not approximations — only
//! [`ENTER_OFFSET_FRACTION`] (the slide distance) is this task's own choice.

use std::time::Duration;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, FrameTime, LayoutCtx,
    PaintCtx, PaintScene, SemanticsCtx, View, Widget, WindowEdgeInsets, any,
};
use frust_theme::{StatusPalette, Theme};
use kurbo::{Affine, Point, Size};
use peniko::Color;

use crate::Timing;
use crate::nav::transition::{TransitionDriver, make_driver};

// ---------------------------------------------------------------------
// Toast (the leaf)
// ---------------------------------------------------------------------

/// A toast's status-dot color category. See the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToastVariant {
    /// The plain accent dot (default) — no particular status.
    #[default]
    Plain,
    Info,
    Success,
    Warning,
    Error,
}

/// The row height floor, in logical px (a one-line toast never shrinks below
/// this). This task's own choice.
const MIN_HEIGHT: f64 = 44.0;
/// Horizontal padding around the content row. This task's own choice.
const PAD_X: f64 = 14.0;
/// Vertical padding above/below the content when the message wraps taller
/// than [`MIN_HEIGHT`] would otherwise need. This task's own choice.
const PAD_Y: f64 = 10.0;
/// Gap between the status dot and the message text. This task's own choice.
const DOT_GAP: f64 = 10.0;
/// Status-dot diameter. This task's own choice.
const DOT_DIAMETER: f64 = 8.0;

/// Unthemed corner-radius fallback — Glyph's canonical `--radius-md` (10px).
const FALLBACK_RADIUS: f64 = 10.0;
/// Unthemed surface fallback — Glyph dark's `bg-overlay` (`#272d3d`), the
/// same role [`super::progress`]'s track uses.
const FALLBACK_SURFACE: Color = Color::from_rgb8(0x27, 0x2d, 0x3d);
/// Unthemed dot fallbacks, one per [`ToastVariant`] — Glyph dark's amber
/// accent / cyan info / success / warning / error tokens (RESEARCH §1.1).
const FALLBACK_PLAIN: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
const FALLBACK_INFO: Color = Color::from_rgb8(0x5e, 0xc8, 0xd8);
const FALLBACK_SUCCESS: Color = Color::from_rgb8(0x5f, 0xd8, 0x8f);
const FALLBACK_WARNING: Color = Color::from_rgb8(0xf5, 0xc8, 0x60);
const FALLBACK_ERROR: Color = Color::from_rgb8(0xff, 0x6b, 0x6b);
/// Unthemed shadow fallbacks — Glyph dark's exact toast recipe (`0 12px 32px
/// rgba(0,0,0,.4)`, RESEARCH §1.3).
const FALLBACK_SHADOW_Y: f64 = 12.0;
const FALLBACK_SHADOW_BLUR: f64 = 32.0;
const FALLBACK_SHADOW_ALPHA: f32 = 0.40;

/// Return `color` with its alpha channel replaced by `alpha` (mirrors the
/// per-module helper of the same shape used across `frust-widgets`).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// A declarative Glyph toast. See the [module docs](self).
pub struct ToastView {
    message: String,
    variant: ToastVariant,
}

/// Create a toast with the given message (default [`ToastVariant::Plain`]).
pub fn toast(message: impl Into<String>) -> ToastView {
    ToastView {
        message: message.into(),
        variant: ToastVariant::Plain,
    }
}

/// PascalCase alias for [`toast`], matching the widget-fn vocabulary
/// (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn Toast(message: impl Into<String>) -> ToastView {
    toast(message)
}

impl ToastView {
    /// Set the status-dot [`ToastVariant`] (default [`ToastVariant::Plain`]).
    pub fn variant(mut self, variant: ToastVariant) -> Self {
        self.variant = variant;
        self
    }
}

/// Build the type-erased message-label view, shared by build/rebuild/
/// teardown so it stays consistent (mirrors `material::chips`' analogous
/// `assist_label_view` helper).
fn message_view<State: 'static>(message: String) -> AnyView<State> {
    any::<State, _>(crate::text::text(message))
}

impl<State: 'static> View<State> for ToastView {
    type Element = ToastWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ToastWidget {
        ToastWidget {
            message: crate::build_child::<State>(&message_view::<State>(self.message.clone()), ctx),
            message_text: self.message.clone(),
            variant: self.variant,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ToastWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.message != self.message {
            element.message_text = self.message.clone();
            let prev_view = message_view::<State>(prev.message.clone());
            let next_view = message_view::<State>(self.message.clone());
            flags |= crate::rebuild_child(&prev_view, &next_view, &mut element.message, ctx);
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut ToastWidget, ctx: &mut BuildCtx<'_>) {
        let view = message_view::<State>(self.message.clone());
        crate::teardown_child(&view, &mut element.message, ctx);
    }
}

/// The retained widget for a [`ToastView`]. See the [module docs](self).
pub struct ToastWidget {
    message: ChildPod,
    message_text: String,
    variant: ToastVariant,
}

impl ToastWidget {
    /// The status-dot color for `variant`: `Plain`/`Error` resolve directly
    /// from `ColorScheme`; `Info`/`Success`/`Warning` resolve the themed
    /// [`StatusPalette`] extension (falling back to `colors.primary` if a
    /// theme carries none — every shipping baseline attaches one, so this is
    /// only reachable for a hand-built `Theme`).
    fn resolve_dot_color(theme: Option<&Theme>, variant: ToastVariant) -> Color {
        match theme {
            Some(t) => match variant {
                ToastVariant::Plain => t.scheme().primary,
                ToastVariant::Error => t.scheme().error,
                ToastVariant::Info | ToastVariant::Success | ToastVariant::Warning => t
                    .extension::<StatusPalette>()
                    .map(|p| {
                        let c = p.colors(t.brightness);
                        match variant {
                            ToastVariant::Info => c.info,
                            ToastVariant::Success => c.success,
                            ToastVariant::Warning => c.warning,
                            _ => unreachable!(),
                        }
                    })
                    .unwrap_or(t.scheme().primary),
            },
            None => match variant {
                ToastVariant::Plain => FALLBACK_PLAIN,
                ToastVariant::Info => FALLBACK_INFO,
                ToastVariant::Success => FALLBACK_SUCCESS,
                ToastVariant::Warning => FALLBACK_WARNING,
                ToastVariant::Error => FALLBACK_ERROR,
            },
        }
    }

    /// The surface fill: themed `colors.surface_container_highest`, or the
    /// unthemed [`FALLBACK_SURFACE`]. The message row's own color comes from
    /// `TextView`'s default `ThemeTextColor::OnSurface` role (see the
    /// [module docs](self)) — not resolved here.
    fn resolve_surface_color(theme: Option<&Theme>) -> Color {
        match theme {
            Some(t) => t.scheme().surface_container_highest,
            None => FALLBACK_SURFACE,
        }
    }

    /// The `(y_offset, blur_std_dev, color)` shadow triple: themed
    /// `elevation.level4.shadow(brightness)` (Glyph's exact toast recipe), or
    /// the unthemed [`FALLBACK_SHADOW_Y`]/[`FALLBACK_SHADOW_BLUR`]/black-at-
    /// [`FALLBACK_SHADOW_ALPHA`].
    fn resolve_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
        match theme {
            Some(t) => {
                let shadow = t.elevation.level4.shadow(t.brightness);
                (
                    shadow.y_offset,
                    shadow.blur_std_dev,
                    with_alpha(t.scheme().shadow, shadow.color_alpha),
                )
            }
            None => (
                FALLBACK_SHADOW_Y,
                FALLBACK_SHADOW_BLUR,
                with_alpha(Color::from_rgb8(0, 0, 0), FALLBACK_SHADOW_ALPHA),
            ),
        }
    }
}

impl Widget for ToastWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inner_max = Size::new(f64::INFINITY, f64::INFINITY);
        let text_size = self
            .message
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        let content_h = text_size.height.max(DOT_DIAMETER);
        let height = MIN_HEIGHT.max(content_h + PAD_Y * 2.0);
        self.message.set_origin(Point::new(
            PAD_X + DOT_DIAMETER + DOT_GAP,
            (height - text_size.height) / 2.0,
        ));
        let width = PAD_X * 2.0 + DOT_DIAMETER + DOT_GAP + text_size.width;
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let surface = Self::resolve_surface_color(theme);
        let radius = theme.map(|t| t.shape.medium).unwrap_or(FALLBACK_RADIUS);
        let (shadow_y, shadow_blur, shadow_color) = Self::resolve_shadow(theme);
        let size = ctx.size();
        let origin = ctx.origin();

        scene.draw_shadow(
            Point::new(origin.x, origin.y + shadow_y),
            size,
            radius,
            shadow_blur,
            shadow_color,
        );
        scene.fill_rounded_rect(origin, size, radius, surface);

        let dot_color = Self::resolve_dot_color(theme, self.variant);
        let dot_cy = origin.y + size.height / 2.0;
        let dot_cx = origin.x + PAD_X + DOT_DIAMETER / 2.0;
        scene.fill_rounded_rect(
            Point::new(dot_cx - DOT_DIAMETER / 2.0, dot_cy - DOT_DIAMETER / 2.0),
            Size::new(DOT_DIAMETER, DOT_DIAMETER),
            DOT_DIAMETER / 2.0,
            dot_color,
        );

        self.message.paint_child(ctx, scene);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Status, |node| {
            node.set_label(self.message_text.as_str());
        });
    }
}

// ---------------------------------------------------------------------
// ToastHost + queue
// ---------------------------------------------------------------------

/// Push `message` onto a toast request queue (FIFO) — pair with
/// [`toast_host`]'s queue snapshot (see the [module docs](self)). A plain,
/// uncomplicated `push` — the app owns the `Vec<String>` (e.g. a
/// `Component::State` field).
pub fn show(queue: &mut Vec<String>, message: impl Into<String>) {
    queue.push(message.into());
}

/// Enter duration (`durations.base`, exact Glyph source value — see the
/// [module docs](self)).
const ENTER_DURATION: Duration = Duration::from_millis(220);
/// Enter easing (Glyph's `spatial` cubic-bezier, exact source value).
const ENTER_CURVE: Curve = Curve::Cubic(0.34, 1.35, 0.64, 1.0);
/// Exit duration (`durations.fast`, exact Glyph source value).
const EXIT_DURATION: Duration = Duration::from_millis(150);
/// Exit easing (Glyph's `exit` cubic-bezier, exact source value).
const EXIT_CURVE: Curve = Curve::Cubic(0.4, 0.0, 1.0, 1.0);
/// Hold duration: exactly 2.4s (RESEARCH §1.4 pattern 08's authored
/// auto-dismiss value).
const HOLD_DURATION: Duration = Duration::from_millis(2400);
/// `reduce_motion`'s collapsed crossfade duration (mirrors
/// `nav::transition::REDUCE_MOTION_DURATION`).
const REDUCE_MOTION_DURATION: Duration = Duration::from_millis(120);
/// The enter slide's starting offset above the resting position, as a
/// fraction of the toast's own height ("from -140%" — the task's phrasing).
/// This task's own choice.
const ENTER_OFFSET_FRACTION: f64 = 1.4;

/// The `(enter, exit)` [`Timing`]s for the current `reduce_motion` state:
/// the authored spatial/exit durations+curves, or both collapsed to
/// [`REDUCE_MOTION_DURATION`] linear (the Details' "toast becomes fast
/// crossfade" rule).
fn resolve_timings(reduce_motion: bool) -> (Timing, Timing) {
    if reduce_motion {
        let t = Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear);
        (t, t)
    } else {
        (
            Timing::Duration(ENTER_DURATION, ENTER_CURVE),
            Timing::Duration(EXIT_DURATION, EXIT_CURVE),
        )
    }
}

/// [`ActiveToast`]'s lifecycle phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToastPhase {
    Enter,
    Hold,
    Exit,
    /// Fully exited; a no-op phase until the next `rebuild` tears it down
    /// and starts the next queued request (see the [module docs](self)).
    Done,
}

/// A currently-playing toast: its content pod plus timeline state.
struct ActiveToast {
    message: String,
    child: ChildPod,
    phase: ToastPhase,
    /// The active enter/exit progress driver — lazily built the first time
    /// [`ToastHostWidget::advance`] sees this phase, since `rebuild` (where
    /// the toast is created) has no theme to resolve a [`Timing`] from (only
    /// `LayoutCtx`/`PaintCtx` carry one).
    driver: Option<TransitionDriver>,
    /// Elapsed hold time, accumulated via `FrameTime` differencing (never
    /// `Instant::now()` — `docs/CODE_STANDARDS.md`).
    hold_elapsed: Duration,
    last_time: Option<FrameTime>,
}

/// Where [`ToastHostWidget`] positions the active toast within the host's
/// (now full-bleed) bounds. See the [module docs](self)'s Anchoring section.
///
/// Default [`ToastAnchor::BottomCenter`] — the framework's v1 opinionated
/// default, matching most platform toast conventions (Android's `Toast`,
/// Material Snackbar).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ToastAnchor {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    #[default]
    BottomCenter,
    BottomRight,
}

impl ToastAnchor {
    /// Whether `self` anchors to the top edge (`TopLeft`/`TopCenter`/
    /// `TopRight`) rather than the bottom — the axis the enter/exit slide
    /// direction and vertical margin both key off (see the [module
    /// docs](self)'s Anchoring section).
    pub fn is_top(self) -> bool {
        matches!(
            self,
            ToastAnchor::TopLeft | ToastAnchor::TopCenter | ToastAnchor::TopRight
        )
    }
}

/// Margin from the anchored edge(s), in logical px — matches the glyph
/// catalog's pre-this-task top margin (`lib.rs`'s `EdgeInsets { top: 10.0,
/// .. }` wrapper this task's host-side anchoring supersedes). This task's own
/// choice, additionally widened by [`frust_core::WindowInsets::padding`] on
/// the anchored edges (see the [module docs](self)'s Anchoring section).
const EDGE_MARGIN: f64 = 10.0;

/// Resolve the active toast's origin within the host's `host_size`, per
/// `anchor`, [`EDGE_MARGIN`], and the window's safe-area `insets` on the
/// anchored edges — additive with the margin (not `max`), so e.g. a
/// bottom-anchored toast's rest position sits `EDGE_MARGIN + insets.bottom`
/// above the host's bottom edge.
fn anchor_origin(
    anchor: ToastAnchor,
    host_size: Size,
    toast_size: Size,
    insets: WindowEdgeInsets,
) -> Point {
    let x = match anchor {
        ToastAnchor::TopLeft | ToastAnchor::BottomLeft => EDGE_MARGIN + insets.left,
        ToastAnchor::TopCenter | ToastAnchor::BottomCenter => {
            (host_size.width - toast_size.width) / 2.0
        }
        ToastAnchor::TopRight | ToastAnchor::BottomRight => {
            host_size.width - EDGE_MARGIN - insets.right - toast_size.width
        }
    };
    let y = if anchor.is_top() {
        EDGE_MARGIN + insets.top
    } else {
        host_size.height - EDGE_MARGIN - insets.bottom - toast_size.height
    };
    Point::new(x, y)
}

/// A declarative toast-request queue snapshot. See the [module docs](self).
pub struct ToastHostView {
    pending: Vec<String>,
    anchor: ToastAnchor,
}

/// Create a toast host reading `pending` (the app's FIFO request log — see
/// [`show`]) as this rebuild's snapshot, anchored [`ToastAnchor::BottomCenter`]
/// by default — see [`ToastHostView::anchor`].
pub fn toast_host(pending: impl Into<Vec<String>>) -> ToastHostView {
    ToastHostView {
        pending: pending.into(),
        anchor: ToastAnchor::default(),
    }
}

impl ToastHostView {
    /// Set the host's [`ToastAnchor`] (default [`ToastAnchor::BottomCenter`]).
    pub fn anchor(mut self, anchor: ToastAnchor) -> Self {
        self.anchor = anchor;
        self
    }
}

/// PascalCase alias for [`toast_host`], matching the widget-fn vocabulary
/// (`Button`/`Image`/…).
#[allow(non_snake_case)]
pub fn ToastHost(pending: impl Into<Vec<String>>) -> ToastHostView {
    toast_host(pending)
}

impl<State: 'static> View<State> for ToastHostView {
    type Element = ToastHostWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ToastHostWidget {
        let mut widget = ToastHostWidget {
            queue: self.pending.clone(),
            next_index: 0,
            active: None,
            anchor: self.anchor,
        };
        let _ = widget.maybe_advance_queue::<State>(ctx);
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ToastHostWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.queue = self.pending.clone();
        element.next_index = element.next_index.min(element.queue.len());
        let anchor_changed = prev.anchor != self.anchor;
        element.anchor = self.anchor;
        let structural_change = element.maybe_advance_queue::<State>(ctx);
        // The active child's own paint-driven animation covers redraw needs
        // when nothing structural happened, so this reports plain PAINT in
        // that case — cheap, and correct (never under-reports). Either an
        // anchor swap (repositions the toast) or a structural queue change
        // (a new child pod was built or the active one torn down — a fresh/
        // absent pod has never been laid out, so it MUST be relaid-out this
        // frame, not just repainted; see the module's diagnosis) additionally
        // forces LAYOUT (the host's own layout resolves the anchored origin,
        // not paint).
        if anchor_changed || structural_change {
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        } else {
            ChangeFlags::PAINT
        }
    }

    fn teardown(&self, element: &mut ToastHostWidget, ctx: &mut BuildCtx<'_>) {
        if let Some(active) = element.active.take() {
            let view = any::<State, _>(toast(active.message));
            let mut pod = active.child;
            crate::teardown_child(&view, &mut pod, ctx);
        }
    }
}

/// The retained widget for a [`ToastHostView`]. See the [module docs](self).
pub struct ToastHostWidget {
    /// The last-seen full request queue (a growing FIFO log — see the
    /// [module docs](self)'s Known v1 limitation).
    queue: Vec<String>,
    /// Index of the next not-yet-started request in `queue`.
    next_index: usize,
    active: Option<ActiveToast>,
    /// Where the active toast is positioned within the (full-bleed) host —
    /// see the [module docs](self)'s Anchoring section.
    anchor: ToastAnchor,
}

impl ToastHostWidget {
    /// If the current `active` toast is [`ToastPhase::Done`], tear it down
    /// and advance `next_index`; then, if nothing is active and a next
    /// request exists, start it. Building/tearing down a [`ChildPod`] needs
    /// a [`BuildCtx`], so this only ever runs from `build`/`rebuild` — see
    /// the [module docs](self).
    ///
    /// Returns whether the active toast was **structurally** changed — a new
    /// child pod was built and/or the previous one torn down — as opposed to
    /// the common case where the existing pod just keeps playing. A caller
    /// (`rebuild`) MUST fold a `true` return into `ChangeFlags::LAYOUT`: a
    /// freshly built pod has never been laid out, so `Widget::layout` MUST
    /// run this frame, not just `paint` (a structural child change is a
    /// layout-affecting event per the widget contract, not a paint-only one
    /// — the mobile layout-skip gate honors under-reported flags literally,
    /// see the module's diagnosis doc comment).
    fn maybe_advance_queue<State: 'static>(&mut self, ctx: &mut BuildCtx<'_>) -> bool {
        let mut structural_change = false;
        if matches!(
            self.active.as_ref().map(|a| a.phase),
            Some(ToastPhase::Done)
        ) {
            let active = self.active.take().expect("checked Some above");
            let view = any::<State, _>(toast(active.message));
            let mut pod = active.child;
            crate::teardown_child(&view, &mut pod, ctx);
            self.next_index += 1;
            structural_change = true;
        }
        if self.active.is_none()
            && let Some(message) = self.queue.get(self.next_index).cloned()
        {
            let child = crate::build_child::<State>(&any::<State, _>(toast(message.clone())), ctx);
            self.active = Some(ActiveToast {
                message,
                child,
                phase: ToastPhase::Enter,
                driver: None,
                hold_elapsed: Duration::ZERO,
                last_time: None,
            });
            structural_change = true;
        }
        structural_change
    }

    /// Advance the active toast's phase machine to frame time `now`,
    /// returning the shown-progress to paint at (`0.0` hidden, `1.0` fully
    /// shown — transiently `> 1.0` mid-enter under the spatial easing's
    /// overshoot), or `None` if nothing is active. Mutates `self.active`'s
    /// phase/driver/hold_elapsed in place.
    ///
    /// Factored out of `paint` so the full enter→hold→exit timeline is
    /// drivable with synthetic [`FrameTime`]s in a unit test — a bare
    /// `PaintCtx` can't be constructed at an arbitrary frame time outside
    /// `frust-core` (mirrors `LoadingIndicatorWidget::step`'s rationale).
    fn advance(&mut self, now: FrameTime, enter: Timing, exit: Timing) -> Option<f64> {
        let active = self.active.as_mut()?;
        let shown = match active.phase {
            ToastPhase::Enter => {
                let adv = active
                    .driver
                    .get_or_insert_with(|| make_driver(enter).0)
                    .advance(now);
                if adv.done {
                    active.phase = ToastPhase::Hold;
                    active.driver = None;
                    active.last_time = Some(now);
                }
                adv.value
            }
            ToastPhase::Hold => {
                if let Some(last) = active.last_time {
                    active.hold_elapsed += now.saturating_sub(last);
                }
                active.last_time = Some(now);
                if active.hold_elapsed >= HOLD_DURATION {
                    active.phase = ToastPhase::Exit;
                    active.driver = None;
                }
                1.0
            }
            ToastPhase::Exit => {
                let adv = active
                    .driver
                    .get_or_insert_with(|| make_driver(exit).0)
                    .advance(now);
                if adv.done {
                    active.phase = ToastPhase::Done;
                }
                1.0 - adv.value.clamp(0.0, 1.0)
            }
            ToastPhase::Done => 0.0,
        };
        Some(shown)
    }
}

impl Widget for ToastHostWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Lay out the active toast (if any) at its own natural size first —
        // needed both to size an unbounded host axis (below) and to position
        // it per anchor. `bc.loosen()` mirrors the old direct pass-through
        // (this host's incoming `bc` is already loose — `Stack` loosens its
        // children — but loosening explicitly keeps the toast's own sizing
        // independent of the host's now-larger, full-bleed `bc`).
        let toast_size = match &mut self.active {
            Some(active) if !matches!(active.phase, ToastPhase::Done) => {
                Some(active.child.layout_child(ctx, &bc.loosen()))
            }
            _ => None,
        };

        // Full-bleed: fill each bounded axis (the common case — Stack hands
        // down the surrounding page's finite size) so the host spans the
        // whole overlay region; shrink-wrap an unbounded axis to the active
        // toast's own size instead (Align's Flutter-parity per-axis rule —
        // `align.rs`'s module docs), or to zero while idle.
        let natural = toast_size.unwrap_or(Size::ZERO);
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            natural.width
        };
        let height = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            natural.height
        };
        let size = bc.constrain(Size::new(width, height));

        if let (Some(active), Some(toast_size)) = (&mut self.active, toast_size) {
            let insets = ctx.window_insets().padding();
            let origin = anchor_origin(self.anchor, size, toast_size, insets);
            active.child.set_origin(origin);
        }

        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let (enter, exit) = resolve_timings(reduce_motion);
        let now = ctx.frame_time();
        let host_origin = ctx.origin();
        // Bottom anchors rise from below (a positive slide offset settling to
        // zero); top anchors keep the pre-existing drop-from-above motion
        // (negative offset) — see the [module docs](self)'s Anchoring section.
        let sign = if self.anchor.is_top() { -1.0 } else { 1.0 };

        if let Some(shown) = self.advance(now, enter, exit) {
            let phase = self.active.as_ref().map(|a| a.phase);
            let done = matches!(phase, Some(ToastPhase::Done));
            if !done && let Some(active) = &mut self.active {
                // The toast's own (anchored) rect, not the host's full-bleed
                // bounds — the layer/transform below only ever cover the
                // toast itself, keeping the rest of the (event-transparent)
                // host untouched.
                let toast_origin = host_origin + active.child.origin().to_vec2();
                let toast_size = active.child.size();
                let dy = (1.0 - shown) * sign * (ENTER_OFFSET_FRACTION * toast_size.height);
                let alpha = shown.clamp(0.0, 1.0) as f32;
                scene.push_transform(Affine::translate((0.0, dy)));
                scene.push_layer(toast_origin, toast_size, alpha);
                active.child.paint_child(ctx, scene);
                scene.pop_layer();
                scene.pop_transform();
            }
            // Kept active (even once `Done`) until the next `rebuild` tears it
            // down — one more frame ensures that rebuild actually runs (see
            // the [module docs](self)). Enter/Exit are genuine transitions
            // (a user-visible endpoint driven by `TransitionDriver`) and
            // request the unpaced class every vsync, like the one-frame
            // tear-down poke past `Done`; Hold's only need is elapsed-time
            // bookkeeping toward `HOLD_DURATION`, so it paces instead — the
            // mobile frame gate throttles it to `cosmetic_loop_rate`
            // (`docs/CODE_STANDARDS.md`'s perpetual-decorative-loop rule; a
            // fixed-length hold isn't perpetual, but the same
            // paint-doesn't-need-full-rate reasoning applies since nothing
            // visible changes frame-to-frame during it).
            if matches!(phase, Some(ToastPhase::Hold)) {
                ctx.request_frame_paced();
            } else {
                ctx.request_frame();
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if let Some(active) = &self.active
            && !matches!(active.phase, ToastPhase::Done)
        {
            active.child.semantics_child(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::BuildCtx;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn build_host(pending: Vec<String>) -> ToastHostWidget {
        let view: ToastHostView = toast_host(pending);
        let mut counter = 0u64;
        <ToastHostView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    // -- Toast leaf -----------------------------------------------------

    #[derive(Default)]
    struct RecordingScene {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        transforms: Vec<Affine>,
        transform_pops: u32,
        layers: Vec<(Point, Size, f32)>,
        layer_pops: u32,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
        fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
            self.layers.push((origin, size, alpha));
        }
        fn pop_layer(&mut self) {
            self.layer_pops += 1;
        }
    }

    fn build_toast(message: &str) -> ToastWidget {
        let view: ToastView = toast(message);
        let mut counter = 0u64;
        <ToastView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn toast_paints_shadow_surface_dot_and_message() {
        use frust_core::LayoutCtx;
        use frust_text::TextContext;
        use std::any::Any;

        let mut w = build_toast("Saved");
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 200.0)));
        let mut ctx = PaintCtx::new(Point::ZERO, size);
        let mut scene = RecordingScene::default();
        w.paint(&mut ctx, &mut scene);
        assert_eq!(scene.shadows.len(), 1);
        assert_eq!(scene.rrects.len(), 2, "surface rect + dot rect");
        assert_eq!(scene.rrects[0].3, FALLBACK_SURFACE);
    }

    #[test]
    fn variant_selects_the_dot_color_unthemed() {
        for (variant, expected) in [
            (ToastVariant::Plain, FALLBACK_PLAIN),
            (ToastVariant::Info, FALLBACK_INFO),
            (ToastVariant::Success, FALLBACK_SUCCESS),
            (ToastVariant::Warning, FALLBACK_WARNING),
            (ToastVariant::Error, FALLBACK_ERROR),
        ] {
            assert_eq!(ToastWidget::resolve_dot_color(None, variant), expected);
        }
    }

    #[test]
    fn glyph_tokens_resolve_dark_and_light() {
        let dark = Theme::glyph_baseline();
        let light = dark.clone().with_brightness(frust_theme::Brightness::Light);
        assert_eq!(
            ToastWidget::resolve_surface_color(Some(&dark)),
            dark.scheme().surface_container_highest
        );
        assert_eq!(
            ToastWidget::resolve_surface_color(Some(&light)),
            light.scheme().surface_container_highest
        );
        assert_ne!(
            dark.scheme().surface_container_highest,
            light.scheme().surface_container_highest
        );
        // Status-based variants resolve StatusPalette per brightness.
        let dark_success = ToastWidget::resolve_dot_color(Some(&dark), ToastVariant::Success);
        let light_success = ToastWidget::resolve_dot_color(Some(&light), ToastVariant::Success);
        assert_eq!(
            dark_success,
            dark.extension::<StatusPalette>().unwrap().dark.success
        );
        assert_eq!(
            light_success,
            light.extension::<StatusPalette>().unwrap().light.success
        );
    }

    // -- ToastHost: queue policy (v1: single visible, FIFO) --------------

    #[test]
    fn only_one_toast_is_active_at_a_time_and_starts_the_first_request() {
        let host = build_host(vec!["first".to_string(), "second".to_string()]);
        let active = host
            .active
            .as_ref()
            .expect("first request starts immediately");
        assert_eq!(active.message, "first");
        assert_eq!(active.phase, ToastPhase::Enter);
        assert_eq!(host.next_index, 0, "not yet advanced past the active item");
    }

    #[test]
    fn finishing_a_toast_starts_the_next_queued_one_on_rebuild() {
        let mut host = build_host(vec!["first".to_string(), "second".to_string()]);
        let (enter, exit) = resolve_timings(false);

        // Drive fully through enter -> hold -> exit -> Done.
        let mut t = 0.0;
        loop {
            host.advance(ft_ms(t), enter, exit);
            if matches!(
                host.active.as_ref().map(|a| a.phase),
                Some(ToastPhase::Done)
            ) {
                break;
            }
            t += 50.0;
            assert!(t < 10_000.0, "timeline never reached Done");
        }

        let view: ToastHostView = toast_host(vec!["first".to_string(), "second".to_string()]);
        let mut counter = 0u64;
        <ToastHostView as View<()>>::rebuild(
            &view,
            &view,
            &mut host,
            &mut BuildCtx::new(&mut counter),
        );

        assert_eq!(host.next_index, 1, "advanced past the finished first toast");
        let active = host.active.as_ref().expect("the second request starts");
        assert_eq!(active.message, "second");
        assert_eq!(active.phase, ToastPhase::Enter);
    }

    // -- ToastHost: rebuild flags (the bug this task fixes — a structural
    // queue change (start or teardown of the active pod) MUST report LAYOUT,
    // or a layout-skip shell (Android) leaves a fresh/torn-down child pod
    // unlaid-out — see the module's diagnosis doc comment) -----------------

    #[test]
    fn rebuild_reports_layout_when_a_pushed_toast_starts() {
        let mut host = build_host(vec![]);
        assert!(host.active.is_none(), "nothing queued yet");

        let prev_view: ToastHostView = toast_host(vec![]);
        let next_view: ToastHostView = toast_host(vec!["hello".to_string()]);
        let mut counter = 0u64;
        let flags = <ToastHostView as View<()>>::rebuild(
            &next_view,
            &prev_view,
            &mut host,
            &mut BuildCtx::new(&mut counter),
        );

        assert!(host.active.is_some(), "the pushed toast starts immediately");
        assert!(
            flags.needs_layout(),
            "a newly-started toast's child pod has never been laid out — \
             the rebuild that builds it MUST report LAYOUT, or a \
             layout-skip shell (Android) leaves it at Size::ZERO for its \
             whole lifecycle"
        );
    }

    #[test]
    fn honoring_rebuild_flags_after_a_push_always_lays_out_the_new_child() {
        // Mirrors the Android-shaped sequence: a shell that trusts the
        // returned ChangeFlags and only calls `layout` when they say to. If
        // `rebuild` under-reports (PAINT-only), a shell honoring that
        // literally paints a never-laid-out (Size::ZERO) child — this test
        // drives exactly that shell contract and confirms the toast layer
        // paints nonzero.
        let mut host = build_host(vec![]);
        let prev_view: ToastHostView = toast_host(vec![]);
        let next_view: ToastHostView = toast_host(vec!["hello".to_string()]);
        let mut counter = 0u64;
        let flags = <ToastHostView as View<()>>::rebuild(
            &next_view,
            &prev_view,
            &mut host,
            &mut BuildCtx::new(&mut counter),
        );

        // The shell contract under test: only relayout when flags say so.
        if flags.needs_layout() {
            layout_host(&mut host, &BoxConstraints::tight(Size::new(400.0, 800.0)));
        }

        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 800.0));
        let mut scene = RecordingScene::default();
        host.paint(&mut ctx, &mut scene);

        assert_eq!(scene.layers.len(), 1, "the active toast paints a layer");
        let (_, layer_size, _) = scene.layers[0];
        assert!(
            layer_size.width > 0.0 && layer_size.height > 0.0,
            "the toast layer must be nonzero — a zero size means the shell \
             skipped layout on a flags contract it correctly honored, i.e. \
             the bug this task fixes: {layer_size:?}"
        );
    }

    #[test]
    fn rebuild_reports_layout_when_a_finished_toast_tears_down() {
        let mut host = build_host(vec!["first".to_string(), "second".to_string()]);
        let (enter, exit) = resolve_timings(false);

        // Drive fully through enter -> hold -> exit -> Done.
        let mut t = 0.0;
        loop {
            host.advance(ft_ms(t), enter, exit);
            if matches!(
                host.active.as_ref().map(|a| a.phase),
                Some(ToastPhase::Done)
            ) {
                break;
            }
            t += 50.0;
            assert!(t < 10_000.0, "timeline never reached Done");
        }

        let view: ToastHostView = toast_host(vec!["first".to_string(), "second".to_string()]);
        let mut counter = 0u64;
        let flags = <ToastHostView as View<()>>::rebuild(
            &view,
            &view,
            &mut host,
            &mut BuildCtx::new(&mut counter),
        );

        assert!(
            flags.needs_layout(),
            "tearing down the finished toast and starting the next is also \
             a structural change and must report LAYOUT"
        );
    }

    #[test]
    fn idle_host_with_no_pending_requests_requests_no_frame() {
        let mut host = build_host(vec![]);
        assert!(host.active.is_none());
        let mut ctx = PaintCtx::new(Point::ZERO, Size::ZERO);
        let mut scene = RecordingScene::default();
        host.paint(&mut ctx, &mut scene);
        assert!(!ctx.needs_frame());
    }

    // -- ToastHost: timeline (enter -> hold 2.4s -> exit), fake frame times --

    #[test]
    fn timeline_progresses_enter_then_hold_then_exit_then_done() {
        let mut host = build_host(vec!["hello".to_string()]);
        let (enter, exit) = resolve_timings(false);

        // Seed the clock; still Enter, shown starts at 0.
        let shown0 = host.advance(ft_ms(0.0), enter, exit).unwrap();
        assert!(shown0.abs() < 1e-6);
        assert_eq!(host.active.as_ref().unwrap().phase, ToastPhase::Enter);

        // Past the 220ms enter duration: settles into Hold at shown == 1.0.
        let shown_settled = host.advance(ft_ms(300.0), enter, exit).unwrap();
        assert!((shown_settled - 1.0).abs() < 1e-6);
        assert_eq!(host.active.as_ref().unwrap().phase, ToastPhase::Hold);

        // Advance through most of the 2.4s hold: still Hold.
        host.advance(ft_ms(300.0 + 2000.0), enter, exit);
        assert_eq!(host.active.as_ref().unwrap().phase, ToastPhase::Hold);

        // Past the full 2.4s hold: flips to Exit (this call only flips the
        // phase; the exit driver itself is created and seeded on the *next*
        // call, mirroring the enter driver's own seed-then-settle shape
        // above).
        host.advance(ft_ms(300.0 + 2500.0), enter, exit);
        assert_eq!(host.active.as_ref().unwrap().phase, ToastPhase::Exit);

        // Seed the exit driver's clock.
        let shown_exit_seed = host
            .advance(ft_ms(300.0 + 2500.0 + 10.0), enter, exit)
            .unwrap();
        assert!(
            (shown_exit_seed - 1.0).abs() < 1e-6,
            "exit starts fully shown"
        );
        assert_eq!(host.active.as_ref().unwrap().phase, ToastPhase::Exit);

        // Past the 150ms exit duration: settles into Done, shown == 0.
        let shown_done = host
            .advance(ft_ms(300.0 + 2500.0 + 10.0 + 300.0), enter, exit)
            .unwrap();
        assert!(shown_done.abs() < 1e-6);
        assert_eq!(host.active.as_ref().unwrap().phase, ToastPhase::Done);
    }

    #[test]
    fn reduce_motion_uses_the_fast_linear_crossfade_timing() {
        let (enter, exit) = resolve_timings(true);
        assert_eq!(
            enter,
            Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear)
        );
        assert_eq!(
            exit,
            Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear)
        );
    }

    #[test]
    fn paint_at_enter_start_emits_translated_faded_layer() {
        let mut host = build_host(vec!["hi".to_string()]);
        {
            use frust_core::LayoutCtx;
            use frust_text::TextContext;
            use std::any::Any;
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            host.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        }
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(200.0, 50.0));
        let mut scene = RecordingScene::default();
        host.paint(&mut ctx, &mut scene);
        assert_eq!(scene.transforms.len(), 1);
        assert_eq!(scene.transform_pops, 1);
        assert_eq!(scene.layers.len(), 1);
        assert_eq!(scene.layer_pops, 1);
        // At the very start of Enter (FrameTime::ZERO seeds the clock), shown
        // is 0: fully faded and offset fully above the resting position.
        assert_eq!(scene.layers[0].2, 0.0);
        assert!(ctx.needs_frame());
    }

    #[test]
    fn hold_phase_paces_frame_requests_but_enter_and_exit_do_not() {
        let mut host = build_host(vec!["hi".to_string()]);
        {
            use frust_core::LayoutCtx;
            use frust_text::TextContext;
            use std::any::Any;
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            host.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        }

        // Enter (the freshly-built default phase): a genuine transition,
        // unpaced.
        assert_eq!(host.active.as_ref().unwrap().phase, ToastPhase::Enter);
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(300.0, 100.0));
        let mut scene = RecordingScene::default();
        host.paint(&mut ctx, &mut scene);
        assert_eq!(host.active.as_ref().unwrap().phase, ToastPhase::Enter);
        assert!(
            !ctx.needs_frame_paced_only(),
            "Enter is a genuine transition and must run unpaced"
        );

        // Force into Hold — mirrors the file's other tests' direct phase
        // manipulation via `advance`/field access (same-file `tests`
        // submodule has private-field access); elapsed-time bookkeeping only,
        // so it must pace.
        host.active.as_mut().unwrap().phase = ToastPhase::Hold;
        host.active.as_mut().unwrap().last_time = None;
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(300.0, 100.0));
        let mut scene = RecordingScene::default();
        host.paint(&mut ctx, &mut scene);
        assert_eq!(host.active.as_ref().unwrap().phase, ToastPhase::Hold);
        assert!(
            ctx.needs_frame_paced_only(),
            "Hold's only need is elapsed-time bookkeeping toward \
             HOLD_DURATION — it must pace to the mobile frame gate's \
             cosmetic-loop rate"
        );

        // Force into Exit: a genuine transition again, unpaced.
        host.active.as_mut().unwrap().phase = ToastPhase::Exit;
        host.active.as_mut().unwrap().driver = None;
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(300.0, 100.0));
        let mut scene = RecordingScene::default();
        host.paint(&mut ctx, &mut scene);
        assert_eq!(host.active.as_ref().unwrap().phase, ToastPhase::Exit);
        assert!(
            !ctx.needs_frame_paced_only(),
            "Exit is a genuine transition and must run unpaced"
        );
    }

    // -- ToastHost: anchoring (task 07, framework-side positioning) -----

    /// Lay `host` out under `bc` with a fresh (empty) `TextContext` — every
    /// active toast contains a `Text` child needing one threaded (mirrors
    /// `paint_at_enter_start_emits_translated_faded_layer`'s inline setup).
    fn layout_host(host: &mut ToastHostWidget, bc: &BoxConstraints) -> Size {
        use frust_core::LayoutCtx;
        use frust_text::TextContext;
        use std::any::Any;
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        host.layout(&mut lctx, bc)
    }

    #[test]
    fn default_anchor_is_bottom_center() {
        assert_eq!(ToastAnchor::default(), ToastAnchor::BottomCenter);
    }

    #[test]
    fn default_anchor_positions_bottom_center_with_margin_no_insets() {
        let mut host = build_host(vec!["hi".to_string()]);
        let size = layout_host(&mut host, &BoxConstraints::tight(Size::new(400.0, 800.0)));
        assert_eq!(size, Size::new(400.0, 800.0), "host fills the full bc");

        let active = host.active.as_ref().expect("toast starts immediately");
        let toast_size = active.child.size();
        let origin = active.child.origin();

        let center_x = origin.x + toast_size.width / 2.0;
        assert!(
            (center_x - 200.0).abs() < 1e-6,
            "toast rect centers horizontally: {center_x}"
        );
        let bottom_edge = origin.y + toast_size.height;
        assert!(
            (bottom_edge - (800.0 - EDGE_MARGIN)).abs() < 1e-6,
            "toast rect's bottom edge sits EDGE_MARGIN above the host bottom: {bottom_edge}"
        );
    }

    #[test]
    fn default_anchor_bottom_margin_additionally_respects_the_bottom_inset() {
        use frust_core::{RenderRoot, WindowEdgeInsets, WindowInsets};
        use frust_text::TextContext;

        fn logic(_: &mut ()) -> ToastHostView {
            toast_host(vec!["hi".to_string()])
        }
        let mut root: RenderRoot<(), ToastHostView> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.set_insets(WindowInsets::new(
            WindowEdgeInsets::new(0.0, 0.0, 0.0, 34.0),
            WindowEdgeInsets::ZERO,
        ));
        let mut tcx = TextContext::new();
        let size = root.layout_with_text(Size::new(400.0, 800.0), &mut tcx);
        assert_eq!(size, Size::new(400.0, 800.0));

        let id = root.root_id().expect("root built");
        let widget = (root.tree().pod(id).expect("root pod").widget() as &dyn std::any::Any)
            .downcast_ref::<ToastHostWidget>()
            .expect("root is a ToastHostWidget");
        let active = widget.active.as_ref().expect("toast starts immediately");
        let toast_size = active.child.size();
        let origin = active.child.origin();
        let bottom_edge = origin.y + toast_size.height;
        assert!(
            (bottom_edge - (800.0 - EDGE_MARGIN - 34.0)).abs() < 1e-6,
            "the 34px bottom inset widens the margin additively: {bottom_edge}"
        );
    }

    #[test]
    fn all_six_anchors_position_correctly() {
        for anchor in [
            ToastAnchor::TopLeft,
            ToastAnchor::TopCenter,
            ToastAnchor::TopRight,
            ToastAnchor::BottomLeft,
            ToastAnchor::BottomCenter,
            ToastAnchor::BottomRight,
        ] {
            let view: ToastHostView = toast_host(vec!["hi".to_string()]).anchor(anchor);
            let mut counter = 0u64;
            let mut host =
                <ToastHostView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter));
            let size = layout_host(&mut host, &BoxConstraints::tight(Size::new(400.0, 800.0)));
            assert_eq!(size, Size::new(400.0, 800.0));

            let active = host.active.as_ref().expect("toast starts immediately");
            let toast_size = active.child.size();
            let origin = active.child.origin();

            let expected_x = match anchor {
                ToastAnchor::TopLeft | ToastAnchor::BottomLeft => EDGE_MARGIN,
                ToastAnchor::TopCenter | ToastAnchor::BottomCenter => {
                    (400.0 - toast_size.width) / 2.0
                }
                ToastAnchor::TopRight | ToastAnchor::BottomRight => {
                    400.0 - EDGE_MARGIN - toast_size.width
                }
            };
            let expected_y = if anchor.is_top() {
                EDGE_MARGIN
            } else {
                800.0 - EDGE_MARGIN - toast_size.height
            };
            assert!(
                (origin.x - expected_x).abs() < 1e-6,
                "{anchor:?}: x = {}, expected {expected_x}",
                origin.x
            );
            assert!(
                (origin.y - expected_y).abs() < 1e-6,
                "{anchor:?}: y = {}, expected {expected_y}",
                origin.y
            );
        }
    }

    #[test]
    fn host_never_swallows_events_inside_or_outside_the_toast_rect() {
        use frust_core::{
            EventCtx, EventResult, InputEvent, PointerButton, PointerEvent, PointerPhase,
        };
        use std::any::Any;

        // `ToastHostWidget` never overrides `Widget::event`, so it always
        // falls through to the default no-op — verified explicitly here so a
        // future accidental override (e.g. adding tap-to-dismiss) trips this
        // test rather than silently starting to swallow taps.
        let mut host = build_host(vec!["hi".to_string()]);
        let mut state = ();
        let state_any: &mut dyn Any = &mut state;
        let mut ectx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 800.0));

        let ev = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(5.0, 5.0),
            button: PointerButton::Primary,
        });
        assert_eq!(host.event(&mut ectx, &ev), EventResult::Ignored);
    }

    #[test]
    fn enter_offset_sign_flips_between_top_and_bottom_anchors() {
        fn early_enter_dy(anchor: ToastAnchor) -> f64 {
            let view: ToastHostView = toast_host(vec!["hi".to_string()]).anchor(anchor);
            let mut counter = 0u64;
            let mut host =
                <ToastHostView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter));
            layout_host(&mut host, &BoxConstraints::tight(Size::new(400.0, 800.0)));

            let mut ctx = PaintCtx::new(Point::ZERO, Size::new(400.0, 800.0));
            let mut scene = RecordingScene::default();
            host.paint(&mut ctx, &mut scene);
            scene.transforms[0].translation().y
        }

        // At the very start of Enter (shown == 0), the slide offset is at its
        // full magnitude — sign alone tells top from bottom.
        let bottom_dy = early_enter_dy(ToastAnchor::BottomCenter);
        let top_dy = early_enter_dy(ToastAnchor::TopCenter);
        assert!(
            bottom_dy > 0.0,
            "a bottom anchor rises from below (positive dy): {bottom_dy}"
        );
        assert!(
            top_dy < 0.0,
            "a top anchor drops from above (negative dy): {top_dy}"
        );
    }
}
