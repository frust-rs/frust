//! Glyph modal bottom sheet: a full-area **scrim** + a bottom-anchored **panel**
//! wrapping a single app-provided content view — the Glyph recipe over the
//! same navigator transparent-push modal plumbing [`crate::glyph::dialog`] uses.
//!
//! # Navigator-modal architecture (reuse, don't fork)
//!
//! Like [`crate::glyph::dialog`], a `GlyphSheetView` is pushed as a
//! **transparent navigator page** via [`NavigatorController::push_with_options`]
//! (with a [`BackPolicy`](crate::nav::navigator::BackPolicy) and a dismiss
//! signal — the exact same seam [`show_glyph_dialog`](crate::glyph::dialog::show_glyph_dialog)
//! uses) — the page below stays visible under the scrim, and the navigator's
//! existing modal contract makes it modal. [`show_glyph_sheet`] wraps the push
//! and wires dismissal to `controller.pop()`. This module edits **no** nav
//! file. **Not** [`NavigatorController::push_transparent_for_result`] — that is
//! `material::dialog`'s precedent, not this module's.
//!
//! # Enter/exit staging — the scrim fades, the panel slides (independently)
//!
//! This is the module's one hard behavioural requirement, recorded at an
//! on-device gate: **the scrim must fade while only the panel slides.** An
//! app's first workaround pushed the whole page with
//! [`PageTransition::SlideUp`](crate::PageTransition::SlideUp) — a scrim
//! painted as part of that same page would visibly sweep bottom-to-top with
//! it, since [`PageTransition::SlideUp`] offsets the *entire* page's paint
//! (see `examples/glyph-catalog`'s `overlays.rs`, whose bottom-sheet demo
//! pushed a bespoke card through exactly that raw primitive before this
//! module existed — with no scrim of its own at all, since one drawn as part
//! of that page would have had this bug). A single navigator
//! [`Layer`](crate::nav::transition::Layer) transform applied to the
//! *whole page* cannot express "panel translates but scrim only fades" — so,
//! exactly like [`crate::glyph::dialog`] (which scales its panel while the
//! scrim cross-fades on its own), this widget drives its **own** enter/exit
//! timeline from `PaintCtx::frame_time` and is pushed with
//! [`TransitionSpec::NONE`] (the navigator supplies the modal contract; the
//! widget supplies the motion):
//!
//! * the **scrim** is painted as a plain full-area fill *before* any
//!   transform is pushed, its alpha driven straight off the timeline's
//!   progress — it never moves, only fades;
//! * the **panel** is painted *inside* a single [`kurbo::Affine::translate`]
//!   pushed after the scrim, sliding from fully off-screen (`y` = panel
//!   height below its rest position) up to rest over the enter duration, and
//!   reversing on exit.
//!
//! Enter runs over `durations.base` (220ms, spatial easing); exit reverses
//! over the faster `durations.fast` (150ms, exit easing) — "exits always
//! faster than entrances" (mirrors [`crate::glyph::dialog`]'s rule). A dismiss
//! gesture (scrim tap, handle drag past threshold, `Escape`, or an Android
//! back press) does **not** pop immediately: it flips the widget into its exit
//! phase via [`begin_exit`](GlyphSheetWidget::begin_exit), and only when that
//! animation completes does the app-supplied close callback fire — from
//! paint, which is sound because [`NavigatorController::pop`] merely enqueues
//! an op applied on the next rebuild. **Every dismiss path funnels through
//! `begin_exit`**, including the handle drag below, which is what keeps the
//! drag from fighting the fade/slide requirement: there is no code path that
//! pops the page without playing the staged exit first.
//!
//! # The dismiss callback is state-free (mirrors `glyph::dialog`)
//!
//! Because the close callback fires from **paint** (after the exit
//! animation), it is a plain `Fn()` — not a `Fn(&mut State)` — exactly like
//! [`crate::glyph::dialog`]'s [`on_close`](GlyphSheetView::on_close) (a
//! deliberate divergence from [`crate::material::sheet`]'s `Fn(&mut State)`
//! `on_dismiss`, whose dismiss is immediate with no staged exit to wait for).
//! App state changes flow through the navigator's `on_result` callback
//! instead, delivered with `&mut State` after the pop.
//!
//! [`on_close`]: GlyphSheetView::on_close
//!
//! # Drag-to-dismiss — added, but routed through the staged exit
//!
//! Material's bottom sheet has a threshold-simple handle drag
//! ([`crate::material::sheet`]); Cupertino's action sheet has none. This
//! module **adds** one, because a bottom sheet — unlike a centered dialog —
//! is a strongly drag-affording shape (Flutter, M3, and iOS all offer it), and
//! withholding it would be the more surprising choice for this widget
//! specifically. It is threshold-simple like material's: a press starting in
//! the top 44px handle strip that releases more than half the panel's height
//! lower calls [`begin_exit`](GlyphSheetWidget::begin_exit) (never an
//! immediate pop) — so a drag-dismiss plays the identical scrim-fade +
//! panel-slide exit a scrim tap or `Escape` does. The sheet does **not**
//! follow the finger frame-by-frame during the drag (no interactive
//! held/settle transform); that richer treatment is deferred, same as
//! material's.
//!
//! # Content
//!
//! Unlike [`crate::glyph::dialog`]'s title/body/actions builder, a sheet wraps
//! exactly **one** app-provided content view — mirroring
//! [`crate::material::sheet::bottom_sheet`]'s generic single-child shape
//! rather than [`crate::cupertino::action_sheet`]'s fixed action-row content
//! model, since a Glyph sheet has no built-in notion of "rows" to lay out. The
//! content lays out full-width, below the drag-handle strip, at its own
//! intrinsic height (loose width, unbounded height) — the panel grows to fit,
//! same overflow contract as `material::sheet`: an unbounded-height child can
//! push the panel past the top of the viewport, so wrap genuinely unbounded
//! content in its own scrolling container first.
//!
//! # Semantics — `Role::Dialog` with the modal flag (not `Role::Menu`)
//!
//! The sheet contributes one [`Role::Dialog`] container node with the
//! accesskit **modal** flag set, and forwards the content subtree through
//! [`ChildPod::semantics_child`]. This mirrors
//! [`crate::material::sheet`]'s choice, **not**
//! [`crate::cupertino::action_sheet`]'s `Role::Menu` (no modal flag): the
//! cupertino sheet earns `Role::Menu` because its content is *always* a fixed
//! list of selectable action rows — genuinely menu-shaped to assistive tech.
//! This module's content is an arbitrary single child (same shape as
//! `material::sheet`'s), with no menu semantics of its own to inherit, so
//! there is nothing menu-like about it — a modal surface floating over the
//! app is what it actually is, the same category [`crate::glyph::dialog`]
//! occupies. accesskit has no dedicated "bottom sheet" role, so `Dialog` +
//! modal is the closest honest fit, same reasoning `material::sheet`
//! documents for its own identical choice.
//!
//! # `dismissable(bool)` + back-dismiss
//!
//! [`GlyphSheetView::dismissable`] (default `true`) is the single barrier flag
//! gating the scrim tap, the handle drag, `Escape`, and an Android back press
//! together — `false` disables all four (only an app-driven `controller.pop()`
//! still closes it) and [`show_glyph_sheet`] pushes the page with
//! [`BackPolicy::Veto`](crate::nav::navigator::BackPolicy::Veto). `true`
//! pushes [`BackPolicy::DismissAnimated`](crate::nav::navigator::BackPolicy::DismissAnimated):
//! `show_glyph_sheet` hands the widget the shared dismiss-signal cell
//! [`NavigatorController::request_back`] bumps on a back press, and the
//! widget's `paint` pass compares it against the last-seen value and calls
//! [`begin_exit`](GlyphSheetWidget::begin_exit) — the identical staged exit a
//! scrim tap/drag/Escape cancel drives (mirrors
//! [`crate::glyph::dialog`]'s `observe_dismiss_signal`).
//!
//! # Interaction with `overlay_host`
//!
//! This widget needs **no** change to be pushed on a root
//! [`overlay_host`](crate::nav::navigator::overlay_host) controller instead of
//! an inner navigator: it fills `ctx.origin()..ctx.size()` with its scrim, and
//! at the root that rect *is* the window (`overlay_host`'s own module docs —
//! "The host owns no scrim" — document this contract). Pushing on a host
//! controller is what gives the sheet root-level modality (dimming chrome
//! above an inner navigator); the sheet itself is unaware which controller it
//! was pushed on.
//!
//! # Only one floating layer
//!
//! Enforcing "at most one dialog/sheet/overlay at a time" is the app's (or the
//! navigator's) concern — this widget does not police it, exactly like
//! [`crate::glyph::dialog`].

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, EventCtx, EventResult,
    FrameTime, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase,
    SemanticsCtx, View, Widget, any,
};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, RoundedRect, RoundedRectRadii, Shape, Size};
use peniko::{Brush, Color};

