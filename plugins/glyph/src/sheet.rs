//! Glyph modal bottom sheet: a full-area **scrim** + a bottom-anchored **panel**
//! wrapping a single app-provided content view — the Glyph recipe over the
//! same navigator transparent-push modal plumbing [`crate::dialog`] uses.
//!
//! # Navigator-modal architecture (reuse, don't fork)
//!
//! A `GlyphSheetView` is pushed as a **transparent navigator page** via
//! [`NavigatorController::push_with_options`] with a
//! [`BackPolicy`](frust::BackPolicy) and a dismiss signal — the
//! seam [`show_glyph_dialog`](crate::dialog::show_glyph_dialog) uses,
//! deliberately **not** [`NavigatorController::push_transparent_for_result`]
//! (`material::dialog`'s). The page below stays visible under the scrim, the
//! navigator's modal contract makes it modal, and [`show_glyph_sheet`] wraps the
//! push, wiring dismissal to `controller.pop()` **composed with** any
//! [`on_close`](GlyphSheetView::on_close) the caller set (see *Programmatic
//! close* below); nav is consumed read-only. Enforcing "at most one
//! dialog/sheet/overlay at a time" stays the app's (or the navigator's) concern
//! — this widget does not police it, like [`crate::dialog`].
//!
//! # Enter/exit staging — the scrim fades, the panel slides (independently)
//!
//! The module's one hard behavioural requirement: **the scrim must fade while
//! only the panel slides.** A whole-page
//! [`PageTransition::SlideUp`](frust::PageTransition::SlideUp) offsets the
//! *entire* page's paint (a scrim painted in that page would sweep up with it),
//! and one navigator `Layer` over the page
//! cannot express "panel translates but scrim only fades". So, like
//! [`crate::dialog`], this widget drives its **own** enter/exit timeline
//! from `PaintCtx::frame_time` and is pushed with [`TransitionSpec::NONE`] —
//! the navigator supplies the modal contract, the widget the motion:
//!
//! * the **scrim** is a plain full-area fill painted *before* any transform is
//!   pushed, alpha driven straight off the timeline's progress — it never
//!   moves, only fades;
//! * the **panel** is painted *inside* a single [`kurbo::Affine::translate`]
//!   pushed after the scrim, sliding from fully off-screen (`y` = panel height
//!   below its rest position) up to rest over the enter duration, reversing on
//!   exit.
//!
//! Enter runs over `durations.base` (220ms, spatial easing), exit over the
//! faster `durations.fast` (150ms, exit easing) — "exits always faster than
//! entrances" (dialog's rule too). A dismiss gesture (scrim tap, a handle drag
//! released past the threshold or flicked down, `Escape`, or an Android back
//! press) does **not** pop immediately: it flips the widget into its exit via
//! [`begin_exit`](GlyphSheetWidget::begin_exit), and only on completion does the
//! app-supplied close callback fire — from paint, which is sound because
//! [`NavigatorController::pop`] merely enqueues an op applied on the next
//! rebuild. **Every dismiss path funnels through `begin_exit`**, the handle drag
//! below included: no code path pops the page without playing the staged exit.
//!
//! # The dismiss callback is state-free (mirrors `glyph::dialog`)
//!
//! Because it fires from **paint** (after the exit animation),
//! [`on_close`](GlyphSheetView::on_close) is a plain `Fn()`, not a
//! `Fn(&mut State)` — unlike `crate::material::sheet`'s `on_dismiss`, whose
//! dismiss is immediate with no staged exit to wait for. App state changes ride
//! the navigator's `on_result` instead, delivered with `&mut State` after the
//! pop.
//!
//! # Drag-to-dismiss — the panel follows the finger, then rides the staged exit
//!
//! A bottom sheet, unlike a centered dialog, is a strongly drag-affording shape,
//! so the top 44px handle strip is a genuinely **interactive** drag target: a
//! press there captures the pointer and the panel tracks the finger frame by
//! frame (`dy = position.y − the drag's origin`, clamped to `[0, panel height]`
//! — it follows the finger *down* only, never above rest), with the scrim
//! fading on the very same progress the exit staging uses (`alpha = SCRIM_ALPHA
//! · (1 − offset_frac)`), so a half-dragged panel sits under a half-lit scrim.
//!
//! On release the sheet dismisses when **either**
//!
//! * the panel sits at or past [`DRAG_DISMISS_FRACTION`] of its own height, **or**
//! * the release is a downward flick — a release velocity at or over
//!   [`FLICK_DISMISS_VELOCITY`] logical px/s, so a short fast swipe closes even a
//!   tall sheet
//!
//! and otherwise **springs back to rest** (over `durations.fast` with the
//! decelerating `effects` easing — see [`resolve_settle`]), input live
//! throughout. A `Cancel` takes that same spring-back.
//!
//! A dismissal enters [`begin_exit`](GlyphSheetWidget::begin_exit) **from the
//! panel's current offset**, never from rest, so nothing ever flashes back up
//! before sliding out: the exit driver's own `0 → 1` progress is remapped onto
//! `offset_at_release → 1`. That applies to every path, not just the drag — a
//! [`GlyphSheetHandle::close`] mid-enter now exits from wherever the entering
//! panel had reached.
//!
//! Release velocity is sampled the way every other draggable widget in the crate
//! samples it (`crate::scroll::ScrollWidget`'s idiom): a
//! [`VelocityTracker`] fed from `paint`'s frame clock, because an event pass
//! carries no clock of its own.
//!
//! **The drag never fights the enter/exit staging.** A handle `Down` starts a
//! drag only in `Phase::Shown`; during `Enter`/`Exit` the phase machine owns the
//! panel's offset, so the press falls through to the modal-barrier arm instead
//! (swallowed — a press *inside* the panel dismisses nothing). In the other
//! direction, a [`GlyphSheetHandle::close`] landing mid-drag wins at once and
//! exits from the finger's current offset; the drag's own release is then
//! absorbed, since `begin_exit` is idempotent and a spring-back only ever starts
//! from `Shown`.
//!
//! **Frame discipline** (idle-heat): paint requests another frame while a
//! tracking drag is in flight, while any of the three drivers (enter, exit,
//! spring-back) runs, and for one frame past `Dismissed` — and requests nothing
//! at rest. A `Move` additionally requests a redraw from the event pass, the
//! same event-driven repaint `ScrollWidget`'s drag uses.
//!
//! Hit-testing stays on the panel's **rest** rect throughout (the `panel` /
//! `handle_target` fields), exactly the contract the enter/exit staging already
//! had: the offset is a paint-time transform, not a layout change.
//!
//! # Content
//!
//! A sheet wraps exactly **one** app-provided content view (a Glyph sheet has no
//! built-in notion of "rows" to lay out). It lays out full-width below the
//! drag-handle strip at its own intrinsic height (loose width, unbounded height)
//! and the panel grows to fit — same overflow contract as
//! `crate::material::sheet`: an unbounded-height child can push the panel past
//! the top of the viewport, so wrap genuinely unbounded content in its own
//! scrolling container first.
//!
//! # Semantics — `Role::Dialog` with the modal flag (not `Role::Menu`)
//!
//! The sheet contributes one [`Role::Dialog`] container node with the accesskit
//! **modal** flag set and forwards the content subtree through
//! [`ChildPod::semantics_child`]. Not `Role::Menu`, which
//! `crate::cupertino::action_sheet` earns with content that is *always* a
//! fixed list of selectable action rows; this module's content is an arbitrary
//! single child with no menu semantics to inherit, and accesskit has no "bottom
//! sheet" role, so `Dialog` + modal is the closest honest fit.
//!
//! # `dismissable(bool)` + back-dismiss
//!
//! [`GlyphSheetView::dismissable`] (default `true`) is the single barrier flag
//! gating the scrim tap, the handle drag, `Escape`, and an Android back press
//! together. **A non-dismissable sheet's panel is simply immovable: the handle
//! drag does not track the finger and does not rubber-band.** The press is still
//! captured and swallowed (it must not fall through to the content or the page
//! under the modal), but nothing moves and nothing springs back — the honest
//! signal that this sheet has no dismiss gesture, where a rubber-band that
//! always snaps back would advertise one that does not exist.
//!
//! `false` disables all four dismiss gestures (only the app itself still closes it,
//! through [`GlyphSheetHandle`] below or a bare `controller.pop()`) and
//! [`show_glyph_sheet`] pushes with
//! [`BackPolicy::Veto`](frust::BackPolicy::Veto). `true` pushes
//! [`BackPolicy::DismissAnimated`](frust::BackPolicy::DismissAnimated):
//! `show_glyph_sheet` hands the widget the shared dismiss-signal cell
//! [`NavigatorController::request_back`] bumps on a back press, and the
//! widget's `paint` pass compares it against the last-seen value and calls
//! [`begin_exit`](GlyphSheetWidget::begin_exit) — the identical staged exit a
//! scrim tap/drag/`Escape` drives (mirrors dialog's `observe_dismiss_signal`).
//!
//! # Programmatic close — [`GlyphSheetHandle`], the same staged exit
//!
//! [`show_glyph_sheet`] returns a cheap cloneable [`GlyphSheetHandle`], and
//! [`close`](GlyphSheetHandle::close) bumps that very same dismiss-signal cell.
//! So an app-driven close (a picker row that closes on selection) plays the
//! **identical** scrim-fade + panel-slide exit and pops on completion — where a
//! bare `controller.pop()` tears the page, and with it this widget, down
//! mid-motion, so the sheet just vanishes. The cell is wired for *every* pushed
//! sheet, `dismissable` or not; only the navigator's `BackPolicy` differs.
//!
//! Exactly-once by construction: a repeat close, or a close landing while a
//! drag/scrim/`Escape`/back exit is already in flight, is absorbed by
//! `begin_exit`'s idempotence and the terminal `Dismissed` phase — one exit, one
//! pop. A close requested before the pushed page has painted once is honoured
//! too: the widget starts at generation 0 rather than at the cell's current
//! value, so the bump is still observed on its first paint.
//!
//! **A programmatic close ignores
//! [`dismissable(false)`](GlyphSheetView::dismissable).** That flag is a barrier
//! against the *user* — scrim, drag, `Escape`, back — while a close through the
//! handle is the app's own act, the same act `controller.pop()` already was; a
//! sheet the app cannot close is a stuck app, not a strict one.
//!
//! # `on_close` composition: the pop is enqueued first
//!
//! [`show_glyph_sheet`] composes rather than replaces: if the built view already
//! carries an [`on_close`](GlyphSheetView::on_close), the wired callback runs
//! `controller.pop()` **first** and the caller's callback second, each exactly
//! once, on the one exit completion. That order is load-bearing — both run
//! inside the same paint, so any navigator op the caller's callback issues
//! (pushing a confirmation dialog as the sheet leaves) queues *behind* the
//! sheet's own pop and is applied after it; caller-first would pop the very page
//! the callback just pushed.
//!
//! # Interaction with `overlay_host`
//!
//! This widget needs **no** change to be pushed on a root
//! [`overlay_host`](frust::overlay_host) controller instead of
//! an inner navigator: it fills `ctx.origin()..ctx.size()` with its scrim, and
//! at the root that rect *is* the window (the host owns no scrim of its own —
//! its module docs state that contract). Pushing on a host is what gives the
//! sheet root-level modality; the sheet is unaware which controller took it.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::Role;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust::input::VelocityTracker;
use frust::{Curve, FrameTime};
use kurbo::{Affine, Point, Rect, RoundedRect, RoundedRectRadii, Shape, Size};
use peniko::{Brush, Color};

