// Ported from `material_3_expressive` v1.0.8's `M3ESearchBar`
// (`lib/components/search/m3e_search_bar.dart`,
// `components/m3e_search_bar_build.dart`, `styles/m3e_search_bar_theme.dart`)
// and the read-only anchor shape `_M3ESearchAnchorBarState`
// (`m3e_search_anchor.dart`) — MIT, © 2026 Paa Developments; see [`super`]'s
// header for the family's full source list and porting decisions.

//! The M3 Expressive **search bar**: the pill-shaped affordance that opens a
//! search view.
//!
//! # Geometry
//!
//! A stadium ([`frust::Theme`]`.shape.full`-equivalent) pill filled with
//! `surfaceContainerHigh`, at least [`super::SEARCH_BAR_MIN_HEIGHT`] (56dp)
//! tall, inset [`super::BAR_HORIZONTAL_PADDING`] (8dp) on both ends, laying out
//! `[leading slot][hint-or-query][trailing slots]` — the reference's
//! `_buildEditingRow` (`m3e_search_bar_build.dart`) with each action wrapped in
//! its own [`super::ACTION_SLOT`] (48dp) touch target, exactly as
//! `_wrapActionSlot` does.
//!
//! The bar fills whatever width it is offered, floored at
//! [`super::SEARCH_BAR_MIN_WIDTH`] only when its constraints are unbounded (see
//! that constant for the reference's overflowing `ConstrainedBox` and why this
//! port clamps instead).
//!
//! # Display-only, by design
//!
//! The bar never edits: it shows `query`, or `hint` when the query is empty,
//! and a press fires [`SearchBarView::on_tap`] — the caller opens the view,
//! whose own field owns editing. That is the reference's own
//! `M3ESearchAnchor.bar` shape (`readOnly: true`, `canRequestFocus: false`);
//! see [`super`]'s *Fidelity decisions* for why the live-editable
//! `M3ESearchBar` variant is not offered.
//!
//! [`search_bar`]'s `on_query_changed` is therefore not an editing callback but
//! the controlled query's **write side**, and this widget's one caller of it is
//! the built-in clear action: with no explicit
//! [`trailing`](SearchBarView::trailing) set and a non-empty query, the bar
//! grows a [`crate::icons::CLOSE`] icon button that reports the empty string —
//! the reference's `_M3ESearchAnchorBarState._buildTrailing`, whose default
//! trailing is exactly one clear button wired to `controller.clear`.
//!
//! # The expand spring
//!
//! `M3ESearchBarTheme` animates a horizontal inset ([`EXPAND_REST`], 8dp)
//! down to half of it ([`EXPAND_ACTIVE`], 4dp) — i.e. the pill *widens* — with
//! `M3EMotion.expressiveSpatialPress` (stiffness 380, damping ratio 0.55, the
//! [`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`] token
//! [`EXPAND_SPRING`] pins). The reference drives it from **focus**
//! (`_handleFocusChange` → `_syncExpandPaddingController`); a display-only bar
//! never takes focus, so this port drives the same spring from the bar's own
//! **press** instead — the nearest live signal, and the one the reference's own
//! spring is named for. The inset is applied in `layout` (it changes the pill's
//! geometry, so hit-testing follows it), which is why a running spring asks for
//! a relayout rather than a bare repaint.
//!
//! `Theme.motion.reduce_motion` snaps the inset to its target with no ramp.
//!
//! # Interaction
//!
//! Fire-on-up-inside with a pointer capture, the M3 state layer
//! (pressed 10% / hovered 8% — the reference's own `pressedOverlayOpacity` /
//! `hoveredOverlayOpacity`), and the catalog's claim/latch/self-correct hover
//! discipline (`docs/CODE_STANDARDS.md`'s Interaction Semantics; the
//! [`crate::list_item`] reference impl). A disabled bar composites at
//! [`super::DISABLED_OPACITY`] (0.38), claims no hover and arms nothing —
//! the reference's `Opacity` + `IgnorePointer` pair.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::TextOverflow;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx,
    ThemeTextColor, View, Widget, any, build_child, erase_callback, rebuild_child,
    rebuild_children, route_event, route_event_single, teardown_child, visit_children,
};
use frust::{AnimationController, Color, FrameTime, SpringDesc, Theme, icon, text};
use kurbo::{Point, Rect, Size};

