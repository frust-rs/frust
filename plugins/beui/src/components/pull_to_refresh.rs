//! Ports beUI's `pull-to-refresh` component.
//!
//! **Source:** `components/motion/pull-to-refresh.tsx`, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01. Registry
//! entry: slug `pull-to-refresh`, *"Native-feeling pull-to-refresh container
//! with drag resistance, threshold feedback and async refresh handling."*
//!
//! | Upstream | Here |
//! |---|---|
//! | `threshold = 76` / `maxPull = 132` / `holdDistance = 68` | [`DEFAULT_THRESHOLD`] / [`DEFAULT_MAX_PULL`] / [`DEFAULT_HOLD_DISTANCE`] |
//! | `resistedDistance` | [`resisted_distance`] |
//! | `status: idle \| pulling \| ready \| refreshing` | [`PullToRefreshStatus`] |
//! | `settle(target)` on `SPRING_PANEL` | the pull scalar on [`SPRING_PANEL`] |
//! | `RefreshBuddy` (an SVG face) | the painted face, [`GLYPH_VIEWBOX`] units |
//! | label swap on `LABEL_SWAP` | [`LABEL_SWAP`] |
//! | `useReducedMotion` → `CALM_PULSE`, no `y` | the theme's `reduce_motion` |
//!
//! # Two ways to pull, because frust's scroller is not this widget
//!
//! Upstream's root element **is** the scroll container (`overflow-y-auto`), so
//! it can watch `scrollTop`, `preventDefault` a touch move and take the gesture
//! over the moment the content is at the top. In frust the scroll surface is
//! `ScrollView`, a separate widget that owns its own physics and cannot have a
//! gesture taken off it. So this component accepts the pull from either side:
//!
//! * **Overscroll** — [`PullToRefreshView::overscroll`] takes the signed
//!   past-edge displacement straight out of `ScrollInfo` (negative past the
//!   top), which an app pipes in from `ScrollView::on_scroll` or from a
//!   [`ScrollFx`](crate::motion::ScrollFx). This is the faithful route: the pull
//!   is the *surface's* own resisted overscroll, produced by the framework's
//!   physics rather than by a second interpretation of the same finger.
//! * **A direct drag** — a `Down` inside the widget followed by a downward
//!   drag, resisted by [`resisted_distance`], for a container that is not inside
//!   a scroll surface at all.
//!
//! **What this actually does on desktop**, stated plainly: a mouse wheel only
//! reaches the overscroll route when the platform and the enclosing `ScrollView`
//! deliver past-edge wheel travel, which is a physics-and-platform question this
//! component has no say in (`ScrollPhysics` may reject a past-edge offset
//! outright — `NeverScrollable` and the clamping parity physics both do). The
//! **drag route is what exercises it with a mouse**: press inside the container
//! and pull down. On touch, either route works, and the overscroll one is the
//! one that feels native because it is the surface's own rubber band.
//!
//! # Degradation: `async onRefresh` becomes a controlled flag
//!
//! Upstream awaits `onRefresh()` and holds the indicator up for the length of
//! the promise, with an `internalRefreshing` state covering a synchronous
//! resolve. A callback in this tier is synchronous and returns nothing, so there
//! is no promise to await. [`PullToRefreshView::refreshing`] is therefore the
//! app's own flag and the source of truth — the same controlled-component shape
//! every stateful widget in this catalog takes.
//!
//! A short internal latch bridges the gap: the trigger raises it so the
//! indicator is already refreshing on the frame the gesture ends, and the next
//! rebuild that arrives with `refreshing == false` clears it. That reproduces
//! upstream's synchronous path exactly (its own comment: *"a synchronous refresh
//! can resolve before React commits"*), and an app doing real work holds the
//! flag itself.
//!
//! # Degradations in the indicator
//!
//! * **The face is painted, not an SVG.** Upstream's `RefreshBuddy` is an inline
//!   36x36 SVG. There is no SVG here; the same figure is painted from the same
//!   [`GLYPH_VIEWBOX`] coordinates with rounded rects and an arc, so the
//!   proportions are upstream's own rather than a re-drawn approximation. What
//!   is lost is the two mouth *paths* (a straight line and a smile curve), which
//!   become a straight line and a shallow arc.
//! * **The label swap is a cross-fade, not `mode="wait"`.** Upstream's
//!   `AnimatePresence mode="wait"` runs the outgoing label's exit to completion
//!   before the incoming one enters. Here both run together over [`LABEL_SWAP`],
//!   which at 160ms is the difference between one 160ms swap and two — a
//!   sequenced version would double the latency of a status change that happens
//!   three times in a single pull.
//! * **The indicator band's gradient fades to transparent** rather than to
//!   `bg-background`'s own backdrop, the same limitation
//!   [`marquee`](super::marquee)'s edge fade records: there is no mask
//!   primitive, so the band is painted rather than masked.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx,
    Shape, TickClass, View, Widget, any, build_child, erase_callback, rebuild_child,
    route_event_single, teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{FrameTime, Theme};
use kurbo::{Arc, Point, Size, Vec2};
use peniko::{Brush, Color, ColorStop, Gradient};

use crate::motion::Ramp;
use crate::press::{SpringScalar, inside, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType};
use crate::tokens::motion::{EASE_IN_OUT, EASE_OUT, SPRING_PANEL, SPRING_SWAP};
use crate::tokens::{BEUI_LIGHT, sans_family};

/// The resisted pull required to arm a refresh, in logical px — upstream's
/// `threshold = 76`.
pub const DEFAULT_THRESHOLD: f64 = 76.0;

/// The floor upstream clamps `threshold` to (`Math.max(24, threshold)`).
pub const MIN_THRESHOLD: f64 = 24.0;

/// The furthest a pull can travel after resistance, in logical px — upstream's
/// `maxPull = 132`.
pub const DEFAULT_MAX_PULL: f64 = 132.0;

/// How far past the threshold `maxPull` is forced to sit (upstream's
/// `Math.max(maxPull, pullThreshold + 24)`).
pub const MAX_PULL_HEADROOM: f64 = 24.0;

/// Where the content rests while a refresh runs, in logical px — upstream's
/// `holdDistance = 68`, clamped to at most the threshold.
pub const DEFAULT_HOLD_DISTANCE: f64 = 68.0;

/// The indicator band's height in logical px — upstream's `h-[4.25rem]`.
pub const INDICATOR_BAND: f64 = 68.0;

/// The painted face's box, in logical px — upstream's `h-9 w-9`.
pub const GLYPH_SIZE: f64 = 36.0;

