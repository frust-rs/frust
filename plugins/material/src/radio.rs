//! The M3 `Radio` button: a generic value/group-value single-selection
//! control with an optional label slot, an error flavor, and an inner-dot
//! scale-in animation.
//!
//! Source: `paadevelopments/material_3_expressive` v1.0.8 (MIT, © 2026 Paa
//! Developments — see `plugins/material/NOTICE`'s root-crate MIT section),
//! `lib/components/radio_button/{m3e_radio_button.dart,
//! styles/m3e_radio_theme.dart}`'s `M3ERadio<T>`/`M3ERadioTheme` (retrieved
//! 2026-08-19). No license changes; the generic-`T` selection model, the
//! optional label slot, and the dot scale-in are all preserved from the Dart
//! source — see the sections below for what changed in translation.
//!
//! # Naming: plain `radio`/`Radio`, deliberately
//!
//! Baseline `frust-widgets` already ships a non-generic `radio`/[`Radio`]
//! ([`frust::radio`]) taking a plain `bool` `selected`. This module reuses
//! the same plain names in `frust_material` on purpose — the same
//! cross-crate naming call `frust_material::switch`/[`super::switch::Switch`]
//! already made against a baseline widget with no direct counterpart, now
//! extended to a name baseline *does* have one for. Two catalogs (or a
//! catalog and the baseline set) legitimately reuse a vocabulary word; an app
//! picks exactly one by import path (`frust_material::radio` vs.
//! `frust::radio`), never both in the same call site. This crate's own
//! `radio` is a strict superset of what the baseline widget can express
//! (generic `T`, group-value selection, optional label, error flavor,
//! animation) — recorded here as the deliberate decision, not an oversight.
//!
//! # Generic selection model
//!
//! [`radio`] takes an owned `value: T` and `group_value: T` (`T: PartialEq +
//! Clone + 'static`, matching the Dart source's `T`/`T?` pair) and computes
//! `selected = value == group_value` once, at `View::build`/`rebuild` — the
//! same shape `M3ERadio<T>`'s `_selected` getter uses. [`RadioView::on_changed`]
//! attaches a single `Fn(&mut State, T) + 'static` callback shared by every
//! member of a group (mirroring Dart's `ValueChanged<T>`): a release inside
//! bounds fires `on_changed(state, value.clone())` — reporting *this* radio's
//! own value, not a derived index — and never self-mutates. `T` is erased out
//! of the retained [`RadioWidget`] entirely: `on_changed` is bound with
//! `value` already captured into a plain [`frust::authoring::ErasedCallback`]
//! at build/rebuild time, so the widget itself carries no generic parameter
//! (the same type-erase-to-avoid-a-generic idiom `docs/CODE_STANDARDS.md`'s
//! Language Idioms describes for a downstream-crate dependency, applied here
//! to a generic instead).
//!
//! # Enabled/disabled
//!
//! Mirroring the Dart source's `_enabled => onChanged != null` (and
//! `M3ETappable`'s `_isInteractive = enabled && onTap != null`, which for
//! `M3ERadio` reduces to the same condition since it never sets `enabled`
//! itself): a radio built with no [`RadioView::on_changed`] call is
//! **disabled** — it paints its ring/dot dimmed to
//! [`crate::interaction::DISABLED_CONTENT_OPACITY`] (also the Dart source's
//! own `disabledOpacity: 0.38`, so the values already agree) over `on_surface`
//! and ignores every pointer event outright, never entering a pressed/captured
//! state. A disabled radio still tracks `selected` (and animates its dot)
//! when the *group's* value changes out from under it via a sibling's own
//! `on_changed` — the Dart source's `AnimatedScale` is keyed on `_selected`
//! alone, independent of `_enabled`, and this port preserves that. The one
//! documented simplification: the optional label's text color does not
//! itself dim when disabled (the Dart source multiplies `onSurface`'s alpha
//! for the label style) — this crate's `frust::text` has no themed-role
//! variant carrying a fixed alpha multiplier, only a plain
//! [`frust::authoring::ThemeTextColor`] role or an explicit
//! [`frust::authoring::AnyView`]-time [`Color`] override neither `build` nor
//! `rebuild` can theme-resolve (see `docs/CODE_STANDARDS.md`'s Theming
//! conventions: only `LayoutCtx`/`PaintCtx` thread a theme). The primary
//! disabled signal — the ring/dot dimming — carries the state correctly; a
//! themed-role-at-reduced-alpha text API is future work, not something this
//! module can reach for.
//!
//! # Dot scale-in: duration + curve, not a spring
//!
//! The reference's `AnimatedScale(scale: _selected ? 1 : 0, duration:
//! M3EMotion.short4, curve: M3EMotion.emphasizedDecelerate)` is a
//! **duration-driven**, not spring-driven, implicit animation — unlike this
//! crate's other toggle-style controls ([`super::switch`], `NavigationBar`'s
//! selection pill), which all animate via
//! [`frust::AnimationController::fling`] against a [`frust::SpringDesc`].
//! This module instead drives [`RadioWidget::anim`] with
//! [`frust::AnimationController::forward`]/[`frust::AnimationController::reverse`]
//! against a fixed duration+curve built once from
//! [`crate::tokens::MaterialMotion::SHORT_4`]/
//! [`crate::tokens::MaterialMotion::EMPHASIZED_DECELERATE`] — both compile-time
//! crate constants, not a per-instance `Theme` lookup, so (unlike `switch`'s
//! themed spring, which must wait for `paint` to have a theme in scope) the
//! motion can start directly in `rebuild` the moment `selected` actually
//! changes. Flutter's `AnimatedScale` applies the *same* curve shape whether
//! animating toward `1` or back toward `0` (not a mirrored/reversed curve);
//! [`frust::AnimationController`]'s own duration-driven interpolation
//! (`value = start + (target - start) * curve.transform(elapsed / duration)`)
//! has the identical property, so `forward`/`reverse` reproduce the Dart
//! source's both-directions animated deselect exactly — a verified fact, not
//! an assumption (`crates/frust-core/src/anim.rs`'s `Drive::Duration` arm).
//!
//! An initially-selected radio never plays a pop-in on first mount (the same
//! contract Flutter's `ImplicitlyAnimatedWidget` gives `AnimatedScale`, and
//! this crate's own [`super::navbar`] indicator pill and [`super::switch`]
//! thumb already establish for their own first-mount snap): [`View::build`]
//! settles [`RadioWidget::anim`] to `1.0` with two `advance` calls — the
//! first only seeds the controller's clock (a documented no-op per
//! [`frust::AnimationController::advance`]), the second covers the whole
//! `SHORT_4` span in one step, so no intermediate frame is ever painted.
//!
//! # Haptics: fires the hook, with the reference's own choice — none
//!
//! [`crate::interaction::MaterialHaptics`] is fired on every successful
//! select — but the Dart source's own `M3ETappable(haptic: ...)` parameter is
//! never passed by `M3ERadio` (verified: neither the widget nor any example
//! usage in the reference package overrides it), so it takes
//! `M3ETappable`'s own default, `M3EHapticFeedback.none`. This module
//! preserves that choice exactly — [`crate::interaction::HapticSignal::None`]
//! is documented as an explicit no-op, identical to firing nothing at all —
//! while still wiring the call site through the shared hook (this crate's
//! first real [`MaterialHaptics::fire`] consumer), so a future upstream
//! change to `M3ERadio`'s own haptic wiring has exactly one call site to
//! update here.

