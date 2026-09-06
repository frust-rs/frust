// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments)
// Upstream: https://github.com/paadevelopments/material_3_expressive
//   lib/components/cards/ (`m3e_cards.dart`, `enums/m3e_card_variant.dart`,
//   `styles/m3e_card_theme.dart`)
// This rework adds the reference's hover/press interaction treatment
// (`M3ETappable`, ported once and shared crate-wide as `crate::interaction`)
// on top of the v1 static-elevation port; `onLongPress` and the
// customization escape hatches (`border`/`color`/`elevation`/`width`/
// `mouseCursor`/`semanticLabel`/`onStateChanged` overrides) stay unported —
// see the module docs' Not ported section.

//! The M3 `Card` container: elevated / filled / outlined variants, wrapping a
//! single [`frust::authoring::AnyView`] child (the same single-child `ChildPod`
//! wrapper shape as [`frust::Padding`]).
//!
//! # Variants
//!
//! All three variants share the medium shape (12dp, `shape.medium`) and
//! `CARD_PADDING` (16dp) content inset — the standard M3 card spec
//! (m3.material.io/components/cards/specs):
//!
//! * **Elevated**: container `surfaceContainerLow`, M3 elevation level 1
//!   (1dp) at rest, level 2 (3dp) while hovered — see Hover elevation lift
//!   below — painted via [`frust::authoring::PaintScene::draw_shadow`].
//! * **Filled**: container `surfaceContainerHighest`, elevation 0 (no
//!   shadow, at rest or hovered).
//! * **Outlined**: container `surface`, a 1dp stroke in `outlineVariant`
//!   (the enabled-state stroke color — `outline` is reserved for a disabled
//!   card, applied only while an interactive card is disabled, see Disabled
//!   state below), no shadow.
//!
//! # Outlined stroke primitive choice
//!
//! No stroked-rounded-rect primitive exists on [`frust::authoring::PaintScene`]
//! (only `stroke_line` and the `stroke_path`/`fill_path` pair). Rather than
//! approximate the rounded stroke with four `stroke_line` calls (visibly
//! square corners), this module builds a real rounded-rect outline via
//! `kurbo::RoundedRect` (a [`kurbo::Shape`], so `.to_path(tolerance)` yields a
//! `BezPath`) and paints it with [`frust::authoring::PaintScene::stroke_path`] —
//! establishing the precedent for any future widget needing a stroked rounded
//! shape.
//!
//! # Interactivity
//!
//! [`CardView::on_press`] makes the whole card surface one interactive
//! target — mirroring [`frust::Button`] (fire-on-up-inside; the card itself
//! owns capture and paints the shared M3E state-layer overlay via
//! [`crate::interaction::InteractionState`], the crate's unified
//! hover/focus/pressed/dragged substrate, tinted `on_surface`), rather than
//! forwarding events into the child. Without `on_press` the card is a
//! transparent, non-interactive wrapper that routes pointer events straight
//! to its child (mirroring [`frust::Padding`]) — [`CardView::enabled`] and
//! [`CardView::haptic`] are then no-ops, since there is no interactive
//! surface for either to act on (mirroring the reference's own
//! `M3ETappable`-only scope for both).
//!
//! # Hover elevation lift
//!
//! An interactive elevated card raises from M3 elevation level 1 (1dp) to
//! level 2 (3dp) while hovered (`M3ECardTheme.elevation(variant, {hovered})`)
//! — a *non-interactive* elevated card (no [`CardView::on_press`]) never
//! hovers at all and stays pinned at level 1, matching the reference's own
//! `_buildCard`, which never wraps a non-interactive card in `M3ETappable`
//! (so its `M3EInteractionState` — and therefore `hovered` — is permanently
//! the default `const M3EInteractionState()`, i.e. every flag `false`).
//!
//! Unlike this crate's [`super::fab`] and [`super::button`], whose own
//! hover-elevation steps are instant, per-frame binary switches (a
//! documented simplification of the reference's `AnimatedContainer`-driven
//! shadow tween, verified against both modules' own doc comments), this
//! module plays the reference's tween for real: [`CardWidget::elevation_anim`]
//! springs the *shadow geometry* (blur/y-offset — see
//! [`resolve_elevation_endpoints`]) between the level-1 and level-2 endpoints
//! whenever the live hover read flips, mirroring `switch`'s `anim_target`
//! idiom (a fresh [`frust::AnimationController::fling`] starts lazily in
//! `paint` whenever the live value disagrees with the target the last fling
//! aimed at). The driving spring is
//! [`crate::interaction::PressSpringId::DefaultEffects`] — this module's
//! first real (non-test) consumer of that seam, per its own doc: "this task
//! ships the shared state and resolved-values plumbing ... not a runtime."
//! A shadow-geometry change is a magnitude/effects transition, not a
//! spatial one (`crate::tokens::motion_scheme()`'s own spatial-vs-effects
//! split), and `DefaultEffects` is critically damped (`damping_ratio: 1.0`),
//! so the lift never overshoots into an unnaturally large or negative
//! shadow. [`ELEVATION_RETARGET_VELOCITY`] mirrors `switch::RELEASE_VELOCITY`
//! exactly: a near-zero signed nudge that only picks the fling's direction
//! (`fling` targets `1.0` for a non-negative velocity, `0.0` otherwise) and
//! leaves the spring's own stiffness/damping to shape the whole motion — a
//! hover flip is a discrete state change, not a directional user gesture
//! like [`super::fab`]'s press/release kick.
//!
//! Shadow *color* never changes between the two endpoints (both M3 elevation
//! levels share the same `0.3` `color_alpha`, per
//! `crate::tokens::metrics::elevation_level`) — only the geometry is
//! interpolated; see [`resolve_shadow_color`].
//!
//! # Disabled state
//!
//! [`CardView::enabled`] (default `true`) is this crate's own extension —
//! the reference `M3ECard` widget itself carries no `enabled` field (only
//! the `M3ETappable` primitive it wraps does, gating `_isInteractive`
//! alongside `onPressed`/`onLongPress`). Rather than literally mirroring
//! that formula (which would make a disabled interactive card
//! indistinguishable from a plain non-interactive one — no dimmed styling,
//! no accesskit disabled flag), this module follows this crate's own
//! established `enabled` convention instead (`button`/`radio`/`switch`/
//! `icon_button`/`toggle_button`/`text_field`): an interactive card that is
//! disabled **keeps** its `Role::Button` semantics (reporting
//! `Node::set_disabled()` instead of `Action::Click` — see
//! [`CardWidget::semantics`]) and its container/outline visually dim to
//! `on_surface` at [`crate::interaction::DISABLED_CONTAINER_OPACITY`] (12%,
//! mirroring `button::core::resolve_colors`'s identical disabled treatment)
//! — but it claims no hover, springs no elevation lift, paints no state
//! layer, and fires no press/haptic. **Hover is enabled-gated** in both the
//! event pass and paint's self-correction (`self.interactive &&
//! self.enabled && ...`) — the radio-hover-regression fix (never react to
//! hover while disabled) applied here from the start rather than
//! retrofitted. A disabled interactive card's own
//! `Widget::event` early-returns `Ignored` for every pointer phase (mirrors
//! `button::core::ButtonWidget::event`'s identical disabled early-return) —
//! it does **not** forward to its child; a disabled card block is fully
//! inert, not a pass-through.
//!
//! Elevation itself carries no disabled branch (`M3ECardTheme.elevation`
//! takes no `enabled` parameter) — a disabled elevated card still rests at
//! level 1, simply never lifting to level 2 since it can never hover.
//!
//! # Haptics
//!
//! [`CardView::haptic`] ports `M3ECard.haptic` (default
//! [`crate::interaction::HapticSignal::None`], passed straight through to
//! `M3ETappable`): fired via [`crate::interaction::MaterialHaptics::fire`]
//! immediately before `on_press`, mirroring `button::core`'s identical
//! ordering. `None` is a documented no-op, elided rather than routed through
//! the process-global hook.
//!
//! # Not ported
//!
//! `onLongPress` — no gesture primitive for it exists anywhere in this
//! crate yet (`button`'s own module docs record the same v1 scope line: "not
//! ported in v1: no tooltip host in the catalog, and neither long-press nor
//! a hover callback has a reference-visual attached to it"). The reference's
//! customization escape hatches (`clipBehavior`, a `borderRadius`/`color`/
//! `elevation`/`border`/`width` override, `surfaceKey`, `mouseCursor`,
//! `semanticLabel`, `animationDuration`/`animationCurve` overrides,
//! `onStateChanged`) are all out of this task's scope (hover-lift/press/
//! disabled treatment) and stay unported.
//!
//! # Semantics
//!
//! An interactive card contributes a [`Role::Button`] container node (its
//! child's own semantics become the button's accesskit children — a card has
//! no single-line text label of its own to flatten into the node, unlike
//! `Button`), reporting `Node::set_disabled()` while disabled instead of
//! `Action::Click` (see Disabled state above). A non-interactive card
//! contributes a [`Role::GenericContainer`] ("group") node instead of
//! transparently forwarding like `Padding` — a deliberate choice to keep a
//! card's content grouped as one semantic unit even when it isn't clickable.

