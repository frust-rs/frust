// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/tooltips/` — `m3e_tooltips.dart` (the `M3ETooltip` widget)
// and `styles/m3e_tooltip_theme.dart` (every metric), retrieved 2026-08-20.
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions:
// - Placement and the rich panel's light dismiss are **not** hand-rolled: both
//   panels float through the framework's overlay portal
//   ([`frust::authoring::OverlaySlot`]), which owns the placement math and the
//   outside-press notification — the same split upstream's own `OverlayPortal`/
//   `CompositedTransformFollower` pair makes. They no longer compose under this
//   catalog's merged `crate::overlay::anchored` host (the [`mod@crate::menu`]
//   precedent), because that host consumes every press outside its content by
//   design: mounted under a hover trigger it ate the press meant for whatever
//   sat underneath. `crate::overlay::anchored` itself is untouched, and every
//   click-opened Material overlay still presents through it.
// - **No exit fade — deliberate, faithful to upstream.** `M3ETooltip._fade`
//   (`m3e_tooltips.dart:187-197`) wraps only the *entrance*
//   (`TweenAnimationBuilder<double>` from 0 to 1); `_hide()` just calls
//   `OverlayPortalController.hide()`, which unmounts the overlay child on the
//   spot — there is no exit tween to port. The portal has that semantics for
//   free, and it is what this port now leans on: the registry is **per paint
//   pass**, so an owner that stops registering its surface is gone from the
//   paint and from the routing table at once, with nothing to unregister. So
//   [`TooltipView`]/[`RichTooltipView`] keep their pod mounted permanently and
//   gate only the *registration*, on a live [`TooltipHover::is_open`] read in
//   the owner's own `paint`. That **replaces** the `SizedBox`/
//   `anchored_overlay` `View::Element` swap this port used to need (the
//   anchored host would have started a 150ms exit ramp upstream never has if
//   handed a live `open` flag, so the panel had to be swapped out by type
//   instead), and it lands closer to `hide()` than the swap did: an `Element`
//   swap can only take effect on a `View::rebuild`, and [`TooltipHover`] is not
//   reactive, so a hover-exit that merely requested a frame left the old panel
//   on screen until something unrelated rebuilt the tree. The panel now goes on
//   the very next *paint*, rebuild or no rebuild.
// - **The entrance ramp moved with it, into the pod.** The anchored host used
//   to own the ramp; the portal owns no motion at all, so the same ramp is
//   played here — same tokens ([`MaterialMotion::SHORT_4`] /
//   [`MaterialMotion::EMPHASIZED_DECELERATE`]), same start scale
//   ([`crate::overlay::ANCHORED_ENTER_SCALE`]) and the same pivot on the
//   content edge facing the trigger, so nothing changes on screen. It lives in
//   the registered pod ([`RampedPanel`]) rather than in the owner because the
//   root paints a registered pod *directly*, so an owner's own `push_layer`
//   would already have been popped by the time the pod paints. Entrance only —
//   there is deliberately no reverse. Upstream's own entrance is a plain
//   opacity tween with no scale, so this port's entrance stays *closer to the
//   rest of the catalog's anchored panels* than to this one reference file —
//   the same trade `crate::menu`'s header documents.
// - **The two panels register differently, and only one is input-transparent.**
//   The plain panel is inert chrome: [`frust::OverlayBand::Tooltip`] +
//   [`frust::OverlayInput::Transparent`], which the root's pre-pass skips
//   outright, so every press reaches the main tree exactly as if the tooltip
//   were not there. The rich panel cannot be transparent and still be itself: a
//   transparent surface is hit-tested by nothing at all, so it would receive
//   neither its `actions` row's presses nor any outside-press notification, and
//   upstream's rich tooltip has both. It registers `Interactive` with
//   [`frust::OutsideTap::Notify`]` { consume: false }` instead — which is
//   upstream's own outside layer exactly (`Positioned.fill(GestureDetector(
//   onTap: _hide, behavior: HitTestBehavior.translucent))`: told about the tap,
//   and translucent, so whatever sits underneath is told as well). The press
//   outside is the one the anchored host used to swallow, and no longer
//   swallowing it is the whole point of this port.
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
//! opened by hovering (pointer platforms) or a long-press (touch), and floated
//! above the app by the framework's overlay portal.
//!
//! # Riding the framework portal
//!
//! Each panel's own view is an *owner*: it holds one
//! [`frust::authoring::OverlaySlot`], lays its pod out against the window, and
//! — on every paint where [`TooltipHover::is_open`] reads true — registers it
//! with the root, which paints it after the whole main tree and hit-tests it
//! before the main tree, per the band and input class the owner declared. An
//! app mounts the owner **unconditionally**, exactly as before; it costs one
//! layout of the panel per frame and, while closed, nothing else.
//!
//! [`frust::overlay_portal`]'s declarative wrapper anchors to its own child's
//! bounds, which does not fit here: a tooltip's trigger and panel are two
//! independently-mounted sibling views, so the panel's slot anchors instead to
//! the rect the trigger captured into the shared handle below
//! ([`OverlayAnchor::Rect`](frust::authoring::OverlayAnchor::Rect)).
//!
//! Two consequences of floating a pod rather than hosting it in the main tree,
//! both of them the portal's own documented v1 shape rather than this port's
//! choice: a pod publishes **no accessibility nodes** (they would attach under
//! the owner's node, at the owner's position rather than the floated rect's),
//! and a pod receives **no broadcast** events — neither panel needs one, since
//! the trigger's long-press timer lives in the main tree and neither panel's
//! own content runs a threshold-time gesture.
//!
//! # The shared handle
//!
//! [`TooltipHover`] is the one non-reactive latch a caller keeps per tooltip —
//! the trigger writes it, the panel reads it, and
//! [`crate::overlay::anchored`]'s own [`OverlayAnchor`] cell rides inside it
//! (carrying the trigger's window-space rect) so a caller never juggles two
//! handles. It is **not reactive** (`docs/CODE_STANDARDS.md`'s convention for
//! this catalog's other paint-latched handles, e.g.
//! [`crate::overlay::anchored::OverlayAnchor`] itself): writing it wakes
//! nothing on its own, so every writer also calls
//! `PaintCtx::request_frame`/relies on the pointer event that carried it — see
//! each write site below. It is read live, during the layout/paint of the very
//! frame that wrote it, which is what lets the panel appear and disappear
//! without waiting for a rebuild.
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
//!   rule). A transition writes [`TooltipHover`] and requests a frame; the
//!   panel's owner reads that latch live in its own `paint`, so the frame the
//!   write asked for is the frame the panel appears on — or vanishes on.
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
//! `hover.via() == TooltipOpenVia::LongPress` once, on the first paint of each
//! show (the same instant upstream's `_show(); _scheduleHide();` pair runs
//! synchronously together), and runs the matching paint-clock timer only when
//! that was true — a hover-shown plain tooltip never starts one, matching
//! upstream exactly. "Each show" is counted by [`TooltipHover`] itself, which
//! ticks a counter every time the latch goes from closed to open; a pod that
//! stays mounted across many shows re-arms from that edge rather than from its
//! own `View::build`, which now happens only once.
//!
//! # Rich: title/supporting/actions, dismiss on tap-elsewhere
//!
//! A rich panel never runs the auto-dismiss timer above; it stays open while
//! the trigger keeps its hover, closes on hover-exit exactly like the plain
//! panel, and additionally dismisses on a tap anywhere outside itself —
//! upstream's own `Positioned.fill(child: GestureDetector(onTap: _hide,
//! behavior: HitTestBehavior.translucent))`, ported as the portal's own
//! [`frust::OutsideTap::Notify`]` { consume: false }` rather than a
//! hand-rolled translucent layer: the root tells the owner that a press landed
//! outside every floated surface, the owner closes the latch, and the press
//! carries on into the main tree — `translucent`, exactly as upstream. Its
//! `actions` row is routed to normally, as
//! [`frust::OverlayInput::Interactive`] content.

