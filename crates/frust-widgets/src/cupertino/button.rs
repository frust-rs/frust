//! `CupertinoButton`: the iOS-27
//! capsule pressable — the Cupertino-styled counterpart to [`crate::Button`]
//! (which is Material-styled). Three kit-mined size classes
//! ([`CupertinoButtonSize`]) and three styles
//! ([`CupertinoButtonStyle`]): `Filled` (tinted accent), `Gray` (secondary
//! fill), and `Glass` (reads [`Theme`]'s glass tokens).
//!
//! [`cupertino_button`] is the declarative view-fn; it produces a
//! [`CupertinoButtonView`] carrying the label and a typed `on_press` closure,
//! which materializes into a retained [`CupertinoButtonWidget`]. Interaction
//! is fire-on-up-inside, mirroring [`crate::Button`]/[`crate::material::card`]:
//! a `Down` inside captures the pointer and starts the press-dip animation;
//! the callback fires on `Up` only if the release lands inside; `Cancel`
//! (platform gesture steal) reverses the dip without firing.
//!
//! # Size classes (kit-mined metrics)
//!
//! Source: a community-mined survey of iOS system UI kit screenshots (266
//! button variants across styles/themes/states). The text-label button's
//! frame **height** is identical
//! across the kit's `Bordered`/`Bordered Prominent` styles, both themes, and
//! the `Idle` state at a given size class — a clean, unambiguous record per
//! class:
//!
//! - **Small**: `Buttons/Dark/Small/Bordered/Text/1 - Idle` → 49×28 px →
//!   height 28.
//! - **Medium**: `Buttons/Dark/Medium/Bordered/Text/1 - Idle` → 56×34 px →
//!   height 34.
//! - **Large**: `Buttons/Dark/Large/Bordered/Text/1 - Idle` → 72×50 px →
//!   height 50.
//!
//! These three heights also match Apple's own published three-tier
//! `UIButton.Configuration.Size` convention (small/medium/large), so they are
//! treated as reliable kit citations, not merely community-approximate.
//!
//! Horizontal/vertical content padding is *derived* from the same table's
//! shared `Label` reference record (39×18 px — a single reused label frame
//! name across the kit's button-size artboards, per `component_metrics.Buttons.Label`):
//! `pad = (button_dim - label_dim) / 2`, to the nearest half px:
//!
//! | Size class | Height | `pad_x` | `pad_y` |
//! |---|---|---|---|
//! | [`CupertinoButtonSize::Small`]  | 28 | 5.0  | 5.0  |
//! | [`CupertinoButtonSize::Medium`] | 34 | 8.5  | 8.0  |
//! | [`CupertinoButtonSize::Large`]  | 50 | 16.5 | 16.0 |
//!
//! **Caveat**: the `Label` record is a single shared frame name reused across the kit's
//! Figma-exported artboards, so its 39×18 size is an indicative single-line
//! reference, not an independently-verified per-size-class measurement. The
//! three **heights** above are the load-bearing citation; the derived
//! paddings are a best-effort extrapolation from the same source rather than
//! a second independently-sourced figure.
//!
//! # Styles
//!
//! * [`CupertinoButtonStyle::Filled`]: background `colors.primary` (iOS
//!   systemBlue), label [`ThemeTextColor::OnPrimary`] — mirrors
//!   `.borderedProminent`.
//! * [`CupertinoButtonStyle::Gray`]: background `colors.secondary_container`
//!   (iOS `secondarySystemBackground`-ish light-gray fill), label
//!   [`ThemeTextColor::OnSurface`] (identical to `on_secondary_container` in
//!   both `ColorScheme::cupertino_light`/`cupertino_dark` — see
//!   `frust-theme::color`'s Cupertino mapping) — mirrors `.bordered`.
//! * [`CupertinoButtonStyle::Glass`]: reads `Theme::glass.control` (kit
//!   `radius 15` no-fill tier — see
//!   `frust-theme::glass`'s module docs). On a translucent (iOS-27) glass
//!   material this paints only that tier's `fills_light`/`fills_dark` washes
//!   (empty for `control` — a pure lens, per the token's own docs — so
//!   nothing but a specular hairline and a soft drop shadow renders; real
//!   backdrop blur is not yet implemented, so "translucent" here
//!   means alpha-composited washes, not a blurred backdrop). On an opaque
//!   ([`GlassMaterial::is_opaque`]) glass material — the Material design
//!   language's degrade path via
//!   [`frust_theme::GlassScale::opaque_material`] — it paints
//!   an opaque `colors.surface_container_highest` fill instead and skips the
//!   hairline (`hairline_alpha` is `0.0` on that path), so one widget API
//!   renders correctly under either design language.
//!
//! # Press feedback
//!
//! A spring-driven `AnimationController` ([`AnimationController::fling`],
//! [`Theme::cupertino_baseline`]'s `motion.default_spatial` — the same
//! Cupertino-idiom precedent [`crate::cupertino::switch`] uses) drives a
//! `0.0` (rest) → `1.0` (pressed) dip fraction, started on `Down`/reversed on
//! `Up`/`Cancel`. At full press this shrinks the painted capsule by
//! [`PRESS_SCALE_DIP`] and composites the whole widget (background + label)
//! at `1.0 - `[`PRESS_OPACITY_DIP`]` * t` opacity via
//! [`frust_core::PaintScene::push_layer`] — the "scale + opacity dip"
//! `Down`-to-`Up` press feedback iOS controls use, with no exact Apple-
//! published dip magnitude (both constants are community-approximate; see
//! their doc comments).

