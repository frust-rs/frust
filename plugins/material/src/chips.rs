//! M3 `Chip`: four types (assist, filter, input, suggestion), an `elevated`
//! modifier, and a `selected` state — a 32dp height, 8dp corner radius
//! pill-shaped control with the shared [`super::state_layer`] interaction
//! overlay.
//!
//! # Attribution
//!
//! Ported from `material_3_expressive` v1.0.8 (MIT, © 2026 Paa Developments)
//! — the files ported here are `lib/components/chips/m3e_chips.dart`,
//! `enums/m3e_chip_type.dart`, and `styles/m3e_chip_theme.dart`. No license
//! changes; the container/foreground color table, the padding/icon-size
//! constants, and the `elevated` shadow are ported verbatim from
//! `M3EChipTheme`. `M3EChip` is a single widget carrying a `type` enum field
//! — this port keeps this crate's existing one-function-per-type shape
//! instead (see *Four types, one color table* below).
//!
//! # Four types, one color table
//!
//! [`assist_chip`], [`filter_chip`], [`input_chip`], and [`suggestion_chip`]
//! each build a dedicated `View`/`Widget` pair, following this crate's
//! established per-type convention rather than the reference's single
//! `M3EChip` + `M3EChipType` enum shape — but they all resolve their
//! container/foreground colors through the same two functions
//! ([`resolve_chip_container`]/[`resolve_chip_foreground`]), because
//! `M3EChipTheme.containerColor` accepts a `type` parameter but never reads
//! it in its body: every type paints identically at a given `(selected,
//! elevated)` state in the reference (confirmed against its
//! `chips_playground.dart` example, which never varies a chip's color by
//! type). What *does* vary by type is purely behavioral, per that same
//! example:
//!
//! - **[`assist_chip`]**/**[`suggestion_chip`]**: action-only — a plain
//!   `on_press`, optional leading glyph, never selectable, never deletable.
//! - **[`filter_chip`]**: the only type exposing `selected` — a
//!   **controlled component** mirroring [`frust::Checkbox`]/[`mod@super::switch`]:
//!   it reports the requested value through `on_select` and never flips its
//!   own field.
//! - **[`input_chip`]**: the only type exposing
//!   [`InputChipView::on_deleted`] — a trailing 18dp delete icon
//!   ([`crate::icons::CLOSE`]) with its **own** hit region, firing
//!   `on_deleted` independently of the chip's own `on_press` (mirrors
//!   [`mod@super::split_button`]'s two-target dispatch). Painted directly
//!   from its resolved [`frust::IconData`] path rather than as a nested
//!   [`frust::icon`] child, since the icon's color must track this module's
//!   own `(selected, elevated)`-derived foreground, resolved at paint time —
//!   after a nested `IconView`'s own build-time color would already be
//!   fixed.
//!
//! # Elevated modifier
//!
//! `AssistChipView::elevated`/`FilterChipView::elevated`/
//! `InputChipView::elevated`/`SuggestionChipView::elevated` (default
//! `false`) — M3 elevation level 1: `surface_container_low` fill plus a
//! themed shadow, painted via [`frust::authoring::PaintScene::draw_shadow`]
//! (the same primitive [`mod@super::card`]'s elevated variant uses).
//!
//! # Container/foreground/outline matrix
//!
//! Ported verbatim from `M3EChipTheme.containerColor`/`.foregroundColor`
//! (this catalog's chips have no disabled state, so the reference's
//! `!enabled` branch is not modeled):
//!
//! | `selected` | `elevated` | container                | outlined?                          | foreground              |
//! |------------|------------|---------------------------|-------------------------------------|--------------------------|
//! | `true`     | either     | `secondary_container`     | no                                   | `on_secondary_container` (exact; the nested label text approximates it, see below) |
//! | `false`    | `true`     | `surface_container_low`   | no                                   | `on_surface_variant`     |
//! | `false`    | `false`    | transparent                | **yes** (1dp `outline_variant`)     | `on_surface_variant`     |
//!
//! `outlined = !selected && !elevated` exactly, and the elevation shadow is
//! gated on `elevated` alone — a selected **and** elevated chip shows both
//! the `secondary_container` fill and the shadow. This corrects this
//! module's original implementation, which painted every unselected chip
//! (assist always, filter when unselected) with a flat
//! `surface_container_low` fill regardless of `elevated` — a v1
//! simplification predating this port; a fully unelevated, unselected chip
//! is now transparent-filled and outlined, matching the reference exactly.
//!
//! # Label color simplification
//!
//! Every chip's label paints through a nested [`frust::text`] run, themed
//! [`ThemeTextColor::OnSurfaceVariant`] (an unselected chip — now exact,
//! matching the table above) or [`ThemeTextColor::OnSurface`] (a selected
//! chip — `TextView` has no `OnSecondaryContainer` role yet, so this
//! remains a documented v1 approximation, tonally close to
//! `on_secondary_container` in the M3 baseline palette; carried over
//! unchanged from this module's original filter-chip implementation). A
//! leading glyph shares the same role as the label (rendered as a second
//! nested text run, not a dedicated `Icon` widget — matching this module's
//! original leading-glyph choice; the delete icon above is the one
//! affordance that does need a real vector icon).
//!
//! # Selected state per type
//!
//! Only [`filter_chip`] exposes `selected` — confirmed against the
//! reference's `chips_playground.dart`, which only ever passes `selected:
//! true` for `M3EChipType.filter` (`selected: type == M3EChipType.filter &&
//! _selected`); assist/input/suggestion always render unselected. This is a
//! behavioral fact read off the reference's own example, not an
//! extrapolation — see this module's `resolution_matrix` tests for the full
//! pinned table.

use std::rc::Rc;

use frust::authoring::{Action, Role, Toggled};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust::authoring::{ThemeTextColor, ThemeTextType};
use frust::{IconData, ShapeScale, Theme};
use kurbo::{Affine, Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use super::icons;
use super::press::presses;
use super::state_layer::StateLayer;

/// Fixed chip height, in logical px (`M3EChipTheme.height`).
const CHIP_HEIGHT: f64 = 32.0;
/// Delete icon side length, in logical px (`M3EChipTheme.iconSize`).
const CHIP_ICON_SIZE: f64 = 18.0;
/// Left padding when there is no leading glyph (`M3EChipTheme.labelStartPadding`).
const CHIP_LABEL_START_PAD: f64 = 16.0;
/// Left padding when there is a leading glyph (`M3EChipTheme.iconStartPadding`).
const CHIP_ICON_START_PAD: f64 = 8.0;
/// Right padding, always (`M3EChipTheme.endPadding`) — distinct from the
/// left-side padding above (this module's original implementation used the
/// same 16dp value on both sides; the reference's right inset is 12dp).
const CHIP_END_PAD: f64 = 12.0;
/// Gap between a leading glyph and the label, and between the label and a
/// trailing delete icon (`M3EChipTheme.iconLabelGap`).
const CHIP_ICON_LABEL_GAP: f64 = 8.0;
/// Corner radius (unthemed fallback; a theme resolves this from
/// `shape.small`, the same 8dp token in the M3 baseline scale).
const CHIP_RADIUS: f64 = 8.0;
/// Outline stroke width, in logical px (Flutter `BorderSide` default).
const CHIP_STROKE_WIDTH: f64 = 1.0;
/// Flattening tolerance for the outlined variant's rounded-rect stroke path
/// (matches [`super::card`]'s `STROKE_TOLERANCE`).
const CHIP_STROKE_TOLERANCE: f64 = 0.1;

/// Unthemed-fallback selected-chip container fill (a theme resolves this
/// from `colors.secondary_container`).
const CHIP_SELECTED_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback elevated (unselected) chip container fill (a theme
/// resolves this from `colors.surface_container_low`).
const CHIP_ELEVATED_CONTAINER: Color = Color::from_rgb8(0xF7, 0xF2, 0xFA);
/// Unthemed-fallback outline stroke color (a theme resolves this from
/// `colors.outline_variant`).
const CHIP_OUTLINE_VARIANT: Color = Color::from_rgb8(0xCA, 0xC4, 0xD0);
/// Unthemed-fallback selected-chip label/foreground color (a theme resolves
/// this from `colors.on_secondary_container` exactly — this is the *raw
/// paint color* resolver, used for the state layer, the delete icon fill,
/// and as the nested `TextView` label's approximation target; only the
/// label itself approximates it, since `TextView` has no
/// `OnSecondaryContainer` role — see the module docs' label-color
/// simplification).
const CHIP_LABEL_SELECTED: Color = Color::from_rgb8(0x1D, 0x19, 0x2B);
/// Unthemed-fallback unselected-chip label/foreground color (a theme
/// resolves this from `colors.on_surface_variant` exactly).
const CHIP_LABEL_UNSELECTED: Color = Color::from_rgb8(0x49, 0x45, 0x4F);

/// Unthemed-fallback shadow y-offset, matching
/// `crate::tokens::elevation().level1`'s `y_offset` exactly (`dp / 2.0 + 1.0`
/// at `dp = 1.0`).
const CHIP_SHADOW_Y_OFFSET: f64 = 1.5;
/// Unthemed-fallback shadow blur std-dev, matching
/// `crate::tokens::elevation().level1`.
const CHIP_SHADOW_BLUR: f64 = 1.0;
/// Unthemed-fallback shadow color (opaque black at
/// `crate::tokens::elevation().level1`'s `0.3` alpha).
const CHIP_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`super::card`]'s/[`super::state_layer`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// The chip corner radius: themed `shape.small`, resolved against the box so
/// it never exceeds a pill. Unthemed: [`CHIP_RADIUS`] exactly.
fn resolve_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.small, size.width, size.height),
        None => CHIP_RADIUS,
    }
}

