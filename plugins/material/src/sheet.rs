// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// whose `lib/components/bottom_sheets/` tree is `m3e_bottom_sheets.dart` +
// `styles/m3e_bottom_sheet_theme.dart`.
// Porting decision: the reference's own drag handling is velocity-only
// (`_handleDragEnd` fires past a fling threshold with no live position
// tracking) and its entrance is a plain duration+curve `SlideTransition`
// (`M3EMotion.long1` + `emphasizedDecelerate`). This port instead tracks the
// drag interactively (the panel follows the finger) and drives the
// entrance/exit/drag-settle with a physical spring
// (`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`), matching the
// spring machinery the rest of this crate's M3E-flavored motion already uses
// (`crate::button::motion`, `crate::loading_indicator`) rather than the
// reference's simpler duration+curve ramp.

//! The modal M3 `BottomSheet`: a full-area widget = **scrim** (32%
//! scrim-color fill, fading with the panel's own spring) + a **bottom-anchored
//! panel** (top corners extra-large 28dp, `surfaceContainerLow`, a centered
//! drag handle **32dp wide × 4dp tall** in a 48dp touch target), pushed as a
//! transparent navigator page via
//! [`NavigatorController::push_transparent_for_result`]. [`show_bottom_sheet`]
//! wraps the push.
//!
//! # Navigator-modal architecture
//!
//! Like [`mod@crate::dialog`], the scrim is part of *this* widget (the
//! navigator provides none and routes input only to the top page), and the
//! sheet is designed to be pushed as a transparent page over a static
//! background. Dismissal is always `controller.pop()` — staged behind the exit
//! spring on that path (see *Who owns the exit*) — wired to three gestures:
//!
//! * a **scrim tap** (a press+release outside the panel) pops with an *empty*
//!   [`PopResult`];
//! * a **drag-release on the handle**, decided by position and velocity (see
//!   below);
//! * **Escape**, once the sheet holds focus (see *Keyboard operability*).
//!
//! # Interactive drag + spring settle
//!
//! Unlike the reference, the panel **follows the finger**: a press in the
//! 48dp handle strip captures the pointer, and once the gesture passes
//! [`frust::input::TOUCH_SLOP`] every `Move` scrubs [`BottomSheetWidget`]'s
//! internal `progress` (`0.0` fully closed/off-screen, `1.0` at rest) by the
//! drag's own fraction of the panel's height — the panel translates with the
//! pointer, clamped to `0.0..=1.0`. A release below [`TOUCH_SLOP`] is a *tap*:
//! unchanged from v1, nothing dismisses (only a real drag can).
//!
//! On release, the panel's fate is decided by **position and velocity**,
//! mirroring [`overlay::modal`](crate::overlay::modal)'s own drag-settle: a
//! release speed at or above [`FLING_VELOCITY`] commits in the direction it
//! was flung (a fast downward flick dismisses even from near the top; a fast
//! upward one stays open even from near the bottom); otherwise the position
//! alone decides — past half the sheet's own height ([`DRAG_DISMISS_FRACTION`])
//! dismisses, short of it springs back open. Either way, the panel
//! **springs** to its resting point (open or closed) rather than snapping or
//! easing on a fixed duration — [`BottomSheetWidget::spring_to`] launches a
//! fresh leg of [`SHEET_SPRING`] (`MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`)
//! from wherever `progress` currently reads, at the release's own measured
//! speed (boosted to at least [`SPRING_KICK`] in the decided direction when
//! the raw reading is too weak or the wrong sign to read as a deliberate
//! commit).
//!
//! # Spring entrance/exit
//!
//! The sheet no longer rides the navigator's own page-transition slide
//! ([`show_bottom_sheet`] pushes with [`TransitionSpec::NONE`] — the widget
//! stages its own motion, the [`crate::overlay::modal`] precedent): on its
//! first `paint`, [`BottomSheetWidget`] kicks [`SHEET_SPRING`] from `progress
//! = 0.0` toward `1.0`, so **the first frame is invisible** (the panel is off
//! -screen) and the slide-up becomes visible only from the second frame
//! onward — the same one-frame lag [`crate::overlay::modal`]'s own ramp
//! documents. A dismiss trigger with no drag behind it (a scrim tap, Escape,
//! an Android back press) kicks the same spring the other way, at
//! [`SPRING_KICK`]. `Theme.motion.reduce_motion` collapses both to a jump:
//! `paint` stops the spring and snaps `progress` straight to its target.
//!
//! # Who owns the exit
//!
//! **The widget's own spring owns it, on every mount, and on the navigator
//! path the pop waits for that spring to settle** — the same
//! [`crate::overlay::modal`] staging shape, reached here without a single new
//! public builder method.
//!
//! The alternative was letting the navigator animate the pop, by pushing with
//! a slide-up page transition instead of [`TransitionSpec::NONE`]. It **is**
//! expressible, and was rejected on its merits rather than for lack of a seam:
//! a [`TransitionSpec`] is one spec per page, applied to the push *and* the pop
//! alike, so a *preset* pop-side slide-down necessarily brings its push-side
//! slide-up back with it — double-animating the entrance the widget's own spring
//! already runs — but [`frust::PageTransition::Custom`] is handed `is_pop` and
//! maps it onto the `(entering, leaving)` layer pair itself, so an asymmetric,
//! pop-only spec is a plain use of that public seam, with no `frust-widgets`
//! edit involved. What decides it is ownership: the entrance is already a spring
//! *inside* this widget, and a navigator-driven exit starts from the page
//! transition's own `0..=1`, knowing nothing of the `progress` a live handle drag
//! left behind — the drag-release settle could no longer pick up where the finger
//! stopped, and the sheet's motion would live in two places at once. So both
//! halves stay under one owner, the widget, and the navigator contributes no page
//! offset in either direction.
//!
//! Staging costs no public surface because the pop it defers is **state-free**:
//! [`show_bottom_sheet`] wires an internal `on_close` (a [`StagedPop`] around
//! `controller.pop()`) onto the view the same private way it already wires the
//! back-press [`dismiss_signal`](PushOptions::dismiss_signal) cell — neither is
//! a builder method, and neither is reachable from outside this module. Every
//! dismiss trigger (scrim tap, handle-drag release, Escape, Android back) then
//! takes one shape: kick the closing spring, mark the widget `exiting`, and let
//! `paint` fire the pop exactly once the spring has settled at `progress == 0`
//! (a `closed` latch makes that firing terminal, so a trigger landing in the
//! frames between the enqueued pop and the rebuild that drains it cannot pop
//! the page underneath). Symmetrically, a leg aimed back *open* — a drag caught
//! mid-exit, a release short of the dismiss threshold, a `Cancel` — clears
//! `exiting` again, since a leg targeting `1.0` is by definition not a
//! dismissal; leaving it set would strand the sheet at rest with a staged exit
//! that can only fire at the closed end, refusing every later trigger.
//! `reduce_motion` collapses the wait rather than skipping it: the exit snaps to
//! `0` and fires on the next paint.
//!
//! The deferred pop is identity-guarded, not blind. It takes the navigator's
//! *top* page, and the ramp it waits out is long enough for the app to push
//! another one (a deep link, an async completion) — which a bare `pop()` would
//! then take instead of this sheet. The hook is therefore a
//! [`StagedPop`]: armed with the stack's identity when the exit is staged, and
//! refusing to fire if that identity moved. A refused pop is *reported* rather
//! than swallowed — the sheet springs back open and stays dismissible, instead
//! of latching `closed` over a page it never removed.
//!
//! **A mount with no `on_close` — a standalone [`frust::Stack`] mount, or any
//! sheet built by [`bottom_sheet`] alone — keeps the immediate, state-touching
//! v1 dismissal**: [`BottomSheetView::on_dismiss`] (a `Fn(&mut State)`) fires
//! in the same event as the spring kick, exactly as before, because there is no
//! state-free hook to defer into. Exactly one of the two fires per dismissal.
//!
//! # Convergence with `overlay::modal`
//!
//! [`crate::overlay::modal`] is the merged host built on top of *this*
//! module's own `BackPolicy`/`dismiss_signal` mechanisms, and now carries a
//! more general drag-to-close-with-snap-points machine of its own. This
//! module does **not** rebuild itself on top of that host — that host
//! generalizes to arbitrary snap points via a duration+curve ramp, where this
//! sheet's motion is a spring throughout — but it now shares the host's
//! **staged-exit shape**: a state-free close hook fired from `paint` on settle,
//! guarded by an `exiting`/`closed` pair, with the state-bearing `on_dismiss`
//! kept as the unstaged fallback for a mount that wires no hook. The
//! theme-role resolution converges too, reading [`crate::overlay`]'s shared
//! accessors (`scrim`/`container`/`on_surface_variant`/`radius`) rather than
//! duplicating them locally.
//!
//! # Panel anatomy
//!
//! The panel spans the full width, anchored to the bottom edge, with its **top**
//! two corners rounded to the extra-large (28dp) shape and its bottom corners
//! square (they sit at the screen edge). It sizes to its content plus the 48dp
//! handle strip at the top; the visual drag-handle indicator is 32×4dp, centered
//! horizontally within that strip. The panel's own extent never changes with
//! `progress` — only its Y offset does, so a drag scrub or a spring-advanced
//! frame repositions it (`BottomSheetWidget::reposition`) without a relayout.
//!
//! # Standalone use
//!
//! A `BottomSheet` is a plain widget: it works inside a [`frust::Stack`] too
//! (the scrim is still its own). The navigator path is the primary, documented
//! one. This module does **not** edit any nav file.
//!
//! # Semantics
//!
//! The sheet contributes one [`Role::Dialog`] container node with the accesskit
//! **modal** flag set — a documented choice: accesskit has no dedicated
//! "bottom sheet" role, and a modal sheet is, to assistive tech, a modal
//! dialog-ish surface (the same role the [`mod@crate::dialog`] uses). Its
//! content becomes the node's accesskit children.
//!
//! # Keyboard operability
//!
//! **Escape-to-dismiss now works, once the sheet has focus.** A `Down`
//! anywhere in the sheet (scrim, handle, or panel background) claims focus
//! via `EventCtx::request_focus` — the sheet already captures its whole area,
//! so this is a pure opt-in with no new hit-testing. Once focused, a
//! focus-routed `Key(Escape)` invokes the *same* dismiss path as a scrim tap
//! or handle drag (`on_dismiss`) — unless the sheet's `content` itself holds
//! the deeper focus path, in which case content gets first crack at the key
//! (mirroring how an action consumes an event before the dialog's modal
//! barrier does). **There is still no hook to focus the sheet on appear**
//! (auto-focus-on-appear) — a caller must complete one pointer interaction
//! with the sheet before Escape does anything; that gap is deferred to a
//! future focus-manager work item.
//!
//! The accesskit **modal** flag set above is nonetheless **kept deliberately**,
//! not dropped. No platform `accesskit_*` adapter is wired yet (see
//! `docs/ARCHITECTURE.md`'s Semantics pass), so there is no live assistive-tech
//! audience the flag could currently mislead.
//!
//! # State layer
//!
//! This sheet has no `StateLayer` surface of its own (the scrim, panel, and
//! drag handle are plain fills, not an M3 interactive surface) — there is
//! nothing here for `StateLayer::set_focused` to wire into.
//!
//! # `dismissable(bool)` + back-dismiss
//!
//! [`BottomSheetView::dismissable`] (default `true`) is Flutter's
//! `isDismissible`+`enableDrag` collapsed into one v1 flag (spec parity):
//! `false` disables the scrim tap, the handle drag (a drag past the
//! dismiss threshold instead **springs back open** — the panel already
//! tracked the finger down toward closed, so a bare no-op would leave it
//! resting there as an invisible full-window barrier), and `Escape`, and
//! [`show_bottom_sheet`] pushes the page with [`frust::BackPolicy::Veto`] so a
//! back press is consumed with no effect. `true` pushes
//! [`frust::BackPolicy::DismissAnimated`]: a back press bumps the shared
//! dismiss-signal cell [`NavigatorController::request_back`] writes,
//! observed from `paint` (`observe_dismiss_signal`) — which stages exactly the
//! reverse ramp every other dismiss trigger stages (see *Who owns the exit*),
//! so back is animated rather than instant. Because the pop that ramp
//! eventually fires only *enqueues* a `NavOp::Pop` (it writes no tracked
//! signal), the paint that fires it requests the next frame
//! ([`PaintCtx::request_frame`]) so the enqueued pop is guaranteed a draining
//! rebuild — otherwise a dirty-driven desktop shell idles and the mobile frame
//! gate skips, leaving the back press dead until an unrelated later frame. The
//! in-flight ramp keeps its own frames coming the same way.
//!
//! # App-initiated dismiss: adopting [`ModalDismiss`]
//!
//! [`BottomSheetView::dismiss_handle`] installs [`crate::overlay::modal`]'s own
//! [`ModalDismiss`] handle — the same public type [`crate::side_sheet`] installs,
//! never a second one minted for this module. The sheet's content (a row tap,
//! an "Apply" button) triggers it, and the widget observes the request through
//! exactly the shape [`mod@crate::overlay::modal`]'s own host does: a
//! generation cell latched at `build`, re-seeded on `rebuild` only when the
//! handle's *identity* changed, and observed in **both** the `event` pass
//! (right after the content routing that most likely raised it) and the
//! `paint` pass (the backstop for a request raised outside an event). Neither
//! observation stages a *second* exit path — both route straight into this
//! module's own [`BottomSheetWidget::request_dismiss`]/`stage_exit`/
//! `fire_staged_close` machinery, the same one every gesture dismissal
//! already takes, so an app dismiss plays the identical spring settle and can
//! carry a [`PopResult`] the same way [`ModalDismiss::dismiss_with`] does for
//! [`crate::side_sheet`].
//!
//! An app dismiss ignores [`BottomSheetView::dismissable`] the same way
//! [`mod@crate::overlay::modal`]'s own handle does (that flag exists to take
//! the *user's* exits away, and a `ModalDismiss` trigger is not one) — free
//! here, since [`BottomSheetWidget::request_dismiss`] already applies no such
//! gate itself; every *gesture* caller (the scrim tap, the handle-drag
//! release, `Escape`) gates it at its own call site instead.
//!
//! # Public API
//!
//! [`bottom_sheet`], [`show_bottom_sheet`], and every [`BottomSheetView`]
//! builder method except [`dismiss_handle`](BottomSheetView::dismiss_handle)
//! are unchanged from v1 — the staged exit above is wired through private
//! view/widget fields only, so this module's public surface is the two
//! functions plus [`BottomSheetView::on_dismiss`]/
//! [`BottomSheetView::dismissable`]/[`BottomSheetView::dismiss_handle`].

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::Role;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust::input::{TOUCH_SLOP, VelocityTracker};
use kurbo::{Point, Rect, RoundedRect, RoundedRectRadii, Shape, Size};
use peniko::{Brush, Color};