use crate::Timing;
use crate::nav::navigator::{BackPolicy, NavigatorController, PopResult, PushOptions};
use crate::nav::transition::{TransitionDriver, TransitionSpec, make_driver};

/// Scrim opacity behind the panel at full enter — the same value
/// [`crate::glyph::dialog`] uses (the Glyph modal barrier constant, applied
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
/// Fraction of the panel's own height a handle-drag must exceed to dismiss —
/// the same community-approximate half-height heuristic
/// [`crate::material::sheet`] uses (no single published constant exists for
/// this).
const DRAG_DISMISS_FRACTION: f64 = 0.5;

/// Unthemed-fallback panel top-corner radius. A theme resolves this from
/// `shape.large` — the same token [`crate::glyph::dialog`]'s panel uses (14px),
/// deliberately not M3's chunkier `extra_large` (28dp): Glyph's terminal
/// chrome reads as one consistent corner language across its floating
/// surfaces.
const RADIUS: f64 = 14.0;
/// Unthemed-fallback panel surface fill (Glyph dark `bg-raised` `#1e2430`,
/// matching [`crate::glyph::dialog`]'s panel — a sheet is a floating surface
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
/// `crate::glyph::dialog`'s constant).
const PATH_TOLERANCE: f64 = 0.1;
/// Unthemed-fallback scrim base color (a theme resolves this from
/// `colors.scrim`).
const SCRIM: Color = Color::from_rgb8(0x00, 0x00, 0x00);
/// Unthemed-fallback drag-handle color (a theme resolves this from
/// `colors.on_surface_variant`).
const HANDLE_COLOR: Color = Color::from_rgb8(0x8a, 0x8f, 0x98);
/// Unthemed-fallback chrome-level shadow (mirrors
/// `crate::glyph::dialog`'s `--shadow-chrome`). A theme resolves this from
/// `elevation.level5`.
const SHADOW_Y: f64 = 12.0;
const SHADOW_BLUR: f64 = 32.0;
const SHADOW_ALPHA: f32 = 0.45;