/// The resolved `(container, outlined)` pair at the given `selected`/
/// `elevated` state — see the module docs' matrix. `type` never enters this
/// resolution (see *Four types, one color table*): every chip type calls
/// this same function.
fn resolve_chip_container(theme: Option<&Theme>, selected: bool, elevated: bool) -> (Color, bool) {
    let outlined = !selected && !elevated;
    let container = if let Some(theme) = theme {
        let scheme = theme.scheme();
        if selected {
            scheme.secondary_container
        } else if elevated {
            scheme.surface_container_low
        } else {
            Color::TRANSPARENT
        }
    } else if selected {
        CHIP_SELECTED_CONTAINER
    } else if elevated {
        CHIP_ELEVATED_CONTAINER
    } else {
        Color::TRANSPARENT
    };
    (container, outlined)
}

/// The resolved label/state-layer content color at the given `selected`
/// state — see the module docs' label-color simplification for why the
/// selected case is an approximation.
fn resolve_chip_foreground(theme: Option<&Theme>, selected: bool) -> Color {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            if selected {
                scheme.on_secondary_container
            } else {
                scheme.on_surface_variant
            }
        }
        None => {
            if selected {
                CHIP_LABEL_SELECTED
            } else {
                CHIP_LABEL_UNSELECTED
            }
        }
    }
}

/// The resolved outline stroke color (unselected, unelevated chips only).
/// Themed: `colors.outline_variant`. Unthemed: [`CHIP_OUTLINE_VARIANT`].
fn resolve_chip_outline(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().outline_variant,
        None => CHIP_OUTLINE_VARIANT,
    }
}

/// The resolved `(blur_std_dev, y_offset, color)` shadow parameters at M3
/// elevation level 1 (the `elevated` modifier's fixed level — mirrors
/// [`super::card`]'s identically-named helper).
fn resolve_chip_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
    match theme {
        Some(theme) => {
            let level = theme.elevation.level1;
            let shadow = level.shadow(theme.brightness);
            let color = with_alpha(theme.scheme().shadow, shadow.color_alpha);
            (shadow.blur_std_dev, shadow.y_offset, color)
        }
        None => (CHIP_SHADOW_BLUR, CHIP_SHADOW_Y_OFFSET, CHIP_SHADOW_COLOR),
    }
}

/// Build the type-erased label/leading-glyph view, themed per `selected`
/// (see the module docs' label-color simplification), its family following
/// the live theme's `labelLarge` role (the M3 chip label token). Shared by
/// every chip type's build/rebuild/teardown, for both the label and an
/// optional leading glyph, so the role stays consistent between them.
fn chip_label_view<State: 'static>(
    text: String,
    selected: bool,
) -> frust::authoring::AnyView<State> {
    let role = if selected {
        ThemeTextColor::OnSurface
    } else {
        ThemeTextColor::OnSurfaceVariant
    };
    any::<State, _>(
        frust::text(text)
            .themed_role(role)
            .themed_family(ThemeTextType::LabelLarge),
    )
}

/// Paint the full M3 chip surface: an optional level-1 elevation shadow
/// (`elevated`), the fill (skipped when fully transparent — mirrors
/// `frust_widgets::button`'s own don't-paint-a-transparent-fill precedent),
/// an optional 1dp `outline_variant` stroke (neither `selected` nor
/// `elevated`), and the shared [`super::state_layer`] interaction overlay.
/// Shared by all four chip widgets' `paint()`. Resolves the theme from `ctx`
/// internally (rather than taking it as a separate `Option<&Theme>`
/// parameter) so its borrow ends before the trailing `state_layer.paint`
/// call re-borrows `ctx` mutably; returns the resolved foreground color so
/// the caller can paint its label/leading/delete-icon content with the same
/// value.
fn paint_chip_surface(
    ctx: &mut PaintCtx,
    scene: &mut dyn PaintScene,
    selected: bool,
    elevated: bool,
    state_layer: &StateLayer,
) -> Color {
    let theme = Theme::from_paint_ctx(ctx);
    let radius = resolve_radius(theme, ctx.size());
    let (container, outlined) = resolve_chip_container(theme, selected, elevated);
    let foreground = resolve_chip_foreground(theme, selected);
    let o = ctx.origin();
    let size = ctx.size();

    if elevated {
        let (blur, y_offset, shadow_color) = resolve_chip_shadow(theme);
        scene.draw_shadow(
            Point::new(o.x, o.y + y_offset),
            size,
            radius,
            blur,
            shadow_color,
        );
    }

    if container != Color::TRANSPARENT {
        scene.fill_rounded_rect(o, size, radius, container);
    }

    if outlined {
        let outline = resolve_chip_outline(theme);
        // Inset by half the stroke width so the 1dp line paints fully
        // inside the chip's own bounds (a stroke is centered on its path) —
        // mirrors `super::card`'s outlined-variant precedent.
        let half = CHIP_STROKE_WIDTH / 2.0;
        let rr = RoundedRect::new(
            half,
            half,
            size.width - half,
            size.height - half,
            (radius - half).max(0.0),
        );
        let path = rr.to_path(CHIP_STROKE_TOLERANCE);
        scene.stroke_path(o, &path, CHIP_STROKE_WIDTH, &Brush::Solid(outline));
    }

    // `theme`'s last use is above (`resolve_chip_outline`, if reached); no
    // further reads of it happen after this point, so its borrow of `ctx`
    // has ended by the time `ctx` is passed mutably below.
    state_layer.paint(
        ctx,
        scene,
        Rect::from_origin_size(o, size),
        radius,
        foreground,
    );
    foreground
}

/// A view-held, typed press callback (erased on build) — shared by every
/// chip type's `on_press`/`on_deleted`.
type OnPress<State> = Rc<dyn Fn(&mut State)>;

// ---------------------------------------------------------------------
// AssistChip
// ---------------------------------------------------------------------

/// A declarative assist chip. See the [module docs](self).
pub struct AssistChipView<State: 'static> {
    label: String,
    leading: Option<String>,
    elevated: bool,
    on_press: OnPress<State>,
}

/// Create an assist chip labelled `label` that runs `on_press` against the
/// app state when released inside its bounds.
pub fn assist_chip<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> AssistChipView<State> {
    AssistChipView {
        label: label.into(),
        leading: None,
        elevated: false,
        on_press: Rc::new(on_press),
    }
}

/// PascalCase alias for [`assist_chip`].
#[allow(non_snake_case)]
pub fn AssistChip<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> AssistChipView<State> {
    assist_chip(label, on_press)
}

impl<State: 'static> AssistChipView<State> {
    /// Attach a leading glyph/icon (rendered as a second nested text run —
    /// see the module docs), painted before the label with a tighter left
    /// inset.
    pub fn leading(mut self, glyph: impl Into<String>) -> Self {
        self.leading = Some(glyph.into());
        self
    }

    /// Apply the `elevated` modifier (default `false`) — see the module
    /// docs' Elevated modifier section.
    pub fn elevated(mut self, elevated: bool) -> Self {
        self.elevated = elevated;
        self
    }
}

/// The retained widget for an [`AssistChipView`].
pub struct AssistChipWidget {
    leading: Option<ChildPod>,
    leading_text: Option<String>,
    label: ChildPod,
    label_text: String,
    elevated: bool,
    /// The pressed *visual* state; follows the cursor in/out while captured.
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    captured: bool,
    state_layer: StateLayer,
    on_press: frust::authoring::ErasedCallback,
}

