// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// whose `lib/components/buttons/` tree is itself vendored from m3e_buttons
// (MIT, © 2026 Mudit Purohit) — `m3e_buttons.dart` + its `components/`,
// `enums/`, `models/`, `res/`, `styles/` parts.
// The Dart widget delegates its surface to Flutter's own FilledButton/
// OutlinedButton/…; this port paints that surface itself (see the module
// docs' Flutter-plumbing mapping table).

//! The Material 3 Expressive **button**: the catalog's flagship control.
//!
//! [`button`] builds the default filled button; [`outlined_button`],
//! [`tonal_button`], [`elevated_button`] and [`text_button`] are the reference's
//! other four named constructors ([`ButtonView::variant`] switches between them
//! after the fact), and [`button_with_icon`] is its `M3EButton.icon` factory —
//! an icon+label row laid out with the size's own icon size and gap. Every one
//! of them fires `on_press` on release inside its bounds and leaves its own
//! state alone: a button reports an action, it confirms no value (see
//! `docs/CODE_STANDARDS.md`'s Interaction Semantics).
//!
//! # Variants
//!
//! Container / label roles, from `m3e_button_theme.dart:111` (`container`) and
//! `:127` (`foreground`):
//!
//! | Variant | Container | Label + icon |
//! |---|---|---|
//! | [`ButtonVariant::Filled`] (default) | `primary` | `on_primary` |
//! | [`ButtonVariant::Tonal`] | `secondary_container` | `on_secondary_container` |
//! | [`ButtonVariant::Elevated`] | `surface_container_low` | `primary` |
//! | [`ButtonVariant::Outlined`] | transparent + `outline` hairline | `primary` |
//! | [`ButtonVariant::Text`] | transparent | `primary` |
//!
//! Disabled (`m3e_button_constants.dart:27`-`:33`): the label/icon drop to
//! `on_surface` at 38%, an opaque container to `on_surface` at 12%, an
//! outline to `on_surface` at 12%; a transparent container stays transparent.
//!
//! # Sizes
//!
//! Height / horizontal padding / icon / icon-gap, from
//! `m3e_button_theme.dart:76`'s `_measurementsTable`, and the label type role
//! from `m3e_base_button_state.dart:169`:
//!
//! | Size | Height | Padding | Icon | Gap | Label role |
//! |---|---|---|---|---|---|
//! | [`ButtonSize::Xs`] | 32 | 16 | 20 | 8 | `label_small` |
//! | [`ButtonSize::Sm`] (default) | 40 | 16 | 20 | 8 | `label_medium` |
//! | [`ButtonSize::Md`] | 56 | 24 | 24 | 8 | `label_large` |
//! | [`ButtonSize::Lg`] | 96 | 48 | 32 | 12 | `title_medium` |
//! | [`ButtonSize::Xl`] | 136 | 64 | 40 | 16 | `title_large` |
//!
//! A button is never narrower than 48dp (`M3EButtonTheme.minWidthFloor`,
//! `m3e_button_theme.dart:16`). The reference's `M3EButtonSize.custom`
//! per-instance measurement override is deliberately not ported here — this
//! enum is closed, and the decoration-shaped override surface it belongs to is
//! the same one the gradient seam below is waiting on.
//!
//! # Shape and the press morph
//!
//! [`ButtonShape::Round`] (the default) is a pill — `height / 2`;
//! [`ButtonShape::Square`] takes the per-size token radius. Pressing morphs
//! the container **squarer** and hovering morphs it partway there, both
//! spring-driven (`m3e_button_content.dart:40`'s `_resolveShapes`):
//!
//! | Size | Square | Hovered | Pressed |
//! |---|---|---|---|
//! | Xs / Sm | 12 | 10 | 8 |
//! | Md | 16 | 14 | 12 |
//! | Lg / Xl | 28 | 22 | 16 |
//!
//! The square and pressed columns coincide exactly with this crate's shape
//! scale, so they resolve from the live [`frust::Theme`] (`shape.medium`/
//! `large`/`extra_large` and `shape.small`/`medium`/`large`) with the literal
//! table as the unthemed fallback — the same themed-with-fallback treatment
//! [`mod@crate::fab`] gives its own radii. The hovered column sits *between*
//! two tokens (each entry is the midpoint of its size's square and pressed
//! radii), so it has no token to resolve from and stays literal.
//!
//! The morph itself — radius and content padding on one spring — is the
//! `motion` submodule's `RadiusPaddingMotion`, flung along
//! `EXPRESSIVE_SPATIAL_PRESS` (380 / 0.55); see that module for the spring
//! citation and for how a mid-flight retarget stays continuous. A plain
//! button only ever retargets the radius: its padding target is the per-size
//! table value and does not move on press (the reference passes a constant
//! `baseInternalPadding`, `m3e_button_content.dart:7`). The padding channel is
//! nonetheless real and layout-affecting, because the same primitive is what
//! the connected/toggle families morph their padding with.
//!
//! # Focus ring
//!
//! `m3e_focus_ring.dart` draws a 2dp `primary` ring 2dp outside the container,
//! with the container's own (animating) radius plus that 4dp outset. This port
//! draws exactly that, gated on [`frust::authoring::PaintCtx::has_focus`] —
//! the authoritative focus read. **Honest limitation:** frust has no keyboard
//! focus traversal for a button today (no autofocus, no `FocusNode`, no
//! Tab-ring), and this widget deliberately does not claim focus on a press
//! (a tap-shows-a-focus-ring button is the wrong behavior on touch, and
//! matches neither the reference nor Material's own focus-highlight mode). So
//! the ring is ported, gated and unit-testable, but nothing in a shipping app
//! reaches it yet — it becomes live the moment focus routing can reach a
//! button.
//!
//! # Seams the follow-up tasks fill
//!
//! Two hook points exist here as documented, no-op-by-default interfaces:
//!
//! - [`ButtonDecoration`] — a paint-time fill/overlay/outline triple plus a
//!   [`foreground_brush`](ButtonDecoration::foreground_brush) resolver,
//!   called with the morphing [`ButtonSurface`], where the reference's
//!   `m3eGradientSurfaceBuilder`/`m3eGradientForegroundBuilder` layers live.
//!   Returning [`DecorationOutcome::Painted`] suppresses the core's own solid
//!   layer. [`gradient::GradientButtonDecoration`] is the built-in
//!   implementation that fills every hook with ported gradient paint — see
//!   that module's docs for the full gradient-class inventory and the
//!   `PaintScene` gradient-support finding.
//! - [`OverflowObserver`] — handed the [`ContentMetrics`] every layout
//!   resolves, including whether the label's natural width exceeded the width
//!   actually available to it. [`overflow::OverflowStrategy`] (installed via
//!   [`ButtonView::overflow`]) is what now decides *what to do* about that —
//!   scroll or defer to a bottom sheet, calling back into an installed
//!   `OverflowObserver` for the latter — porting the reference's
//!   `M3EOverflowStrategy` family onto this seam; see that module's docs for
//!   the full mapping (the family is upstream a `ButtonGroup`-level
//!   abstraction, not a single button's own prop).
//!
//! # Mapping Flutter plumbing that has no frust analogue
//!
//! | Reference | Here |
//! |---|---|
//! | `WidgetStatesController` + `WidgetStateProperty` resolvers | [`crate::interaction::InteractionState`] plus plain per-state resolver fns; there is no external states controller to inject |
//! | `FilledButton`/`OutlinedButton`/… delegation | this widget paints its own container, outline, state layer and content |
//! | `InkSparkle`/`splashFactory` ripple | [`crate::state_layer::StateLayer`] — an M3 state-layer overlay, no ripple engine in this framework |
//! | `FocusNode`/`autofocus`/`onFocusChange` | `PaintCtx::has_focus` only (see Focus ring above) |
//! | `Tooltip`, `onLongPress`, `onHover` callbacks | not ported in v1: no tooltip host in the catalog, and neither long-press nor a hover callback has a reference-visual attached to it |
//! | `enableFeedback` (platform click sound) | no analogue; haptics are the [`crate::interaction::MaterialHaptics`] hook below |
//!
//! # Haptics
//!
//! `m3e_button_content.dart:179` fires `M3EHaptics.trigger(decoration?.haptic
//! ?? M3EHapticFeedback.none)` inside its own `onPressed` wrapper — i.e. a
//! button is silent unless its decoration asked for a signal. This port
//! matches that exactly: [`ButtonView::haptic`] defaults to
//! [`HapticSignal::None`] and the fire happens on the same up-inside edge that
//! runs `on_press`, through [`crate::interaction::MaterialHaptics::fire`]
//! (a no-op until an app installs a hook).
//!
//! # Attribution
//!
//! See `plugins/material/NOTICE`'s "MIT License — Additional Copyright Holders
//! (Vendored Components)" section (Mudit Purohit / m3e_buttons) and its Module
//! Attribution Header Convention.