use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::{Action, Role};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust::{AnimationController, Tween};
use kurbo::{Point, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use crate::interaction::{
    DISABLED_CONTAINER_OPACITY, HapticSignal, InteractionState, MaterialHaptics, PressSpringId,
};

use super::press::presses;

/// Content padding on all four edges, in logical px (M3 card spec).
const CARD_PADDING: f64 = 16.0;
/// Corner radius (unthemed fallback; a theme resolves this from
/// `shape.medium`, a 12dp token).
const RADIUS: f64 = 12.0;
/// Outline stroke width, in logical px (M3 card spec: hairline 1dp).
const STROKE_WIDTH: f64 = 1.0;
/// Flattening tolerance for the outlined variant's rounded-rect stroke path
/// (see the module docs' primitive-choice note) — a visually-lossless value
/// for on-screen corner radii.
const STROKE_TOLERANCE: f64 = 0.1;

/// Unthemed-fallback elevated-card container fill (a theme resolves this from
/// `colors.surface_container_low`).
const ELEVATED_CONTAINER: Color = Color::from_rgb8(0xF7, 0xF2, 0xFA);
/// Unthemed-fallback filled-card container fill (a theme resolves this from
/// `colors.surface_container_highest`).
const FILLED_CONTAINER: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);
/// Unthemed-fallback outlined-card container fill (a theme resolves this from
/// `colors.surface`).
const OUTLINED_CONTAINER: Color = Color::from_rgb8(0xFE, 0xF7, 0xFF);
/// Unthemed-fallback outline stroke color (a theme resolves this from
/// `colors.outline_variant` — the enabled-state stroke).
const OUTLINE_VARIANT: Color = Color::from_rgb8(0xCA, 0xC4, 0xD0);
/// Unthemed-fallback state-layer content color for an interactive card (a
/// theme resolves this from `colors.on_surface`).
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