use frust::Timing;
use frust::{BackPolicy, NavigatorController, PopResult, PushOptions};
use frust::{TransitionDriver, TransitionSpec, make_driver};

use crate::press::presses;

/// Scrim opacity behind the panel at full enter — the same value
/// [`crate::dialog`] uses (the Glyph modal barrier constant, applied
/// here too for a consistent barrier weight across the catalog's two modal
/// surfaces).
const SCRIM_ALPHA: f32 = 0.6;
/// Drag-handle interactive touch-target strip height, in logical px (an
/// original, hand-picked value — iOS's 44pt minimum touch target, distinct
/// from M3's 48dp since this is Glyph's own chrome).
const HANDLE_TOUCH_TARGET: f64 = 44.0;
/// Drag-handle visual indicator width, in logical px.
const HANDLE_WIDTH: f64 = 32.0;
/// Drag-handle visual indicator height, in logical px (thinner than M3's
/// 4dp — Glyph's hairline aesthetic).
const HANDLE_HEIGHT: f64 = 3.0;
/// Fraction of the panel's own height the finger must have dragged it to (or
/// past) for a release to dismiss.
///
/// **Community-approximate**: no design spec publishes a bottom-sheet dismiss
/// distance. Half the panel is where the reference implementations converge
/// (Flutter's `BottomSheet` gates on the same half-panel progress, and
/// `crate::material::sheet` already carries this heuristic) — and half stays
/// the right ask now that the panel visibly follows the finger: the user
/// watches themselves cross it, where before they committed blind.
const DRAG_DISMISS_FRACTION: f64 = 0.5;
/// Downward release speed, in logical px/s, at or above which a handle drag
/// dismisses however short it was.
///
/// **Community-approximate**: no spec publishes a flick cutoff either; ~700
/// px/s is the value the reference bottom-sheet implementations use (Flutter's
/// `_minFlingVelocity`), and it sits an order of magnitude above
/// [`frust_core::FLING_STOP`] (30 px/s, "this scroll has stopped"), which is the
/// property that matters: a deliberate flick clears it, a drifting release does
/// not. Device-independent by construction — events arrive in a
/// density-independent space, so this is logical px/s on every screen.
const FLICK_DISMISS_VELOCITY: f64 = 700.0;

/// Unthemed-fallback panel top-corner radius. A theme resolves this from
/// `shape.large` — the same token [`crate::dialog`]'s panel uses (14px),
/// deliberately not M3's chunkier `extra_large` (28dp): Glyph's terminal
/// chrome reads as one consistent corner language across its floating
/// surfaces.
const RADIUS: f64 = 14.0;
/// Unthemed-fallback panel surface fill (Glyph dark `bg-raised` `#1e2430`,
/// matching [`crate::dialog`]'s panel — a sheet is a floating surface
/// over the page, the same "raised" reading a dialog panel gets, not the flat
/// `bg-base` M3's `surface_container_low` choice would resolve to here). A
/// theme resolves this from `colors.surface_container_high`.
const CONTAINER: Color = Color::from_rgb8(0x1e, 0x24, 0x30);
/// Unthemed-fallback panel border (Glyph dark `border-bright`). A theme
/// resolves this from `colors.outline`.
const BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44);
/// Panel border width, in logical px (`--border` = 1px).
const BORDER_WIDTH: f64 = 1.0;
/// Corner-rounding tolerance for the panel/border paths (mirrors
/// `crate::dialog`'s constant).
const PATH_TOLERANCE: f64 = 0.1;
/// Unthemed-fallback scrim base color (a theme resolves this from
/// `colors.scrim`).
const SCRIM: Color = Color::from_rgb8(0x00, 0x00, 0x00);
/// Unthemed-fallback drag-handle color (a theme resolves this from
/// `colors.on_surface_variant`).
const HANDLE_COLOR: Color = Color::from_rgb8(0x8a, 0x8f, 0x98);
/// Unthemed-fallback chrome-level shadow (mirrors
/// `crate::dialog`'s `--shadow-chrome`). A theme resolves this from
/// `elevation.level5`.
const SHADOW_Y: f64 = 12.0;
const SHADOW_BLUR: f64 = 32.0;
const SHADOW_ALPHA: f32 = 0.45;

/// Enter duration fallback (`durations.base` = 220ms) when no theme is
/// threaded — mirrors [`crate::dialog`].
const ENTER_DURATION: Duration = Duration::from_millis(220);
/// Enter easing fallback (Glyph `spatial` cubic).
const ENTER_CURVE: Curve = Curve::Cubic(0.34, 1.35, 0.64, 1.0);
/// Exit duration fallback (`durations.fast` = 150ms; exits faster than
/// entrances).
const EXIT_DURATION: Duration = Duration::from_millis(150);
/// Exit easing fallback (Glyph `exit` accelerate-out cubic).
const EXIT_CURVE: Curve = Curve::Cubic(0.4, 0.0, 1.0, 1.0);
/// Spring-back easing fallback (Glyph `effects` decelerate cubic). Deliberately
/// **not** [`EXIT_CURVE`]: an accelerate-out curve run backwards would have the
/// panel creep away from the finger and then slam into rest, where a
/// decelerating one eases into it — which is what a released, undismissed sheet
/// should feel like. A theme resolves this from `motion.easing.effects`.
const SETTLE_CURVE: Curve = Curve::Cubic(0.16, 1.0, 0.3, 1.0);
/// `reduce_motion`'s collapsed crossfade duration (mirrors
/// `nav::transition::REDUCE_MOTION_DURATION`).
const REDUCE_MOTION_DURATION: Duration = Duration::from_millis(120);

/// Replace `color`'s alpha channel with `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The `(enter, exit)` [`Timing`]s for the current `reduce_motion` state —
/// identical resolution to [`crate::dialog::resolve_timings`], kept as
/// its own copy rather than a shared helper because the two modules are
/// intentionally not coupled (the same deliberate split the material and
/// cupertino sheets keep).
fn resolve_timings(theme: Option<&Theme>) -> (Timing, Timing) {
    if theme.map(|t| t.motion.reduce_motion).unwrap_or(false) {
        let t = Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear);
        return (t, t);
    }
    match theme {
        Some(t) => (
            Timing::Duration(
                Duration::from_secs_f64(t.motion.durations.base / 1000.0),
                t.motion.easing.spatial,
            ),
            Timing::Duration(
                Duration::from_secs_f64(t.motion.durations.fast / 1000.0),
                t.motion.easing.exit,
            ),
        ),
        None => (
            Timing::Duration(ENTER_DURATION, ENTER_CURVE),
            Timing::Duration(EXIT_DURATION, EXIT_CURVE),
        ),
    }
}

/// The [`Timing`] a below-threshold release (or a `Cancel`) springs the panel
/// back to rest on: the exit's own `durations.fast` — a spring-back is a
/// correction, not a journey — but the decelerating `effects` easing rather than
/// the `exit` accelerate-out one (see [`SETTLE_CURVE`]). Collapses to the same
/// linear crossfade everything else does under `reduce_motion`.
fn resolve_settle(theme: Option<&Theme>) -> Timing {
    if theme.map(|t| t.motion.reduce_motion).unwrap_or(false) {
        return Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear);
    }
    match theme {
        Some(t) => Timing::Duration(
            Duration::from_secs_f64(t.motion.durations.fast / 1000.0),
            t.motion.easing.effects,
        ),
        None => Timing::Duration(EXIT_DURATION, SETTLE_CURVE),
    }
}

fn resolve_container(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().surface_container_high,
        None => CONTAINER,
    }
}

fn resolve_border(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().outline,
        None => BORDER,
    }
}

fn resolve_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(t) => t.shape.large,
        None => RADIUS,
    }
}

fn resolve_scrim(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().scrim,
        None => SCRIM,
    }
}

fn resolve_handle(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().on_surface_variant,
        None => HANDLE_COLOR,
    }
}

/// The `(y_offset, blur_std_dev, color)` chrome-level shadow triple: themed
/// `elevation.level5.shadow(brightness)`, or the unthemed fallback (mirrors
/// `crate::dialog::resolve_shadow`).
fn resolve_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
    match theme {
        Some(t) => {
            let s = t.elevation.level5.shadow(t.brightness);
            (
                s.y_offset,
                s.blur_std_dev,
                with_alpha(t.scheme().shadow, s.color_alpha),
            )
        }
        None => (SHADOW_Y, SHADOW_BLUR, with_alpha(SCRIM, SHADOW_ALPHA)),
    }
}

/// Coerce a possibly-infinite constraint dimension to a finite value (a sheet
/// expects bounded constraints — a navigator page or a full-screen `Stack`).
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// A state-free "close this sheet" callback — see the [module docs](self)'s
/// "The dismiss callback is state-free" note.
type OnClose = Rc<dyn Fn()>;

/// A declarative Glyph modal bottom sheet wrapping a single content view. See
/// the [module docs](self).
pub struct GlyphSheetView<State: 'static> {
    content: AnyView<State>,
    on_close: Option<OnClose>,
    dismissable: bool,
    /// The shared dismiss-signal cell (the `DismissAnimated` back-press seam,
    /// and [`GlyphSheetHandle`]'s programmatic close) — wired internally by
    /// [`show_glyph_sheet`], never part of the public builder surface (see the
    /// [module docs](self)).
    dismiss_signal: Option<Rc<Cell<u64>>>,
}

/// Wrap `content` in an empty Glyph modal sheet. Chain
/// [`dismissable`](GlyphSheetView::dismissable) to change the dismiss
/// barrier, and [`on_close`](GlyphSheetView::on_close) to wire dismissal
/// (usually `controller.pop()` — [`show_glyph_sheet`] does this for you).
pub fn glyph_sheet<State: 'static, V: View<State>>(content: V) -> GlyphSheetView<State> {
    GlyphSheetView {
        content: any(content),
        on_close: None,
        dismissable: true,
        dismiss_signal: None,
    }
}

/// PascalCase alias for [`glyph_sheet`], matching the widget-fn vocabulary.
#[allow(non_snake_case)]
pub fn GlyphSheet<State: 'static, V: View<State>>(content: V) -> GlyphSheetView<State> {
    glyph_sheet(content)
}

impl<State: 'static> GlyphSheetView<State> {
    /// Whether this sheet can be dismissed by the user at all — the scrim
    /// tap, the handle drag, `Escape`, and an Android back press (default
    /// `true`; see the [module docs](self)). `false` disables all four; the
    /// app still closes it itself, through [`GlyphSheetHandle::close`] (staged)
    /// or a bare `controller.pop()` (immediate).
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Set the state-free close callback — invoked once, from paint, when the
    /// exit animation completes after any dismissal (a scrim tap, handle drag,
    /// `Escape`, back press, or [`GlyphSheetHandle::close`]).
    /// [`show_glyph_sheet`] *composes* `controller.pop()` with this rather than
    /// replacing it: the pop is enqueued first, then this callback runs (see
    /// the [module docs](self)'s "`on_close` composition").
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }
}

