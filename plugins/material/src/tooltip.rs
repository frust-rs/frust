// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/tooltips/` — `m3e_tooltips.dart` (the `M3ETooltip` widget)
// and `styles/m3e_tooltip_theme.dart` (every metric), retrieved 2026-08-20.
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions:
// - Placement/light-dismiss/entrance ramp are **not** hand-rolled: this
//   catalog's merged `crate::overlay::anchored` host owns all three, so the
//   panels below compose under it (the [`mod@crate::menu`] precedent) rather
//   than beside it, the way upstream's own `OverlayPortal`/
//   `CompositedTransformFollower` pair does.
// - **No exit fade — deliberate, faithful to upstream.** `M3ETooltip._fade`
//   (`m3e_tooltips.dart:187-197`) wraps only the *entrance*
//   (`TweenAnimationBuilder<double>` from 0 to 1); `_hide()` just calls
//   `OverlayPortalController.hide()`, which unmounts the overlay child on the
//   spot — there is no exit tween to port. The anchored host's own kept-mounted
//   contract plays a 150ms exit ramp when an app hands it a live `open` flag
//   (see `crate::overlay::anchored`'s module docs), which would fabricate a fade
//   upstream never has — so these panels use the host's **other** documented
//   mount contract instead: **mount-on-open**. [`TooltipView`]/
//   [`RichTooltipView`] swap their own `View::Element` between a zero-size
//   [`frust::SizedBox`] and a freshly-built `anchored_overlay(..)` (never
//   passing `.open(false)`, so the host never starts its own exit ramp) each
//   time [`TooltipHover::is_open`] flips — the type change tears the panel down
//   on the very rebuild that closes it, vanishing exactly as `hide()` does. The
//   host's entrance ramp (fade **and** anchor-pivoted scale, `crate::overlay`'s
//   own "the host owns the ramp" precedent) still plays on open; upstream's own
//   entrance is a plain opacity tween with no scale, so this port's entrance is
//   *closer to the rest of the catalog's anchored panels* than to this one
//   reference file — the same trade `crate::menu`'s header documents.
// - Neither trigger mechanism upstream owns a *show delay*: `MouseRegion.onEnter`
//   calls `_show()` synchronously (no `Timer`), and `onLongPress` fires only
//   after Flutter's own long-press recognizer threshold elapses. This port
//   mirrors both literally — hover opens on the very next paint that observes
//   [`PaintCtx::is_hovered`], zero added delay — and reuses this catalog's
//   established 500ms long-press threshold
//   ([`crate::toggle_button`]'s own citation: Android's `ViewConfiguration`
//   ~400-500ms default, iOS's `UILongPressGestureRecognizer.minimumPressDuration`
//   0.5s) as the trigger's own "show delay" for touch, delegated to
//   `frust::GestureDetector` (see the [Hover vs. long-press](#hover-vs-long-press)
//   section) rather than a hand-rolled recognizer.
// - Upstream enforces no "one tooltip visible at a time" rule anywhere in
//   `_M3ETooltipState` — no shared registry, no portal singleton — so this port
//   adds none either; two [`TooltipHover`]s opened independently may both show.

//! Material 3 Expressive tooltips: a plain, auto-dismissing label and a rich
//! panel with a title, supporting text and up to two action buttons — both
//! opened by hovering (pointer platforms) or a long-press (touch), and
//! positioned by the merged [`crate::overlay::anchored`] host.
//!
//! # The shared handle
//!
//! [`TooltipHover`] is the one non-reactive latch a caller keeps per tooltip —
//! the trigger writes it, the panel reads it, the [`crate::overlay::anchored`]
//! host's own [`OverlayAnchor`] rides inside it so a caller never juggles two
//! handles. It is **not reactive** (`docs/CODE_STANDARDS.md`'s convention for
//! this catalog's other paint-latched handles, e.g.
//! [`crate::overlay::anchored::OverlayAnchor`] itself): writing it wakes
//! nothing on its own, so every writer also calls
//! `PaintCtx::request_frame`/relies on the pointer event that carried it — see
//! each write site below.
//!
//! # Hover vs. long-press
//!
//! [`tooltip_trigger`] wraps an arbitrary child transparently (every event
//! still reaches it) and does two independent things:
//!
//! - **Hover** is read the same way [`crate::overlay`]'s shadcn-ported sibling
//!   catalog reads it for its own tooltip (`frust_shadcn::components::tooltip`):
//!   `PaintCtx::is_hovered` is the *authoritative* signal (self-correcting every
//!   paint, the one pass that reliably learns a pointer left without a
//!   dedicated event), claimed for via `EventCtx::claim_hover` on an in-bounds
//!   `Move` so a hovered control *inside* the child still wins the claim and
//!   this wrapper still reads hovered through the path (the claim-ordering
//!   rule). A transition writes [`TooltipHover`] and requests a frame so the
//!   panel's own mount-on-open swap (driven by ordinary `View::rebuild`, not by
//!   this write) picks it up on the very next driven frame.
//! - **Long-press** is delegated whole to `frust::GestureDetector` (nested as
//!   this trigger's one child, wrapping the caller's real child in turn) rather
//!   than hand-rolled: touch has no hover, and `GestureDetector`'s own
//!   paint-clock timer + `frust_core::mark_pending_result_flush` machinery is
//!   the only way a plugin crate reaches a threshold-time fire with no further
//!   pointer event (`crate::toggle_button`'s own module docs explain why that
//!   function is unreachable from a design-system plugin directly — see its
//!   "Long-press" section). The callback ignores the `&mut State` it is handed
//!   and writes straight into the shared, non-reactive [`TooltipHover`] instead.
//!
//! [`TooltipTriggerView::enabled`] gates both: `false` suppresses hover opening
//! it, the long-press callback becomes a no-op, and an already-open tooltip
//! snaps shut the moment `enabled` goes false mid-show.
//!
//! # Plain: auto-dismiss, long-press only
//!
//! `M3ETooltipState._scheduleHide` (`m3e_tooltips.dart:72-78`) arms a
//! [`MaterialMotion::EXTRA_LONG_4`]-long (1000ms) one-shot timer **only** from
//! the `onLongPress` handler — hover's `onEnter` never calls it, closing purely
//! on `onExit` instead — and never for a rich tooltip (`if (widget._isRich)
//! return;`). This port's plain panel widget captures
//! `hover.via() == TooltipOpenVia::LongPress` once, at the paint the panel
//! first mounts (the same instant upstream's `_show(); _scheduleHide();` pair
//! runs synchronously together), and runs the matching paint-clock timer only
//! when that was true — a hover-shown plain tooltip never starts one, matching
//! upstream exactly.
//!
//! # Rich: title/supporting/actions, dismiss on tap-elsewhere
//!
//! A rich panel never runs the auto-dismiss timer above; it stays open while
//! the trigger keeps its hover, closes on hover-exit exactly like the plain
//! panel, and additionally dismisses on a tap anywhere outside itself —
//! upstream's own `Positioned.fill(child: GestureDetector(onTap: _hide,
//! behavior: HitTestBehavior.translucent))`, ported as the anchored host's
//! **own** light-dismiss (`AnchoredOverlayView::on_dismiss`) rather than a
//! hand-rolled translucent layer, since the host already swallows every `Down`
//! outside its placed content and fires exactly that callback.