use super::{
    ACTION_SLOT, BAR_HORIZONTAL_PADDING, BODY_LARGE_LINE_HEIGHT, BODY_LARGE_SIZE, DISABLED_OPACITY,
    SEARCH_BAR_MIN_HEIGHT, SEARCH_BAR_MIN_WIDTH,
};
use crate::icon_button::icon_button;
use crate::overlay::{finite_or_zero, reduce_motion};
use crate::press::presses;
use crate::state_layer::StateLayer;

/// The pill's resting horizontal inset, in logical px —
/// `M3ESearchBarTheme.restingExpandPadding` (8dp).
pub const EXPAND_REST: f64 = 8.0;

/// The pill's active (pressed) horizontal inset, in logical px — the
/// reference's `_focusedExpandPadding`, which is
/// `_restingExpandPadding(barTheme) / 2`.
pub const EXPAND_ACTIVE: f64 = EXPAND_REST / 2.0;

/// The expand spring: stiffness 380, damping ratio 0.55, mass 1 —
/// `M3ESearchBarTheme.focusExpandSpring`'s `M3EMotion.expressiveSpatialPress`
/// default, i.e. [`crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`].
///
/// A named constant rather than a theme read for the reason every press spring
/// in this crate is: a press starts in the event pass, and event-pass code
/// never reads a theme (`docs/CODE_STANDARDS.md`'s Theming conventions).
const EXPAND_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 380.0,
    damping_ratio: 0.55,
};

/// Nominal period seeding the expand [`AnimationController`]'s clock. The
/// motion is spring-driven via `fling`, so this duration backs the
/// controller's construction only and is not itself a timing (the same shape
/// [`crate::button`]'s press morph uses).
const EXPAND_PERIOD: Duration = Duration::from_millis(300);

/// Launch velocity (progress-units/sec) handed to the expand spring's `fling`
/// — the same modest kick every press spring in this catalog takes.
const EXPAND_FLING_VELOCITY: f64 = 4.0;

/// Dead band (logical px) a new inset target must exceed to start a new spring
/// leg, so a re-resolving paint pass cannot restart the spring every frame
/// ([`crate::button`]'s `RETARGET_TOLERANCE`).
const RETARGET_TOLERANCE: f64 = 0.1;

/// Unthemed-fallback `surfaceContainerHigh` — the pill's fill
/// (`M3ESearchBarTheme.backgroundColor`), M3 baseline light.
const FALLBACK_CONTAINER_HIGH: Color = Color::from_rgb8(0xEC, 0xE6, 0xF0);

/// Unthemed-fallback `onSurface` — the state layer's ink, M3 baseline light.
const FALLBACK_ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

/// A view-held, typed text callback (erased on build).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;

/// A view-held, typed state callback (erased on build).
type OnState<State> = Rc<dyn Fn(&mut State)>;

/// The pill's horizontal-inset spring. See the [module docs](self)' *expand
/// spring*.
struct ExpandMotion {
    from: f64,
    to: f64,
    anim: AnimationController,
}

impl ExpandMotion {
    /// A motion resting at [`EXPAND_REST`] with nothing running.
    fn new() -> Self {
        Self {
            from: EXPAND_REST,
            to: EXPAND_REST,
            anim: AnimationController::new(EXPAND_PERIOD),
        }
    }

    /// Aim the inset at `target`, returning whether a new spring leg started.
    /// A target inside [`RETARGET_TOLERANCE`] of the live one is ignored.
    fn retarget(&mut self, target: f64) -> bool {
        if (self.to - target).abs() <= RETARGET_TOLERANCE {
            return false;
        }
        self.from = self.value();
        self.to = target;
        self.anim = AnimationController::new(EXPAND_PERIOD);
        self.anim.fling(EXPAND_FLING_VELOCITY, EXPAND_SPRING);
        true
    }

    /// Pin the inset to its target with no motion (`reduce_motion`, and the
    /// reference's own `snapToTarget`).
    fn snap(&mut self) {
        self.from = self.to;
        self.anim = AnimationController::new(EXPAND_PERIOD);
    }

    /// Advance the spring to `now`, reporting whether it is still running.
    fn advance(&mut self, now: FrameTime) -> bool {
        self.anim.advance(now)
    }