use std::rc::Rc;

use frust::authoring::{Action, Role};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, ThemeTextColor, TypedArgCallback,
    View, Widget, any,
};
use frust::{AnimationController, FrameTime, Theme};
use kurbo::{Circle, Point, Rect, Shape, Size};
use peniko::{Brush, Color};

use super::press::presses;
use super::state_layer::StateLayer;
use crate::interaction::{DISABLED_CONTENT_OPACITY, HapticSignal, MaterialHaptics};
use crate::tokens::MaterialMotion;

/// The control's own tap-target/state-layer diameter, in logical px
/// (`M3ERadioTheme.hitSize`) — also the width the control column reserves in
/// layout, wider than the visible ring so the state layer can extend past it.
const HIT_SIZE: f64 = 40.0;
/// The visible outer ring's diameter, in logical px (`M3ERadioTheme.ringSize`).
const RING_SIZE: f64 = 20.0;
/// The inner dot's fully-scaled-in diameter, in logical px
/// (`M3ERadioTheme.dotSize`).
const DOT_SIZE: f64 = 10.0;
/// The ring's stroke width, in logical px (`M3ERadioTheme.borderWidth`).
const BORDER_WIDTH: f64 = 2.0;
/// Gap between the control and an optional label, in logical px
/// (`M3ERadioTheme.labelGap`).
const LABEL_GAP: f64 = 8.0;
/// Flattening tolerance for the ring's stroked-circle path — a
/// visually-lossless value for on-screen radii, matching
/// [`super::card`]'s `STROKE_TOLERANCE` precedent (the same primitive-choice
/// rationale: no stroked-circle primitive exists on
/// [`frust::authoring::PaintScene`], only `stroke_path` over a `kurbo`
/// [`kurbo::Shape`]).
const STROKE_TOLERANCE: f64 = 0.1;