use std::cell::Cell;
use std::rc::Rc;

use frust::authoring::text::{FontWeight, LineHeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{
    AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect, SemanticsCtx, Size,
    View, Widget, any, build_child, rebuild_child, rebuild_children, route_event_single,
    teardown_child, visit_children,
};
use frust::{FrameTime, GestureDetector, SizedBox, Theme};

use crate::button::text_button;
use crate::overlay::{
    OverlayAlign, OverlayAnchor, OverlayContainer, OverlayElevation, OverlayPlacement, OverlaySide,
    anchored_overlay, container as overlay_container, on_surface_variant, shadow as overlay_shadow,
};
use crate::tokens::{MaterialDimensions, MaterialMotion, MaterialSpacing};

// ---- Timing -----------------------------------------------------------------

/// Touch's long-press threshold — [`crate::toggle_button`]'s own
/// `LONG_PRESS_MS` citation applies verbatim (community-approximate, matching
/// both platforms' gesture-recognizer defaults). Delegated to
/// [`frust::GestureDetector`], not measured here — see the module docs.
const LONG_PRESS_MS: u64 = 500;

/// The plain tooltip's auto-dismiss delay after a long-press-triggered show —
/// `M3ETooltipTheme.plainDismissDelay` (`m3e_tooltip_theme.dart:51`),
/// [`MaterialMotion::EXTRA_LONG_4`] (1000ms).
const PLAIN_DISMISS_MS: f64 = MaterialMotion::EXTRA_LONG_4.as_millis() as f64;

// ---- Geometry (`M3ETooltipTheme.defaults`, `m3e_tooltip_theme.dart:9-18`) ---

/// `plainMaxWidth`.
const PLAIN_MAX_WIDTH: f64 = 200.0;
/// `plainPadding` horizontal (`EdgeInsets.symmetric(horizontal: 8, ..)`) —
/// [`MaterialSpacing::SM`].
const PLAIN_PAD_X: f64 = MaterialSpacing::SM;
/// `plainPadding` vertical (`.. vertical: 4)`) — [`MaterialSpacing::XS`].
const PLAIN_PAD_Y: f64 = MaterialSpacing::XS;
/// `richMaxWidth`.
const RICH_MAX_WIDTH: f64 = 320.0;
/// `richPadding` (`EdgeInsets.all(16)`) — [`MaterialSpacing::LG`].
const RICH_PAD: f64 = MaterialSpacing::LG;
/// `richTitleGap` — [`MaterialSpacing::XS`].
const RICH_TITLE_GAP: f64 = MaterialSpacing::XS;
/// `richActionsGap` — [`MaterialSpacing::MD`].
const RICH_ACTIONS_GAP: f64 = MaterialSpacing::MD;

/// Unthemed-fallback plain container fill (`colors.inverseSurface`, M3
/// baseline light) — the same value [`crate::snackbar`]'s identically-rooted
/// fallback carries.
const FALLBACK_INVERSE_SURFACE: Color = Color::from_rgb8(0x32, 0x2F, 0x35);
/// Unthemed-fallback plain message ink (`colors.inverseOnSurface`, M3
/// baseline light).
const FALLBACK_INVERSE_ON_SURFACE: Color = Color::from_rgb8(0xF5, 0xEF, 0xF7);
/// Unthemed-fallback rich title ink (`colors.onSurface`, M3 baseline light).
const FALLBACK_ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

// ---- Type styles (`bodySmall`/`titleSmall`/`bodyMedium`, `type_scale.rs`'s
// own documented table) -------------------------------------------------------

fn plain_message_style() -> TextStyle {
    TextStyle {
        weight: FontWeight::REGULAR,
        letter_spacing: 0.4,
        line_height: LineHeight::Absolute(16.0),
        ..TextStyle::new(12.0, Color::BLACK)
    }
}

fn rich_title_style() -> TextStyle {
    TextStyle {
        weight: FontWeight::MEDIUM,
        letter_spacing: 0.1,
        line_height: LineHeight::Absolute(20.0),
        ..TextStyle::new(14.0, Color::BLACK)
    }
}

fn rich_body_style() -> TextStyle {
    TextStyle {
        weight: FontWeight::REGULAR,
        letter_spacing: 0.25,
        line_height: LineHeight::Absolute(20.0),
        ..TextStyle::new(14.0, Color::BLACK)
    }
}

// ---- The shared handle -------------------------------------------------------

/// Which trigger mechanism opened a tooltip — the plain panel's own record of
/// "was this the long-press that arms the auto-dismiss timer" (see the module
/// docs' Plain section).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TooltipOpenVia {
    Hover,
    LongPress,
}