/// Unthemed-fallback shadow y-offset at rest (M3 elevation level 1),
/// matching `crate::tokens::elevation().level1`'s `y_offset` exactly
/// (`dp / 2.0 + 1.0` at `dp = 1.0`).
const FALLBACK_SHADOW_Y_OFFSET: f64 = 1.5;
/// Unthemed-fallback shadow blur std-dev at rest (M3 elevation level 1),
/// matching `crate::tokens::elevation().level1`.
const FALLBACK_SHADOW_BLUR: f64 = 1.0;
/// Unthemed-fallback shadow y-offset while hovered (M3 elevation level 2),
/// matching `crate::tokens::elevation().level2`'s `y_offset` exactly
/// (`dp / 2.0 + 1.0` at `dp = 3.0`).
const FALLBACK_HOVER_SHADOW_Y_OFFSET: f64 = 2.5;
/// Unthemed-fallback shadow blur std-dev while hovered (M3 elevation level
/// 2), matching `crate::tokens::elevation().level2`.
const FALLBACK_HOVER_SHADOW_BLUR: f64 = 3.0;
/// Unthemed-fallback shadow color (opaque black at
/// `crate::tokens::elevation()`'s `0.3` alpha — identical at every level, so
/// this one constant covers both the rest and hovered endpoints).
const FALLBACK_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// A near-zero signed velocity that only picks
/// [`frust::AnimationController::fling`]'s direction, leaving the spring
/// itself to shape the motion — mirrors `switch::RELEASE_VELOCITY` exactly.
/// See the [module docs](self)' Hover elevation lift section for why a
/// hover flip uses this rather than [`super::fab`]'s directional kick.
const ELEVATION_RETARGET_VELOCITY: f64 = 1e-3;

/// The M3 card container variant. See the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardVariant {
    Elevated,
    Filled,
    Outlined,
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`super::state_layer`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved container fill for `variant`. `disabled` (an interactive
/// card with [`CardView::enabled`]`(false)`) overrides every variant to a
/// dimmed `on_surface` wash — see the [module docs](self)' Disabled state
/// section. Themed (enabled): `surface_container_low` (elevated) /
/// `surface_container_highest` (filled) / `surface` (outlined). Unthemed
/// (enabled): [`ELEVATED_CONTAINER`]/[`FILLED_CONTAINER`]/
/// [`OUTLINED_CONTAINER`] exactly.
fn resolve_container(theme: Option<&Theme>, variant: CardVariant, disabled: bool) -> Color {
    if disabled {
        return with_alpha(resolve_content_color(theme), DISABLED_CONTAINER_OPACITY);
    }
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            match variant {
                CardVariant::Elevated => scheme.surface_container_low,
                CardVariant::Filled => scheme.surface_container_highest,
                CardVariant::Outlined => scheme.surface,
            }
        }
        None => match variant {
            CardVariant::Elevated => ELEVATED_CONTAINER,
            CardVariant::Filled => FILLED_CONTAINER,
            CardVariant::Outlined => OUTLINED_CONTAINER,
        },
    }
}

/// The resolved corner radius. Themed: `shape.medium`. Unthemed: [`RADIUS`]
/// exactly.
fn resolve_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(theme) => theme.shape.medium,
        None => RADIUS,
    }
}

/// The resolved outline stroke color (outlined variant only). `disabled`
/// dims to the same `on_surface` wash [`resolve_container`] uses — see the
/// [module docs](self)' Disabled state section. Themed (enabled):
/// `colors.outline_variant`. Unthemed (enabled): [`OUTLINE_VARIANT`]
/// exactly.
fn resolve_outline(theme: Option<&Theme>, disabled: bool) -> Color {
    if disabled {
        return with_alpha(resolve_content_color(theme), DISABLED_CONTAINER_OPACITY);
    }
    match theme {
        Some(theme) => theme.scheme().outline_variant,
        None => OUTLINE_VARIANT,
    }
}

/// The resolved state-layer content color for an interactive card. Themed:
/// `colors.on_surface`. Unthemed: [`ON_SURFACE`] exactly.
fn resolve_content_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface,
        None => ON_SURFACE,
    }
}

/// The `(blur_std_dev, y_offset)` shadow-geometry endpoints the elevated
/// variant's hover-lift spring lerps between: resting (M3 elevation level 1)
/// and fully hovered (level 2). See the [module docs](self)' Hover elevation
/// lift section for why only geometry (not color) is interpolated. Themed:
/// `theme.elevation.{level1,level2}`'s own `ShadowSpec`. Unthemed: the
/// `FALLBACK_*`/`FALLBACK_HOVER_*` constants exactly (dp 1.0/3.0, matching
/// `crate::tokens::elevation()`'s table).
fn resolve_elevation_endpoints(theme: Option<&Theme>) -> ((f64, f64), (f64, f64)) {
    match theme {
        Some(theme) => {
            let rest = theme.elevation.level1.shadow(theme.brightness);
            let hover = theme.elevation.level2.shadow(theme.brightness);
            (
                (rest.blur_std_dev, rest.y_offset),
                (hover.blur_std_dev, hover.y_offset),
            )
        }
        None => (
            (FALLBACK_SHADOW_BLUR, FALLBACK_SHADOW_Y_OFFSET),
            (FALLBACK_HOVER_SHADOW_BLUR, FALLBACK_HOVER_SHADOW_Y_OFFSET),
        ),
    }
}

/// The elevated variant's shadow color — identical at rest and hovered (see
/// [`resolve_elevation_endpoints`]'s doc). Themed: `colors.shadow` at
/// `theme.elevation.level1`'s `color_alpha`. Unthemed: [`FALLBACK_SHADOW_COLOR`]
/// exactly.
fn resolve_shadow_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => {
            let alpha = theme.elevation.level1.shadow(theme.brightness).color_alpha;
            with_alpha(theme.scheme().shadow, alpha)
        }
        None => FALLBACK_SHADOW_COLOR,
    }
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// A view-held, typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// A declarative M3 card wrapping a single child. See the [module docs](self).
pub struct CardView<State: 'static> {
    variant: CardVariant,
    child: AnyView<State>,
    on_press: Option<OnPress<State>>,
    enabled: bool,
    haptic: HapticSignal,
}