impl<State: 'static> View<State> for AssistChipView<State> {
    type Element = AssistChipWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AssistChipWidget {
        let label_view = chip_label_view::<State>(self.label.clone(), false);
        let leading = self.leading.as_ref().map(|glyph| {
            frust::authoring::build_child(&chip_label_view::<State>(glyph.clone(), false), ctx)
        });
        AssistChipWidget {
            leading,
            leading_text: self.leading.clone(),
            label: frust::authoring::build_child(&label_view, ctx),
            label_text: self.label.clone(),
            elevated: self.elevated,
            pressed: false,
            captured: false,
            state_layer: StateLayer::new(),
            on_press: frust::authoring::erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AssistChipWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = frust::authoring::erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;

        if prev.elevated != self.elevated {
            element.elevated = self.elevated;
            flags |= ChangeFlags::PAINT;
        }

        if prev.label != self.label {
            element.label_text = self.label.clone();
            let prev_view = chip_label_view::<State>(prev.label.clone(), false);
            let next_view = chip_label_view::<State>(self.label.clone(), false);
            flags |=
                frust::authoring::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }

        match (&prev.leading, &self.leading) {
            (None, None) => {}
            (Some(prev_glyph), Some(next_glyph)) => {
                element.leading_text = self.leading.clone();
                let prev_view = chip_label_view::<State>(prev_glyph.clone(), false);
                let next_view = chip_label_view::<State>(next_glyph.clone(), false);
                let pod = element
                    .leading
                    .as_mut()
                    .expect("leading pod present when prev.leading is Some");
                flags |= frust::authoring::rebuild_child(&prev_view, &next_view, pod, ctx);
            }
            (None, Some(next_glyph)) => {
                element.leading_text = self.leading.clone();
                element.leading = Some(frust::authoring::build_child(
                    &chip_label_view::<State>(next_glyph.clone(), false),
                    ctx,
                ));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(prev_glyph), None) => {
                element.leading_text = None;
                let prev_view = chip_label_view::<State>(prev_glyph.clone(), false);
                let mut pod = element
                    .leading
                    .take()
                    .expect("leading pod present when prev.leading is Some");
                frust::authoring::teardown_child(&prev_view, &mut pod, ctx);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }

        flags
    }

    fn teardown(&self, element: &mut AssistChipWidget, ctx: &mut BuildCtx<'_>) {
        let label_view = chip_label_view::<State>(self.label.clone(), false);
        frust::authoring::teardown_child(&label_view, &mut element.label, ctx);
        if let (Some(glyph), Some(pod)) = (&self.leading, &mut element.leading) {
            let leading_view = chip_label_view::<State>(glyph.clone(), false);
            frust::authoring::teardown_child(&leading_view, pod, ctx);
        }
    }
}

impl Widget for AssistChipWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inner_max = Size::new(f64::INFINITY, CHIP_HEIGHT);
        let mut x = if self.leading.is_some() {
            CHIP_ICON_START_PAD
        } else {
            CHIP_LABEL_START_PAD
        };
        if let Some(leading) = &mut self.leading {
            let size = leading.layout_child(ctx, &BoxConstraints::loose(inner_max));
            leading.set_origin(Point::new(x, (CHIP_HEIGHT - size.height) / 2.0));
            x += size.width + CHIP_ICON_LABEL_GAP;
        }
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        self.label
            .set_origin(Point::new(x, (CHIP_HEIGHT - label_size.height) / 2.0));
        x += label_size.width + CHIP_END_PAD;
        bc.constrain(Size::new(x, CHIP_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        paint_chip_surface(ctx, scene, false, self.elevated, &self.state_layer);
        if let Some(leading) = &mut self.leading {
            leading.paint_child(ctx, scene);
        }
        self.label.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
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
                if inside(p.position, ctx.size()) {
                    (self.on_press)(ctx);
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
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label_text.as_str());
            node.add_action(Action::Click);
        });
    }

    frust::authoring::visit_children!(leading, label);
}

// ---------------------------------------------------------------------
// FilterChip
// ---------------------------------------------------------------------

/// A view-held, typed select callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative filter chip. See the [module docs](self).
pub struct FilterChipView<State: 'static> {
    label: String,
    selected: bool,
    elevated: bool,
    on_select: OnSelect<State>,
}

/// Create a filter chip labelled `label` reflecting `selected`, that fires
/// `on_select(state, !selected)` on release inside its bounds.
pub fn filter_chip<State: 'static, F: Fn(&mut State, bool) + 'static>(
    label: impl Into<String>,
    selected: bool,
    on_select: F,
) -> FilterChipView<State> {
    FilterChipView {
        label: label.into(),
        selected,
        elevated: false,
        on_select: Rc::new(on_select),
    }
}

/// PascalCase alias for [`filter_chip`].
#[allow(non_snake_case)]
pub fn FilterChip<State: 'static, F: Fn(&mut State, bool) + 'static>(
    label: impl Into<String>,
    selected: bool,
    on_select: F,
) -> FilterChipView<State> {
    filter_chip(label, selected, on_select)
}

impl<State: 'static> FilterChipView<State> {
    /// Apply the `elevated` modifier (default `false`) — see the module
    /// docs' Elevated modifier section.
    pub fn elevated(mut self, elevated: bool) -> Self {
        self.elevated = elevated;
        self
    }
}

/// The retained widget for a [`FilterChipView`].
pub struct FilterChipWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    selected: bool,
    elevated: bool,
    label: ChildPod,
    label_text: String,
    /// The pressed *visual* state; follows the cursor in/out while captured.
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    captured: bool,
    state_layer: StateLayer,
    on_select: frust::authoring::ErasedArgCallback<bool>,
}

impl<State: 'static> View<State> for FilterChipView<State> {
    type Element = FilterChipWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FilterChipWidget {
        let label_view = chip_label_view::<State>(self.label.clone(), self.selected);
        FilterChipWidget {
            selected: self.selected,
            elevated: self.elevated,
            label: frust::authoring::build_child(&label_view, ctx),
            label_text: self.label.clone(),
            pressed: false,
            captured: false,
            state_layer: StateLayer::new(),
            on_select: frust::authoring::erase_callback_arg(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FilterChipWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = frust::authoring::erase_callback_arg(&self.on_select);
        let mut flags = ChangeFlags::NONE;
        if prev.selected != self.selected {
            // The app is the source of truth: adopt the new value on rebuild.
            element.selected = self.selected;
            flags |= ChangeFlags::PAINT;
        }
        if prev.elevated != self.elevated {
            element.elevated = self.elevated;
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label || prev.selected != self.selected {
            // The label's themed role tracks `selected` too (see the module
            // docs' label-color simplification), so a selected-only change
            // still needs a label rebuild.
            element.label_text = self.label.clone();
            let prev_view = chip_label_view::<State>(prev.label.clone(), prev.selected);
            let next_view = chip_label_view::<State>(self.label.clone(), self.selected);
            flags |=
                frust::authoring::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }
        flags
    }

    fn teardown(&self, element: &mut FilterChipWidget, ctx: &mut BuildCtx<'_>) {
        let label_view = chip_label_view::<State>(self.label.clone(), self.selected);
        frust::authoring::teardown_child(&label_view, &mut element.label, ctx);
    }
}

impl Widget for FilterChipWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inner_max = Size::new(f64::INFINITY, CHIP_HEIGHT);
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        self.label.set_origin(Point::new(
            CHIP_LABEL_START_PAD,
            (CHIP_HEIGHT - label_size.height) / 2.0,
        ));
        bc.constrain(Size::new(
            label_size.width + CHIP_LABEL_START_PAD + CHIP_END_PAD,
            CHIP_HEIGHT,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        paint_chip_surface(ctx, scene, self.selected, self.elevated, &self.state_layer);
        self.label.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
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
                if inside(p.position, ctx.size()) {
                    // Report the *requested* value; never self-toggle.
                    let requested = !self.selected;
                    (self.on_select)(ctx, requested);
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
        // No dedicated "toggle button"/"selectable chip" accesskit role
        // exists; a filter chip is exposed as a Button carrying Toggled
        // state (the common aria-pressed-style pattern for a toggleable
        // button-shaped control).
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label_text.as_str());
            node.set_toggled(Toggled::from(self.selected));
            node.add_action(Action::Click);
        });
    }

    frust::authoring::visit_children!(label);
}

// ---------------------------------------------------------------------
// SuggestionChip
// ---------------------------------------------------------------------

/// A declarative suggestion chip: action-only, like [`AssistChipView`] —
/// its own type since M3 draws a semantic (not visual) distinction between
/// "an assist action" and "a dynamically generated suggestion". See the
/// [module docs](self).
pub struct SuggestionChipView<State: 'static> {
    label: String,
    leading: Option<String>,
    elevated: bool,
    on_press: OnPress<State>,
}

/// Create a suggestion chip labelled `label` that runs `on_press` against
/// the app state when released inside its bounds.
pub fn suggestion_chip<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> SuggestionChipView<State> {
    SuggestionChipView {
        label: label.into(),
        leading: None,
        elevated: false,
        on_press: Rc::new(on_press),
    }
}

