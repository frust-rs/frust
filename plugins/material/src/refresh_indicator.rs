// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/refresh_indicator/` — itself, per that package's own
// attribution, a Rust-ward-facing port of Álvaro N's `expressive-refresh`
// (https://github.com/alvaronp/expressive-refresh, MIT, © 2025 Álvaro N; see
// `plugins/material/NOTICE`'s Additional Copyright Holders section). Porting
// decisions below.

//! M3E pull-to-refresh: a wrapper around a scrollable child that shows the
//! [`mod@crate::loading_indicator`] spinner while the app's async refresh
//! runs, triggered by pulling past the baseline's own overscroll trigger.
//!
//! # Architecture: this widget owns the [`frust::scroll_view`]
//!
//! Upstream wraps an *app-supplied* `Scrollable` descendant via
//! `NotificationListener`, observing whatever scroll surface happens to sit
//! underneath. The framework's public surface has no notification-bubbling
//! seam like that — the two seams a scroll surface exposes are
//! [`frust::ScrollView::on_scroll`] and
//! [`frust::ScrollView::on_refresh_release`], both installed on the view at
//! construction, not observed after the fact. So [`refresh_indicator`] takes
//! the child *content* (not an already-built `ScrollView`) and builds the
//! `scroll_view` itself, installing both callbacks internally — the same
//! shape [`crate`]'s `frust_shadcn::scroll_area` precedent uses for the one
//! other in-tree wrapper over this baseline.
//!
//! # Pull-progress mapping
//!
//! Upstream's drag mapping normalizes the raw drag delta against 25% of the
//! scrollable's `containerExtent` (`_checkDragOffset`,
//! `m3e_refresh_indicator_scroll.dart:248-279`) — a value `ScrollInfo` does
//! not carry (no viewport dimension crosses this framework's seam). The port
//! maps [`ScrollInfo::overscroll`] (already resistance-applied by
//! `ScrollView`'s own rubber-band, negative past the top) directly in
//! logical px instead: [`visual_pull`] clamps `-overscroll` to
//! `displacement + indicator_height`, the same "resting refresh position"
//! target upstream's own curve converges on. This is a **simpler curve, not
//! a functionality loss** — document, not a `LIMITATIONS.md` entry.
//!
//! # Refresh lifecycle: controlled
//!
//! [`refresh_indicator`]'s `on_refresh` callback fires exactly once per pull
//! released past the baseline's trigger — [`frust::ScrollView`]'s own
//! `REFRESH_TRIGGER_PX` (64px, post-resistance) is the arming threshold, not
//! a value this crate restates. Unlike upstream's `Future`-returning
//! `onRefresh`, which the indicator itself awaits via a `Completer`, this
//! port cannot hold an arbitrary async task inside a widget (there is no
//! widget-local executor seam — see `docs/CORE_ARCHITECTURE.md`'s
//! `spawn`/`spawn_blocking` routing). The lifecycle is **controlled**
//! instead, matching this crate's `Checkbox`/`Slider` convention: `on_refresh`
//! returns control to the app immediately; the app flips its own
//! `refreshing` state to `true` (typically inside `on_refresh`, spawning the
//! real async work alongside it) and feeds it back through
//! [`RefreshIndicatorView::refreshing`] on the next rebuild. The indicator
//! shows its spinner for as long as `refreshing` reads `true`, and only ends
//! it (scaling away) on a rebuild where the app confirms `refreshing =
//! false` — an app that never flips it back leaves the spinner showing
//! forever, the same contract violation an unconfirmed `Checkbox` toggle is.
//!
//! A second release-past-trigger while a refresh is already in flight does
//! **not** re-invoke `on_refresh` (`in_flight`, guarded at the callback
//! itself) — the underlying scroll content stays interactive during a
//! refresh (see the next section), so a fast second pull is a real,
//! reachable case, not a hypothetical one.
//!
//! # Documented seam gap: the scroll offset is not held during a refresh
//!
//! Upstream visually "holds" the pulled content at the indicator's resting
//! offset for the whole refresh, tied to the same `_positionController` that
//! drives the indicator. `ScrollView`'s public seam has no "pin the offset
//! until released" affordance — its own release-settle always eases back to
//! the clamped edge once the pointer lifts, independent of whatever this
//! wrapper is doing. The port accepts this: the indicator overlays its own
//! resting position via [`Widget::paint`]-time compositing (translate +
//! optional scale, [`frust::authoring::PaintScene::push_transform`]) while
//! the scroll content underneath is free to settle and scroll normally
//! during a refresh. **FINDING candidate**: a `ScrollView` seam to suppress
//! the release-settle until a caller releases it would close this gap
//! properly; per the plugin charter this crate does not patch
//! `frust-widgets` to add it.
//!
//! A second, narrower gap: [`frust::ScrollView::on_scroll`] delivers an
//! animation-driven change (the release-settle) **one event late**
//! (documented on that method) rather than every paint frame, so this
//! widget's own reveal never rides the baseline's settle motion — it runs
//! its own `snap`/`retract` [`AnimationController`]s from the pull distance
//! observed at release/cancel instead, independent of whatever the
//! underlying surface does afterward.
//!
//! # Variants
//!
//! [`refresh_indicator`] is the expressive default
//! ([`crate::LoadingIndicatorVariant::Default`]);
//! [`RefreshIndicatorView::contained`] switches to
//! [`crate::LoadingIndicatorVariant::Contained`] (both delegate their
//! MaterialTokens color resolution to [`mod@crate::loading_indicator`]
//! itself — this module never overrides a color). Upstream's `material`
//! and `adaptive` variants (`RefreshProgressIndicator`,
//! `CupertinoActivityIndicator`) are out of scope: only the two M3E-shaped
//! ones plus [`RefreshIndicatorView::no_spinner`] (upstream's
//! `M3ERefreshIndicator.noSpinner` — the trigger/lifecycle contract with no
//! visual at all, for an app that draws its own affordance elsewhere) are
//! ported.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, SemanticsCtx, Size, View,
    Widget, any, build_child, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{AnimationController, ScrollInfo, scroll_view};