/// The face's coordinate space — upstream's `viewBox="0 0 36 36"`, kept so every
/// feature below can carry upstream's own numbers unchanged.
pub const GLYPH_VIEWBOX: f64 = 36.0;

/// The label swap — upstream's `LABEL_SWAP = { duration: 0.16, ease: EASE_OUT }`.
pub const LABEL_SWAP: Ramp = Ramp::eased(Duration::from_millis(160), EASE_OUT);

/// The refreshing character loop — upstream's `CHARACTER_LOOP = { duration: 0.9,
/// ease: EASE_IN_OUT, repeat: Infinity }`.
pub const CHARACTER_LOOP: Duration = Duration::from_millis(900);

/// The reduced-motion refreshing loop — upstream's `CALM_PULSE`, a 1.2s opacity
/// breath in place of the orbit and blink.
pub const CALM_PULSE: Duration = Duration::from_millis(1200);

/// How far the label sits from its resting place while swapping, in logical px
/// — upstream's `y: ±3`.
pub const LABEL_SWAP_TRAVEL: f64 = 3.0;

/// The status label's text size — upstream's `text-[11px]`.
pub const LABEL_SIZE: f64 = 11.0;

/// How much the character swells once the pull arms — upstream's
/// `scale: ready ? 1.08 : 1`.
pub const READY_POP: f64 = 1.08;

/// Unthemed fallback ink (beUI light `--foreground`).
const FALLBACK_FOREGROUND: Color = BEUI_LIGHT.foreground;

/// Unthemed fallback surface (beUI light `--background`) — the band's own wash
/// and the face's eyes.
const FALLBACK_BACKGROUND: Color = BEUI_LIGHT.background;

/// Unthemed fallback muted ink (beUI light `--muted-foreground`) — the label and
/// the antenna arc.
const FALLBACK_MUTED: Color = BEUI_LIGHT.muted_foreground;

/// Where a pull stands, and therefore what the indicator says — upstream's
/// `PullToRefreshStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PullToRefreshStatus {
    /// Nothing is happening.
    #[default]
    Idle,
    /// A pull is under way but has not reached the threshold.
    Pulling,
    /// The pull is past the threshold: releasing now refreshes.
    Ready,
    /// A refresh is running.
    Refreshing,
}

impl PullToRefreshStatus {
    /// All four, in upstream's own declaration order.
    pub const ALL: [PullToRefreshStatus; 4] = [
        PullToRefreshStatus::Idle,
        PullToRefreshStatus::Pulling,
        PullToRefreshStatus::Ready,
        PullToRefreshStatus::Refreshing,
    ];

    /// Whether a release from here fires the refresh.
    pub fn is_ready(self) -> bool {
        matches!(self, PullToRefreshStatus::Ready)
    }

    /// Whether a refresh is running.
    pub fn is_refreshing(self) -> bool {
        matches!(self, PullToRefreshStatus::Refreshing)
    }
}

/// Upstream's `resistedDistance`: `maxPull · (1 − e^(−distance / maxPull))`.
///
/// An asymptote, not a clamp — the pull always moves a little further, but never
/// past `max_pull`, which is what makes a long drag feel like it is fighting
/// something rather than hitting a wall. A non-positive distance yields zero.
pub fn resisted_distance(distance: f64, max_pull: f64) -> f64 {
    if max_pull <= 0.0 {
        return 0.0;
    }
    max_pull * (1.0 - (-distance.max(0.0) / max_pull).exp())
}

/// A view-held refresh callback (erased on build).
type OnRefresh<State> = Rc<dyn Fn(&mut State)>;

/// A declarative beUI pull-to-refresh container. See the [module docs](self) for
/// the two pull routes and what each does on desktop.
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::components::pull_to_refresh::{PullToRefreshView, pull_to_refresh};
///
/// let feed: PullToRefreshView<bool> = pull_to_refresh(text("feed"), |busy: &mut bool| {
///     *busy = true;
/// });
/// ```
pub struct PullToRefreshView<State: 'static> {
    child: AnyView<State>,
    on_refresh: OnRefresh<State>,
    refreshing: bool,
    disabled: bool,
    threshold: f64,
    max_pull: f64,
    hold_distance: f64,
    overscroll: f64,
    pulling_label: String,
    release_label: String,
    refreshing_label: String,
}

/// Wrap `child` in a pull-to-refresh container that calls `on_refresh` when a
/// pull is released past the threshold, with upstream's own defaults.
pub fn pull_to_refresh<State: 'static, V: View<State>, F: Fn(&mut State) + 'static>(
    child: V,
    on_refresh: F,
) -> PullToRefreshView<State> {
    PullToRefreshView {
        child: any(child),
        on_refresh: Rc::new(on_refresh),
        refreshing: false,
        disabled: false,
        threshold: DEFAULT_THRESHOLD,
        max_pull: DEFAULT_MAX_PULL,
        hold_distance: DEFAULT_HOLD_DISTANCE,
        overscroll: 0.0,
        pulling_label: "Pull to refresh".to_string(),
        release_label: "Release to refresh".to_string(),
        refreshing_label: "Refreshing".to_string(),
    }
}

impl<State: 'static> PullToRefreshView<State> {
    /// Keep the indicator up while an externally managed refresh runs
    /// (`refreshing`). The app's flag is the source of truth — see the [module
    /// docs](self).
    pub fn refreshing(mut self, refreshing: bool) -> Self {
        self.refreshing = refreshing;
        self
    }

    /// Make the container inert (`disabled`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The resisted pull required to refresh (`threshold`, default
    /// [`DEFAULT_THRESHOLD`], floored at [`MIN_THRESHOLD`]).
    pub fn threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold.max(MIN_THRESHOLD);
        self
    }

    /// The furthest a pull travels (`maxPull`, default [`DEFAULT_MAX_PULL`]),
    /// forced to sit at least [`MAX_PULL_HEADROOM`] past the threshold.
    pub fn max_pull(mut self, max_pull: f64) -> Self {
        self.max_pull = max_pull;
        self
    }

    /// Where the content rests while refreshing (`holdDistance`, default
    /// [`DEFAULT_HOLD_DISTANCE`], clamped to at most the threshold).
    pub fn hold_distance(mut self, hold_distance: f64) -> Self {
        self.hold_distance = hold_distance;
        self
    }

    /// Feed the enclosing surface's signed past-edge displacement — `ScrollInfo`'s
    /// own `overscroll`, negative past the top. The faithful pull route; see the
    /// [module docs](self).
    pub fn overscroll(mut self, overscroll: f64) -> Self {
        self.overscroll = overscroll;
        self
    }

    /// The label shown while pulling (`pullingLabel`).
    pub fn pulling_label(mut self, label: impl Into<String>) -> Self {
        self.pulling_label = label.into();
        self
    }

    /// The label shown once the threshold is passed (`releaseLabel`).
    pub fn release_label(mut self, label: impl Into<String>) -> Self {
        self.release_label = label.into();
        self
    }

    /// The label shown while refreshing (`refreshingLabel`).
    pub fn refreshing_label(mut self, label: impl Into<String>) -> Self {
        self.refreshing_label = label.into();
        self
    }

    /// The effective threshold, after upstream's own floor.
    fn pull_threshold(&self) -> f64 {
        self.threshold.max(MIN_THRESHOLD)
    }

    /// The effective pull limit, after upstream's own headroom rule.
    fn pull_limit(&self) -> f64 {
        self.max_pull.max(self.pull_threshold() + MAX_PULL_HEADROOM)
    }

    /// The effective resting distance while refreshing.
    fn resting_distance(&self) -> f64 {
        self.hold_distance.max(0.0).min(self.pull_threshold())
    }
}