mod core;
pub mod gradient;
pub(crate) mod motion;
mod overflow;

use std::rc::Rc;

use frust::authoring::{AnyView, BuildCtx, ChangeFlags, ChildPod, View};
use kurbo::Point;

use crate::interaction::{HapticSignal, InteractionState};

use self::core::LabelRun;
pub use self::core::{
    ButtonDecoration, ButtonSurface, ContentMetrics, DecorationOutcome, OverflowObserver,
};
pub use self::gradient::{
    GradientAlignment, GradientButtonDecoration, GradientProperty, GradientSpec, GradientStates,
    LinearGradientSpec, RadialGradientSpec, SweepGradientSpec, constant_gradient, implied_stops,
};
use self::motion::RadiusPaddingMotion;
pub use self::overflow::OverflowStrategy;

/// Which container treatment a button paints — the reference's
/// `M3EButtonStyle` (`m3e_button_enums.dart:19`). See the [module docs](self)'
/// Variants table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    /// Solid `primary` container, highest emphasis (the default).
    #[default]
    Filled,
    /// Transparent container with an `outline` hairline.
    Outlined,
    /// `secondary_container` container, medium emphasis.
    Tonal,
    /// `surface_container_low` container that carries a shadow.
    Elevated,
    /// No container and no outline, lowest emphasis.
    Text,
}