use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::{Action, Role};
use frust_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, SpringDesc, View,
    Widget, any,
};
use frust_theme::{Brightness, GlassMaterial, ShadowSpec, ShapeScale, Theme};
use kurbo::{Point, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use crate::authoring::ThemeTextColor;
use crate::text;

/// Small size-class content height, logical px. Source: `Buttons/Dark/Small/
/// Bordered/Text/1 - Idle` (49×28) — see the module docs' size-class table.
const SMALL_HEIGHT: f64 = 28.0;
/// Medium size-class content height. Source: `Buttons/Dark/Medium/Bordered/
/// Text/1 - Idle` (56×34).
const MEDIUM_HEIGHT: f64 = 34.0;
/// Large size-class content height. Source: `Buttons/Dark/Large/Bordered/
/// Text/1 - Idle` (72×50).
const LARGE_HEIGHT: f64 = 50.0;

/// Small size-class horizontal content padding: `(49 - 39) / 2`. See the
/// module docs' derivation and caveat.
const SMALL_PAD_X: f64 = 5.0;
/// Small size-class vertical content padding: `(28 - 18) / 2`.
const SMALL_PAD_Y: f64 = 5.0;
/// Medium size-class horizontal content padding: `(56 - 39) / 2`.
const MEDIUM_PAD_X: f64 = 8.5;
/// Medium size-class vertical content padding: `(34 - 18) / 2`.
const MEDIUM_PAD_Y: f64 = 8.0;
/// Large size-class horizontal content padding: `(72 - 39) / 2`.
const LARGE_PAD_X: f64 = 16.5;
/// Large size-class vertical content padding: `(50 - 18) / 2`.
const LARGE_PAD_Y: f64 = 16.0;

/// Fill for [`CupertinoButtonStyle::Filled`]'s unthemed fallback — matches
/// `ColorScheme::cupertino_light().primary` (systemBlue) exactly, duplicated
/// here per `docs/CODE_STANDARDS.md`'s unthemed-fallback-constant convention
/// (mirrors [`crate::Button`]'s `FILL`).
const FILLED_FALLBACK: Color = Color::from_rgb8(0x00, 0x87, 0xFF);
/// Fill for [`CupertinoButtonStyle::Gray`]'s unthemed fallback — matches
/// `ColorScheme::cupertino_light().secondary_container` exactly.
const GRAY_FALLBACK: Color = Color::from_rgb8(0xF2, 0xF2, 0xF7);
// Note on [`CupertinoButtonStyle::Glass`]'s *opaque-material* degrade path
// (Material design language, no fill washes to read): a themed opaque paint
// always resolves `colors.surface_container_highest` directly (see
// [`CupertinoButtonWidget::paint`]'s `Glass` arm) rather than reusing
// [`GRAY_FALLBACK`]; only the *unthemed* `Glass` fallback ships the
// Cupertino (iOS-27, translucent) lens instead, mirroring every other
// `cupertino::*` widget's unthemed-fallback convention — see
// [`fallback_control_glass`].

/// Hairline stroke width, in logical px (matches
/// [`crate::material::card`]'s outlined-variant stroke width).
const HAIRLINE_WIDTH: f64 = 1.0;
/// Tessellation tolerance for the rounded-rect hairline path (matches
/// [`crate::material::card`]'s `STROKE_TOLERANCE`).
const STROKE_TOLERANCE: f64 = 0.1;

/// Community-approximate iOS press-dip scale factor — the fraction the
/// capsule shrinks by at full press (`t == 1.0`). Apple does not publish an
/// exact press-highlight scale; this value keeps the dip visible without
/// distorting the capsule noticeably.
const PRESS_SCALE_DIP: f64 = 0.04;
/// Community-approximate iOS press-dip opacity reduction — the fraction of
/// alpha removed at full press (`t == 1.0`), applied to the whole widget via
/// [`frust_core::PaintScene::push_layer`]. Apple does not publish an exact
/// press-highlight alpha; community `UIControl`/SwiftUI `.buttonStyle`
/// reimplementations commonly dim a pressed control to roughly this range.
const PRESS_OPACITY_DIP: f64 = 0.35;

/// Nominal period seeding the press [`AnimationController`]'s clock; the
/// motion is spring-driven (via `fling`), so this duration is never actually
/// consulted (mirrors [`crate::material::button_group`]'s `PRESS_ANIM_PERIOD`
/// of the same shape).
const PRESS_ANIM_PERIOD: Duration = Duration::from_millis(200);

/// A sub-visible fling launch velocity — only its sign matters (selects which
/// end, `0.0`/`1.0`, the spring targets). Mirrors
/// [`crate::cupertino::switch`]'s `RELEASE_VELOCITY`.
const FLING_VELOCITY: f64 = 1e-3;

/// The press-dip spring: matches [`Theme::cupertino_baseline`]'s
/// `motion.default_spatial` exactly (ζ ≈ 0.5753, stiffness 170).
///
/// Used **unconditionally** (never re-resolved from a live theme), for two
/// reasons: the press dip starts in [`CupertinoButtonWidget::event`], which —
/// unlike `paint` — carries no `Theme` at all
/// (`docs/CODE_STANDARDS.md`'s Theming conventions say `EventCtx` threads no
/// theme), the same reason
/// [`crate::material::button_group`]'s `PRESS_SPRING` is a hardcoded constant
/// rather than a paint-time theme read; and `MotionScheme::cupertino()` (see
/// `frust-theme::motion`'s module docs) applies this exact spring
/// uniformly to all six of its slots, so a themed Cupertino baseline would
/// resolve to the identical value anyway. A Material-language theme's
/// differing spring is intentionally not reflected in this Cupertino-family
/// widget's press feedback — a documented simplification, not a bug.
const PRESS_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: 170.0,
    damping_ratio: 0.5753,
};