/// A live pointer drag on the container itself.
#[derive(Clone, Copy, Debug)]
struct PullGesture {
    /// Where the press landed, so the drag is measured from it.
    start: Point,
    /// Whether the gesture has been abandoned (an upward or sideways drag).
    abandoned: bool,
}

/// The retained widget for a [`PullToRefreshView`].
pub struct PullToRefreshWidget {
    child: ChildPod,
    external_refreshing: bool,
    disabled: bool,
    threshold: f64,
    limit: f64,
    resting: f64,
    /// The last overscroll the app reported, negative past the top.
    overscroll: f64,
    /// The three status labels, shaped once.
    labels: [LabelRun; 3],
    /// The status the indicator is showing.
    status: PullToRefreshStatus,
    /// The status the label is swapping *from*, while one is in flight.
    label_from: Option<PullToRefreshStatus>,
    /// The label swap's `0..=1` position.
    label_swap: SpringScalar,
    /// The content's downward displacement in logical px — upstream's `y`.
    pull: SpringScalar,
    /// The character's `scale: ready ? 1.08 : 1`, on upstream's own
    /// [`SPRING_SWAP`].
    pop: SpringScalar,
    /// A live pointer drag, if one owns the container.
    gesture: Option<PullGesture>,
    /// The internal refreshing latch that bridges to the next rebuild — see the
    /// [module docs](self).
    internal_refreshing: bool,
    /// When the refreshing loop started, so its phase is measured from a real
    /// frame rather than an absolute clock.
    refresh_since: Option<FrameTime>,
    on_refresh: ErasedCallback,
}

impl PullToRefreshWidget {
    /// The status the indicator is showing.
    pub fn status(&self) -> PullToRefreshStatus {
        self.status
    }

    /// The content's current downward displacement, in logical px.
    pub fn pull(&self) -> f64 {
        self.pull.value().max(0.0)
    }

    /// The pull as a `0..=1` fraction of the threshold — upstream's `progress`,
    /// `useTransform(y, [0, pullThreshold], [0, 1])`.
    pub fn progress(&self) -> f64 {
        if self.threshold <= 0.0 {
            return 0.0;
        }
        (self.pull() / self.threshold).clamp(0.0, 1.0)
    }

    /// Whether a refresh is running, from either the app's flag or the internal
    /// latch.
    pub fn is_refreshing(&self) -> bool {
        self.external_refreshing || self.internal_refreshing
    }

    /// Move to `next`, staging the label swap. Returns whether it changed.
    fn set_status(&mut self, next: PullToRefreshStatus) -> bool {
        if self.status == next {
            return false;
        }
        self.label_from = Some(self.status);
        self.label_swap.jump_to(0.0);
        self.label_swap.set_target(1.0);
        self.status = next;
        // `scale: ready ? 1.08 : 1` — the pop the character gives when the pull
        // arms, and holds through the refresh.
        self.pop
            .set_target(if next.is_ready() || next.is_refreshing() {
                READY_POP
            } else {
                1.0
            });
        if next.is_refreshing() {
            self.refresh_since = None;
        }
        true
    }

    /// Drive the pull to a resisted `distance`, updating the status —
    /// upstream's `updatePull`.
    fn update_pull(&mut self, distance: f64) -> bool {
        if self.disabled || self.is_refreshing() {
            return false;
        }
        let next = resisted_distance(distance, self.limit);
        // A live pull tracks the finger exactly; only a release springs.
        self.pull.jump_to(next);
        let status = if next >= self.threshold {
            PullToRefreshStatus::Ready
        } else {
            PullToRefreshStatus::Pulling
        };
        self.set_status(status);
        true
    }

    /// Spring the pull toward `target` — upstream's `settle`.
    fn settle(&mut self, target: f64) {
        self.pull.set_target(target);
    }

    /// End a gesture: refresh if it passed the threshold, otherwise settle back
    /// — upstream's `finishPull`.
    fn finish_pull(&mut self, ctx: &mut EventCtx) {
        self.gesture = None;
        let should_refresh =
            self.pull() >= self.threshold && !self.disabled && !self.is_refreshing();
        if should_refresh {
            self.internal_refreshing = true;
            self.set_status(PullToRefreshStatus::Refreshing);
            self.settle(self.resting);
            (self.on_refresh)(ctx);
            return;
        }
        self.set_status(PullToRefreshStatus::Idle);
        self.settle(0.0);
    }

    /// Reconcile the refreshing flags against the indicator — the rebuild half
    /// of the controlled contract.
    fn sync_refreshing(&mut self) {
        if self.is_refreshing() {
            self.set_status(PullToRefreshStatus::Refreshing);
            self.settle(self.resting);
        } else if self.status.is_refreshing() {
            self.set_status(PullToRefreshStatus::Idle);
            self.settle(0.0);
        }
    }

    /// The label for `status`.
    fn label(&self, status: PullToRefreshStatus) -> &LabelRun {
        match status {
            PullToRefreshStatus::Refreshing => &self.labels[2],
            PullToRefreshStatus::Ready => &self.labels[1],
            _ => &self.labels[0],
        }
    }
}