/// [`TooltipHover`]'s inner, `Copy` payload.
#[derive(Clone, Copy, Debug)]
struct TooltipLatch {
    open: bool,
    via: TooltipOpenVia,
    enabled: bool,
}

impl Default for TooltipLatch {
    fn default() -> Self {
        Self {
            open: false,
            via: TooltipOpenVia::Hover,
            enabled: true,
        }
    }
}

/// The shared, non-reactive handle a caller keeps one of per tooltip: whether
/// the panel should be showing, how it was opened, whether the trigger is
/// enabled, and the trigger's captured window-space rect (the merged
/// [`crate::overlay::anchored`] host's own [`OverlayAnchor`], carried inside
/// rather than threaded separately). See the [module docs](self).
///
/// Clone it — the clone shares the same cells. Hand one clone to
/// [`tooltip_trigger`], another to [`tooltip`]/[`rich_tooltip`].
#[derive(Clone, Debug, Default)]
pub struct TooltipHover {
    anchor: OverlayAnchor,
    latch: Rc<Cell<TooltipLatch>>,
}

impl TooltipHover {
    /// A fresh handle: closed, enabled, no trigger captured yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the panel should be showing.
    pub fn is_open(&self) -> bool {
        self.latch.get().open
    }

    /// Force the open state — for an app that wants to dismiss a tooltip from
    /// outside the trigger/panel pair (e.g. on navigation).
    pub fn set_open(&self, open: bool) {
        let mut l = self.latch.get();
        l.open = open;
        self.latch.set(l);
    }

    fn via(&self) -> TooltipOpenVia {
        self.latch.get().via
    }

    fn set_via(&self, via: TooltipOpenVia) {
        let mut l = self.latch.get();
        l.via = via;
        self.latch.set(l);
    }

    fn is_enabled(&self) -> bool {
        self.latch.get().enabled
    }

    fn set_enabled(&self, enabled: bool) {
        let mut l = self.latch.get();
        l.enabled = enabled;
        self.latch.set(l);
    }

    /// The captured trigger rect, riding the merged anchored host's own
    /// [`OverlayAnchor`].
    fn anchor(&self) -> &OverlayAnchor {
        &self.anchor
    }
}

// ---- The trigger --------------------------------------------------------------

/// Wrap `child` as a tooltip trigger writing into `hover`. See the [module
/// docs](self)' Hover vs. long-press section.
pub fn tooltip_trigger<State: 'static, V: View<State>>(
    hover: &TooltipHover,
    child: V,
) -> TooltipTriggerView<State> {
    let cb_hover = hover.clone();
    let gesture = GestureDetector(child)
        .on_long_press(move |_state: &mut State| {
            if cb_hover.is_enabled() {
                cb_hover.set_open(true);
                cb_hover.set_via(TooltipOpenVia::LongPress);
            }
        })
        .hold_threshold_ms(LONG_PRESS_MS);
    TooltipTriggerView {
        child: any(gesture),
        hover: hover.clone(),
        enabled: true,
    }
}

/// A declarative tooltip trigger. See [`tooltip_trigger`].
pub struct TooltipTriggerView<State: 'static> {
    child: AnyView<State>,
    hover: TooltipHover,
    enabled: bool,
}

impl<State: 'static> TooltipTriggerView<State> {
    /// Gate both trigger mechanisms. `true` by default. See the [module
    /// docs](self)' Hover vs. long-press section for what `false` does to an
    /// already-open tooltip.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// The retained widget for a [`TooltipTriggerView`].
pub struct TooltipTriggerWidget {
    child: ChildPod,
    hover: TooltipHover,
    was_hovered: bool,
}