/// PascalCase alias for [`suggestion_chip`].
#[allow(non_snake_case)]
pub fn SuggestionChip<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> SuggestionChipView<State> {
    suggestion_chip(label, on_press)
}

impl<State: 'static> SuggestionChipView<State> {
    /// Attach a leading glyph/icon (rendered as a second nested text run —
    /// see the module docs), painted before the label with a tighter left
    /// inset.
    pub fn leading(mut self, glyph: impl Into<String>) -> Self {
        self.leading = Some(glyph.into());
        self
    }

    /// Apply the `elevated` modifier (default `false`) — see the module
    /// docs' Elevated modifier section.
    pub fn elevated(mut self, elevated: bool) -> Self {
        self.elevated = elevated;
        self
    }
}

/// The retained widget for a [`SuggestionChipView`].
pub struct SuggestionChipWidget {
    leading: Option<ChildPod>,
    leading_text: Option<String>,
    label: ChildPod,
    label_text: String,
    elevated: bool,
    pressed: bool,
    captured: bool,
    state_layer: StateLayer,
    on_press: frust::authoring::ErasedCallback,
}

impl<State: 'static> View<State> for SuggestionChipView<State> {
    type Element = SuggestionChipWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SuggestionChipWidget {
        let label_view = chip_label_view::<State>(self.label.clone(), false);
        let leading = self.leading.as_ref().map(|glyph| {
            frust::authoring::build_child(&chip_label_view::<State>(glyph.clone(), false), ctx)
        });
        SuggestionChipWidget {
            leading,
            leading_text: self.leading.clone(),
            label: frust::authoring::build_child(&label_view, ctx),
            label_text: self.label.clone(),
            elevated: self.elevated,
            pressed: false,
            captured: false,
            state_layer: StateLayer::new(),
            on_press: frust::authoring::erase_callback(&self.on_press),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SuggestionChipWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = frust::authoring::erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;

        if prev.elevated != self.elevated {
            element.elevated = self.elevated;
            flags |= ChangeFlags::PAINT;
        }

        if prev.label != self.label {
            element.label_text = self.label.clone();
            let prev_view = chip_label_view::<State>(prev.label.clone(), false);
            let next_view = chip_label_view::<State>(self.label.clone(), false);
            flags |=
                frust::authoring::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }

        match (&prev.leading, &self.leading) {
            (None, None) => {}
            (Some(prev_glyph), Some(next_glyph)) => {
                element.leading_text = self.leading.clone();
                let prev_view = chip_label_view::<State>(prev_glyph.clone(), false);
                let next_view = chip_label_view::<State>(next_glyph.clone(), false);
                let pod = element
                    .leading
                    .as_mut()
                    .expect("leading pod present when prev.leading is Some");
                flags |= frust::authoring::rebuild_child(&prev_view, &next_view, pod, ctx);
            }
            (None, Some(next_glyph)) => {
                element.leading_text = self.leading.clone();
                element.leading = Some(frust::authoring::build_child(
                    &chip_label_view::<State>(next_glyph.clone(), false),
                    ctx,
                ));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(prev_glyph), None) => {
                element.leading_text = None;
                let prev_view = chip_label_view::<State>(prev_glyph.clone(), false);
                let mut pod = element
                    .leading
                    .take()
                    .expect("leading pod present when prev.leading is Some");
                frust::authoring::teardown_child(&prev_view, &mut pod, ctx);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }

        flags
    }

    fn teardown(&self, element: &mut SuggestionChipWidget, ctx: &mut BuildCtx<'_>) {
        let label_view = chip_label_view::<State>(self.label.clone(), false);
        frust::authoring::teardown_child(&label_view, &mut element.label, ctx);
        if let (Some(glyph), Some(pod)) = (&self.leading, &mut element.leading) {
            let leading_view = chip_label_view::<State>(glyph.clone(), false);
            frust::authoring::teardown_child(&leading_view, pod, ctx);
        }
    }
}

impl Widget for SuggestionChipWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inner_max = Size::new(f64::INFINITY, CHIP_HEIGHT);
        let mut x = if self.leading.is_some() {
            CHIP_ICON_START_PAD
        } else {
            CHIP_LABEL_START_PAD
        };
        if let Some(leading) = &mut self.leading {
            let size = leading.layout_child(ctx, &BoxConstraints::loose(inner_max));
            leading.set_origin(Point::new(x, (CHIP_HEIGHT - size.height) / 2.0));
            x += size.width + CHIP_ICON_LABEL_GAP;
        }
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        self.label
            .set_origin(Point::new(x, (CHIP_HEIGHT - label_size.height) / 2.0));
        x += label_size.width + CHIP_END_PAD;
        bc.constrain(Size::new(x, CHIP_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        paint_chip_surface(ctx, scene, false, self.elevated, &self.state_layer);
        if let Some(leading) = &mut self.leading {
            leading.paint_child(ctx, scene);
        }
        self.label.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
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
                if inside(p.position, ctx.size()) {
                    (self.on_press)(ctx);
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
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label_text.as_str());
            node.add_action(Action::Click);
        });
    }

    frust::authoring::visit_children!(leading, label);
}

// ---------------------------------------------------------------------
// InputChip
// ---------------------------------------------------------------------

/// Which region of an input chip's surface a pointer interaction targets:
/// its own [`InputChipView::on_press`] surface (`Body`), or — only when
/// [`InputChipView::on_deleted`] is set — the trailing delete icon
/// (`Delete`). Mirrors [`mod@super::split_button`]'s `Half` two-target
/// dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputRegion {
    Body,
    Delete,
}

/// A declarative input chip: user-entered information, with an optional
/// trailing delete affordance. See the [module docs](self).
pub struct InputChipView<State: 'static> {
    label: String,
    leading: Option<String>,
    elevated: bool,
    on_press: OnPress<State>,
    on_deleted: Option<OnPress<State>>,
}

/// Create an input chip labelled `label` that runs `on_press` against the
/// app state when released inside its bounds (outside the delete icon, if
/// one is attached via [`InputChipView::on_deleted`]).
pub fn input_chip<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> InputChipView<State> {
    InputChipView {
        label: label.into(),
        leading: None,
        elevated: false,
        on_press: Rc::new(on_press),
        on_deleted: None,
    }
}

/// PascalCase alias for [`input_chip`].
#[allow(non_snake_case)]
pub fn InputChip<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> InputChipView<State> {
    input_chip(label, on_press)
}

impl<State: 'static> InputChipView<State> {
    /// Attach a leading glyph/icon (rendered as a second nested text run —
    /// see the module docs), painted before the label with a tighter left
    /// inset.
    pub fn leading(mut self, glyph: impl Into<String>) -> Self {
        self.leading = Some(glyph.into());
        self
    }

    /// Apply the `elevated` modifier (default `false`) — see the module
    /// docs' Elevated modifier section.
    pub fn elevated(mut self, elevated: bool) -> Self {
        self.elevated = elevated;
        self
    }

    /// Attach a trailing 18dp delete icon ([`crate::icons::CLOSE`]), firing
    /// `on_deleted` against the app state when released inside its own hit
    /// region — independent of [`InputChipView`]'s own `on_press` (see the
    /// module docs). Omit this to render a plain, non-deletable input chip.
    pub fn on_deleted<F: Fn(&mut State) + 'static>(mut self, on_deleted: F) -> Self {
        self.on_deleted = Some(Rc::new(on_deleted));
        self
    }
}

/// The retained widget for an [`InputChipView`].
pub struct InputChipWidget {
    leading: Option<ChildPod>,
    leading_text: Option<String>,
    label: ChildPod,
    label_text: String,
    elevated: bool,
    has_delete: bool,
    /// The delete icon's local hit rect, from [`Widget::layout`]. `Rect::ZERO`
    /// when `has_delete` is `false`.
    delete_rect: Rect,
    /// The armed region of an in-flight press (`None` when idle).
    armed: Option<InputRegion>,
    /// Whether the armed pointer is currently inside the armed region.
    pressed_inside: bool,
    state_layer: StateLayer,
    on_press: frust::authoring::ErasedCallback,
    on_deleted: Option<frust::authoring::ErasedCallback>,
}

