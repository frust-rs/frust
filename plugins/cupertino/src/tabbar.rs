//! `CupertinoTabBar`: the iOS bottom tab bar — a 49pt content-height band of
//! icon+label destinations, systemBlue when selected and secondaryLabel
//! otherwise.
//!
//! # Floating glass pill
//!
//! The bar is a **floating inset pill** (the iOS 26 Liquid-Glass idiom: it
//! floats above content with horizontal margins and capsule corners) rather
//! than a full-width docked slab. Its background is read from the theme's
//! `bar`-tier glass token (`Theme.glass.bar`, `frust_theme::glass`), never
//! a hardcoded fill: on the Cupertino design language that tier is a
//! translucent Liquid-Glass lens, so the pill composites the token's wash stack
//! (chosen by the active brightness) over the live content behind it (no real
//! backdrop blur yet), then strokes a **specular hairline**
//! whose alpha is the tier's `hairline_alpha` as a concentric inset outline
//! (radius via [`ShapeScale::concentric_inner`], nested inside the pill). On the
//! **Material** design language (or a bare-core/unthemed bar) `glass.bar` is
//! opaque, so the same code fills the pill with the opaque surface color and
//! draws the token's elevation shadow instead — the opaque path.
//!
//! # Minimize-on-scroll
//!
//! The floating bar **minimizes on downward scroll and restores on upward
//! scroll**, spring-animated with the theme's Cupertino
//! [`MotionScheme`](frust::MotionScheme) spring. The minimize *state
//! machine* (`MinimizeState`) is a pure toggle driven off the vertical scroll
//! delta sign (matching the `offset + dy` convention every scrollable widget
//! in this framework uses — a positive delta scrolls toward later content and
//! minimizes; a negative delta restores); the spring *animation* is driven at
//! paint time from the frame clock, so the two split exactly like every other
//! controlled widget (event = state, paint = motion; see
//! `docs/CODE_STANDARDS.md`).
//!
//! The bar consumes the scroll signal through its own
//! [`InputEvent::Scroll`](frust::authoring::InputEvent) handler — the existing
//! input scroll surface (`frust-core::input`), the only cross-widget scroll
//! signal exposed today. A floating bar minimizing in response to a *sibling*
//! scroll view's content scroll (rather than a scroll that lands on the bar
//! itself) would need a scroll-delta signal the baseline scroll view does not
//! expose; wiring one is out of scope here. The
//! minimize state machine and spring wiring are exercised at the state level
//! regardless.
//!
//! `CupertinoTabBarView`/`CupertinoTabBarWidget` are a **controlled** component
//! (see `docs/CODE_STANDARDS.md`'s Interaction Semantics), mirroring a
//! Material `navbar`: the view reports the *requested* selection
//! through `on_select(index)` and never self-mutates; the app feeds the
//! confirmed `selected` index back in on the next rebuild.
//!
//! Each item is a small retained `TabItemWidget` (not `View`/[`AnyView`]-erased
//! — there is only one concrete item type) wrapped in a [`ChildPod`] purely so
//! the bar can route pointer/semantics through the shared
//! [`frust::authoring::route_event`]/[`ChildPod::semantics_child`] helpers. Each item lays
//! out an optional caller-supplied `icon` (an opaque [`AnyView`] — tint is the
//! caller's responsibility) above a `label` (a real child [`frust::text`]),
//! and fires `on_select` on release-inside like [`frust::Button`].
//!
//! # Label color
//!
//! An **unselected** label is a child [`frust::text`] themed
//! `on_surface_variant` (iOS secondaryLabel), so it live-swaps with the theme.
//! A **selected** label is painted with an explicit `SYSTEM_BLUE` — iOS's
//! systemBlue is *community-measured*, not Apple-published, and has no
//! deferred-color `Text` role to resolve from, so it is baked at build time
//! (a light↔dark systemBlue nuance therefore does not live-swap — an accepted,
//! documented limitation, pending future work on the widgets' runtime iOS
//! look).
//!
//! # Safe area
//!
//! This widget lays out only the 49pt content band. The home-indicator safe-area
//! inset a real iOS tab bar extends into is **shell-future work**, not yet
//! wired — an app currently supplies its own bottom padding.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::Role;
use frust::authoring::text::LineHeight;
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase, ScrollDelta,
    SemanticsCtx, ThemeTextColor, View, Widget,
};
use frust::input::WHEEL_LINE_PX;
use frust::{
    AnimationController, Brightness, FrameTime, GlassFill, GlassMaterial, ShapeScale, SpringDesc,
    Theme, text,
};
use kurbo::{Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

/// Content height of the iOS tab bar, in logical px (source: Apple HIG — the
/// standard tab bar content band is 49pt, excluding the home-indicator safe
/// area a shell adds; see the [module docs](self)'s safe-area note).
const HEIGHT: f64 = 49.0;
/// Top inset from the bar's top edge to an item's icon, in logical px.
const PAD_TOP: f64 = 4.0;
/// Gap between an item's icon and its label below it, in logical px.
const LABEL_GAP: f64 = 2.0;

/// The label's iOS *Caption* type-role size, in logical px.
///
/// **Community-approximate**: iOS tab-bar labels render at roughly SF Caption2
/// (~10pt); this is the community-converged size, not an Apple-published tab-bar
/// metric.
const LABEL_SIZE: f32 = 10.0;
const LABEL_LINE_HEIGHT: f32 = 12.0;

/// systemBlue (the selected item tint).
///
/// **Kit-measured** (2026-07-18 refresh): the iOS 27 UI Kit's `System
/// Colors/Light/8 Blue` swatch is
/// `#0087FF`, matching `ColorScheme::cupertino_light().primary` exactly —
/// Apple does not publish an exact hex for the system accent colors (they
/// vary by trait environment), so this remains a design-tool snapshot rather
/// than a guarantee, just a newer/more-precise one than the pre-refresh
/// community value it replaces (`#007AFF`), mirroring `switch.rs`'s
/// `SYSTEM_GREEN_LIGHT` refresh note (see also `color.rs`'s module docs).
/// Baked as an explicit label color (see the [module docs](self)'s Label
/// color note).
const SYSTEM_BLUE: Color = Color::from_rgb8(0x00, 0x87, 0xFF);

/// Unthemed fallback bar fill (a theme resolves this from `colors.surface`,
/// iOS systemBackground).
const CONTAINER: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed fallback hairline color (a theme resolves this from
/// `colors.outline_variant`, iOS separator).
///
/// **Composite-derived** (re-derived for the 2026-07-18 kit refresh — see
/// [`super::navbar`]'s `SEPARATOR` for the full derivation, mirrored here):
/// `0.12` black over white ≈ `0xE0E0E0`, superseding the pre-refresh
/// `#C6C6C8` approximation of the old translucent `rgba(60,60,67,0.29)`
/// token.
const SEPARATOR: Color = Color::from_rgb8(0xE0, 0xE0, 0xE0);
/// The hairline separator's stroke width, in logical px (see
/// [`super::navbar`]'s identical hairline note).
const HAIRLINE_W: f64 = 1.0;

/// Horizontal inset from the widget's edges to the floating pill, in logical
/// px.
///
/// **Community-approximate**: iOS 26's floating tab bar side margin is not an
/// Apple-published constant; ~16pt is the community-converged inset (it matches
/// the standard content layout margin the bar aligns to).
const MARGIN_X: f64 = 16.0;
/// Vertical inset (top and bottom) from the widget's band edges to the floating
/// pill, in logical px. The widget's total height is therefore
/// `HEIGHT + 2 * MARGIN_V`; the app still supplies the home-indicator safe-area
/// padding below that (see the [module docs](self)'s Safe area note).
///
/// **Community-approximate**: like [`MARGIN_X`], not an Apple-published figure.
const MARGIN_V: f64 = 8.0;
/// Inset of the specular hairline outline from the pill edge, in logical px —
/// the nesting depth its concentric-inner corner radius is derived from.
const HAIRLINE_INSET: f64 = 1.0;
/// Flatness tolerance for tessellating the specular hairline's rounded-rect
/// outline into a [`kurbo::BezPath`] (mirrors a Material `split_button`'s
/// `PATH_TOLERANCE`).
const PATH_TOLERANCE: f64 = 0.1;

/// The specular-highlight color of a glass edge — white, its alpha supplied by
/// the tier's `hairline_alpha` token. See [`super::navbar`]'s identical
/// `SPECULAR` note (the recipe stores only the alpha; white is the fixed
/// specular color, not a recipe rgba).
const SPECULAR: Color = Color::WHITE;

/// The resolved `(container, separator)` colors for the **opaque** path
/// (Material design language, or an unthemed bar). Themed: `colors.surface` /
/// `colors.outline_variant`. Unthemed: the [`CONTAINER`]/[`SEPARATOR`] constants.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => (theme.scheme().surface, theme.scheme().outline_variant),
        None => (CONTAINER, SEPARATOR),
    }
}