use frust::{
    AnimationController, BackPolicy, FrameTime, NavigatorController, PopResult, PushOptions,
    SpringDesc, TransitionSpec,
};

use super::press::presses;
use crate::overlay::modal::{ModalDismiss, StagedPop};
use crate::overlay::{OverlayContainer, container, on_surface_variant, radius, scrim};
use crate::tokens::MaterialSpring;

/// Drag-handle visual indicator width, in logical px (M3 token
/// `m3_comp_sheet_bottom_docked_drag_handle_width`).
const HANDLE_WIDTH: f64 = 32.0;
/// Drag-handle visual indicator height, in logical px (M3 token
/// `m3_comp_sheet_bottom_docked_drag_handle_height`).
const HANDLE_HEIGHT: f64 = 4.0;
/// Drag-handle interactive touch-target strip height, in logical px (M3
/// enforces a 48dp minimum touch target on the handle). The whole
/// full-width strip at the top of the sheet is the drag zone.
const HANDLE_TOUCH_TARGET: f64 = 48.0;
/// Flattening tolerance for the top-rounded panel path (a visually-lossless
/// value for on-screen corner radii — the [`crate::card`] precedent).
const PATH_TOLERANCE: f64 = 0.1;

/// Fraction of the sheet's own height a handle-drag must have closed past to
/// dismiss on release, when the release speed is below [`FLING_VELOCITY`]
/// (i.e. `progress < DRAG_DISMISS_FRACTION` dismisses,
/// `progress >= DRAG_DISMISS_FRACTION` springs back open) — see the [module
/// docs](self)' Interactive drag section.
///
/// **Community-approximate**: Material's bottom-sheet dismiss threshold is a
/// fling/drag heuristic with no single published constant; half the sheet
/// height is the conventional drag-past-the-midpoint commit point (the same
/// 0.5 the navigator's edge-swipe uses, and the same value
/// [`crate::overlay::modal`]'s own no-snap-points default reduces to).
const DRAG_DISMISS_FRACTION: f64 = 0.5;

/// Release speed (logical px/s along the drag axis) at or above which a
/// handle drag commits in the direction it was flung, overriding the
/// position-based decision above.
///
/// **Community-approximate**: Material publishes no fling threshold for a
/// sheet dismiss — mirrors [`crate::overlay::modal::OVERLAY_FLING_VELOCITY`],
/// an order of magnitude above `frust::input::FLING_STOP`'s 30 px/s "this
/// fling is over" floor.
const FLING_VELOCITY: f64 = 400.0;

/// Launch velocity (`progress`-units/sec, `0..=1` scale) for a spring leg
/// with no real pointer velocity behind it — the entrance, and every non-drag
/// dismiss (scrim tap, Escape, an Android back press) — and the floor a drag
/// release's own measured velocity is boosted to when it is too weak, or the
/// wrong sign, to read as a deliberate commit in the decided direction. The
/// same modest-kick convention `crate::button::motion::FLING_VELOCITY` uses
/// for its own `AnimationController::fling`.
const SPRING_KICK: f64 = 4.0;

/// How close to fully-closed a settled exit spring must have landed for the
/// staged pop to fire (`progress` units) — a sub-pixel slack on any sheet
/// shorter than a thousand logical px, so it reads as "arrived", never as a
/// spring stopped somewhere visible.
const SETTLED_EPSILON: f64 = 1e-3;

/// The spring driving the sheet's entrance, its dismiss, and a drag settle —
/// M3E's `expressiveSpatialDefault` preset
/// ([`MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`], stiffness 380 / damping
/// ratio 0.8): the same spatial-motion default this crate already uses
/// elsewhere for a position/shape change (`crate::loading_indicator`'s morph,
/// `crate::carousel`'s snap).
const SHEET_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.stiffness,
    damping_ratio: MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.damping_ratio,
};

/// Coerce a possibly-infinite constraint dimension to a finite value (a sheet
/// expects bounded constraints — a navigator page or a full-screen `Stack`).
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// Return `color` with its own alpha scaled by `factor` — how the scrim rides
/// the spring (`crate::overlay::modal`'s own `scale_alpha`, ported in rather
/// than shared: a two-line pure function, not worth a `pub(crate)` seam for).
fn scale_alpha(color: Color, factor: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], c[3] * factor])
}

/// A view-held, typed scrim/drag-dismiss callback (erased on build).
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;

/// The state-free navigator pop the exit spring stages, fired from `paint` on
/// settle — see the [module docs](self)' *Who owns the exit*. State-free is
/// what makes staging possible at all: `paint` carries no app state.
///
/// The [`StagedPop`] shared with [`crate::overlay::modal`] (the same convergence
/// as this module's theme accessors — one implementation of the guard, so both
/// hosts provably behave the same): it carries the stack identity the exit was
/// staged against and refuses a pop the stack moved out from under.
type OnClose = Rc<StagedPop>;

/// A declarative modal M3 bottom sheet wrapping a single content child. See the
/// [module docs](self).
pub struct BottomSheetView<State: 'static> {
    content: AnyView<State>,
    on_dismiss: Option<OnDismiss<State>>,
    dismissable: bool,
    /// The staged, state-free pop — wired internally by [`show_bottom_sheet`],
    /// never part of the public builder surface (see the [module docs](self)).
    on_close: Option<OnClose>,
    /// The shared back-press dismiss-signal cell (the `DismissAnimated` seam),
    /// wired internally by [`show_bottom_sheet`] alongside `on_close`.
    dismiss_signal: Option<Rc<Cell<u64>>>,
    /// The app-triggered staged-dismiss handle, installed by
    /// [`BottomSheetView::dismiss_handle`] — unlike `on_close`/
    /// `dismiss_signal`, never wired automatically; an app mints and installs
    /// it itself (see the [module docs](self)' app-initiated dismiss
    /// section).
    dismiss_handle: Option<ModalDismiss>,
}

/// Wrap `content` in a modal bottom sheet. Chain
/// [`BottomSheetView::on_dismiss`] to handle a dismiss gesture (usually a
/// `controller.pop()` — [`show_bottom_sheet`] wires this for you).
pub fn bottom_sheet<State: 'static, V: View<State>>(content: V) -> BottomSheetView<State> {
    BottomSheetView {
        content: any(content),
        on_dismiss: None,
        dismissable: true,
        on_close: None,
        dismiss_signal: None,
        dismiss_handle: None,
    }
}

impl<State: 'static> BottomSheetView<State> {
    /// Set the dismiss callback — invoked with `&mut State` on a scrim tap or a
    /// handle drag release decided closed. [`show_bottom_sheet`] wires this to
    /// `controller.pop()` automatically.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    /// Whether this sheet can be dismissed by the user at all — the scrim
    /// tap, the handle drag, `Escape`, and an Android back press (default
    /// `true`; see the [module docs](self)). `false` disables all four; only
    /// an explicit close control the app wires through its own content still
    /// dismisses it.
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Install the app-triggered staged-dismiss handle: the sheet's own
    /// content can then close it through the very spring exit the scrim tap,
    /// the handle drag, `Escape`, and an Android back press all take —
    /// instead of popping the navigator directly, which would drop the page
    /// out from under the sheet mid-slide. Reuses
    /// [`crate::overlay::modal::ModalDismiss`] — the same handle type
    /// [`crate::side_sheet::SideSheetView::dismiss_handle`] installs — so an
    /// app mixing sheet families never juggles two handle types. See the
    /// [module docs](self)' app-initiated dismiss section.
    ///
    /// Mint the handle *outside* [`show_bottom_sheet`]'s page builder and
    /// clone it in — the builder re-runs on every navigator rebuild.
    ///
    /// ```ignore
    /// let dismiss = ModalDismiss::new();
    /// let select = dismiss.clone();
    /// show_bottom_sheet(
    ///     &nav,
    ///     move || {
    ///         bottom_sheet(menu_panel(rows(), move |state: &mut State, sel| {
    ///             on_select(state, sel);
    ///             select.dismiss();
    ///         }))
    ///         .dismiss_handle(dismiss.clone())
    ///     },
    ///     |state: &mut State, result: PopResult| { /* ... */ },
    /// );
    /// ```
    pub fn dismiss_handle(mut self, dismiss: ModalDismiss) -> Self {
        self.dismiss_handle = Some(dismiss);
        self
    }
}

/// Push `build`'s sheet as a transparent navigator page and register
/// `on_result` for the value the sheet pops with. The sheet owns both halves of
/// its own spring motion ([module docs](self)' *Who owns the exit*), so the page
/// transition itself is [`TransitionSpec::NONE`] — a page-level transition on
/// top would animate the same slide twice, in both directions.
///
/// The sheet's dismiss (scrim tap + handle drag + Escape + Android back) is
/// wired to `controller.pop()` for you (dismiss pops with an *empty*
/// [`PopResult`], overriding any [`BottomSheetView::on_dismiss`] the builder
/// set).
///
/// The pop is wired **twice, by design** — as the state-free `on_close` hook
/// the exit spring fires on settle (the path every dismissal takes here), and
/// as the unstaged [`BottomSheetView::on_dismiss`] callback, which this path
/// only reaches if there were no spring to reverse. Exactly one of the two
/// fires per dismissal.
///
/// ```ignore
/// show_bottom_sheet(
///     &state.nav,
///     || bottom_sheet(sheet_content()),
///     |state: &mut State, result: PopResult| { /* ... */ },
/// );
/// ```
pub fn show_bottom_sheet<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> BottomSheetView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let dismiss_ctrl = controller.clone();
    // Built **once**, outside the page builder below, and only cloned into each
    // build: that builder re-runs on every navigator rebuild, and a fresh hook
    // per pass would throw away the stack identity an in-flight staged exit
    // armed (see [`StagedPop`]).
    let staged: OnClose = Rc::new(StagedPop::navigator_pop(controller));
    // Peeked once, at show-time: the back policy/dismiss-signal wiring is
    // fixed for the life of this pushed page (mirrors the navigator's
    // push-time `PushOptions` contract, and
    // `frust_glyph::dialog::show_glyph_dialog`'s identical peek), even
    // though `build` is re-invoked on every later
    // navigator rebuild to diff the page's content.
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
            let ctrl = dismiss_ctrl.clone();
            let mut view = build().on_dismiss(move |_state: &mut State| ctrl.pop());
            view.on_close = Some(staged.clone());
            view.dismiss_signal = widget_signal.clone();
            any::<State, _>(view)
        },
        options,
    );
}