impl<State: 'static> View<State> for InputChipView<State> {
    type Element = InputChipWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> InputChipWidget {
        let label_view = chip_label_view::<State>(self.label.clone(), false);
        let leading = self.leading.as_ref().map(|glyph| {
            frust::authoring::build_child(&chip_label_view::<State>(glyph.clone(), false), ctx)
        });
        InputChipWidget {
            leading,
            leading_text: self.leading.clone(),
            label: frust::authoring::build_child(&label_view, ctx),
            label_text: self.label.clone(),
            elevated: self.elevated,
            has_delete: self.on_deleted.is_some(),
            delete_rect: Rect::ZERO,
            armed: None,
            pressed_inside: false,
            state_layer: StateLayer::new(),
            on_press: frust::authoring::erase_callback(&self.on_press),
            on_deleted: self
                .on_deleted
                .as_ref()
                .map(frust::authoring::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut InputChipWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = frust::authoring::erase_callback(&self.on_press);
        element.on_deleted = self
            .on_deleted
            .as_ref()
            .map(frust::authoring::erase_callback);
        let mut flags = ChangeFlags::NONE;

        if prev.elevated != self.elevated {
            element.elevated = self.elevated;
            flags |= ChangeFlags::PAINT;
        }

        let now_has_delete = self.on_deleted.is_some();
        if element.has_delete != now_has_delete {
            element.has_delete = now_has_delete;
            // Dropping the delete affordance mid-gesture must not leave a
            // dangling capture behind (mirrors `super::card`'s
            // interactive-flag-drop guard).
            if !now_has_delete && element.armed == Some(InputRegion::Delete) {
                element.armed = None;
                element.pressed_inside = false;
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.label != self.label {
            element.label_text = self.label.clone();
            let prev_view = chip_label_view::<State>(prev.label.clone(), false);
            let next_view = chip_label_view::<State>(self.label.clone(), false);
            flags |=
                frust::authoring::rebuild_child(&prev_view, &next_view, &mut element.label, ctx);
        }

        match (&prev.leading, &self.leading) {
            (None, None) => {}
            (Some(prev_glyph), Some(next_glyph)) => {
                element.leading_text = self.leading.clone();
                let prev_view = chip_label_view::<State>(prev_glyph.clone(), false);
                let next_view = chip_label_view::<State>(next_glyph.clone(), false);
                let pod = element
                    .leading
                    .as_mut()
                    .expect("leading pod present when prev.leading is Some");
                flags |= frust::authoring::rebuild_child(&prev_view, &next_view, pod, ctx);
            }
            (None, Some(next_glyph)) => {
                element.leading_text = self.leading.clone();
                element.leading = Some(frust::authoring::build_child(
                    &chip_label_view::<State>(next_glyph.clone(), false),
                    ctx,
                ));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(prev_glyph), None) => {
                element.leading_text = None;
                let prev_view = chip_label_view::<State>(prev_glyph.clone(), false);
                let mut pod = element
                    .leading
                    .take()
                    .expect("leading pod present when prev.leading is Some");
                frust::authoring::teardown_child(&prev_view, &mut pod, ctx);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }

        flags
    }

    fn teardown(&self, element: &mut InputChipWidget, ctx: &mut BuildCtx<'_>) {
        let label_view = chip_label_view::<State>(self.label.clone(), false);
        frust::authoring::teardown_child(&label_view, &mut element.label, ctx);
        if let (Some(glyph), Some(pod)) = (&self.leading, &mut element.leading) {
            let leading_view = chip_label_view::<State>(glyph.clone(), false);
            frust::authoring::teardown_child(&leading_view, pod, ctx);
        }
    }
}

impl Widget for InputChipWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inner_max = Size::new(f64::INFINITY, CHIP_HEIGHT);
        let mut x = if self.leading.is_some() {
            CHIP_ICON_START_PAD
        } else {
            CHIP_LABEL_START_PAD
        };
        if let Some(leading) = &mut self.leading {
            let size = leading.layout_child(ctx, &BoxConstraints::loose(inner_max));
            leading.set_origin(Point::new(x, (CHIP_HEIGHT - size.height) / 2.0));
            x += size.width + CHIP_ICON_LABEL_GAP;
        }
        let label_size = self
            .label
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        self.label
            .set_origin(Point::new(x, (CHIP_HEIGHT - label_size.height) / 2.0));
        x += label_size.width;
        if self.has_delete {
            x += CHIP_ICON_LABEL_GAP;
            let icon_y = (CHIP_HEIGHT - CHIP_ICON_SIZE) / 2.0;
            self.delete_rect = Rect::new(x, icon_y, x + CHIP_ICON_SIZE, icon_y + CHIP_ICON_SIZE);
            x += CHIP_ICON_SIZE;
        } else {
            self.delete_rect = Rect::ZERO;
        }
        x += CHIP_END_PAD;
        bc.constrain(Size::new(x, CHIP_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Input chips are never `selected` — see the module docs' Selected
        // state per type.
        let foreground = paint_chip_surface(ctx, scene, false, self.elevated, &self.state_layer);
        if let Some(leading) = &mut self.leading {
            leading.paint_child(ctx, scene);
        }
        self.label.paint_child(ctx, scene);
        if self.has_delete {
            let (base_path, design) = IconData::from(icons::CLOSE).resolve();
            let scale = if design > 0.0 {
                CHIP_ICON_SIZE / design
            } else {
                1.0
            };
            let scaled = Affine::scale(scale) * base_path;
            let icon_origin = Point::new(
                ctx.origin().x + self.delete_rect.x0,
                ctx.origin().y + self.delete_rect.y0,
            );
            scene.fill_path(icon_origin, &scaled, &Brush::Solid(foreground));
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let region = if self.has_delete && self.delete_rect.contains(p.position) {
                    InputRegion::Delete
                } else if inside(p.position, ctx.size()) {
                    InputRegion::Body
                } else {
                    return EventResult::Ignored;
                };
                self.armed = Some(region);
                self.pressed_inside = true;
                if region == InputRegion::Body {
                    // The delete icon carries no interaction-state feedback
                    // of its own, matching the reference's bare
                    // `GestureDetector` (no state-layer coupling) — see the
                    // module docs.
                    self.state_layer.set_pressed(true);
                }
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let Some(region) = self.armed else {
                    return EventResult::Ignored;
                };
                let now_inside = match region {
                    InputRegion::Delete => self.delete_rect.contains(p.position),
                    InputRegion::Body => inside(p.position, ctx.size()),
                };
                self.pressed_inside = now_inside;
                if region == InputRegion::Body {
                    self.state_layer.set_pressed(now_inside);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(region) = self.armed else {
                    return EventResult::Ignored;
                };
                if self.pressed_inside {
                    match region {
                        InputRegion::Body => (self.on_press)(ctx),
                        InputRegion::Delete => {
                            if let Some(on_deleted) = &mut self.on_deleted {
                                (on_deleted)(ctx);
                            }
                        }
                    }
                }
                self.armed = None;
                self.pressed_inside = false;
                self.state_layer.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.armed.is_none() {
                    return EventResult::Ignored;
                }
                self.armed = None;
                self.pressed_inside = false;
                self.state_layer.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.has_delete {
            // Two separate interactive targets under one group, mirroring
            // `super::split_button`'s Group + two-Button semantics shape.
            ctx.push_container(
                Role::Group,
                |_| {},
                |ctx| {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(self.label_text.as_str());
                        node.add_action(Action::Click);
                    });
                    ctx.push_node(Role::Button, |node| {
                        node.set_label("Remove");
                        node.add_action(Action::Click);
                    });
                },
            );
        } else {
            ctx.push_node(Role::Button, |node| {
                node.set_label(self.label_text.as_str());
                node.add_action(Action::Click);
            });
        }
    }

    frust::authoring::visit_children!(leading, label);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Primary,
        })
    }

    /// Records each rounded rect's `(origin, size, radius, color)`, each
    /// shadow's `(origin, size, radius, std_dev, color)`, each stroked
    /// path's `(origin, width, color)`, and each filled path's `(origin,
    /// color)` — mirrors `super::card`'s test `Recorder`.
    #[derive(Default)]
    struct ChipRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        strokes: Vec<(Point, f64, Color)>,
        fills: Vec<(Point, Color)>,
    }

    impl PaintScene for ChipRecorder {
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
        fn fill_path(&mut self, origin: Point, _path: &kurbo::BezPath, brush: &Brush) {
            if let Brush::Solid(color) = brush {
                self.fills.push((origin, *color));
            }
        }
    }

    fn paint_rec(w: &mut dyn Widget, size: Size, theme: Option<&Theme>) -> ChipRecorder {
        let mut rec = ChipRecorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    // --- Shared resolution matrix (acceptance criterion 1) ---

    mod resolution_matrix {
        use super::*;

        #[test]
        fn unthemed_unselected_unelevated_is_transparent_and_outlined() {
            let (container, outlined) = resolve_chip_container(None, false, false);
            assert_eq!(container, Color::TRANSPARENT);
            assert!(outlined);
        }

        #[test]
        fn unthemed_elevated_unselected_is_elevated_container_not_outlined() {
            let (container, outlined) = resolve_chip_container(None, false, true);
            assert_eq!(container, CHIP_ELEVATED_CONTAINER);
            assert!(!outlined);
        }

        #[test]
        fn unthemed_selected_is_selected_container_not_outlined_regardless_of_elevated() {
            let (container, outlined) = resolve_chip_container(None, true, false);
            assert_eq!(container, CHIP_SELECTED_CONTAINER);
            assert!(!outlined);

            let (container, outlined) = resolve_chip_container(None, true, true);
            assert_eq!(container, CHIP_SELECTED_CONTAINER);
            assert!(!outlined);
        }

        #[test]
        fn themed_matrix_resolves_exact_scheme_roles() {
            let theme = crate::baseline();
            let scheme = theme.scheme();

            let (unselected_unelevated, outlined) =
                resolve_chip_container(Some(&theme), false, false);
            assert_eq!(unselected_unelevated, Color::TRANSPARENT);
            assert!(outlined);

            let (elevated, outlined) = resolve_chip_container(Some(&theme), false, true);
            assert_eq!(elevated, scheme.surface_container_low);
            assert!(!outlined);

            let (selected, outlined) = resolve_chip_container(Some(&theme), true, false);
            assert_eq!(selected, scheme.secondary_container);
            assert!(!outlined);

            // Selected + elevated: still `secondary_container`, still not
            // outlined (the shadow is gated on `elevated` alone, in the
            // widget-level paint helper, not here).
            let (selected_elevated, outlined) = resolve_chip_container(Some(&theme), true, true);
            assert_eq!(selected_elevated, scheme.secondary_container);
            assert!(!outlined);
        }

        #[test]
        fn themed_foreground_resolves_on_surface_variant_unselected_on_secondary_container_selected()
         {
            let theme = crate::baseline();
            let scheme = theme.scheme();
            assert_eq!(
                resolve_chip_foreground(Some(&theme), false),
                scheme.on_surface_variant
            );
            // `resolve_chip_foreground` is the exact raw-paint-color
            // resolver (state layer, delete icon, outline); only the
            // nested `TextView` label approximates the selected case as
            // `ThemeTextColor::OnSurface` (see the module docs' label-color
            // simplification) — this function itself resolves the real
            // `on_secondary_container` role.
            assert_eq!(
                resolve_chip_foreground(Some(&theme), true),
                scheme.on_secondary_container
            );
        }

        #[test]
        fn unthemed_foreground_matches_fallback_constants() {
            assert_eq!(resolve_chip_foreground(None, false), CHIP_LABEL_UNSELECTED);
            assert_eq!(resolve_chip_foreground(None, true), CHIP_LABEL_SELECTED);
        }

        #[test]
        fn themed_outline_resolves_outline_variant() {
            let theme = crate::baseline();
            assert_eq!(
                resolve_chip_outline(Some(&theme)),
                theme.scheme().outline_variant
            );
            assert_eq!(resolve_chip_outline(None), CHIP_OUTLINE_VARIANT);
        }
    }

    // --- AssistChip ---

    mod assist {
        use super::*;

        #[derive(Default)]
        struct Presses(u32);

        fn widget() -> AssistChipWidget {
            let view = assist_chip::<Presses, _>("Go", |s: &mut Presses| s.0 += 1);
            let mut counter = 0u64;
            View::<Presses>::build(&view, &mut BuildCtx::new(&mut counter))
        }

        fn dispatch(w: &mut AssistChipWidget, state: &mut Presses, event: &InputEvent) {
            let state_any: &mut dyn Any = state;
            let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, CHIP_HEIGHT));
            w.event(&mut ctx, event);
        }

        #[test]
        fn up_inside_fires_once() {
            let mut w = widget();
            let mut state = Presses::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
            assert_eq!(state.0, 1);
        }

        #[test]
        fn up_outside_does_not_fire() {
            let mut w = widget();
            let mut state = Presses::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 12.0));
            assert_eq!(state.0, 0);
        }

        #[test]
        fn cancel_clears_armed_state_without_firing() {
            let mut w = widget();
            let mut state = Presses::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            assert!(w.captured);
            dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 5.0, 12.0));
            assert!(!w.captured);
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
            assert_eq!(state.0, 0);
        }

        #[test]
        fn hover_move_without_down_is_ignored_noop() {
            let mut w = widget();
            let mut state = Presses::default();
            let state_any: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, CHIP_HEIGHT));
            let result = w.event(&mut ctx, &ev(PointerPhase::Move, 5.0, 12.0));
            assert!(matches!(result, EventResult::Ignored));
            assert!(!ctx.needs_redraw());
            assert_eq!(state.0, 0);
        }

        /// Corrects this module's original expectation (a flat
        /// `surface_container_low` fill for every unselected chip): an
        /// unelevated, unselected assist chip is now transparent-filled and
        /// outlined — see the module docs' matrix.
        #[test]
        fn unthemed_unelevated_paints_transparent_and_outlined() {
            let mut w = widget();
            let rec = paint_rec(&mut w, Size::new(100.0, CHIP_HEIGHT), None);
            assert!(
                rec.rrects.is_empty(),
                "a transparent fill is skipped entirely, not painted"
            );
            assert_eq!(rec.strokes.len(), 1);
            assert_eq!(rec.strokes[0].1, CHIP_STROKE_WIDTH);
            assert_eq!(rec.strokes[0].2, CHIP_OUTLINE_VARIANT);
        }

        #[test]
        fn elevated_paints_elevated_container_and_shadow_no_outline() {
            let view = assist_chip::<Presses, _>("Go", |_s: &mut Presses| {}).elevated(true);
            let mut counter = 0u64;
            let mut w = View::<Presses>::build(&view, &mut BuildCtx::new(&mut counter));
            let rec = paint_rec(&mut w, Size::new(100.0, CHIP_HEIGHT), None);
            assert_eq!(rec.rrects[0].3, CHIP_ELEVATED_CONTAINER);
            assert_eq!(rec.shadows.len(), 1);
            assert!(rec.strokes.is_empty(), "an elevated chip is never outlined");
        }

        #[test]
        fn themed_unelevated_paint_resolves_outline_variant_stroke() {
            let theme = crate::baseline();
            let mut w = widget();
            let rec = paint_rec(&mut w, Size::new(100.0, CHIP_HEIGHT), Some(&theme));
            assert!(rec.rrects.is_empty());
            assert_eq!(rec.strokes[0].2, theme.scheme().outline_variant);
        }

        #[test]
        fn themed_elevated_paint_resolves_surface_container_low() {
            let theme = crate::baseline();
            let view = assist_chip::<Presses, _>("Go", |_s: &mut Presses| {}).elevated(true);
            let mut counter = 0u64;
            let mut w = View::<Presses>::build(&view, &mut BuildCtx::new(&mut counter));
            let rec = paint_rec(&mut w, Size::new(100.0, CHIP_HEIGHT), Some(&theme));
            assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_low);
            assert_eq!(rec.rrects[0].2, theme.shape.small);
        }

        #[test]
        fn leading_glyph_is_optional_and_shifts_label_layout() {
            use frust::authoring::text::TextContext;
            use std::any::Any;

            let mut counter = 0u64;
            let plain = assist_chip::<Presses, _>("Go", |_s: &mut Presses| {});
            let mut w_plain = View::<Presses>::build(&plain, &mut BuildCtx::new(&mut counter));
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let size_plain =
                w_plain.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)));

            let with_leading = assist_chip::<Presses, _>("Go", |_s: &mut Presses| {}).leading("*");
            let mut w_leading =
                View::<Presses>::build(&with_leading, &mut BuildCtx::new(&mut counter));
            let mut lctx2 = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let size_leading =
                w_leading.layout(&mut lctx2, &BoxConstraints::loose(Size::new(400.0, 100.0)));

            assert!(
                size_leading.width > size_plain.width,
                "a leading glyph widens the chip"
            );
            assert_eq!(size_plain.height, CHIP_HEIGHT);
            assert_eq!(size_leading.height, CHIP_HEIGHT);
        }

        #[test]
        fn rebuild_elevated_toggle_flags_paint_only() {
            let mut counter = 0u64;
            let prev = assist_chip::<Presses, _>("Go", |_s: &mut Presses| {});
            let mut w = View::<Presses>::build(&prev, &mut BuildCtx::new(&mut counter));
            let next = assist_chip::<Presses, _>("Go", |_s: &mut Presses| {}).elevated(true);
            let flags =
                View::<Presses>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
            assert!(w.elevated);
            assert!(flags.needs_paint());
        }

        #[test]
        fn semantics_reports_button_role_and_label() {
            use frust::authoring::text::TextContext;
            use std::any::Any;

            fn logic(_s: &mut Presses) -> AssistChipView<Presses> {
                assist_chip::<Presses, _>("Go", |_s: &mut Presses| {})
            }
            let mut root: frust_core::RenderRoot<Presses, AssistChipView<Presses>> =
                frust_core::RenderRoot::new();
            let mut state = Presses::default();
            root.rebuild(&mut logic, &mut state);
            let mut tcx = TextContext::new();
            root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
            let update = root.semantics();
            let (_, node) = update
                .nodes
                .iter()
                .find(|(_, n)| n.role() == Role::Button)
                .expect("assist chip contributes a Role::Button node");
            assert_eq!(node.label(), Some("Go"));
            assert!(node.supports_action(Action::Click));
        }
    }

    // --- FilterChip ---

    mod filter {
        use super::*;

        #[derive(Default)]
        struct SelectState {
            last: Option<bool>,
            selects: u32,
        }

        fn widget(selected: bool) -> FilterChipWidget {
            let view =
                filter_chip::<SelectState, _>("Vegan", selected, |s: &mut SelectState, v: bool| {
                    s.last = Some(v);
                    s.selects += 1;
                });
            let mut counter = 0u64;
            View::<SelectState>::build(&view, &mut BuildCtx::new(&mut counter))
        }

        fn dispatch(w: &mut FilterChipWidget, state: &mut SelectState, event: &InputEvent) {
            let state_any: &mut dyn Any = state;
            let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, CHIP_HEIGHT));
            w.event(&mut ctx, event);
        }

        #[test]
        fn unselected_fires_true_and_does_not_self_mutate() {
            let mut w = widget(false);
            let mut state = SelectState::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
            assert_eq!(state.last, Some(true));
            assert_eq!(state.selects, 1);
            assert!(!w.selected, "chip must not mutate its own selected flag");
        }

        #[test]
        fn selected_fires_false() {
            let mut w = widget(true);
            let mut state = SelectState::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
            assert_eq!(state.last, Some(false));
            assert!(w.selected, "still selected until the app rebuilds it");
        }

        #[test]
        fn up_outside_does_not_fire() {
            let mut w = widget(false);
            let mut state = SelectState::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 12.0));
            assert_eq!(state.selects, 0);
        }

        #[test]
        fn rebuild_adopts_new_selected_value_without_self_mutation() {
            let mut counter = 0u64;
            let prev = filter_chip::<SelectState, _>("Vegan", false, |_s, _v| {});
            let mut w = View::<SelectState>::build(&prev, &mut BuildCtx::new(&mut counter));
            assert!(!w.selected);
            let next = filter_chip::<SelectState, _>("Vegan", true, |_s, _v| {});
            let flags = View::<SelectState>::rebuild(
                &next,
                &prev,
                &mut w,
                &mut BuildCtx::new(&mut counter),
            );
            assert!(w.selected);
            assert!(flags.needs_paint());
        }

        /// Corrects this module's original expectations (an unselected
        /// filter chip filled `surface_container_low`, a selected one
        /// filled `secondary_container`): unselected is now
        /// transparent-filled and outlined; selected is unchanged (it was
        /// already `secondary_container`) — see the module docs' matrix.
        #[test]
        fn unthemed_paint_matches_the_matrix() {
            let mut off = widget(false);
            let rec = paint_rec(&mut off, Size::new(100.0, CHIP_HEIGHT), None);
            assert!(
                rec.rrects.is_empty(),
                "unselected+unelevated is transparent"
            );
            assert_eq!(rec.strokes.len(), 1);
            assert_eq!(rec.strokes[0].2, CHIP_OUTLINE_VARIANT);

            let mut on = widget(true);
            let rec = paint_rec(&mut on, Size::new(100.0, CHIP_HEIGHT), None);
            assert_eq!(rec.rrects[0].3, CHIP_SELECTED_CONTAINER);
            assert!(rec.strokes.is_empty(), "a selected chip is never outlined");
        }

        #[test]
        fn themed_paint_resolves_secondary_container_when_selected() {
            let theme = crate::baseline();
            let mut off = widget(false);
            let rec = paint_rec(&mut off, Size::new(100.0, CHIP_HEIGHT), Some(&theme));
            assert!(rec.rrects.is_empty());
            assert_eq!(rec.strokes[0].2, theme.scheme().outline_variant);

            let mut on = widget(true);
            let rec = paint_rec(&mut on, Size::new(100.0, CHIP_HEIGHT), Some(&theme));
            assert_eq!(rec.rrects[0].3, theme.scheme().secondary_container);
        }

        #[test]
        fn elevated_and_selected_paints_both_shadow_and_selected_container() {
            let view = filter_chip::<SelectState, _>("Vegan", true, |_s, _v| {}).elevated(true);
            let mut counter = 0u64;
            let mut w = View::<SelectState>::build(&view, &mut BuildCtx::new(&mut counter));
            let rec = paint_rec(&mut w, Size::new(100.0, CHIP_HEIGHT), None);
            assert_eq!(rec.rrects[0].3, CHIP_SELECTED_CONTAINER);
            assert_eq!(
                rec.shadows.len(),
                1,
                "elevated is gated independently of selected"
            );
            assert!(rec.strokes.is_empty());
        }

        #[test]
        fn semantics_reports_toggled_state() {
            use frust::authoring::text::TextContext;
            use std::any::Any;

            fn logic(_s: &mut SelectState) -> FilterChipView<SelectState> {
                filter_chip::<SelectState, _>("Vegan", true, |_s, _v| {})
            }
            let mut root: frust_core::RenderRoot<SelectState, FilterChipView<SelectState>> =
                frust_core::RenderRoot::new();
            let mut state = SelectState::default();
            root.rebuild(&mut logic, &mut state);
            let mut tcx = TextContext::new();
            root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
            let update = root.semantics();
            let (_, node) = update
                .nodes
                .iter()
                .find(|(_, n)| n.role() == Role::Button)
                .expect("filter chip contributes a Role::Button node");
            assert_eq!(node.label(), Some("Vegan"));
            assert_eq!(node.toggled(), Some(Toggled::True));
        }
    }

    // --- SuggestionChip ---

    mod suggestion {
        use super::*;

        #[derive(Default)]
        struct Presses(u32);

        fn widget() -> SuggestionChipWidget {
            let view = suggestion_chip::<Presses, _>("Try this", |s: &mut Presses| s.0 += 1);
            let mut counter = 0u64;
            View::<Presses>::build(&view, &mut BuildCtx::new(&mut counter))
        }

        fn dispatch(w: &mut SuggestionChipWidget, state: &mut Presses, event: &InputEvent) {
            let state_any: &mut dyn Any = state;
            let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, CHIP_HEIGHT));
            w.event(&mut ctx, event);
        }

        #[test]
        fn up_inside_fires_once() {
            let mut w = widget();
            let mut state = Presses::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
            assert_eq!(state.0, 1);
        }

        #[test]
        fn up_outside_does_not_fire() {
            let mut w = widget();
            let mut state = Presses::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 12.0));
            assert_eq!(state.0, 0);
        }

        #[test]
        fn unthemed_unelevated_paints_transparent_and_outlined() {
            let mut w = widget();
            let rec = paint_rec(&mut w, Size::new(100.0, CHIP_HEIGHT), None);
            assert!(rec.rrects.is_empty());
            assert_eq!(rec.strokes[0].2, CHIP_OUTLINE_VARIANT);
        }

        #[test]
        fn elevated_paints_elevated_container_and_shadow() {
            let view =
                suggestion_chip::<Presses, _>("Try this", |_s: &mut Presses| {}).elevated(true);
            let mut counter = 0u64;
            let mut w = View::<Presses>::build(&view, &mut BuildCtx::new(&mut counter));
            let rec = paint_rec(&mut w, Size::new(100.0, CHIP_HEIGHT), None);
            assert_eq!(rec.rrects[0].3, CHIP_ELEVATED_CONTAINER);
            assert_eq!(rec.shadows.len(), 1);
        }

        #[test]
        fn semantics_reports_button_role_and_label() {
            use frust::authoring::text::TextContext;
            use std::any::Any;

            fn logic(_s: &mut Presses) -> SuggestionChipView<Presses> {
                suggestion_chip::<Presses, _>("Try this", |_s: &mut Presses| {})
            }
            let mut root: frust_core::RenderRoot<Presses, SuggestionChipView<Presses>> =
                frust_core::RenderRoot::new();
            let mut state = Presses::default();
            root.rebuild(&mut logic, &mut state);
            let mut tcx = TextContext::new();
            root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
            let update = root.semantics();
            let (_, node) = update
                .nodes
                .iter()
                .find(|(_, n)| n.role() == Role::Button)
                .expect("suggestion chip contributes a Role::Button node");
            assert_eq!(node.label(), Some("Try this"));
            assert!(node.supports_action(Action::Click));
        }
    }

    // --- InputChip ---

    mod input {
        use super::*;

        #[derive(Default)]
        struct Log {
            presses: u32,
            deletes: u32,
        }

        fn widget_no_delete() -> InputChipWidget {
            let view = input_chip::<Log, _>("Tag", |s: &mut Log| s.presses += 1);
            let mut counter = 0u64;
            View::<Log>::build(&view, &mut BuildCtx::new(&mut counter))
        }

        fn widget_with_delete() -> InputChipWidget {
            let view = input_chip::<Log, _>("Tag", |s: &mut Log| s.presses += 1)
                .on_deleted(|s: &mut Log| s.deletes += 1);
            let mut counter = 0u64;
            View::<Log>::build(&view, &mut BuildCtx::new(&mut counter))
        }

        /// Give the widget a known delete rect without a text-context
        /// layout — mirrors `split_button`'s `with_rects` test helper.
        fn with_delete_rect(w: &mut InputChipWidget) {
            w.delete_rect = Rect::new(70.0, 7.0, 88.0, 25.0);
        }

        fn dispatch(w: &mut InputChipWidget, state: &mut Log, event: &InputEvent) {
            let state_any: &mut dyn Any = state;
            let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, CHIP_HEIGHT));
            w.event(&mut ctx, event);
        }

        #[test]
        fn no_delete_configured_means_no_hit_region_and_body_press_fires() {
            let mut w = widget_no_delete();
            assert!(!w.has_delete);
            let mut state = Log::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
            assert_eq!(state.presses, 1);
            assert_eq!(state.deletes, 0);
        }

        #[test]
        fn body_press_fires_on_press_only() {
            let mut w = widget_with_delete();
            with_delete_rect(&mut w);
            let mut state = Log::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            assert_eq!(w.armed, Some(InputRegion::Body));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 12.0));
            assert_eq!(state.presses, 1);
            assert_eq!(state.deletes, 0, "a body press must not fire on_deleted");
        }

        #[test]
        fn delete_press_fires_on_deleted_only() {
            let mut w = widget_with_delete();
            with_delete_rect(&mut w);
            let mut state = Log::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 75.0, 12.0));
            assert_eq!(w.armed, Some(InputRegion::Delete));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 75.0, 12.0));
            assert_eq!(state.deletes, 1);
            assert_eq!(state.presses, 0, "a delete press must not fire on_press");
        }

        #[test]
        fn delete_press_does_not_arm_the_state_layer() {
            let mut w = widget_with_delete();
            with_delete_rect(&mut w);
            let mut state = Log::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 75.0, 12.0));
            assert!(
                !w.state_layer.is_active(),
                "the delete icon carries no state-layer feedback (see the module docs)"
            );
        }

        #[test]
        fn body_press_does_arm_the_state_layer() {
            let mut w = widget_with_delete();
            with_delete_rect(&mut w);
            let mut state = Log::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            assert!(w.state_layer.is_active());
        }

        #[test]
        fn release_outside_the_armed_region_fires_nothing() {
            let mut w = widget_with_delete();
            with_delete_rect(&mut w);
            let mut state = Log::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 12.0));
            assert_eq!(state.presses, 0);
            assert_eq!(state.deletes, 0);
        }

        #[test]
        fn cancel_disarms_without_firing() {
            let mut w = widget_with_delete();
            with_delete_rect(&mut w);
            let mut state = Log::default();
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 75.0, 12.0));
            dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 75.0, 12.0));
            assert!(w.armed.is_none());
            assert_eq!(state.deletes, 0);
        }

        #[test]
        fn down_outside_both_regions_is_ignored() {
            let mut w = widget_with_delete();
            with_delete_rect(&mut w);
            let mut state = Log::default();
            let state_any: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, CHIP_HEIGHT));
            let result = w.event(&mut ctx, &ev(PointerPhase::Down, 300.0, 12.0));
            assert!(matches!(result, EventResult::Ignored));
            assert!(w.armed.is_none());
        }

        #[test]
        fn layout_sizes_the_delete_icon_at_18dp_after_the_label() {
            use frust::authoring::text::TextContext;

            let view = input_chip::<Log, _>("Tag", |_s: &mut Log| {}).on_deleted(|_s: &mut Log| {});
            let mut counter = 0u64;
            let mut w = View::<Log>::build(&view, &mut BuildCtx::new(&mut counter));
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let total = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(400.0, 100.0)));

            assert_eq!(w.delete_rect.width(), CHIP_ICON_SIZE);
            assert_eq!(w.delete_rect.height(), CHIP_ICON_SIZE);
            // The delete icon's right edge, plus the end padding, is the
            // chip's total width.
            assert_eq!(w.delete_rect.x1 + CHIP_END_PAD, total.width);
            assert_eq!(total.height, CHIP_HEIGHT);
        }

        #[test]
        fn layout_without_delete_has_a_zero_rect_and_no_trailing_icon_gap() {
            use frust::authoring::text::TextContext;

            let with_delete =
                input_chip::<Log, _>("Tag", |_s: &mut Log| {}).on_deleted(|_s: &mut Log| {});
            let without_delete = input_chip::<Log, _>("Tag", |_s: &mut Log| {});
            let mut counter = 0u64;
            let mut w_with = View::<Log>::build(&with_delete, &mut BuildCtx::new(&mut counter));
            let mut w_without =
                View::<Log>::build(&without_delete, &mut BuildCtx::new(&mut counter));
            let mut tcx = TextContext::new();
            let mut lctx1 = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let size_with =
                w_with.layout(&mut lctx1, &BoxConstraints::loose(Size::new(400.0, 100.0)));
            let mut lctx2 = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            let size_without =
                w_without.layout(&mut lctx2, &BoxConstraints::loose(Size::new(400.0, 100.0)));

            assert_eq!(w_without.delete_rect, Rect::ZERO);
            assert!(
                size_with.width > size_without.width,
                "a delete icon widens the chip"
            );
        }

        #[test]
        fn paints_a_close_glyph_fill_at_the_delete_rect_origin() {
            let mut w = widget_with_delete();
            with_delete_rect(&mut w);
            let rec = paint_rec(&mut w, Size::new(100.0, CHIP_HEIGHT), None);
            let (origin, color) = rec
                .fills
                .last()
                .copied()
                .expect("the delete icon paints one filled path");
            assert_eq!(origin, Point::new(70.0, 7.0));
            assert_eq!(color, CHIP_LABEL_UNSELECTED);
        }

        #[test]
        fn rebuild_dropping_on_deleted_disarms_a_pending_delete_press() {
            let mut counter = 0u64;
            let prev = input_chip::<Log, _>("Tag", |_s: &mut Log| {}).on_deleted(|_s: &mut Log| {});
            let mut w = View::<Log>::build(&prev, &mut BuildCtx::new(&mut counter));
            with_delete_rect(&mut w);
            w.armed = Some(InputRegion::Delete);
            w.pressed_inside = true;

            let next = input_chip::<Log, _>("Tag", |_s: &mut Log| {});
            let flags =
                View::<Log>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
            assert!(!w.has_delete);
            assert!(w.armed.is_none());
            assert!(!w.pressed_inside);
            assert!(flags.needs_layout());
        }

        #[test]
        fn semantics_with_delete_reports_a_group_with_two_buttons() {
            fn logic(_s: &mut ()) -> InputChipView<()> {
                input_chip::<(), _>("Tag", |_s: &mut ()| {}).on_deleted(|_s: &mut ()| {})
            }
            let mut root: frust_core::RenderRoot<(), InputChipView<()>> =
                frust_core::RenderRoot::new();
            let mut state = ();
            root.rebuild(&mut logic, &mut state);
            let mut tcx = frust::authoring::text::TextContext::new();
            root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
            let update = root.semantics();

            let group = update
                .nodes
                .iter()
                .find(|(_, n)| n.role() == Role::Group)
                .expect("a Group container node is contributed");
            assert_eq!(group.1.children().len(), 2);

            let label_node = update
                .nodes
                .iter()
                .find(|(_, n)| n.label() == Some("Tag"))
                .expect("the label button node is present");
            assert!(label_node.1.supports_action(Action::Click));

            let delete_node = update
                .nodes
                .iter()
                .find(|(_, n)| n.label() == Some("Remove"))
                .expect("the delete button node is present");
            assert!(delete_node.1.supports_action(Action::Click));
        }

        #[test]
        fn semantics_without_delete_reports_a_single_button() {
            fn logic(_s: &mut ()) -> InputChipView<()> {
                input_chip::<(), _>("Tag", |_s: &mut ()| {})
            }
            let mut root: frust_core::RenderRoot<(), InputChipView<()>> =
                frust_core::RenderRoot::new();
            let mut state = ();
            root.rebuild(&mut logic, &mut state);
            let mut tcx = frust::authoring::text::TextContext::new();
            root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
            let update = root.semantics();
            assert!(
                update.nodes.iter().all(|(_, n)| n.role() != Role::Group),
                "no delete affordance means no Group container"
            );
            let (_, node) = update
                .nodes
                .iter()
                .find(|(_, n)| n.role() == Role::Button)
                .expect("input chip without delete contributes a single Role::Button node");
            assert_eq!(node.label(), Some("Tag"));
        }
    }
}