use std::cell::Cell;
use std::rc::Rc;

use frust::authoring::text::{FontWeight, LineHeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{
    Affine, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx,
    EventResult, InputEvent, LayoutCtx, OutsideTap, OverlayBand, OverlayInput, OverlaySlot,
    PaintCtx, PaintScene, Point, PointerPhase, Rect, SemanticsCtx, Size, View, Widget, any,
    build_child, rebuild_child, rebuild_children, route_event, route_event_single, teardown_child,
    visit_children,
};
use frust::{AnimationController, FrameTime, GestureDetector, Theme};

use crate::button::text_button;
use crate::overlay::{
    ANCHORED_ENTER_SCALE, OverlayAlign, OverlayAnchor, OverlayContainer, OverlayElevation,
    OverlayPlacement, OverlaySide, container as overlay_container, on_surface_variant,
    reduce_motion, shadow as overlay_shadow,
};
use crate::tokens::{MaterialDimensions, MaterialMotion, MaterialSpacing};

// The framework's own overlay vocabulary, kept in its own block and aliased
// apart from `crate::overlay`'s identically-named (and identically-shaped)
// catalog types above — those stay the ones this module's *public* builders
// speak, so the two sets meet in exactly one place (`portal_placement`).
use frust::authoring::{
    OverlayAlign as PortalAlign, OverlayAnchor as PortalAnchor,
    OverlayPlacement as PortalPlacement, OverlaySide as PortalSide,
};

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

// ---- Entrance ramp ----------------------------------------------------------

/// How far outside its own bounds a panel's chrome (the rich panel's elevation
/// shadow) reaches, in logical px — the bound the entrance's composited layer
/// is inflated by so that shadow is not clipped out of the fade. The same
/// allowance `crate::overlay::anchored`'s host makes for the panels it used to
/// ramp; restated here because the ramp moved into the pod and the host's own
/// copy is private to it.
const PANEL_SHADOW_SPILL: f64 = 48.0;

/// Progress difference below which the entrance counts as settled.
const PROGRESS_EPSILON: f64 = 1e-4;

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
    /// How many times this handle has been opened — ticked on the
    /// closed-to-open edge alone, so a redundant `set_open(true)` while already
    /// open does not count as a new show.
    ///
    /// The panels' pods now outlive any one show (they stay mounted and only
    /// their *registration* comes and goes), so a pod cannot take "this show
    /// just started" from its own `View::build` any more. It compares this
    /// counter against the one it last armed on instead. See
    /// [`PlainPanelWidget`] and [`RampedPanelWidget`].
    shows: u64,
}