/// The retained widget for a [`BottomSheetView`]. See the [module docs](self).
pub struct BottomSheetWidget {
    content: ChildPod,
    on_dismiss: Option<frust::authoring::ErasedCallback>,
    dismissable: bool,
    /// The state-free pop staged behind the exit spring, fired from `paint` on
    /// settle — `None` on a standalone mount, which keeps the immediate,
    /// state-touching `on_dismiss` instead (see the [module docs](self)' *Who
    /// owns the exit*).
    on_close: Option<OnClose>,
    /// The shared back-press dismiss-signal cell (the `DismissAnimated`
    /// seam) — see
    /// [`observe_dismiss_signal`](Self::observe_dismiss_signal).
    dismiss_signal: Option<Rc<Cell<u64>>>,
    /// The last generation observed from `dismiss_signal` (0 with no signal
    /// wired, or a `Veto`/non-dismissable sheet).
    last_seen_dismiss: u64,
    /// The app's own staged-dismiss handle, installed by
    /// [`BottomSheetView::dismiss_handle`] (see the [module docs](self)'
    /// app-initiated dismiss section).
    dismiss_handle: Option<ModalDismiss>,
    /// The last request generation observed from `dismiss_handle` — seeded
    /// from the handle's own generation at build, so a request raised before
    /// this widget existed is not aimed at it
    /// (`crate::overlay::modal::OverlayModalWidget`'s own `last_seen_request`
    /// field, mirrored).
    last_seen_request: u64,
    /// The payload a [`ModalDismiss::dismiss_with`] request armed, handed to
    /// the staged pop when the spring settles closed.
    ///
    /// Held here rather than left in the handle for the length of the exit
    /// so that a *refused* pop (the stack moved — see
    /// [`StagedPop::fire_with`]) keeps it: that dismissal delivered nothing,
    /// the sheet springs back open, and the payload rides whichever exit
    /// finally lands.
    pending_result: Option<PopResult>,
    /// Whether the running spring leg ends in a dismissal — set the moment a
    /// trigger stages one ([`Self::stage_exit`]), cleared by the settle that
    /// fires the pop and by every leg aimed back open: a drag that catches the
    /// closing panel, a release short of the dismiss threshold, a `Cancel`
    /// ([`Self::spring_open`]).
    exiting: bool,
    /// One-shot latch set the instant the staged `on_close` actually pops.
    /// Terminal for this widget's mounted lifetime: the hook only *enqueues* a
    /// navigator pop, so the sheet stays mounted (and still a modal barrier) for
    /// one or more frames after firing, during which a further dismiss trigger
    /// must not re-fire it — that would pop whatever page is now underneath.
    /// Nothing ever clears it back to `false`, which is exactly why a
    /// [`StagedPop`] that *refused* its pop must not set it: no page was
    /// removed, so latching would brick a still-live sheet instead of recording
    /// a finished dismissal.
    closed: bool,

    /// The content's measured size from the last real `layout` — cached so a
    /// drag scrub or a spring-advanced paint frame can reposition the panel
    /// without a full relayout (the panel's own extent never changes with
    /// `progress`, only its Y offset does).
    content_size: Size,
    /// The bottom-anchored panel rect in the widget's own local coordinate
    /// space, at its full (unclamped) extent — translated by `progress`, not
    /// resized by it (computed by [`Self::reposition`], read for scrim/handle
    /// hit-testing at event time).
    panel: Rect,
    /// The full-width 48dp drag-handle touch strip at the top of the panel
    /// (local coords, tracks the panel's own translation).
    handle_target: Rect,

    /// `0.0` fully closed/off-screen, `1.0` at rest (open) — may transiently
    /// read outside that range mid-spring (an under-damped spring's overshoot
    /// past its target is real, intended motion; see
    /// `crates/frust-core/src/anim.rs`'s `AnimationController` docs).
    progress: f64,
    /// The spring engine driving `progress` between legs — freshly re-seeded
    /// at the start of every new leg ([`Self::spring_to`]) from whatever
    /// `progress` currently reads, and left untouched (not advanced) while a
    /// drag is scrubbing `progress` directly.
    anim: AnimationController,
    /// Whether the first paint has kicked off the entrance spring.
    started: bool,

    /// A handle-strip press is in flight (the sheet captured the pointer on a
    /// `Down` in the handle strip).
    drag_captured: bool,
    /// The `Down` y the drag distance is measured from.
    drag_start_y: f64,
    /// `progress` at the start of this drag — the scrub's own baseline.
    drag_from: f64,
    /// Whether the gesture has passed [`TOUCH_SLOP`] into a real drag; below
    /// it a release is a tap and nothing dismisses (unchanged from v1).
    drag_moved: bool,
    /// The release-velocity estimator, clocked from the last painted frame —
    /// pointer events carry no timestamp of their own (`frust_widgets::scroll`'s
    /// precedent, also `crate::overlay::modal`'s).
    tracker: VelocityTracker,
    last_frame_time: FrameTime,

    /// A scrim/panel-background press is in flight (the modal barrier).
    scrim_captured: bool,
    /// Whether that press started outside the panel (only an outside press
    /// released outside dismisses).
    scrim_down_outside: bool,
}

impl<State: 'static> View<State> for BottomSheetView<State> {
    type Element = BottomSheetWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> BottomSheetWidget {
        BottomSheetWidget {
            content: frust::authoring::build_child(&self.content, ctx),
            on_dismiss: self
                .on_dismiss
                .as_ref()
                .map(frust::authoring::erase_callback),
            dismissable: self.dismissable,
            on_close: self.on_close.clone(),
            dismiss_signal: self.dismiss_signal.clone(),
            // Seeded from the cell's *current* generation: a bump that predates
            // this widget is not a back press aimed at it.
            last_seen_dismiss: self.dismiss_signal.as_ref().map_or(0, |sig| sig.get()),
            dismiss_handle: self.dismiss_handle.clone(),
            // Seeded from the handle's *current* generation, the same rule as
            // `last_seen_dismiss` above: a request raised before this widget
            // existed is not aimed at it.
            last_seen_request: self
                .dismiss_handle
                .as_ref()
                .map_or(0, ModalDismiss::generation),
            pending_result: None,
            exiting: false,
            closed: false,
            content_size: Size::ZERO,
            panel: Rect::ZERO,
            handle_target: Rect::ZERO,
            progress: 0.0,
            anim: AnimationController::new(Duration::ZERO),
            started: false,
            drag_captured: false,
            drag_start_y: 0.0,
            drag_from: 0.0,
            drag_moved: false,
            tracker: VelocityTracker::new(),
            last_frame_time: FrameTime::ZERO,
            scrim_captured: false,
            scrim_down_outside: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BottomSheetWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let flags = frust::authoring::rebuild_child(
            &prev.content,
            &self.content,
            &mut element.content,
            ctx,
        );
        // Closures aren't comparable — reinstall the dismiss adapter cheaply.
        element.on_dismiss = self
            .on_dismiss
            .as_ref()
            .map(frust::authoring::erase_callback);
        element.dismissable = self.dismissable;
        // Both staging seams are reinstalled the same cheap way: the close
        // hook is a closure (never comparable), and the dismiss-signal cell's
        // identity is fixed at push time (see `show_bottom_sheet`), so
        // reinstalling it here never disturbs `last_seen_dismiss` — nor do
        // either of them disturb the in-flight `exiting`/`closed` state.
        element.on_close = self.on_close.clone();
        element.dismiss_signal = self.dismiss_signal.clone();
        // The app's dismiss handle follows the view whenever the view still
        // carries one, gated on *identity* — exactly
        // `crate::overlay::modal::OverlayModalView::rebuild`'s own re-seed: a
        // view handing back the same cell keeps the live request generation
        // (a button's own pending tap must not be forgotten), while a view
        // that stopped carrying one leaves the installed handle standing,
        // the same pinning `on_close`/`dismiss_signal` get above.
        if let Some(handle) = &self.dismiss_handle
            && !element
                .dismiss_handle
                .as_ref()
                .is_some_and(|installed| installed.same_cell(handle))
        {
            element.last_seen_request = handle.generation();
            element.dismiss_handle = Some(handle.clone());
        }
        flags
    }

    fn teardown(&self, element: &mut BottomSheetWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.content, &mut element.content, ctx);
    }
}

impl BottomSheetWidget {
    /// Observe the shared back-press dismiss-signal cell (see the
    /// `dismiss_signal` field docs) and stage the exit exactly once per bump —
    /// the `BackPolicy::DismissAnimated` seam's widget-side half
    /// (`nav::navigator::BackPolicy`'s documented observation seam).
    ///
    /// The paint-pass twin of [`Self::request_dismiss`]: identical guards, and
    /// the same closing spring, so an Android back press animates out exactly
    /// like a scrim tap. It requests the frame the fresh spring leg needs; the
    /// pop itself is fired later, by [`Self::fire_staged_close`] on settle.
    fn observe_dismiss_signal(&mut self, ctx: &mut PaintCtx) {
        let Some(signal) = &self.dismiss_signal else {
            return;
        };
        let current = signal.get();
        if current == self.last_seen_dismiss {
            return;
        }
        self.last_seen_dismiss = current;
        if self.exiting || self.closed || self.on_close.is_none() {
            return;
        }
        self.stage_exit(-SPRING_KICK);
        ctx.request_frame();
    }

    /// Latch the app's [`ModalDismiss`] handle's request generation, taking
    /// any payload it carries — `true` when a request this widget has not
    /// seen before is pending.
    ///
    /// Mirrors `crate::overlay::modal::OverlayModalWidget`'s own
    /// `take_dismiss_request` exactly: the payload is taken here, at
    /// observation, rather than at the settle that consumes it, since the
    /// handle is the app's and may be re-armed for a later presentation,
    /// while this exit's payload belongs to this exit.
    fn take_dismiss_request(&mut self) -> bool {
        let Some(handle) = &self.dismiss_handle else {
            return false;
        };
        let current = handle.generation();
        if current == self.last_seen_request {
            return false;
        }
        self.last_seen_request = current;
        if let Some(result) = handle.take_result() {
            self.pending_result = Some(result);
        }
        true
    }

    /// Observe the app's [`ModalDismiss`] handle in the *event* pass, right
    /// after the content routing that most likely raised the request (a row
    /// tap, an action button), so an app dismiss stages in the very pass that
    /// asked for it.
    ///
    /// Routed straight through [`Self::request_dismiss`] — the same staged
    /// path every gesture dismissal already takes, never a second staging
    /// core. `request_dismiss` itself applies no `dismissable` gate (every
    /// *gesture* caller gates it at its own call site instead), so an app
    /// dismiss is correctly ungated here for free — matching
    /// [`mod@crate::overlay::modal`]'s own documented rule that an app
    /// dismiss ignores [`BottomSheetView::dismissable`].
    fn observe_dismiss_request(&mut self, ctx: &mut EventCtx) {
        if !self.take_dismiss_request() {
            return;
        }
        self.request_dismiss(ctx, -SPRING_KICK);
    }

    /// The paint-pass twin of [`Self::observe_dismiss_request`] — the
    /// backstop for a request raised outside an event (an async completion, a
    /// timer). Mirrors [`Self::observe_dismiss_signal`]'s shape exactly,
    /// keyed off the handle's own generation instead of the back-press cell.
    ///
    /// No unstaged fallback exists here, by design: `PaintCtx` carries no app
    /// state to invoke [`BottomSheetView::on_dismiss`] with — the same gap
    /// [`mod@crate::overlay::modal`]'s own paint-pass twin documents (its
    /// `Unstaged` arm only ever reaches the state-free `on_close`). A request
    /// raised here with no `on_close` wired (a `Stack` mount with no
    /// navigator behind it) is silently dropped.
    fn observe_paint_dismiss_request(&mut self, ctx: &mut PaintCtx) {
        if !self.take_dismiss_request() {
            return;
        }
        if self.exiting || self.closed || self.on_close.is_none() {
            return;
        }
        self.stage_exit(-SPRING_KICK);
        ctx.request_frame();
    }

    /// Fire the staged, state-free pop once the closing spring has settled at
    /// the fully-closed end — the one place the navigator path's dismissal
    /// actually happens (see the [module docs](self)' *Who owns the exit*).
    ///
    /// **Scheduling the draining frame.** `on_close` only *enqueues* a
    /// `NavOp::Pop` on the controller; it writes no tracked reactive signal, so
    /// nothing else schedules the frame that drains it. The paint that fires it
    /// therefore must itself request the next frame ([`PaintCtx::request_frame`])
    /// — exactly as `frust_glyph::dialog` requests a frame past its `Dismissed`
    /// phase so the rebuild that applies the pop runs. Without this, a
    /// dirty-driven desktop shell (`ControlFlow::Wait`) idles and the mobile
    /// frame gate `Skip`s the next tick, so the enqueued pop never drains until
    /// an unrelated later frame.
    ///
    /// **The refused pop.** The hook may decline to pop at all, when the stack
    /// moved out from under the staged exit (see [`StagedPop`]) — a page pushed
    /// while the ramp ran would otherwise be popped in this sheet's place. A
    /// refusal must not latch [`Self::closed`]: that latch is terminal, and
    /// setting it over a page this widget never removed would leave an
    /// invisible, permanently undismissable barrier. The sheet springs back
    /// open instead (`reduce_motion` snaps it), and a later dismiss stages a
    /// fresh, correctly-armed exit.
    ///
    /// **The carried payload.** [`Self::pending_result`], armed by a
    /// [`ModalDismiss::dismiss_with`] request, rides this fire via
    /// [`StagedPop::fire_with`] — taken only when the pop actually happens,
    /// so a refused pop leaves it for whichever exit eventually lands.
    fn fire_staged_close(&mut self, ctx: &mut PaintCtx, reduce: bool) {
        if !self.exiting || self.anim.is_animating() || self.progress > SETTLED_EPSILON {
            return;
        }
        self.exiting = false;
        self.progress = 0.0;
        // Cloned out of `self` so the recovery arm below can take `&mut self`;
        // it is an `Rc`, and this runs once per staged exit.
        let Some(on_close) = self.on_close.clone() else {
            return;
        };
        if on_close.fire_with(&mut self.pending_result) {
            self.closed = true;
            // Guarantee the enqueued pop one draining rebuild — see the
            // `Scheduling the draining frame` note above.
            ctx.request_frame();
            return;
        }
        if reduce {
            self.progress = 1.0;
        } else {
            self.spring_open(SPRING_KICK);
        }
        ctx.request_frame();
    }