impl<State: 'static> View<State> for TooltipTriggerView<State> {
    type Element = TooltipTriggerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TooltipTriggerWidget {
        self.hover.set_enabled(self.enabled);
        TooltipTriggerWidget {
            child: build_child(&self.child, ctx),
            hover: self.hover.clone(),
            was_hovered: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TooltipTriggerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.hover.set_enabled(self.enabled);
        element.hover = self.hover.clone();
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut TooltipTriggerWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for TooltipTriggerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.hover
            .anchor()
            .set(Rect::from_origin_size(ctx.origin(), ctx.size()));

        if self.hover.is_enabled() {
            // `PaintCtx::is_hovered` is authoritative — see the module docs'
            // Hover vs. long-press section.
            let now = ctx.is_hovered();
            if now != self.was_hovered {
                self.was_hovered = now;
                self.hover.set_open(now);
                if now {
                    self.hover.set_via(TooltipOpenVia::Hover);
                }
                ctx.request_frame();
            }
        } else if self.was_hovered {
            // Disabled mid-hover: drop the link and snap any open panel shut.
            self.was_hovered = false;
            self.hover.set_open(false);
            ctx.request_frame();
        }

        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Transparent wrapper: the nested `GestureDetector` (and whatever real
        // child it wraps) sees every event unchanged; this widget only claims
        // hover, after routing (the claim-ordering rule).
        let routed = route_event_single(&mut self.child, ctx, event);
        if !event.is_broadcast()
            && let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Move
            && inside(p.position, ctx.size())
        {
            ctx.claim_hover();
        }
        routed
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}

/// Whether local point `pos` is inside a widget of `size` — the same
/// hit-test [`crate::toggle_button`]'s own copy runs.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

// ---- A lazily-shaped, paint-time-rebrushed, wrap-capable text run ----------

/// The same idiom [`crate::snackbar`]'s `TextRun`/[`crate::badge`]'s
/// `LabelRun` use, generalized with a wrap width — upstream's plain `maxWidth`
/// and rich title/body all wrap, unlike those single-line callers.
struct TooltipTextRun {
    content: String,
    layout: Option<TextLayout>,
}

impl TooltipTextRun {
    fn new() -> Self {
        Self {
            content: String::new(),
            layout: None,
        }
    }

    fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
        }
    }

    fn shape(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: f64) -> Size {
        if let Some(layout) = &self.layout {
            return layout.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, Some(max_width as f32));
        let size = laid.size();
        self.layout = Some(laid);
        size
    }

    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let Some(layout) = &self.layout else {
            return;
        };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

// ---- Plain tooltip -----------------------------------------------------------

/// Build a plain tooltip over `hover`, to be mounted unconditionally beside
/// [`tooltip_trigger`]'s own view — the mount-on-open swap
/// happens internally (see the module docs' no-exit-fade section).
pub fn tooltip(hover: &TooltipHover, message: impl Into<String>) -> TooltipView {
    TooltipView {
        open: hover.is_open(),
        hover: hover.clone(),
        message: message.into(),
        placement: OverlayPlacement::default(),
    }
}

/// A declarative plain tooltip. See [`tooltip`].
pub struct TooltipView {
    /// [`TooltipHover::is_open`], **snapshotted at construction** — `rebuild`
    /// compares two successive `TooltipView` *values*, so `prev`'s own branch
    /// must reflect what `hover` held when `prev` was built, not the live
    /// (shared, mutable) cell's value right now. Reading `self.hover.is_open()`
    /// directly inside `content_view` would make `prev.content_view()` replay
    /// the *current* state instead of `prev`'s own, corrupting the `AnyView`
    /// type-swap the very frame that closes a tooltip (caught by this file's
    /// own `closing_tears_the_panel_down_on_the_next_rebuild_no_ramp_frames`
    /// test, which failed against a live read before this field existed).
    open: bool,
    hover: TooltipHover,
    message: String,
    placement: OverlayPlacement,
}

impl TooltipView {
    /// Set the side the panel opens on (`bottom` by default — upstream's own
    /// static placement, `targetAnchor: bottomCenter`/`followerAnchor:
    /// topCenter`).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self
    }

    /// Set the cross-axis alignment (`center` by default).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self
    }

    /// Set the gap between trigger and panel (`anchorOffset`, 4dp by default).
    pub fn offset(mut self, offset: f64) -> Self {
        self.placement.offset = offset;
        self
    }

    fn content_view<State: 'static>(&self) -> AnyView<State> {
        if self.open {
            any(anchored_overlay(PlainPanel {
                message: self.message.clone(),
                hover: self.hover.clone(),
            })
            .anchor(self.hover.anchor())
            .placement(self.placement))
        } else {
            any(SizedBox(None, None))
        }
    }
}

impl<State: 'static> View<State> for TooltipView {
    type Element = Box<dyn Widget>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        View::build(&self.content_view::<State>(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(
            &self.content_view::<State>(),
            &prev.content_view::<State>(),
            element,
            ctx,
        )
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.content_view::<State>(), element, ctx);
    }
}

/// The plain panel's content, hosted by [`anchored_overlay`]. Never
/// constructed by an app directly.
struct PlainPanel {
    message: String,
    hover: TooltipHover,
}