/// Enter duration fallback (`durations.base` = 220ms) when no theme is
/// threaded — mirrors [`crate::glyph::dialog`].
const ENTER_DURATION: Duration = Duration::from_millis(220);
/// Enter easing fallback (Glyph `spatial` cubic).
const ENTER_CURVE: Curve = Curve::Cubic(0.34, 1.35, 0.64, 1.0);
/// Exit duration fallback (`durations.fast` = 150ms; exits faster than
/// entrances).
const EXIT_DURATION: Duration = Duration::from_millis(150);
/// Exit easing fallback (Glyph `exit` accelerate-out cubic).
const EXIT_CURVE: Curve = Curve::Cubic(0.4, 0.0, 1.0, 1.0);
/// `reduce_motion`'s collapsed crossfade duration (mirrors
/// `nav::transition::REDUCE_MOTION_DURATION`).
const REDUCE_MOTION_DURATION: Duration = Duration::from_millis(120);

/// Replace `color`'s alpha channel with `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The `(enter, exit)` [`Timing`]s for the current `reduce_motion` state —
/// identical resolution to [`crate::glyph::dialog::resolve_timings`] (kept as
/// its own copy rather than a shared helper: the two modules are intentionally
/// not coupled, see this crate's `docs/REVIEW_FOCUS.md` on the material/
/// cupertino sheet split this module was scoped to avoid repeating).
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
/// `crate::glyph::dialog::resolve_shadow`).
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
    /// The shared back-press dismiss-signal cell (the `DismissAnimated` seam)
    /// — wired internally by [`show_glyph_sheet`], never part of the public
    /// builder surface (see the [module docs](self)).
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
    /// `true`; see the [module docs](self)). `false` disables all four; only
    /// an app-driven `controller.pop()` still closes it.
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Set the state-free close callback — invoked once, from paint, when the
    /// exit animation completes after a scrim tap/handle drag/Escape cancel.
    /// [`show_glyph_sheet`] wires this to `controller.pop()` automatically.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }
}