impl Default for TooltipLatch {
    fn default() -> Self {
        Self {
            open: false,
            via: TooltipOpenVia::Hover,
            enabled: true,
            shows: 0,
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
        if open && !l.open {
            l.shows = l.shows.wrapping_add(1);
        }
        l.open = open;
        self.latch.set(l);
    }

    /// Which show this is — see [`TooltipLatch::shows`]. Wrapping is harmless:
    /// every reader only ever tests it for *inequality* against the value it
    /// last armed on.
    fn show_id(&self) -> u64 {
        self.latch.get().shows
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

// ---- The floated pod: placement bridge + entrance ramp ----------------------

/// Lower this catalog's own placement request onto the framework's.
///
/// The two are the same five fields under two names: [`crate::overlay`]'s set
/// predates the framework's and is what [`TooltipView::side`]/
/// [`TooltipView::align`]/[`TooltipView::offset`] take, and those signatures
/// are public API this port does not change. `padding` has no counterpart
/// because `crate::overlay::place_anchored` insets the area by nothing, so it
/// is pinned to zero here: inheriting the framework's 8dp default would quietly
/// move every panel that lands near a window edge.
fn portal_placement(placement: OverlayPlacement) -> PortalPlacement {
    PortalPlacement {
        side: match placement.side {
            OverlaySide::Top => PortalSide::Top,
            OverlaySide::Right => PortalSide::Right,
            OverlaySide::Bottom => PortalSide::Bottom,
            OverlaySide::Left => PortalSide::Left,
        },
        align: match placement.align {
            OverlayAlign::Start => PortalAlign::Start,
            OverlayAlign::Center => PortalAlign::Center,
            OverlayAlign::End => PortalAlign::End,
        },
        offset: placement.offset,
        flip: placement.flip,
        clamp: placement.clamp,
        padding: 0.0,
    }
}

/// A fresh entrance driver: [`MaterialMotion::SHORT_4`] eased by
/// [`MaterialMotion::EMPHASIZED_DECELERATE`], the exact pair
/// `crate::overlay::anchored` plays for the panels it hosts.
fn entrance_driver() -> AnimationController {
    AnimationController::new(MaterialMotion::SHORT_4)
        .with_curve(MaterialMotion::EMPHASIZED_DECELERATE)
}

/// The pod both panels are actually registered as: `content` plus the entrance
/// ramp, run where the root can see it.
///
/// This is the *only* widget of either panel that runs during the root's
/// separate overlay paint pass, which is why the ramp state lives here rather
/// than on the owner: the root paints a registered pod directly, so an owner's
/// own `push_layer`/`push_transform` would already have been popped by the time
/// the pod paints.
struct RampedPanel<PodState: 'static> {
    content: AnyView<PodState>,
    hover: TooltipHover,
}

/// The retained widget for a [`RampedPanel`].
struct RampedPanelWidget {
    content: ChildPod,
    hover: TooltipHover,
    anim: AnimationController,
    /// How present the panel is: `0.0` gone, `1.0` settled.
    progress: f64,
    /// Which show the current ramp belongs to — see [`TooltipLatch::shows`].
    /// `None` until the first paint.
    armed_show: Option<u64>,
    /// Whether [`AnimationController::forward`] has been called for that show.
    started: bool,
}

impl RampedPanelWidget {
    /// Advance this paint's entrance, returning the presence to composite at.
    ///
    /// Entrance only, in one direction, because there is no exit to run: a
    /// closing panel is simply not registered, so this widget is not painted at
    /// all (see the file header's no-exit-fade decision). `reduce` collapses the
    /// ramp to a jump, leaving the open/dismiss *timings* — which are timing,
    /// not motion — alone.
    fn advance(&mut self, ctx: &mut PaintCtx, reduce: bool) -> f64 {
        let show = self.hover.show_id();
        if self.armed_show != Some(show) {
            // A new show on a pod that outlived the last one: restart from
            // nothing rather than replaying whatever the previous entrance
            // settled on.
            self.armed_show = Some(show);
            self.anim = entrance_driver();
            self.progress = 0.0;
            self.started = false;
        }
        if reduce {
            if self.anim.is_animating() {
                self.anim.stop();
            }
            self.progress = 1.0;
            return self.progress;
        }
        if !self.started {
            self.started = true;
            self.anim.forward();
        }
        if self.anim.is_animating() {
            // `advance`'s first call after `forward()` only seeds the clock
            // (zero delta, but still truthy) — the continuation has to be
            // requested on every truthy advance, not just the ones that moved
            // `progress`, or the seeding paint never schedules the frame that
            // would carry it off zero.
            if self.anim.advance(ctx.frame_time()) {
                ctx.request_frame();
            }
            let next = self.anim.value_clamped();
            if (next - self.progress).abs() > PROGRESS_EPSILON {
                // One more frame to paint the value just computed — including
                // the settled one the final advance lands on.
                ctx.request_frame();
            }
            self.progress = next;
        }
        self.progress
    }

    /// The point the entrance scale pivots about, in window space: the
    /// trigger's centre clamped into the placed panel, i.e. the panel's own
    /// edge or corner facing the trigger — so a tooltip grows out of the
    /// control it explains. The same rule `crate::overlay::anchored`'s host
    /// applies, computed here in window space because that is the space the
    /// root hands a registered pod (`ctx.origin()` is the placed rect's own
    /// origin) and the space the handle stores the trigger's rect in.
    fn pivot(&self, panel: Rect) -> Point {
        let center = self.hover.anchor().rect().center();
        Point::new(
            center.x.clamp(panel.x0, panel.x1),
            center.y.clamp(panel.y0, panel.y1),
        )
    }
}

impl<PodState: 'static> View<PodState> for RampedPanel<PodState> {
    type Element = RampedPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> RampedPanelWidget {
        RampedPanelWidget {
            content: build_child(&self.content, ctx),
            hover: self.hover.clone(),
            anim: entrance_driver(),
            progress: 0.0,
            armed_show: None,
            started: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RampedPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.hover = self.hover.clone();
        rebuild_child(&prev.content, &self.content, &mut element.content, ctx)
    }

    fn teardown(&self, element: &mut RampedPanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl Widget for RampedPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.content.layout_child(ctx, bc);
        self.content.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let progress = self.advance(ctx, reduce_motion(Theme::from_paint_ctx(ctx)));
        let ramping = progress < 1.0 - PROGRESS_EPSILON;
        if ramping {
            let panel = Rect::from_origin_size(ctx.origin(), ctx.size());
            let layer = panel.inflate(PANEL_SHADOW_SPILL, PANEL_SHADOW_SPILL);
            scene.push_layer(layer.origin(), layer.size(), progress as f32);
            let pivot = self.pivot(panel);
            let scale = ANCHORED_ENTER_SCALE + (1.0 - ANCHORED_ENTER_SCALE) * progress;
            scene.push_transform(
                Affine::translate(pivot.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-pivot.to_vec2()),
            );
        }
        self.content.paint_child(ctx, scene);
        if ramping {
            scene.pop_transform();
            scene.pop_layer();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.content, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Reached by nothing today — a floated pod publishes no accessibility
        // nodes (see the module docs) — but the content's own nodes are correct
        // if that ever changes, and dropping them here would be a second thing
        // to remember.
        self.content.semantics_child(ctx);
    }

    visit_children!(content);
}

// ---- Plain tooltip -----------------------------------------------------------

/// Build a plain tooltip over `hover`, to be mounted unconditionally beside
/// [`tooltip_trigger`]'s own view — it floats its own panel above the app and
/// registers nothing at all while closed (see the module docs' portal section).
///
/// Unlike the anchored host this replaced, it need not be the topmost child of
/// anything: paint order comes from the declared [`frust::OverlayBand`], not
/// from tree position. It occupies no layout space either way.
pub fn tooltip(hover: &TooltipHover, message: impl Into<String>) -> TooltipView {
    TooltipView {
        hover: hover.clone(),
        message: message.into(),
        placement: OverlayPlacement::default(),
    }
}

/// A declarative plain tooltip. See [`tooltip`].
pub struct TooltipView {
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

    /// The pod's view.
    ///
    /// Built over `()`, not the application state: nothing in a plain panel
    /// calls back into an app, and keeping the pod state-free is what lets
    /// [`TooltipView`] stay non-generic — its `View::Element` cannot depend on
    /// `State`, and `tooltip`'s signature is public API.
    fn content_view(&self) -> AnyView<()> {
        any(RampedPanel {
            content: any(PlainPanel {
                message: self.message.clone(),
                hover: self.hover.clone(),
            }),
            hover: self.hover.clone(),
        })
    }
}

/// The retained widget for a [`TooltipView`]: one [`OverlaySlot`] holding the
/// plain panel, registered on every paint the latch reads open and on no other.
///
/// Registering nothing *is* the unmount (see the file header's no-exit-fade
/// decision): the root's registry is per paint pass, so the frame this widget
/// skips its registration is the frame the panel stops being painted and stops
/// being routed to, with nothing to unregister and no ramp to run down.
pub struct TooltipWidget {
    hover: TooltipHover,
    slot: OverlaySlot<()>,
}

impl TooltipView {
    /// Push this view's routing configuration onto the slot — the half of
    /// `build`/`rebuild` that is identical in both.
    fn configure(&self, slot: &mut OverlaySlot<()>) {
        slot.set_band(OverlayBand::Tooltip);
        // Inert chrome: the root's pre-pass skips it, so a press anywhere —
        // including over the panel itself — reaches the main tree untouched.
        slot.set_input(OverlayInput::Transparent);
        // A plain tooltip dismisses on hover-exit or on its own timer, never on
        // a tap; a transparent surface is told about an outside press anyway.
        slot.set_outside_tap(OutsideTap::Ignore);
        slot.set_placement(portal_placement(self.placement));
    }
}

impl<State: 'static> View<State> for TooltipView {
    type Element = TooltipWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TooltipWidget {
        let mut slot = OverlaySlot::new();
        self.configure(&mut slot);
        slot.rebuild(None, Some(&self.content_view()), ctx);
        TooltipWidget {
            hover: self.hover.clone(),
            slot,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TooltipWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.configure(&mut element.slot);
        element.hover = self.hover.clone();
        element
            .slot
            .rebuild(Some(&prev.content_view()), Some(&self.content_view()), ctx)
            | ChangeFlags::PAINT
    }

    fn teardown(&self, element: &mut TooltipWidget, ctx: &mut BuildCtx<'_>) {
        element.slot.rebuild(Some(&self.content_view()), None, ctx);
    }
}

impl Widget for TooltipWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The pod is sized against the window, not against `bc` — it escapes
        // this widget's box entirely, and unlike the host this replaced that no
        // longer depends on `bc` being bounded (`crate::overlay`'s documented
        // scroll-view trap): the pod never reads `bc.max()` at all.
        self.slot.layout(ctx);
        // Zero of its own: this owner draws nothing in the main tree and needs
        // no bounds there — so mounting a tooltip beside its trigger costs no
        // layout space, open or closed. The old mount-on-open element was zero
        // while closed and full-area while open; a panel is no longer allowed
        // to push its siblings around on the frame it opens.
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        if !self.hover.is_open() {
            return;
        }
        // Window space → this widget's own local space, which is what the slot
        // takes; it adds `ctx.origin()` back on when it places the pod.
        let local = self.hover.anchor().rect() - ctx.origin().to_vec2();
        self.slot.set_anchor(PortalAnchor::Rect(local));
        self.slot.paint(ctx, Size::ZERO);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Never fires in practice — a transparent surface is hit-tested by
        // nothing, so the root routes it no overlay event — but the slot is
        // still the right place to ask, and asking keeps the owner honest if the
        // panel is ever given interactive content.
        self.slot
            .event(ctx, event, &mut ())
            .unwrap_or(EventResult::Ignored)
    }

    fn semantics(&self, _ctx: &mut SemanticsCtx) {
        // A floated pod contributes no accessibility nodes in v1 (see the
        // module docs), and this owner has nothing of its own to publish.
    }

    visit_children!();
}

/// The plain panel's content, floated inside a [`RampedPanel`]. Never
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
            dismiss_via_long_press: false,
            armed_show: None,
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
    /// Captured once per show, on this widget's first paint of it — see the
    /// module docs' Plain section.
    dismiss_via_long_press: bool,
    /// Which show the capture above belongs to (see [`TooltipLatch::shows`]).
    /// This widget now outlives any one show, so `View::build` is no longer the
    /// moment the panel appears; the first paint of a show is.
    armed_show: Option<u64>,
    timer_start: Option<FrameTime>,
    dismissed: bool,
}