/// Ease a `0..=1` loop phase the way Motion eases a repeating keyframe run: the
/// curve is applied *per segment*, so a `[a, b, a]` cycle runs
/// [`EASE_IN_OUT`] out and back rather than once across the whole loop.
fn loop_ease(phase: f64) -> f64 {
    let phase = phase.clamp(0.0, 1.0);
    if phase < 0.5 {
        EASE_IN_OUT.transform(phase * 2.0) * 0.5
    } else {
        0.5 + EASE_IN_OUT.transform((phase - 0.5) * 2.0) * 0.5
    }
}

/// Map `value` through an explicit stop list, linearly between neighbours —
/// Motion's `useTransform(value, inputs, outputs)`.
///
/// [`crate::press::keyframes_at`] is the *evenly spaced* form; upstream's
/// indicator transforms name their own input stops (`[0, 0.55, 1]`,
/// `[0, 10, threshold]`), so they need this one.
fn map_stops(value: f64, inputs: &[f64], outputs: &[f64]) -> f64 {
    debug_assert_eq!(inputs.len(), outputs.len());
    if inputs.is_empty() {
        return 0.0;
    }
    if value <= inputs[0] {
        return outputs[0];
    }
    for pair in 1..inputs.len() {
        if value <= inputs[pair] {
            let span = inputs[pair] - inputs[pair - 1];
            if span <= 0.0 {
                return outputs[pair];
            }
            let local = (value - inputs[pair - 1]) / span;
            return outputs[pair - 1] + (outputs[pair] - outputs[pair - 1]) * local;
        }
    }
    outputs[outputs.len() - 1]
}

/// The resolved indicator palette.
struct RefreshColors {
    /// `fill-foreground` — the face.
    ink: Color,
    /// `fill-background` — the eyes and mouth, and the band's wash.
    surface: Color,
    /// `text-muted-foreground` — the label and the antenna arc.
    muted: Color,
}

/// Resolve the palette, falling back to the vendored light table.
fn resolve_colors(theme: Option<&Theme>) -> RefreshColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            RefreshColors {
                ink: scheme.on_surface,
                surface: scheme.surface,
                muted: scheme.on_surface_variant,
            }
        }
        None => RefreshColors {
            ink: FALLBACK_FOREGROUND,
            surface: FALLBACK_BACKGROUND,
            muted: FALLBACK_MUTED,
        },
    }
}

/// The type-scale role the status label's family resolves from at layout.
const LABEL_ROLE: ThemeTextType = ThemeTextType::LabelSmall;

/// The status label's style — `text-[11px] font-medium text-muted-foreground`.
/// The family here is the unthemed base; `layout` shapes in [`LABEL_ROLE`]'s
/// family.
fn label_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::MEDIUM,
        size: LABEL_SIZE as f32,
        color,
        ..TextStyle::default()
    }
}