use crate::loading_indicator::loading_indicator;
use crate::tokens::MaterialMotion;

/// Default resting offset (logical px) between the scrollable's leading edge
/// and the indicator once a refresh is showing — upstream's
/// `kDefaultDisplacement` (`m3e_refresh_indicator_theme.dart:9-10`).
const DEFAULT_DISPLACEMENT: f64 = 40.0;

/// Duration of the release→resting snap: the indicator sliding in from
/// wherever the pull left it while it starts spinning — upstream's
/// `indicatorSnapDuration` (`m3e_refresh_indicator_theme.dart:32`, 150ms),
/// which maps 1:1 onto this crate's own [`MaterialMotion::SHORT_3`] token.
const SNAP_DURATION: Duration = MaterialMotion::SHORT_3;

/// Duration of a retract — sliding back out (canceled) or shrinking away
/// (done) — upstream's `indicatorScaleDuration`
/// (`m3e_refresh_indicator_theme.dart:33`, 200ms), mapping onto
/// [`MaterialMotion::SHORT_4`].
const RETRACT_DURATION: Duration = MaterialMotion::SHORT_4;

/// Fallback indicator diameter used only for the sliver of time between
/// [`View::build`] and the first [`Widget::layout`] populating
/// [`RefreshIndicatorWidget::indicator_size`] — matches
/// [`mod@crate::loading_indicator`]'s own container size (its
/// `LOADING_DIAMETER`, private to that module).
const INDICATOR_FALLBACK_DIAMETER: f64 = 48.0;

/// Maps the live pull distance to a px reveal, clamped to `max_pull` —
/// upstream's `_visualPull` (`m3e_refresh_indicator_build.dart:19-21`). See
/// the [module docs](self)'s Pull-progress mapping section for why this
/// reads `overscroll` directly rather than a `containerExtent`-normalized
/// value.
fn visual_pull(overscroll: f64, max_pull: f64) -> f64 {
    (-overscroll).clamp(0.0, max_pull.max(0.0))
}

/// The indicator's pull distance while [`Status::Refreshing`]'s snap-in ramp
/// is running: an eased lerp from the pull distance captured at release
/// (`from`) to the resting position (`max_pull`) — upstream's `_show`
/// animating `_positionController` to `1.0 / kDragSizeFactorLimit`
/// (`m3e_refresh_indicator_scroll.dart:354-358`), read back through
/// `_pullDistance`'s snap/refresh/done/canceled branch
/// (`m3e_refresh_indicator_build.dart:38-39`).
fn snap_pull(from: f64, max_pull: f64, t: f64) -> f64 {
    from + (max_pull - from) * t.clamp(0.0, 1.0)
}