/// The button's size tier — the reference's `M3EButtonSize`
/// (`m3e_button_enums.dart:64`). See the [module docs](self)' Sizes table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    /// 32dp tall.
    Xs,
    /// 40dp tall (the default).
    #[default]
    Sm,
    /// 56dp tall.
    Md,
    /// 96dp tall.
    Lg,
    /// 136dp tall.
    Xl,
}

/// The button's corner-radius family — the reference's `M3EButtonShape`
/// (`m3e_button_enums.dart:45`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonShape {
    /// A pill: `height / 2` (the default).
    #[default]
    Round,
    /// The per-size token radius. See the [module docs](self)' shape table.
    Square,
}

/// Which side of the label an icon sits on — the reference's `IconAlignment`
/// as consumed by `_M3EButtonIconLayout` (`m3e_button_state.dart:38`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconAlignment {
    /// Leading (the default).
    #[default]
    Start,
    /// Trailing.
    End,
}

/// A view-held, typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// A declarative M3 Expressive button. See the [module docs](self).
pub struct ButtonView<State: 'static> {
    label: String,
    variant: ButtonVariant,
    size: ButtonSize,
    shape: ButtonShape,
    enabled: bool,
    icon: Option<AnyView<State>>,
    icon_alignment: IconAlignment,
    corner_radius: Option<f64>,
    pressed_radius: Option<f64>,
    haptic: HapticSignal,
    decoration: Option<Rc<dyn ButtonDecoration>>,
    overflow: Option<Rc<dyn OverflowObserver>>,
    overflow_strategy: OverflowStrategy,
    on_press: OnPress<State>,
}