    /// The inset this frame lays out with — never negative, since an
    /// under-damped overshoot toward the smaller target can otherwise drive it
    /// past zero (the reference's own `pad > 0 ? pad : 0.0` guard).
    fn value(&self) -> f64 {
        let t = self.anim.value();
        let t = if t.is_finite() { t } else { 1.0 };
        (self.from + (self.to - self.from) * t).max(0.0)
    }
}

/// A declarative M3 search bar. See the [module docs](self).
pub struct SearchBarView<State: 'static> {
    query: String,
    on_query_changed: OnText<State>,
    hint: Option<String>,
    leading: Option<AnyView<State>>,
    trailing: Vec<AnyView<State>>,
    on_tap: Option<OnState<State>>,
    enabled: bool,
}

/// Build a controlled M3 search bar showing `query`.
///
/// `on_query_changed(state, new_query)` is the controlled value's write side;
/// this widget itself calls it only from the built-in clear action (see the
/// [module docs](self)), and the search view it opens calls it for every
/// keystroke. Wire [`SearchBarView::on_tap`] to open that view.
///
/// ```ignore
/// search_bar(state.query.clone(), |s: &mut App, q| s.query = q)
///     .hint("Search recipes")
///     .on_tap(|s: &mut App| s.search_open = true)
/// ```
pub fn search_bar<State: 'static, F: Fn(&mut State, String) + 'static>(
    query: impl Into<String>,
    on_query_changed: F,
) -> SearchBarView<State> {
    SearchBarView {
        query: query.into(),
        on_query_changed: Rc::new(on_query_changed),
        hint: None,
        leading: None,
        trailing: Vec::new(),
        on_tap: None,
        enabled: true,
    }
}

impl<State: 'static> SearchBarView<State> {
    /// Set the placeholder shown while `query` is empty
    /// (`M3ESearchBar.hintText`). It also labels the bar's accessibility node.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Replace the leading slot's default [`crate::icons::SEARCH`] icon.
    ///
    /// The slot itself is never empty — the reference's anchor bar passes
    /// `barLeading ?? Icon(M3EIcons.search)`, so its
    /// `noLeadingHintExtraPadding` (the extra start inset for a leadingless
    /// bar) has no reachable case here and is not ported.
    pub fn leading(mut self, view: AnyView<State>) -> Self {
        self.leading = Some(view);
        self
    }

    /// Append a trailing action (an icon button, an avatar). Setting any
    /// trailing action replaces the built-in clear button —
    /// `_M3ESearchAnchorBarState._buildTrailing`'s own
    /// `widget.barTrailing ?? [clear]` precedence.
    pub fn trailing(mut self, view: AnyView<State>) -> Self {
        self.trailing.push(view);
        self
    }

    /// Set the press handler — where a caller opens the search view
    /// (`M3ESearchBar.onTap`, which the reference's anchor wires to
    /// `controller.openView()`).
    pub fn on_tap<F: Fn(&mut State) + 'static>(mut self, on_tap: F) -> Self {
        self.on_tap = Some(Rc::new(on_tap));
        self
    }

    /// Set whether the bar responds to input (default `true`). A disabled bar
    /// composites at [`super::DISABLED_OPACITY`] and arms nothing.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The leading slot's default view (the M3 `search` glyph). Rebuilt per
    /// pass rather than stored, since [`AnyView`] is opaque — the
    /// [`crate::side_sheet`] precedent for a generated child view.
    fn default_leading(&self) -> AnyView<State> {
        any::<State, _>(icon(crate::icons::SEARCH))
    }

    /// The leading view this pass mounts: the caller's, or `default`.
    fn leading_ref<'a>(&'a self, default: &'a AnyView<State>) -> &'a AnyView<State> {
        self.leading.as_ref().unwrap_or(default)
    }

    /// The built-in trailing actions: one clear button while the query is
    /// non-empty and the caller set no trailing of its own, else nothing.
    fn generated_trailing(&self) -> Vec<AnyView<State>> {
        if !self.trailing.is_empty() || self.query.is_empty() {
            return Vec::new();
        }
        let on_query_changed = self.on_query_changed.clone();
        vec![any::<State, _>(
            icon_button(any::<State, _>(icon(crate::icons::CLOSE)), move |state| {
                on_query_changed(state, String::new());
            })
            .semantic_label(CLEAR_LABEL),
        )]
    }

    /// Every trailing view this pass mounts, in order: the caller's own, or
    /// `generated` — `_buildTrailing`'s `widget.barTrailing ?? [clear]`.
    fn trailing_refs<'a>(&'a self, generated: &'a [AnyView<State>]) -> Vec<&'a AnyView<State>> {
        if self.trailing.is_empty() {
            generated.iter().collect()
        } else {
            self.trailing.iter().collect()
        }
    }

    /// The label run: the query, or the hint while it is empty.
    fn label_view(&self) -> AnyView<State> {
        let (content, role) = if self.query.is_empty() {
            (
                self.hint.clone().unwrap_or_default(),
                ThemeTextColor::OnSurfaceVariant,
            )
        } else {
            (self.query.clone(), ThemeTextColor::OnSurface)
        };
        any::<State, _>(
            text(content)
                .size(BODY_LARGE_SIZE)
                .line_height(BODY_LARGE_LINE_HEIGHT)
                .themed_role(role)
                .max_lines(1)
                .overflow(TextOverflow::Ellipsis),
        )
    }
}