/// The indicator's pull distance while [`RetractKind::Canceled`]'s retract
/// ramp is running: an eased lerp from the pull distance captured at
/// cancellation back to zero — upstream's `_animateDismiss`'s `canceled`
/// branch animating `_positionController` to `0`
/// (`m3e_refresh_indicator_scroll.dart:328-331`).
fn cancel_retract_pull(from: f64, t: f64) -> f64 {
    from * (1.0 - t.clamp(0.0, 1.0))
}

/// The indicator's scale while [`RetractKind::Done`]'s retract ramp is
/// running — upstream's `_oneToZeroTween`/`_scaleFactor`
/// (`m3e_refresh_indicator.dart:213-216,225`), animated to `1` over
/// `indicatorScaleDuration` in `_animateDismiss`'s `done` branch.
fn done_scale(t: f64) -> f64 {
    1.0 - t.clamp(0.0, 1.0)
}

/// The indicator's lifecycle state — mirrors [upstream's
/// `M3ERefreshStatus`](m3e_refresh_indicator/enums/m3e_refresh_status.dart),
/// collapsed to what this port's simpler drag model needs (see the
/// [module docs](self)). `Idle` (upstream's `null` status) paints nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// No pull in progress; nothing painted.
    Idle,
    /// A drag is pulling the surface past its top edge; the indicator
    /// follows [`ScrollInfo::overscroll`] 1:1 (via [`visual_pull`]).
    Pulling,
    /// Released past the trigger: the indicator snaps to its resting
    /// position and spins, holding there until the app confirms
    /// [`RefreshIndicatorView::refreshing`]`(false)`.
    Refreshing,
    /// Retracting away, either because the pull was released under the
    /// trigger (or abandoned) or because the app confirmed completion.
    Retracting(RetractKind),
}

/// Which retract animation [`Status::Retracting`] is running — see
/// [`cancel_retract_pull`]/[`done_scale`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RetractKind {
    /// Released under the trigger (or the gesture was abandoned): the
    /// indicator slides back out, position animating to zero.
    Canceled,
    /// The app confirmed the refresh finished: the indicator shrinks away
    /// in place, scale animating to zero.
    Done,
}

/// A declarative M3E pull-to-refresh wrapper. See the [module docs](self).
pub struct RefreshIndicatorView<State: 'static> {
    /// The internally-built [`frust::scroll_view`], already carrying both
    /// callbacks (see the [module docs](self)'s Architecture section).
    scroll: AnyView<State>,
    /// The spinner, or `None` for [`RefreshIndicatorView::no_spinner`].
    indicator: Option<AnyView<State>>,
    /// Shared with `scroll`'s installed `on_scroll` closure: the live
    /// overscroll, written every scroll notification.
    pull: Rc<Cell<f64>>,
    /// Shared with `scroll`'s installed `on_refresh_release` closure:
    /// edge-triggered, set `true` the instant a release past the trigger
    /// occurs, drained by [`RefreshIndicatorWidget::event`] on the very next
    /// event.
    released: Rc<Cell<bool>>,
    /// Shared with the same closure: guards against re-invoking the app's
    /// `on_refresh` while a refresh is already in flight (see the
    /// [module docs](self)).
    in_flight: Rc<Cell<bool>>,
    displacement: f64,
    refreshing: bool,
}

/// Wrap `child` in a pull-to-refresh surface, firing `on_refresh` once per
/// pull released past the baseline's trigger. See the [module docs](self)
/// for the controlled `refreshing` lifecycle contract.
pub fn refresh_indicator<State, V, R>(child: V, on_refresh: R) -> RefreshIndicatorView<State>
where
    State: 'static,
    V: View<State>,
    R: Fn(&mut State) + 'static,
{
    let pull = Rc::new(Cell::new(0.0_f64));
    let released = Rc::new(Cell::new(false));
    let in_flight = Rc::new(Cell::new(false));

    let pull_for_scroll = pull.clone();
    let released_for_scroll = released.clone();
    let in_flight_for_scroll = in_flight.clone();
    let on_refresh: Rc<dyn Fn(&mut State)> = Rc::new(on_refresh);

    let scroll = any(scroll_view(child)
        .on_scroll(move |_state: &mut State, info: ScrollInfo| {
            pull_for_scroll.set(info.overscroll);
        })
        .on_refresh_release(move |state: &mut State| {
            if in_flight_for_scroll.replace(true) {
                // A refresh is already running — see the module docs'
                // concurrency note; the app's callback fires at most once
                // per in-flight refresh.
                return;
            }
            released_for_scroll.set(true);
            on_refresh(state);
        }));

    RefreshIndicatorView {
        scroll,
        indicator: Some(any(loading_indicator())),
        pull,
        released,
        in_flight,
        displacement: DEFAULT_DISPLACEMENT,
        refreshing: false,
    }
}