/// Create a filled (highest-emphasis) button labelled `label`, running
/// `on_press` on release inside its bounds — the reference's default
/// `M3EButton`/`M3EButton.filled`.
///
/// Chain [`ButtonView::size`]/[`ButtonView::shape`]/[`ButtonView::variant`] for
/// the other tiers, [`ButtonView::icon`] to add a leading icon, and
/// [`ButtonView::enabled`] to disable it.
pub fn button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    ButtonView {
        label: label.into(),
        variant: ButtonVariant::Filled,
        size: ButtonSize::default(),
        shape: ButtonShape::default(),
        enabled: true,
        icon: None,
        icon_alignment: IconAlignment::default(),
        corner_radius: None,
        pressed_radius: None,
        haptic: HapticSignal::None,
        decoration: None,
        overflow: None,
        overflow_strategy: OverflowStrategy::default(),
        on_press: Rc::new(on_press),
    }
}

/// PascalCase alias for [`button`], matching this catalog's view-fn vocabulary
/// (`ButtonGroup`, `AssistChip`, …).
#[allow(non_snake_case)]
pub fn Button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    button(label, on_press)
}

/// Create a filled button — [`button`] under the name the reference's
/// `M3EButton.filled` constructor uses.
pub fn filled_button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    button(label, on_press)
}

/// Create an outlined (medium-emphasis, hairline-bordered) button —
/// `M3EButton.outlined`.
pub fn outlined_button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    button(label, on_press).variant(ButtonVariant::Outlined)
}

/// Create a tonal (medium-emphasis, `secondary_container`) button —
/// `M3EButton.tonal`.
pub fn tonal_button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    button(label, on_press).variant(ButtonVariant::Tonal)
}

/// Create an elevated (medium-emphasis, shadowed) button —
/// `M3EButton.elevated`.
pub fn elevated_button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    button(label, on_press).variant(ButtonVariant::Elevated)
}

/// Create a text (lowest-emphasis, chrome-free) button — `M3EButton.text`.
pub fn text_button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    button(label, on_press).variant(ButtonVariant::Text)
}

/// Create a filled button laying `icon` out beside `label` — the reference's
/// `M3EButton.icon` factory (`m3e_buttons.dart:64`).
///
/// The icon is measured at the size's own icon dimension and separated from
/// the label by the size's icon gap (the [module docs](self)' Sizes table);
/// [`ButtonView::icon_alignment`] moves it to the trailing side. Identical to
/// `button(label, on_press).icon(icon)` — both spellings exist because the
/// reference has both a factory and an `iconAlignment`-carrying decoration.
pub fn button_with_icon<State: 'static, F: Fn(&mut State) + 'static>(
    icon: AnyView<State>,
    label: impl Into<String>,
    on_press: F,
) -> ButtonView<State> {
    button(label, on_press).icon(icon)
}

impl<State: 'static> ButtonView<State> {
    /// Set the container treatment (the [module docs](self)' Variants table).
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set the size tier (the [module docs](self)' Sizes table).
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Set the corner-radius family (pill vs. per-size token radius).
    pub fn shape(mut self, shape: ButtonShape) -> Self {
        self.shape = shape;
        self
    }

    /// Whether the button accepts a press. A disabled button paints the 38% /
    /// 12% disabled roles, reports disabled semantics, and swallows nothing —
    /// it simply never arms (`m3e_button_content.dart:174`'s null
    /// `onPressed`).
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Attach a leading icon (see [`button_with_icon`]).
    pub fn icon(mut self, icon: AnyView<State>) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Put the icon on the trailing side instead of the leading one.
    pub fn icon_alignment(mut self, alignment: IconAlignment) -> Self {
        self.icon_alignment = alignment;
        self
    }

    /// Override the resting corner radius, ignoring both the shape family and
    /// the per-size token — the reference's `M3EButtonDecoration.borderRadius`
    /// (`m3e_button_content.dart:42`). It also becomes the hovered radius, and
    /// the pressed one unless [`ButtonView::pressed_radius`] is set too, since
    /// that is the precedence `_resolveShapes` applies.
    pub fn corner_radius(mut self, radius: f64) -> Self {
        self.corner_radius = Some(radius);
        self
    }

    /// Override the pressed corner radius — the reference's
    /// `M3EButtonDecoration.pressedRadius`, the highest-precedence pressed
    /// input.
    pub fn pressed_radius(mut self, radius: f64) -> Self {
        self.pressed_radius = Some(radius);
        self
    }

    /// The haptic signal to fire on a press. Defaults to
    /// [`HapticSignal::None`] — matching the reference, where a button is
    /// silent unless its decoration names a signal (the [module docs](self)'
    /// Haptics section).
    pub fn haptic(mut self, haptic: HapticSignal) -> Self {
        self.haptic = haptic;
        self
    }

    /// Install a paint-time decoration layer (the [module docs](self)' Seams
    /// section).
    pub fn decoration(mut self, decoration: Rc<dyn ButtonDecoration>) -> Self {
        self.decoration = Some(decoration);
        self
    }

    /// Install a layout-time content-measurement observer (the
    /// [module docs](self)' Seams section).
    pub fn overflow_observer(mut self, observer: Rc<dyn OverflowObserver>) -> Self {
        self.overflow = Some(observer);
        self
    }

    /// Select which strategy an overflowing label uses —
    /// [`OverflowStrategy::None`] (the default) by default. See
    /// [`OverflowStrategy`]'s own docs for the full three-way contract, and
    /// [`ButtonView::overflow_observer`] for the hook
    /// [`OverflowStrategy::BottomSheet`] calls into.
    pub fn overflow(mut self, strategy: OverflowStrategy) -> Self {
        self.overflow_strategy = strategy;
        self
    }
}