    /// Start a fresh spring leg, launched with `velocity`
    /// (`progress`-units/sec — positive opens, negative closes; see
    /// [`AnimationController::fling`]), always picking up from wherever
    /// `progress` currently reads — even when that was just set directly by a
    /// drag scrub rather than by this engine's own last leg.
    ///
    /// `AnimationController` exposes no direct value setter; a zero-duration
    /// `animate_to` is the sanctioned snap instead — with `duration <= 0.0`,
    /// `start_duration` assigns the value immediately, with no motion in
    /// between (`crates/frust-core/src/anim.rs`).
    fn spring_to(&mut self, velocity: f64) {
        let mut seed = AnimationController::new(Duration::ZERO);
        seed.animate_to(self.progress);
        self.anim = seed;
        self.anim.fling(velocity, SHEET_SPRING);
    }

    /// Launch a fresh leg aimed back **open** (`velocity` positive), cancelling
    /// any staged exit it overtakes.
    ///
    /// Clearing `exiting` is the point: a leg targeting `1.0` is by definition
    /// not a dismissal, and a staged exit left flagged across one would settle at
    /// rest where [`Self::fire_staged_close`] can never fire it, while
    /// [`Self::request_dismiss`] and [`Self::observe_dismiss_signal`] both
    /// hard-return on the flag — every later scrim tap, `Escape` and back press
    /// dead, with the sheet still a modal barrier.
    fn spring_open(&mut self, velocity: f64) {
        self.exiting = false;
        self.spring_to(velocity);
    }

    /// Stage an exit: launch the closing leg at `velocity`, flag it, and arm the
    /// staged pop against the navigator stack as it stands right now (see
    /// [`StagedPop`]). The one choke point every staging path goes through.
    fn stage_exit(&mut self, velocity: f64) {
        self.spring_to(velocity);
        self.exiting = true;
        if let Some(on_close) = &self.on_close {
            on_close.arm();
        }
    }

    /// Take a dismiss trigger (scrim tap, Escape, a handle-drag release
    /// decided closed, or — via [`Self::observe_dismiss_request`] — the app's
    /// own [`ModalDismiss`] handle): kick the closing spring at `velocity`,
    /// then either **stage** the navigator pop behind it (the
    /// [`show_bottom_sheet`] path, which wires a state-free `on_close`) or
    /// fire the state-touching [`BottomSheetView::on_dismiss`] on the spot (a
    /// standalone mount, which has nothing to defer into) — see the [module
    /// docs](self)' *Who owns the exit*.
    ///
    /// A trigger arriving while a staged exit is already running, or after its
    /// pop has fired ([`Self::closed`]), is a no-op: the sheet stays mounted for
    /// a frame or more past the enqueued pop, and a second pop there would take
    /// the page underneath with it — the same no-op that makes an app dismiss
    /// through the handle idempotent (a second `dismiss()` mid-exit changes
    /// nothing here, on top of [`Self::take_dismiss_request`]'s own
    /// generation latch already refusing to observe a request twice).
    ///
    /// No `dismissable` gate lives here — every *gesture* caller applies it at
    /// its own call site, and [`Self::observe_dismiss_request`] deliberately
    /// does not, so an app dismiss reaches this unconditionally (see the
    /// [module docs](self)' app-initiated dismiss section).
    fn request_dismiss(&mut self, ctx: &mut EventCtx, velocity: f64) {
        if self.exiting || self.closed {
            return;
        }
        if self.on_close.is_some() {
            self.stage_exit(velocity);
            // The spring's own frames come from `paint`, but only once one runs
            // — ask for that first one here.
            ctx.request_redraw();
            return;
        }
        self.spring_to(velocity);
        if let Some(on_dismiss) = self.on_dismiss.as_mut() {
            on_dismiss(ctx);
        }
    }

    /// The last painted frame time in milliseconds — the event pass's clock,
    /// since a pointer event carries none.
    fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    /// Recompute the panel/handle-strip rects and the content's origin for the
    /// current `progress`, without a relayout — the panel's own extent (fixed
    /// by the last real layout) never changes with `progress`, only its Y
    /// offset does. Used by `layout`, by every spring-advanced `paint` frame,
    /// and by the drag scrub (`event`, which has no `request_layout` to reach
    /// for — `EventCtx` carries none).
    fn reposition(&mut self, area: Size) {
        let panel_h = (HANDLE_TOUCH_TARGET + self.content_size.height).min(area.height);
        let panel_y = area.height - panel_h * self.progress;
        self.panel =
            Rect::from_origin_size(Point::new(0.0, panel_y), Size::new(area.width, panel_h));
        self.handle_target = Rect::new(0.0, panel_y, area.width, panel_y + HANDLE_TOUCH_TARGET);
        let content_x = ((area.width - self.content_size.width) / 2.0).max(0.0);
        self.content
            .set_origin(Point::new(content_x, panel_y + HANDLE_TOUCH_TARGET));
    }
}