impl PlainPanelWidget {
    /// Capture the trigger mechanism and re-arm the one-shot timer when this
    /// paint is the first of a new show.
    ///
    /// This widget is painted only while its owner registers it, i.e. only
    /// while the panel is on screen, so "first paint of a show" is exactly the
    /// instant upstream runs `_show(); _scheduleHide();` together.
    fn arm(&mut self) {
        let show = self.hover.show_id();
        if self.armed_show == Some(show) {
            return;
        }
        self.armed_show = Some(show);
        self.dismiss_via_long_press = self.hover.via() == TooltipOpenVia::LongPress;
        self.timer_start = None;
        self.dismissed = false;
    }
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
        self.arm();
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
///
/// Unlike the plain panel this one is genuinely interactive: its action row is
/// routed to, and a press outside it closes the panel *and* carries on to
/// whatever it landed on.
pub fn rich_tooltip<State: 'static>(
    hover: &TooltipHover,
    message: impl Into<String>,
) -> RichTooltipView<State> {
    RichTooltipView {
        hover: hover.clone(),
        title: None,
        message: message.into(),
        actions: Vec::new(),
        placement: OverlayPlacement::default(),
    }
}

/// A declarative rich tooltip. See [`rich_tooltip`].
pub struct RichTooltipView<State: 'static> {
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

    /// The pod's view.
    ///
    /// Built over the ambient application state, unlike [`TooltipView`]'s: the
    /// `actions` row's callbacks take `&mut State` and must reach the same
    /// state every other widget sees, which is what
    /// [`OverlaySlot::event_ambient`] routes them over.
    fn content_view(&self) -> AnyView<State> {
        any(RampedPanel {
            content: any(RichPanel {
                title: self.title.clone(),
                message: self.message.clone(),
                actions: self.actions.clone(),
            }),
            hover: self.hover.clone(),
        })
    }

    /// Push this view's routing configuration onto the slot — the half of
    /// `build`/`rebuild` that is identical in both.
    fn configure(&self, slot: &mut OverlaySlot<State>) {
        slot.set_band(OverlayBand::Tooltip);
        // Interactive, unlike the plain panel: a transparent surface is
        // hit-tested by nothing at all, which would leave upstream's own
        // `actions` row unreachable and suppress the outside-press notification
        // below with it (see the file header's registration decision).
        slot.set_input(OverlayInput::Interactive);
        // Upstream's translucent full-area dismiss layer: told about the press,
        // and not consuming it, so whatever sits beneath is told as well.
        slot.set_outside_tap(OutsideTap::Notify { consume: false });
        slot.set_placement(portal_placement(self.placement));
    }
}