/// Push `build`'s sheet as a transparent navigator page (the page below stays
/// visible under the scrim), and register `on_result` for the value the sheet
/// pops with. The scrim tap/handle drag/Escape cancel is wired to
/// `controller.pop()` (an empty [`PopResult`]).
///
/// The sheet is pushed with [`TransitionSpec::NONE`] — the navigator supplies
/// the modal contract, the widget supplies its own scrim-fade + panel-slide
/// staging (see the [module docs](self)).
///
/// ```ignore
/// show_glyph_sheet(
///     &state.nav,
///     || glyph_sheet(pane_picker_rows()),
///     |state: &mut State, result: PopResult| { /* ... */ },
/// );
/// ```
pub fn show_glyph_sheet<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> GlyphSheetView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let close_ctrl = controller.clone();
    // Peeked once, at show-time: the back policy/dismiss-signal wiring is
    // fixed for the life of this pushed page (mirrors
    // `crate::glyph::dialog::show_glyph_dialog`'s identical peek), even
    // though `build` is re-invoked on every later navigator rebuild to diff
    // the page's content.
    let dismissable = build().dismissable;
    let signal = dismissable.then(|| Rc::new(Cell::new(0u64)));
    let widget_signal = signal.clone();
    let mut options = PushOptions::transparent()
        .transition(TransitionSpec::NONE)
        .back(if dismissable {
            BackPolicy::DismissAnimated
        } else {
            BackPolicy::Veto
        })
        .on_result(on_result);
    if let Some(sig) = &signal {
        options = options.dismiss_signal(sig.clone());
    }
    controller.push_with_options(
        move || {
            let ctrl = close_ctrl.clone();
            let mut view = build().on_close(move || ctrl.pop());
            view.dismiss_signal = widget_signal.clone();
            any::<State, _>(view)
        },
        options,
    );
}

/// The sheet's enter/exit lifecycle phase (mirrors
/// `crate::glyph::dialog::Phase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Animating in (panel sliding up from off-screen, scrim fading in).
    Enter,
    /// Fully shown, at rest.
    Shown,
    /// Animating out after a scrim/handle-drag/Escape cancel.
    Exit,
    /// Exit complete; the close callback has fired and the page will be popped.
    Dismissed,
}