/// Unthemed fallback selected-ring/dot color (a theme resolves this from
/// `colors.primary`).
const PRIMARY: Color = Color::from_rgb8(0x67, 0x50, 0xA4);
/// Unthemed fallback unselected-ring color (a theme resolves this from
/// `colors.on_surface_variant`).
const ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed fallback state-layer base color, both states share the ring's
/// selected color only when selected (a theme resolves this from
/// `colors.on_surface`).
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed fallback error-flavor ring/dot color (a theme resolves this from
/// `colors.error`).
const ERROR: Color = Color::from_rgb8(0xB3, 0x26, 0x1E);

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`super::state_layer`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved `(ring_color, state_layer_color)` pair.
///
/// Ring/dot (`M3ERadioTheme.color`): disabled wins over everything —
/// `on_surface` at [`DISABLED_CONTENT_OPACITY`]; else error wins over
/// selection — `colors.error`; else `primary` if selected, `on_surface_variant`
/// otherwise. State layer (`M3ERadioTheme.stateLayerColor`): `primary` if
/// selected, `on_surface` otherwise — **not** conditioned on `error`, exactly
/// mirroring the Dart source (the state-layer overlay tints the same either
/// way; only the ring/dot recolor for the error flavor).
fn resolve_colors(
    theme: Option<&Theme>,
    selected: bool,
    error: bool,
    enabled: bool,
) -> (Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            let ring = if !enabled {
                with_alpha(scheme.on_surface, DISABLED_CONTENT_OPACITY)
            } else if error {
                scheme.error
            } else if selected {
                scheme.primary
            } else {
                scheme.on_surface_variant
            };
            let state_layer = if selected {
                scheme.primary
            } else {
                scheme.on_surface
            };
            (ring, state_layer)
        }
        None => {
            let ring = if !enabled {
                with_alpha(ON_SURFACE, DISABLED_CONTENT_OPACITY)
            } else if error {
                ERROR
            } else if selected {
                PRIMARY
            } else {
                ON_SURFACE_VARIANT
            };
            let state_layer = if selected { PRIMARY } else { ON_SURFACE };
            (ring, state_layer)
        }
    }
}

/// Whether a widget-local `pos` lies within a `size`-sized box anchored at
/// the origin (mirrors [`super::switch`]'s helper of the same shape).
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// Build the optional label's type-erased child view, themed `on_surface`
/// (`frust::text`'s default body style) — see the module docs' Enabled/
/// disabled section for why this doesn't dim when the radio is disabled.
fn label_view<State: 'static>(label: String) -> AnyView<State> {
    any::<State, _>(frust::text(label).themed_role(ThemeTextColor::OnSurface))
}

/// Erase a view-held, generic `Fn(&mut State, T)` group handler into a plain
/// [`frust::authoring::ErasedCallback`] with `value` already bound — see the
/// module docs' Generic selection model section for why this is what lets
/// [`RadioWidget`] carry no generic parameter of its own.
fn bind_on_changed<State: 'static, T: Clone + 'static>(
    on_changed: &TypedArgCallback<State, T>,
    value: T,
) -> frust::authoring::ErasedCallback {
    let on_changed = on_changed.clone();
    Box::new(move |ctx: &mut EventCtx| {
        let state = ctx.state_mut::<State>();
        on_changed(state, value.clone());
    })
}

/// A declarative M3 radio button. See the [module docs](self).
pub struct RadioView<State: 'static, T: PartialEq + Clone + 'static> {
    value: T,
    group_value: T,
    on_changed: Option<TypedArgCallback<State, T>>,
    label: Option<String>,
    error: bool,
}

/// Create a radio for `value`, reflecting `selected = value == group_value`.
/// Attach a shared group handler with [`RadioView::on_changed`]; a radio with
/// none is **disabled** — see the [module docs](self)' Enabled/disabled
/// section.
pub fn radio<State: 'static, T: PartialEq + Clone + 'static>(
    value: T,
    group_value: T,
) -> RadioView<State, T> {
    RadioView {
        value,
        group_value,
        on_changed: None,
        label: None,
        error: false,
    }
}

/// PascalCase alias for [`radio`].
#[allow(non_snake_case)]
pub fn Radio<State: 'static, T: PartialEq + Clone + 'static>(
    value: T,
    group_value: T,
) -> RadioView<State, T> {
    radio(value, group_value)
}