/// The clear action's accessible name — `M3ESearchConstants.clearButtonTooltip`.
const CLEAR_LABEL: &str = "Clear";

impl<State: 'static> View<State> for SearchBarView<State> {
    type Element = SearchBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SearchBarWidget {
        let default_leading = self.default_leading();
        let generated = self.generated_trailing();
        SearchBarWidget {
            leading: build_child(self.leading_ref(&default_leading), ctx),
            label: build_child(&self.label_view(), ctx),
            trailing: self
                .trailing_refs(&generated)
                .into_iter()
                .map(|v| build_child(v, ctx))
                .collect(),
            on_tap: self.on_tap.as_ref().map(erase_callback),
            enabled: self.enabled,
            hint: self.hint.clone(),
            state: StateLayer::new(),
            captured: false,
            expand: ExpandMotion::new(),
            pill: Rect::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SearchBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let prev_default_leading = prev.default_leading();
        let next_default_leading = self.default_leading();
        let mut flags = rebuild_child(
            prev.leading_ref(&prev_default_leading),
            self.leading_ref(&next_default_leading),
            &mut element.leading,
            ctx,
        );
        flags |= rebuild_child(
            &prev.label_view(),
            &self.label_view(),
            &mut element.label,
            ctx,
        );

        let prev_generated = prev.generated_trailing();
        let next_generated = self.generated_trailing();
        let prev_refs = prev.trailing_refs(&prev_generated);
        let next_refs = self.trailing_refs(&next_generated);
        flags |= rebuild_children(
            &prev_refs,
            &next_refs,
            &mut element.trailing,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );

        element.on_tap = self.on_tap.as_ref().map(erase_callback);
        if element.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                element.state.set_pressed(false);
                element.state.set_hovered(false);
                element.captured = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        if element.hint != self.hint {
            element.hint = self.hint.clone();
        }
        flags
    }

    fn teardown(&self, element: &mut SearchBarWidget, ctx: &mut BuildCtx<'_>) {
        let default_leading = self.default_leading();
        teardown_child(
            self.leading_ref(&default_leading),
            &mut element.leading,
            ctx,
        );
        teardown_child(&self.label_view(), &mut element.label, ctx);
        let generated = self.generated_trailing();
        for (view, pod) in self
            .trailing_refs(&generated)
            .into_iter()
            .zip(element.trailing.iter_mut())
        {
            teardown_child(view, pod, ctx);
        }
    }
}

/// The retained widget for a [`SearchBarView`]. See the [module docs](self).
pub struct SearchBarWidget {
    leading: ChildPod,
    label: ChildPod,
    trailing: Vec<ChildPod>,
    on_tap: Option<ErasedCallback>,
    enabled: bool,
    /// The hint, kept for the accessibility node's label.
    hint: Option<String>,
    state: StateLayer,
    captured: bool,
    expand: ExpandMotion,
    /// The pill's rect in this widget's own space — the hit region, and what
    /// the fill and state layer paint.
    pill: Rect,
}

impl SearchBarWidget {
    /// The pill's rect in this widget's own coordinate space: the widget's box
    /// inset by the live [expand](self) spring value on both ends.
    pub fn pill_rect(&self) -> Rect {
        self.pill
    }

    /// The horizontal inset the expand spring is currently at, in logical px.
    pub fn expand_inset(&self) -> f64 {
        self.expand.value()
    }
}