/// The retained widget for a [`GlyphSheetView`]. See the [module docs](self).
pub struct GlyphSheetWidget {
    content: ChildPod,
    on_close: Option<OnClose>,
    dismissable: bool,
    /// The shared back-press dismiss-signal cell (the `DismissAnimated`
    /// seam) — see [`observe_dismiss_signal`](Self::observe_dismiss_signal).
    dismiss_signal: Option<Rc<Cell<u64>>>,
    /// The last generation observed from `dismiss_signal` (0 with no signal
    /// wired, or a `Veto`/non-dismissable sheet).
    last_seen_dismiss: u64,
    /// The bottom-anchored panel rect **at rest** (fully shown), in the
    /// widget's own local coordinate space — used both to lay the panel out
    /// and, deliberately unanimated, as the hit-test region for the scrim/
    /// panel split (mirrors `crate::glyph::dialog`'s `panel` field, which is
    /// likewise the widget's rest-state rect regardless of its live paint
    /// transform).
    panel: Rect,
    /// The full-width drag-handle touch strip at the top of the panel (local
    /// coords, at rest).
    handle_target: Rect,
    phase: Phase,
    /// The enter/exit progress driver — lazily built on the first paint that
    /// sees the phase (rebuild has no theme to resolve a [`Timing`] from).
    driver: Option<TransitionDriver>,
    /// A handle drag is in flight (the sheet captured the pointer on a `Down`
    /// in the handle strip).
    drag_active: bool,
    /// The `Down` y the drag distance is measured from.
    drag_start_y: f64,
    /// A scrim/panel-background press is in flight (the modal barrier
    /// captured the pointer).
    scrim_captured: bool,
    /// Whether that press started outside the panel (only an outside press
    /// released outside dismisses).
    scrim_down_outside: bool,
    /// Whether a `Down` has already claimed focus this open — the claim-once
    /// guard (see `event`'s "fresh events" Down arm). Seeding `false` in
    /// [`View::build`] is correct with no reset needed elsewhere:
    /// [`show_glyph_sheet`] pushes a fresh page per open, so a new widget
    /// instance (and a fresh `false`) is built every time the sheet opens;
    /// there is no retained instance to reset on close.
    focus_claimed: bool,
}