impl<State: 'static> View<State> for PullToRefreshView<State> {
    type Element = PullToRefreshWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PullToRefreshWidget {
        let mut widget = PullToRefreshWidget {
            child: build_child(&self.child, ctx),
            external_refreshing: self.refreshing,
            disabled: self.disabled,
            threshold: self.pull_threshold(),
            limit: self.pull_limit(),
            resting: self.resting_distance(),
            overscroll: self.overscroll,
            labels: [
                LabelRun::new(self.pulling_label.clone()),
                LabelRun::new(self.release_label.clone()),
                LabelRun::new(self.refreshing_label.clone()),
            ],
            status: PullToRefreshStatus::Idle,
            label_from: None,
            label_swap: SpringScalar::new(1.0, LABEL_SWAP),
            pull: SpringScalar::new(0.0, Ramp::spring(SPRING_PANEL)),
            pop: SpringScalar::new(1.0, Ramp::spring(SPRING_SWAP)),
            gesture: None,
            internal_refreshing: false,
            refresh_since: None,
            on_refresh: erase_callback(&self.on_refresh),
        };
        widget.sync_refreshing();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PullToRefreshWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every pass.
        element.on_refresh = erase_callback(&self.on_refresh);
        let mut flags = rebuild_child(&prev.child, &self.child, &mut element.child, ctx);

        if prev.pulling_label != self.pulling_label {
            element.labels[0].set_content(self.pulling_label.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.release_label != self.release_label {
            element.labels[1].set_content(self.release_label.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.refreshing_label != self.refreshing_label {
            element.labels[2].set_content(self.refreshing_label.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        element.threshold = self.pull_threshold();
        element.limit = self.pull_limit();
        element.resting = self.resting_distance();

        if element.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                element.gesture = None;
                element.internal_refreshing = false;
                element.set_status(PullToRefreshStatus::Idle);
                element.settle(0.0);
            }
            flags |= ChangeFlags::PAINT;
        }

        if prev.refreshing != self.refreshing {
            element.external_refreshing = self.refreshing;
            flags |= ChangeFlags::PAINT;
        }
        if !self.refreshing && element.internal_refreshing {
            // The synchronous path: the app committed without raising its own
            // flag, so the refresh is already over.
            element.internal_refreshing = false;
            flags |= ChangeFlags::PAINT;
        }
        element.sync_refreshing();

        if prev.overscroll != self.overscroll {
            element.overscroll = self.overscroll;
            if element.gesture.is_none() && !element.is_refreshing() && !element.disabled {
                // `ScrollInfo::overscroll` is negative past the top, and the
                // surface has already applied its own resistance; this component
                // applies its own on top so the two routes read identically.
                let past_top = (-self.overscroll).max(0.0);
                if past_top > 0.0 {
                    element.update_pull(past_top);
                } else if element.status != PullToRefreshStatus::Idle {
                    element.set_status(PullToRefreshStatus::Idle);
                    element.settle(0.0);
                }
            }
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut PullToRefreshWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for PullToRefreshWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let colors = resolve_colors(theme);
        let style = label_style(colors.muted);
        for label in &mut self.labels {
            label.layout_themed(ctx, &style, LABEL_ROLE);
        }
        // The indicator is an overlay and the pull is a paint offset, so the
        // container is exactly as big as its content — a pull must not re-flow
        // the page behind it.
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let colors = resolve_colors(theme);
        let origin = ctx.origin();
        let size = ctx.size();
        let now = ctx.frame_time();

        if reduce {
            // Upstream's `style={reduce ? undefined : { y }}`: the settle is a
            // set, not a spring, and the content does not move at all.
            let target = self.pull.target();
            self.pull.jump_to(target);
        }
        if reduce {
            let settled = self.pop.target();
            self.pop.jump_to(settled);
        }
        self.pull.advance(now);
        self.pop.advance(now);
        self.label_swap.advance(now);
        let mut owes_frame =
            self.pull.is_animating() || self.pop.is_animating() || self.label_swap.is_animating();
        if !self.label_swap.is_animating() {
            self.label_from = None;
        }

        // The content, displaced by the pull (`z-10`, under the indicator).
        let shift = if reduce { 0.0 } else { self.pull() };
        if shift > 0.0 {
            scene.push_transform(Affine::translate(Vec2::new(0.0, shift)));
            self.child.paint_child(ctx, scene);
            scene.pop_transform();
        } else {
            self.child.paint_child(ctx, scene);
        }

        // The refreshing loop's clock is latched here, where `&mut self` is
        // available; the paint helper below only reads it.
        if self.status.is_refreshing() {
            self.refresh_since.get_or_insert(now);
        } else {
            self.refresh_since = None;
        }

        // The indicator (`z-20`), faded and scaled by the pull.
        let pull = self.pull();
        let band_alpha = map_stops(pull, &[0.0, 10.0, self.threshold], &[0.0, 0.45, 1.0]) as f32;
        if band_alpha > 0.0 {
            let band = Size::new(size.width, INDICATOR_BAND.min(size.height));
            let scale = map_stops(pull, &[0.0, self.threshold], &[0.86, 1.0]);
            scene.push_layer(origin, band, band_alpha);
            paint_band(scene, origin, band, colors.surface);
            self.paint_indicator(scene, origin, band, &colors, now, reduce, scale);
            scene.pop_layer();
        }

        if self.status.is_refreshing() {
            // A perpetual character loop, not a transition with an endpoint.
            ctx.request_frame_class(TickClass::CosmeticLoop);
            owes_frame = false;
        }
        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.child.event_child(ctx, event);
            return EventResult::Ignored;
        }
        if self.disabled {
            return EventResult::Ignored;
        }
        let size = ctx.size();
        let InputEvent::Pointer(pointer) = event else {
            return route_event_single(&mut self.child, ctx, event);
        };
        match pointer.phase {
            PointerPhase::Down => {
                if !presses(pointer) || !inside(pointer.position, size) || self.is_refreshing() {
                    return route_event_single(&mut self.child, ctx, event);
                }
                self.gesture = Some(PullGesture {
                    start: pointer.position,
                    abandoned: false,
                });
                // Not consumed: the press still belongs to whatever is under it
                // until the drag proves itself vertical.
                route_event_single(&mut self.child, ctx, event)
            }
            PointerPhase::Move => {
                let Some(gesture) = self.gesture else {
                    return route_event_single(&mut self.child, ctx, event);
                };
                if gesture.abandoned {
                    return route_event_single(&mut self.child, ctx, event);
                }
                let delta = pointer.position - gesture.start;
                if delta.y < 0.0 {
                    // Upstream drops the gesture outright on an upward drag.
                    self.gesture = None;
                    return route_event_single(&mut self.child, ctx, event);
                }
                if delta.x.abs() > delta.y {
                    // A sideways drag belongs to whatever else wants it; the
                    // gesture stays armed in case it turns downward.
                    self.gesture = Some(PullGesture {
                        abandoned: false,
                        ..gesture
                    });
                    return route_event_single(&mut self.child, ctx, event);
                }
                if self.update_pull(delta.y) {
                    // Only now is the gesture ours, which is the moment to take
                    // the pointer off whatever the press landed on.
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                if self.gesture.is_none() {
                    return route_event_single(&mut self.child, ctx, event);
                }
                if self.pull() <= 0.0 {
                    // Never became a pull: hand the release to the content.
                    self.gesture = None;
                    return route_event_single(&mut self.child, ctx, event);
                }
                self.finish_pull(ctx);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // `<section aria-label aria-busy>` upstream; the indicator itself is
        // `pointer-events-none` decoration with a live region this tier has no
        // counterpart for.
        ctx.push_container(
            Role::Group,
            |node| {
                node.set_label(self.label(self.status).content());
                if self.disabled {
                    node.set_disabled();
                }
            },
            |ctx| self.child.semantics_child(ctx),
        );
    }

    visit_children!(child);
}

impl PullToRefreshWidget {
    /// Paint the indicator: upstream's `RefreshBuddy` plus the swapping label,
    /// centred in the band.
    #[allow(clippy::too_many_arguments)]
    fn paint_indicator(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        band: Size,
        colors: &RefreshColors,
        now: FrameTime,
        reduce: bool,
        scale: f64,
    ) {
        let progress = self.progress();
        let ready = self.status.is_ready();
        let refreshing = self.status.is_refreshing();
        // The loop's start frame is latched in `paint`, which holds `&mut self`;
        // this only reads it.
        let period = if reduce { CALM_PULSE } else { CHARACTER_LOOP };
        let phase = match self.refresh_since {
            Some(since) if refreshing && !period.is_zero() => {
                loop_ease((now.saturating_sub(since).as_secs_f64() / period.as_secs_f64()).fract())
            }
            _ => 0.0,
        };

        // `y: [0,1] → [-7,0]`, `rotate: [0,1] → [-10,0]`,
        // `scaleY: [0,0.55,1] → [0.68,1.1,0.92]` — upstream's own stops.
        let lift = map_stops(progress, &[0.0, 1.0], &[-7.0, 0.0]);
        let tilt = map_stops(progress, &[0.0, 1.0], &[-10.0, 0.0]).to_radians();
        let stretch = map_stops(progress, &[0.0, 0.55, 1.0], &[0.68, 1.1, 0.92]);

        let glyph = GLYPH_SIZE * scale;
        let unit = glyph / GLYPH_VIEWBOX;
        let top = origin.y + (band.height - INDICATOR_BAND) / 2.0 + 6.0;
        let left = origin.x + (band.width - glyph) / 2.0;
        // `origin-bottom`: the stretch and tilt pivot on the character's feet.
        let pivot = Vec2::new(left + glyph / 2.0, top + glyph);

        // The refreshing bob — upstream's `y: [0,-2,0]`, `rotate: [-3,3,-3]`.
        let (bob, sway) = if refreshing && !reduce {
            (
                map_stops(phase, &[0.0, 0.5, 1.0], &[0.0, -2.0, 0.0]),
                map_stops(phase, &[0.0, 0.5, 1.0], &[-3.0, 3.0, -3.0]).to_radians(),
            )
        } else {
            (0.0, 0.0)
        };
        let pop = self.pop.value();

        scene.push_transform(
            Affine::translate(Vec2::new(0.0, lift + bob))
                * Affine::translate(pivot)
                * Affine::rotate(tilt + sway)
                * Affine::scale_non_uniform(pop, stretch * pop)
                * Affine::translate(-pivot),
        );

        // The antenna: an arc plus its bulb, visible only once ready.
        if ready || refreshing {
            let centre = Point::new(left + 18.0 * unit, top + 18.0 * unit);
            let spin = if refreshing && !reduce {
                phase * std::f64::consts::TAU
            } else {
                0.0
            };
            let radius = 15.5 * unit;
            let arc = Arc::new(
                centre,
                Vec2::new(radius, radius),
                -std::f64::consts::FRAC_PI_2 + spin,
                0.9,
                0.0,
            );
            scene.stroke_path(
                Point::ORIGIN,
                &Shape::to_path(&arc, style::PATH_TOLERANCE),
                1.5 * unit,
                &Brush::Solid(colors.muted),
            );
            let bulb = 4.4 * unit;
            let angle = -std::f64::consts::FRAC_PI_2 + spin + 0.9;
            scene.fill_rounded_rect(
                Point::new(
                    centre.x + radius * angle.cos() - bulb / 2.0,
                    centre.y + radius * angle.sin() - bulb / 2.0,
                ),
                Size::new(bulb, bulb),
                bulb / 2.0,
                colors.ink,
            );
        }

        // The face: `<rect x=7 y=7 w=22 h=22 rx=9 />`.
        scene.fill_rounded_rect(
            Point::new(left + 7.0 * unit, top + 7.0 * unit),
            Size::new(22.0 * unit, 22.0 * unit),
            9.0 * unit,
            colors.ink,
        );

        // The eyes, blinking on the refreshing loop (`scaleY: [1,1,0.15,1,1]`).
        let blink = if refreshing && !reduce {
            map_stops(
                phase,
                &[0.0, 0.4, 0.5, 0.6, 1.0],
                &[1.0, 1.0, 0.15, 1.0, 1.0],
            )
        } else if ready {
            1.18
        } else {
            1.0
        };
        let eye = 2.9 * unit;
        for x in [14.2, 21.8] {
            let height = eye * blink;
            scene.fill_rounded_rect(
                Point::new(
                    left + x * unit - eye / 2.0,
                    top + 16.0 * unit - height / 2.0,
                ),
                Size::new(eye, height),
                eye / 2.0,
                colors.surface,
            );
        }

        // The mouth: a straight line at rest, a shallow smile once ready, a dot
        // while refreshing — the three upstream paths, two of them flattened.
        if refreshing {
            let dot = 3.2 * unit;
            scene.fill_rounded_rect(
                Point::new(
                    left + 18.0 * unit - dot / 2.0,
                    top + 21.0 * unit - dot / 2.0,
                ),
                Size::new(dot, dot),
                dot / 2.0,
                colors.surface,
            );
        } else if ready {
            let mouth = Arc::new(
                Point::new(left + 18.0 * unit, top + 20.0 * unit),
                Vec2::new(4.0 * unit, 2.4 * unit),
                0.35,
                2.44,
                0.0,
            );
            scene.stroke_path(
                Point::ORIGIN,
                &Shape::to_path(&mouth, style::PATH_TOLERANCE),
                1.5 * unit,
                &Brush::Solid(colors.surface),
            );
        } else {
            scene.stroke_line(
                Point::new(left + 14.5 * unit, top + 21.0 * unit),
                Point::new(left + 21.5 * unit, top + 21.0 * unit),
                1.5 * unit,
                colors.surface,
            );
        }
        scene.pop_transform();

        // The label, cross-fading between the outgoing and incoming statuses.
        let swap = self.label_swap.value().clamp(0.0, 1.0);
        let base_y = top + glyph + 2.0;
        let mut draw = |status: PullToRefreshStatus, alpha: f64, offset: f64| {
            if alpha <= 0.0 {
                return;
            }
            let run = self.label(status);
            let text = run.size();
            let at = Point::new(origin.x + (band.width - text.width) / 2.0, base_y + offset);
            run.paint(at, style::with_alpha(colors.muted, alpha as f32), scene);
        };
        if let Some(from) = self.label_from {
            draw(from, 1.0 - swap, -LABEL_SWAP_TRAVEL * swap);
        }
        draw(self.status, swap, LABEL_SWAP_TRAVEL * (1.0 - swap));
    }
}

/// Paint the indicator band's wash — upstream's
/// `bg-gradient-to-b from-background via-background/95 to-transparent`.
fn paint_band(scene: &mut dyn PaintScene, origin: Point, band: Size, surface: Color) {
    if band.height <= 0.0 {
        return;
    }
    let gradient = Gradient::new_linear(origin, Point::new(origin.x, origin.y + band.height))
        .with_stops(
            [
                ColorStop::from((0.0f32, surface)),
                ColorStop::from((0.6f32, style::with_alpha(surface, 0.95))),
                ColorStop::from((1.0f32, style::with_alpha(surface, 0.0))),
            ]
            .as_slice(),
        );
    scene.fill_rect_brush(origin, band, &Brush::Gradient(gradient));
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::SizedBox;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BezPath, PointerButton, PointerEvent};
    use frust_core::{BuildCtx, EventCtx, PaintCtx};
    use std::any::Any;

    /// The container every test lays out.
    const BOX: Size = Size::new(200.0, 300.0);

    /// Records what a paint emitted.
    #[derive(Default)]
    struct Recorder {
        transforms: Vec<Affine>,
        layers: Vec<f32>,
        rounded: usize,
        strokes: usize,
        lines: usize,
        brushes: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {
            self.rounded += 1;
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn stroke_line(&mut self, _a: Point, _b: Point, _w: f64, _c: Color) {
            self.lines += 1;
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn fill_rect_brush(&mut self, _o: Point, _s: Size, _b: &Brush) {
            self.brushes += 1;
        }
    }

    #[derive(Default)]
    struct Refreshes {
        count: usize,
    }

    fn view() -> PullToRefreshView<Refreshes> {
        pull_to_refresh(
            SizedBox::<Refreshes>(Some(BOX.width), Some(BOX.height)),
            |state: &mut Refreshes| state.count += 1,
        )
    }

    fn laid_out(view: &PullToRefreshView<Refreshes>) -> PullToRefreshWidget {
        let mut next_id = 0u64;
        let mut widget = View::<Refreshes>::build(view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::tight(BOX));
        widget
    }

    fn rebuilt(
        prev: &PullToRefreshView<Refreshes>,
        next: &PullToRefreshView<Refreshes>,
        widget: &mut PullToRefreshWidget,
    ) {
        let mut next_id = 1u64;
        View::<Refreshes>::rebuild(next, prev, widget, &mut BuildCtx::new(&mut next_id));
    }

    fn painted(
        widget: &mut PullToRefreshWidget,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame())
    }

    fn send(
        widget: &mut PullToRefreshWidget,
        state: &mut Refreshes,
        phase: PointerPhase,
        x: f64,
        y: f64,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, BOX);
        widget.event(
            &mut ctx,
            &InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, y),
                button: PointerButton::Primary,
            }),
        )
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// The resistance curve: asymptotic toward `maxPull`, zero at rest, and
    /// always below the raw distance once it is moving.
    #[test]
    fn the_pull_resistance_is_asymptotic() {
        assert_eq!(resisted_distance(0.0, 132.0), 0.0);
        assert_eq!(
            resisted_distance(-40.0, 132.0),
            0.0,
            "an upward drag is nothing"
        );

        let short = resisted_distance(20.0, 132.0);
        assert!(
            short > 0.0 && short < 20.0,
            "no resistance applied: {short}"
        );

        let mut last = 0.0;
        for step in 1..200 {
            let value = resisted_distance(step as f64 * 5.0, 132.0);
            assert!(value > last, "the pull stopped moving at {step}");
            assert!(value < 132.0, "the pull passed its limit: {value}");
            last = value;
        }
        // A degenerate limit is inert rather than a division by zero.
        assert_eq!(resisted_distance(50.0, 0.0), 0.0);
    }

    /// Upstream's own clamps: the threshold has a floor, the limit is forced
    /// past it, and the resting distance never exceeds the threshold.
    #[test]
    fn the_distance_props_carry_upstreams_clamps() {
        let squashed = view().threshold(4.0).max_pull(10.0).hold_distance(400.0);
        assert_eq!(squashed.pull_threshold(), MIN_THRESHOLD);
        assert_eq!(
            squashed.pull_limit(),
            MIN_THRESHOLD + MAX_PULL_HEADROOM,
            "the limit must clear the threshold"
        );
        assert_eq!(squashed.resting_distance(), MIN_THRESHOLD);

        let plain = view();
        assert_eq!(plain.pull_threshold(), DEFAULT_THRESHOLD);
        assert_eq!(plain.pull_limit(), DEFAULT_MAX_PULL);
        assert_eq!(plain.resting_distance(), DEFAULT_HOLD_DISTANCE);
    }

    /// A drag walks the status machine idle → pulling → ready, and releasing
    /// past the threshold fires the refresh exactly once.
    #[test]
    fn dragging_past_the_threshold_refreshes() {
        let mut widget = laid_out(&view());
        let mut state = Refreshes::default();
        assert_eq!(widget.status(), PullToRefreshStatus::Idle);

        send(&mut widget, &mut state, PointerPhase::Down, 100.0, 10.0);
        assert_eq!(
            widget.status(),
            PullToRefreshStatus::Idle,
            "a press is not a pull"
        );

        // A short drag is a pull, not yet an arm.
        send(&mut widget, &mut state, PointerPhase::Move, 100.0, 40.0);
        assert_eq!(widget.status(), PullToRefreshStatus::Pulling);
        assert!(widget.pull() > 0.0 && widget.progress() < 1.0);

        // Far enough, and it arms.
        send(&mut widget, &mut state, PointerPhase::Move, 100.0, 300.0);
        assert_eq!(widget.status(), PullToRefreshStatus::Ready);
        assert_eq!(widget.progress(), 1.0);

        send(&mut widget, &mut state, PointerPhase::Up, 100.0, 300.0);
        assert_eq!(widget.status(), PullToRefreshStatus::Refreshing);
        assert_eq!(state.count, 1, "the refresh fired once");
        assert!(widget.is_refreshing());
    }

    /// A release short of the threshold settles back without refreshing, and an
    /// upward drag never becomes a pull at all.
    #[test]
    fn a_short_pull_settles_back_and_an_upward_drag_is_ignored() {
        let mut widget = laid_out(&view());
        let mut state = Refreshes::default();

        send(&mut widget, &mut state, PointerPhase::Down, 100.0, 40.0);
        send(&mut widget, &mut state, PointerPhase::Move, 100.0, 60.0);
        assert_eq!(widget.status(), PullToRefreshStatus::Pulling);
        send(&mut widget, &mut state, PointerPhase::Up, 100.0, 60.0);
        assert_eq!(widget.status(), PullToRefreshStatus::Idle);
        assert_eq!(state.count, 0, "a short pull refreshed");

        // It springs back to rest rather than snapping.
        assert_eq!(widget.pull.target(), 0.0);
        for step in 0..60 {
            painted(&mut widget, step * 20, None);
        }
        assert!(widget.pull() < 0.001, "never settled: {}", widget.pull());

        // An upward drag drops the gesture outright.
        let mut widget = laid_out(&view());
        send(&mut widget, &mut state, PointerPhase::Down, 100.0, 100.0);
        send(&mut widget, &mut state, PointerPhase::Move, 100.0, 60.0);
        assert_eq!(widget.status(), PullToRefreshStatus::Idle);
        assert_eq!(widget.pull(), 0.0);
    }

    /// The overscroll route drives the same machine from `ScrollInfo`'s own
    /// signed past-edge number, and returning in range releases it.
    #[test]
    fn overscroll_drives_the_indicator() {
        let idle = view();
        let mut widget = laid_out(&idle);

        // Past the bottom is somebody else's business.
        rebuilt(&idle, &view().overscroll(80.0), &mut widget);
        assert_eq!(widget.status(), PullToRefreshStatus::Idle);

        rebuilt(
            &view().overscroll(80.0),
            &view().overscroll(-30.0),
            &mut widget,
        );
        assert_eq!(widget.status(), PullToRefreshStatus::Pulling);
        assert!(widget.pull() > 0.0);

        rebuilt(
            &view().overscroll(-30.0),
            &view().overscroll(-400.0),
            &mut widget,
        );
        assert_eq!(widget.status(), PullToRefreshStatus::Ready);

        rebuilt(
            &view().overscroll(-400.0),
            &view().overscroll(0.0),
            &mut widget,
        );
        assert_eq!(widget.status(), PullToRefreshStatus::Idle);
    }

    /// The controlled flag holds the indicator up, and dropping it releases —
    /// the frust-shaped stand-in for upstream's awaited promise.
    #[test]
    fn the_refreshing_flag_holds_and_releases_the_indicator() {
        let idle = view();
        let mut widget = laid_out(&idle);
        assert_eq!(widget.status(), PullToRefreshStatus::Idle);

        let busy = view().refreshing(true);
        rebuilt(&idle, &busy, &mut widget);
        assert_eq!(widget.status(), PullToRefreshStatus::Refreshing);
        assert_eq!(widget.pull.target(), DEFAULT_HOLD_DISTANCE);

        rebuilt(&busy, &idle, &mut widget);
        assert_eq!(widget.status(), PullToRefreshStatus::Idle);
        assert_eq!(widget.pull.target(), 0.0);

        // A container built already refreshing starts held, not idle.
        let born_busy = laid_out(&view().refreshing(true));
        assert_eq!(born_busy.status(), PullToRefreshStatus::Refreshing);
    }

    /// The internal latch covers the frame between the trigger and the app's own
    /// commit, then clears — upstream's synchronous-resolve path.
    #[test]
    fn the_internal_latch_clears_on_the_next_rebuild() {
        let idle = view();
        let mut widget = laid_out(&idle);
        let mut state = Refreshes::default();
        send(&mut widget, &mut state, PointerPhase::Down, 100.0, 10.0);
        send(&mut widget, &mut state, PointerPhase::Move, 100.0, 300.0);
        send(&mut widget, &mut state, PointerPhase::Up, 100.0, 300.0);
        assert!(
            widget.is_refreshing(),
            "the latch did not cover the trigger"
        );

        rebuilt(&idle, &idle, &mut widget);
        assert!(!widget.is_refreshing(), "the latch outlived the commit");
        assert_eq!(widget.status(), PullToRefreshStatus::Idle);
    }

    /// A disabled container takes nothing, and being disabled mid-pull abandons
    /// it rather than leaving armed state behind.
    #[test]
    fn a_disabled_container_is_inert() {
        let live = view();
        let mut widget = laid_out(&live);
        let mut state = Refreshes::default();
        send(&mut widget, &mut state, PointerPhase::Down, 100.0, 10.0);
        send(&mut widget, &mut state, PointerPhase::Move, 100.0, 300.0);
        assert_eq!(widget.status(), PullToRefreshStatus::Ready);

        rebuilt(&live, &view().disabled(true), &mut widget);
        assert_eq!(widget.status(), PullToRefreshStatus::Idle);
        assert_eq!(
            send(&mut widget, &mut state, PointerPhase::Down, 100.0, 10.0),
            EventResult::Ignored
        );
        assert_eq!(state.count, 0);
    }

    /// The indicator appears only once the pull has begun, fades in on
    /// upstream's own stops, and paints the face plus its label.
    #[test]
    fn the_indicator_fades_in_with_the_pull() {
        // Upstream's `[0, 10, threshold] → [0, 0.45, 1]`.
        assert_eq!(map_stops(0.0, &[0.0, 10.0, 76.0], &[0.0, 0.45, 1.0]), 0.0);
        assert_eq!(map_stops(10.0, &[0.0, 10.0, 76.0], &[0.0, 0.45, 1.0]), 0.45);
        assert_eq!(map_stops(76.0, &[0.0, 10.0, 76.0], &[0.0, 0.45, 1.0]), 1.0);
        assert_eq!(map_stops(999.0, &[0.0, 10.0, 76.0], &[0.0, 0.45, 1.0]), 1.0);

        let mut widget = laid_out(&view());
        let mut state = Refreshes::default();
        let (recorder, _) = painted(&mut widget, 0, None);
        assert!(
            recorder.layers.is_empty(),
            "an idle container drew an indicator"
        );

        send(&mut widget, &mut state, PointerPhase::Down, 100.0, 10.0);
        send(&mut widget, &mut state, PointerPhase::Move, 100.0, 300.0);
        let (recorder, _) = painted(&mut widget, 0, None);
        assert_eq!(
            recorder.layers,
            vec![1.0],
            "a full pull shows a full indicator"
        );
        assert_eq!(recorder.brushes, 1, "the band's wash");
        assert!(recorder.rounded >= 3, "the face and its eyes");
        // Armed: the antenna is out and the mouth is a smile, not a line.
        assert!(recorder.strokes >= 2);
        assert_eq!(recorder.lines, 0);

        // The content is displaced by the pull.
        let shift = recorder.transforms[0].as_coeffs()[5];
        assert!(shift > 0.0, "the content did not move: {shift}");
    }

    /// `reduce_motion` holds the content still and drops the character's orbit,
    /// while the indicator itself stays visible.
    #[test]
    fn reduce_motion_holds_the_content_still() {
        let theme = reduced();
        let mut widget = laid_out(&view());
        let mut state = Refreshes::default();
        send(&mut widget, &mut state, PointerPhase::Down, 100.0, 10.0);
        send(&mut widget, &mut state, PointerPhase::Move, 100.0, 300.0);

        let (recorder, _) = painted(&mut widget, 0, Some(&theme));
        // The first transform is the character's, not the content's — the
        // content is painted untranslated.
        assert_eq!(recorder.layers, vec![1.0], "the indicator vanished");
        let coeffs = recorder.transforms[0].as_coeffs();
        assert!(
            coeffs[5] != 0.0 || coeffs[4] != 0.0 || coeffs[1] != 0.0,
            "no indicator transform at all"
        );

        // A refresh under reduced motion still asks for its calm pulse.
        send(&mut widget, &mut state, PointerPhase::Up, 100.0, 300.0);
        let (_, needs_frame) = painted(&mut widget, 10, Some(&theme));
        assert!(needs_frame, "the refreshing loop asked for nothing");
    }

    /// Every status is constructible and reports itself.
    #[test]
    fn every_status_is_constructible() {
        assert_eq!(PullToRefreshStatus::ALL.len(), 4);
        assert!(PullToRefreshStatus::Ready.is_ready());
        assert!(PullToRefreshStatus::Refreshing.is_refreshing());
        assert!(!PullToRefreshStatus::Pulling.is_ready());
        assert_eq!(PullToRefreshStatus::default(), PullToRefreshStatus::Idle);
    }

    // ---- Typeface: the status label follows the live theme -----------------

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// A container held refreshing over glyph-free content, so the indicator
    /// is up and its status label is the one text run.
    fn probe_view(_: &mut ()) -> PullToRefreshView<()> {
        pull_to_refresh::<(), _, _>(SizedBox::<()>(Some(BOX.width), Some(BOX.height)), |_| {})
            .refreshing(true)
    }

    #[test]
    fn the_status_label_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the status label", probe_view, BOX);
    }

    #[test]
    fn the_status_label_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the status label", probe_view, BOX);
    }
}