/// One of the kit-mined size classes (see the module docs' table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CupertinoButtonSize {
    Small,
    #[default]
    Medium,
    Large,
}

/// A size class's resolved `(height, pad_x, pad_y)` metrics, in logical px.
struct SizeMetrics {
    height: f64,
    pad_x: f64,
    pad_y: f64,
}

impl CupertinoButtonSize {
    /// Resolve this size class's metrics (see the module docs' table).
    fn metrics(self) -> SizeMetrics {
        match self {
            CupertinoButtonSize::Small => SizeMetrics {
                height: SMALL_HEIGHT,
                pad_x: SMALL_PAD_X,
                pad_y: SMALL_PAD_Y,
            },
            CupertinoButtonSize::Medium => SizeMetrics {
                height: MEDIUM_HEIGHT,
                pad_x: MEDIUM_PAD_X,
                pad_y: MEDIUM_PAD_Y,
            },
            CupertinoButtonSize::Large => SizeMetrics {
                height: LARGE_HEIGHT,
                pad_x: LARGE_PAD_X,
                pad_y: LARGE_PAD_Y,
            },
        }
    }
}

/// One of the three button styles (see the module docs' Styles section).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CupertinoButtonStyle {
    #[default]
    Filled,
    Gray,
    Glass,
}

/// The unthemed-fallback glass-control material for [`CupertinoButtonStyle::Glass`],
/// matching `GlassScale::ios27().control` exactly — duplicated
/// here rather than constructed via `GlassScale::ios27()` so this module has
/// no runtime dependency on that constructor's shape, mirroring every other
/// `cupertino::*` widget's unthemed-fallback-constant convention. A
/// pre-theme app therefore renders the same pure-lens control an app on the
/// `Theme::cupertino_baseline()` would.
fn fallback_control_glass() -> GlassMaterial {
    GlassMaterial {
        blur_radius_intent: 15.0,
        fills_light: Vec::new(),
        fills_dark: Vec::new(),
        hairline_alpha: 0.3,
        shadow: ShadowSpec {
            y_offset: 2.0,
            blur_std_dev: 4.0,
            color_alpha: 0.12,
        },
    }
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`crate::material::card`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The themed label-role + resting fill fallback for `style`.
fn label_role(style: CupertinoButtonStyle) -> ThemeTextColor {
    match style {
        CupertinoButtonStyle::Filled => ThemeTextColor::OnPrimary,
        CupertinoButtonStyle::Gray | CupertinoButtonStyle::Glass => ThemeTextColor::OnSurface,
    }
}

/// Whether a widget-local `pos` lies within a `size`-sized box anchored at the
/// origin (mirrors [`crate::Button`]'s helper of the same shape).
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// A view-held, typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// Build the type-erased label view, tagged with the style's themed color
/// role (see [`label_role`]). Shared by build/rebuild/teardown so the role
/// stays consistent across the child's whole lifecycle.
fn label_view<State: 'static>(
    label: String,
    style: CupertinoButtonStyle,
) -> frust_core::AnyView<State> {
    any::<State, _>(text(label).themed_role(label_role(style)))
}