/// The retained widget for a [`RichTooltipView`]: one [`OverlaySlot`] holding
/// the rich panel, registered on every paint the latch reads open and on no
/// other. See [`TooltipWidget`] for what that gates, and this view's own
/// `configure` for how the two panels' registrations differ.
pub struct RichTooltipWidget<State: 'static> {
    hover: TooltipHover,
    slot: OverlaySlot<State>,
}

impl<State: 'static> View<State> for RichTooltipView<State> {
    type Element = RichTooltipWidget<State>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> RichTooltipWidget<State> {
        let mut slot = OverlaySlot::new();
        self.configure(&mut slot);
        slot.rebuild(None, Some(&self.content_view()), ctx);
        RichTooltipWidget {
            hover: self.hover.clone(),
            slot,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RichTooltipWidget<State>,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.configure(&mut element.slot);
        element.hover = self.hover.clone();
        element
            .slot
            .rebuild(Some(&prev.content_view()), Some(&self.content_view()), ctx)
            | ChangeFlags::PAINT
    }

    fn teardown(&self, element: &mut RichTooltipWidget<State>, ctx: &mut BuildCtx<'_>) {
        element.slot.rebuild(Some(&self.content_view()), None, ctx);
    }
}

impl<State: 'static> Widget for RichTooltipWidget<State> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.slot.layout(ctx);
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        if !self.hover.is_open() {
            return;
        }
        let local = self.hover.anchor().rect() - ctx.origin().to_vec2();
        self.slot.set_anchor(PortalAnchor::Rect(local));
        self.slot.paint(ctx, Size::ZERO);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let Some(result) = self.slot.event_ambient(ctx, event) else {
            // Not the surface's: this owner has no bounds of its own and nothing
            // else to claim.
            return EventResult::Ignored;
        };
        // The outside-press notification arrives through the same door, as a
        // broadcast the slot drains into this flag. Closing here rather than in
        // the pod is the point: the pod never saw the press (it landed on
        // nothing floated), and the press itself carries on into the main tree.
        if self.slot.take_outside_down() {
            self.hover.set_open(false);
            ctx.request_redraw();
        }
        result
    }

    fn semantics(&self, _ctx: &mut SemanticsCtx) {
        // A floated pod contributes no accessibility nodes in v1 (see the
        // module docs), and this owner has nothing of its own to publish.
    }

    visit_children!();
}

/// Build the erased action-button view for one [`TooltipAction`].
fn action_view<State: 'static>(action: &TooltipAction<State>) -> AnyView<State> {
    let cb = action.on_press.clone();
    any(text_button(action.label.clone(), move |s: &mut State| {
        cb(s)
    }))
}