/// The retained widget for a [`ButtonView`]. Its layout/paint/event core lives
/// in this module's `core` submodule.
pub struct ButtonWidget {
    pub(super) variant: ButtonVariant,
    pub(super) size: ButtonSize,
    pub(super) shape: ButtonShape,
    pub(super) enabled: bool,
    /// The button's own shaped label run (not a child pod: the label's ink is
    /// resolved per variant *and* per interaction state at paint time — the
    /// same reason [`mod@crate::text_field`] owns its runs).
    label: LabelRun,
    /// Where `layout` placed the label, in widget-local coordinates.
    label_origin: Point,
    icon: Option<ChildPod>,
    icon_alignment: IconAlignment,
    corner_radius: Option<f64>,
    pressed_radius: Option<f64>,
    haptic: HapticSignal,
    /// Hover/focus/pressed tracking feeding the state-layer overlay and the
    /// morph's target radius.
    state: InteractionState,
    /// Whether this widget holds the pointer capture a `Down` took.
    captured: bool,
    /// The radius+padding press morph.
    motion: RadiusPaddingMotion,
    /// What the last layout measured — also what [`OverflowObserver`] is
    /// handed.
    metrics: ContentMetrics,
    decoration: Option<Rc<dyn ButtonDecoration>>,
    overflow: Option<Rc<dyn OverflowObserver>>,
    /// Which behavior an overflowing label uses — see [`overflow`]'s module
    /// docs.
    pub(super) overflow_strategy: OverflowStrategy,
    /// [`OverflowStrategy::Scroll`]'s live pan offset, logical px from the
    /// label's leading edge. Clamped every layout to
    /// `overflow::max_scroll`'s range, and reset to `0` whenever the label
    /// stops overflowing or the strategy changes away from `Scroll`.
    pub(super) scroll_offset: f64,
    /// The pointer's `x` at the last `Move` seen while captured — the
    /// previous sample [`OverflowStrategy::Scroll`] diffs a new `Move`
    /// against to find the drag delta. `None` outside a capture.
    pub(super) drag_last_x: Option<f64>,
    on_press: frust::authoring::ErasedCallback,
}