/// A declarative iOS capsule/glass button. See the [module docs](self).
pub struct CupertinoButtonView<State: 'static> {
    label: String,
    size: CupertinoButtonSize,
    style: CupertinoButtonStyle,
    on_press: OnPress<State>,
}

/// Create an iOS capsule button labelled `label` (default
/// [`CupertinoButtonSize::Medium`], [`CupertinoButtonStyle::Filled`]) that
/// runs `on_press` against the app state when released inside its bounds.
pub fn cupertino_button<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> CupertinoButtonView<State> {
    CupertinoButtonView {
        label: label.into(),
        size: CupertinoButtonSize::default(),
        style: CupertinoButtonStyle::default(),
        on_press: Rc::new(on_press),
    }
}

/// PascalCase alias for [`cupertino_button`], matching
/// [`crate::cupertino::switch::CupertinoSwitch`]'s alias convention.
#[allow(non_snake_case)]
pub fn CupertinoButton<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> CupertinoButtonView<State> {
    cupertino_button(label, on_press)
}

impl<State: 'static> CupertinoButtonView<State> {
    /// Set the size class (default [`CupertinoButtonSize::Medium`]).
    pub fn size(mut self, size: CupertinoButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Set the style (default [`CupertinoButtonStyle::Filled`]).
    pub fn style(mut self, style: CupertinoButtonStyle) -> Self {
        self.style = style;
        self
    }
}

/// The retained widget for a [`CupertinoButtonView`].
pub struct CupertinoButtonWidget {
    label: ChildPod,
    /// The label text, retained for the semantics node's accessible name (see
    /// [`crate::Button`]'s field of the same shape/rationale).
    label_text: String,
    size: CupertinoButtonSize,
    style: CupertinoButtonStyle,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    captured: bool,
    /// The spring-driven press dip: `0.0` rest → `1.0` fully pressed.
    press_anim: AnimationController,
    on_press: crate::authoring::ErasedCallback,
}

impl CupertinoButtonWidget {
    /// Start the press-in dip, using [`PRESS_SPRING`] (see its doc comment for
    /// why this is never theme-resolved).
    fn press_in(&mut self) {
        self.press_anim.fling(FLING_VELOCITY, PRESS_SPRING);
    }

    /// Start the press-out (release) dip.
    fn press_out(&mut self) {
        self.press_anim.fling(-FLING_VELOCITY, PRESS_SPRING);
    }
}