impl<State: 'static> RefreshIndicatorView<State> {
    /// Switch to the contained spinner
    /// ([`crate::LoadingIndicatorVariant::Contained`] — a filled, fully
    /// rounded container behind the shape).
    pub fn contained(mut self) -> Self {
        self.indicator = Some(any(loading_indicator().contained()));
        self
    }

    /// Drop the built-in spinner entirely — upstream's
    /// `M3ERefreshIndicator.noSpinner`. The pull/trigger/`refreshing`
    /// contract still runs; nothing is painted beyond the wrapped content.
    pub fn no_spinner(mut self) -> Self {
        self.indicator = None;
        self
    }

    /// Override the resting offset (logical px) between the leading edge and
    /// the indicator once it is showing — upstream's `displacement`.
    /// Negative values clamp to `0`.
    pub fn displacement(mut self, displacement: f64) -> Self {
        self.displacement = displacement.max(0.0);
        self
    }

    /// The controlled completion prop: `true` while the app's async refresh
    /// is running. See the [module docs](self)'s Refresh lifecycle section —
    /// flipping this back to `false` on a later rebuild is what ends the
    /// spinner.
    pub fn refreshing(mut self, refreshing: bool) -> Self {
        self.refreshing = refreshing;
        self
    }
}

impl<State: 'static> View<State> for RefreshIndicatorView<State> {
    type Element = RefreshIndicatorWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> RefreshIndicatorWidget {
        RefreshIndicatorWidget {
            scroll: build_child(&self.scroll, ctx),
            indicator: self.indicator.as_ref().map(|v| build_child(v, ctx)),
            pull: self.pull.clone(),
            released: self.released.clone(),
            in_flight: self.in_flight.clone(),
            displacement: self.displacement,
            refreshing: self.refreshing,
            status: Status::Idle,
            pull_at_transition: 0.0,
            snap: AnimationController::new(SNAP_DURATION),
            retract: AnimationController::new(RETRACT_DURATION),
            indicator_size: Size::new(INDICATOR_FALLBACK_DIAMETER, INDICATOR_FALLBACK_DIAMETER),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RefreshIndicatorWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.scroll, &self.scroll, &mut element.scroll, ctx);

        // Closures are not comparable — always reinstall the shared cells
        // (`ScrollWidget`'s own rebuild follows the identical discipline).
        element.pull = self.pull.clone();
        element.released = self.released.clone();
        element.in_flight = self.in_flight.clone();
        // A fresh cell defaults to `false`; re-sync it from the widget's own
        // authoritative status rather than trust the fresh value, so an
        // unrelated rebuild mid-refresh can never let a second pull re-fire
        // `on_refresh` (see the module docs' concurrency note).
        element
            .in_flight
            .set(matches!(element.status, Status::Refreshing));

        if (element.displacement - self.displacement).abs() > f64::EPSILON {
            element.displacement = self.displacement;
            flags |= ChangeFlags::PAINT;
        }

        match (&prev.indicator, &self.indicator) {
            (None, None) => {}
            (Some(p), Some(n)) => {
                let pod = element.indicator.as_mut().expect("indicator pod present");
                flags |= rebuild_child(p, n, pod, ctx);
            }
            (None, Some(n)) => {
                element.indicator = Some(build_child(n, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(p), None) => {
                let mut pod = element.indicator.take().expect("indicator pod present");
                teardown_child(p, &mut pod, ctx);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }

        // The controlled completion prop: a confirmed `refreshing: true ->
        // false` transition, while still showing the spinner, begins the
        // done-retract. Any other combination (still true, already
        // retracting/idle) is a no-op here.
        if !self.refreshing && element.refreshing && matches!(element.status, Status::Refreshing) {
            element.begin_retract(RetractKind::Done);
            flags |= ChangeFlags::PAINT;
        }
        element.refreshing = self.refreshing;

        flags
    }

    fn teardown(&self, element: &mut RefreshIndicatorWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.scroll, &mut element.scroll, ctx);
        if let (Some(view), Some(pod)) = (&self.indicator, element.indicator.as_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

/// The retained widget for a [`RefreshIndicatorView`]. See the
/// [module docs](self).
pub struct RefreshIndicatorWidget {
    scroll: ChildPod,
    indicator: Option<ChildPod>,
    pull: Rc<Cell<f64>>,
    released: Rc<Cell<bool>>,
    in_flight: Rc<Cell<bool>>,
    displacement: f64,
    refreshing: bool,
    status: Status,
    /// The pull distance (px) captured at the moment [`Status::Refreshing`]
    /// or [`Status::Retracting`]`(Canceled)` began — the `from` endpoint
    /// [`snap_pull`]/[`cancel_retract_pull`] lerp away from.
    pull_at_transition: f64,
    /// Drives [`Status::Refreshing`]'s snap-to-resting position.
    snap: AnimationController,
    /// Drives [`Status::Retracting`]'s position (canceled) or scale (done).
    retract: AnimationController,
    /// The spinner's natural size, cached from the last [`Widget::layout`]
    /// (defaults to [`INDICATOR_FALLBACK_DIAMETER`] before the first one).
    indicator_size: Size,
}

impl RefreshIndicatorWidget {
    fn max_pull(&self) -> f64 {
        self.displacement + self.indicator_size.height
    }

    /// Begin [`Status::Refreshing`]: snap the indicator from wherever the
    /// pull currently reads toward its resting position. The app's
    /// `on_refresh` has already been invoked by the installed closure at
    /// this point (see [`refresh_indicator`]) — this only starts the visual
    /// ramp and marks the in-flight guard.
    fn begin_refresh(&mut self) {
        self.pull_at_transition = visual_pull(self.pull.get(), self.max_pull());
        self.snap = AnimationController::new(SNAP_DURATION);
        self.snap.forward();
        self.status = Status::Refreshing;
        self.in_flight.set(true);
    }

    /// Begin [`RetractKind::Canceled`]: slide the indicator back out from
    /// wherever the pull currently reads.
    fn begin_cancel(&mut self) {
        self.pull_at_transition = visual_pull(self.pull.get(), self.max_pull());
        self.retract = AnimationController::new(RETRACT_DURATION);
        self.retract.forward();
        self.status = Status::Retracting(RetractKind::Canceled);
    }

    /// Begin a retract of `kind` (called for [`RetractKind::Done`] from
    /// [`RefreshIndicatorView::rebuild`] once the app confirms completion;
    /// [`RefreshIndicatorWidget::begin_cancel`] is the `Canceled` entry
    /// point instead, since that one starts from a live event rather than a
    /// prop change).
    fn begin_retract(&mut self, kind: RetractKind) {
        self.retract = AnimationController::new(RETRACT_DURATION);
        self.retract.forward();
        self.status = Status::Retracting(kind);
        self.in_flight.set(false);
    }
}

impl Widget for RefreshIndicatorWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.scroll.layout_child(ctx, bc);
        self.scroll.set_origin(Point::ORIGIN);
        if let Some(indicator) = self.indicator.as_mut() {
            let d = indicator.layout_child(ctx, &BoxConstraints::loose(size));
            self.indicator_size = d;
            indicator.set_origin(Point::new((size.width - d.width) / 2.0, 0.0));
        }
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (origin, size) = (ctx.origin(), ctx.size());
        self.scroll.paint_child(ctx, scene);

        let max_pull = self.max_pull();
        let (pull_px, scale) = match self.status {
            Status::Idle => return,
            Status::Pulling => (visual_pull(self.pull.get(), max_pull), 1.0),
            Status::Refreshing => {
                if self.snap.advance(ctx.frame_time()) {
                    ctx.request_frame();
                }
                (
                    snap_pull(self.pull_at_transition, max_pull, self.snap.value_clamped()),
                    1.0,
                )
            }
            Status::Retracting(kind) => {
                if self.retract.advance(ctx.frame_time()) {
                    ctx.request_frame();
                } else {
                    // Settled: retire back to idle on this same, final frame
                    // (the last one still paints the settled state below).
                    self.status = Status::Idle;
                    self.pull.set(0.0);
                }
                let t = self.retract.value_clamped();
                match kind {
                    RetractKind::Canceled => (cancel_retract_pull(self.pull_at_transition, t), 1.0),
                    RetractKind::Done => (max_pull, done_scale(t)),
                }
            }
        };

        let Some(indicator) = self.indicator.as_mut() else {
            return;
        };
        let indicator_h = self.indicator_size.height;
        let inset = pull_px - indicator_h;

        // A hard-edge clip over the whole box — the indicator's reveal slot,
        // matching a `Stack`'s default clip behavior (upstream relies on its
        // ancestor `Stack` for the same effect).
        scene.push_clip(origin, size);
        if scale < 1.0 {
            let cx = origin.x + indicator.origin().x + indicator.size().width / 2.0;
            let cy = origin.y + inset + indicator_h / 2.0;
            scene.push_transform(
                Affine::translate((cx, cy))
                    * Affine::scale(scale)
                    * Affine::translate((-cx, -cy))
                    * Affine::translate((0.0, inset)),
            );
        } else {
            scene.push_transform(Affine::translate((0.0, inset)));
        }
        indicator.paint_child(ctx, scene);
        scene.pop_transform();
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let result = route_event_single(&mut self.scroll, ctx, event);

        if self.released.replace(false) && !matches!(self.status, Status::Refreshing) {
            self.begin_refresh();
        } else {
            match self.status {
                Status::Idle => {
                    if self.pull.get() < 0.0 {
                        self.status = Status::Pulling;
                    }
                }
                Status::Pulling => {
                    let released_gesture = matches!(
                        event,
                        InputEvent::Pointer(p)
                            if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
                    );
                    if released_gesture {
                        self.begin_cancel();
                    }
                }
                Status::Refreshing | Status::Retracting(_) => {}
            }
        }

        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The scroll surface contributes the node; the indicator is
        // decoration (the same call `frust_shadcn::scroll_area` makes for
        // its own overlay).
        self.scroll.semantics_child(ctx);
    }

    visit_children!(scroll, indicator);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::{
        BezPath, Brush, Color, PaintCtx as TestPaintCtx, PointerButton, PointerEvent,
    };
    use std::any::Any;

    // -----------------------------------------------------------------------
    // Pull-progress mapping (pure functions) — AC1.
    // -----------------------------------------------------------------------

    #[test]
    fn visual_pull_tracks_overscroll_and_clamps_to_max_pull() {
        assert_eq!(visual_pull(0.0, 88.0), 0.0, "in range: no reveal");
        assert_eq!(visual_pull(-10.0, 88.0), 10.0);
        assert_eq!(visual_pull(-64.0, 88.0), 64.0, "exactly at the trigger");
        assert_eq!(
            visual_pull(-500.0, 88.0),
            88.0,
            "clamps to the resting-position cap"
        );
        assert_eq!(
            visual_pull(5.0, 88.0),
            0.0,
            "past-bottom overscroll is not a pull"
        );
    }

    #[test]
    fn snap_pull_lerps_from_the_captured_pull_to_the_resting_position() {
        assert_eq!(snap_pull(20.0, 88.0, 0.0), 20.0, "t=0: still at the pull");
        assert_eq!(snap_pull(20.0, 88.0, 1.0), 88.0, "t=1: fully at rest");
        assert_eq!(snap_pull(20.0, 88.0, 0.5), 54.0, "t=0.5: halfway");
        assert_eq!(snap_pull(0.0, 88.0, 2.0), 88.0, "t is clamped past 1");
    }

    #[test]
    fn cancel_retract_pull_lerps_the_captured_pull_back_to_zero() {
        assert_eq!(cancel_retract_pull(40.0, 0.0), 40.0);
        assert_eq!(cancel_retract_pull(40.0, 1.0), 0.0);
        assert_eq!(cancel_retract_pull(40.0, 0.5), 20.0);
        assert_eq!(
            cancel_retract_pull(40.0, -1.0),
            40.0,
            "t is clamped below 0"
        );
    }

    #[test]
    fn done_scale_shrinks_from_one_to_zero() {
        assert_eq!(done_scale(0.0), 1.0);
        assert_eq!(done_scale(1.0), 0.0);
        assert_eq!(done_scale(0.25), 0.75);
    }

    // -----------------------------------------------------------------------
    // Lifecycle fixtures — AC2/AC3.
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct AppState {
        refreshes: u32,
        refreshing: bool,
    }

    #[derive(Clone, Copy)]
    struct Content(Size);
    struct ContentWidget(Size);

    impl<S: 'static> View<S> for Content {
        type Element = ContentWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ContentWidget {
            ContentWidget(self.0)
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ContentWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for ContentWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    const VIEWPORT: Size = Size::new(200.0, 400.0);
    const CONTENT: Size = Size::new(200.0, 1000.0);

    fn build_view(on_refresh: impl Fn(&mut AppState) + 'static) -> RefreshIndicatorView<AppState> {
        refresh_indicator(Content(CONTENT), on_refresh)
    }

    fn build(view: &RefreshIndicatorView<AppState>) -> RefreshIndicatorWidget {
        let mut counter = 0u64;
        let mut w = View::<AppState>::build(view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(VIEWPORT));
        w
    }

    fn ptr(phase: PointerPhase, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(50.0, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut RefreshIndicatorWidget, state: &mut AppState, event: &InputEvent) {
        let sa: &mut dyn Any = state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, VIEWPORT);
        w.event(&mut ctx, event);
    }

    /// Drags past the baseline's refresh trigger (raw -200 -> resisted -100,
    /// past the 64px threshold — the same sequence
    /// `frust_widgets::scroll`'s own `on_refresh_release` test uses) and
    /// releases.
    fn drag_past_trigger(w: &mut RefreshIndicatorWidget, state: &mut AppState) {
        dispatch(w, state, &ptr(PointerPhase::Down, 50.0));
        dispatch(w, state, &ptr(PointerPhase::Move, 90.0)); // takeover
        dispatch(w, state, &ptr(PointerPhase::Move, 290.0)); // past trigger
        dispatch(w, state, &ptr(PointerPhase::Up, 290.0));
    }

    #[derive(Default)]
    struct Recorder {
        fills: usize,
        rrects: usize,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_path(&mut self, _o: Point, _p: &BezPath, _b: &Brush) {
            self.fills += 1;
        }
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {
            self.rrects += 1;
        }
    }

    fn paint_at(w: &mut RefreshIndicatorWidget, t_ms: u64) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = TestPaintCtx::for_test(
            Point::ZERO,
            VIEWPORT,
            FrameTime::from_nanos(t_ms * 1_000_000),
        );
        w.paint(&mut ctx, &mut rec);
        rec
    }

    // -----------------------------------------------------------------------
    // Release-trigger + refreshing-state lifecycle — AC2.
    // -----------------------------------------------------------------------

    #[test]
    fn a_pull_under_the_trigger_does_not_refresh_and_retracts() {
        let view = build_view(|_| {});
        let mut w = build(&view);
        let mut state = AppState::default();
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Down, 50.0));
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Move, 90.0)); // takeover
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Move, 180.0)); // raw -90 -> -45
        assert_eq!(w.status, Status::Pulling);
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Up, 180.0));

        assert_eq!(state.refreshes, 0, "released under the trigger");
        assert_eq!(w.status, Status::Retracting(RetractKind::Canceled));

        paint_at(&mut w, 0); // seed
        paint_at(&mut w, 300); // past RETRACT_DURATION (200ms)
        assert_eq!(w.status, Status::Idle, "settles back to idle");
    }

    #[test]
    fn a_pull_past_the_trigger_fires_on_refresh_exactly_once_and_holds_while_refreshing() {
        let view = build_view(|s: &mut AppState| {
            s.refreshes += 1;
            s.refreshing = true;
        });
        let mut w = build(&view);
        let mut state = AppState::default();
        drag_past_trigger(&mut w, &mut state);

        assert_eq!(state.refreshes, 1);
        assert!(state.refreshing, "on_refresh flipped the app's own flag");
        assert_eq!(w.status, Status::Refreshing);

        // The spinner holds at rest regardless of how much time passes,
        // until the app confirms completion.
        paint_at(&mut w, 0);
        paint_at(&mut w, 1000);
        assert_eq!(w.status, Status::Refreshing, "still holding");
    }

    #[test]
    fn cancel_mid_pull_abandons_without_firing_refresh() {
        let view = build_view(|s: &mut AppState| s.refreshes += 1);
        let mut w = build(&view);
        let mut state = AppState::default();
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Down, 50.0));
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Move, 90.0));
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Move, 290.0)); // past trigger
        assert_eq!(w.status, Status::Pulling, "not yet released");
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Cancel, 290.0));

        assert_eq!(
            state.refreshes, 0,
            "a Cancel never fires the refresh callback"
        );
        assert_eq!(w.status, Status::Retracting(RetractKind::Canceled));
    }

    #[test]
    fn a_second_pull_while_refreshing_does_not_refire_on_refresh() {
        let view = build_view(|s: &mut AppState| {
            s.refreshes += 1;
            s.refreshing = true;
        });
        let mut w = build(&view);
        let mut state = AppState::default();
        drag_past_trigger(&mut w, &mut state);
        assert_eq!(state.refreshes, 1);
        assert_eq!(w.status, Status::Refreshing);

        // A second pull, released past the trigger again, while the app has
        // not yet confirmed completion.
        drag_past_trigger(&mut w, &mut state);
        assert_eq!(
            state.refreshes, 1,
            "a refresh already in flight is not re-triggered"
        );
    }

    #[test]
    fn refreshing_confirmed_false_ends_the_spinner_and_settles_to_idle() {
        let make = || {
            build_view(|s: &mut AppState| {
                s.refreshes += 1;
                s.refreshing = true;
            })
        };
        let view1 = make();
        let mut w = build(&view1);
        let mut state = AppState::default();
        drag_past_trigger(&mut w, &mut state);
        assert_eq!(w.status, Status::Refreshing);

        // Next rebuild: the app's `refreshing = true` is fed straight back —
        // no transition yet.
        let view2 = make().refreshing(true);
        let mut counter = 1u64;
        View::<AppState>::rebuild(&view2, &view1, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.status, Status::Refreshing, "still confirmed in-flight");

        // The app's async work completes; the next rebuild confirms
        // `refreshing = false`.
        let view3 = make().refreshing(false);
        View::<AppState>::rebuild(&view3, &view2, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.status, Status::Retracting(RetractKind::Done));

        paint_at(&mut w, 0);
        paint_at(&mut w, 300); // past RETRACT_DURATION
        assert_eq!(w.status, Status::Idle);

        // The in-flight guard released too: a fresh pull can refresh again.
        drag_past_trigger(&mut w, &mut state);
        assert_eq!(state.refreshes, 2);
    }

    // -----------------------------------------------------------------------
    // Variants render — AC3.
    // -----------------------------------------------------------------------

    #[test]
    fn idle_paints_only_the_wrapped_content() {
        let view = build_view(|_| {});
        let mut w = build(&view);
        let rec = paint_at(&mut w, 0);
        assert_eq!(rec.fills, 0);
        assert_eq!(rec.rrects, 0);
    }

    #[test]
    fn pulling_reveals_the_default_expressive_spinner_with_no_container() {
        let view = build_view(|_| {});
        let mut w = build(&view);
        let mut state = AppState::default();
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Down, 50.0));
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Move, 90.0));
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Move, 150.0));
        assert_eq!(w.status, Status::Pulling);

        let rec = paint_at(&mut w, 0);
        assert_eq!(rec.fills, 1, "the morphing shape paints");
        assert_eq!(rec.rrects, 0, "the default variant paints no container");
    }

    #[test]
    fn contained_variant_paints_a_container_behind_the_shape() {
        let view = build_view(|_| {}).contained();
        let mut w = build(&view);
        let mut state = AppState::default();
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Down, 50.0));
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Move, 90.0));
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Move, 150.0));

        let rec = paint_at(&mut w, 0);
        assert_eq!(rec.fills, 1);
        assert_eq!(rec.rrects, 1, "the contained variant paints its container");
    }

    #[test]
    fn no_spinner_paints_nothing_extra_through_the_whole_lifecycle() {
        let view = build_view(|s: &mut AppState| s.refreshing = true).no_spinner();
        let mut w = build(&view);
        let mut state = AppState::default();

        dispatch(&mut w, &mut state, &ptr(PointerPhase::Down, 50.0));
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Move, 90.0));
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Move, 150.0));
        assert_eq!(w.status, Status::Pulling, "the lifecycle still tracks");
        let rec = paint_at(&mut w, 0);
        assert_eq!(rec.fills, 0);
        assert_eq!(rec.rrects, 0);

        dispatch(&mut w, &mut state, &ptr(PointerPhase::Move, 290.0));
        dispatch(&mut w, &mut state, &ptr(PointerPhase::Up, 290.0));
        assert_eq!(
            w.status,
            Status::Refreshing,
            "the trigger contract still fires"
        );
        assert!(state.refreshing);
        let rec = paint_at(&mut w, 0);
        assert_eq!(rec.fills, 0, "still nothing painted");
        assert_eq!(rec.rrects, 0);
    }

    #[test]
    fn displacement_and_indicator_height_set_the_resting_pull_cap() {
        let view = build_view(|_| {}).displacement(100.0);
        let w = build(&view);
        assert_eq!(w.max_pull(), 100.0 + INDICATOR_FALLBACK_DIAMETER_FOR_TEST);
    }

    /// Mirrors [`INDICATOR_FALLBACK_DIAMETER`] for the one geometry
    /// assertion above — [`Widget::layout`] always runs before this test
    /// reads `max_pull`, via [`build`], so `indicator_size` already reflects
    /// the real (identical) 48×48 spinner size; kept as its own named
    /// constant so the assertion doesn't silently drift if the fallback
    /// value ever changes without the real spinner's size changing too.
    const INDICATOR_FALLBACK_DIAMETER_FOR_TEST: f64 = 48.0;
}