impl<State: 'static> View<State> for PlainPanel {
    type Element = PlainPanelWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> PlainPanelWidget {
        let mut run = TooltipTextRun::new();
        run.set_content(&self.message);
        PlainPanelWidget {
            message: run,
            hover: self.hover.clone(),
            dismiss_via_long_press: self.hover.via() == TooltipOpenVia::LongPress,
            timer_start: None,
            dismissed: false,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut PlainPanelWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.message.content != self.message {
            element.message.set_content(&self.message);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.hover = self.hover.clone();
        flags
    }

    fn teardown(&self, _element: &mut PlainPanelWidget, _ctx: &mut BuildCtx<'_>) {}
}

/// The retained widget for a [`PlainPanel`].
struct PlainPanelWidget {
    message: TooltipTextRun,
    hover: TooltipHover,
    /// Captured once at [`View::build`] — see the module docs' Plain section.
    dismiss_via_long_press: bool,
    timer_start: Option<FrameTime>,
    dismissed: bool,
}

impl Widget for PlainPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width_cap = bc.max().width.min(PLAIN_MAX_WIDTH);
        let inner_w = (width_cap - 2.0 * PLAIN_PAD_X).max(0.0);
        let msg = self.message.shape(ctx, &plain_message_style(), inner_w);
        let size = Size::new(
            msg.width + 2.0 * PLAIN_PAD_X,
            msg.height + 2.0 * PLAIN_PAD_Y,
        );
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let container = theme.map_or(FALLBACK_INVERSE_SURFACE, |t| t.scheme().inverse_surface);
        let ink = theme.map_or(FALLBACK_INVERSE_ON_SURFACE, |t| {
            t.scheme().inverse_on_surface
        });
        let radius = theme.map_or(MaterialDimensions::RADIUS_EXTRA_SMALL, |t| {
            t.shape.extra_small
        });
        let (origin, size) = (ctx.origin(), ctx.size());
        scene.fill_rounded_rect(origin, size, radius, container);
        self.message.paint(
            Point::new(origin.x + PLAIN_PAD_X, origin.y + PLAIN_PAD_Y),
            ink,
            scene,
        );

        // Auto-dismiss (long-press-opened only) — a one-shot paint-clock
        // timer, matching `crate::toggle_button::LongPressState`'s idiom but
        // firing straight from paint: the write below is a plain, non-reactive
        // `TooltipHover` cell, not an app-state mutation, so no
        // `mark_pending_result_flush`/`EventCtx` round trip is needed.
        if self.dismiss_via_long_press && !self.dismissed {
            let start = *self.timer_start.get_or_insert(ctx.frame_time());
            let elapsed_ms = ctx.frame_time().saturating_sub(start).as_secs_f64() * 1000.0;
            if elapsed_ms >= PLAIN_DISMISS_MS {
                self.dismissed = true;
                self.hover.set_open(false);
            }
            ctx.request_frame();
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(frust::authoring::Role::Label, |node| {
            node.set_label(self.message.content.clone());
        });
    }
}

// ---- Rich tooltip -------------------------------------------------------------

/// One rich-tooltip action button: a label plus its own press callback. Build
/// with [`tooltip_action`], attach with [`RichTooltipView::action`].
pub struct TooltipAction<State: 'static> {
    label: String,
    on_press: Rc<dyn Fn(&mut State)>,
}

impl<State: 'static> Clone for TooltipAction<State> {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            on_press: self.on_press.clone(),
        }
    }
}

/// Build a rich-tooltip action labeled `label`, firing `on_press`.
pub fn tooltip_action<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> TooltipAction<State> {
    TooltipAction {
        label: label.into(),
        on_press: Rc::new(on_press),
    }
}

/// Build a rich tooltip over `hover`, to be mounted unconditionally beside
/// [`tooltip_trigger`]'s own view. See [`tooltip`] for the mount contract this
/// mirrors, and [`RichTooltipView::title`]/[`RichTooltipView::action`] for the
/// upstream `richTitle`/`actions` slots.
pub fn rich_tooltip<State: 'static>(
    hover: &TooltipHover,
    message: impl Into<String>,
) -> RichTooltipView<State> {
    RichTooltipView {
        open: hover.is_open(),
        hover: hover.clone(),
        title: None,
        message: message.into(),
        actions: Vec::new(),
        placement: OverlayPlacement::default(),
    }
}

/// A declarative rich tooltip. See [`rich_tooltip`].
pub struct RichTooltipView<State: 'static> {
    /// [`TooltipHover::is_open`], snapshotted at construction — see
    /// [`TooltipView::open`]'s doc comment for why a live read here would
    /// corrupt `rebuild`'s `prev` comparison.
    open: bool,
    hover: TooltipHover,
    title: Option<String>,
    message: String,
    actions: Vec<TooltipAction<State>>,
    placement: OverlayPlacement,
}

impl<State: 'static> RichTooltipView<State> {
    /// Set the title (`richTitle`), shown above the supporting text.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Append an action button (`actions`). Upstream accepts any number
    /// (`Row(children: widget.actions)`, no cap in code); M3 guidance caps a
    /// rich tooltip at two, which this port does not enforce either — a
    /// documentation convention, not a code constraint upstream carries.
    pub fn action(mut self, action: TooltipAction<State>) -> Self {
        self.actions.push(action);
        self
    }

    /// Set the side the panel opens on (`bottom` by default).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self
    }

    /// Set the cross-axis alignment (`center` by default).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self
    }

    /// Set the gap between trigger and panel (`anchorOffset`, 4dp by default).
    pub fn offset(mut self, offset: f64) -> Self {
        self.placement.offset = offset;
        self
    }

    fn content_view(&self) -> AnyView<State> {
        if self.open {
            let dismiss_hover = self.hover.clone();
            any(anchored_overlay(RichPanel {
                title: self.title.clone(),
                message: self.message.clone(),
                actions: self.actions.clone(),
            })
            .anchor(self.hover.anchor())
            .placement(self.placement)
            .on_dismiss(move |_state: &mut State| dismiss_hover.set_open(false)))
        } else {
            any(SizedBox(None, None))
        }
    }
}

impl<State: 'static> View<State> for RichTooltipView<State> {
    type Element = Box<dyn Widget>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        View::build(&self.content_view(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.content_view(), &prev.content_view(), element, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.content_view(), element, ctx);
    }
}

/// Build the erased action-button view for one [`TooltipAction`].
fn action_view<State: 'static>(action: &TooltipAction<State>) -> AnyView<State> {
    let cb = action.on_press.clone();
    any(text_button(action.label.clone(), move |s: &mut State| {
        cb(s)
    }))
}