impl<State: 'static> View<State> for CupertinoButtonView<State> {
    type Element = CupertinoButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CupertinoButtonWidget {
        let label_view = label_view::<State>(self.label.clone(), self.style);
        CupertinoButtonWidget {
            label: crate::authoring::build_child(&label_view, ctx),
            label_text: self.label.clone(),
            size: self.size,
            style: self.style,
            captured: false,
            press_anim: AnimationController::new(PRESS_ANIM_PERIOD),
            on_press: crate::authoring::erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CupertinoButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable — always reinstall the adapter.
        element.on_press = crate::authoring::erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;

        if prev.label != self.label || prev.style != self.style {
            element.label_text = self.label.clone();
            element.style = self.style;
            let prev_view = label_view::<State>(prev.label.clone(), prev.style);
            let next_view = label_view::<State>(self.label.clone(), self.style);
            flags |=
                crate::authoring::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut CupertinoButtonWidget, ctx: &mut BuildCtx<'_>) {
        let label_view = label_view::<State>(self.label.clone(), self.style);
        crate::authoring::teardown_child(&label_view, &mut element.label, ctx);
    }
}

impl Widget for CupertinoButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let metrics = self.size.metrics();
        let inner_max = Size::new(
            (bc.max().width - metrics.pad_x * 2.0).max(0.0),
            (metrics.height - metrics.pad_y * 2.0).max(0.0),
        );
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        self.label.set_origin(Point::new(
            metrics.pad_x,
            (metrics.height - label_size.height) / 2.0,
        ));
        bc.constrain(Size::new(
            label_size.width + metrics.pad_x * 2.0,
            metrics.height,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Advance/request_frame need `&mut ctx`; run them before taking the
        // immutable `theme`/`origin`/`size` borrows below (an interleaved
        // mutable call would otherwise conflict with `theme`'s live range —
        // see `crate::cupertino::switch`'s paint for the same ordering).
        if self.press_anim.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        let t = self.press_anim.value_clamped();

        let theme = Theme::from_paint_ctx(ctx);
        let o = ctx.origin();
        let size = ctx.size();
        let radius = theme
            .map(|theme| ShapeScale::resolve(theme.shape.full, size.width, size.height))
            .unwrap_or(size.height / 2.0);

        // Scale dip: shrink the painted capsule around its center by up to
        // PRESS_SCALE_DIP at full press.
        let scale = 1.0 - PRESS_SCALE_DIP * t;
        let inset_x = size.width * (1.0 - scale) / 2.0;
        let inset_y = size.height * (1.0 - scale) / 2.0;
        let draw_origin = Point::new(o.x + inset_x, o.y + inset_y);
        let draw_size = Size::new(
            (size.width - 2.0 * inset_x).max(0.0),
            (size.height - 2.0 * inset_y).max(0.0),
        );
        let draw_radius = (radius * scale).max(0.0);

        // Opacity dip: composite the whole widget (background + label) at
        // reduced alpha while pressed.
        let layer_alpha = (1.0 - PRESS_OPACITY_DIP * t) as f32;
        scene.push_layer(o, size, layer_alpha);

        match self.style {
            CupertinoButtonStyle::Filled => {
                let fill = theme.map(|t| t.scheme().primary).unwrap_or(FILLED_FALLBACK);
                scene.fill_rounded_rect(draw_origin, draw_size, draw_radius, fill);
            }
            CupertinoButtonStyle::Gray => {
                let fill = theme
                    .map(|t| t.scheme().secondary_container)
                    .unwrap_or(GRAY_FALLBACK);
                scene.fill_rounded_rect(draw_origin, draw_size, draw_radius, fill);
            }
            CupertinoButtonStyle::Glass => {
                let material = theme
                    .map(|t| t.glass.control.clone())
                    .unwrap_or_else(fallback_control_glass);
                if material.is_opaque() {
                    // Material design-language degrade path (GlassScale::opaque_material).
                    let fill = theme
                        .map(|t| t.scheme().surface_container_highest)
                        .unwrap_or(GRAY_FALLBACK);
                    let shadow_color = theme
                        .map(|t| with_alpha(t.scheme().shadow, material.shadow.color_alpha))
                        .unwrap_or_else(|| {
                            with_alpha(Color::from_rgb8(0, 0, 0), material.shadow.color_alpha)
                        });
                    scene.draw_shadow(
                        Point::new(draw_origin.x, draw_origin.y + material.shadow.y_offset),
                        draw_size,
                        draw_radius,
                        material.shadow.blur_std_dev,
                        shadow_color,
                    );
                    scene.fill_rounded_rect(draw_origin, draw_size, draw_radius, fill);
                } else {
                    // The iOS-27 translucent lens: composite this brightness's
                    // fill washes (empty for `control` — a pure lens, see the
                    // module docs), then the drop shadow, then the specular
                    // hairline.
                    let brightness = theme.map(|t| t.brightness).unwrap_or(Brightness::Light);
                    let washes = match brightness {
                        Brightness::Light => &material.fills_light,
                        Brightness::Dark => &material.fills_dark,
                    };
                    for wash in washes {
                        scene.fill_rounded_rect(draw_origin, draw_size, draw_radius, wash.color);
                    }
                    let shadow_color =
                        with_alpha(Color::from_rgb8(0, 0, 0), material.shadow.color_alpha);
                    scene.draw_shadow(
                        Point::new(draw_origin.x, draw_origin.y + material.shadow.y_offset),
                        draw_size,
                        draw_radius,
                        material.shadow.blur_std_dev,
                        shadow_color,
                    );
                    if material.hairline_alpha > 0.0 {
                        let half = HAIRLINE_WIDTH / 2.0;
                        let rr = RoundedRect::new(
                            half,
                            half,
                            draw_size.width - half,
                            draw_size.height - half,
                            (draw_radius - half).max(0.0),
                        );
                        let path = rr.to_path(STROKE_TOLERANCE);
                        let hairline = Color::new([1.0, 1.0, 1.0, material.hairline_alpha]);
                        scene.stroke_path(
                            draw_origin,
                            &path,
                            HAIRLINE_WIDTH,
                            &Brush::Solid(hairline),
                        );
                    }
                }
            }
        }

        self.label.paint_child(ctx, scene);
        scene.pop_layer();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                self.captured = true;
                self.press_in();
                ctx.capture_pointer();
                ctx.request_redraw();
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
                    (self.on_press)(ctx);
                }
                self.captured = false;
                self.press_out();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                self.press_out();
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label_text.as_str());
            node.add_action(Action::Click);
        });
    }

    crate::authoring::visit_children!(label);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{FrameTime, RenderRoot};
    use frust_text::TextContext;
    use std::any::Any;

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    fn widget_with(
        size: CupertinoButtonSize,
        style: CupertinoButtonStyle,
    ) -> CupertinoButtonWidget {
        let view = cupertino_button::<Counter, _>("Go", |s: &mut Counter| s.presses += 1)
            .size(size)
            .style(style);
        let mut counter = 0u64;
        View::<Counter>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn widget() -> CupertinoButtonWidget {
        widget_with(
            CupertinoButtonSize::default(),
            CupertinoButtonStyle::default(),
        )
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut CupertinoButtonWidget, state: &mut Counter, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(80.0, 34.0));
        w.event(&mut ctx, event);
    }

    fn layout(w: &mut CupertinoButtonWidget, max_w: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(max_w, 200.0)))
    }

    // --- Size classes produce distinct expected metrics ---

    #[test]
    fn three_size_classes_produce_distinct_heights() {
        let mut small = widget_with(CupertinoButtonSize::Small, CupertinoButtonStyle::Filled);
        let mut medium = widget_with(CupertinoButtonSize::Medium, CupertinoButtonStyle::Filled);
        let mut large = widget_with(CupertinoButtonSize::Large, CupertinoButtonStyle::Filled);

        assert_eq!(layout(&mut small, 400.0).height, SMALL_HEIGHT);
        assert_eq!(layout(&mut medium, 400.0).height, MEDIUM_HEIGHT);
        assert_eq!(layout(&mut large, 400.0).height, LARGE_HEIGHT);
    }

    #[test]
    fn size_classes_expose_distinct_padding_metrics() {
        let small = CupertinoButtonSize::Small.metrics();
        let medium = CupertinoButtonSize::Medium.metrics();
        let large = CupertinoButtonSize::Large.metrics();

        assert_eq!((small.height, small.pad_x, small.pad_y), (28.0, 5.0, 5.0));
        assert_eq!(
            (medium.height, medium.pad_x, medium.pad_y),
            (34.0, 8.5, 8.0)
        );
        assert_eq!((large.height, large.pad_x, large.pad_y), (50.0, 16.5, 16.0));
        // Distinct, monotonically increasing across the three classes.
        assert!(small.height < medium.height && medium.height < large.height);
        assert!(small.pad_x < medium.pad_x && medium.pad_x < large.pad_x);
    }

    #[test]
    fn width_grows_to_wrap_the_label_plus_padding() {
        let mut short = widget_with(CupertinoButtonSize::Small, CupertinoButtonStyle::Filled);
        let short_size = layout(&mut short, 400.0);
        // Width is always at least the two-sided horizontal padding...
        assert!(short_size.width >= SMALL_PAD_X * 2.0);

        // ...and a longer label grows the button wider, proving it wraps the
        // label's real shaped width rather than collapsing to a fixed size.
        let view = cupertino_button::<Counter, _>("A much longer label", |_s: &mut Counter| {})
            .size(CupertinoButtonSize::Small);
        let mut counter = 0u64;
        let mut long = View::<Counter>::build(&view, &mut BuildCtx::new(&mut counter));
        let long_size = layout(&mut long, 400.0);
        assert!(
            long_size.width > short_size.width,
            "a longer label must widen the capsule"
        );
    }

    // --- Glass style reads the control tier ---

    /// Records each rounded rect's `(size, radius, color)`, each `stroke_path`
    /// call's `(width, color, alpha)`, and each `draw_shadow` call, in paint
    /// order.
    #[derive(Default)]
    struct PaintRecorder {
        rrects: Vec<(Size, f64, Color)>,
        strokes: Vec<(f64, Color)>,
        shadows: Vec<(f64, f64, Color)>,
        layers: Vec<f32>,
    }

    impl PaintScene for PaintRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((s, radius, color));
        }
        fn stroke_path(
            &mut self,
            _origin: Point,
            _path: &kurbo::BezPath,
            width: f64,
            brush: &Brush,
        ) {
            if let Brush::Solid(color) = brush {
                self.strokes.push((width, *color));
            }
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((radius, std_dev, color));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
    }

    fn paint_rec(w: &mut CupertinoButtonWidget, theme: Option<&Theme>) -> PaintRecorder {
        let mut rec = PaintRecorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, Size::new(80.0, 34.0)).with_theme(t),
            None => PaintCtx::new(Point::ZERO, Size::new(80.0, 34.0)),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn glass_style_reads_the_control_tier_translucent_on_cupertino() {
        let theme = Theme::cupertino_baseline();
        assert!(!theme.glass.control.is_opaque());
        let mut w = widget_with(CupertinoButtonSize::Medium, CupertinoButtonStyle::Glass);
        let rec = paint_rec(&mut w, Some(&theme));

        // `control` is a pure lens (empty fill washes) — no rounded-rect fill.
        assert!(
            rec.rrects.is_empty(),
            "control tier paints no fill washes (pure lens)"
        );
        // The specular hairline is drawn at the tier's hairline_alpha.
        assert_eq!(rec.strokes.len(), 1);
        assert_eq!(
            rec.strokes[0].1.components[3],
            theme.glass.control.hairline_alpha
        );
        // The tier's drop shadow is drawn.
        assert_eq!(rec.shadows.len(), 1);
        assert_eq!(rec.shadows[0].1, theme.glass.control.shadow.blur_std_dev);
    }

    #[test]
    fn glass_style_degrades_to_opaque_fill_on_material() {
        let theme = Theme::m3_baseline();
        assert!(theme.glass.control.is_opaque());
        let mut w = widget_with(CupertinoButtonSize::Medium, CupertinoButtonStyle::Glass);
        let rec = paint_rec(&mut w, Some(&theme));

        // Opaque path: exactly one fill, matching surface_container_highest.
        assert_eq!(rec.rrects.len(), 1);
        assert_eq!(rec.rrects[0].2, theme.scheme().surface_container_highest);
        // No hairline on the opaque path (hairline_alpha == 0.0).
        assert!(rec.strokes.is_empty());
        assert_eq!(rec.shadows.len(), 1);
    }

    #[test]
    fn glass_style_unthemed_fallback_matches_ios27_control() {
        let mut w = widget_with(CupertinoButtonSize::Medium, CupertinoButtonStyle::Glass);
        let rec = paint_rec(&mut w, None);
        let fallback = fallback_control_glass();
        assert!(rec.rrects.is_empty());
        assert_eq!(rec.strokes.len(), 1);
        assert_eq!(rec.strokes[0].1.components[3], fallback.hairline_alpha);
        assert_eq!(rec.shadows[0].1, fallback.shadow.blur_std_dev);
    }

    #[test]
    fn filled_and_gray_styles_never_touch_glass() {
        // Filled/Gray paint exactly one fill and no hairline/shadow, regardless
        // of theme.
        let theme = Theme::cupertino_baseline();
        let mut filled = widget_with(CupertinoButtonSize::Medium, CupertinoButtonStyle::Filled);
        let rec = paint_rec(&mut filled, Some(&theme));
        assert_eq!(rec.rrects.len(), 1);
        assert_eq!(rec.rrects[0].2, theme.scheme().primary);
        assert!(rec.strokes.is_empty());
        assert!(rec.shadows.is_empty());

        let mut gray = widget_with(CupertinoButtonSize::Medium, CupertinoButtonStyle::Gray);
        let rec = paint_rec(&mut gray, Some(&theme));
        assert_eq!(rec.rrects.len(), 1);
        assert_eq!(rec.rrects[0].2, theme.scheme().secondary_container);
        assert!(rec.strokes.is_empty());
        assert!(rec.shadows.is_empty());
    }

    #[test]
    fn unthemed_filled_uses_fallback_constant() {
        let mut w = widget_with(CupertinoButtonSize::Medium, CupertinoButtonStyle::Filled);
        let rec = paint_rec(&mut w, None);
        assert_eq!(rec.rrects[0].2, FILLED_FALLBACK);
    }

    // --- Press state transitions ---

    #[test]
    fn down_then_up_inside_fires_once() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.captured);
        assert!(w.press_anim.is_animating(), "Down starts the press-in dip");
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.presses, 1);
        assert!(!w.captured);
    }

    #[test]
    fn up_outside_does_not_fire_but_still_reverses_dip() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 500.0));
        assert_eq!(state.presses, 0, "up outside must not fire");
        assert!(
            w.press_anim.is_animating(),
            "Up still starts the press-out dip"
        );
    }

    #[test]
    fn cancel_reverses_the_dip_without_firing() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 10.0, 10.0));
        assert!(!w.captured);
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn hover_move_without_down_is_ignored() {
        let mut w = widget();
        let mut state = Counter::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(80.0, 34.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 20.0, 20.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.captured);
    }

    #[test]
    fn press_dip_settles_back_to_rest_after_release() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0));

        let mut running = true;
        let mut t = 0.0;
        for _ in 0..100_000 {
            running = w.press_anim.advance(ft_secs(t));
            if !running {
                break;
            }
            t += 1.0 / 120.0;
        }
        assert!(!running, "press-out fling failed to settle");
        assert!((w.press_anim.value() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn press_advance_requests_another_frame_while_animating() {
        let mut w = widget();
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(80.0, 34.0));
        let mut rec = PaintRecorder::default();
        w.paint(&mut ctx, &mut rec);
        assert!(
            ctx.needs_frame(),
            "an in-flight press dip requests another frame"
        );
    }

    // --- Facade-shape smoke test (semantics + full render root pass) ---

    #[test]
    fn semantics_reports_button_role_and_label() {
        fn logic(_state: &mut Counter) -> CupertinoButtonView<Counter> {
            cupertino_button::<Counter, _>("Continue", |_s| {})
        }
        let mut root: RenderRoot<Counter, CupertinoButtonView<Counter>> = RenderRoot::new();
        let mut state = Counter::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("button contributes a Role::Button node");
        assert_eq!(node.label(), Some("Continue"));
        assert!(node.supports_action(Action::Click));
    }
}