impl<State: 'static> View<State> for ButtonView<State> {
    type Element = ButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ButtonWidget {
        let icon = self
            .icon
            .as_ref()
            .map(|icon| frust::authoring::build_child(icon, ctx));
        ButtonWidget {
            variant: self.variant,
            size: self.size,
            shape: self.shape,
            enabled: self.enabled,
            label: LabelRun::new(self.label.clone()),
            label_origin: Point::ZERO,
            icon,
            icon_alignment: self.icon_alignment,
            corner_radius: self.corner_radius,
            pressed_radius: self.pressed_radius,
            haptic: self.haptic,
            state: InteractionState::new(),
            captured: false,
            motion: RadiusPaddingMotion::new(),
            metrics: ContentMetrics::empty(),
            decoration: self.decoration.clone(),
            overflow: self.overflow.clone(),
            overflow_strategy: self.overflow_strategy,
            scroll_offset: 0.0,
            drag_last_x: None,
            on_press: frust::authoring::erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = frust::authoring::erase_callback(&self.on_press);
        element.decoration = self.decoration.clone();
        element.overflow = self.overflow.clone();
        element.haptic = self.haptic;
        let mut flags = ChangeFlags::NONE;

        if prev.overflow_strategy != self.overflow_strategy {
            element.overflow_strategy = self.overflow_strategy;
            // A live pan position from the old strategy means nothing under
            // the new one.
            element.scroll_offset = 0.0;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.label != self.label {
            element.label.set_content(&self.label);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.shape != self.shape {
            element.shape = self.shape;
            flags |= ChangeFlags::PAINT;
        }
        if prev.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                // A button disabled mid-press keeps neither the press nor the
                // capture it was holding.
                element.state.set_pressed(false);
                element.captured = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        if prev.icon_alignment != self.icon_alignment {
            element.icon_alignment = self.icon_alignment;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.corner_radius != self.corner_radius {
            element.corner_radius = self.corner_radius;
            flags |= ChangeFlags::PAINT;
        }
        if prev.pressed_radius != self.pressed_radius {
            element.pressed_radius = self.pressed_radius;
            flags |= ChangeFlags::PAINT;
        }

        match (&prev.icon, &self.icon) {
            (None, None) => {}
            (Some(p), Some(n)) => {
                let pod = element.icon.as_mut().expect("icon pod present");
                flags |= frust::authoring::rebuild_child(p, n, pod, ctx);
            }
            (None, Some(n)) => {
                element.icon = Some(frust::authoring::build_child(n, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(p), None) => {
                let mut pod = element.icon.take().expect("icon pod present");
                frust::authoring::teardown_child(p, &mut pod, ctx);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }

        flags
    }

    fn teardown(&self, element: &mut ButtonWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(icon_view), Some(pod)) = (&self.icon, element.icon.as_mut()) {
            frust::authoring::teardown_child(icon_view, pod, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_widgets::test_support::leaf_any;

    /// Build a widget from a view over any state type — icon-carrying views
    /// are `()`-stated, since `frust-widgets`' `leaf_any` fixture erases to
    /// `AnyView<()>`.
    fn build<S: 'static>(view: &ButtonView<S>) -> ButtonWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn the_named_constructors_map_onto_the_reference_styles() {
        // m3e_buttons.dart:115/:137/:159/:181/:203 — filled/tonal/elevated/
        // outlined/text named constructors; the unnamed one defaults to
        // filled (`:45`).
        assert_eq!(
            button::<u32, _>("a", |_| {}).variant,
            ButtonVariant::Filled,
            "the default constructor is filled"
        );
        assert_eq!(
            filled_button::<u32, _>("a", |_| {}).variant,
            ButtonVariant::Filled
        );
        assert_eq!(
            outlined_button::<u32, _>("a", |_| {}).variant,
            ButtonVariant::Outlined
        );
        assert_eq!(
            tonal_button::<u32, _>("a", |_| {}).variant,
            ButtonVariant::Tonal
        );
        assert_eq!(
            elevated_button::<u32, _>("a", |_| {}).variant,
            ButtonVariant::Elevated
        );
        assert_eq!(
            text_button::<u32, _>("a", |_| {}).variant,
            ButtonVariant::Text
        );
    }

    #[test]
    fn defaults_match_the_reference_constructor_defaults() {
        // m3e_buttons.dart:45-:48 — style filled, size sm, shape round,
        // enabled true; m3e_buttons.dart:312 — haptic none.
        let view = button::<u32, _>("Save", |_| {});
        assert_eq!(view.size, ButtonSize::Sm);
        assert_eq!(view.shape, ButtonShape::Round);
        assert!(view.enabled);
        assert_eq!(view.haptic, HapticSignal::None);
        assert_eq!(view.icon_alignment, IconAlignment::Start);
        assert!(view.icon.is_none());
        assert_eq!(view.overflow_strategy, OverflowStrategy::None);
    }

    #[test]
    fn the_icon_factory_and_the_builder_method_agree() {
        let factory = button_with_icon::<(), _>(leaf_any(20.0, 20.0), "Save", |_| {});
        let builder = button::<(), _>("Save", |_| {}).icon(leaf_any(20.0, 20.0));
        assert!(factory.icon.is_some());
        assert!(builder.icon.is_some());
        assert_eq!(factory.label, builder.label);
        assert_eq!(factory.variant, builder.variant);
    }

    #[test]
    fn build_carries_every_prop_onto_the_widget() {
        let view = button::<(), _>("Save", |_| {})
            .variant(ButtonVariant::Tonal)
            .size(ButtonSize::Lg)
            .shape(ButtonShape::Square)
            .enabled(false)
            .haptic(HapticSignal::Light)
            .icon(leaf_any(32.0, 32.0))
            .icon_alignment(IconAlignment::End)
            .corner_radius(9.0)
            .pressed_radius(3.0)
            .overflow(OverflowStrategy::Scroll);
        let widget = build(&view);
        assert_eq!(widget.variant, ButtonVariant::Tonal);
        assert_eq!(widget.size, ButtonSize::Lg);
        assert_eq!(widget.shape, ButtonShape::Square);
        assert!(!widget.enabled);
        assert_eq!(widget.haptic, HapticSignal::Light);
        assert_eq!(widget.icon_alignment, IconAlignment::End);
        assert_eq!(widget.corner_radius, Some(9.0));
        assert_eq!(widget.pressed_radius, Some(3.0));
        assert!(widget.icon.is_some());
        assert_eq!(widget.overflow_strategy, OverflowStrategy::Scroll);
    }

    #[test]
    fn rebuild_carries_a_changed_overflow_strategy_and_resets_the_scroll_offset() {
        let prev = button::<u32, _>("Save", |_| {});
        let next = button::<u32, _>("Save", |_| {}).overflow(OverflowStrategy::BottomSheet);
        let mut widget = build(&prev);
        widget.scroll_offset = 12.0;
        let mut counter = 0u64;
        let flags =
            View::<u32>::rebuild(&next, &prev, &mut widget, &mut BuildCtx::new(&mut counter));
        assert_eq!(widget.overflow_strategy, OverflowStrategy::BottomSheet);
        assert_eq!(widget.scroll_offset, 0.0);
        assert!(flags.contains(ChangeFlags::LAYOUT));
        assert!(flags.contains(ChangeFlags::PAINT));
    }

    #[test]
    fn rebuild_moves_props_and_flags_what_changed() {
        let prev = button::<u32, _>("Save", |_| {});
        let next = button::<u32, _>("Saved", |_| {})
            .variant(ButtonVariant::Text)
            .size(ButtonSize::Xl);
        let mut widget = build(&prev);
        let mut counter = 0u64;
        let flags =
            View::<u32>::rebuild(&next, &prev, &mut widget, &mut BuildCtx::new(&mut counter));
        assert_eq!(widget.variant, ButtonVariant::Text);
        assert_eq!(widget.size, ButtonSize::Xl);
        assert_eq!(
            widget.label.content(),
            "Saved",
            "the run takes the new text (and drops its cached shaping)"
        );
        assert!(flags.contains(ChangeFlags::LAYOUT), "text + size relayout");
        assert!(flags.contains(ChangeFlags::PAINT));
    }

    #[test]
    fn disabling_mid_press_clears_the_press_and_the_capture() {
        let prev = button::<u32, _>("Save", |_| {});
        let next = button::<u32, _>("Save", |_| {}).enabled(false);
        let mut widget = build(&prev);
        widget.state.set_pressed(true);
        widget.captured = true;
        let mut counter = 0u64;
        View::<u32>::rebuild(&next, &prev, &mut widget, &mut BuildCtx::new(&mut counter));
        assert!(!widget.state.pressed);
        assert!(!widget.captured);
    }
}