/// Wrap `child` in a card of the given `variant`. Chain [`CardView::on_press`]
/// to make the whole surface interactive.
pub fn card<State: 'static, V: View<State>>(variant: CardVariant, child: V) -> CardView<State> {
    CardView {
        variant,
        child: any(child),
        on_press: None,
        enabled: true,
        haptic: HapticSignal::None,
    }
}

/// Wrap `child` in an elevated card (`surfaceContainerLow`, M3 elevation
/// level 1 at rest / level 2 hovered).
pub fn elevated_card<State: 'static, V: View<State>>(child: V) -> CardView<State> {
    card(CardVariant::Elevated, child)
}

/// Wrap `child` in a filled card (`surfaceContainerHighest`, no elevation).
pub fn filled_card<State: 'static, V: View<State>>(child: V) -> CardView<State> {
    card(CardVariant::Filled, child)
}

/// Wrap `child` in an outlined card (`surface` fill, `outlineVariant` stroke).
pub fn outlined_card<State: 'static, V: View<State>>(child: V) -> CardView<State> {
    card(CardVariant::Outlined, child)
}

impl<State: 'static> CardView<State> {
    /// Make the whole card surface interactive, firing `on_press` on release
    /// inside its bounds (see the module docs' Interactivity section).
    pub fn on_press<F: Fn(&mut State) + 'static>(mut self, on_press: F) -> Self {
        self.on_press = Some(Rc::new(on_press));
        self
    }

    /// Gate the interactive treatment (hover elevation lift, state-layer
    /// hover/press tint, and firing [`CardView::on_press`]/
    /// [`CardView::haptic`] at all) without dropping back to a
    /// non-interactive, transparently-forwarding card. Defaults to `true`.
    /// Only meaningful once [`CardView::on_press`] is chained — a no-op on a
    /// plain wrapper card. See the [module docs](self)' Disabled state
    /// section for the full behavior and how it differs from the reference.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the haptic fired immediately before [`CardView::on_press`] on a
    /// successful release (`M3ECard.haptic`). Defaults to
    /// [`HapticSignal::None`] (no feedback). A no-op on a non-interactive or
    /// disabled card.
    pub fn haptic(mut self, haptic: HapticSignal) -> Self {
        self.haptic = haptic;
        self
    }
}

/// The retained widget for a [`CardView`].
pub struct CardWidget {
    variant: CardVariant,
    child: ChildPod,
    interactive: bool,
    enabled: bool,
    haptic: HapticSignal,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on
    /// `Up`/`Cancel`/loss of interactivity or enablement.
    captured: bool,
    state: InteractionState,
    /// Drives the elevated variant's hover-lift shadow-geometry lerp —
    /// `0.0` at rest (M3 elevation level 1) .. `1.0` fully hovered (level
    /// 2). See the [module docs](self)' Hover elevation lift section.
    /// Unused (stays at its build-time rest value) for the filled/outlined
    /// variants, which never elevate.
    elevation_anim: AnimationController,
    /// The `hovered` value [`CardWidget::elevation_anim`]'s current fling is
    /// driving toward — compared against the live hover read each paint to
    /// decide whether a fresh fling needs to start (mirrors `switch`'s
    /// `anim_target` idiom).
    elevation_target: bool,
    on_press: Option<frust::authoring::ErasedCallback>,
}