impl Widget for SearchBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let avail = finite_or_zero(bc.max().width);
        let width = if avail > 0.0 {
            avail.max(bc.min().width)
        } else {
            SEARCH_BAR_MIN_WIDTH
        };
        // The 56dp spec height, floored by the incoming minimum and capped by a
        // finite maximum — so the pill's own rect never disagrees with the size
        // this layout reports.
        let height = SEARCH_BAR_MIN_HEIGHT.max(bc.min().height);
        let height = if bc.max().height.is_finite() {
            height.min(bc.max().height)
        } else {
            height
        };

        let inset = self.expand.value();
        // A spring overshoot may not eat the pill: the inset can never take
        // more than the box itself.
        let inset = inset.min(width / 2.0);
        self.pill = Rect::new(inset, 0.0, width - inset, height);

        let content_x0 = self.pill.x0 + BAR_HORIZONTAL_PADDING;
        let content_x1 = self.pill.x1 - BAR_HORIZONTAL_PADDING;
        let slot_bc = BoxConstraints::loose(Size::new(ACTION_SLOT, height));

        let leading_size = self.leading.layout_child(ctx, &slot_bc);
        self.leading.set_origin(Point::new(
            content_x0 + (ACTION_SLOT - leading_size.width) / 2.0,
            (height - leading_size.height) / 2.0,
        ));

        // Trailing actions pack from the end edge inward, each centred in its
        // own 48dp slot.
        let mut trailing_x = content_x1;
        for pod in self.trailing.iter_mut().rev() {
            let size = pod.layout_child(ctx, &slot_bc);
            trailing_x -= ACTION_SLOT;
            pod.set_origin(Point::new(
                trailing_x + (ACTION_SLOT - size.width) / 2.0,
                (height - size.height) / 2.0,
            ));
        }

        let label_x0 = content_x0 + ACTION_SLOT;
        let label_w = (trailing_x - label_x0).max(0.0);
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(Size::new(label_w, height)));
        self.label
            .set_origin(Point::new(label_x0, (height - label_size.height) / 2.0));

        Size::new(width, height)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Resolve every themed value up front: the theme borrows `ctx`, and the
        // motion arms below need it mutably.
        let (reduce, fill, ink) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                reduce_motion(theme),
                theme.map_or(FALLBACK_CONTAINER_HIGH, |t| {
                    t.scheme().surface_container_high
                }),
                theme.map_or(FALLBACK_ON_SURFACE, |t| t.scheme().on_surface),
            )
        };
        // The inset is a *layout* value, so a running spring asks for a
        // relayout, not a bare repaint.
        if reduce {
            self.expand.snap();
        } else if self.expand.advance(ctx.frame_time()) {
            ctx.request_layout();
            ctx.request_frame();
        }

        // The pod's hover path is authoritative: correct the latched flag from
        // it, whatever the event pass last recorded
        // (`docs/CODE_STANDARDS.md`'s Interaction Semantics).
        let hovered = ctx.is_hovered() && self.enabled;
        self.state.set_hovered(hovered);

        let origin = ctx.origin();
        let pill_origin = Point::new(origin.x + self.pill.x0, origin.y + self.pill.y0);
        let pill = Rect::from_origin_size(pill_origin, self.pill.size());
        let radius = pill.height() / 2.0;

        // A disabled bar composites whole at 38% (`Opacity` upstream), so its
        // chrome and its slot content dim together.
        let dimmed = !self.enabled;
        if dimmed {
            scene.push_layer(pill.origin(), pill.size(), DISABLED_OPACITY);
        }
        scene.fill_rounded_rect(pill.origin(), pill.size(), radius, fill);
        self.state.paint(ctx, scene, pill, radius, ink);
        self.leading.paint_child(ctx, scene);
        self.label.paint_child(ctx, scene);
        for pod in &mut self.trailing {
            pod.paint_child(ctx, scene);
        }
        if dimmed {
            scene.pop_layer();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A broadcast reaches every child unconditionally and is never consumed.
        if event.is_broadcast() {
            self.leading.event_child(ctx, event);
            self.label.event_child(ctx, event);
            for pod in &mut self.trailing {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if !self.enabled {
            // Disabled: the reference's `IgnorePointer` — nothing is armed and
            // no slot is reachable.
            return EventResult::Ignored;
        }
        // Content first, the container-claims-after-routing rule: the trailing
        // clear action must win the press it lands on. A captured bar keeps the
        // pass to itself so a drag cannot reach a slot.
        if !self.captured {
            if route_event_single(&mut self.leading, ctx, event) == EventResult::Handled {
                return EventResult::Handled;
            }
            if route_event(&mut self.trailing, ctx, event) == EventResult::Handled {
                return EventResult::Handled;
            }
        }

        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) || !self.pill.contains(p.position) {
                    return EventResult::Ignored;
                }
                self.state.set_pressed(true);
                self.captured = true;
                self.expand.retarget(EXPAND_ACTIVE);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    // The hover pass: claim on every qualifying move (a claim
                    // covers only its own pass) and gate the redraw on the
                    // latch's changed-return.
                    let over = self.pill.contains(p.position);
                    if over {
                        ctx.claim_hover();
                    }
                    if self.state.set_hovered(over) {
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
                if self.state.set_pressed(self.pill.contains(p.position)) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                let inside = self.pill.contains(p.position);
                self.state.set_pressed(false);
                self.captured = false;
                self.expand.retarget(EXPAND_REST);
                if inside && let Some(on_tap) = self.on_tap.as_mut() {
                    on_tap(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // A `Cancel` arm clears flags only — never app state.
                self.state.set_pressed(false);
                self.captured = false;
                self.expand.retarget(EXPAND_REST);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.hint.clone();
        ctx.push_container(
            Role::SearchInput,
            |node| {
                if let Some(label) = &label {
                    node.set_label(label.as_str());
                }
            },
            |ctx| {
                self.leading.semantics_child(ctx);
                self.label.semantics_child(ctx);
                for pod in &self.trailing {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(leading, label, trailing);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent, any as core_any};
    use frust_core::RenderRoot;
    use std::any::Any;

    /// The window every layout test uses.
    const WINDOW: Size = Size::new(400.0, 200.0);

    #[derive(Default)]
    struct App {
        query: String,
        taps: u32,
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        layers: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn pop_layer(&mut self) {}
    }

    fn build<S: 'static>(view: &SearchBarView<S>) -> SearchBarWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut SearchBarWidget, bc: &BoxConstraints) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, bc)
    }

    fn paint(w: &mut SearchBarWidget) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, WINDOW, FrameTime::ZERO);
        w.paint(&mut pctx, &mut rec);
        rec
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut SearchBarWidget, state: &mut App, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event(&mut ctx, event)
    }

    fn bar(query: &str) -> SearchBarView<App> {
        search_bar(query.to_string(), |s: &mut App, q| s.query = q)
    }

    // ---- Geometry ----

    #[test]
    fn the_bar_is_a_56dp_pill_inset_by_the_resting_expand_padding() {
        let view = bar("").hint("Search");
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));

        assert_eq!(size, Size::new(400.0, SEARCH_BAR_MIN_HEIGHT));
        let pill = w.pill_rect();
        assert_eq!(pill.x0, EXPAND_REST);
        assert_eq!(pill.x1, 400.0 - EXPAND_REST);
        assert_eq!(pill.height(), SEARCH_BAR_MIN_HEIGHT);
    }

    #[test]
    fn a_loose_box_still_floors_the_height_at_the_spec_56dp() {
        let view = bar("");
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 500.0)));
        assert_eq!(size.height, SEARCH_BAR_MIN_HEIGHT);
    }

    #[test]
    fn an_unbounded_width_falls_back_to_the_reference_min_width() {
        let view = bar("");
        let mut w = build(&view);
        let size = layout(
            &mut w,
            &BoxConstraints::loose(Size::new(f64::INFINITY, 500.0)),
        );
        assert_eq!(size.width, SEARCH_BAR_MIN_WIDTH);
    }

    #[test]
    fn a_narrow_parent_clamps_rather_than_overflowing() {
        let view = bar("");
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::tight(Size::new(300.0, 56.0)));
        assert_eq!(
            size.width, 300.0,
            "the reference's ConstrainedBox would overflow; this port clamps"
        );
    }

    #[test]
    fn the_leading_slot_sits_in_a_48dp_target_at_the_pill_start() {
        let view = bar("");
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));
        // 24dp icon centred in a 48dp slot that starts one 8dp inset in.
        let expected_x = EXPAND_REST + BAR_HORIZONTAL_PADDING + (ACTION_SLOT - 24.0) / 2.0;
        assert_eq!(w.leading.origin().x, expected_x);
    }

    #[test]
    fn a_non_empty_query_grows_the_built_in_clear_action_at_the_end_edge() {
        let empty = bar("");
        let w = build(&empty);
        assert!(
            w.trailing.is_empty(),
            "an empty query carries no clear action"
        );

        let filled = bar("pasta");
        let mut w = build(&filled);
        assert_eq!(w.trailing.len(), 1, "a non-empty query grows one");
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));
        let slot_x = w.trailing[0].origin().x;
        assert!(
            slot_x > 400.0 - EXPAND_REST - BAR_HORIZONTAL_PADDING - ACTION_SLOT - 1.0,
            "the clear action packs against the end edge"
        );
    }

    #[test]
    fn an_explicit_trailing_action_replaces_the_built_in_clear() {
        let view = bar("pasta").trailing(leaf_any_app(24.0, 24.0));
        let w = build(&view);
        assert_eq!(w.trailing.len(), 1);
        // The built-in clear would have been mounted too if it were additive;
        // the reference's `barTrailing ?? [clear]` is a replacement.
    }

    // ---- Paint ----

    #[test]
    fn the_pill_paints_the_surface_container_high_role() {
        let theme = crate::baseline();
        let view = bar("").hint("Search");
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, WINDOW, FrameTime::ZERO).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        let (origin, size, radius, color) = rec.rrects[0];
        assert_eq!(origin, Point::new(EXPAND_REST, 0.0));
        assert_eq!(size, Size::new(400.0 - 2.0 * EXPAND_REST, 56.0));
        assert_eq!(radius, 28.0, "a stadium pill rounds to half its height");
        assert_eq!(color, theme.scheme().surface_container_high);
    }

    #[test]
    fn an_unthemed_pill_paints_the_m3_baseline_light_container() {
        let light = crate::baseline().with_brightness(frust::Brightness::Light);
        let view = bar("");
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));
        let rec = paint(&mut w);
        assert_eq!(rec.rrects[0].3, light.scheme().surface_container_high);
    }

    #[test]
    fn a_disabled_bar_composites_at_the_reference_disabled_opacity() {
        let view = bar("").enabled(false);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));
        let rec = paint(&mut w);
        assert_eq!(rec.layers, vec![DISABLED_OPACITY]);
    }

    // ---- Interaction ----

    #[test]
    fn a_press_and_release_inside_the_pill_fires_on_tap() {
        let view = bar("").on_tap(|s: &mut App| s.taps += 1);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));

        let mut state = App::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, 28.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 200.0, 28.0));
        assert_eq!(state.taps, 1);
    }

    #[test]
    fn a_release_outside_the_pill_fires_nothing() {
        let view = bar("").on_tap(|s: &mut App| s.taps += 1);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));

        let mut state = App::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, 28.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 200.0, 400.0));
        assert_eq!(state.taps, 0);
    }

    #[test]
    fn a_secondary_press_never_arms_the_bar() {
        let view = bar("").on_tap(|s: &mut App| s.taps += 1);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));

        let mut state = App::default();
        let down = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(200.0, 28.0),
            button: PointerButton::Secondary,
        });
        assert_eq!(
            dispatch(&mut w, &mut state, &down),
            EventResult::Ignored,
            "a context gesture is not an activation"
        );
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 200.0, 28.0));
        assert_eq!(state.taps, 0);
    }

    #[test]
    fn a_disabled_bar_arms_nothing() {
        let view = bar("").enabled(false).on_tap(|s: &mut App| s.taps += 1);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));

        let mut state = App::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, 28.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 200.0, 28.0));
        assert_eq!(state.taps, 0);
    }

    #[test]
    fn a_cancel_clears_the_press_without_firing() {
        let view = bar("").on_tap(|s: &mut App| s.taps += 1);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));

        let mut state = App::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, 28.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 200.0, 28.0));
        assert_eq!(state.taps, 0);
        assert!(!w.captured);
    }

    #[test]
    fn the_clear_action_reports_an_empty_query_through_on_query_changed() {
        fn logic(state: &mut App) -> SearchBarView<App> {
            search_bar(state.query.clone(), |s: &mut App, q| s.query = q).hint("Search")
        }
        let mut root: RenderRoot<App, SearchBarView<App>> = RenderRoot::new();
        let mut state = App {
            query: "pasta".into(),
            taps: 0,
        };
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 56.0), &mut tcx as &mut dyn Any);

        // The clear button's own 48dp slot sits against the end edge.
        let x = 400.0 - EXPAND_REST - BAR_HORIZONTAL_PADDING - ACTION_SLOT / 2.0;
        root.event(&mut state, &ev(PointerPhase::Down, x, 28.0));
        root.event(&mut state, &ev(PointerPhase::Up, x, 28.0));
        assert_eq!(state.query, "", "the built-in clear empties the query");
    }

    // ---- Expand spring ----

    #[test]
    fn the_expand_spring_matches_the_reference_focus_expand_spring() {
        // `M3ESearchBarTheme.focusExpandSpring` defaults to
        // `M3EMotion.expressiveSpatialPress` (380 / 0.55).
        let token = crate::tokens::MaterialSpring::EXPRESSIVE_SPATIAL_PRESS;
        assert_eq!(EXPAND_SPRING.stiffness, token.stiffness);
        assert_eq!(EXPAND_SPRING.damping_ratio, token.damping_ratio);
        assert_eq!(EXPAND_SPRING.mass, 1.0);
        assert_eq!(
            EXPAND_ACTIVE,
            EXPAND_REST / 2.0,
            "the reference's _focusedExpandPadding is half the resting inset"
        );
    }

    #[test]
    fn a_press_springs_the_inset_toward_the_active_value_and_back() {
        let view = bar("").on_tap(|s: &mut App| s.taps += 1);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));
        assert_eq!(w.expand_inset(), EXPAND_REST);

        let mut state = App::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, 28.0));
        // Advance the spring past its settling time.
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, WINDOW, FrameTime::ZERO);
        w.paint(&mut pctx, &mut rec);
        let mut pctx =
            PaintCtx::for_test(Point::ZERO, WINDOW, FrameTime::from_nanos(3_000_000_000));
        w.paint(&mut pctx, &mut rec);
        assert!(
            (w.expand_inset() - EXPAND_ACTIVE).abs() < 0.5,
            "a held press settles at the active inset, got {}",
            w.expand_inset()
        );

        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 200.0, 28.0));
        // The release starts a fresh leg: one paint seeds its clock, the next
        // (well past the spring's settling time) reads it settled.
        for t in [3_500_000_000u64, 7_000_000_000] {
            let mut pctx = PaintCtx::for_test(Point::ZERO, WINDOW, FrameTime::from_nanos(t));
            w.paint(&mut pctx, &mut rec);
        }
        assert!(
            (w.expand_inset() - EXPAND_REST).abs() < 0.5,
            "the release springs back to the resting inset, got {}",
            w.expand_inset()
        );
    }

    #[test]
    fn reduce_motion_snaps_the_inset_with_no_ramp() {
        let theme = crate::baseline();
        let mut theme = theme;
        theme.motion.reduce_motion = true;
        let view = bar("").on_tap(|s: &mut App| s.taps += 1);
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(Size::new(400.0, 56.0)));

        let mut state = App::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 200.0, 28.0));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, WINDOW, FrameTime::ZERO).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);
        assert_eq!(w.expand_inset(), EXPAND_ACTIVE);
    }

    // ---- Semantics ----

    #[test]
    fn semantics_is_a_search_input_labelled_by_the_hint() {
        fn logic(_s: &mut App) -> SearchBarView<App> {
            search_bar("", |_: &mut App, _| {}).hint("Search recipes")
        }
        let mut root: RenderRoot<App, SearchBarView<App>> = RenderRoot::new();
        let mut state = App::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 56.0), &mut tcx as &mut dyn Any);

        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::SearchInput)
            .expect("a Role::SearchInput node is contributed");
        assert_eq!(node.label(), Some("Search recipes"));
    }

    /// An `App`-stated fixed leaf, for the explicit-trailing test (the shared
    /// `leaf_any` fixture is `AnyView<()>`).
    fn leaf_any_app(w: f64, h: f64) -> AnyView<App> {
        core_any::<App, _>(FixedLeaf {
            size: Size::new(w, h),
        })
    }
    struct FixedLeaf {
        size: Size,
    }
    struct FixedLeafW {
        size: Size,
    }
    impl View<App> for FixedLeaf {
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
}