impl<State: 'static, T: PartialEq + Clone + 'static> RadioView<State, T> {
    /// Fire `on_changed(state, value.clone())` on release inside this
    /// radio's bounds (whole control **and** an attached [`Self::label`]) —
    /// unconditionally, even if already selected, mirroring the Dart
    /// source's `onTap`. Never self-mutates. Omitting this call leaves the
    /// radio disabled — see the [module docs](self).
    pub fn on_changed<F: Fn(&mut State, T) + 'static>(mut self, on_changed: F) -> Self {
        self.on_changed = Some(Rc::new(on_changed));
        self
    }

    /// Attach an optional text label beside the control; tapping it also
    /// selects (the whole control+label bounds is one press target).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the error flavor: recolors the ring/dot to `colors.error` while
    /// enabled (a disabled radio ignores this — see [`resolve_colors`]).
    pub fn error(mut self, error: bool) -> Self {
        self.error = error;
        self
    }
}

/// The retained widget for a [`RadioView`] — carries no generic parameter;
/// see the module docs' Generic selection model section.
pub struct RadioWidget {
    selected: bool,
    enabled: bool,
    error: bool,
    /// Drives the inner dot's `0.0` (hidden) .. `1.0` (fully scaled in)
    /// fraction — duration+curve, not a spring; see the module docs.
    anim: AnimationController,
    state_layer: StateLayer,
    label_pod: Option<ChildPod>,
    /// The label text, retained for the semantics node's accessible name.
    label_text: Option<String>,
    /// The pressed *visual* state; follows the cursor in/out while captured.
    pressed: bool,
    /// Armed by a `Down`, cleared on `Up`/`Cancel`.
    captured: bool,
    on_changed: Option<frust::authoring::ErasedCallback>,
}

impl<State: 'static, T: PartialEq + Clone + 'static> View<State> for RadioView<State, T> {
    type Element = RadioWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> RadioWidget {
        let selected = self.value == self.group_value;
        let enabled = self.on_changed.is_some();

        let label_pod = self
            .label
            .as_ref()
            .map(|label| frust::authoring::build_child(&label_view::<State>(label.clone()), ctx));

        let mut anim = AnimationController::new(MaterialMotion::SHORT_4)
            .with_curve(MaterialMotion::EMPHASIZED_DECELERATE);
        if selected {
            anim.forward();
            // Settle instantly on first mount — see the module docs' Dot
            // scale-in section for why two `advance` calls are needed.
            anim.advance(FrameTime::ZERO);
            anim.advance(FrameTime::from_nanos(
                MaterialMotion::SHORT_4.as_nanos() as u64
            ));
        }

        RadioWidget {
            selected,
            enabled,
            error: self.error,
            anim,
            state_layer: StateLayer::new(),
            label_pod,
            label_text: self.label.clone(),
            pressed: false,
            captured: false,
            on_changed: self
                .on_changed
                .as_ref()
                .map(|cb| bind_on_changed::<State, T>(cb, self.value.clone())),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RadioWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        // Closures aren't comparable — always reinstall the adapter.
        element.on_changed = self
            .on_changed
            .as_ref()
            .map(|cb| bind_on_changed::<State, T>(cb, self.value.clone()));

        let selected = self.value == self.group_value;
        if element.selected != selected {
            element.selected = selected;
            // Compile-time duration+curve — no theme needed, so (unlike
            // `switch`'s themed spring) this can start right here rather
            // than waiting for `paint`. See the module docs.
            if selected {
                element.anim.forward();
            } else {
                element.anim.reverse();
            }
            flags |= ChangeFlags::PAINT;
        }

        let enabled = self.on_changed.is_some();
        if element.enabled != enabled {
            element.enabled = enabled;
            flags |= ChangeFlags::PAINT;
        }

        if element.error != self.error {
            element.error = self.error;
            flags |= ChangeFlags::PAINT;
        }

        match (&prev.label, &self.label) {
            (None, None) => {}
            (Some(p), Some(n)) if p == n => {}
            (Some(_), Some(n)) => {
                element.label_text = Some(n.clone());
                let pod = element.label_pod.as_mut().expect("label pod present");
                let prev_view = label_view::<State>(prev.label.clone().unwrap_or_default());
                let next_view = label_view::<State>(n.clone());
                flags |= frust::authoring::rebuild_child(&prev_view, &next_view, pod, ctx);
            }
            (None, Some(n)) => {
                element.label_text = Some(n.clone());
                element.label_pod = Some(frust::authoring::build_child(
                    &label_view::<State>(n.clone()),
                    ctx,
                ));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(p), None) => {
                element.label_text = None;
                let mut pod = element.label_pod.take().expect("label pod present");
                let prev_view = label_view::<State>(p.clone());
                frust::authoring::teardown_child(&prev_view, &mut pod, ctx);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }

        flags
    }

    fn teardown(&self, element: &mut RadioWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(label), Some(pod)) = (&self.label, element.label_pod.as_mut()) {
            let label_view = label_view::<State>(label.clone());
            frust::authoring::teardown_child(&label_view, pod, ctx);
        }
    }
}

impl Widget for RadioWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let Some(label) = self.label_pod.as_mut() else {
            return bc.constrain(Size::new(HIT_SIZE, HIT_SIZE));
        };
        let label_max = Size::new(
            (bc.max().width - HIT_SIZE - LABEL_GAP).max(0.0),
            bc.max().height,
        );
        let label_size = label.layout_child(ctx, &BoxConstraints::loose(label_max));
        let height = label_size.height.max(HIT_SIZE);
        // Vertically centre the label against the control column.
        label.set_origin(Point::new(
            HIT_SIZE + LABEL_GAP,
            (height - label_size.height) / 2.0,
        ));
        bc.constrain(Size::new(HIT_SIZE + LABEL_GAP + label_size.width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (ring_color, state_layer_color) =
            resolve_colors(theme, self.selected, self.error, self.enabled);

        if self.anim.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        // Duration+curve motion is already bounded 0.0..=1.0 (see
        // `AnimationController::value`'s own docs) — no clamping needed.
        let progress = self.anim.value();

        let o = ctx.origin();
        let cy = ctx.size().height / 2.0;
        let center_local = Point::new(HIT_SIZE / 2.0, cy);
        let center_abs = Point::new(o.x + HIT_SIZE / 2.0, o.y + cy);

        self.state_layer.paint(
            ctx,
            scene,
            Rect::from_center_size(center_abs, Size::new(HIT_SIZE, HIT_SIZE)),
            HIT_SIZE / 2.0,
            state_layer_color,
        );

        let ring_radius = (RING_SIZE - BORDER_WIDTH) / 2.0;
        let ring_path = Circle::new(center_local, ring_radius).to_path(STROKE_TOLERANCE);
        scene.stroke_path(o, &ring_path, BORDER_WIDTH, &Brush::Solid(ring_color));

        let dot_diam = DOT_SIZE * progress;
        scene.fill_rounded_rect(
            Point::new(center_abs.x - dot_diam / 2.0, center_abs.y - dot_diam / 2.0),
            Size::new(dot_diam, dot_diam),
            dot_diam / 2.0,
            ring_color,
        );

        if let Some(label) = self.label_pod.as_mut() {
            label.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        // A disabled radio (`M3ETappable`'s `_isInteractive` reducing to
        // `_enabled` for this widget — see the module docs) ignores every
        // pointer phase outright, never arming a press.
        if !self.enabled {
            return EventResult::Ignored;
        }
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                self.state_layer.set_pressed(true);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                let inside_now = inside(p.position, ctx.size());
                self.pressed = inside_now;
                self.state_layer.set_pressed(inside_now);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size())
                    && let Some(on_changed) = self.on_changed.as_mut()
                {
                    // The reference's own choice — see the module docs'
                    // Haptics section.
                    MaterialHaptics::fire(HapticSignal::None);
                    (on_changed)(ctx);
                }
                self.pressed = false;
                self.captured = false;
                self.state_layer.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                self.state_layer.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::RadioButton, |node| {
            if let Some(label) = &self.label_text {
                node.set_label(label.as_str());
            }
            node.set_selected(self.selected);
            if self.enabled {
                node.add_action(Action::Click);
            }
        });
    }