/// The Liquid-Glass `bar`-tier material to paint from, or `None` when the bar
/// should take the **opaque** path instead: no theme, or a theme whose
/// `glass.bar` is opaque (the Material design language). Mirrors
/// [`super::navbar`]'s helper of the same name — the single-API branch that
/// keeps Material bars opaque while Cupertino bars read glass.
fn glass_bar(theme: Option<&Theme>) -> Option<&GlassMaterial> {
    let material = &theme?.glass.bar;
    (!material.is_opaque()).then_some(material)
}

/// The wash stack to composite for the active `brightness`, bottom-to-top —
/// read purely from the tier token.
fn glass_fills(material: &GlassMaterial, brightness: Brightness) -> &[GlassFill] {
    match brightness {
        Brightness::Light => &material.fills_light,
        Brightness::Dark => &material.fills_dark,
    }
}

/// Return `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// [`SPECULAR`] white with its alpha replaced by the tier's `hairline_alpha`.
fn specular(alpha: f32) -> Color {
    with_alpha(SPECULAR, alpha)
}

/// The capsule corner radius of the floating pill (`min(w, h) / 2` — a true
/// pill), resolved via [`ShapeScale::resolve`] from the `full` token.
fn pill_radius(pill_size: Size) -> f64 {
    ShapeScale::resolve(f64::INFINITY, pill_size.width, pill_size.height)
}