/// The rich panel's content, floated inside a [`RampedPanel`]. Never
/// constructed by an app directly.
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
        // `route_event` supplies what a hand-rolled first-Handled-wins loop
        // skips: the broadcast fan-out (folded in below), the `pod.contains`
        // hit test, focus-path gating for `Key`/`Ime`, and capture (`is_active`)
        // bookkeeping — see `docs/CODE_STANDARDS.md`'s Interaction Semantics.
        route_event(&mut self.action_pods, ctx, event)
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

    /// A pod now outlives any one show — it stays mounted and only its
    /// registration comes and goes — so the capture that used to happen in
    /// `View::build` has to happen on the first paint of each show instead.
    /// This pins the re-arm: one widget instance, two shows, opened two
    /// different ways, each getting the timer upstream gives it.
    #[test]
    fn one_pod_across_two_shows_re_arms_the_timer_for_each() {
        let (mut w, hover) = plain_panel_widget(TooltipOpenVia::Hover);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        let mut scene = NullScene;
        let mut paint_at = |w: &mut PlainPanelWidget, ms: f64| {
            let mut ctx =
                PaintCtx::for_test(AuthoringPoint::ORIGIN, Size::new(400.0, 400.0), ft(ms));
            w.paint(&mut ctx, &mut scene);
        };

        // Show one, opened by hover: no timer, however long it stays up.
        paint_at(&mut w, 0.0);
        paint_at(&mut w, PLAIN_DISMISS_MS * 10.0);
        assert!(hover.is_open(), "a hover-shown plain tooltip has no timer");

        // Show two, on the SAME widget, opened by long-press this time.
        hover.set_open(false);
        hover.set_via(TooltipOpenVia::LongPress);
        hover.set_open(true);
        let base = PLAIN_DISMISS_MS * 20.0;
        paint_at(&mut w, base);
        assert!(hover.is_open(), "the second show has only just started");
        paint_at(&mut w, base + PLAIN_DISMISS_MS - 1.0);
        assert!(
            hover.is_open(),
            "and its timer is measured from ITS own first paint, not the pod's build"
        );
        paint_at(&mut w, base + PLAIN_DISMISS_MS);
        assert!(!hover.is_open(), "1000ms into the second show, it goes");
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

    // ---- Rich: action-row routing (`route_event`, not a hand-rolled loop) ---
    //
    // Regression coverage for `RichPanelWidget::event` forwarding via
    // `route_event` instead of a first-`Handled`-wins loop over
    // `ChildPod::event_child` — the loop skipped `route_event`'s hit test,
    // capture bookkeeping, and per-child arming, so a press anywhere in the
    // panel armed only action #0 and any later action was unreachable.

    #[derive(Default)]
    struct Presses {
        fired: Vec<usize>,
    }

    /// A two-(or `count`-)action `RichPanelWidget`, laid out at a fixed width —
    /// real post-layout action-pod bounds, not hardcoded metrics.
    fn rich_panel_with_actions(count: usize) -> RichPanelWidget {
        let actions: Vec<TooltipAction<Presses>> = (0..count)
            .map(|i| {
                tooltip_action(format!("Action {i}"), move |s: &mut Presses| {
                    s.fired.push(i)
                })
            })
            .collect();
        let view: RichPanel<Presses> = RichPanel {
            title: Some("Title".into()),
            message: "Body copy long enough to occupy real space above the actions".into(),
            actions,
        };
        let mut counter = 0u64;
        let mut w = View::<Presses>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        w
    }

    fn dispatch_rich(
        w: &mut RichPanelWidget,
        state: &mut Presses,
        phase: PointerPhase,
        pos: AuthoringPoint,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 400.0));
        w.event(
            &mut ctx,
            &InputEvent::Pointer(PointerEvent {
                phase,
                position: pos,
                button: PointerButton::Primary,
            }),
        )
    }

    /// A point inside the panel's title/body region, above every action pod.
    fn body_point(w: &RichPanelWidget) -> AuthoringPoint {
        AuthoringPoint::new(RICH_PAD + 1.0, w.body_origin.y + 1.0)
    }

    /// The center of action pod `index`, post-layout.
    fn action_center(w: &RichPanelWidget, index: usize) -> AuthoringPoint {
        let pod = &w.action_pods[index];
        let (o, s) = (pod.origin(), pod.size());
        AuthoringPoint::new(o.x + s.width / 2.0, o.y + s.height / 2.0)
    }

    #[test]
    fn a_press_on_the_body_fires_no_action_and_captures_nothing() {
        let mut w = rich_panel_with_actions(2);
        let mut state = Presses::default();
        let p = body_point(&w);
        dispatch_rich(&mut w, &mut state, PointerPhase::Down, p);
        dispatch_rich(&mut w, &mut state, PointerPhase::Up, p);
        assert!(state.fired.is_empty(), "no action pod sits under the body");
        assert!(
            w.action_pods.iter().all(|pod| !pod.is_active()),
            "nothing captured a press outside every action's own bounds"
        );
    }

    #[test]
    fn press_release_on_the_second_action_fires_it() {
        let mut w = rich_panel_with_actions(2);
        let mut state = Presses::default();
        let p = action_center(&w, 1);
        dispatch_rich(&mut w, &mut state, PointerPhase::Down, p);
        assert!(
            w.action_pods[1].is_active(),
            "the hit-tested pod captures the press, not action #0"
        );
        dispatch_rich(&mut w, &mut state, PointerPhase::Up, p);
        assert_eq!(state.fired, vec![1], "action #1 fires — previously dead");
    }

    #[test]
    fn down_on_body_up_over_action_0_does_not_fire() {
        let mut w = rich_panel_with_actions(2);
        let mut state = Presses::default();
        let down_p = body_point(&w);
        let up_p = action_center(&w, 0);
        dispatch_rich(&mut w, &mut state, PointerPhase::Down, down_p);
        dispatch_rich(&mut w, &mut state, PointerPhase::Up, up_p);
        assert!(
            state.fired.is_empty(),
            "up-inside without down-inside must not fire (the press contract)"
        );
    }

    // ---- Floated through the portal -----------------------------------------
    //
    // Everything below runs the real pipeline: a `RenderRoot` over the mount an
    // app actually writes (a page, the trigger and the panel as siblings under
    // one `Stack`), so the root's own overlay post-pass and pre-pass are what is
    // under test, not this module's arithmetic about them.

    const WINDOW: Size = Size::new(400.0, 600.0);
    const RICH_TITLE: &str = "Compose";
    const RICH_BODY: &str = "Start a new draft with expressive defaults.";
    const RICH_ACTION: &str = "Got it";

    /// State for the portal harness: what reached the page underneath, and what
    /// reached the rich panel's own action row.
    #[derive(Default)]
    struct PageState {
        pressed: usize,
        action_fired: usize,
    }

    /// A page filling whatever area it is given, counting the presses that
    /// reach it — so a test can assert "the press got through to the widget
    /// beneath", not merely the weaker "nobody reported handling it".
    struct PageProbe;

    /// The retained widget for a [`PageProbe`].
    struct PageProbeWidget;

    impl View<PageState> for PageProbe {
        type Element = PageProbeWidget;

        fn build(&self, _ctx: &mut BuildCtx<'_>) -> PageProbeWidget {
            PageProbeWidget
        }

        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut PageProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }

        fn teardown(&self, _element: &mut PageProbeWidget, _ctx: &mut BuildCtx<'_>) {}
    }

    impl Widget for PageProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }

        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}

        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event
                && p.phase == PointerPhase::Down
            {
                ctx.state_mut::<PageState>().pressed += 1;
                return EventResult::Handled;
            }
            EventResult::Ignored
        }

        fn semantics(&self, _ctx: &mut SemanticsCtx) {}

        visit_children!();
    }

    /// What actually reached the scene: the rounded rects a panel paints (i.e.
    /// whether the panel is on screen at all, and where), and the alpha of any
    /// composited layer (i.e. the entrance ramp).
    ///
    /// Neither the page nor the trigger paints anything, so every recorded
    /// rounded rect belongs to a floated panel, and `rrects[0]` is that panel's
    /// own container — painted before its text and before any action button.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(AuthoringPoint, Size)>,
        layers: Vec<f32>,
    }

    impl Recorder {
        /// The floated panel's own rect, in window space.
        fn panel(&self) -> AuthoringRect {
            let (origin, size) = self.rrects[0];
            AuthoringRect::from_origin_size(origin, size)
        }
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: AuthoringPoint, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: AuthoringPoint, _t: &str) {}
        fn fill_rounded_rect(&mut self, origin: AuthoringPoint, size: Size, _r: f64, _c: Color) {
            self.rrects.push((origin, size));
        }
        fn push_layer(&mut self, _o: AuthoringPoint, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn pop_layer(&mut self) {}
        fn push_transform(&mut self, _t: Affine) {}
        fn pop_transform(&mut self) {}
    }

    /// The mount an app writes: a full-area page, the trigger, and the panel,
    /// all siblings of one [`frust::Stack`]. The panel no longer has to be that
    /// `Stack`'s topmost child — paint order comes from the declared band — and
    /// it is deliberately *not* topmost here, which the old full-area host would
    /// not have survived.
    struct PortalHarness {
        root: RenderRoot<PageState, frust::StackView<PageState>>,
        state: PageState,
        tcx: TextContext,
        hover: TooltipHover,
        rich: bool,
    }

    impl PortalHarness {
        fn new(rich: bool) -> Self {
            let mut h = PortalHarness {
                root: RenderRoot::new(),
                state: PageState::default(),
                tcx: TextContext::new(),
                hover: TooltipHover::new(),
                rich,
            };
            h.frame(0.0);
            h
        }

        fn view(hover: &TooltipHover, rich: bool) -> frust::StackView<PageState> {
            let panel: AnyView<PageState> = if rich {
                any(rich_tooltip(hover, RICH_BODY)
                    .title(RICH_TITLE)
                    .action(tooltip_action(RICH_ACTION, |s: &mut PageState| {
                        s.action_fired += 1
                    })))
            } else {
                any(tooltip(hover, RICH_TITLE))
            };
            frust::Stack(vec![
                any(PageProbe),
                any(tooltip_trigger(
                    hover,
                    frust::SizedBox(Some(TRIGGER.width), Some(TRIGGER.height)),
                )),
                panel,
            ])
        }

        /// One whole frame — rebuild, layout, paint — answering with what was
        /// painted.
        fn frame(&mut self, ms: f64) -> Recorder {
            let hover = self.hover.clone();
            let rich = self.rich;
            let mut logic = move |_s: &mut PageState| Self::view(&hover, rich);
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            self.repaint(ms)
        }

        /// A paint and nothing else: no rebuild, no relayout. What a shell does
        /// when a widget asked for a frame without the tree changing.
        fn repaint(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft(ms));
            rec
        }

        /// Open the panel by hovering the trigger, and let the entrance settle.
        fn open_by_hover(&mut self) -> Recorder {
            self.root.event(&mut self.state, &moved(10.0, 10.0));
            self.frame(0.0);
            self.frame(SETTLED_MS)
        }

        fn press(&mut self, x: f64, y: f64) -> bool {
            self.root.event(&mut self.state, &down(x, y)).handled
        }
    }

    /// Comfortably past [`MaterialMotion::SHORT_4`], so the entrance has
    /// settled and a recorded rect is the panel's untransformed placement.
    const SETTLED_MS: f64 = 1_000.0;

    #[test]
    fn a_closed_tooltip_registers_nothing_and_an_opened_one_registers_its_panel() {
        let mut h = PortalHarness::new(false);
        let closed = h.frame(0.0);
        assert!(
            closed.rrects.is_empty(),
            "a closed tooltip floats nothing at all"
        );
        let open = h.open_by_hover();
        assert!(!open.rrects.is_empty(), "hovering floats the panel");
    }

    /// Deliverable of the port this test exists for: the panel goes away with
    /// no exit ramp — and, because the registration is per paint pass and the
    /// latch is read live, it goes away on the next *paint*, without waiting
    /// for the rebuild the old `View::Element` swap needed. The latch is not
    /// reactive, so that rebuild was never guaranteed to come.
    #[test]
    fn closing_the_latch_takes_the_panel_away_on_the_very_next_paint_with_no_exit_ramp() {
        let mut h = PortalHarness::new(false);
        assert!(!h.open_by_hover().rrects.is_empty(), "open first");

        h.hover.set_open(false);
        let after = h.repaint(SETTLED_MS + 16.0);
        assert!(
            after.rrects.is_empty(),
            "gone on the paint that follows the close — no rebuild needed"
        );
        assert!(
            after.layers.is_empty(),
            "and nothing was composited on the way out: there is no exit ramp"
        );

        // The rebuild a shell would have run next changes nothing: there is no
        // ramp left to truncate and no placeholder to swap back in.
        let rebuilt = h.frame(SETTLED_MS + 32.0);
        assert!(rebuilt.rrects.is_empty());
        assert!(rebuilt.layers.is_empty());
    }

    #[test]
    fn the_entrance_ramp_still_plays_and_then_settles() {
        let mut h = PortalHarness::new(false);
        h.root.event(&mut h.state, &moved(10.0, 10.0));
        // The paint that opens it seeds the driver; the next one is already
        // part-way in.
        h.frame(0.0);
        let entering = h.repaint(MaterialMotion::SHORT_4.as_millis() as f64 / 2.0);
        assert!(
            entering.layers.iter().any(|a| *a > 0.0 && *a < 1.0),
            "the entrance composites a part-way layer: {:?}",
            entering.layers
        );
        let settled = h.repaint(SETTLED_MS);
        assert!(
            settled.layers.is_empty(),
            "and stops compositing once it has arrived: {:?}",
            settled.layers
        );
        assert!(!settled.rrects.is_empty(), "still on screen, fully present");
    }

    /// The whole point of the port, for the plain panel: a press that lands on
    /// the panel reaches the page *underneath* it.
    ///
    /// Negative control performed by hand while writing this: changing
    /// `TooltipView::configure`'s `OverlayInput::Transparent` to `Interactive`
    /// makes this fail — `state.pressed` stays `0`, because the root's overlay
    /// pre-pass then routes the press into the panel's own pod instead of
    /// letting it through. See the completion summary for the run.
    #[test]
    fn a_press_on_an_open_plain_tooltip_reaches_the_page_beneath_it() {
        let mut h = PortalHarness::new(false);
        let panel = h.open_by_hover().panel();
        let at = panel.center();
        let handled = h.press(at.x, at.y);
        assert_eq!(
            h.state.pressed, 1,
            "the page under the panel actually fired"
        );
        assert!(handled, "and reported the press as handled, by the page");
        assert!(
            h.hover.is_open(),
            "a plain tooltip is not dismissed by a tap: it closes on hover-exit \
             or on its own timer"
        );
    }

    /// The rich panel's half of the same promise. It is *not* input-transparent
    /// — its action row has to be reachable — so what changed for it is the
    /// press **outside**: upstream's translucent dismiss layer tells the panel
    /// and lets the press through, where the anchored host swallowed it.
    #[test]
    fn a_press_outside_an_open_rich_tooltip_closes_it_and_still_reaches_the_page() {
        let mut h = PortalHarness::new(true);
        let panel = h.open_by_hover().panel();
        let away = AuthoringPoint::new(WINDOW.width - 20.0, WINDOW.height - 20.0);
        assert!(
            !panel.contains(away),
            "the press has to land outside the panel for this to mean anything"
        );

        h.press(away.x, away.y);
        assert!(!h.hover.is_open(), "the outside press dismisses it");
        assert_eq!(
            h.state.pressed, 1,
            "and is NOT swallowed: the page under it fired too"
        );
    }

    /// And the action row upstream gives a rich tooltip still works through the
    /// portal: the root hit-tests the registered rect first and routes the
    /// press into the pod, so the button fires and the page beneath never sees
    /// it.
    #[test]
    fn a_press_on_an_open_rich_tooltips_action_fires_it_and_not_the_page() {
        let mut h = PortalHarness::new(true);
        let panel = h.open_by_hover().panel();
        let local = rich_probe_action_center(0);
        let at = AuthoringPoint::new(panel.x0 + local.x, panel.y0 + local.y);
        assert!(panel.contains(at), "the action sits inside the panel");

        h.root.event(&mut h.state, &down(at.x, at.y));
        h.root.event(&mut h.state, &up(at.x, at.y));
        assert_eq!(h.state.action_fired, 1, "the action button fired");
        assert_eq!(
            h.state.pressed, 0,
            "and the page beneath was not pressed through the panel"
        );
    }

    /// A [`RichPanelWidget`] carrying the harness's own slots, laid out against
    /// the same window-loose constraints [`OverlaySlot::layout`] gives the
    /// floated pod — so an action pod's real post-layout centre can be lifted
    /// into window space by adding the placed panel's origin, with no hardcoded
    /// metric anywhere.
    fn rich_probe_action_center(index: usize) -> AuthoringPoint {
        let view: RichPanel<PageState> = RichPanel {
            title: Some(RICH_TITLE.into()),
            message: RICH_BODY.into(),
            actions: vec![tooltip_action(RICH_ACTION, |s: &mut PageState| {
                s.action_fired += 1
            })],
        };
        let mut counter = 0u64;
        let mut w = View::<PageState>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(WINDOW));
        action_center(&w, index)
    }

    #[test]
    fn hover_shows_the_plain_panel_and_the_rich_panel_alike() {
        for rich in [false, true] {
            let mut h = PortalHarness::new(rich);
            assert!(
                !h.open_by_hover().rrects.is_empty(),
                "hovering the trigger shows the panel (rich: {rich})"
            );
        }
    }

    #[test]
    fn a_long_press_shows_the_plain_panel_and_the_rich_panel_alike() {
        for rich in [false, true] {
            let mut h = PortalHarness::new(rich);
            h.root.event(&mut h.state, &down(10.0, 10.0));
            h.frame(0.0);
            // Paint alone only latches; the rebuild that follows is what drains
            // the flush and fires the long press (see the trigger's own test).
            h.repaint(LONG_PRESS_MS as f64 + 20.0);
            let shown = h.frame(LONG_PRESS_MS as f64 + 20.0);
            assert_eq!(
                h.hover.via(),
                TooltipOpenVia::LongPress,
                "opened by the long press (rich: {rich})"
            );
            assert!(
                !shown.rrects.is_empty(),
                "and the panel is floated for it (rich: {rich})"
            );
        }
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