impl<State: 'static> View<State> for CardView<State> {
    type Element = CardWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CardWidget {
        CardWidget {
            variant: self.variant,
            child: frust::authoring::build_child(&self.child, ctx),
            interactive: self.on_press.is_some(),
            enabled: self.enabled,
            haptic: self.haptic,
            captured: false,
            state: InteractionState::new(),
            elevation_anim: AnimationController::new(Duration::ZERO),
            elevation_target: false,
            on_press: self.on_press.as_ref().map(frust::authoring::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CardWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        let now_interactive = self.on_press.is_some();
        if element.interactive != now_interactive {
            element.interactive = now_interactive;
            // Losing the interactive surface mid-gesture must not leave a
            // dangling capture, press, or hover behind.
            if !now_interactive {
                element.captured = false;
                element.state.set_pressed(false);
                element.state.set_hovered(false);
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled && element.interactive {
                // A card disabled mid-gesture keeps neither the press nor
                // the hover it was holding — mirrors `switch`/`radio`'s
                // identical disabled-mid-interaction clear.
                element.captured = false;
                element.state.set_pressed(false);
                element.state.set_hovered(false);
            }
            if element.interactive {
                flags |= ChangeFlags::PAINT;
            }
        }
        element.haptic = self.haptic;
        element.on_press = self.on_press.as_ref().map(frust::authoring::erase_callback);
        flags |= frust::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut CardWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for CardWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inset = CARD_PADDING * 2.0;
        let inner_max = Size::new(
            (bc.max().width - inset).max(0.0),
            (bc.max().height - inset).max(0.0),
        );
        let inner_min = Size::new(
            (bc.min().width - inset).max(0.0),
            (bc.min().height - inset).max(0.0),
        );
        let child_size = self
            .child
            .layout_child(ctx, &BoxConstraints::new(inner_min, inner_max));
        self.child
            .set_origin(Point::new(CARD_PADDING, CARD_PADDING));
        bc.constrain(Size::new(
            child_size.width + inset,
            child_size.height + inset,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Every theme read happens here, before `ctx` is taken mutably below
        // (`request_frame`) — mirrors `button::ButtonWidget::paint`'s own
        // documented ordering.
        let theme = Theme::from_paint_ctx(ctx);
        let disabled = self.interactive && !self.enabled;
        let container = resolve_container(theme, self.variant, disabled);
        let radius = resolve_radius(theme);
        // Authoritative hover read, self-correcting the latched flag —
        // inert while non-interactive or disabled (`docs/CODE_STANDARDS.md`'s
        // Interaction Semantics; the radio-hover-regression fix: never react
        // to hover while disabled).
        let hovered = self.interactive && self.enabled && ctx.is_hovered();
        let (elevation_rest, elevation_hover) = resolve_elevation_endpoints(theme);
        let shadow_color = resolve_shadow_color(theme);
        let outline =
            (self.variant == CardVariant::Outlined).then(|| resolve_outline(theme, disabled));
        // `theme`'s last use: resolved here, before `ctx` is taken mutably
        // below (`request_frame`) — mirrors `button::ButtonWidget::paint`'s
        // own documented ordering.
        let content_color = resolve_content_color(theme);

        self.state.set_hovered(hovered);

        let o = ctx.origin();
        let size = ctx.size();

        if self.variant == CardVariant::Elevated {
            if hovered != self.elevation_target {
                let velocity = if hovered {
                    ELEVATION_RETARGET_VELOCITY
                } else {
                    -ELEVATION_RETARGET_VELOCITY
                };
                self.elevation_anim
                    .fling(velocity, PressSpringId::DefaultEffects.resolve());
                self.elevation_target = hovered;
            }
            if self.elevation_anim.advance(ctx.frame_time()) {
                ctx.request_frame();
            }
            let t = self.elevation_anim.value_clamped();
            let blur = Tween::new(elevation_rest.0, elevation_hover.0).lerp(t);
            let y_offset = Tween::new(elevation_rest.1, elevation_hover.1).lerp(t);
            scene.draw_shadow(
                Point::new(o.x, o.y + y_offset),
                size,
                radius,
                blur,
                shadow_color,
            );
        }

        scene.fill_rounded_rect(o, size, radius, container);

        if let Some(outline) = outline {
            // Inset by half the stroke width so the 1dp line paints fully
            // inside the card's own bounds (a stroke is centered on its path).
            let half = STROKE_WIDTH / 2.0;
            let rr = RoundedRect::new(
                half,
                half,
                size.width - half,
                size.height - half,
                (radius - half).max(0.0),
            );
            let path = rr.to_path(STROKE_TOLERANCE);
            scene.stroke_path(o, &path, STROKE_WIDTH, &Brush::Solid(outline));
        }

        if self.interactive {
            let opacity = self.state.resolve_opacity();
            if opacity > 0.0 {
                scene.fill_rounded_rect(o, size, radius, with_alpha(content_color, opacity));
            }
        }

        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.interactive {
            return frust::authoring::route_event_single(&mut self.child, ctx, event);
        }
        // Non-pointer events (Key, Ime, focus-routed) must be forwarded to
        // the child, even when interactive. Only pointer events drive the
        // interactive card's own capture/press/hover behavior.
        let InputEvent::Pointer(p) = event else {
            return frust::authoring::route_event_single(&mut self.child, ctx, event);
        };
        if !self.enabled {
            // A disabled interactive card arms nothing and claims no hover —
            // mirrors `button::core::ButtonWidget::event`'s disabled
            // early-return, and the radio-hover-regression fix (never react
            // to hover while disabled). It does not forward to its child
            // either: a disabled card block is fully inert, not a
            // pass-through (see the module docs' Disabled state section).
            return EventResult::Ignored;
        }
        let on_press = self
            .on_press
            .as_mut()
            .expect("on_press is set whenever interactive is true");
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                self.state.set_pressed(true);
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    // No capture: this is the hover pass — claim, latch, and
                    // let paint self-correct (`docs/CODE_STANDARDS.md`'s
                    // three-part hover contract; see the [module docs](self)'
                    // Hover elevation lift section).
                    let over = inside(p.position, ctx.size());
                    if over {
                        ctx.claim_hover();
                    }
                    if self.state.set_hovered(over) {
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
                let inside_now = inside(p.position, ctx.size());
                if self.state.set_pressed(inside_now) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    if self.haptic != HapticSignal::None {
                        MaterialHaptics::fire(self.haptic);
                    }
                    (on_press)(ctx);
                }
                self.state.set_pressed(false);
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.state.set_pressed(false);
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.interactive {
            ctx.push_container(
                Role::Button,
                |node| {
                    if self.enabled {
                        node.add_action(Action::Click);
                    } else {
                        node.set_disabled();
                    }
                },
                |ctx| {
                    self.child.semantics_child(ctx);
                },
            );
        } else {
            ctx.push_container(
                Role::GenericContainer,
                |_| {},
                |ctx| {
                    self.child.semantics_child(ctx);
                },
            );
        }
    }

    frust::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Primary,
        })
    }

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    /// Records each rounded rect's `(origin, size, radius, color)`, each
    /// shadow's `(origin, size, radius, std_dev, color)`, and each stroked
    /// path's `(origin, width, color)`.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        strokes: Vec<(Point, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
        fn stroke_path(
            &mut self,
            origin: Point,
            _path: &kurbo::BezPath,
            width: f64,
            brush: &Brush,
        ) {
            if let Brush::Solid(color) = brush {
                self.strokes.push((origin, width, *color));
            }
        }
    }

    fn build<S: 'static>(view: &CardView<S>) -> CardWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn paint(w: &mut CardWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn layout_insets_child_by_card_padding() {
        let view: CardView<()> = elevated_card(leaf_any(40.0, 20.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(40.0 + 32.0, 20.0 + 32.0));
        assert_eq!(w.child.origin(), Point::new(CARD_PADDING, CARD_PADDING));
    }

    #[test]
    fn elevated_unthemed_paints_container_and_shadow() {
        let view: CardView<()> = elevated_card(leaf_any(40.0, 20.0));
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(100.0, 60.0), None);
        assert_eq!(rec.rrects[0].3, ELEVATED_CONTAINER);
        assert_eq!(rec.rrects[0].2, RADIUS);
        assert_eq!(rec.shadows.len(), 1);
        assert_eq!(rec.shadows[0].3, FALLBACK_SHADOW_BLUR);
        assert!(rec.strokes.is_empty(), "elevated card has no stroke");
    }

    #[test]
    fn filled_unthemed_paints_container_with_no_shadow() {
        let view: CardView<()> = filled_card(leaf_any(40.0, 20.0));
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(100.0, 60.0), None);
        assert_eq!(rec.rrects[0].3, FILLED_CONTAINER);
        assert!(rec.shadows.is_empty(), "filled card paints no shadow");
        assert!(rec.strokes.is_empty(), "filled card has no stroke");
    }

    #[test]
    fn outlined_unthemed_paints_container_and_stroke_with_no_shadow() {
        let view: CardView<()> = outlined_card(leaf_any(40.0, 20.0));
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(100.0, 60.0), None);
        assert_eq!(rec.rrects[0].3, OUTLINED_CONTAINER);
        assert!(rec.shadows.is_empty(), "outlined card paints no shadow");
        assert_eq!(rec.strokes.len(), 1);
        assert_eq!(rec.strokes[0].1, STROKE_WIDTH);
        assert_eq!(rec.strokes[0].2, OUTLINE_VARIANT);
    }

    #[test]
    fn themed_elevated_paint_shadow_is_unchanged_on_m3_light() {
        // `ElevationLevel::shadow` carries a per-brightness split
        // (`shadow_light`/`shadow_dark`), and the M3 v1 mapping duplicates
        // the same value into both slots. This pins the elevated card's
        // rendered (resting, unhovered) shadow on `Brightness::Light` to
        // `theme.elevation.level1`'s shadow spec, byte-identical to the
        // pre-migration single-field output.
        let theme = crate::baseline();
        assert_eq!(theme.brightness, frust::Brightness::Light);
        let level1 = theme.elevation.level1;

        let view: CardView<()> = elevated_card(leaf_any(40.0, 20.0));
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(100.0, 60.0), Some(&theme));

        assert_eq!(rec.shadows.len(), 1);
        assert_eq!(rec.shadows[0].3, level1.shadow_light.blur_std_dev);
        assert_eq!(
            rec.shadows[0].4.components[3],
            level1.shadow_light.color_alpha
        );
        // The two brightness slots are identical for M3 (behavior-preserving
        // duplication), so the accessor call resolves to the same values.
        assert_eq!(
            rec.shadows[0].3,
            level1.shadow(theme.brightness).blur_std_dev
        );
    }

    #[test]
    fn themed_paint_resolves_r11_tokens() {
        let theme = crate::baseline();
        let scheme = theme.scheme();

        let elevated: CardView<()> = elevated_card(leaf_any(40.0, 20.0));
        let mut w = build(&elevated);
        let rec = paint(&mut w, Size::new(100.0, 60.0), Some(&theme));
        assert_eq!(rec.rrects[0].3, scheme.surface_container_low);
        assert_eq!(rec.rrects[0].2, theme.shape.medium);

        let filled: CardView<()> = filled_card(leaf_any(40.0, 20.0));
        let mut w = build(&filled);
        let rec = paint(&mut w, Size::new(100.0, 60.0), Some(&theme));
        assert_eq!(rec.rrects[0].3, scheme.surface_container_highest);

        let outlined: CardView<()> = outlined_card(leaf_any(40.0, 20.0));
        let mut w = build(&outlined);
        let rec = paint(&mut w, Size::new(100.0, 60.0), Some(&theme));
        assert_eq!(rec.rrects[0].3, scheme.surface);
        assert_eq!(rec.strokes[0].2, scheme.outline_variant);
    }

    // --- Disabled interactive card: dimmed styling, no shadow lift ---

    #[test]
    fn disabled_interactive_card_dims_container_uniformly_across_variants() {
        let theme = crate::baseline();
        let expected = with_alpha(theme.scheme().on_surface, DISABLED_CONTAINER_OPACITY);
        for variant in [
            CardVariant::Elevated,
            CardVariant::Filled,
            CardVariant::Outlined,
        ] {
            let view: CardView<()> = card(variant, leaf_any(40.0, 20.0))
                .on_press(|_: &mut ()| {})
                .enabled(false);
            let mut w = build(&view);
            let rec = paint(&mut w, Size::new(100.0, 60.0), Some(&theme));
            assert_eq!(
                rec.rrects[0].3, expected,
                "{variant:?} container dims while disabled"
            );
        }
    }

    #[test]
    fn disabled_interactive_outlined_card_dims_its_outline_too() {
        let theme = crate::baseline();
        let expected = with_alpha(theme.scheme().on_surface, DISABLED_CONTAINER_OPACITY);
        let view: CardView<()> = outlined_card(leaf_any(40.0, 20.0))
            .on_press(|_: &mut ()| {})
            .enabled(false);
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(100.0, 60.0), Some(&theme));
        assert_eq!(rec.strokes[0].2, expected);
    }

    #[test]
    fn disabled_interactive_elevated_card_still_rests_at_level1_unlifted() {
        let view: CardView<()> = elevated_card(leaf_any(40.0, 20.0))
            .on_press(|_: &mut ()| {})
            .enabled(false);
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(100.0, 60.0), None);
        assert_eq!(rec.shadows.len(), 1);
        assert_eq!(
            rec.shadows[0].3, FALLBACK_SHADOW_BLUR,
            "never lifts: a disabled card never hovers"
        );
        assert!(
            rec.rrects.get(1).is_none(),
            "no state-layer overlay while disabled"
        );
    }

    // --- Non-interactive: transparent event routing to the child ---

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    fn dispatch<S: 'static>(w: &mut CardWidget, state: &mut S, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(200.0, 200.0));
        w.event(&mut ctx, event)
    }

    #[test]
    fn non_interactive_card_forwards_events_to_its_child() {
        let view: CardView<Counter> =
            elevated_card(frust::button::<Counter, _>("go", |s: &mut Counter| {
                s.presses += 1
            }));
        let mut w = build(&view);
        let mut tcx = frust::authoring::text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));

        let origin = w.child.origin();
        let mut state = Counter::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, origin.x + 2.0, origin.y + 2.0),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, origin.x + 2.0, origin.y + 2.0),
        );
        assert_eq!(state.presses, 1, "the nested button fires through the card");
    }

    // --- Interactive: whole-surface capture ---

    /// A minimal `State`-generic content stand-in (`test_support::leaf_any`
    /// only produces an `AnyView<()>`; the interactive tests below need a
    /// generic `State`, so a plain [`frust::text`] run stands in for
    /// the child content — irrelevant to firing behavior).
    fn content_stub<S: 'static>() -> AnyView<S> {
        any::<S, _>(frust::text("content"))
    }

    #[test]
    fn interactive_card_fires_on_up_inside_without_forwarding_to_child() {
        let view: CardView<Counter> =
            filled_card(content_stub::<Counter>()).on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.presses, 1);
    }

    #[test]
    fn interactive_card_up_outside_does_not_fire() {
        let view: CardView<Counter> =
            filled_card(content_stub::<Counter>()).on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 500.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 500.0));
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn interactive_card_cancel_clears_armed_state() {
        let view: CardView<Counter> =
            filled_card(content_stub::<Counter>()).on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 5.0, 5.0));
        assert!(!w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn disabled_interactive_card_ignores_every_pointer_phase() {
        let view: CardView<Counter> = filled_card(content_stub::<Counter>())
            .on_press(|s: &mut Counter| s.presses += 1)
            .enabled(false);
        let mut w = build(&view);
        let mut state = Counter::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0)),
            EventResult::Ignored
        );
        assert!(!w.captured, "a disabled card never captures");
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.presses, 0, "and never fires");
    }

    // --- Interactive with focus-routed child events ---

    /// A minimal widget that consumes focus and handles Key events for testing.
    struct FocusConsumer;

    impl FocusConsumer {
        fn new() -> Self {
            FocusConsumer
        }
    }

    struct FocusConsumerWidget;

    impl View<Counter> for FocusConsumer {
        type Element = FocusConsumerWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> FocusConsumerWidget {
            FocusConsumerWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut FocusConsumerWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> frust::authoring::ChangeFlags {
            frust::authoring::ChangeFlags::NONE
        }
    }

    impl Widget for FocusConsumerWidget {
        fn layout(&mut self, _ctx: &mut frust::authoring::LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            // Handle any key event (focus-routed events reach here only if the
            // pod is focused, so a Key event here proves the routing worked).
            if let InputEvent::Key(_) = event {
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn interactive_card_forwards_key_events_to_focused_child() {
        let view: CardView<Counter> =
            filled_card(FocusConsumer::new()).on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();

        // Manually set the child as focused to simulate a prior focus state.
        // (In real usage, the child would be focused by a pointer-down event,
        // but here we're directly testing the event-routing path.)
        w.child.set_focused(true);

        // Send a Key event — it should be forwarded to the focused child.
        let key_event = InputEvent::Key(frust::authoring::KeyEvent {
            key: frust::authoring::Key::Named(frust::authoring::NamedKey::Backspace),
            modifiers: frust::authoring::Modifiers::default(),
            repeat: false,
        });
        let result = dispatch(&mut w, &mut state, &key_event);

        // Verify the key event was handled (forwarded to and handled by child).
        assert_eq!(
            result,
            EventResult::Handled,
            "key event must be forwarded to child"
        );

        // The card press should not have fired (the card only fires on pointer Up).
        assert_eq!(
            state.presses, 0,
            "card press callback does not fire on key event"
        );
    }

    #[test]
    fn semantics_interactive_card_is_a_button_container_forwarding_child() {
        // A leaf without its own `semantics()` override (e.g. `test_support`'s
        // `Leaf`) contributes no node — use a `Text` child, which does, so the
        // "forwards the child's semantics" assertion is meaningful.
        fn logic(_s: &mut ()) -> CardView<()> {
            filled_card(frust::text("body")).on_press(|_s: &mut ()| {})
        }
        let mut root: frust_core::RenderRoot<(), CardView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("interactive card contributes a Role::Button container node");
        assert!(
            !node.children().is_empty(),
            "the child is a semantics child"
        );
        assert!(!node.is_disabled());
    }

    #[test]
    fn semantics_disabled_interactive_card_reports_disabled_and_no_click_action() {
        fn logic(_s: &mut ()) -> CardView<()> {
            filled_card(frust::text("body"))
                .on_press(|_s: &mut ()| {})
                .enabled(false)
        }
        let mut root: frust_core::RenderRoot<(), CardView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("a disabled interactive card still contributes a Role::Button node");
        assert!(node.is_disabled());
    }

    #[test]
    fn semantics_non_interactive_card_is_a_generic_container() {
        fn logic(_s: &mut ()) -> CardView<()> {
            filled_card(frust::text("body"))
        }
        let mut root: frust_core::RenderRoot<(), CardView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::GenericContainer)
            .expect("non-interactive card contributes a Role::GenericContainer node");
        assert!(
            !node.children().is_empty(),
            "the child is a semantics child"
        );
    }

    // --- Hover elevation lift + press state layer (through a real RenderRoot) ---
    //
    // Hover and focus are *authoritative reads* off `PaintCtx`, seeded by the
    // pod chain, so a bare `PaintCtx` can never fake them — only a real tree
    // can (the pattern `list_item.rs`'s own hover harness uses, and
    // `button::core`'s own `Harness` mirrors). These tests dispatch through a
    // real `frust_core::RenderRoot`.

    const HARNESS_WINDOW: Size = Size::new(300.0, 200.0);

    struct Harness {
        root: frust_core::RenderRoot<u32, CardView<u32>>,
        state: u32,
        tcx: frust::authoring::text::TextContext,
        variant: CardVariant,
        enabled: bool,
    }

    impl Harness {
        fn new(variant: CardVariant, enabled: bool) -> Self {
            let mut h = Self {
                root: frust_core::RenderRoot::new(),
                state: 0,
                tcx: frust::authoring::text::TextContext::new(),
                variant,
                enabled,
            };
            h.sync();
            h
        }

        fn sync(&mut self) {
            let (variant, enabled) = (self.variant, self.enabled);
            let mut app = move |_: &mut u32| {
                card::<u32, _>(variant, content_stub::<u32>())
                    .on_press(|s: &mut u32| *s += 1)
                    .enabled(enabled)
            };
            self.root.rebuild(&mut app, &mut self.state);
            self.root
                .layout_with_text(HARNESS_WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn dispatch(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(frust::authoring::PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: frust::authoring::PointerButton::Primary,
                }),
            );
        }

        fn paint_at(&mut self, ft: FrameTime) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft);
            rec
        }

        /// The alpha of the state-layer overlay painted this frame, if any
        /// (the container fill is always the first rect).
        fn overlay_alpha(&mut self, ft: FrameTime) -> Option<f32> {
            let rec = self.paint_at(ft);
            rec.rrects
                .get(1)
                .map(|(_, _, _, color)| color.components[3])
        }
    }

    #[test]
    fn hover_lifts_the_elevated_card_shadow_from_level1_to_level2_and_back() {
        let mut h = Harness::new(CardVariant::Elevated, true);
        let rest = h.paint_at(FrameTime::ZERO);
        assert_eq!(rest.shadows[0].3, FALLBACK_SHADOW_BLUR, "rests at level1");
        assert_eq!(rest.shadows[0].0, Point::new(0.0, FALLBACK_SHADOW_Y_OFFSET));

        h.dispatch(PointerPhase::Move, 10.0, 10.0);
        // First paint after the fling starts only seeds the spring's clock
        // (`AnimationController::advance`'s own documented first-call
        // contract).
        h.paint_at(ft_secs(0.0));
        let settled = h.paint_at(ft_secs(1.0));
        assert_eq!(
            settled.shadows[0].3, FALLBACK_HOVER_SHADOW_BLUR,
            "settles at level2's blur once the hover-lift spring settles"
        );
        assert_eq!(
            settled.shadows[0].0,
            Point::new(0.0, FALLBACK_HOVER_SHADOW_Y_OFFSET)
        );

        h.dispatch(PointerPhase::Move, 290.0, 190.0);
        h.paint_at(ft_secs(1.0));
        let dropped = h.paint_at(ft_secs(2.0));
        assert_eq!(
            dropped.shadows[0].3, FALLBACK_SHADOW_BLUR,
            "drops back to level1 once unhovered"
        );
    }

    #[test]
    fn hover_never_lifts_a_disabled_interactive_card() {
        let mut h = Harness::new(CardVariant::Elevated, false);
        h.dispatch(PointerPhase::Move, 10.0, 10.0);
        let rec = h.paint_at(ft_secs(1.0));
        assert_eq!(
            rec.shadows[0].3, FALLBACK_SHADOW_BLUR,
            "a disabled card never reacts to hover (the radio-hover-regression fix)"
        );
        h.dispatch(PointerPhase::Down, 10.0, 10.0);
        h.dispatch(PointerPhase::Up, 10.0, 10.0);
        assert_eq!(h.state, 0, "and never fires either");
    }

    #[test]
    fn hovering_paints_the_hover_state_layer_and_pressing_outranks_it() {
        let mut h = Harness::new(CardVariant::Filled, true);
        assert_eq!(h.overlay_alpha(FrameTime::ZERO), None, "no overlay at rest");

        h.dispatch(PointerPhase::Move, 10.0, 10.0);
        assert_eq!(
            h.overlay_alpha(ft_secs(0.0)),
            Some(crate::interaction::HOVER_OPACITY),
            "the hovered card tints at the M3E hover opacity"
        );

        h.dispatch(PointerPhase::Down, 10.0, 10.0);
        assert_eq!(
            h.overlay_alpha(ft_secs(0.0)),
            Some(frust::authoring::PRESSED_OPACITY),
            "pressed outranks hover in the precedence order"
        );

        h.dispatch(PointerPhase::Up, 10.0, 10.0);
        assert_eq!(h.state, 1, "and the release fired the callback");
    }

    #[test]
    fn disabled_card_never_tints_and_never_fires() {
        let mut h = Harness::new(CardVariant::Filled, false);
        h.dispatch(PointerPhase::Move, 10.0, 10.0);
        assert_eq!(
            h.overlay_alpha(ft_secs(0.0)),
            None,
            "a disabled card claims no hover"
        );
        h.dispatch(PointerPhase::Down, 10.0, 10.0);
        h.dispatch(PointerPhase::Up, 10.0, 10.0);
        assert_eq!(h.state, 0);
    }
}