/// Stroke the specular hairline as a **concentric inset outline** nested inside
/// the pill: inset by [`HAIRLINE_INSET`] on all sides, with its corner radius
/// derived from the pill's via [`ShapeScale::concentric_inner`] so inner and
/// outer capsules share a geometric center (the Liquid-Glass concentric-corner
/// principle). `pill_origin` is absolute; the built path is already absolute so
/// it is stroked at the origin.
fn stroke_specular_outline(
    scene: &mut dyn PaintScene,
    pill_origin: Point,
    pill_size: Size,
    outer_radius: f64,
    hairline_alpha: f32,
) {
    let inner = Rect::from_origin_size(
        Point::new(
            pill_origin.x + HAIRLINE_INSET,
            pill_origin.y + HAIRLINE_INSET,
        ),
        Size::new(
            (pill_size.width - 2.0 * HAIRLINE_INSET).max(0.0),
            (pill_size.height - 2.0 * HAIRLINE_INSET).max(0.0),
        ),
    );
    let radius = ShapeScale::concentric_inner(outer_radius, HAIRLINE_INSET);
    let path = RoundedRect::from_rect(inner, radius).to_path(PATH_TOLERANCE);
    scene.stroke_path(
        Point::ZERO,
        &path,
        HAIRLINE_W,
        &Brush::Solid(specular(hairline_alpha)),
    );
}

/// Build a tab label's type-erased child view. Selected → explicit
/// [`SYSTEM_BLUE`]; unselected → the live `OnSurfaceVariant` (secondaryLabel)
/// role. See the [module docs](self)'s Label color note.
fn label_view<State: 'static>(label: String, selected: bool) -> AnyView<State> {
    let base = text(label)
        .size(LABEL_SIZE)
        .line_height(LineHeight::Absolute(LABEL_LINE_HEIGHT));
    let styled = if selected {
        base.color(SYSTEM_BLUE)
    } else {
        base.themed_role(ThemeTextColor::OnSurfaceVariant)
    };
    frust::authoring::any::<State, _>(styled)
}

/// Whether a widget-local `pos` lies within a `size`-sized box anchored at the
/// origin.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// One destination's declarative content: an optional leading icon (an opaque
/// [`AnyView`] — tint is the caller's responsibility) and a text label.
pub struct TabItem<State: 'static> {
    icon: Option<AnyView<State>>,
    label: String,
}

/// Create a tab item labelled `label`, with no icon (attach one with
/// [`TabItem::icon`]).
pub fn tab_item<State: 'static>(label: impl Into<String>) -> TabItem<State> {
    TabItem {
        icon: None,
        label: label.into(),
    }
}

impl<State: 'static> TabItem<State> {
    /// Attach a leading icon, erased as an [`AnyView`]. Tint is the supplied
    /// view's own responsibility — see the [module docs](self).
    pub fn icon(mut self, icon: AnyView<State>) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// A view-held, typed selection callback (erased per-item on build/rebuild).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative iOS bottom tab bar. See the [module docs](self).
pub struct CupertinoTabBarView<State: 'static> {
    items: Vec<TabItem<State>>,
    selected: usize,
    on_select: OnSelect<State>,
}

/// Create a tab bar over `items`, with `selected` the current (app-confirmed)
/// index. Fires `on_select(state, index)` on a release inside an item — a
/// **controlled** component: `selected` is never self-mutated.
pub fn cupertino_tab_bar<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<TabItem<State>>,
    selected: usize,
    on_select: F,
) -> CupertinoTabBarView<State> {
    CupertinoTabBarView {
        items,
        selected,
        on_select: Rc::new(on_select),
    }
}

/// PascalCase alias for [`cupertino_tab_bar`].
#[allow(non_snake_case)]
pub fn CupertinoTabBar<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<TabItem<State>>,
    selected: usize,
    on_select: F,
) -> CupertinoTabBarView<State> {
    cupertino_tab_bar(items, selected, on_select)
}

/// Erase `on_select` into a per-item callback that always reports `idx`.
fn item_on_select<State: 'static>(
    on_select: &OnSelect<State>,
    idx: usize,
) -> frust::authoring::ErasedCallback {
    let callback = on_select.clone();
    Box::new(move |ctx: &mut EventCtx| {
        let state = ctx.state_mut::<State>();
        callback(state, idx);
    })
}

/// One destination's retained content (see the [module docs](self)).
struct TabItemWidget {
    icon: Option<ChildPod>,
    label: ChildPod,
    label_text: String,
    selected: bool,
    on_select: frust::authoring::ErasedCallback,
    /// Armed by a `Down`, cleared on `Up`/`Cancel` — the fire-on-up-inside
    /// contract [`frust::Button`] uses.
    captured: bool,
}

/// Build one item's retained [`ChildPod`] (wrapping a fresh [`TabItemWidget`]).
fn build_item<State: 'static>(
    item: &TabItem<State>,
    selected: bool,
    on_select: &OnSelect<State>,
    idx: usize,
    ctx: &mut BuildCtx<'_>,
) -> ChildPod {
    let label_view = label_view::<State>(item.label.clone(), selected);
    let icon = item
        .icon
        .as_ref()
        .map(|icon| frust::authoring::build_child(icon, ctx));
    let label = frust::authoring::build_child(&label_view, ctx);
    let widget = TabItemWidget {
        icon,
        label,
        label_text: item.label.clone(),
        selected,
        on_select: item_on_select::<State>(on_select, idx),
        captured: false,
    };
    ChildPod::new(Box::new(widget))
}