    frust::authoring::visit_children!(label_pod);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Freq {
        Daily,
        Weekly,
        Monthly,
    }

    struct GroupState {
        selected: Freq,
        changes: u32,
    }

    fn on_changed(s: &mut GroupState, v: Freq) {
        s.selected = v;
        s.changes += 1;
    }

    fn built(value: Freq, group_value: Freq, enabled: bool) -> RadioWidget {
        let view = radio::<GroupState, Freq>(value, group_value);
        let view = if enabled {
            view.on_changed(on_changed)
        } else {
            view
        };
        let mut counter = 0u64;
        View::<GroupState>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Primary,
        })
    }

    fn secondary_ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Secondary,
        })
    }

    fn dispatch(w: &mut RadioWidget, state: &mut GroupState, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(HIT_SIZE, HIT_SIZE));
        w.event(&mut ctx, event);
    }

    // ---- Acceptance 1: select/deselect across a 3-member group -----------

    #[test]
    fn select_and_deselect_across_a_three_member_group() {
        let mut state = GroupState {
            selected: Freq::Daily,
            changes: 0,
        };

        // Round 1: group starts on Daily.
        let daily = built(Freq::Daily, state.selected, true);
        let mut weekly = built(Freq::Weekly, state.selected, true);
        let monthly = built(Freq::Monthly, state.selected, true);
        assert!(daily.selected);
        assert!(!weekly.selected);
        assert!(!monthly.selected);

        // Tap "weekly": fires the shared handler with its own value.
        dispatch(&mut weekly, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut weekly, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.selected, Freq::Weekly);
        assert_eq!(state.changes, 1);
        assert!(
            !weekly.selected,
            "must not self-mutate — still false until the app rebuilds"
        );

        // The app rebuilds all three against the new group value.
        let daily = built(Freq::Daily, state.selected, true);
        let weekly = built(Freq::Weekly, state.selected, true);
        let monthly = built(Freq::Monthly, state.selected, true);
        assert!(!daily.selected, "Daily deselected");
        assert!(weekly.selected, "Weekly now selected");
        assert!(!monthly.selected);

        // Round 2: tap "monthly" next — deselects Weekly, selects Monthly.
        let mut monthly = monthly;
        dispatch(&mut monthly, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut monthly, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.selected, Freq::Monthly);
        assert_eq!(state.changes, 2);

        let daily = built(Freq::Daily, state.selected, true);
        let weekly = built(Freq::Weekly, state.selected, true);
        let monthly = built(Freq::Monthly, state.selected, true);
        assert!(!daily.selected);
        assert!(!weekly.selected, "Weekly deselected");
        assert!(monthly.selected, "Monthly now selected");
    }

    #[test]
    fn already_selected_still_fires() {
        let mut w = built(Freq::Daily, Freq::Daily, true);
        let mut state = GroupState {
            selected: Freq::Daily,
            changes: 0,
        };
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.changes, 1);
        assert!(w.selected, "still selected until the app rebuilds it");
    }

    #[test]
    fn up_outside_does_not_fire() {
        let mut w = built(Freq::Weekly, Freq::Daily, true);
        let mut state = GroupState {
            selected: Freq::Daily,
            changes: 0,
        };
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 12.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 12.0));
        assert_eq!(state.changes, 0);
    }

    #[test]
    fn hover_move_without_down_is_ignored_noop() {
        let mut w = built(Freq::Weekly, Freq::Daily, true);
        let mut state = GroupState {
            selected: Freq::Daily,
            changes: 0,
        };
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(HIT_SIZE, HIT_SIZE));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 5.0, 12.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.pressed);
        assert!(!ctx.needs_redraw());
    }

    #[test]
    fn cancel_clears_armed_state() {
        let mut w = built(Freq::Weekly, Freq::Daily, true);
        let mut state = GroupState {
            selected: Freq::Daily,
            changes: 0,
        };
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 5.0, 12.0));
        assert!(!w.captured, "Cancel disarms the press");
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.changes, 0);
    }

    #[test]
    fn a_secondary_press_never_presses_captures_or_fires() {
        let mut w = built(Freq::Weekly, Freq::Daily, true);
        let mut state = GroupState {
            selected: Freq::Daily,
            changes: 0,
        };
        dispatch(
            &mut w,
            &mut state,
            &secondary_ev(PointerPhase::Down, 5.0, 12.0),
        );
        assert!(!w.pressed);
        assert!(!w.captured);
        dispatch(
            &mut w,
            &mut state,
            &secondary_ev(PointerPhase::Up, 5.0, 12.0),
        );
        assert_eq!(state.changes, 0);

        // The primary gesture is untouched by the guard.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
        assert_eq!(state.changes, 1);
    }

    #[test]
    fn disabled_radio_ignores_every_pointer_phase() {
        let mut w = built(Freq::Weekly, Freq::Daily, false);
        assert!(!w.enabled);
        let mut state = GroupState {
            selected: Freq::Daily,
            changes: 0,
        };
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(HIT_SIZE, HIT_SIZE));
        let down = w.event(&mut ctx, &ev(PointerPhase::Down, 5.0, 12.0));
        assert!(matches!(down, EventResult::Ignored));
        assert!(!w.pressed);
        assert!(!w.captured);
    }

    // ---- Acceptance 2: dot animation progress at t=0/mid/1 ---------------

    fn paint_rec(w: &mut RadioWidget, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, Size::new(HIT_SIZE, HIT_SIZE)).with_theme(t),
            None => PaintCtx::new(Point::ZERO, Size::new(HIT_SIZE, HIT_SIZE)),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[derive(Default)]
    struct Recorder {
        /// `fill_rounded_rect` calls, in paint order: the state layer (only
        /// while active) then the dot.
        rrects: Vec<(Point, Size, f64, Color)>,
        /// `stroke_path` calls' solid brush color, in paint order (the ring).
        strokes: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _path: &kurbo::BezPath, _width: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
    }

    #[test]
    fn dot_scale_in_progress_observable_at_zero_mid_and_settled() {
        let mut counter = 0u64;
        let prev = radio::<GroupState, Freq>(Freq::Daily, Freq::Weekly).on_changed(on_changed);
        let mut w = View::<GroupState>::build(&prev, &mut BuildCtx::new(&mut counter));
        assert!(!w.selected);
        assert_eq!(w.anim.value(), 0.0);

        let next = radio::<GroupState, Freq>(Freq::Daily, Freq::Daily).on_changed(on_changed);
        let flags =
            View::<GroupState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.selected);
        assert!(flags.needs_paint());
        assert!(w.anim.is_animating(), "rebuild started the forward drive");

        // t=0: the first `advance` (inside `paint`) only seeds the clock.
        let rec0 = paint_rec(&mut w, None);
        assert_eq!(
            rec0.rrects[0].1,
            Size::ZERO,
            "dot has not started scaling in yet"
        );

        // t=mid: advance halfway through SHORT_4 directly on the controller
        // (mirroring `super::switch`'s own test pattern — `PaintCtx::new`
        // always reads `FrameTime::ZERO`, so a real elapsed span is driven
        // by hand), then paint again purely to record the result.
        let half_ns = (MaterialMotion::SHORT_4.as_nanos() / 2) as u64;
        w.anim.advance(FrameTime::from_nanos(half_ns));
        let rec_mid = paint_rec(&mut w, None);
        let mid_diam = rec_mid.rrects[0].1.width;
        assert!(
            mid_diam > 0.0 && mid_diam < DOT_SIZE,
            "dot mid-animation diameter should be strictly between 0 and {DOT_SIZE}, got {mid_diam}"
        );

        // t=1: advance to (and past) the full SHORT_4 span — settles exactly.
        let full_ns = MaterialMotion::SHORT_4.as_nanos() as u64;
        w.anim.advance(FrameTime::from_nanos(full_ns));
        assert!(!w.anim.is_animating(), "fling settled");
        let rec_done = paint_rec(&mut w, None);
        assert_eq!(rec_done.rrects[0].1, Size::new(DOT_SIZE, DOT_SIZE));
    }

    #[test]
    fn deselect_also_animates_both_ways() {
        let mut counter = 0u64;
        let prev = radio::<GroupState, Freq>(Freq::Daily, Freq::Daily).on_changed(on_changed);
        let mut w = View::<GroupState>::build(&prev, &mut BuildCtx::new(&mut counter));
        // Settle to fully selected first (build's own instant-settle path).
        assert_eq!(w.anim.value(), 1.0);

        let next = radio::<GroupState, Freq>(Freq::Daily, Freq::Weekly).on_changed(on_changed);
        View::<GroupState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(!w.selected);
        assert!(w.anim.is_animating(), "rebuild started the reverse drive");

        let full_ns = MaterialMotion::SHORT_4.as_nanos() as u64;
        w.anim.advance(FrameTime::ZERO); // seed
        w.anim.advance(FrameTime::from_nanos(full_ns));
        assert!(!w.anim.is_animating());
        assert_eq!(w.anim.value(), 0.0);
        let rec = paint_rec(&mut w, None);
        assert_eq!(rec.rrects[0].1, Size::ZERO, "dot fully scaled back out");
    }

    // ---- Acceptance 3: error flavor colors --------------------------------

    #[test]
    fn error_flavor_recolors_ring_unthemed() {
        let mut w = built(Freq::Weekly, Freq::Daily, true);
        w.error = true;
        let rec = paint_rec(&mut w, None);
        assert_eq!(rec.strokes, vec![ERROR]);
    }

    #[test]
    fn error_flavor_recolors_ring_themed() {
        let theme = crate::baseline();
        let scheme = theme.scheme();
        let mut w = built(Freq::Weekly, Freq::Daily, true);
        w.error = true;
        let rec = paint_rec(&mut w, Some(&theme));
        assert_eq!(rec.strokes, vec![scheme.error]);

        // A selected+error radio still reads `error`, not `primary` — error
        // wins over selection (matches `M3ERadioTheme.color`).
        let mut selected = built(Freq::Daily, Freq::Daily, true);
        selected.error = true;
        let rec = paint_rec(&mut selected, Some(&theme));
        assert_eq!(rec.strokes, vec![scheme.error]);

        // The state layer never recolors for error — always primary/on_surface.
        selected.state_layer.set_pressed(true);
        let rec = paint_rec(&mut selected, Some(&theme));
        assert_eq!(rec.rrects[0].3, with_alpha(scheme.primary, 0.10));
    }

    #[test]
    fn disabled_wins_over_error() {
        let theme = crate::baseline();
        let scheme = theme.scheme();
        let mut w = built(Freq::Weekly, Freq::Daily, false);
        w.error = true;
        let rec = paint_rec(&mut w, Some(&theme));
        assert_eq!(rec.strokes, vec![with_alpha(scheme.on_surface, 0.38)]);
    }

    // ---- Paint: unthemed/themed role resolution ---------------------------

    #[test]
    fn unthemed_paint_uses_fallback_constants() {
        let mut off = built(Freq::Weekly, Freq::Daily, true);
        let rec = paint_rec(&mut off, None);
        assert_eq!(rec.strokes, vec![ON_SURFACE_VARIANT]);

        let mut on = built(Freq::Daily, Freq::Daily, true);
        let rec = paint_rec(&mut on, None);
        assert_eq!(rec.strokes, vec![PRIMARY]);
        assert_eq!(rec.rrects[0].3, PRIMARY, "the fully-settled dot is PRIMARY");
    }

    #[test]
    fn themed_paint_resolves_roles() {
        let theme = crate::baseline();
        let scheme = theme.scheme();
        let mut off = built(Freq::Weekly, Freq::Daily, true);
        let rec = paint_rec(&mut off, Some(&theme));
        assert_eq!(rec.strokes, vec![scheme.on_surface_variant]);

        let mut on = built(Freq::Daily, Freq::Daily, true);
        let rec = paint_rec(&mut on, Some(&theme));
        assert_eq!(rec.strokes, vec![scheme.primary]);
    }

    // ---- rebuild ------------------------------------------------------------

    #[test]
    fn rebuild_adopts_new_selected_value() {
        let mut counter = 0u64;
        let prev = radio::<GroupState, Freq>(Freq::Weekly, Freq::Daily).on_changed(on_changed);
        let mut w = View::<GroupState>::build(&prev, &mut BuildCtx::new(&mut counter));
        assert!(!w.selected);
        let next = radio::<GroupState, Freq>(Freq::Weekly, Freq::Weekly).on_changed(on_changed);
        let flags =
            View::<GroupState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.selected);
        assert!(flags.needs_paint());
    }

    #[test]
    fn rebuild_toggles_enabled_state() {
        let mut counter = 0u64;
        let prev = radio::<GroupState, Freq>(Freq::Weekly, Freq::Daily);
        let mut w = View::<GroupState>::build(&prev, &mut BuildCtx::new(&mut counter));
        assert!(!w.enabled);
        let next = radio::<GroupState, Freq>(Freq::Weekly, Freq::Daily).on_changed(on_changed);
        let flags =
            View::<GroupState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.enabled);
        assert!(flags.needs_paint());
    }

    // ---- label: presence/text reconciliation -------------------------------

    #[test]
    fn label_presence_change_builds_and_tears_down_the_child() {
        let mut counter = 0u64;
        let prev = radio::<GroupState, Freq>(Freq::Weekly, Freq::Daily).on_changed(on_changed);
        let mut w = View::<GroupState>::build(&prev, &mut BuildCtx::new(&mut counter));
        assert!(w.label_pod.is_none());

        let next = radio::<GroupState, Freq>(Freq::Weekly, Freq::Daily)
            .on_changed(on_changed)
            .label("Weekly");
        let flags =
            View::<GroupState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.label_pod.is_some());
        assert_eq!(w.label_text.as_deref(), Some("Weekly"));
        assert!(flags.needs_layout());

        let prev2 = next;
        let next2 = radio::<GroupState, Freq>(Freq::Weekly, Freq::Daily).on_changed(on_changed);
        let flags =
            View::<GroupState>::rebuild(&next2, &prev2, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(w.label_pod.is_none());
        assert!(w.label_text.is_none());
        assert!(flags.needs_layout());
    }

    // ---- semantics ----------------------------------------------------------

    #[test]
    fn semantics_reports_role_label_selected_and_bounds() {
        fn logic(_s: &mut ()) -> RadioView<(), Freq> {
            radio::<(), Freq>(Freq::Daily, Freq::Daily)
                .on_changed(|_, _| {})
                .label("daily")
        }
        let mut root: frust_core::RenderRoot<(), RadioView<(), Freq>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::RadioButton)
            .expect("radio contributes a Role::RadioButton node");
        assert_eq!(node.label(), Some("daily"));
        assert_eq!(node.is_selected(), Some(true));
        assert!(node.supports_action(Action::Click));
        let bounds = node.bounds().expect("radio node has bounds");
        assert_eq!((bounds.x0, bounds.y0), (0.0, 0.0));
    }

    #[test]
    fn semantics_omits_click_action_when_disabled() {
        fn logic(_s: &mut ()) -> RadioView<(), Freq> {
            radio::<(), Freq>(Freq::Daily, Freq::Weekly)
        }
        let mut root: frust_core::RenderRoot<(), RadioView<(), Freq>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust::authoring::text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::RadioButton)
            .expect("radio contributes a Role::RadioButton node");
        assert!(!node.supports_action(Action::Click));
        assert_eq!(node.is_selected(), Some(false));
    }
}