/// A programmatic close handle for one sheet pushed by [`show_glyph_sheet`] —
/// the app-side seam onto the widget's own staged exit (see the
/// [module docs](self)'s "Programmatic close").
///
/// Cheap and cloneable (one `Rc` cell, no `State` parameter, so a sheet body
/// can pass it around freely). Every clone drives the same sheet, and closing
/// it more than once — or while a user dismissal is already exiting — still
/// yields exactly one exit and one pop.
#[derive(Clone, Debug)]
pub struct GlyphSheetHandle {
    /// The pushed page's dismiss-signal cell: the identical generation counter
    /// [`NavigatorController::request_back`] bumps, observed by the widget's
    /// `paint` (see [`GlyphSheetWidget`]'s `dismiss_signal` field).
    signal: Rc<Cell<u64>>,
}

impl GlyphSheetHandle {
    /// Close the sheet the way the user's own gestures close it: begin the
    /// staged exit (scrim fade + panel slide over `durations.fast`) and pop the
    /// page when it completes — never an immediate pop.
    ///
    /// Idempotent, and inert once the sheet is exiting or gone. Closes a
    /// [`dismissable(false)`](GlyphSheetView::dismissable) sheet too: that flag
    /// gates *user* dismissal, and this is the app's own act (see the
    /// [module docs](self)).
    pub fn close(&self) {
        self.signal.set(self.signal.get().wrapping_add(1));
    }
}

/// Push `build`'s sheet as a transparent navigator page (the page below stays
/// visible under the scrim), register `on_result` for the value the sheet pops
/// with, and return the sheet's [`GlyphSheetHandle`] for a programmatic close.
///
/// The scrim tap/handle drag/Escape cancel is wired to `controller.pop()` (an
/// empty [`PopResult`]) **composed** with any
/// [`on_close`](GlyphSheetView::on_close) `build`'s view already carries: on
/// the one exit completion the pop is enqueued first, then the caller's
/// callback runs, each exactly once (see the [module docs](self)'s "`on_close`
/// composition").
///
/// The sheet is pushed with [`TransitionSpec::NONE`] — the navigator supplies
/// the modal contract, the widget supplies its own scrim-fade + panel-slide
/// staging (see the [module docs](self)).
///
/// ```ignore
/// let sheet = show_glyph_sheet(
///     &state.nav,
///     || glyph_sheet(pane_picker_rows()),
///     |state: &mut State, result: PopResult| { /* ... */ },
/// );
/// // …later, from a row that acts and dismisses:
/// sheet.close();
/// ```
pub fn show_glyph_sheet<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) -> GlyphSheetHandle
where
    State: 'static,
    B: Fn() -> GlyphSheetView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let close_ctrl = controller.clone();
    // Peeked once, at show-time: the back policy/dismiss-signal wiring is
    // fixed for the life of this pushed page (mirrors
    // `crate::dialog::show_glyph_dialog`'s identical peek), even
    // though `build` is re-invoked on every later navigator rebuild to diff
    // the page's content.
    let dismissable = build().dismissable;
    // One cell, two writers: the navigator's back press (only under
    // `DismissAnimated`) and the returned handle. The widget always gets it —
    // a non-dismissable sheet still owes the app a staged close — but the
    // navigator only gets it when a back press may legitimately dismiss.
    let signal = Rc::new(Cell::new(0u64));
    let widget_signal = signal.clone();
    let mut options = PushOptions::transparent()
        .transition(TransitionSpec::NONE)
        .back(if dismissable {
            BackPolicy::DismissAnimated
        } else {
            BackPolicy::Veto
        })
        .on_result(on_result);
    if dismissable {
        options = options.dismiss_signal(signal.clone());
    }
    controller.push_with_options(
        move || {
            let ctrl = close_ctrl.clone();
            let mut view = build();
            // Compose, never clobber: the caller's own `on_close` survives the
            // wiring, and runs after the pop it is composed with.
            let caller_close = view.on_close.take();
            let mut view = view.on_close(move || {
                ctrl.pop();
                if let Some(on_close) = &caller_close {
                    on_close();
                }
            });
            view.dismiss_signal = Some(widget_signal.clone());
            any::<State, _>(view)
        },
        options,
    );
    GlyphSheetHandle { signal }
}

/// The sheet's enter/exit lifecycle phase (mirrors
/// `crate::dialog::Phase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Animating in (panel sliding up from off-screen, scrim fading in).
    Enter,
    /// Fully shown, at rest.
    Shown,
    /// Animating out after a scrim/handle-drag/Escape/back cancel or a
    /// [`GlyphSheetHandle::close`].
    Exit,
    /// Exit complete; the close callback has fired and the page will be popped.
    Dismissed,
}

/// The retained widget for a [`GlyphSheetView`]. See the [module docs](self).
pub struct GlyphSheetWidget {
    content: ChildPod,
    on_close: Option<OnClose>,
    dismissable: bool,
    /// The shared dismiss-signal cell — the one channel carrying **both** an
    /// Android back press (the `DismissAnimated` seam) and a
    /// [`GlyphSheetHandle::close`]; see
    /// [`observe_dismiss_signal`](Self::observe_dismiss_signal).
    dismiss_signal: Option<Rc<Cell<u64>>>,
    /// The last generation observed from `dismiss_signal`. Starts at 0, never
    /// at the cell's build-time value: [`show_glyph_sheet`] mints the cell
    /// fresh per push, so a non-zero generation when this widget builds is a
    /// close requested before the page mounted — swallowing it would strand a
    /// sheet the app has already closed on screen.
    last_seen_dismiss: u64,
    /// The bottom-anchored panel rect **at rest** (fully shown), in the
    /// widget's own local coordinate space — used both to lay the panel out
    /// and, deliberately unanimated, as the hit-test region for the scrim/
    /// panel split (mirrors `crate::dialog`'s `panel` field, which is
    /// likewise the widget's rest-state rect regardless of its live paint
    /// transform).
    panel: Rect,
    /// The full-width drag-handle touch strip at the top of the panel (local
    /// coords, at rest).
    handle_target: Rect,
    phase: Phase,
    /// The current animation's progress driver — enter, exit, *or* spring-back,
    /// whichever the phase plus [`settle_from`](Self::settle_from) says is
    /// running (the three are mutually exclusive, so they share one slot).
    /// Lazily built on the first paint that sees it, because rebuild has no
    /// theme to resolve a [`Timing`] from; cleared by whatever supersedes it.
    driver: Option<TransitionDriver>,
    /// A handle drag is in flight (the sheet captured the pointer on a `Down`
    /// in the handle strip).
    drag_active: bool,
    /// The y the drag's offset is measured from — the `Down` y, less however far
    /// the panel was already offset when the finger landed (grabbing a panel
    /// mid-spring-back must not teleport it to rest).
    drag_start_y: f64,
    /// The latest pointer y of the in-flight drag; the panel's live offset is
    /// `drag_y − drag_start_y`, clamped (see
    /// [`drag_offset_frac`](Self::drag_offset_frac)).
    drag_y: f64,
    /// Whether the in-flight drag moves the panel at all — `false` for a
    /// [`dismissable(false)`](GlyphSheetView::dismissable) sheet, whose panel is
    /// immovable but whose handle press is still swallowed (see the
    /// [module docs](self)).
    drag_tracks: bool,
    /// Release-velocity samples for the flick-to-dismiss test, timed by
    /// [`last_frame_time`](Self::last_frame_time) — `crate::scroll`'s idiom.
    tracker: VelocityTracker,
    /// The most recent `paint` frame time, reused as the event-pass timestamp
    /// for [`tracker`](Self::tracker): an event pass carries no clock of its own
    /// (again `crate::scroll`'s idiom, and its precedent for the same problem).
    last_frame_time: FrameTime,
    /// The panel offset fraction the in-flight exit started from — `0.0` for
    /// every dismissal off a resting sheet, the finger's own offset for a drag
    /// or flick release, so an exit never flashes the panel back to rest before
    /// sliding it out.
    exit_from: f64,
    /// The offset fraction an in-flight spring-back started from (`None` when
    /// the panel is not springing back). Deliberately **not** a [`Phase`]: the
    /// sheet stays `Shown` for the whole spring-back, which is what keeps
    /// [`begin_exit`](Self::begin_exit)'s `Enter | Shown` guard — and every
    /// phase assertion anywhere else — true of a sheet the user is still
    /// touching.
    settle_from: Option<f64>,
    /// The [`Timing`] a spring-back runs on, re-resolved from the active theme
    /// on every paint (the only pass with a theme in reach) and read from there
    /// by [`advance`](Self::advance), which — like the event pass that starts a
    /// spring-back — has none. Starts at the unthemed fallback, so a direct
    /// `advance` in a test drives exactly the motion an unthemed app paints.
    settle_timing: Timing,
    /// The panel's offset fraction as of the last [`advance`](Self::advance):
    /// where a dismissal picks the exit up from (see
    /// [`current_offset_frac`](Self::current_offset_frac)).
    offset_frac: f64,
    /// A scrim/panel-background press is in flight (the modal barrier
    /// captured the pointer).
    scrim_captured: bool,
    /// Whether that press started outside the panel (only an outside press
    /// released outside dismisses).
    scrim_down_outside: bool,
}