impl Widget for BottomSheetWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area = Size::new(
            finite_or_zero(bc.max().width),
            finite_or_zero(bc.max().height),
        );

        // Content lays out full-width, below the handle strip, in the room left
        // under it.
        let content_max_h = (area.height - HANDLE_TOUCH_TARGET).max(0.0);
        let content_bc = BoxConstraints::loose(Size::new(area.width, content_max_h));
        self.content_size = self.content.layout_child(ctx, &content_bc);

        self.reposition(area);

        bc.constrain(area)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The back-press seam, first: a bump staged here rides the very same
        // spring the pointer/key gestures below drive. The app's own handle
        // is observed alongside it, as the backstop for a request raised
        // outside an event pass (`mod@crate::overlay::modal`'s own paint
        // ordering, mirrored).
        self.observe_dismiss_signal(ctx);
        self.observe_paint_dismiss_request(ctx);
        self.last_frame_time = ctx.frame_time();
        // Every theme read in one scope: `Theme::from_paint_ctx` ties its
        // returned reference to *this* borrow of `ctx`, and the frame-request/
        // reposition calls below need `ctx`/`self` mutably (the
        // `crate::overlay::modal` precedent for the same shape).
        let (reduce, panel_radius, scrim_color, container_color, handle_color) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                radius(theme),
                scrim(theme),
                container(theme, OverlayContainer::Low),
                on_surface_variant(theme),
            )
        };

        if !self.started {
            self.started = true;
            if reduce {
                self.progress = 1.0;
            } else if !self.exiting {
                // …unless a dismiss beat the first paint here (nothing has been
                // shown yet, so there is nothing to slide *in*): re-kicking the
                // entrance would fight the exit leg already running and strand
                // a staged pop that only fires at the closed end.
                self.spring_to(SPRING_KICK);
            }
        }
        if reduce {
            if self.anim.is_animating() {
                self.anim.stop();
            }
            // `reduce_motion` collapses the staged exit rather than skipping
            // it: the panel lands closed on this frame and the pop fires just
            // below, instead of waiting out a spring that will never run.
            if self.exiting {
                self.progress = 0.0;
            }
        } else if self.anim.is_animating() {
            if self.anim.advance(self.last_frame_time) {
                ctx.request_frame();
            }
            self.progress = self.anim.value();
        }
        // A settled exit is where the staged pop is finally fired.
        self.fire_staged_close(ctx, reduce);
        // Keep the geometry in step with whatever `progress` this frame
        // settled on — the spring above (and `observe_dismiss_signal`) only
        // ever touches `progress` itself, so re-derive the panel/handle/
        // content placement here rather than waiting a frame for the next
        // `layout`.
        self.reposition(ctx.size());

        // Scrim over the whole area, fading with the panel's own progress.
        scene.fill_rect(
            ctx.origin(),
            ctx.size(),
            scale_alpha(scrim_color, self.progress.clamp(0.0, 1.0) as f32),
        );

        // Panel: full-width, bottom-anchored, top corners rounded and bottom
        // corners square (the bottom sits at the screen edge). No stroked/
        // per-corner rounded-rect primitive exists on `PaintScene`, so build the
        // path with `kurbo` and fill it — the `card` module's precedent.
        let panel_local = Rect::new(self.panel.x0, self.panel.y0, self.panel.x1, self.panel.y1);
        let radii = RoundedRectRadii::new(panel_radius, panel_radius, 0.0, 0.0);
        let path = RoundedRect::from_rect(panel_local, radii).to_path(PATH_TOLERANCE);
        scene.fill_path(ctx.origin(), &path, &Brush::Solid(container_color));

        // Drag handle: 32×4dp indicator, centered horizontally within the top
        // strip, with fully-rounded ends.
        let handle_x = self.panel.x0 + (self.panel.width() - HANDLE_WIDTH) / 2.0;
        let handle_y = self.panel.y0 + (HANDLE_TOUCH_TARGET - HANDLE_HEIGHT) / 2.0;
        scene.fill_rounded_rect(
            Point::new(ctx.origin().x + handle_x, ctx.origin().y + handle_y),
            Size::new(HANDLE_WIDTH, HANDLE_HEIGHT),
            HANDLE_HEIGHT / 2.0,
            handle_color,
        );

        self.content.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // 0. A broadcast is not user input, so it belongs to neither the drag nor
        //    the scrim arm below: forward it to the content unconditionally and
        //    consume nothing, so a deferred callback queued inside the sheet still
        //    flushes while a finger is down.
        if event.is_broadcast() {
            frust::authoring::route_event_single(&mut self.content, ctx, event);
            return EventResult::Ignored;
        }
        // 1. An in-flight handle drag owns the pointer stream.
        if self.drag_captured {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Handled;
            };
            match p.phase {
                PointerPhase::Move => {
                    self.tracker.record(self.event_time_ms(), p.position.y);
                    let dy = p.position.y - self.drag_start_y;
                    if !self.drag_moved && dy.abs() > TOUCH_SLOP {
                        self.drag_moved = true;
                    }
                    if self.drag_moved {
                        let extent = self.panel.height().max(1.0);
                        let target = (self.drag_from - dy / extent).clamp(0.0, 1.0);
                        self.progress = target;
                        // A live drag takes the panel over from whatever leg was
                        // running, a staged exit included: catching a closing
                        // sheet cancels its dismissal, and the release below
                        // decides again from where the finger left it. (Once the
                        // pop has actually fired, `closed` is terminal and no
                        // drag resurrects the sheet.)
                        self.exiting = false;
                        self.reposition(ctx.size());
                        ctx.request_redraw();
                    }
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    self.drag_captured = false;
                    if !self.drag_moved {
                        // A tap on the handle with no real movement: unchanged
                        // from v1 — nothing dismisses (only a real drag past
                        // the threshold, or a fling, can).
                        return EventResult::Handled;
                    }
                    let velocity = self.tracker.velocity();
                    let extent = self.panel.height().max(1.0);
                    let stay_open = if velocity.abs() >= FLING_VELOCITY {
                        // A flick toward the closing edge (positive velocity,
                        // moving down) commits closed; away from it, open.
                        velocity < 0.0
                    } else {
                        self.progress >= DRAG_DISMISS_FRACTION
                    };
                    // Reparametrize the release speed (px/s along the drag
                    // axis) into `progress`-units/sec, then clamp its sign to
                    // match the decided direction — a raw reading pointing
                    // the other way (a slow release with a stray twitch) must
                    // not fling the spring toward the wrong target, since
                    // `fling`'s own target is derived purely from the sign it
                    // is handed.
                    let raw = -velocity / extent;
                    if stay_open || !self.dismissable {
                        // `dismissable(false)`: a past-threshold drag springs
                        // back open instead of firing — see the module docs.
                        // `spring_open` (not `spring_to`) because a back press
                        // may have staged an exit *under* this live drag: this
                        // leg overrules it, and a staged exit left flagged here
                        // would settle at rest and refuse every later dismiss.
                        self.spring_open(raw.max(SPRING_KICK));
                    } else {
                        self.request_dismiss(ctx, raw.min(-SPRING_KICK));
                    }
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    // A `Cancel` arm never touches state — clear the drag flag
                    // and, if it had actually moved, spring back open (a leg
                    // that also cancels any exit staged under the drag, exactly
                    // as the `Up` reopening leg above does; an Android back
                    // *gesture* delivers this `Cancel` itself).
                    self.drag_captured = false;
                    if self.drag_moved {
                        self.spring_open(SPRING_KICK);
                    }
                    EventResult::Handled
                }
                PointerPhase::Down => EventResult::Handled,
            }
        }
        // 2. An in-flight scrim/panel-background press owns the stream.
        else if self.scrim_captured {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Handled;
            };
            match p.phase {
                PointerPhase::Up => {
                    let released_outside = !self.panel.contains(p.position);
                    if self.dismissable && self.scrim_down_outside && released_outside {
                        self.request_dismiss(ctx, -SPRING_KICK);
                    }
                    self.scrim_captured = false;
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    self.scrim_captured = false;
                    EventResult::Handled
                }
                _ => EventResult::Handled,
            }
        }
        // 3. Fresh events.
        else {
            let InputEvent::Pointer(p) = event else {
                // Escape (when the sheet itself, not deeper content, holds the
                // focus path) dismisses through the same path as a scrim/drag
                // dismiss — content gets first crack at it if it holds the
                // deeper focus path (mirrors an action's own routing
                // precedence on `material::dialog`).
                if let InputEvent::Key(key_event) = event
                    && self.dismissable
                    && key_event.key == Key::Named(NamedKey::Escape)
                    && !self.content.is_focused()
                    && self.on_dismiss.is_some()
                {
                    self.request_dismiss(ctx, -SPRING_KICK);
                    return EventResult::Handled;
                }
                // Everything else focus-routed goes to the content.
                let result = frust::authoring::route_event_single(&mut self.content, ctx, event);
                // The content may have just fired the app's own `ModalDismiss`
                // handle (a keyboard-driven action) — stage it in this pass.
                self.observe_dismiss_request(ctx);
                return result;
            };
            match p.phase {
                PointerPhase::Down => {
                    // Claim focus on every Down anywhere in the sheet.
                    // Load-bearing: the root treats a Down that bubbles no
                    // claim as a blur (`release_focus_session` drops focus +
                    // IME state), so the re-claim is what keeps the session
                    // alive while this sheet is up. Re-claiming while already
                    // focused is a change-guarded no-op (no generation bump)
                    // — do not add a claim-once guard, it kills the session
                    // on the second tap (claim-once-hygiene review,
                    // 2026-08-06).
                    ctx.request_focus();
                    // A Down in the handle strip begins a drag — primary only:
                    // a secondary press drags nothing.
                    if presses(p) && self.handle_target.contains(p.position) {
                        self.drag_captured = true;
                        self.drag_start_y = p.position.y;
                        self.drag_from = self.progress;
                        self.drag_moved = false;
                        self.tracker.clear();
                        self.tracker.record(self.event_time_ms(), p.position.y);
                        ctx.capture_pointer();
                        return EventResult::Handled;
                    }
                    // Otherwise give the content a chance to consume it.
                    if frust::authoring::route_event_single(&mut self.content, ctx, event)
                        == EventResult::Handled
                    {
                        // An action the content just ran (e.g. a row that
                        // fires on `Down`) may have asked to dismiss through
                        // the app's own handle: stage it in this very pass.
                        self.observe_dismiss_request(ctx);
                        return EventResult::Handled;
                    }
                    // An invisible sheet absorbs nothing — the anchored
                    // overlay host's own closed-state gate, ported: at
                    // `progress == 0` the scrim is fully transparent and the
                    // panel sits off-screen, and a widget still mounted behind
                    // an enqueued pop (or one whose entrance has not drawn its
                    // first frame) paints exactly that. A barrier that swallows
                    // what it does not show is an invisible input trap.
                    if self.progress <= SETTLED_EPSILON {
                        return EventResult::Ignored;
                    }
                    // Falls to the modal barrier: swallow, arming a scrim dismiss
                    // when the press started outside the panel. A secondary
                    // press is swallowed the same way but arms nothing, so a
                    // right-click can never dismiss the sheet.
                    if !presses(p) {
                        return EventResult::Handled;
                    }
                    self.scrim_captured = true;
                    self.scrim_down_outside = !self.panel.contains(p.position);
                    ctx.capture_pointer();
                    EventResult::Handled
                }
                // A captured content child keeps receiving Move/Up/Cancel via the
                // route helper's active-path fast lane.
                _ => {
                    let result =
                        frust::authoring::route_event_single(&mut self.content, ctx, event);
                    // Fire-on-up-inside (`docs/CODE_STANDARDS.md`'s Interaction
                    // Semantics) means a row's own action most often lands on
                    // this very `Up` — observe the app's handle right after,
                    // the same pass the content just ran in.
                    self.observe_dismiss_request(ctx);
                    result
                }
            }
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

    frust::authoring::visit_children!(content);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::NavigatorView;
    use frust::TransitionSpec;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent, any as core_any,
    };
    use frust_core::RenderRoot;
    use frust_widgets::navigator;
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;

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

    fn ft(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    fn build(view: &BottomSheetView<()>) -> BottomSheetWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    /// Records filled rects, rounded rects, and filled paths (as a brush color
    /// plus the path's window-space bounding box).
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        paths: Vec<Color>,
        path_boxes: Vec<Rect>,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn fill_path(&mut self, origin: Point, path: &kurbo::BezPath, brush: &Brush) {
            if let Brush::Solid(color) = brush {
                self.paths.push(*color);
            }
            let b = path.bounding_box();
            self.path_boxes.push(Rect::new(
                b.x0 + origin.x,
                b.y0 + origin.y,
                b.x1 + origin.x,
                b.y1 + origin.y,
            ));
        }
    }
    impl Recorder {
        /// The window-space top edge of the sheet panel this frame — the panel
        /// is the only filled *path* either navigator test below paints, so its
        /// bounding box is the panel's own rect, and its `y0` is what the exit
        /// and entrance ramps move.
        fn panel_top(&self) -> Option<f64> {
            self.path_boxes.first().map(|b| b.y0)
        }
    }

    /// Drive `w` through a generous, bounded schedule of `paint` calls (each
    /// seeded with an advancing [`FrameTime`] via [`PaintCtx::for_test`])
    /// until its spring settles — every test below that needs the sheet at
    /// rest calls this first, since `layout`/`paint` position the panel by
    /// `progress`, and `progress` only reaches its target once enough spring
    /// frames have run (the [module docs](super)' "first frame is invisible"
    /// contract).
    fn settle(w: &mut BottomSheetWidget, area: Size) {
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(area));
        for i in 0..40u64 {
            let mut pctx = PaintCtx::for_test(Point::ZERO, area, ft(i * 20));
            w.paint(&mut pctx, &mut Recorder::default());
            if !w.anim.is_animating() {
                return;
            }
        }
        panic!("sheet spring failed to settle within the test bound");
    }

    /// Advance `w`'s own clock by `delta_ms` from wherever it currently sits
    /// (`w.last_frame_time`, most recently stamped by `settle`) via a no-op
    /// `paint` — the velocity tests below need at least one clock tick
    /// between a `Down` and a `Move` for `VelocityTracker` to read a real,
    /// non-zero speed (a pointer event carries no timestamp of its own, and a
    /// hardcoded absolute `FrameTime` here would regress *behind*
    /// `settle`'s own advancing clock, corrupting the tracker's window).
    fn tick(w: &mut BottomSheetWidget, area: Size, delta_ms: u64) {
        let next_ms = w.last_frame_time.as_nanos() / 1_000_000 + delta_ms;
        let mut pctx = PaintCtx::for_test(Point::ZERO, area, ft(next_ms));
        w.paint(&mut pctx, &mut Recorder::default());
    }

    #[test]
    fn layout_starts_fully_offscreen_before_the_entrance_settles() {
        let view: BottomSheetView<()> = bottom_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let area = Size::new(400.0, 600.0);
        let size = w.layout(&mut lctx, &BoxConstraints::tight(area));
        assert_eq!(size, area, "the sheet fills the whole area (scrim)");
        // `progress` starts at 0.0 (build default) — the panel sits fully
        // below the fold until the first `paint` kicks the entrance spring.
        assert_eq!(w.panel.y0, area.height);
    }

    #[test]
    fn layout_anchors_panel_to_bottom_with_handle_strip_once_settled() {
        let view: BottomSheetView<()> = bottom_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let area = Size::new(400.0, 600.0);
        settle(&mut w, area);

        // Panel: full width, bottom-anchored, sized to content + handle strip.
        assert_eq!(w.panel.x0, 0.0);
        assert_eq!(w.panel.x1, area.width);
        assert!(
            (w.panel.y1 - area.height).abs() < 1e-6,
            "panel bottom is the screen edge once at rest"
        );
        assert!((w.panel.height() - (HANDLE_TOUCH_TARGET + 200.0)).abs() < 1e-6);
        // The handle strip is the top 48dp of the panel.
        assert_eq!(w.handle_target.y0, w.panel.y0);
        assert_eq!(w.handle_target.height(), HANDLE_TOUCH_TARGET);
        // Content sits below the handle strip.
        assert!((w.content.origin().y - (w.panel.y0 + HANDLE_TOUCH_TARGET)).abs() < 1e-6);
    }

    #[test]
    fn unthemed_paint_draws_scrim_panel_and_handle_once_settled() {
        let view: BottomSheetView<()> = bottom_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let area = Size::new(400.0, 600.0);
        settle(&mut w, area);

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, area, ft(1000));
        w.paint(&mut pctx, &mut rec);

        // Scrim: full-area fill at 32% of the fallback scrim, fully faded in
        // now that the sheet is at rest.
        assert_eq!(rec.rects[0].1, area);
        assert_eq!(rec.rects[0].2, scrim(None));
        // Panel: a filled top-rounded path in the fallback container color.
        assert_eq!(rec.paths, vec![container(None, OverlayContainer::Low)]);
        // Handle: a 32×4 rounded indicator in the fallback handle color.
        let handle = rec
            .rrects
            .iter()
            .find(|(_, s, _, _)| *s == Size::new(HANDLE_WIDTH, HANDLE_HEIGHT))
            .expect("the drag handle is painted");
        assert_eq!(handle.3, on_surface_variant(None));
    }

    #[test]
    fn themed_paint_resolves_tokens_once_settled() {
        let theme = crate::baseline();
        let scheme = theme.scheme();
        let view: BottomSheetView<()> = bottom_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let area = Size::new(400.0, 600.0);
        settle(&mut w, area);

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, area, ft(1000)).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(rec.rects[0].2, scrim(Some(&theme)));
        assert_eq!(rec.paths, vec![scheme.surface_container_low]);
        let handle = rec
            .rrects
            .iter()
            .find(|(_, s, _, _)| *s == Size::new(HANDLE_WIDTH, HANDLE_HEIGHT))
            .expect("the drag handle is painted");
        assert_eq!(handle.3, scheme.on_surface_variant);
    }

    // --- Spring entrance/exit pinned. ---

    #[test]
    fn the_entrance_progresses_smoothly_rather_than_jumping() {
        let view: BottomSheetView<()> = bottom_sheet(leaf_any(300.0, 200.0));
        let mut w = build(&view);
        let area = Size::new(400.0, 600.0);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(area));

        // First paint: seeds the spring, but only seeds the clock too (zero
        // delta) — progress stays exactly where it started.
        let mut pctx = PaintCtx::for_test(Point::ZERO, area, ft(0));
        w.paint(&mut pctx, &mut Recorder::default());
        assert_eq!(w.progress, 0.0, "the first frame is invisible");
        assert!(w.anim.is_animating());

        // A handful of frames in, the panel has moved but not yet arrived —
        // a real (spring) ramp, not an instant snap.
        let mut pctx = PaintCtx::for_test(Point::ZERO, area, ft(80));
        w.paint(&mut pctx, &mut Recorder::default());
        assert!(
            w.progress > 0.0 && w.progress < 1.2,
            "mid-flight progress {} should sit between the endpoints (allowing \
             a modest expressive overshoot)",
            w.progress
        );

        settle(&mut w, area);
        assert!((w.progress - 1.0).abs() < 1e-6, "settles exactly at rest");
    }

    #[test]
    fn a_dismiss_trigger_springs_the_panel_closed_over_several_frames() {
        let view: BottomSheetView<Flag> =
            bottom_sheet(leaf_any_flag(300.0, 200.0)).on_dismiss(|s: &mut Flag| s.dismissed += 1);
        let area = Size::new(400.0, 600.0);
        let mut counter = 0u64;
        let mut w = View::<Flag>::build(&view, &mut BuildCtx::new(&mut counter));
        settle(&mut w, area);
        assert_eq!(w.progress, 1.0);

        // A scrim tap outside the panel dismisses immediately (state-touching,
        // per the module docs) but also kicks a real closing spring — the
        // widget doesn't just vanish or snap to 0 on the spot.
        let mut state = Flag::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 1);
        // The closing spring is kicked (`self.anim`), but `progress` itself
        // only moves once a `paint` advances it — the same "first frame is
        // invisible" contract the entrance carries (`event` has no clock of
        // its own to advance against).
        assert!(w.anim.is_animating());
        assert_eq!(w.progress, 1.0, "progress hasn't moved yet — no paint ran");

        // Left mounted (the standalone/`Stack` use case), it settles fully
        // closed rather than snapping — and visibly moves along the way.
        let mut saw_partial_progress = false;
        for i in 0..40u64 {
            let mut pctx = PaintCtx::for_test(Point::ZERO, area, ft(i * 20));
            w.paint(&mut pctx, &mut Recorder::default());
            if w.progress > 1e-3 && w.progress < 1.0 - 1e-3 {
                saw_partial_progress = true;
            }
            if !w.anim.is_animating() {
                break;
            }
        }
        assert!(
            saw_partial_progress,
            "the exit is a real ramp, not an instant snap to 0"
        );
        assert!((w.progress - 0.0).abs() < 1e-6);
    }

    // --- Drag-to-dismiss (interactive, position + velocity). ---

    #[derive(Default)]
    struct Flag {
        dismissed: u32,
    }

    fn build_flag(view: &BottomSheetView<Flag>) -> BottomSheetWidget {
        let mut counter = 0u64;
        View::<Flag>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn dispatch(w: &mut BottomSheetWidget, state: &mut Flag, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 600.0));
        w.event(&mut ctx, event)
    }

    fn laid_out_flag_sheet() -> BottomSheetWidget {
        let view: BottomSheetView<Flag> =
            bottom_sheet(leaf_any_flag(300.0, 200.0)).on_dismiss(|s: &mut Flag| s.dismissed += 1);
        let mut w = build_flag(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 600.0)));
        w
    }

    /// The same fixture, already settled at rest (`progress == 1.0`) — the
    /// tests below that assert *live position-tracking* need a clean open
    /// baseline to scrub down from.
    fn settled_flag_sheet() -> BottomSheetWidget {
        let view: BottomSheetView<Flag> =
            bottom_sheet(leaf_any_flag(300.0, 200.0)).on_dismiss(|s: &mut Flag| s.dismissed += 1);
        let mut counter = 0u64;
        let mut w = View::<Flag>::build(&view, &mut BuildCtx::new(&mut counter));
        settle(&mut w, Size::new(400.0, 600.0));
        w
    }

    // A `()`-erased leaf is `AnyView<()>`; the Flag-state sheet needs an
    // `AnyView<Flag>`, so wrap a fixed-size content view generic over state.
    fn leaf_any_flag(w: f64, h: f64) -> AnyView<Flag> {
        core_any::<Flag, _>(FixedLeaf {
            size: Size::new(w, h),
        })
    }
    struct FixedLeaf {
        size: Size,
    }
    struct FixedLeafW {
        size: Size,
    }
    impl View<Flag> for FixedLeaf {
        type Element = FixedLeafW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> FixedLeafW {
            FixedLeafW { size: self.size }
        }
        fn rebuild(&self, _p: &Self, _e: &mut FixedLeafW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for FixedLeafW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
    }

    #[test]
    fn mid_drag_progress_tracks_the_pointer() {
        let mut w = settled_flag_sheet();
        let mut state = Flag::default();
        let handle_y = w.panel.y0 + 10.0;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        assert_eq!(w.progress, 1.0, "no movement yet");

        // Past `TOUCH_SLOP` (18px): the panel starts following the finger.
        let extent = w.panel.height();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 200.0, handle_y + 40.0),
        );
        let expected = (1.0 - 40.0 / extent).clamp(0.0, 1.0);
        assert!(
            (w.progress - expected).abs() < 1e-6,
            "progress {} should track the 40px drag exactly ({} expected)",
            w.progress,
            expected
        );
        assert_eq!(state.dismissed, 0, "still mid-drag — nothing decided yet");

        // Moving back up retraces the same live mapping.
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 200.0, handle_y + 10.0),
        );
        let expected = (1.0 - 10.0 / extent).clamp(0.0, 1.0);
        assert!((w.progress - expected).abs() < 1e-6);
    }

    #[test]
    fn a_sub_slop_move_does_not_move_the_panel() {
        let mut w = settled_flag_sheet();
        let mut state = Flag::default();
        let handle_y = w.panel.y0 + 10.0;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 200.0, handle_y + 10.0), // < TOUCH_SLOP (18)
        );
        assert_eq!(w.progress, 1.0, "below the slop the panel doesn't move yet");
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 200.0, handle_y + 10.0),
        );
        assert_eq!(state.dismissed, 0, "a tap-shaped release does not dismiss");
    }

    #[test]
    fn handle_drag_past_threshold_dismisses() {
        let mut w = laid_out_flag_sheet();
        // panel height = 48 + 200 = 248; threshold = 124.
        let handle_y = w.panel.y0 + 10.0; // inside the 48dp handle strip
        let mut state = Flag::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        assert!(w.drag_captured);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 200.0, handle_y + 200.0),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 200.0, handle_y + 200.0),
        );
        assert_eq!(state.dismissed, 1, "a drag past half the sheet height pops");
    }

    #[test]
    fn short_handle_drag_does_not_dismiss() {
        let mut w = laid_out_flag_sheet();
        let handle_y = w.panel.y0 + 10.0;
        let mut state = Flag::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 200.0, handle_y + 20.0),
        );
        assert_eq!(state.dismissed, 0, "a short drag does not dismiss");
    }

    #[test]
    fn a_drag_released_under_the_dismiss_threshold_springs_back_open() {
        let mut w = settled_flag_sheet();
        let mut state = Flag::default();
        let handle_y = w.panel.y0 + 10.0;
        let extent = w.panel.height();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        // 30px is past TOUCH_SLOP but well short of the 50%-of-extent
        // dismiss threshold (124px on a 248px-tall panel).
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 200.0, handle_y + 30.0),
        );
        assert!(w.progress < 1.0 && w.progress > DRAG_DISMISS_FRACTION);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 200.0, handle_y + 30.0),
        );
        assert_eq!(state.dismissed, 0, "under the threshold — stays open");
        assert!(
            w.anim.is_animating(),
            "a real spring leg back toward 1.0 is running, not an instant snap"
        );
        let _ = extent;
    }

    #[test]
    fn a_fast_downward_flick_dismisses_regardless_of_position() {
        let mut w = settled_flag_sheet();
        let mut state = Flag::default();
        let handle_y = w.panel.y0 + 10.0;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        // Advance the clock 16ms between samples so the velocity tracker has
        // a real, non-zero reading — a small (30px) but *fast* downward move.
        tick(&mut w, Size::new(400.0, 600.0), 16);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 200.0, handle_y + 30.0),
        );
        // Position alone (30px of a 248px extent) would stay open, but the
        // ~1875px/s downward flick overrides it.
        assert!(w.progress > DRAG_DISMISS_FRACTION);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 200.0, handle_y + 30.0),
        );
        assert_eq!(state.dismissed, 1, "a fast downward flick dismisses");
    }

    #[test]
    fn a_fast_upward_flick_stays_open_regardless_of_position() {
        let mut w = settled_flag_sheet();
        let mut state = Flag::default();
        let handle_y = w.panel.y0 + 10.0;
        let area = Size::new(400.0, 600.0);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        // Drag well past the dismiss threshold first…
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 200.0, handle_y + 200.0),
        );
        assert!(w.progress < DRAG_DISMISS_FRACTION);
        // …then flick back upward. `VelocityTracker` reads off a trailing
        // window (`VELOCITY_WINDOW_MS`, 100ms), not an instantaneous
        // derivative, so age the downward sample out of that window first —
        // otherwise the still-in-window `Down`→+200 leg would dominate the
        // net reading despite the recent upward motion.
        tick(&mut w, area, 150);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 200.0, handle_y + 150.0),
        );
        tick(&mut w, area, 16);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 200.0, handle_y + 100.0),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 200.0, handle_y + 100.0),
        );
        assert_eq!(state.dismissed, 0, "a fast upward flick stays open");
    }

    // --- `dismissable(false)` gates the scrim + drag together. ---

    fn laid_out_non_dismissable_flag_sheet() -> BottomSheetWidget {
        let view: BottomSheetView<Flag> = bottom_sheet(leaf_any_flag(300.0, 200.0))
            .on_dismiss(|s: &mut Flag| s.dismissed += 1)
            .dismissable(false);
        let mut w = build_flag(&view);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 600.0)));
        w
    }

    #[test]
    fn dismissable_false_gates_handle_drag_dismiss() {
        let mut w = laid_out_non_dismissable_flag_sheet();
        // panel height = 48 + 200 = 248; threshold = 124 — well past it.
        let handle_y = w.panel.y0 + 10.0;
        let mut state = Flag::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 200.0, handle_y + 200.0),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 200.0, handle_y + 200.0),
        );
        assert_eq!(
            state.dismissed, 0,
            "dismissable(false): a past-threshold drag settles back without firing"
        );
        assert!(
            w.anim.is_animating(),
            "it springs back open rather than staying stuck where the drag left it"
        );
    }

    #[test]
    fn dismissable_false_gates_scrim_tap() {
        let mut w = laid_out_non_dismissable_flag_sheet();
        let mut state = Flag::default();
        assert!(!w.panel.contains(Point::new(5.0, 5.0)));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 0, "dismissable(false) gates the scrim tap");
    }

    #[test]
    fn scrim_tap_outside_panel_dismisses() {
        // Settled, not merely laid out: the barrier only swallows what it
        // shows, so a sheet whose entrance has not drawn a frame yet
        // (`progress == 0`) is input-transparent by design — see the Down arm's
        // visibility gate.
        let mut w = settled_flag_sheet();
        let mut state = Flag::default();
        // (5, 5) is in the top scrim, above the bottom-anchored panel.
        assert!(!w.panel.contains(Point::new(5.0, 5.0)));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 1);
    }

    // --- Navigator integration: scrim tap pops with an empty result. ---

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<i32>>,
    }

    #[test]
    fn scrim_tap_pops_sheet_via_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);

        // Push the sheet with an instant transition (no slide to settle),
        // wiring the dismiss to a plain pop.
        {
            let dismiss = controller.clone();
            controller.push_transparent_for_result(
                move || {
                    let d = dismiss.clone();
                    core_any::<NavState, _>(
                        bottom_sheet(bg_page(400.0, 200.0))
                            .on_dismiss(move |_s: &mut NavState| d.pop()),
                    )
                },
                TransitionSpec::NONE,
                |state: &mut NavState, result: PopResult| {
                    state.results.push(result.take::<i32>());
                },
            );
        }
        root.rebuild(&mut app, &mut state);
        // Settle the entrance before tapping: the barrier only swallows what it
        // shows, so a sheet that has not drawn a frame yet is input-transparent
        // by design (the Down arm's visibility gate).
        settle_navigator_frame(&mut root, Size::new(400.0, 600.0));

        // Tap the top scrim, above the bottom-anchored sheet.
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "scrim tap pops with an empty result"
        );
    }

    // --- Focus + Escape opt-in. ---

    /// Settle the navigator's own frame loop (a bounded pump of `paint`
    /// calls) so the sheet's internal entrance spring is fully at rest before
    /// a test relies on rest-position geometry (`handle_y` computed from the
    /// sheet's fixed panel height) — the entrance now lives inside the widget
    /// rather than riding the navigator's own page transition, so it needs
    /// its own settle pass here too.
    fn settle_navigator_frame<State, V>(root: &mut RenderRoot<State, V>, area: Size)
    where
        V: frust::authoring::View<State>,
    {
        let mut tcx = TextContext::new();
        for i in 0..40u64 {
            root.layout_with_text(area, &mut tcx as &mut dyn Any);
            root.paint(&mut Recorder::default(), ft(i * 20));
        }
    }

    #[test]
    fn escape_after_a_short_handle_press_claims_focus_and_dismisses_via_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        {
            let dismiss = controller.clone();
            controller.push_transparent_for_result(
                move || {
                    let d = dismiss.clone();
                    core_any::<NavState, _>(
                        bottom_sheet(bg_page(400.0, 200.0))
                            .on_dismiss(move |_s: &mut NavState| d.pop()),
                    )
                },
                TransitionSpec::NONE,
                |state: &mut NavState, result: PopResult| {
                    state.results.push(result.take::<i32>());
                },
            );
        }
        root.rebuild(&mut app, &mut state);
        settle_navigator_frame(&mut root, area);

        // panel height = 48 (handle strip) + 200 (content) = 248; panel_y = 352.
        // A short handle-strip drag (well under the 124px dismiss threshold)
        // claims focus without itself dismissing (mirrors
        // `short_handle_drag_does_not_dismiss`).
        let handle_y = area.height - 248.0 + 10.0;
        root.event(&mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        root.event(&mut state, &ev(PointerPhase::Up, 200.0, handle_y + 10.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        assert!(
            state.results.is_empty(),
            "a short handle drag does not dismiss"
        );

        // Escape now reaches the focused sheet and dismisses it, the same
        // dismiss path as a scrim tap / long handle drag.
        root.event(&mut state, &escape_event());
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "Escape dismisses the focused sheet"
        );
    }

    #[test]
    fn escape_without_a_prior_press_does_nothing() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        {
            let dismiss = controller.clone();
            controller.push_transparent_for_result(
                move || {
                    let d = dismiss.clone();
                    core_any::<NavState, _>(
                        bottom_sheet(bg_page(400.0, 200.0))
                            .on_dismiss(move |_s: &mut NavState| d.pop()),
                    )
                },
                TransitionSpec::NONE,
                |state: &mut NavState, result: PopResult| {
                    state.results.push(result.take::<i32>());
                },
            );
        }
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        // No press yet — the sheet never claimed focus, so Escape has no focus
        // chain to travel and is dropped.
        root.event(&mut state, &escape_event());
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert!(
            state.results.is_empty(),
            "Escape without prior focus is a no-op"
        );
    }

    #[test]
    fn a_down_reclaims_focus_after_an_external_blur() {
        use frust::authoring::ChildPod;

        let view: BottomSheetView<()> = bottom_sheet(leaf_any(300.0, 200.0));
        let area = Size::new(400.0, 600.0);
        let w = build(&view);
        let mut pod = ChildPod::new(Box::new(w));
        let mut lctx = LayoutCtx::new();
        pod.layout_child(&mut lctx, &BoxConstraints::tight(area));

        let mut dummy = ();
        // A scrim press well above the bottom-anchored panel (outside
        // content/handle) claims focus, then releases outside — dismissing
        // (mirrors `scrim_tap_outside_panel_dismisses`), which clears
        // `scrim_captured` regardless, returning to the "fresh events" arm.
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 5.0, 5.0));
        }
        assert!(pod.is_focused(), "the first Down claims focus");
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Up, 5.0, 5.0));
        }

        // Simulate an external blur (mirrors the root's own `Down`-with-no-
        // claim release path) so the second Down's own re-claim is what's
        // under test, not a leftover flag.
        pod.set_focused(false);
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 5.0, 5.0));
        }
        assert!(
            pod.is_focused(),
            "a second Down must re-claim focus after an external blur — this is what \
             keeps the root's focus/IME session alive while the sheet is up"
        );
    }

    // --- Root-level regression: an unclaimed Down must not blur a live focus/IME
    //     session (claim-once-hygiene fix-2a). ---
    //
    // The h1 claim-once guard broke exactly this: a second Down landing on the
    // sheet's own handle strip (a region the sheet's `event` handles itself,
    // *before* routing reaches `content` — see the "fresh events" Down arm
    // above) skipped `ctx.request_focus()` because the guard had already fired
    // once. With nothing else on that dispatch's path claiming focus either
    // (the handle-strip branch returns before `content` is ever routed to),
    // `RenderRoot`'s own Down arm (`crates/frust-core/src/app.rs`) saw no claim
    // bubble up at all and took its blur branch — `release_focus_session`,
    // dropping `focus_active` **and** `ime_state` together — even though a
    // `TextInput` inside the sheet's content was still mid-edit.

    #[derive(Default)]
    struct FieldState {
        value: String,
    }

    // A fixed-height (200dp) content slot wrapping a real `TextInput`, giving
    // the sheet the exact same panel geometry (`48 + 200 = 248dp`) the
    // `bg_page(400.0, 200.0)`-based focus tests above already rely on, so
    // `handle_y` below is derived the same way
    // `escape_after_a_short_handle_press_claims_focus_and_dismisses_via_navigator`
    // computes it.
    fn field_sheet_logic(state: &mut FieldState) -> BottomSheetView<FieldState> {
        bottom_sheet(frust::SizedBox(None, Some(200.0)).child(frust::text_input(
            state.value.clone(),
            |s: &mut FieldState, v: String| s.value = v,
        )))
    }

    #[test]
    fn a_second_down_on_chrome_keeps_the_root_focus_session() {
        let mut root: RenderRoot<FieldState, BottomSheetView<FieldState>> = RenderRoot::new();
        let mut state = FieldState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut field_sheet_logic, &mut state);
        settle_navigator_frame(&mut root, area);

        // Focus the field with a tap inside the content slot (panel height
        // 248, so content spans y in [400, 600] — well clear of the handle
        // strip at the panel's top).
        root.event(&mut state, &ev(PointerPhase::Down, 200.0, 500.0));
        root.event(&mut state, &ev(PointerPhase::Up, 200.0, 500.0));
        assert!(root.is_focus_active(), "tap inside the field focuses it");
        assert!(
            root.ime_state().is_some(),
            "focusing the field publishes an IME surface"
        );

        // A second, fresh Down on the handle strip (panel_y = 600 - 248 =
        // 352; strip is the top 48dp of the panel) — a region the sheet's
        // own `event` claims before `content` is ever routed to. Pre-fix,
        // this is exactly the dispatch the claim-once guard broke: no widget
        // on this path called `request_focus`, so the root's Down arm took
        // its blur branch and released the whole session.
        let handle_y = area.height - 248.0 + 10.0;
        root.event(&mut state, &ev(PointerPhase::Down, 200.0, handle_y));

        assert!(
            root.is_focus_active(),
            "a second Down on the sheet's own chrome must not blur the root's \
             live focus session"
        );
        assert!(
            root.ime_state().is_some(),
            "the field's IME surface must survive a second Down on chrome that \
             never itself routed to the field"
        );
    }

    // --- `dismissable(false)` gates Escape too (via the navigator). ---

    #[test]
    fn dismissable_false_gates_escape_via_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        {
            let dismiss = controller.clone();
            controller.push_transparent_for_result(
                move || {
                    let d = dismiss.clone();
                    core_any::<NavState, _>(
                        bottom_sheet(bg_page(400.0, 200.0))
                            .dismissable(false)
                            .on_dismiss(move |_s: &mut NavState| d.pop()),
                    )
                },
                TransitionSpec::NONE,
                |state: &mut NavState, result: PopResult| {
                    state.results.push(result.take::<i32>());
                },
            );
        }
        root.rebuild(&mut app, &mut state);
        settle_navigator_frame(&mut root, area);

        // A short handle-strip press claims focus (unaffected by
        // `dismissable`), but the subsequent Escape is gated.
        let handle_y = area.height - 248.0 + 10.0;
        root.event(&mut state, &ev(PointerPhase::Down, 200.0, handle_y));
        root.event(&mut state, &ev(PointerPhase::Up, 200.0, handle_y + 10.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        root.event(&mut state, &escape_event());
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert!(
            state.results.is_empty(),
            "dismissable(false) gates Escape too"
        );
    }

    // --- Back request routes through dismissable/BackPolicy. ---

    #[test]
    fn back_request_dismissable_true_dismisses_via_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_bottom_sheet(
            &controller,
            || bottom_sheet(bg_page(400.0, 200.0)),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        // Settle the sheet's own internal spring entrance (it no longer rides
        // a navigator page transition — the entrance moved inside the widget,
        // see the module docs' *Spring entrance/exit* section).
        settle_navigator_frame(&mut root, area);

        assert!(
            controller.back_interest(),
            "a dismissable sheet claims back interest"
        );

        controller.request_back();
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            controller.depth(),
            2,
            "DismissAnimated: the stack is unchanged immediately"
        );

        // The back-press rebuild flagged PAINT; this paint observes the bumped
        // signal and *stages* the exit — the pop itself waits for the closing
        // spring to settle (the module docs' *Who owns the exit*; the staging
        // is what the assertions below changed for). Either way this same paint
        // MUST schedule the next frame, asserted here BEFORE any further
        // rebuild (the honest contract; the old test hand-called `rebuild()`
        // here, hiding exactly this gap).
        let outcome = root.paint(&mut Recorder::default(), ft(900));
        assert!(
            outcome.needs_frame,
            "the back-dismiss paint schedules the frames its staged exit needs"
        );
        assert_eq!(
            controller.depth(),
            2,
            "the exit ramp is only started at paint — the pop comes later"
        );

        // Drive the loop ONLY via outcome-honoring frames (rebuild + layout +
        // paint, continuing while the outcome asks for another, bounded): no
        // manual extra rebuild. The exit ramp runs and the pop drains through
        // the shell contract alone.
        let mut frames = 0u64;
        loop {
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(area, &mut tcx as &mut dyn Any);
            let outcome = root.paint(&mut Recorder::default(), ft(1000 + frames * 100));
            frames += 1;
            assert!(
                frames < 20,
                "the exit ramp settles and the pop drains within a bounded number of frames"
            );
            if !outcome.needs_frame {
                break;
            }
        }
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "a back request dismisses the sheet"
        );
        assert_eq!(controller.depth(), 1);
    }

    // --- The staged exit, through the real `show_bottom_sheet` navigator
    //     path (not a hand-mounted widget the pop never reaches). ---

    /// The whole point of staging: on the navigator path the panel must be seen
    /// walking off-screen across several frames *while the page is still
    /// mounted*, and only then does the pop land. A sheet whose pop drains a
    /// frame into its exit spring would show one or two positions here, not a
    /// ramp that reaches the bottom edge.
    #[test]
    fn a_scrim_tap_animates_the_panel_out_before_the_navigator_pop_lands() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_bottom_sheet(
            &controller,
            || bottom_sheet(bg_page(400.0, 200.0)),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        settle_navigator_frame(&mut root, area);

        // Panel height = 48 (handle strip) + 200 (content) = 248, so at rest
        // its top edge sits at 600 - 248 = 352.
        let rest_top = area.height - 248.0;

        // A scrim tap above the panel.
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(
            controller.depth(),
            2,
            "the tap stages the exit — it does not pop on the spot"
        );

        // Drive shell-shaped frames (rebuild + layout + paint) until the pop has
        // drained, recording where the panel's top edge sits on each one.
        let mut tops: Vec<f64> = Vec::new();
        let mut frames = 0u64;
        while controller.depth() == 2 {
            frames += 1;
            assert!(
                frames < 60,
                "the staged exit settles and pops within a bounded number of frames"
            );
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(area, &mut tcx as &mut dyn Any);
            let mut rec = Recorder::default();
            root.paint(&mut rec, ft(1000 + frames * 20));
            if let Some(top) = rec.panel_top() {
                tops.push(top);
            }
        }

        // A real ramp: several *distinct* intermediate positions between rest
        // and fully off-screen, all painted while the page was still mounted.
        let mid: Vec<f64> = tops
            .iter()
            .copied()
            .filter(|t| *t > rest_top + 1.0 && *t < area.height - 1.0)
            .collect();
        assert!(
            mid.len() >= 3,
            "the exit is a visible ramp, not a snap: intermediate panel tops {mid:?} \
             (all frames: {tops:?})"
        );
        assert!(
            mid.windows(2).all(|w| w[1] > w[0]),
            "the panel only ever travels downward on the way out: {mid:?}"
        );
        assert!(
            tops.last().is_some_and(|t| (t - area.height).abs() < 1.0),
            "the panel is fully off-screen on the frame the pop finally fires: {tops:?}"
        );

        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));
        assert_eq!(
            state.results,
            vec![None],
            "and the sheet really did pop, with an empty result"
        );
        assert_eq!(controller.depth(), 1);
    }

    /// `reduce_motion` must *collapse* the staged exit, never leave it waiting:
    /// a pop deferred behind a spring that is never allowed to run would strand
    /// the sheet on screen forever.
    #[test]
    fn reduce_motion_collapses_the_staged_exit_and_still_pops() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;

        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        root.set_theme(Box::new(theme));
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_bottom_sheet(
            &controller,
            || bottom_sheet(bg_page(400.0, 200.0)),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        settle_navigator_frame(&mut root, area);

        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 5.0));

        // One paint to collapse the exit and fire the pop, one rebuild to drain
        // it — no ramp to wait out.
        for i in 0..3u64 {
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(area, &mut tcx as &mut dyn Any);
            root.paint(&mut Recorder::default(), ft(1000 + i * 20));
        }
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "reduce_motion still dismisses — it only skips the ramp"
        );
        assert_eq!(controller.depth(), 1);
    }

    /// The staged pop waits out a whole exit ramp, and a navigator pop always
    /// takes the *top* page: a page the app pushes inside that window must not
    /// be popped in the sheet's place — and the sheet must stay dismissible
    /// afterwards rather than latching itself closed over a page it never
    /// removed.
    #[test]
    fn a_page_pushed_during_the_exit_ramp_survives_and_the_sheet_stays_dismissable() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_bottom_sheet(
            &controller,
            || bottom_sheet(bg_page(400.0, 200.0)),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        settle_navigator_frame(&mut root, area);

        // A scrim tap stages the exit…
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(controller.depth(), 2, "the tap only stages");

        // …and one frame into the ramp the app pushes a page of its own (a deep
        // link, an async completion landing — it cannot come from the user,
        // since the sheet is a barrier). Transparent, so the sheet keeps
        // painting underneath it and its ramp really does reach the settle that
        // fires the staged pop; an opaque page would cull the sheet's paint and
        // the pop would simply never fire.
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft(1000));
        controller.push_transparent(|| core_any::<NavState, _>(bg_page(400.0, 100.0)));

        for i in 0..40u64 {
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(area, &mut tcx as &mut dyn Any);
            root.paint(&mut Recorder::default(), ft(1020 + i * 20));
        }
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            controller.depth(),
            3,
            "the pushed page survives — the staged pop was refused, not aimed at it"
        );
        assert!(
            state.results.is_empty(),
            "and the sheet reported no dismissal it never made"
        );

        // Recoverable, not bricked: back out of the pushed page, then dismiss
        // the sheet again. The fresh exit is staged against the stack it
        // actually pops, so this one lands.
        controller.request_back();
        for i in 0..40u64 {
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(area, &mut tcx as &mut dyn Any);
            root.paint(&mut Recorder::default(), ft(2000 + i * 20));
        }
        assert_eq!(controller.depth(), 2, "the pushed page backs out normally");

        controller.request_back();
        for i in 0..40u64 {
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(area, &mut tcx as &mut dyn Any);
            root.paint(&mut Recorder::default(), ft(3000 + i * 20));
        }
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));
        assert_eq!(
            state.results,
            vec![None],
            "the sheet is still dismissible after the refused pop"
        );
        assert_eq!(controller.depth(), 1);
    }

    // --- The app-initiated dismiss (`ModalDismiss`) seam — the same handle
    //     type `crate::overlay::modal`/`crate::side_sheet` install, routed
    //     through this module's own staged-exit path. ---

    /// The whole point of adopting the handle: a row's own action must not
    /// pop the navigator directly (which would take the page out from under
    /// the sheet mid-slide) — it stages the very same spring exit a scrim
    /// tap does, and can carry a result with it, the same way
    /// `crate::side_sheet`'s own "actions" test proves for its handle.
    #[test]
    fn a_dismiss_handle_stages_the_slide_out_instead_of_popping_at_once() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        // Minted outside the page builder, exactly as an app does it.
        let dismiss = ModalDismiss::new();
        let installed = dismiss.clone();
        show_bottom_sheet(
            &controller,
            move || bottom_sheet(bg_page(400.0, 200.0)).dismiss_handle(installed.clone()),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        settle_navigator_frame(&mut root, area);

        // A row's own "select" action, through the handle.
        dismiss.dismiss_with(PopResult::of(3));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft(1000));
        assert_eq!(
            controller.depth(),
            2,
            "the request stages the exit — it does not pop on the spot"
        );

        let mut frames = 0u64;
        while controller.depth() == 2 {
            frames += 1;
            assert!(
                frames < 60,
                "the staged exit settles and pops within a bounded number of frames"
            );
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(area, &mut tcx as &mut dyn Any);
            root.paint(&mut Recorder::default(), ft(1000 + frames * 20));
        }
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![Some(3)],
            "the request's own payload rides the staged pop"
        );
        assert_eq!(controller.depth(), 1);
    }

    /// Two triggers landing before either is observed collapse into one
    /// staged exit (the generation latch), and a further trigger arriving
    /// while that exit is already running is a no-op (`request_dismiss`'s own
    /// `exiting`/`closed` guard) — the sheet pops exactly once either way.
    #[test]
    fn repeated_dismiss_requests_are_idempotent() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        let dismiss = ModalDismiss::new();
        let installed = dismiss.clone();
        show_bottom_sheet(
            &controller,
            move || bottom_sheet(bg_page(400.0, 200.0)).dismiss_handle(installed.clone()),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        settle_navigator_frame(&mut root, area);

        // Two requests before either is observed — the second's generation
        // bump wins the latch, but only one exit stages.
        dismiss.dismiss();
        dismiss.dismiss();
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft(1000));
        assert_eq!(controller.depth(), 2, "still staging, not popped yet");

        // A third request while the exit is already running: `stage_exit`'s
        // own guard refuses to restart the ramp or re-arm the pop.
        dismiss.dismiss();
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft(1020));

        let mut frames = 0u64;
        while controller.depth() == 2 {
            frames += 1;
            assert!(frames < 60, "the exit still settles within a bounded time");
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(area, &mut tcx as &mut dyn Any);
            root.paint(&mut Recorder::default(), ft(1050 + frames * 20));
        }
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "exactly one pop, however many requests were raised"
        );
        assert_eq!(controller.depth(), 1);
    }

    // --- The staging flags: what cancels a staged exit, and what a refused pop
    //     must not latch. ---

    /// A sheet wired the way [`show_bottom_sheet`] wires one — the state-free
    /// close hook plus the shared back-press dismiss cell — but mounted
    /// directly, so a test can drive its event and paint passes without a
    /// navigator. Returns the widget, the back-press generation cell, and a
    /// count of the hook's own firings.
    fn staged_flag_sheet() -> (BottomSheetWidget, Rc<Cell<u64>>, Rc<Cell<u32>>) {
        let signal = Rc::new(Cell::new(0u64));
        let pops = Rc::new(Cell::new(0u32));
        let counter = pops.clone();
        let mut view: BottomSheetView<Flag> =
            bottom_sheet(leaf_any_flag(300.0, 200.0)).on_dismiss(|s: &mut Flag| s.dismissed += 1);
        view.on_close = Some(Rc::new(StagedPop::unguarded(move || {
            counter.set(counter.get() + 1);
        })));
        view.dismiss_signal = Some(signal.clone());
        let mut w = build_flag(&view);
        settle(&mut w, Size::new(400.0, 600.0));
        (w, signal, pops)
    }

    /// Pump `paint` frames from wherever `w`'s own clock currently sits until
    /// its spring settles — [`settle`]'s continuation, for a test that has
    /// already driven the widget past its entrance. A second `settle` would
    /// rewind the clock behind `w.last_frame_time` and corrupt the spring's own
    /// timeline (the same hazard [`tick`] exists to avoid).
    fn settle_from_here(w: &mut BottomSheetWidget, area: Size) {
        for _ in 0..60u64 {
            tick(w, area, 20);
            if !w.anim.is_animating() {
                return;
            }
        }
        panic!("sheet spring failed to settle within the test bound");
    }

    /// A back press landing *while a handle drag is live* stages an exit under
    /// that drag — and the release then aims a fresh leg back open. That leg
    /// must cancel the staging: an exit left flagged over a sheet resting at
    /// `1.0` can never fire (the settle only fires at the closed end), while
    /// every later trigger hard-returns on the flag — scrim tap, `Escape` and
    /// back all dead, with the sheet still a modal barrier.
    #[test]
    fn a_release_back_open_cancels_an_exit_staged_under_the_drag() {
        for cancelled in [false, true] {
            let (mut w, signal, pops) = staged_flag_sheet();
            let area = Size::new(400.0, 600.0);
            let mut state = Flag::default();
            let handle_y = w.panel.y0 + 10.0;

            // A live handle drag, short of the dismiss threshold (30px of a
            // 248px panel).
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, handle_y));
            dispatch(
                &mut w,
                &mut state,
                &ev(PointerPhase::Move, 200.0, handle_y + 30.0),
            );
            assert!(w.progress > DRAG_DISMISS_FRACTION);

            // An Android back press lands mid-drag: the next paint observes the
            // bumped cell and stages the exit with the finger still down
            // (`observe_dismiss_signal` has no drag guard, by design — the
            // release below is what decides).
            signal.set(signal.get() + 1);
            tick(&mut w, area, 16);
            assert!(w.exiting, "the back press stages an exit under the drag");

            // Then the finger lifts above the dismiss threshold — or the OS
            // cancels the gesture outright, which an Android back *gesture*
            // does. Either way the panel springs back open.
            let phase = if cancelled {
                PointerPhase::Cancel
            } else {
                PointerPhase::Up
            };
            dispatch(&mut w, &mut state, &ev(phase, 200.0, handle_y + 30.0));
            assert!(
                !w.exiting,
                "a leg aimed back open cancels the staged exit (cancelled: {cancelled})"
            );

            settle_from_here(&mut w, area);
            assert!(
                (w.progress - 1.0).abs() < 1e-6,
                "the sheet is back at rest, open: {}",
                w.progress
            );
            assert_eq!(pops.get(), 0, "nothing popped — the sheet reopened");

            // The regression itself: the sheet is still dismissible.
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
            settle_from_here(&mut w, area);
            assert_eq!(
                pops.get(),
                1,
                "a scrim tap after the reopen still dismisses (cancelled: {cancelled})"
            );
        }
    }

    /// A sheet that has finished its exit paints nothing — no scrim (alpha
    /// rides `progress`), no panel (off-screen) — but stays mounted until the
    /// enqueued pop drains. It must not go on swallowing presses there: an
    /// invisible barrier is an input trap with nothing on screen to explain it.
    #[test]
    fn a_sheet_that_has_finished_its_exit_swallows_no_press() {
        let (mut w, _signal, pops) = staged_flag_sheet();
        let area = Size::new(400.0, 600.0);
        let mut state = Flag::default();
        let over_panel = Point::new(200.0, w.panel.y0 + 100.0);

        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        settle_from_here(&mut w, area);
        assert_eq!(pops.get(), 1, "the staged pop fired");
        assert!(w.closed, "and latched — the sheet is on its way out");
        assert!(w.progress <= SETTLED_EPSILON, "painting nothing");

        let result = dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, over_panel.x, over_panel.y),
        );
        assert_eq!(
            result,
            EventResult::Ignored,
            "an invisible sheet absorbs nothing"
        );
        assert!(!w.scrim_captured, "and arms no scrim dismiss either");
    }

    /// The mirror pin for the entrance: it belongs to the widget's spring
    /// alone. A page-level slide would offset the *whole* sheet page — scrim
    /// included — so every full-area fill sitting at the window origin is what
    /// proves `TransitionSpec::NONE` is still what `show_bottom_sheet` pushes,
    /// while the panel path underneath it still climbs.
    #[test]
    fn the_entrance_animates_only_the_panel_never_the_page() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_bottom_sheet(
            &controller,
            || bottom_sheet(bg_page(400.0, 200.0)),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);

        let mut tops: Vec<f64> = Vec::new();
        for i in 0..40u64 {
            root.layout_with_text(area, &mut tcx as &mut dyn Any);
            let mut rec = Recorder::default();
            root.paint(&mut rec, ft(i * 20));
            for (origin, size, _) in &rec.rects {
                if *size == area {
                    assert_eq!(
                        *origin,
                        Point::ZERO,
                        "a full-area fill painted away from the window origin means the \
                         navigator is sliding the page too (frame {i})"
                    );
                }
            }
            if let Some(top) = rec.panel_top() {
                tops.push(top);
            }
        }

        let rest_top = area.height - 248.0;
        assert!(
            tops.first().is_some_and(|t| (t - area.height).abs() < 1e-6),
            "the first frame is invisible — the panel starts fully below the fold: {tops:?}"
        );
        assert!(
            tops.iter()
                .any(|t| *t > rest_top + 1.0 && *t < area.height - 1.0),
            "the panel climbs through intermediate positions: {tops:?}"
        );
        assert!(
            tops.last().is_some_and(|t| (t - rest_top).abs() < 1e-6),
            "…and comes to rest with its top edge at {rest_top}: {tops:?}"
        );
    }

    #[test]
    fn back_request_dismissable_false_vetoes_stack_unchanged() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || core_any::<NavState, _>(bg_page(400.0, 600.0)))
            }
        };
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_bottom_sheet(
            &controller,
            || bottom_sheet(bg_page(400.0, 200.0)).dismissable(false),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        settle_navigator_frame(&mut root, area);

        assert!(
            controller.back_interest(),
            "a Veto (non-dismissable) sheet still claims back interest"
        );

        controller.request_back();
        root.rebuild(&mut app, &mut state);
        assert_eq!(controller.depth(), 2, "Veto leaves the stack unchanged");

        root.paint(&mut Recorder::default(), ft(950));
        root.paint(&mut Recorder::default(), ft(1400));
        assert!(
            state.results.is_empty(),
            "a non-dismissable sheet never pops on a back request"
        );
        assert_eq!(controller.depth(), 2);
    }

    // A minimal opaque page for the navigator integration test.
    struct BgPage {
        size: Size,
    }
    struct BgPageW {
        size: Size,
    }
    impl View<NavState> for BgPage {
        type Element = BgPageW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> BgPageW {
            BgPageW { size: self.size }
        }
        fn rebuild(&self, _p: &Self, _e: &mut BgPageW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for BgPageW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }
    fn bg_page(w: f64, h: f64) -> BgPage {
        BgPage {
            size: Size::new(w, h),
        }
    }

    #[test]
    fn semantics_is_a_modal_dialog_ish_node() {
        fn logic(_s: &mut ()) -> BottomSheetView<()> {
            bottom_sheet(leaf_any(300.0, 200.0))
        }
        let mut root: RenderRoot<(), BottomSheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Dialog)
            .expect("a Role::Dialog node is contributed");
        assert!(node.is_modal(), "the sheet node sets the modal flag");
    }
}