/// The rich panel's content, hosted by [`anchored_overlay`]. Never constructed
/// by an app directly.
struct RichPanel<State: 'static> {
    title: Option<String>,
    message: String,
    actions: Vec<TooltipAction<State>>,
}

impl<State: 'static> View<State> for RichPanel<State> {
    type Element = RichPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> RichPanelWidget {
        let title = self.title.as_ref().map(|t| {
            let mut run = TooltipTextRun::new();
            run.set_content(t);
            run
        });
        let mut body = TooltipTextRun::new();
        body.set_content(&self.message);
        let action_pods = self
            .actions
            .iter()
            .map(|a| build_child(&action_view(a), ctx))
            .collect();
        RichPanelWidget {
            title,
            body,
            action_pods,
            title_origin: Point::ORIGIN,
            body_origin: Point::ORIGIN,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RichPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        match (&self.title, &mut element.title) {
            (Some(t), Some(run)) => {
                if run.content != *t {
                    run.set_content(t);
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
            (Some(t), None) => {
                let mut run = TooltipTextRun::new();
                run.set_content(t);
                element.title = Some(run);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (None, Some(_)) => {
                element.title = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (None, None) => {}
        }
        if element.body.content != self.message {
            element.body.set_content(&self.message);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        let prev_views: Vec<AnyView<State>> = prev.actions.iter().map(action_view).collect();
        let next_views: Vec<AnyView<State>> = self.actions.iter().map(action_view).collect();
        flags |= rebuild_children(
            &prev_views,
            &next_views,
            &mut element.action_pods,
            ctx,
            |v: &AnyView<State>| v,
            |_| None,
        );
        flags
    }

    fn teardown(&self, element: &mut RichPanelWidget, ctx: &mut BuildCtx<'_>) {
        let views: Vec<AnyView<State>> = self.actions.iter().map(action_view).collect();
        for (pod, view) in element.action_pods.iter_mut().zip(views.iter()) {
            teardown_child(view, pod, ctx);
        }
    }
}

/// The retained widget for a [`RichPanel`].
struct RichPanelWidget {
    title: Option<TooltipTextRun>,
    body: TooltipTextRun,
    action_pods: Vec<ChildPod>,
    title_origin: Point,
    body_origin: Point,
}

impl Widget for RichPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width_cap = bc.max().width.min(RICH_MAX_WIDTH);
        let inner_w = (width_cap - 2.0 * RICH_PAD).max(0.0);
        let mut content_w: f64 = 0.0;
        let mut y = RICH_PAD;

        if let Some(title) = &mut self.title {
            let sz = title.shape(ctx, &rich_title_style(), inner_w);
            self.title_origin = Point::new(RICH_PAD, y);
            content_w = content_w.max(sz.width);
            y += sz.height + RICH_TITLE_GAP;
        }

        let body_sz = self.body.shape(ctx, &rich_body_style(), inner_w);
        self.body_origin = Point::new(RICH_PAD, y);
        content_w = content_w.max(body_sz.width);
        y += body_sz.height;

        if !self.action_pods.is_empty() {
            y += RICH_ACTIONS_GAP;
            let mut x = RICH_PAD;
            let mut row_h: f64 = 0.0;
            for pod in &mut self.action_pods {
                let sz = pod.layout_child(
                    ctx,
                    &BoxConstraints::loose(Size::new(inner_w, f64::INFINITY)),
                );
                pod.set_origin(Point::new(x, y));
                x += sz.width;
                row_h = row_h.max(sz.height);
            }
            content_w = content_w.max(x - RICH_PAD);
            y += row_h;
        }

        let panel_w = content_w.min(inner_w) + 2.0 * RICH_PAD;
        let panel_h = y + RICH_PAD;
        bc.constrain(Size::new(panel_w, panel_h))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let container = overlay_container(theme, OverlayContainer::Standard);
        let radius = theme.map_or(MaterialDimensions::RADIUS_MEDIUM, |t| t.shape.medium);
        let (origin, size) = (ctx.origin(), ctx.size());

        if let Some((blur, y_offset, color)) = overlay_shadow(theme, OverlayElevation::Level2) {
            scene.draw_shadow(
                Point::new(origin.x, origin.y + y_offset),
                size,
                radius,
                blur,
                color,
            );
        }
        scene.fill_rounded_rect(origin, size, radius, container);

        let title_ink = theme.map_or(FALLBACK_ON_SURFACE, |t| t.scheme().on_surface);
        let body_ink = on_surface_variant(theme);
        if let Some(title) = &self.title {
            title.paint(
                Point::new(
                    origin.x + self.title_origin.x,
                    origin.y + self.title_origin.y,
                ),
                title_ink,
                scene,
            );
        }
        self.body.paint(
            Point::new(origin.x + self.body_origin.x, origin.y + self.body_origin.y),
            body_ink,
            scene,
        );

        for pod in &mut self.action_pods {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in &mut self.action_pods {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        for pod in &mut self.action_pods {
            if pod.event_child(ctx, event) == EventResult::Handled {
                return EventResult::Handled;
            }
        }
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            frust::authoring::Role::Group,
            |_node| {},
            |ctx| {
                for pod in &self.action_pods {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(action_pods);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Brightness;
    use frust::authoring::{
        Point as AuthoringPoint, PointerButton, PointerEvent, Rect as AuthoringRect,
    };
    use frust_core::RenderRoot;
    use std::any::Any;

    fn ft(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    // ---- TooltipHover -------------------------------------------------------

    #[test]
    fn a_fresh_handle_is_closed_and_enabled() {
        let hover = TooltipHover::new();
        assert!(!hover.is_open());
        assert!(hover.is_enabled());
        assert_eq!(hover.via(), TooltipOpenVia::Hover);
    }

    #[test]
    fn clones_share_the_same_cells() {
        let hover = TooltipHover::new();
        let clone = hover.clone();
        clone.set_open(true);
        assert!(hover.is_open(), "the clone shares the latch");
        hover.anchor().set(AuthoringRect::new(1.0, 2.0, 3.0, 4.0));
        assert_eq!(
            clone.anchor().rect(),
            AuthoringRect::new(1.0, 2.0, 3.0, 4.0)
        );
    }

    // ---- Trigger: hover has no show-delay ------------------------------------

    #[derive(Default)]
    struct AppState;

    /// Mount the trigger over a fixed-size leaf, as the sole child of a
    /// `RenderRoot` — enough to drive `PaintCtx::is_hovered` through the real
    /// pipeline.
    struct Harness {
        root: RenderRoot<AppState, TooltipTriggerView<AppState>>,
        state: AppState,
        hover: TooltipHover,
        enabled: bool,
    }

    const TRIGGER: Size = Size::new(80.0, 32.0);

    impl Harness {
        fn new() -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState,
                hover: TooltipHover::new(),
                enabled: true,
            };
            h.pass();
            h.root.layout(Size::new(400.0, 400.0));
            h
        }

        fn pass(&mut self) {
            let hover = self.hover.clone();
            let enabled = self.enabled;
            let mut logic = move |_s: &mut AppState| {
                tooltip_trigger(
                    &hover,
                    frust::SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                )
                .enabled(enabled)
            };
            self.root.rebuild(&mut logic, &mut self.state);
        }

        fn paint(&mut self, ms: f64) {
            self.root.paint(&mut NullScene, ft(ms));
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }
    }

    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: AuthoringPoint, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: AuthoringPoint, _t: &str) {}
    }

    fn moved(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: AuthoringPoint::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn down(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: AuthoringPoint::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn up(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Up,
            position: AuthoringPoint::new(x, y),
            button: PointerButton::Primary,
        })
    }

    #[test]
    fn hover_opens_on_the_very_next_paint_zero_delay() {
        let mut h = Harness::new();
        h.paint(0.0);
        assert!(!h.hover.is_open());
        h.event(moved(10.0, 10.0));
        // No timer at all — the paint immediately following the hovering move
        // already opens it, matching upstream's synchronous `_show()`.
        h.paint(1.0);
        assert!(h.hover.is_open());
        assert_eq!(h.hover.via(), TooltipOpenVia::Hover);
    }

    #[test]
    fn hover_closes_the_instant_the_pointer_leaves() {
        let mut h = Harness::new();
        h.event(moved(10.0, 10.0));
        h.paint(1.0);
        assert!(h.hover.is_open());
        h.event(moved(500.0, 500.0));
        h.paint(2.0);
        assert!(!h.hover.is_open());
    }

    #[test]
    fn disabled_suppresses_hover_and_snaps_an_open_panel_shut() {
        let mut h = Harness::new();
        h.event(moved(10.0, 10.0));
        h.paint(1.0);
        assert!(h.hover.is_open());
        h.enabled = false;
        h.pass();
        h.root.layout(Size::new(400.0, 400.0));
        h.paint(2.0);
        assert!(!h.hover.is_open(), "disabling mid-show snaps it shut");
    }

    // ---- Trigger: long-press threshold (frame clock) -------------------------

    #[test]
    fn a_stationary_long_press_opens_after_the_threshold_via_housekeeping() {
        let mut h = Harness::new();
        h.event(down(10.0, 10.0));
        h.paint(0.0);
        let flags = h
            .root
            .paint(&mut NullScene, ft(LONG_PRESS_MS as f64 + 20.0));
        assert!(!h.hover.is_open(), "paint alone only latches, never fires");
        assert!(
            flags.needs_frame,
            "requests the follow-up frame that fires it"
        );

        // Every shell runs rebuild (which drains the flush) before its own
        // paint — `crate::toggle_button`'s own Long-press section documents
        // why a plugin reaches this only through `frust::GestureDetector`.
        h.pass();
        assert!(
            h.hover.is_open(),
            "fires via Housekeeping, no further pointer event"
        );
        assert_eq!(h.hover.via(), TooltipOpenVia::LongPress);
    }

    #[test]
    fn a_quick_release_before_the_threshold_never_opens() {
        let mut h = Harness::new();
        h.event(down(10.0, 10.0));
        h.paint(0.0);
        h.paint(100.0); // short of LONG_PRESS_MS
        h.event(up(11.0, 11.0));
        h.pass();
        assert!(!h.hover.is_open());
    }

    // ---- Plain: auto-dismiss timeline (frame clock) --------------------------

    fn plain_panel_widget(via: TooltipOpenVia) -> (PlainPanelWidget, TooltipHover) {
        let hover = TooltipHover::new();
        hover.set_open(true);
        hover.set_via(via);
        let view = PlainPanel {
            message: "hi".into(),
            hover: hover.clone(),
        };
        let mut counter = 0u64;
        let w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        (w, hover)
    }

    #[test]
    fn a_long_press_opened_plain_tooltip_auto_dismisses_at_extra_long_4() {
        let (mut w, hover) = plain_panel_widget(TooltipOpenVia::LongPress);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));

        let mut scene = NullScene;
        let mut ctx0 = PaintCtx::for_test(AuthoringPoint::ORIGIN, Size::new(400.0, 400.0), ft(0.0));
        w.paint(&mut ctx0, &mut scene);
        assert!(hover.is_open(), "not yet at the delay");

        let mut mid = PaintCtx::for_test(
            AuthoringPoint::ORIGIN,
            Size::new(400.0, 400.0),
            ft(PLAIN_DISMISS_MS - 1.0),
        );
        w.paint(&mut mid, &mut scene);
        assert!(hover.is_open(), "just short of it");

        let mut done = PaintCtx::for_test(
            AuthoringPoint::ORIGIN,
            Size::new(400.0, 400.0),
            ft(PLAIN_DISMISS_MS),
        );
        w.paint(&mut done, &mut scene);
        assert!(!hover.is_open(), "1000ms (EXTRA_LONG_4) auto-dismisses it");
    }

    #[test]
    fn a_hover_opened_plain_tooltip_never_auto_dismisses() {
        let (mut w, hover) = plain_panel_widget(TooltipOpenVia::Hover);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        let mut scene = NullScene;
        let mut ctx = PaintCtx::for_test(
            AuthoringPoint::ORIGIN,
            Size::new(400.0, 400.0),
            ft(PLAIN_DISMISS_MS * 10.0),
        );
        w.paint(&mut ctx, &mut scene);
        assert!(
            hover.is_open(),
            "hover-shown plain tooltips have no timer at all"
        );
    }

    // ---- No exit fade: mount-on-open ------------------------------------------

    #[test]
    fn closing_tears_the_panel_down_on_the_next_rebuild_no_ramp_frames() {
        let hover = TooltipHover::new();
        let view = tooltip(&hover, "hi");
        let mut counter = 0u64;
        let mut element = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        // Closed: the mount-on-open branch is a zero-size `SizedBox`.
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = element.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        assert_eq!(size, Size::ZERO);

        // Open it and rebuild in place: the concrete `Element` swaps type
        // (`AnyView`'s xilem-pattern teardown+rebuild), so this is the SAME
        // `rebuild` call that would, for a *kept-mounted* host, have started
        // an exit ramp the other way — here it only ever starts an entrance.
        hover.set_open(true);
        let mut counter2 = 0u64;
        let opened = tooltip(&hover, "hi");
        View::<AppState>::rebuild(
            &opened,
            &view,
            &mut element,
            &mut BuildCtx::new(&mut counter2),
        );
        let size = element.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        assert!(
            size.width > 0.0 && size.height > 0.0,
            "the panel is now mounted"
        );

        // Close it: the very next rebuild tears the panel widget down and
        // swaps back to the zero-size placeholder — no lingering frames, no
        // fade, unlike the anchored host's own kept-mounted exit ramp.
        hover.set_open(false);
        let mut counter3 = 0u64;
        let closed = tooltip(&hover, "hi");
        View::<AppState>::rebuild(
            &closed,
            &opened,
            &mut element,
            &mut BuildCtx::new(&mut counter3),
        );
        let size = element.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        assert_eq!(size, Size::ZERO, "gone the instant it closes, no exit ramp");
    }

    // ---- Rich: slots, dismiss rules ---------------------------------------------

    #[test]
    fn rich_never_arms_the_plain_auto_dismiss_timer() {
        // A rich panel's own widget carries no timer field at all — the
        // absence itself is the pin: `RichPanelWidget` has no
        // `dismiss_via_long_press`/`timer_start` the way `PlainPanelWidget`
        // does, so there is nothing to elapse. Exercised indirectly: a
        // long-press-opened rich tooltip stays open across many frames.
        let hover = TooltipHover::new();
        hover.set_open(true);
        hover.set_via(TooltipOpenVia::LongPress);
        let view: RichPanel<AppState> = RichPanel {
            title: Some("Title".into()),
            message: "Body copy".into(),
            actions: vec![],
        };
        let mut counter = 0u64;
        let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        let mut scene = NullScene;
        let mut ctx = PaintCtx::for_test(
            AuthoringPoint::ORIGIN,
            Size::new(400.0, 400.0),
            ft(PLAIN_DISMISS_MS * 5.0),
        );
        w.paint(&mut ctx, &mut scene);
        assert!(hover.is_open(), "no auto-dismiss timer for rich, ever");
    }

    #[test]
    fn rich_dismisses_on_a_tap_outside_via_the_hosts_own_light_dismiss() {
        let hover = TooltipHover::new();
        hover.set_open(true);
        hover
            .anchor()
            .set(AuthoringRect::new(100.0, 100.0, 180.0, 132.0));
        let mut root: RenderRoot<AppState, RichTooltipView<AppState>> = RenderRoot::new();
        let mut state = AppState;
        let mut logic = move |_s: &mut AppState| rich_tooltip(&hover, "Body").title("Title");
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        root.paint(&mut NullScene, ft(0.0));

        let outcome = root.event(&mut state, &down(5.0, 5.0));
        assert!(outcome.handled, "the host's own light dismiss consumes it");
    }

    // ---- Attribution smoke: MaterialTokens-only ------------------------------

    #[test]
    fn plain_and_rich_resolve_off_the_theme_not_literals() {
        let light = crate::baseline().with_brightness(Brightness::Light);
        let dark = crate::baseline().with_brightness(Brightness::Dark);
        assert_ne!(
            light.scheme().inverse_surface,
            dark.scheme().inverse_surface
        );
        assert_ne!(
            overlay_container(Some(&light), OverlayContainer::Standard),
            overlay_container(Some(&dark), OverlayContainer::Standard)
        );
    }
}