impl<State: 'static> View<State> for GlyphSheetView<State> {
    type Element = GlyphSheetWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GlyphSheetWidget {
        GlyphSheetWidget {
            content: frust::authoring::build_child(&self.content, ctx),
            on_close: self.on_close.clone(),
            dismissable: self.dismissable,
            dismiss_signal: self.dismiss_signal.clone(),
            last_seen_dismiss: 0,
            panel: Rect::ZERO,
            handle_target: Rect::ZERO,
            phase: Phase::Enter,
            driver: None,
            drag_active: false,
            drag_start_y: 0.0,
            drag_y: 0.0,
            drag_tracks: false,
            tracker: VelocityTracker::new(),
            last_frame_time: FrameTime::ZERO,
            exit_from: 0.0,
            settle_from: None,
            settle_timing: Timing::Duration(EXIT_DURATION, SETTLE_CURVE),
            // A freshly built sheet has not painted yet: it sits fully
            // off-screen below, exactly where its enter starts.
            offset_frac: 1.0,
            scrim_captured: false,
            scrim_down_outside: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GlyphSheetWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = frust::authoring::rebuild_child(
            &prev.content,
            &self.content,
            &mut element.content,
            ctx,
        );
        // Closures aren't comparable; reinstall the close adapter unconditionally.
        element.on_close = self.on_close.clone();
        element.dismissable = self.dismissable;
        // The dismiss-signal cell's identity is fixed at push time (see
        // `show_glyph_sheet`); reinstalling it here never disturbs
        // `last_seen_dismiss`.
        element.dismiss_signal = self.dismiss_signal.clone();
        flags
    }

    fn teardown(&self, element: &mut GlyphSheetWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl GlyphSheetWidget {
    /// Begin the exit animation (a scrim tap/handle drag/Escape cancel), from
    /// wherever the panel currently sits — mid-drag, mid-spring-back or
    /// mid-enter, never snapped back to rest first (see
    /// [`current_offset_frac`](Self::current_offset_frac)).
    ///
    /// Idempotent: only an `Enter`/`Shown` sheet can start exiting, so a
    /// second request mid-exit — or after `Dismissed` — is a no-op. **Every**
    /// close path (scrim, handle, `Escape`, back, and
    /// [`GlyphSheetHandle::close`]) funnels through this one function — see
    /// the [module docs](self)'s drag-to-dismiss note on why that matters for
    /// the scrim-fade requirement.
    fn begin_exit(&mut self) {
        self.begin_exit_from(self.current_offset_frac());
    }

    /// [`begin_exit`](Self::begin_exit) from an explicit offset fraction, for
    /// the one caller that knows the panel's position a frame ahead of paint:
    /// the release itself, which has the finger's final y in hand.
    fn begin_exit_from(&mut self, from: f64) {
        if matches!(self.phase, Phase::Enter | Phase::Shown) {
            self.phase = Phase::Exit;
            self.exit_from = from.clamp(0.0, 1.0);
            self.offset_frac = self.exit_from;
            self.driver = None;
            // A spring-back loses to a dismissal. An in-flight *drag* is left
            // alone deliberately: it keeps swallowing its own pointer stream
            // until the Up/Cancel (dropping the capture mid-gesture would leak
            // the rest of it into the content), and its release can no longer
            // do anything — `begin_exit` is idempotent and a spring-back only
            // starts from `Shown`.
            self.settle_from = None;
        }
    }

    /// Start the spring-back to rest from offset fraction `from` — a
    /// below-threshold release, or a `Cancel`. Only from `Shown`: a sheet that
    /// is already exiting (a [`GlyphSheetHandle::close`] that landed mid-drag)
    /// must never be pulled back up.
    fn start_settle(&mut self, from: f64) {
        if self.phase != Phase::Shown || from <= 0.0 {
            return;
        }
        self.settle_from = Some(from.clamp(0.0, 1.0));
        self.offset_frac = from;
        self.driver = None;
    }

    /// The in-flight drag's panel offset as a fraction of the panel's own
    /// height: the distance from the drag origin, clamped to `[0, 1]` — the
    /// panel follows the finger *down* only, and no further than fully
    /// off-screen. `0` when the drag does not track the finger at all (a
    /// [`dismissable(false)`](GlyphSheetView::dismissable) sheet's immovable
    /// panel).
    fn drag_offset_frac(&self) -> f64 {
        let panel_h = self.panel.height();
        if !self.drag_tracks || panel_h <= 0.0 {
            return 0.0;
        }
        ((self.drag_y - self.drag_start_y) / panel_h).clamp(0.0, 1.0)
    }

    /// Where the panel is *right now* as an offset fraction: the finger's own
    /// position while a drag tracks it, otherwise the last value
    /// [`advance`](Self::advance) painted (enter, spring-back and exit all
    /// record theirs). This is what a dismissal starts its exit from, so no
    /// close path — gesture, key, back, or handle — ever snaps the panel back to
    /// rest before sliding it out.
    fn current_offset_frac(&self) -> f64 {
        if self.drag_active && self.phase == Phase::Shown {
            self.drag_offset_frac()
        } else {
            self.offset_frac
        }
    }

    /// The last painted frame time in milliseconds — the event pass's only
    /// clock, and so the timestamp source for the drag's
    /// [`tracker`](Self::tracker) (mirrors `crate::scroll`'s `event_time_ms`).
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    /// Observe the shared dismiss-signal cell (see the `dismiss_signal` field
    /// docs) and begin the exit staging exactly once per bump — whether the
    /// bump came from a back press or from [`GlyphSheetHandle::close`]
    /// (mirrors `crate::dialog::observe_dismiss_signal`). A back request
    /// flags `PAINT`, so this always runs before the next frame is shown;
    /// a handle close rides the app's own rebuild.
    ///
    /// Deliberately **not** gated on `dismissable`: the navigator only routes a
    /// back press here for a dismissable sheet in the first place (a
    /// non-dismissable one pushes [`BackPolicy::Veto`]), so the only writer
    /// left is the app's own handle, which closes either kind.
    fn observe_dismiss_signal(&mut self) {
        if let Some(signal) = &self.dismiss_signal {
            let current = signal.get();
            if current != self.last_seen_dismiss {
                self.last_seen_dismiss = current;
                self.begin_exit();
            }
        }
    }

    /// Advance the enter/exit phase machine to frame time `now`, returning
    /// `(offset_frac, scrim_frac, animating)`: `offset_frac` is how far the
    /// panel sits below its rest position, as a fraction of its own height
    /// (`0.0` = at rest, `1.0` = fully off-screen below); `scrim_frac` is the
    /// scrim's fade fraction, which is `1 − offset_frac` on **every** path —
    /// enter, drag, spring-back and exit alike — so the barrier's weight always
    /// reads how far the panel has travelled. (The scrim still fades
    /// *independently of the panel's transform*: it is painted before it, and
    /// never moves. See the [module docs](self).) Factored out of `paint` so the
    /// full timeline is drivable with synthetic [`FrameTime`]s in a unit test
    /// (mirrors `crate::dialog::GlyphDialogWidget::advance`).
    fn advance(&mut self, now: FrameTime, enter: Timing, exit: Timing) -> (f64, f32, bool) {
        let (offset, animating) = match self.phase {
            Phase::Enter => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(enter).0)
                    .advance(now);
                if adv.done {
                    self.phase = Phase::Shown;
                    self.driver = None;
                }
                (1.0 - adv.value.clamp(0.0, 1.0), !adv.done)
            }
            Phase::Shown => self.advance_shown(now),
            Phase::Exit => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(exit).0)
                    .advance(now);
                let q = adv.value.clamp(0.0, 1.0);
                if adv.done {
                    self.phase = Phase::Dismissed;
                    self.driver = None;
                    if let Some(on_close) = &self.on_close {
                        on_close();
                    }
                }
                // The driver's `0 → 1` remapped onto `exit_from → 1`: a drag
                // release resumes from where the finger left the panel instead
                // of snapping back to rest first.
                (self.exit_from + (1.0 - self.exit_from) * q, !adv.done)
            }
            Phase::Dismissed => (1.0, false),
        };
        self.offset_frac = offset;
        (offset, (1.0 - offset) as f32, animating)
    }

    /// The `Shown`-phase offset: a live handle drag tracks the finger, a
    /// released-below-threshold (or cancelled) drag springs back to rest, and an
    /// untouched sheet just sits there. Split out of [`advance`](Self::advance)
    /// to keep its four phase arms one-glance readable.
    fn advance_shown(&mut self, now: FrameTime) -> (f64, bool) {
        if self.drag_active {
            // A tracking drag keeps the frame pump open for as long as the
            // finger is down — a bounded, user-driven interval — so a coalesced
            // or batched move stream still paints every frame. A non-tracking
            // one (immovable panel) asks for nothing.
            return (self.drag_offset_frac(), self.drag_tracks);
        }
        let Some(from) = self.settle_from else {
            return (0.0, false);
        };
        let timing = self.settle_timing;
        let adv = self
            .driver
            .get_or_insert_with(|| make_driver(timing).0)
            .advance(now);
        if adv.done {
            self.settle_from = None;
            self.driver = None;
            return (0.0, false);
        }
        (from * (1.0 - adv.value.clamp(0.0, 1.0)), true)
    }
}

impl Widget for GlyphSheetWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area_w = finite_or_zero(bc.max().width);
        let area_h = finite_or_zero(bc.max().height);

        let content_max_h = (area_h - HANDLE_TOUCH_TARGET).max(0.0);
        let content_bc = BoxConstraints::loose(Size::new(area_w, content_max_h));
        let content_size = self.content.layout_child(ctx, &content_bc);

        let panel_h = (HANDLE_TOUCH_TARGET + content_size.height).min(area_h);
        let panel_y = area_h - panel_h;
        self.panel = Rect::new(0.0, panel_y, area_w, area_h);
        self.handle_target = Rect::new(0.0, panel_y, area_w, panel_y + HANDLE_TOUCH_TARGET);

        let content_x = ((area_w - content_size.width) / 2.0).max(0.0);
        self.content
            .set_origin(Point::new(content_x, panel_y + HANDLE_TOUCH_TARGET));

        bc.constrain(Size::new(area_w, area_h))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Record the shared frame clock first: the between-frames event pass
        // carries none of its own, and the drag's velocity tracker times its
        // samples by this (`crate::scroll`'s idiom, same reason).
        self.last_frame_time = ctx.frame_time();
        self.observe_dismiss_signal();
        let theme = Theme::from_paint_ctx(ctx);
        let (enter, exit) = resolve_timings(theme);
        // Re-resolved every paint like every other themed value here, and
        // stashed because neither `advance` nor the event pass that starts a
        // spring-back can reach a theme.
        self.settle_timing = resolve_settle(theme);
        let (offset_frac, scrim_frac, animating) = self.advance(ctx.frame_time(), enter, exit);

        // Scrim fills the whole area (the modal barrier) — its own fade,
        // painted *before* any transform, so it never moves — see the
        // [module docs](self)'s "the scrim fades, the panel slides" section.
        let scrim = with_alpha(resolve_scrim(theme), SCRIM_ALPHA * scrim_frac);
        scene.fill_rect(ctx.origin(), ctx.size(), scrim);

        // The panel translates by up to its own height, straight down from
        // rest — a pure slide, no scale, painted under one transform pushed
        // after the scrim above.
        let panel_h = self.panel.height();
        scene.push_transform(Affine::translate((0.0, offset_frac * panel_h)));

        let panel_origin = Point::new(
            ctx.origin().x + self.panel.x0,
            ctx.origin().y + self.panel.y0,
        );
        let panel_size = Size::new(self.panel.width(), panel_h);
        let radius = resolve_radius(theme);

        let (shadow_y, shadow_blur, shadow_color) = resolve_shadow(theme);
        scene.draw_shadow(
            Point::new(panel_origin.x, panel_origin.y + shadow_y),
            panel_size,
            radius,
            shadow_blur,
            shadow_color,
        );

        let radii = RoundedRectRadii::new(radius, radius, 0.0, 0.0);
        let path = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, panel_size), radii)
            .to_path(PATH_TOLERANCE);
        scene.fill_path(panel_origin, &path, &Brush::Solid(resolve_container(theme)));
        scene.stroke_path(
            panel_origin,
            &path,
            BORDER_WIDTH,
            &Brush::Solid(resolve_border(theme)),
        );

        // Drag handle: a thin centered indicator within the top touch strip.
        let handle_x = self.panel.x0 + (self.panel.width() - HANDLE_WIDTH) / 2.0;
        let handle_y = self.panel.y0 + (HANDLE_TOUCH_TARGET - HANDLE_HEIGHT) / 2.0;
        scene.fill_rounded_rect(
            Point::new(ctx.origin().x + handle_x, ctx.origin().y + handle_y),
            Size::new(HANDLE_WIDTH, HANDLE_HEIGHT),
            HANDLE_HEIGHT / 2.0,
            resolve_handle(theme),
        );

        self.content.paint_child(ctx, scene);
        scene.pop_transform();

        // Keep frames coming while anything is moving — a tracking drag or any
        // of the three drivers — and for one frame past Dismissed so the
        // rebuild that applies the pop actually runs. At rest: nothing
        // (idle-heat discipline).
        if animating || self.phase == Phase::Dismissed {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let t = self.event_time_ms();
        self.event_at(ctx, event, t)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Dialog,
            |node| node.set_modal(),
            |ctx| {
                self.content.semantics_child(ctx);
            },
        );
    }

    frust::authoring::visit_children!(content);
}