impl Widget for TabItemWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = bc.max().width;
        let mut icon_bottom = PAD_TOP;
        if let Some(icon) = self.icon.as_mut() {
            let icon_size = icon.layout_child(
                ctx,
                &BoxConstraints::loose(Size::new(f64::INFINITY, HEIGHT)),
            );
            let icon_x = (width - icon_size.width) / 2.0;
            icon.set_origin(Point::new(icon_x, PAD_TOP));
            icon_bottom = PAD_TOP + icon_size.height;
        }
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(Size::new(width, f64::INFINITY)));
        let label_x = (width - label_size.width) / 2.0;
        self.label
            .set_origin(Point::new(label_x, icon_bottom + LABEL_GAP));
        bc.constrain(Size::new(width, HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if let Some(icon) = self.icon.as_mut() {
            icon.paint_child(ctx, scene);
        }
        self.label.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                self.captured = true;
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    (self.on_select)(ctx);
                }
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Tab, |node| {
            node.set_label(self.label_text.as_str());
            node.set_selected(self.selected);
        });
    }

    frust::authoring::visit_children!(icon, label);
}

/// Synthesize a [`PointerPhase::Cancel`] into a still-armed item pod (mirrors
/// a Material `navbar`'s `cancel_item`, reimplemented locally since a
/// removed item's `TabItemWidget` isn't `AnyView`-wrapped).
fn cancel_item(pod: &mut ChildPod) {
    let mut dummy_state = ();
    let mut ctx = EventCtx::new(&mut dummy_state, pod.origin(), pod.size());
    let cancel = InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Cancel,
        position: Point::ZERO,
        button: PointerButton::Primary,
    });
    pod.event_child(&mut ctx, &cancel);
}

/// Minimum vertical scroll delta (logical px) that flips the minimize state — a
/// deadzone so incidental sub-pixel scroll jitter never toggles the bar.
const MINIMIZE_DEADZONE: f64 = 1.0;

/// Nominal spring launch velocity (progress-units/sec) for a minimize/restore
/// toggle. The state machine carries no measured fling velocity, so it kicks
/// the Cupertino spring with a fixed magnitude whose *sign* selects the target
/// (`+` → minimized/`1.0`, `-` → restored/`0.0`; see
/// [`AnimationController::fling`]).
///
/// **Community-approximate**: iOS's exact tab-bar minimize velocity is not a
/// published constant; this is a tuned Frust value giving a brisk,
/// gently-overshooting settle under the Cupertino spring.
const MINIMIZE_SPRING_VELOCITY: f64 = 4.0;

/// Fallback Cupertino spring used to drive the minimize animation when no theme
/// is threaded into paint (bare-core/pre-theme). Mirrors
/// `crate::tokens::motion_scheme`'s single baseline (mass 1.0,
/// stiffness 170.0, damping ratio ≈ 0.5753); a themed bar uses
/// `theme.motion.default_spatial` instead.
const FALLBACK_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 170.0,
    damping_ratio: 0.5753,
};

/// The floating tab bar's minimize-on-scroll state machine: a pure
/// toggle (`minimized`) plus the spring-driven `0.0` (shown) → `1.0`
/// (minimized) `progress` the paint pass animates.
///
/// Split by pass, like every controlled widget: [`MinimizeState::apply_scroll`]
/// runs on the event pass (no theme) and only flips the boolean target;
/// [`MinimizeState::drive`] runs on the paint pass (theme available) and starts
/// the Cupertino spring toward the target on a change, then advances it off the
/// frame clock.
struct MinimizeState {
    /// The current target: `true` = minimized (compact/slid out).
    minimized: bool,
    /// Spring-driven progress, `0.0` (fully shown) .. `1.0` (minimized).
    progress: AnimationController,
    /// The target the `progress` controller is currently driving toward
    /// (`0.0`/`1.0`), so `drive` restarts the spring only on a real change.
    anim_target: f64,
}

impl MinimizeState {
    fn new() -> Self {
        Self {
            minimized: false,
            // Spring-driven, so the duration is unused; a nominal value keeps
            // the controller valid.
            progress: AnimationController::new(Duration::from_millis(300)),
            anim_target: 0.0,
        }
    }

    /// Apply a vertical scroll delta `dy` (logical px, the `offset + dy` sign
    /// convention every scrollable widget in this framework uses): a
    /// positive delta past the deadzone minimizes, a negative one restores.
    /// Returns whether the target changed.
    fn apply_scroll(&mut self, dy: f64) -> bool {
        let want = if dy > MINIMIZE_DEADZONE {
            true
        } else if dy < -MINIMIZE_DEADZONE {
            false
        } else {
            self.minimized
        };
        let changed = want != self.minimized;
        self.minimized = want;
        changed
    }

    /// Progress toward the minimized state, clamped to `0.0..=1.0` (the spring's
    /// transient overshoot is clipped for the geometric slide).
    fn progress(&self) -> f64 {
        self.progress.value_clamped()
    }