impl<State: 'static> View<State> for GlyphSheetView<State> {
    type Element = GlyphSheetWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GlyphSheetWidget {
        GlyphSheetWidget {
            content: crate::authoring::build_child(&self.content, ctx),
            on_close: self.on_close.clone(),
            dismissable: self.dismissable,
            dismiss_signal: self.dismiss_signal.clone(),
            last_seen_dismiss: self.dismiss_signal.as_ref().map(|s| s.get()).unwrap_or(0),
            panel: Rect::ZERO,
            handle_target: Rect::ZERO,
            phase: Phase::Enter,
            driver: None,
            drag_active: false,
            drag_start_y: 0.0,
            scrim_captured: false,
            scrim_down_outside: false,
            focus_claimed: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GlyphSheetWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = crate::authoring::rebuild_child(
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
        crate::authoring::teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl GlyphSheetWidget {
    /// Begin the exit animation (a scrim tap/handle drag/Escape cancel).
    /// Idempotent: only an `Enter`/`Shown` sheet can start exiting. **Every**
    /// dismiss path (scrim, handle, `Escape`, back) funnels through this one
    /// function — see the [module docs](self)'s drag-to-dismiss note on why
    /// that matters for the scrim-fade requirement.
    fn begin_exit(&mut self) {
        if matches!(self.phase, Phase::Enter | Phase::Shown) {
            self.phase = Phase::Exit;
            self.driver = None;
        }
    }

    /// Observe the shared back-press dismiss-signal cell (see the
    /// `dismiss_signal` field docs) and begin the exit staging exactly once
    /// per bump (mirrors `crate::glyph::dialog::observe_dismiss_signal`). A
    /// back request flags `PAINT`, so this always runs before the next frame
    /// is shown.
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
    /// scrim's own independent fade fraction. Factored out of `paint` so the
    /// full timeline is drivable with synthetic [`FrameTime`]s in a unit test
    /// (mirrors `crate::glyph::dialog::GlyphDialogWidget::advance`).
    fn advance(&mut self, now: FrameTime, enter: Timing, exit: Timing) -> (f64, f32, bool) {
        match self.phase {
            Phase::Enter => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(enter).0)
                    .advance(now);
                if adv.done {
                    self.phase = Phase::Shown;
                    self.driver = None;
                }
                let p = adv.value.clamp(0.0, 1.0);
                (1.0 - p, p as f32, !adv.done)
            }
            Phase::Shown => (0.0, 1.0, false),
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
                (q, (1.0 - q) as f32, !adv.done)
            }
            Phase::Dismissed => (1.0, 0.0, false),
        }
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
        self.observe_dismiss_signal();
        let theme = Theme::from_paint_ctx(ctx);
        let (enter, exit) = resolve_timings(theme);
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

        // Keep frames coming while animating, and for one frame past
        // Dismissed so the rebuild that applies the pop actually runs.
        if animating || self.phase == Phase::Dismissed {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // 0. A broadcast is not user input, so it belongs to neither the drag nor
        //    the scrim arm below: forward it to the content unconditionally and
        //    consume nothing, so a deferred callback queued inside the sheet still
        //    flushes while a finger is down.
        if event.is_broadcast() {
            crate::authoring::route_event_single(&mut self.content, ctx, event);
            return EventResult::Ignored;
        }
        // 1. An in-flight handle drag owns the pointer stream.
        if self.drag_active {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Handled;
            };
            return match p.phase {
                PointerPhase::Move => EventResult::Handled,
                PointerPhase::Up => {
                    let dy = p.position.y - self.drag_start_y;
                    let threshold = self.panel.height() * DRAG_DISMISS_FRACTION;
                    if self.dismissable && dy > threshold {
                        self.begin_exit();
                    }
                    self.drag_active = false;
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    self.drag_active = false;
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
            return crate::authoring::route_event_single(&mut self.content, ctx, event);
        };
        match p.phase {
            PointerPhase::Down => {
                // The first Down anywhere in the sheet claims focus, so a
                // subsequent Escape has a focus chain to travel; the claim is
                // held until this page pops, so a later press re-claiming
                // would be redundant.
                if !self.focus_claimed {
                    ctx.request_focus();
                    self.focus_claimed = true;
                }
                if self.handle_target.contains(p.position) {
                    self.drag_active = true;
                    self.drag_start_y = p.position.y;
                    ctx.capture_pointer();
                    return EventResult::Handled;
                }
                if crate::authoring::route_event_single(&mut self.content, ctx, event)
                    == EventResult::Handled
                {
                    return EventResult::Handled;
                }
                self.scrim_captured = true;
                self.scrim_down_outside = !self.panel.contains(p.position);
                ctx.capture_pointer();
                EventResult::Handled
            }
            _ => crate::authoring::route_event_single(&mut self.content, ctx, event),
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::navigator::{NavigatorView, navigator};
    use crate::test_support::leaf_any;
    use crate::text::text;
    use frust_core::{
        BuildCtx, KeyEvent, Modifiers, PointerButton, PointerEvent, RenderRoot, any as core_any,
    };
    use frust_text::TextContext;
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
    fn second_press_does_not_reclaim_focus_after_one_open() {
        use frust_core::ChildPod;

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

        // Simulate an external clear so the second Down's own effect on the
        // recorded flag is isolated: if the guard holds, the widget itself
        // never re-calls `request_focus`, so the flag stays as we set it.
        pod.set_focused(false);
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 10.0, 10.0));
        }
        assert!(
            !pod.is_focused(),
            "a second Down within the same open must not re-request focus"
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
        let controller = crate::nav::navigator::NavigatorController::<()>::new();
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

    fn leaf_any_widget(w: f64, h: f64) -> impl View<(), Element = crate::test_support::LeafWidget> {
        crate::test_support::leaf(w, h)
    }
}