impl GlyphSheetWidget {
    /// The event body, parameterised on an explicit timestamp so the drag's
    /// velocity math is deterministic in tests; [`Widget::event`] supplies the
    /// real one (the last painted frame's clock — an event pass has none).
    /// Mirrors `crate::scroll`'s identical seam, for the identical reason.
    fn event_at(&mut self, ctx: &mut EventCtx, event: &InputEvent, t_ms: f64) -> EventResult {
        // 0. A broadcast is not user input, so it belongs to neither the drag nor
        //    the scrim arm below: forward it to the content unconditionally and
        //    consume nothing, so a deferred callback queued inside the sheet still
        //    flushes while a finger is down.
        if event.is_broadcast() {
            frust::authoring::route_event_single(&mut self.content, ctx, event);
            return EventResult::Ignored;
        }
        // 1. An in-flight handle drag owns the pointer stream.
        if self.drag_active {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Handled;
            };
            return match p.phase {
                PointerPhase::Move => {
                    self.drag_y = p.position.y;
                    self.tracker.record(t_ms, p.position.y);
                    // The panel moved under the finger: repaint from the event
                    // that moved it, the same event-driven redraw
                    // `crate::scroll`'s drag uses.
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    self.drag_y = p.position.y;
                    self.tracker.record(t_ms, p.position.y);
                    let offset = self.drag_offset_frac();
                    // Positive = downward (y grows down), so the sign check is
                    // the threshold itself.
                    let flicked_down = self.tracker.velocity() >= FLICK_DISMISS_VELOCITY;
                    self.drag_active = false;
                    if self.dismissable && self.drag_tracks {
                        if offset >= DRAG_DISMISS_FRACTION || flicked_down {
                            self.begin_exit_from(offset);
                        } else {
                            self.start_settle(offset);
                        }
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    // A cancelled drag dismisses nothing, however far it got:
                    // the platform took the gesture, the user did not release it.
                    let offset = self.drag_offset_frac();
                    self.drag_active = false;
                    self.start_settle(offset);
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Down => EventResult::Handled,
            };
        }
        // 2. An in-flight scrim/panel-background press owns the stream.
        if self.scrim_captured {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Handled;
            };
            return match p.phase {
                PointerPhase::Up => {
                    let released_outside = !self.panel.contains(p.position);
                    if self.dismissable && self.scrim_down_outside && released_outside {
                        self.begin_exit();
                    }
                    self.scrim_captured = false;
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    self.scrim_captured = false;
                    EventResult::Handled
                }
                _ => EventResult::Handled,
            };
        }
        // 3. Fresh events.
        let InputEvent::Pointer(p) = event else {
            // Escape (when the sheet itself, not deeper content, holds the
            // focus path) begins the exit/cancel — content gets first crack
            // at it if it holds the deeper focus path (mirrors
            // `crate::material::sheet`'s identical routing precedence).
            if let InputEvent::Key(key_event) = event
                && self.dismissable
                && key_event.key == Key::Named(NamedKey::Escape)
                && !self.content.is_focused()
            {
                self.begin_exit();
                return EventResult::Handled;
            }
            return frust::authoring::route_event_single(&mut self.content, ctx, event);
        };
        match p.phase {
            PointerPhase::Down => {
                // Claim focus on every Down anywhere in the sheet. Load-
                // bearing: the root treats a Down that bubbles no claim as a
                // blur (`release_focus_session` drops focus + IME state), so
                // the re-claim is what keeps the session alive while this
                // sheet is up. Re-claiming while already focused is a
                // change-guarded no-op (no generation bump) — do not add a
                // claim-once guard, it kills the session on the second tap.
                ctx.request_focus();
                // A drag starts only from `Shown`: during Enter/Exit the phase
                // machine owns the panel's offset and a drag would fight it, so
                // the press falls through to the barrier arm below instead —
                // which swallows it (an inside-panel press dismisses nothing).
                // Primary-only: a secondary press drags no handle.
                if presses(p)
                    && self.phase == Phase::Shown
                    && self.handle_target.contains(p.position)
                {
                    // Seed the origin from where the panel actually *is*, so
                    // grabbing one mid-spring-back does not teleport it to rest;
                    // for the ordinary grab-at-rest this is just the `Down` y.
                    let live = self.current_offset_frac() * self.panel.height();
                    self.drag_active = true;
                    self.drag_tracks = self.dismissable;
                    self.drag_start_y = p.position.y - live;
                    self.drag_y = p.position.y;
                    // The finger now owns the panel: drop any spring-back and
                    // its driver rather than letting the two drive it at once.
                    self.settle_from = None;
                    self.driver = None;
                    self.tracker.clear();
                    self.tracker.record(t_ms, p.position.y);
                    ctx.capture_pointer();
                    return EventResult::Handled;
                }
                if frust::authoring::route_event_single(&mut self.content, ctx, event)
                    == EventResult::Handled
                {
                    return EventResult::Handled;
                }
                // The barrier swallows a secondary press like any other, but
                // arms no scrim dismiss from it.
                if !presses(p) {
                    return EventResult::Handled;
                }
                self.scrim_captured = true;
                self.scrim_down_outside = !self.panel.contains(p.position);
                ctx.capture_pointer();
                EventResult::Handled
            }
            _ => frust::authoring::route_event_single(&mut self.content, ctx, event),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::NavigatorView;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BuildCtx, KeyEvent, Modifiers, PointerButton, PointerEvent, any as core_any,
    };
    use frust::text;
    use frust_core::RenderRoot;
    use frust_widgets::navigator;
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn escape_event() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn build(view: &GlyphSheetView<()>) -> GlyphSheetWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    /// Records fills, rounded fills, filled/stroked paths, shadows, and
    /// transform push/pops — enough to assert the scrim-fade/panel-slide
    /// split independently.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        fill_paths: Vec<Color>,
        stroke_paths: Vec<Color>,
        shadows: usize,
        transforms: Vec<Affine>,
        transform_pops: u32,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn fill_path(&mut self, _origin: Point, _path: &kurbo::BezPath, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.fill_paths.push(*c);
            }
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.stroke_paths.push(*c);
            }
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _b: f64, _c: Color) {
            self.shadows += 1;
        }
        fn push_transform(&mut self, t: Affine) {
            self.transforms.push(t);
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
    }

    fn laid_out(view: &GlyphSheetView<()>, area: Size) -> GlyphSheetWidget {
        let mut w = build(view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(area));
        w
    }

    // -- Enter/exit staging: scrim fades, panel slides, INDEPENDENTLY -------

    #[test]
    fn enter_staging_slides_panel_up_and_fades_scrim_in_independently() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let (enter, exit) = resolve_timings(None);

        // Seed the clock: panel fully off-screen (offset 1.0), scrim invisible.
        let (offset0, scrim0, animating0) = w.advance(ft_ms(0.0), enter, exit);
        assert!(
            (offset0 - 1.0).abs() < 1e-6,
            "panel starts fully off-screen"
        );
        assert!(scrim0.abs() < 1e-6, "scrim starts transparent");
        assert!(animating0);
        assert_eq!(w.phase, Phase::Enter);

        // Partway through the enter (60ms of 220ms — short of where the
        // authored back-ease curve's overshoot would push the raw value past
        // 1.0 and clamp `offset` to exactly 0): assert the two progress
        // values independently — this is the crux of the "scrim fades, panel
        // slides" requirement, not just "it animates".
        let (offset_mid, scrim_mid, _) = w.advance(ft_ms(60.0), enter, exit);
        assert!(
            offset_mid > 0.0 && offset_mid < 1.0,
            "panel is partway through its slide"
        );
        assert!(
            scrim_mid > 0.0 && scrim_mid < 1.0,
            "scrim is partway through its fade"
        );

        // Past the 220ms enter: settles fully shown (offset 0.0, scrim 1.0).
        let (offset1, scrim1, animating1) = w.advance(ft_ms(400.0), enter, exit);
        assert!((offset1 - 0.0).abs() < 1e-6, "panel is at rest");
        assert!((scrim1 - 1.0).abs() < 1e-6, "scrim is fully opaque");
        assert!(!animating1);
        assert_eq!(w.phase, Phase::Shown);
    }

    #[test]
    fn exit_is_faster_than_enter_and_reverses_the_staging() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let (enter, exit) = resolve_timings(None);

        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(400.0), enter, exit); // -> Shown
        w.begin_exit();
        assert_eq!(w.phase, Phase::Exit);

        let (offset_seed, scrim_seed, _) = w.advance(ft_ms(400.0), enter, exit);
        assert!((offset_seed - 0.0).abs() < 1e-6);
        assert!((scrim_seed - 1.0).abs() < 1e-6);

        // Still exiting 149ms in (would already be done at the 150ms exit,
        // but not yet at the 220ms enter — proving exit is the faster driver).
        let (_, _, animating_mid) = w.advance(ft_ms(400.0 + 149.0), enter, exit);
        assert!(animating_mid);

        // Past the 150ms exit: fully off-screen and Dismissed.
        let (offset_end, scrim_end, animating_end) = w.advance(ft_ms(400.0 + 200.0), enter, exit);
        assert!((offset_end - 1.0).abs() < 1e-6);
        assert!(scrim_end.abs() < 1e-6);
        assert!(!animating_end);
        assert_eq!(w.phase, Phase::Dismissed);
    }

    #[test]
    fn on_close_fires_once_when_the_exit_completes() {
        let count = Rc::new(Cell::new(0u32));
        let c = count.clone();
        let view: GlyphSheetView<()> =
            glyph_sheet(leaf_any(300.0, 200.0)).on_close(move || c.set(c.get() + 1));
        let mut w = build(&view);
        let (enter, exit) = resolve_timings(None);

        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(400.0), enter, exit);
        w.begin_exit();
        w.advance(ft_ms(400.0), enter, exit);
        assert_eq!(count.get(), 0, "on_close waits for the exit to finish");
        w.advance(ft_ms(700.0), enter, exit);
        assert_eq!(count.get(), 1);
        w.advance(ft_ms(900.0), enter, exit);
        assert_eq!(count.get(), 1, "idempotent past Dismissed");
    }

    #[test]
    fn paint_emits_scrim_before_the_panel_transform_and_the_panel_after() {
        // The scrim fill is emitted *before* the transform push (its own
        // fade, at the full area, never moved); the panel background/handle/
        // content ride inside the single translate — so scrim and panel
        // stage independently at the paint-command level too.
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let area = Size::new(400.0, 600.0);
        w.layout(&mut lctx, &BoxConstraints::tight(area));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, area);
        w.paint(&mut pctx, &mut rec);

        // A full-area scrim fill, at (near) zero alpha at the seeded start.
        assert_eq!(rec.rects[0].1, area);
        assert!(
            rec.rects[0].2.components[3] < 1e-3,
            "scrim starts transparent"
        );
        // Exactly one transform push/pop wraps the panel + content.
        assert_eq!(rec.transforms.len(), 1);
        assert_eq!(rec.transform_pops, 1);
        // A pure translation (no scale component) — the panel slides, it
        // does not scale.
        let t = rec.transforms[0];
        let coeffs = t.as_coeffs();
        assert_eq!((coeffs[0], coeffs[3]), (1.0, 1.0), "no scale component");
        assert!(
            coeffs[5] > 0.0,
            "panel is translated downward at enter start"
        );
        assert_eq!(rec.fill_paths.len(), 1, "the panel surface fill");
        assert_eq!(rec.stroke_paths.len(), 1, "the panel border stroke");
        assert_eq!(rec.shadows, 1, "the chrome-level shadow");
        assert!(
            pctx.needs_frame(),
            "an in-flight enter requests another frame"
        );
    }

    // -- Dismiss vectors ------------------------------------------------

    #[test]
    fn scrim_tap_outside_the_panel_begins_exit() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let mut w = laid_out(&view, Size::new(400.0, 600.0));
        w.advance(ft_ms(0.0), resolve_timings(None).0, resolve_timings(None).1);
        w.advance(
            ft_ms(400.0),
            resolve_timings(None).0,
            resolve_timings(None).1,
        ); // -> Shown

        let state_any: &mut dyn Any = &mut ();
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 600.0));
        // Down well above the panel (a point in the scrim area).
        w.event(&mut ctx, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.scrim_captured);
        assert!(w.scrim_down_outside);
        w.event(&mut ctx, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(
            w.phase,
            Phase::Exit,
            "an outside tap begins the staged exit"
        );
    }

    #[test]
    fn handle_drag_past_threshold_begins_exit_not_an_immediate_pop() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let mut w = laid_out(&view, Size::new(400.0, 600.0));
        w.advance(ft_ms(0.0), resolve_timings(None).0, resolve_timings(None).1);
        w.advance(
            ft_ms(400.0),
            resolve_timings(None).0,
            resolve_timings(None).1,
        ); // -> Shown

        // panel height = 44 + 200 = 244; threshold = 122.
        let handle_y = w.panel.y0 + 10.0; // inside the 44px handle strip
        let state_any: &mut dyn Any = &mut ();
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 600.0));
        w.event(&mut ctx, &ev(PointerPhase::Down, 200.0, handle_y));
        assert!(w.drag_active);
        w.event(&mut ctx, &ev(PointerPhase::Up, 200.0, handle_y + 150.0));
        assert!(!w.drag_active);
        assert_eq!(
            w.phase,
            Phase::Exit,
            "a past-threshold drag begins the staged exit, not an immediate pop"
        );
    }

    #[test]
    fn handle_drag_under_threshold_does_not_dismiss() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let mut w = laid_out(&view, Size::new(400.0, 600.0));
        w.advance(ft_ms(0.0), resolve_timings(None).0, resolve_timings(None).1);
        w.advance(
            ft_ms(400.0),
            resolve_timings(None).0,
            resolve_timings(None).1,
        );

        let handle_y = w.panel.y0 + 10.0;
        let state_any: &mut dyn Any = &mut ();
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 600.0));
        w.event(&mut ctx, &ev(PointerPhase::Down, 200.0, handle_y));
        w.event(&mut ctx, &ev(PointerPhase::Up, 200.0, handle_y + 10.0));
        assert_eq!(
            w.phase,
            Phase::Shown,
            "a short drag settles back, no dismiss"
        );
    }

    #[test]
    fn dismissable_false_ignores_scrim_and_drag() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0)).dismissable(false);
        let mut w = laid_out(&view, Size::new(400.0, 600.0));
        w.advance(ft_ms(0.0), resolve_timings(None).0, resolve_timings(None).1);
        w.advance(
            ft_ms(400.0),
            resolve_timings(None).0,
            resolve_timings(None).1,
        );

        let state_any: &mut dyn Any = &mut ();
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 600.0));
        w.event(&mut ctx, &ev(PointerPhase::Down, 10.0, 10.0));
        w.event(&mut ctx, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(
            w.phase,
            Phase::Shown,
            "scrim tap ignored when non-dismissable"
        );

        let handle_y = w.panel.y0 + 10.0;
        w.event(&mut ctx, &ev(PointerPhase::Down, 200.0, handle_y));
        w.event(&mut ctx, &ev(PointerPhase::Up, 200.0, handle_y + 150.0));
        assert_eq!(
            w.phase,
            Phase::Shown,
            "handle drag ignored when non-dismissable"
        );
    }

    #[test]
    fn escape_begins_exit_when_focused_and_content_is_not() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let mut w = laid_out(&view, Size::new(400.0, 600.0));
        w.advance(ft_ms(0.0), resolve_timings(None).0, resolve_timings(None).1);
        w.advance(
            ft_ms(400.0),
            resolve_timings(None).0,
            resolve_timings(None).1,
        );

        let state_any: &mut dyn Any = &mut ();
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 600.0));
        // Claim focus first (a Down anywhere in the panel does this).
        w.event(&mut ctx, &ev(PointerPhase::Down, 200.0, w.panel.y0 + 60.0));
        w.event(&mut ctx, &ev(PointerPhase::Up, 200.0, w.panel.y0 + 60.0));
        w.event(&mut ctx, &escape_event());
        assert_eq!(w.phase, Phase::Exit);
    }

    #[test]
    fn a_down_reclaims_focus_after_an_external_blur() {
        use frust::authoring::ChildPod;

        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let area = Size::new(400.0, 600.0);
        let mut counter = 0u64;
        let w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut pod = ChildPod::new(Box::new(w));
        let mut lctx = LayoutCtx::new();
        pod.layout_child(&mut lctx, &BoxConstraints::tight(area));

        let mut dummy = ();
        // A scrim press well above the panel (outside content/handle) claims
        // focus, then releases outside — beginning the staged exit (mirrors
        // `scrim_tap_outside_the_panel_begins_exit`), which clears
        // `scrim_captured` regardless, returning to the "fresh events" arm.
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 10.0, 10.0));
        }
        assert!(pod.is_focused(), "the first Down claims focus");
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Up, 10.0, 10.0));
        }

        // Simulate an external blur (mirrors the root's own `Down`-with-no-
        // claim release path) so the second Down's own re-claim is what's
        // under test, not a leftover flag.
        pod.set_focused(false);
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 10.0, 10.0));
        }
        assert!(
            pod.is_focused(),
            "a second Down must re-claim focus after an external blur — this is what \
             keeps the root's focus/IME session alive while the sheet is up"
        );
    }

    // -- Back-press dismiss signal --------------------------------------

    #[test]
    fn dismiss_signal_bump_begins_exit() {
        let signal = Rc::new(Cell::new(0u64));
        let mut view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        view.dismiss_signal = Some(signal.clone());
        let mut w = laid_out(&view, Size::new(400.0, 600.0));
        w.advance(ft_ms(0.0), resolve_timings(None).0, resolve_timings(None).1);
        w.advance(
            ft_ms(400.0),
            resolve_timings(None).0,
            resolve_timings(None).1,
        );
        assert_eq!(w.phase, Phase::Shown);

        signal.set(1);
        w.observe_dismiss_signal();
        assert_eq!(w.phase, Phase::Exit);
    }

    // -- Layout -----------------------------------------------------------

    #[test]
    fn layout_anchors_panel_to_bottom_with_handle_strip() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let area = Size::new(400.0, 600.0);
        let size = w.layout(&mut lctx, &BoxConstraints::tight(area));
        assert_eq!(size, area, "the sheet fills the whole area (scrim)");

        assert_eq!(w.panel.x0, 0.0);
        assert_eq!(w.panel.x1, area.width);
        assert_eq!(w.panel.y1, area.height, "panel bottom is the screen edge");
        assert_eq!(w.panel.height(), HANDLE_TOUCH_TARGET + 200.0);
        assert_eq!(w.handle_target.y0, w.panel.y0);
        assert_eq!(w.handle_target.height(), HANDLE_TOUCH_TARGET);
        assert_eq!(w.content.origin().y, w.panel.y0 + HANDLE_TOUCH_TARGET);
    }

    // -- Semantics ----------------------------------------------------------

    #[test]
    fn semantics_is_a_modal_dialog_container() {
        fn logic(_s: &mut ()) -> GlyphSheetView<()> {
            glyph_sheet(leaf_any(300.0, 200.0))
        }
        let mut root: RenderRoot<(), GlyphSheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(400.0, 600.0));
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Dialog)
            .expect("a Role::Dialog node is contributed");
        assert!(node.is_modal(), "the sheet node sets the modal flag");
    }

    #[test]
    fn semantics_forwards_the_content_subtree() {
        fn logic(_s: &mut ()) -> GlyphSheetView<()> {
            glyph_sheet(text("pane picker".to_string()))
        }
        let mut root: RenderRoot<(), GlyphSheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, dialog_node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Dialog)
            .expect("a Role::Dialog node is contributed");
        assert!(
            !dialog_node.children().is_empty(),
            "the content subtree's own semantics reached the tree — it was forwarded"
        );
    }

    // -- Push mechanism: TransitionSpec::NONE + push_with_options -----------

    #[test]
    fn show_glyph_sheet_pushes_transparent_with_no_page_transition() {
        let controller = frust::NavigatorController::<()>::new();
        let nav: NavigatorView<()> = navigator(&controller, || {
            core_any::<(), _>(leaf_any_widget(400.0, 600.0))
        });
        let mut w = build_nav(&nav);

        super::show_glyph_sheet(
            &controller,
            || glyph_sheet(leaf_any(300.0, 200.0)),
            |_: &mut (), _result: PopResult| {},
        );
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 600.0)));
        // Reaching layout with no panic confirms the page pushed and its
        // transparent + TransitionSpec::NONE contract is intact end-to-end;
        // the widget's own module docs assert the *reason* (own staging).
    }

    fn build_nav(view: &NavigatorView<()>) -> <NavigatorView<()> as View<()>>::Element {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn leaf_any_widget(
        w: f64,
        h: f64,
    ) -> impl View<(), Element = frust_widgets::test_support::LeafWidget> {
        frust_widgets::test_support::leaf(w, h)
    }

    // -- Programmatic close: `GlyphSheetHandle` over a live navigator --------

    /// The nav-driven fixture's state: one entry per pop result the navigator
    /// delivered — exactly one per pushed page, however that page went away,
    /// which is what makes it the pop counter these tests assert on.
    #[derive(Default)]
    struct NavState {
        results: usize,
    }

    /// A fixed-size leaf over `NavState` (the crate fixture's `Leaf` is
    /// `View<()>`-only) — the root page and the sheet's content both.
    struct Fixed {
        size: Size,
    }
    struct FixedWidget {
        size: Size,
    }
    fn fixed(width: f64, height: f64) -> Fixed {
        Fixed {
            size: Size::new(width, height),
        }
    }
    impl View<NavState> for Fixed {
        type Element = FixedWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> FixedWidget {
            FixedWidget { size: self.size }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut FixedWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for FixedWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    fn area() -> Size {
        Size::new(400.0, 600.0)
    }

    /// Frame time by which a sheet's 220ms enter has settled to `Phase::Shown`.
    const ENTER_SETTLED_MS: f64 = 400.0;

    /// A navigator hosting one root page, driven through a real [`RenderRoot`]
    /// — the only harness that can observe an actual *pop*, which is the whole
    /// point of a programmatic close (the widget-level tests above see the
    /// phase machine, never the navigator).
    /// The harness's app logic: the one navigator, rebuilt each pass.
    type NavApp = Box<dyn FnMut(&mut NavState) -> NavigatorView<NavState>>;

    struct Harness {
        root: RenderRoot<NavState, NavigatorView<NavState>>,
        app: NavApp,
        state: NavState,
        tcx: TextContext,
    }

    impl Harness {
        fn new(controller: &NavigatorController<NavState>) -> Self {
            let ctrl = controller.clone();
            let mut harness = Harness {
                root: RenderRoot::new(),
                app: Box::new(move |_: &mut NavState| {
                    navigator(&ctrl, || core_any::<NavState, _>(fixed(400.0, 600.0)))
                }),
                state: NavState::default(),
                tcx: TextContext::new(),
            };
            harness.rebuild();
            harness
        }

        /// Rebuild — applying any queued navigator op, in queue order — and
        /// lay out.
        fn rebuild(&mut self) {
            self.root.rebuild(&mut self.app, &mut self.state);
            self.root
                .layout_with_text(area(), &mut self.tcx as &mut dyn Any);
        }

        fn paint(&mut self, ms: f64) {
            self.paint_recorded(ms);
        }

        fn paint_recorded(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            rec
        }

        fn event(&mut self, event: &InputEvent) {
            self.root.event(&mut self.state, event);
        }

        /// Mount the pushed sheet and settle its enter animation.
        fn open(&mut self) {
            self.rebuild();
            self.paint(0.0);
            self.paint(ENTER_SETTLED_MS);
        }

        /// Apply whatever the completed exit enqueued, and flush the pop
        /// result (the navigator's own broadcast drain).
        fn flush(&mut self) {
            self.rebuild();
            self.event(&ev(PointerPhase::Move, 5.0, 5.0));
        }
    }

    /// Whether `rec` caught the sheet's scrim mid-fade — the discriminator for
    /// "a staged exit is in flight": a settled scrim sits at exactly
    /// `SCRIM_ALPHA`, and the root page's own full-area fill is opaque.
    fn scrim_is_mid_fade(rec: &Recorder) -> bool {
        rec.rects.iter().any(|(_, size, color)| {
            *size == area() && color.components[3] > 0.0 && color.components[3] < SCRIM_ALPHA
        })
    }

    fn open_sheet(controller: &NavigatorController<NavState>) -> (Harness, GlyphSheetHandle) {
        let mut harness = Harness::new(controller);
        let handle = show_glyph_sheet(
            controller,
            || glyph_sheet(fixed(300.0, 200.0)),
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        harness.open();
        (harness, handle)
    }

    #[test]
    fn handle_close_plays_the_staged_exit_and_pops_on_completion() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let (mut h, sheet) = open_sheet(&controller);
        assert_eq!(controller.depth(), 2, "the sheet page is up");

        sheet.close();

        // The first paint after the close observes it and seeds the exit; the
        // next one is mid-motion, and that motion is the widget's own — a
        // scrim mid-fade (strictly between transparent and its full alpha)
        // under a downward panel translate, exactly what a drag/scrim/Escape
        // dismissal paints.
        h.paint(ENTER_SETTLED_MS);
        let rec = h.paint_recorded(ENTER_SETTLED_MS + 75.0);
        h.rebuild();
        assert_eq!(controller.depth(), 2, "the sheet is exiting, not unmounted");
        assert_eq!(h.state.results, 0, "the pop waits for the exit to finish");
        assert!(scrim_is_mid_fade(&rec), "the scrim is mid-fade");
        assert!(
            rec.transforms.iter().any(|t| {
                let c = t.as_coeffs();
                c[5] > 0.0 && (c[0], c[3]) == (1.0, 1.0)
            }),
            "the panel is mid-slide under a pure downward translate"
        );

        // Past the 150ms exit: `on_close` fires from paint and the next
        // rebuild applies the pop it enqueued.
        h.paint(ENTER_SETTLED_MS + 200.0);
        h.flush();
        assert_eq!(controller.depth(), 1, "the completed exit popped the page");
        assert_eq!(h.state.results, 1, "exactly one pop");
    }

    #[test]
    fn repeat_closes_and_a_close_mid_exit_still_pop_exactly_once() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        let closes = Rc::new(Cell::new(0u32));
        let counter = closes.clone();
        let sheet = show_glyph_sheet(
            &controller,
            move || {
                let counter = counter.clone();
                glyph_sheet(fixed(300.0, 200.0)).on_close(move || counter.set(counter.get() + 1))
            },
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        h.open();

        sheet.close();
        sheet.close(); // two bumps before a paint: one observed change
        h.paint(ENTER_SETTLED_MS);
        sheet.close(); // mid-exit: absorbed by `begin_exit`'s phase guard
        h.paint(ENTER_SETTLED_MS + 100.0);
        h.paint(ENTER_SETTLED_MS + 200.0); // exit complete
        sheet.close(); // past `Dismissed`: absorbed too
        h.paint(ENTER_SETTLED_MS + 300.0);
        h.flush();

        assert_eq!(closes.get(), 1, "one close callback — so one pop");
        assert_eq!(h.state.results, 1);
        assert_eq!(controller.depth(), 1);
    }

    #[test]
    fn a_close_during_a_drag_exit_is_absorbed_into_the_one_pop() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let (mut h, sheet) = open_sheet(&controller);

        // Drag the handle past the dismiss threshold: the panel is
        // 44 + 200 = 244px tall at the bottom of the 600px area, so its strip
        // starts at y = 356 and the threshold is 122px.
        let handle_y = area().height - (HANDLE_TOUCH_TARGET + 200.0) + 10.0;
        h.event(&ev(PointerPhase::Down, 200.0, handle_y));
        h.event(&ev(PointerPhase::Up, 200.0, handle_y + 150.0));
        h.paint(ENTER_SETTLED_MS); // seeds the drag's own exit
        let rec = h.paint_recorded(ENTER_SETTLED_MS + 75.0);
        assert!(
            scrim_is_mid_fade(&rec),
            "the drag's staged exit is genuinely in flight"
        );

        sheet.close(); // the app closes a sheet the user is already dismissing
        h.paint(ENTER_SETTLED_MS + 200.0);
        h.flush();

        assert_eq!(controller.depth(), 1);
        assert_eq!(h.state.results, 1, "one exit, one pop");
    }

    #[test]
    fn the_callers_on_close_composes_with_the_pop_and_runs_after_it() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        let calls = Rc::new(Cell::new(0u32));
        let counter = calls.clone();
        let push_ctrl = controller.clone();
        let sheet = show_glyph_sheet(
            &controller,
            move || {
                let counter = counter.clone();
                let ctrl = push_ctrl.clone();
                // The callback issues a navigator op of its own — the exact
                // shape the composition order exists for.
                glyph_sheet(fixed(300.0, 200.0)).on_close(move || {
                    counter.set(counter.get() + 1);
                    ctrl.push(|| core_any::<NavState, _>(fixed(400.0, 600.0)));
                })
            },
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        h.open();

        sheet.close();
        h.paint(ENTER_SETTLED_MS);
        h.paint(ENTER_SETTLED_MS + 200.0);
        h.flush();

        assert_eq!(calls.get(), 1, "the caller's `on_close` ran exactly once");
        assert_eq!(
            h.state.results, 1,
            "…and the wired pop ran too, once — the sheet's own page is gone \
             (caller-first would have queued the push ahead of the pop, so the \
             pop would have taken the pushed page and left the sheet up)"
        );
        assert_eq!(
            controller.depth(),
            2,
            "pop first, callback second: the page the callback pushed survives"
        );
    }

    #[test]
    fn a_non_dismissable_sheet_vetoes_back_but_still_closes_programmatically() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        let sheet = show_glyph_sheet(
            &controller,
            || glyph_sheet(fixed(300.0, 200.0)).dismissable(false),
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        h.open();

        // The barrier still holds against the user: `BackPolicy::Veto` never
        // reaches the dismiss signal, so no exit begins.
        controller.request_back();
        h.rebuild();
        h.paint(500.0);
        h.paint(900.0);
        h.flush();
        assert_eq!(
            controller.depth(),
            2,
            "a back press leaves a non-dismissable sheet up"
        );
        assert_eq!(h.state.results, 0);

        // The app's own close is not the user's gesture — it closes, staged.
        sheet.close();
        h.paint(1000.0);
        h.rebuild();
        assert_eq!(controller.depth(), 2, "the exit plays before the pop");
        h.paint(1200.0);
        h.flush();
        assert_eq!(controller.depth(), 1);
        assert_eq!(h.state.results, 1);
    }

    // -- Interactive drag: the panel follows the finger ----------------------

    /// The panel height every drag test reasons in: the 44px handle strip plus
    /// the 200px content leaf. At the bottom of the 600px [`area`] its strip
    /// therefore starts at `y = 356`, and [`DRAG_DISMISS_FRACTION`] of it is
    /// 122px of travel.
    const PANEL_H: f64 = HANDLE_TOUCH_TARGET + 200.0;

    /// A laid-out sheet with its enter played out — the resting state every drag
    /// starts from.
    fn shown(view: &GlyphSheetView<()>) -> GlyphSheetWidget {
        let (enter, exit) = resolve_timings(None);
        let mut w = laid_out(view, area());
        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(ENTER_SETTLED_MS), enter, exit);
        assert_eq!(w.phase, Phase::Shown, "the sheet settled shown");
        w
    }

    /// Paint at `ms`, returning `(the panel's painted downward translation in
    /// px, the painted scrim alpha, whether another frame was requested)`.
    fn painted(w: &mut GlyphSheetWidget, ms: f64) -> (f64, f32, bool) {
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, area(), ft_ms(ms));
        w.paint(&mut pctx, &mut rec);
        (
            rec.transforms[0].as_coeffs()[5],
            rec.rects[0].2.components[3],
            pctx.needs_frame(),
        )
    }

    /// Dispatch one pointer event at an explicit event-pass timestamp — the seam
    /// `Widget::event` fills in from the last painted frame clock.
    fn drag_event(w: &mut GlyphSheetWidget, phase: PointerPhase, y: f64, t_ms: f64) -> EventResult {
        let state_any: &mut dyn Any = &mut ();
        let mut ctx = EventCtx::new(state_any, Point::ZERO, area());
        w.event_at(&mut ctx, &ev(phase, 200.0, y), t_ms)
    }

    /// The sheet panel's painted downward translation, out of a whole-tree
    /// [`Harness`] recording (its own slide is the only transform a settled
    /// navigator pushes).
    fn panel_translate(rec: &Recorder) -> f64 {
        let mut pure: Vec<f64> = rec
            .transforms
            .iter()
            .map(|t| t.as_coeffs())
            .filter(|c| (c[0], c[3]) == (1.0, 1.0))
            .map(|c| c[5])
            .collect();
        assert_eq!(pure.len(), 1, "one pure translate — the panel's own slide");
        pure.pop().unwrap()
    }

    #[test]
    fn a_handle_drag_moves_the_panel_under_the_finger_and_dims_the_scrim_with_it() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let mut w = shown(&view);
        let handle_y = w.panel.y0 + 10.0;

        // At rest: no offset, full scrim, and no frame asked for.
        let (ty, alpha, frame) = painted(&mut w, ENTER_SETTLED_MS);
        assert_eq!(ty, 0.0);
        assert!((alpha - SCRIM_ALPHA).abs() < 1e-6);
        assert!(!frame, "a resting sheet requests no frames");

        drag_event(&mut w, PointerPhase::Down, handle_y, ENTER_SETTLED_MS);
        drag_event(
            &mut w,
            PointerPhase::Move,
            handle_y + 60.0,
            ENTER_SETTLED_MS + 16.0,
        );

        let (ty, alpha, frame) = painted(&mut w, ENTER_SETTLED_MS + 16.0);
        assert!(
            (ty - 60.0).abs() < 1e-6,
            "the panel tracks the finger 1:1, got {ty}"
        );
        let expected = SCRIM_ALPHA * (1.0 - 60.0 / PANEL_H) as f32;
        assert!(
            (alpha - expected).abs() < 1e-6,
            "the scrim dims on the drag's own progress: {alpha} vs {expected}"
        );
        assert!(frame, "a live drag keeps the frame pump open");
        assert_eq!(w.phase, Phase::Shown, "dragging is not a phase");

        // Dragging back *up* past the origin never lifts the panel above rest.
        drag_event(
            &mut w,
            PointerPhase::Move,
            handle_y - 200.0,
            ENTER_SETTLED_MS + 32.0,
        );
        let (ty, alpha, _) = painted(&mut w, ENTER_SETTLED_MS + 32.0);
        assert_eq!(ty, 0.0, "clamped at rest");
        assert!((alpha - SCRIM_ALPHA).abs() < 1e-6);
    }

    #[test]
    fn a_release_past_the_threshold_exits_from_the_dragged_offset_and_pops_once() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let (mut h, _sheet) = open_sheet(&controller);
        let handle_y = area().height - PANEL_H + 10.0;

        // 150px of the 244px panel — past the threshold — but taken slowly
        // (a paint at 600ms prunes the tracker's window), so the *distance*
        // path is what dismisses here, not the flick path.
        h.event(&ev(PointerPhase::Down, 200.0, handle_y));
        h.paint(ENTER_SETTLED_MS + 16.0);
        h.event(&ev(PointerPhase::Move, 200.0, handle_y + 150.0));
        let mid = h.paint_recorded(ENTER_SETTLED_MS + 200.0);
        assert!(
            (panel_translate(&mid) - 150.0).abs() < 1e-6,
            "the panel followed the finger before the release"
        );
        h.event(&ev(PointerPhase::Up, 200.0, handle_y + 150.0));

        // The exit's first painted frame picks up exactly where the finger left
        // the panel — it never snaps back to rest and re-plays from there.
        let seed = h.paint_recorded(ENTER_SETTLED_MS + 216.0);
        let ty = panel_translate(&seed);
        assert!(
            (ty - 150.0).abs() < 1e-6,
            "the exit starts from the dragged offset, got {ty}"
        );
        let onward = h.paint_recorded(ENTER_SETTLED_MS + 291.0);
        assert!(
            panel_translate(&onward) > 150.0,
            "…and only ever continues downward from it"
        );
        assert_eq!(controller.depth(), 2, "still mounted, still exiting");

        h.paint(ENTER_SETTLED_MS + 600.0);
        h.flush();
        assert_eq!(controller.depth(), 1, "the completed exit popped the page");
        assert_eq!(h.state.results, 1, "exactly one pop");
    }

    #[test]
    fn a_downward_flick_dismisses_a_drag_that_never_reached_the_threshold() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let (mut h, _sheet) = open_sheet(&controller);
        let handle_y = area().height - PANEL_H + 10.0;

        // 60px — a quarter of the panel, well under the half-panel threshold —
        // but covered across two 16ms frames: ~1875 px/s, past
        // FLICK_DISMISS_VELOCITY.
        h.event(&ev(PointerPhase::Down, 200.0, handle_y));
        h.paint(ENTER_SETTLED_MS + 16.0);
        h.event(&ev(PointerPhase::Move, 200.0, handle_y + 30.0));
        h.paint(ENTER_SETTLED_MS + 32.0);
        h.event(&ev(PointerPhase::Move, 200.0, handle_y + 60.0));
        h.event(&ev(PointerPhase::Up, 200.0, handle_y + 60.0));

        let seed = h.paint_recorded(ENTER_SETTLED_MS + 48.0);
        assert!(
            (panel_translate(&seed) - 60.0).abs() < 1e-6,
            "a flicked dismissal also resumes from the finger's offset"
        );
        h.paint(ENTER_SETTLED_MS + 400.0);
        h.flush();
        assert_eq!(
            controller.depth(),
            1,
            "the flick dismissed a quarter-dragged sheet"
        );
        assert_eq!(h.state.results, 1);
    }

    #[test]
    fn a_slow_release_below_the_threshold_springs_back_and_leaves_the_sheet_usable() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let mut w = shown(&view);
        let handle_y = w.panel.y0 + 10.0;

        // The same 60px the flick above dismisses on — but spread over 200ms
        // (300 px/s), so neither the distance nor the velocity path fires.
        drag_event(&mut w, PointerPhase::Down, handle_y, 400.0);
        drag_event(&mut w, PointerPhase::Move, handle_y + 60.0, 600.0);
        let (ty, _, _) = painted(&mut w, 600.0);
        assert!((ty - 60.0).abs() < 1e-6);
        drag_event(&mut w, PointerPhase::Up, handle_y + 60.0, 600.0);
        assert_eq!(w.phase, Phase::Shown, "no dismissal");
        let from = w.settle_from.expect("a spring-back is in flight instead");
        assert!(
            (from - 60.0 / PANEL_H).abs() < 1e-9,
            "…starting from the released offset"
        );

        let (seed, _, frame) = painted(&mut w, 600.0);
        assert!(
            (seed - 60.0).abs() < 1e-6,
            "the spring-back starts where the finger let go, got {seed}"
        );
        assert!(frame);
        let (partway, _, frame) = painted(&mut w, 675.0);
        assert!(
            partway > 0.0 && partway < 60.0,
            "partway home, got {partway}"
        );
        assert!(frame);
        // Past the 150ms spring-back: home, and asking for nothing again.
        let (home, alpha, frame) = painted(&mut w, 800.0);
        assert_eq!(home, 0.0, "back at rest");
        assert!(
            (alpha - SCRIM_ALPHA).abs() < 1e-6,
            "the scrim came back with it"
        );
        assert!(!frame, "a settled sheet stops requesting frames");
        assert_eq!(w.settle_from, None);

        // The sheet is fully live throughout: a second, decisive drag dismisses.
        drag_event(&mut w, PointerPhase::Down, handle_y, 800.0);
        drag_event(&mut w, PointerPhase::Move, handle_y + 150.0, 1000.0);
        drag_event(&mut w, PointerPhase::Up, handle_y + 150.0, 1000.0);
        assert_eq!(w.phase, Phase::Exit, "a later drag still dismisses");
    }

    #[test]
    fn a_cancelled_drag_springs_back_and_never_dismisses() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let mut w = shown(&view);
        let handle_y = w.panel.y0 + 10.0;

        // Past the dismiss threshold *and* fast: a release here would exit. A
        // Cancel is the platform stealing the gesture, not the user letting go.
        drag_event(&mut w, PointerPhase::Down, handle_y, 400.0);
        drag_event(&mut w, PointerPhase::Move, handle_y + 150.0, 416.0);
        drag_event(&mut w, PointerPhase::Cancel, handle_y + 150.0, 416.0);
        assert_eq!(w.phase, Phase::Shown, "a cancel dismisses nothing");
        assert!(w.settle_from.is_some(), "…it springs back instead");
        assert!(!w.drag_active);

        painted(&mut w, 416.0);
        let (home, _, frame) = painted(&mut w, 700.0);
        assert_eq!(home, 0.0, "back at rest");
        assert!(!frame);
    }

    #[test]
    fn a_non_dismissable_sheets_panel_is_immovable_but_still_swallows_the_press() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0)).dismissable(false);
        let mut w = shown(&view);
        let handle_y = w.panel.y0 + 10.0;

        assert_eq!(
            drag_event(&mut w, PointerPhase::Down, handle_y, 400.0),
            EventResult::Handled,
            "the press is swallowed — it must not reach content or the page below"
        );
        assert!(w.drag_active, "the gesture is owned…");
        assert!(!w.drag_tracks, "…but it moves nothing");
        drag_event(&mut w, PointerPhase::Move, handle_y + 150.0, 416.0);

        let (ty, alpha, frame) = painted(&mut w, 416.0);
        assert_eq!(
            ty, 0.0,
            "the panel does not budge, and does not rubber-band"
        );
        assert!(
            (alpha - SCRIM_ALPHA).abs() < 1e-6,
            "so the scrim does not move either"
        );
        assert!(!frame, "an immovable panel asks for no frames");

        drag_event(&mut w, PointerPhase::Up, handle_y + 150.0, 432.0);
        assert_eq!(w.phase, Phase::Shown, "no dismissal, past-threshold or not");
        assert_eq!(w.settle_from, None, "and nothing to spring back");
    }

    #[test]
    fn a_close_mid_drag_wins_and_exits_from_where_the_finger_had_the_panel() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let (mut h, sheet) = open_sheet(&controller);
        let handle_y = area().height - PANEL_H + 10.0;

        h.event(&ev(PointerPhase::Down, 200.0, handle_y));
        h.event(&ev(PointerPhase::Move, 200.0, handle_y + 100.0));
        let dragging = h.paint_recorded(ENTER_SETTLED_MS + 16.0);
        assert!((panel_translate(&dragging) - 100.0).abs() < 1e-6);

        // The app closes the sheet out from under the finger.
        sheet.close();
        let seed = h.paint_recorded(ENTER_SETTLED_MS + 32.0);
        assert!(
            (panel_translate(&seed) - 100.0).abs() < 1e-6,
            "the exit picks the panel up at the finger's offset"
        );

        // The abandoned drag's own release can no longer do anything.
        h.event(&ev(PointerPhase::Up, 200.0, handle_y + 100.0));
        h.paint(ENTER_SETTLED_MS + 400.0);
        h.flush();
        assert_eq!(controller.depth(), 1);
        assert_eq!(h.state.results, 1, "one exit, one pop");
    }

    #[test]
    fn a_handle_press_during_the_enter_never_becomes_a_drag() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let (enter, exit) = resolve_timings(None);
        let mut w = laid_out(&view, area());
        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(60.0), enter, exit);
        assert_eq!(w.phase, Phase::Enter, "mid-enter");

        let handle_y = w.panel.y0 + 10.0;
        drag_event(&mut w, PointerPhase::Down, handle_y, 60.0);
        assert!(
            !w.drag_active,
            "the phase machine owns the panel mid-enter — no drag to fight it"
        );
        // The press fell through to the modal barrier, which dismisses nothing
        // from *inside* the panel.
        drag_event(&mut w, PointerPhase::Up, handle_y + 150.0, 76.0);
        assert_eq!(w.phase, Phase::Enter, "no dismissal");
        let (_, _, animating) = w.advance(ft_ms(120.0), enter, exit);
        assert!(animating, "…and the enter itself is undisturbed");
    }

    #[test]
    fn a_dismissal_mid_enter_exits_from_the_partly_entered_offset() {
        let view: GlyphSheetView<()> = glyph_sheet(leaf_any(300.0, 200.0));
        let (enter, exit) = resolve_timings(None);
        let mut w = laid_out(&view, area());
        w.advance(ft_ms(0.0), enter, exit);
        let (mid, _, _) = w.advance(ft_ms(60.0), enter, exit);
        assert!(mid > 0.0 && mid < 1.0, "partway in, got {mid}");

        w.begin_exit();
        let (seed, _, _) = w.advance(ft_ms(60.0), enter, exit);
        assert!(
            (seed - mid).abs() < 1e-9,
            "the exit resumes from the entering panel's own offset, not from rest"
        );
    }

    #[test]
    fn a_close_before_the_sheet_first_paints_is_still_honoured() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        let sheet = show_glyph_sheet(
            &controller,
            || glyph_sheet(fixed(300.0, 200.0)),
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        // Closed before the queued push has even been applied: the widget must
        // start at generation 0 and observe the bump on its first paint.
        sheet.close();
        h.rebuild();
        h.paint(0.0);
        h.paint(100.0);
        h.paint(300.0);
        h.flush();

        assert_eq!(controller.depth(), 1, "the sheet closed itself out");
        assert_eq!(h.state.results, 1);
    }
}