    /// Drive the spring toward the current `minimized` target using `spring`
    /// (the theme's Cupertino spring) and advance it to frame time `now`.
    /// Restarts the spring only when the target changed since the last drive.
    /// Returns whether the animation is still in flight (the caller should then
    /// request another frame).
    fn drive(&mut self, spring: SpringDesc, now: FrameTime) -> bool {
        let want = if self.minimized { 1.0 } else { 0.0 };
        if (self.anim_target - want).abs() > f64::EPSILON {
            // Sign of the launch velocity selects the fling target.
            let velocity = if want > self.anim_target {
                MINIMIZE_SPRING_VELOCITY
            } else {
                -MINIMIZE_SPRING_VELOCITY
            };
            self.progress.fling(velocity, spring);
            self.anim_target = want;
        }
        self.progress.advance(now)
    }
}

/// The retained widget for a [`CupertinoTabBarView`].
pub struct CupertinoTabBarWidget {
    items: Vec<ChildPod>,
    /// The floating pill's minimize-on-scroll animation state.
    minimize: MinimizeState,
}

impl<State: 'static> View<State> for CupertinoTabBarView<State> {
    type Element = CupertinoTabBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CupertinoTabBarWidget {
        let mut items = Vec::with_capacity(self.items.len());
        for (i, item) in self.items.iter().enumerate() {
            items.push(build_item(
                item,
                i == self.selected,
                &self.on_select,
                i,
                ctx,
            ));
        }
        CupertinoTabBarWidget {
            items,
            minimize: MinimizeState::new(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CupertinoTabBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        let common = prev.items.len().min(self.items.len());

        for i in 0..common {
            let prev_item = &prev.items[i];
            let next_item = &self.items[i];
            let was_selected = i == prev.selected;
            let now_selected = i == self.selected;
            let pod = &mut element.items[i];
            let widget = pod
                .widget_mut()
                .downcast_mut::<TabItemWidget>()
                .expect("tab item pod holds a TabItemWidget");

            match (&prev_item.icon, &next_item.icon) {
                (Some(p), Some(n)) => {
                    flags |= frust::authoring::rebuild_child(
                        p,
                        n,
                        widget.icon.as_mut().expect("icon pod present"),
                        ctx,
                    );
                }
                (None, Some(n)) => {
                    widget.icon = Some(frust::authoring::build_child(n, ctx));
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                (Some(p), None) => {
                    if let Some(mut old) = widget.icon.take() {
                        frust::authoring::teardown_child(p, &mut old, ctx);
                    }
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                (None, None) => {}
            }

            if prev_item.label != next_item.label || was_selected != now_selected {
                widget.label_text = next_item.label.clone();
                widget.selected = now_selected;
                let prev_view = label_view::<State>(prev_item.label.clone(), was_selected);
                let next_view = label_view::<State>(next_item.label.clone(), now_selected);
                flags |=
                    frust::authoring::rebuild_child(&prev_view, &next_view, &mut widget.label, ctx);
                flags |= ChangeFlags::PAINT;
            }

            // Closures aren't comparable — always reinstall the adapter.
            widget.on_select = item_on_select::<State>(&self.on_select, i);
        }

        if self.items.len() > prev.items.len() {
            for (offset, item) in self.items[common..].iter().enumerate() {
                let idx = common + offset;
                element.items.push(build_item(
                    item,
                    idx == self.selected,
                    &self.on_select,
                    idx,
                    ctx,
                ));
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if self.items.len() < prev.items.len() {
            for (offset, item) in prev.items[common..].iter().enumerate() {
                let idx = common + offset;
                let pod = &mut element.items[idx];
                if pod.is_active() {
                    cancel_item(pod);
                    pod.set_active(false);
                }
                let widget = pod
                    .widget_mut()
                    .downcast_mut::<TabItemWidget>()
                    .expect("tab item pod holds a TabItemWidget");
                if let (Some(icon_view), Some(icon_pod)) = (&item.icon, widget.icon.as_mut()) {
                    frust::authoring::teardown_child(icon_view, icon_pod, ctx);
                }
                let label_view = label_view::<State>(item.label.clone(), idx == prev.selected);
                frust::authoring::teardown_child(&label_view, &mut widget.label, ctx);
            }
            element.items.truncate(common);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut CupertinoTabBarWidget, ctx: &mut BuildCtx<'_>) {
        for (i, (item, pod)) in self.items.iter().zip(element.items.iter_mut()).enumerate() {
            let widget = pod
                .widget_mut()
                .downcast_mut::<TabItemWidget>()
                .expect("tab item pod holds a TabItemWidget");
            if let (Some(icon_view), Some(icon_pod)) = (&item.icon, widget.icon.as_mut()) {
                frust::authoring::teardown_child(icon_view, icon_pod, ctx);
            }
            let label_view = label_view::<State>(item.label.clone(), i == self.selected);
            frust::authoring::teardown_child(&label_view, &mut widget.label, ctx);
        }
    }
}

impl CupertinoTabBarWidget {
    /// The total widget band height: the 49pt content pill plus the vertical
    /// floating margins above and below it.
    fn band_height() -> f64 {
        HEIGHT + 2.0 * MARGIN_V
    }

    /// The floating pill's inner width for a given widget `width` (the band
    /// width minus the horizontal margins), never negative.
    fn pill_width(width: f64) -> f64 {
        (width - 2.0 * MARGIN_X).max(0.0)
    }

    /// The relative origin of item `i`'s slot within the widget band, at a given
    /// widget `width`, before the minimize slide is applied.
    fn item_base_origin(width: f64, i: usize, n: usize) -> Point {
        let slot_w = Self::pill_width(width) / n as f64;
        Point::new(MARGIN_X + i as f64 * slot_w, MARGIN_V)
    }
}

impl Widget for CupertinoTabBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let n = self.items.len();
        if n == 0 {
            return bc.constrain(Size::new(width, Self::band_height()));
        }
        let slot_w = Self::pill_width(width) / n as f64;
        for (i, pod) in self.items.iter_mut().enumerate() {
            pod.layout_child(ctx, &BoxConstraints::tight(Size::new(slot_w, HEIGHT)));
            pod.set_origin(Self::item_base_origin(width, i, n));
        }
        bc.constrain(Size::new(width, Self::band_height()))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Drive the minimize spring off the frame clock (event set the target;
        // paint animates it — see the module docs). Request another frame while
        // it is still in flight so the desktop `ControlFlow::Wait` loop keeps
        // scheduling. The theme borrow for the spring is scoped to this
        // statement so `ctx.request_frame()` (a mutable borrow) is free below.
        let frame_time = ctx.frame_time();
        let spring = Theme::from_paint_ctx(ctx)
            .map(|t| SpringDesc::from(t.motion.default_spatial))
            .unwrap_or(FALLBACK_SPRING);
        if self.minimize.drive(spring, frame_time) {
            ctx.request_frame();
        }
        let slide = self.minimize.progress() * (HEIGHT + MARGIN_V);

        let origin = ctx.origin();
        let width = ctx.size().width;
        let pill_size = Size::new(Self::pill_width(width), HEIGHT);
        let pill_origin = Point::new(origin.x + MARGIN_X, origin.y + MARGIN_V + slide);
        let radius = pill_radius(pill_size);

        // Paint the pill background under a scoped theme borrow (no mutable
        // `ctx` use inside — `scene` is a separate borrow), so the item loop
        // below can borrow `ctx` mutably again.
        {
            let theme = Theme::from_paint_ctx(ctx);

            // Drop shadow from the `bar` tier: the ios27 glass bar carries none
            // (`color_alpha == 0.0`), the opaque Material bar carries its
            // elevation shadow. Drawn only when the token asks for one.
            if let Some(t) = theme {
                let sh = t.glass.bar.shadow;
                if sh.color_alpha > 0.0 {
                    scene.draw_shadow(
                        Point::new(pill_origin.x, pill_origin.y + sh.y_offset),
                        pill_size,
                        radius,
                        sh.blur_std_dev,
                        with_alpha(t.scheme().shadow, sh.color_alpha),
                    );
                }
            }

            match glass_bar(theme) {
                // Cupertino Liquid Glass: composite the token's wash stack into
                // the pill, then stroke the concentric specular hairline.
                Some(material) => {
                    let brightness = theme.map(|t| t.brightness).unwrap_or_default();
                    for fill in glass_fills(material, brightness) {
                        scene.fill_rounded_rect(pill_origin, pill_size, radius, fill.color);
                    }
                    stroke_specular_outline(
                        scene,
                        pill_origin,
                        pill_size,
                        radius,
                        material.hairline_alpha,
                    );
                }
                // Opaque path (Material design language, or an unthemed bar):
                // fill the floating pill with the opaque surface color.
                None => {
                    let (container, _) = resolve_colors(theme);
                    scene.fill_rounded_rect(pill_origin, pill_size, radius, container);
                }
            }
        }

        // Slide the item pods to match the pill, then paint them. (Layout reset
        // them to their base origins this frame; overriding here keeps the
        // paint-driven slide out of layout.)
        let n = self.items.len();
        for (i, pod) in self.items.iter_mut().enumerate() {
            let base = Self::item_base_origin(width, i, n);
            pod.set_origin(Point::new(base.x, base.y + slide));
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Consume the input scroll signal to drive the minimize state machine
        // (see the module docs): the target flip happens here; the spring
        // animation is driven at paint.
        if let InputEvent::Scroll { delta, .. } = event {
            let dy = match delta {
                ScrollDelta::Lines(_, y) => y * WHEEL_LINE_PX,
                ScrollDelta::Pixels(_, y) => *y,
            };
            if self.minimize.apply_scroll(dy) {
                ctx.request_redraw();
            }
            return EventResult::Handled;
        }
        frust::authoring::route_event(&mut self.items, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::TabList,
            |_| {},
            |ctx| {
                for pod in &self.items {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(items);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    fn build(view: &CupertinoTabBarView<()>) -> CupertinoTabBarWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut ctx(&mut counter))
    }

    fn layout(w: &mut CupertinoTabBarWidget, bc: &BoxConstraints) -> Size {
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        w.layout(&mut lctx, bc)
    }

    fn three_items() -> Vec<TabItem<()>> {
        vec![tab_item("Home"), tab_item("Search"), tab_item("Profile")]
    }

    #[test]
    fn layout_is_a_floating_pill_dividing_the_inset_width_evenly() {
        // The bar is a floating inset pill — the band is
        // `HEIGHT + 2*MARGIN_V` tall, and items divide the horizontally-inset
        // pill width, laid out at `MARGIN_V` from the band top.
        let view: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size, Size::new(300.0, HEIGHT + 2.0 * MARGIN_V));
        let pill_w = 300.0 - 2.0 * MARGIN_X;
        let slot_w = pill_w / 3.0;
        assert_eq!(w.items[0].origin(), Point::new(MARGIN_X, MARGIN_V));
        assert_eq!(w.items[1].origin(), Point::new(MARGIN_X + slot_w, MARGIN_V));
        assert_eq!(
            w.items[2].origin(),
            Point::new(MARGIN_X + 2.0 * slot_w, MARGIN_V)
        );
        assert_eq!(w.items[0].size(), Size::new(slot_w, HEIGHT));
    }

    /// A recorder capturing filled rects, filled rounded rects (with radius and
    /// color), and stroked lines.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rounded: Vec<(Point, Size, f64, Color)>,
        lines: Vec<(Point, Point, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, c: Color) {
            self.rounded.push((o, s, radius, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn stroke_line(&mut self, p0: Point, p1: Point, width: f64, color: Color) {
            self.lines.push((p0, p1, width, color));
        }
    }

    /// The widget's full band size for a given width (a floating pill reserves
    /// vertical margins above and below the 49pt content).
    fn band(width: f64) -> Size {
        Size::new(width, HEIGHT + 2.0 * MARGIN_V)
    }

    #[test]
    fn unthemed_paint_fills_a_floating_pill_with_the_container_color() {
        // Unthemed → the opaque path: a rounded floating pill filled with the
        // fallback container color, inset by the floating margins.
        let view: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut scene = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, band(300.0));
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rounded.len(), 1, "one opaque pill fill");
        let (o, s, radius, c) = scene.rounded[0];
        assert_eq!(o, Point::new(MARGIN_X, MARGIN_V));
        assert_eq!(s, Size::new(300.0 - 2.0 * MARGIN_X, HEIGHT));
        assert_eq!(radius, HEIGHT / 2.0, "capsule corners");
        assert_eq!(c, CONTAINER);
    }

    #[test]
    fn cupertino_theme_paints_the_glass_pill_wash_stack() {
        // The cupertino `bar` tier is a glass lens, so the pill
        // composites the token's over-light wash stack (read purely from
        // `Theme.glass.bar.fills_light`) as rounded-rect washes — the fill
        // stack reaches the paint layer.
        let theme = crate::baseline();
        let material = &theme.glass.bar;
        assert!(
            !material.is_opaque(),
            "the cupertino bar tier is a glass lens"
        );

        let view: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(&theme as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut scene = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, band(300.0)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);

        let washes: Vec<Color> = material.fills_light.iter().map(|f| f.color).collect();
        assert!(!washes.is_empty());
        let painted: Vec<Color> = scene.rounded.iter().map(|(_, _, _, c)| *c).collect();
        assert_eq!(
            painted, washes,
            "one rounded wash per fill-stack entry, in order"
        );
        // Each wash is a capsule inset by the floating margins.
        for (o, s, radius, _) in &scene.rounded {
            assert_eq!(*o, Point::new(MARGIN_X, MARGIN_V));
            assert_eq!(*s, Size::new(300.0 - 2.0 * MARGIN_X, HEIGHT));
            assert_eq!(*radius, HEIGHT / 2.0);
        }
    }

    #[test]
    fn material_theme_keeps_the_bar_opaque() {
        // An M3 theme's `glass.bar` is opaque, so the same code
        // fills the pill with the opaque surface color — never the wash stack.
        let theme = Theme::neutral();
        assert!(theme.glass.bar.is_opaque());
        let view: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(&theme as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut scene = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, band(300.0)).with_theme(&theme);
        w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rounded.len(), 1, "one opaque pill fill");
        assert_eq!(scene.rounded[0].3, theme.scheme().surface);
    }

    #[test]
    fn minimize_state_machine_toggles_on_scroll_direction() {
        // Scroll-down → minimized, scroll-up → restored, at the state level.
        let mut m = MinimizeState::new();
        assert!(!m.minimized, "starts fully shown");

        // A downward scroll (positive delta, past the deadzone) minimizes.
        assert!(m.apply_scroll(40.0), "target changed");
        assert!(m.minimized);
        // A further downward scroll leaves it minimized (no spurious change).
        assert!(!m.apply_scroll(40.0), "already minimized, no change");
        assert!(m.minimized);

        // An upward scroll restores.
        assert!(m.apply_scroll(-40.0), "target changed");
        assert!(!m.minimized);

        // A sub-deadzone jitter never toggles.
        assert!(!m.apply_scroll(0.5));
        assert!(!m.minimized);
        assert!(!m.apply_scroll(-0.5));
        assert!(!m.minimized);
    }

    #[test]
    fn minimize_spring_advances_progress_toward_the_minimized_target() {
        // The spring wiring: after a minimize toggle, driving the Cupertino
        // spring off the frame clock moves `progress` off zero toward 1.0.
        let mut m = MinimizeState::new();
        assert_eq!(m.progress(), 0.0);
        m.apply_scroll(40.0); // → minimized
        let spring = SpringDesc::from(crate::baseline().motion.default_spatial);
        // First drive seeds the clock (zero delta); the next advances.
        m.drive(spring, FrameTime::from_nanos(0));
        assert_eq!(m.progress(), 0.0, "the seeding frame makes no progress");
        m.drive(spring, FrameTime::from_nanos(50_000_000)); // +50ms
        assert!(m.progress() > 0.0, "the spring advanced toward minimized");

        // Restoring reverses the target and drives back toward zero.
        m.apply_scroll(-40.0);
        m.drive(spring, FrameTime::from_nanos(60_000_000));
        let restoring = m.progress();
        m.drive(spring, FrameTime::from_nanos(400_000_000));
        assert!(
            m.progress() < restoring,
            "the spring advanced back toward shown"
        );
    }

    #[test]
    fn scroll_event_drives_the_minimize_target() {
        // The in-widget hookup: an `InputEvent::Scroll` routed to the bar flips
        // the minimize target (the existing input scroll signal — see the
        // module docs).
        let view: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));

        fn scroll(w: &mut CupertinoTabBarWidget, dy: f64) {
            let mut unit = ();
            let sa: &mut dyn Any = &mut unit;
            let mut ctx = EventCtx::new(sa, Point::ZERO, band(300.0));
            w.event(
                &mut ctx,
                &InputEvent::Scroll {
                    position: Point::new(150.0, 20.0),
                    delta: ScrollDelta::Pixels(0.0, dy),
                },
            );
        }
        scroll(&mut w, 40.0);
        assert!(w.minimize.minimized, "downward scroll minimizes");
        scroll(&mut w, -40.0);
        assert!(!w.minimize.minimized, "upward scroll restores");
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    #[allow(clippy::ptr_arg)]
    fn dispatch(w: &mut CupertinoTabBarWidget, state: &mut Vec<usize>, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(300.0, HEIGHT));
        w.event(&mut ctx, event);
    }

    #[test]
    fn tap_inside_an_item_reports_its_index_without_self_mutating() {
        let view: CupertinoTabBarView<Vec<usize>> = cupertino_tab_bar(
            vec![tab_item("Home"), tab_item("Search"), tab_item("Profile")],
            0,
            |s: &mut Vec<usize>, i| s.push(i),
        );
        let mut counter = 0u64;
        let mut w = View::<Vec<usize>>::build(&view, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));

        let mut log: Vec<usize> = Vec::new();
        // Item 1 ("Search") occupies x in [100, 200).
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 150.0, 20.0));
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 150.0, 20.0));
        assert_eq!(log, vec![1]);
    }

    #[test]
    fn up_outside_does_not_fire() {
        let view: CupertinoTabBarView<Vec<usize>> =
            cupertino_tab_bar(three_items_typed(), 0, |s: &mut Vec<usize>, i| s.push(i));
        let mut counter = 0u64;
        let mut w = View::<Vec<usize>>::build(&view, &mut ctx(&mut counter));
        layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut log: Vec<usize> = Vec::new();
        dispatch(&mut w, &mut log, &ev(PointerPhase::Down, 150.0, 20.0));
        // Release far away (past the item, outside the whole bar height).
        dispatch(&mut w, &mut log, &ev(PointerPhase::Up, 150.0, 500.0));
        assert!(log.is_empty());
    }

    fn three_items_typed() -> Vec<TabItem<Vec<usize>>> {
        vec![tab_item("Home"), tab_item("Search"), tab_item("Profile")]
    }

    #[test]
    fn rebuild_adopts_new_selected_index() {
        let mut counter = 0u64;
        let prev: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 0, |_s: &mut (), _i| {});
        let mut w = View::<()>::build(&prev, &mut ctx(&mut counter));
        let widget0 = w.items[0]
            .widget_mut()
            .downcast_mut::<TabItemWidget>()
            .unwrap();
        assert!(widget0.selected);

        let next: CupertinoTabBarView<()> =
            cupertino_tab_bar(three_items(), 2, |_s: &mut (), _i| {});
        View::<()>::rebuild(&next, &prev, &mut w, &mut ctx(&mut counter));
        let widget0 = w.items[0]
            .widget_mut()
            .downcast_mut::<TabItemWidget>()
            .unwrap();
        assert!(!widget0.selected, "selection moved off item 0");
        let widget2 = w.items[2]
            .widget_mut()
            .downcast_mut::<TabItemWidget>()
            .unwrap();
        assert!(widget2.selected, "item 2 is now selected");
    }

    #[test]
    fn icon_slot_is_laid_out_above_the_label() {
        let view: CupertinoTabBarView<()> = cupertino_tab_bar(
            vec![tab_item("Home").icon(leaf_any(24.0, 24.0))],
            0,
            |_s: &mut (), _i| {},
        );
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::loose(Size::new(100.0, 200.0)));
        let widget0 = w.items[0]
            .widget_mut()
            .downcast_mut::<TabItemWidget>()
            .unwrap();
        let icon = widget0.icon.as_ref().expect("icon pod present");
        assert!(icon.origin().y < widget0.label.origin().y);
    }

    #[test]
    fn semantics_yields_a_tablist_of_tabs_with_selection() {
        fn logic(_state: &mut ()) -> CupertinoTabBarView<()> {
            cupertino_tab_bar(three_items(), 1, |_s: &mut (), _i| {})
        }
        let mut root: frust_core::RenderRoot<(), CupertinoTabBarView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(300.0, HEIGHT), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let tablist = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TabList)
            .expect("a TabList container node is contributed");
        assert_eq!(tablist.1.children().len(), 3);

        let tabs: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Tab)
            .collect();
        assert_eq!(tabs.len(), 3);
        let selected = tabs
            .iter()
            .find(|(_, n)| n.label() == Some("Search"))
            .expect("the Search tab is present");
        assert_eq!(selected.1.is_selected(), Some(true));
    }
}
