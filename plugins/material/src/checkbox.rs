//! The M3(E) `Checkbox`: a controlled component supporting the binary
//! (`Some(bool)`) and, via [`tristate_checkbox`], the tristate
//! (`Option<bool>`, `None` = indeterminate) shapes, plus an `error` flavor —
//! an 18dp box with a 40dp state-layer halo, animated container fill/border
//! and check-mark draw.
//!
//! Ported from `material_3_expressive` v1.0.8's `M3ECheckbox`/
//! `M3ECheckboxTheme` (MIT, © 2026 Paa Developments;
//! `tmp/material_3_expressive/lib/components/checkbox/m3e_checkbox.dart`,
//! `styles/m3e_checkbox_theme.dart`, retrieved 2026-08-19).
//!
//! # Naming: plain `Checkbox`, mirroring `switch`'s cross-crate precedent
//!
//! Baseline `frust-widgets` already ships a labelled `Checkbox`/`checkbox`
//! (`frust::Checkbox`) — but a plugin crate's flat re-export lives in its
//! *own* namespace (`frust_material::Checkbox`), never the facade's, so the
//! two coexist without collision the same way [`super::switch::Switch`] is
//! named plainly despite `frust::Switch` existing in spirit (baseline ships
//! no `Switch` at all, but the crate's stated policy — this module doc's own
//! decision record — is the same regardless: a design-system catalog names
//! its widgets after the thing they are, not after avoiding a same-named
//! symbol one import-path over). `MaterialCheckbox` was considered and
//! rejected as needless stutter (`frust_material::MaterialCheckbox` reads no
//! more clearly than `frust_material::Checkbox` at any real call site, which
//! always qualifies through the crate path or a `use` alias already).
//!
//! # Controlled component, tap-cycle order
//!
//! [`checkbox`] (binary) and [`tristate_checkbox`] both produce a
//! [`CheckboxView`] that reports the *requested* next value through
//! `on_changed` and never self-mutates — the app mutates its state and the
//! next `rebuild` feeds the confirmed value back in, exactly like
//! [`super::switch::switch`]/[`frust::Checkbox`]. The release-inside-bounds
//! cycle order is [`next_value`], pinned exactly against the reference's
//! `M3ECheckbox._handleTap`: `Some(false) -> Some(true)`; `Some(true) ->`
//! `tristate ? None : Some(false)`; `None -> Some(false)`.
//!
//! # No disabled state
//!
//! The reference's `fillColor`/`borderColor` theme methods carry an
//! `enabled` branch (an `onSurface`-at-`disabledOpacity` fill/border). This
//! port carries only the `enabled: true` semantics — mirroring every sibling
//! controlled toggle in this crate ([`super::switch`], `filter_chip`,
//! baseline `frust::Checkbox`), none of which model a disabled state yet —
//! so [`resolve_colors`] has no `enabled` parameter and the Dart source's
//! `disabledOpacity` token has no counterpart here.
//!
//! # Check-mark draw: a deliberate enhancement over the reference
//!
//! The reference wraps its box in an `AnimatedContainer` (`duration:`
//! [`MaterialMotion::SHORT_3`], `curve:` [`MaterialMotion::STANDARD`]) that
//! tweens the container's own fill/border — but the mark *inside* it
//! (`_buildMark`'s `Icon`/dash) is an instant, un-animated widget swap;
//! Flutter's implicit-animation machinery only tweens a `Container`'s own
//! decoration properties, never a child-identity change. This port instead
//! path-draws the check mark as a two-segment stroke ([`check_mark_path`])
//! and grows the indeterminate dash from its center, both revealed over the
//! *same* `0.0..=1.0` progress already driving the box's fill/border lerp —
//! a single [`frust::AnimationController`] paces all of it, advanced during
//! [`Widget::paint`] and re-requested via [`PaintCtx::request_frame`] while
//! in flight, per `docs/CODE_STANDARDS.md`'s Theming & Animation
//! Conventions. Unlike [`super::switch`]'s spring (which resolves from
//! `Theme::motion` and so must wait for a themed paint pass to start), this
//! crate's [`MaterialMotion`] duration/curve tokens are plain constants
//! needing no theme — but the transition is still started lazily in `paint`
//! (comparing the live `is_active` value against `anim_target`) rather than
//! in `rebuild`, for consistency with that established pattern. A freshly
//! built already-checked/indeterminate checkbox settles instantly with no
//! mount-time fade-in (see [`View::build`]'s doc), matching the reference's
//! own `ImplicitlyAnimatedWidget` semantics (a first build's implicit tween
//! has equal begin/end endpoints).
//!
//! # State-layer halo: the interaction core's precedence resolver
//!
//! The 40dp halo uses [`InteractionState::resolve_opacity`] (`dragged >`
//! `pressed > focused > hovered`) rather than [`super::state_layer::StateLayer`]'s
//! max-of-active resolver — this crate's new documented default (see
//! [`crate::interaction`]'s module docs) — with hover claimed from the
//! widget's own uncaptured `Move` arm and press captured on `Down`, mirroring
//! [`super::list_item`]'s reference implementation of the hover-claim
//! contract.

use std::rc::Rc;

use frust::authoring::{
    Action, BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, PointerPhase, Role, SemanticsCtx, Toggled, View, Widget,
};
use frust::{AnimationController, FrameTime, Theme, Tween};
use kurbo::{BezPath, Point, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use super::press::presses;
use crate::interaction::InteractionState;
use crate::tokens::MaterialMotion;

/// The full interactive hit target, in logical px (`M3ECheckboxTheme.hitSize`)
/// — independent of the smaller [`BOX`] visual box.
const HIT_SIZE: f64 = 40.0;
/// Side length of the visual box, in logical px (`M3ECheckboxTheme.boxSize`).
const BOX: f64 = 18.0;
/// The box's local top-left offset within the [`HIT_SIZE`] hit target (the
/// box sits centered).
const BOX_OFFSET: f64 = (HIT_SIZE - BOX) / 2.0;
/// Diameter of the state-layer halo — the same [`HIT_SIZE`] as the hit
/// target (`M3ECheckboxTheme.hitSize` doubles as both).
const STATE_LAYER_SIZE: f64 = HIT_SIZE;
/// Side length of the check-mark glyph within the box, in logical px
/// (`M3ECheckboxTheme.markSize`).
const MARK_SIZE: f64 = 16.0;
/// The mark's local top-left offset within the hit target (centered inside
/// the box, which is itself centered inside the hit target).
const MARK_OFFSET: f64 = BOX_OFFSET + (BOX - MARK_SIZE) / 2.0;
/// Width of the indeterminate dash, in logical px
/// (`M3ECheckboxTheme.indeterminateWidth`).
const INDETERMINATE_WIDTH: f64 = 10.0;
/// Height of the indeterminate dash, in logical px
/// (`M3ECheckboxTheme.indeterminateHeight`).
const INDETERMINATE_HEIGHT: f64 = 2.0;
/// Box border stroke width, in logical px (`M3ECheckboxTheme.borderWidth`).
const BORDER_WIDTH: f64 = 2.0;
/// Check-mark stroke width, in logical px — an authored value: the
/// reference paints the mark as a filled `Icon` glyph, not a stroke, so
/// there is no upstream stroke width to carry over (see the module docs'
/// Check-mark draw section).
const CHECK_STROKE_WIDTH: f64 = 2.0;
/// Unthemed-fallback box corner radius (a theme resolves this from
/// `shape.extra_small`; the reference's `borderRadius` resolves
/// `M3EShapes.radiusExtraSmall`, the same 4dp token).
const RADIUS: f64 = 4.0;
/// `kurbo::Shape::to_path` flattening tolerance for the box's stroked border
/// outline (see [`super::card`]'s identical precedent).
const PATH_TOLERANCE: f64 = 0.1;

/// Unthemed-fallback active fill/border (a theme resolves this from
/// `colors.primary`) — the same placeholder blue [`super::switch::SwitchWidget`]'s
/// `TRACK_ON` uses.
const FILL_ACTIVE: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
/// Unthemed-fallback mark color on a non-error active fill (a theme resolves
/// this from `colors.onPrimary`).
const ON_PRIMARY: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
/// Unthemed-fallback inactive border (a theme resolves this from
/// `colors.onSurfaceVariant`).
const ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x6B, 0x72, 0x80);
/// Unthemed-fallback state-layer base when neither active nor error (a theme
/// resolves this from `colors.onSurface`) — the same placeholder gray-900
/// [`super::chips`]'s `CHIP_LABEL` (`colors.onSurface`) uses.
const ON_SURFACE: Color = Color::from_rgb8(0x11, 0x18, 0x27);
/// Unthemed-fallback error fill/border (a theme resolves this from
/// `colors.error`).
const ERROR: Color = Color::from_rgb8(0xEF, 0x44, 0x44);
/// Unthemed-fallback mark color on an error fill (a theme resolves this from
/// `colors.onError`).
const ON_ERROR: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

/// Whether the box/state-layer render in their "on" tone — checked, or
/// indeterminate. Mirrors the reference's `active = value == null || checked`
/// (`value == null` being this port's `None`).
fn is_active(value: Option<bool>) -> bool {
    !matches!(value, Some(false))
}

/// The next value a release-inside-bounds reports, given the current `value`
/// and whether `tristate` is enabled. Ports `M3ECheckbox._handleTap`'s exact
/// switch — see the module docs.
fn next_value(value: Option<bool>, tristate: bool) -> Option<bool> {
    match value {
        Some(false) => Some(true),
        Some(true) => {
            if tristate {
                None
            } else {
                Some(false)
            }
        }
        None => Some(false),
    }
}

/// The theme-resolved colors [`Widget::paint`] lerps/reads from — ports
/// `M3ECheckboxTheme.fillColor`/`borderColor`/`markColor`/`stateLayerColor`
/// (`enabled: true` only; see the module docs).
struct ResolvedColors {
    /// `error ? colors.error : colors.primary` — the box fill/border's
    /// `progress == 1.0` end, and the state-layer base whenever `active` or
    /// `error`.
    active: Color,
    /// `error ? colors.error : colors.onSurfaceVariant` — the box border's
    /// `progress == 0.0` end.
    border_inactive: Color,
    /// `colors.onSurface` — the state-layer base when neither `active` nor
    /// `error`.
    on_surface: Color,
    /// `error ? colors.onError : colors.onPrimary` — the check-mark/dash
    /// color (untweened; the reference's `markColor` doesn't key on
    /// `active` at all, only `error`).
    mark: Color,
}

/// Resolve [`ResolvedColors`]. Themed: `colors.primary`/`error`/
/// `onSurfaceVariant`/`onSurface`/`onPrimary`/`onError`. Unthemed: this
/// module's own fallback constants exactly.
fn resolve_colors(theme: Option<&Theme>, error: bool) -> ResolvedColors {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            ResolvedColors {
                active: if error { s.error } else { s.primary },
                border_inactive: if error { s.error } else { s.on_surface_variant },
                on_surface: s.on_surface,
                mark: if error { s.on_error } else { s.on_primary },
            }
        }
        None => ResolvedColors {
            active: if error { ERROR } else { FILL_ACTIVE },
            border_inactive: if error { ERROR } else { ON_SURFACE_VARIANT },
            on_surface: ON_SURFACE,
            mark: if error { ON_ERROR } else { ON_PRIMARY },
        },
    }
}

/// The state-layer halo's base color: ports `M3ECheckboxTheme.stateLayerColor`
/// exactly (`error` wins outright; otherwise `active ? primary : onSurface`).
fn state_layer_base(colors: &ResolvedColors, active: bool, error: bool) -> Color {
    if error || active {
        colors.active
    } else {
        colors.on_surface
    }
}

/// The box corner radius. Themed: `shape.extra_small`. Unthemed: [`RADIUS`]
/// exactly.
fn resolve_radius(theme: Option<&Theme>) -> f64 {
    theme.map_or(RADIUS, |t| t.shape.extra_small)
}

/// Interpolate from `begin` to `end` at `t`, snapping exactly to an endpoint
/// when `t` is at (or past) `0.0`/`1.0` — see [`super::switch`]'s identical
/// helper for why this matters (an at-rest box must paint pixel-identical to
/// its resting color, never a few-ULPs-off `Tween::lerp` result).
fn lerp_color_exact(begin: Color, end: Color, t: f64) -> Color {
    if t <= 0.0 {
        begin
    } else if t >= 1.0 {
        end
    } else {
        Tween::new(begin, end).lerp(t)
    }
}

/// Return `color` with its alpha channel replaced by `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Paint the state-layer halo: a `diameter`-wide filled circle centered at
/// `center`, in `base_color` at `state`'s
/// [precedence-resolved](InteractionState::resolve_opacity) opacity. A
/// no-op when no interaction state is active — see the module docs for why
/// this uses the precedence resolver rather than
/// [`super::state_layer::StateLayer`]'s max-of-active one.
fn paint_state_layer(
    scene: &mut dyn PaintScene,
    center: Point,
    diameter: f64,
    state: InteractionState,
    base_color: Color,
) {
    let opacity = state.resolve_opacity();
    if opacity <= 0.0 {
        return;
    }
    let r = diameter / 2.0;
    scene.fill_rounded_rect(
        Point::new(center.x - r, center.y - r),
        Size::new(diameter, diameter),
        r,
        with_alpha(base_color, opacity),
    );
}

/// The check mark's two-segment polyline within a `mark_size`×`mark_size`
/// box anchored at local `origin` — proportions mirror the baseline
/// `frust::Checkbox`'s hand-authored check mark
/// (`crates/frust-widgets/src/checkbox.rs`), rescaled from its 20dp box to
/// this component's 16dp `markSize`. `progress` (`0.0..=1.0`) reveals the
/// stroke from its first point onward: `<= 0.0` yields an empty path, `1.0`
/// the complete two-segment mark. See the module docs' Check-mark draw
/// section for why this animated reveal is a deliberate enhancement over
/// the reference's instant, un-animated mark swap.
fn check_mark_path(origin: Point, mark_size: f64, progress: f64) -> BezPath {
    let mut path = BezPath::new();
    if progress <= 0.0 {
        return path;
    }
    let p0 = Point::new(origin.x + mark_size * 0.24, origin.y + mark_size * 0.52);
    let p1 = Point::new(origin.x + mark_size * 0.42, origin.y + mark_size * 0.70);
    let p2 = Point::new(origin.x + mark_size * 0.76, origin.y + mark_size * 0.30);
    let l1 = p0.distance(p1);
    let l2 = p1.distance(p2);
    let total = l1 + l2;
    if total <= 0.0 {
        return path;
    }
    path.move_to(p0);
    let drawn = progress.min(1.0) * total;
    if drawn <= l1 {
        let t = if l1 > 0.0 { drawn / l1 } else { 1.0 };
        path.line_to(p0.lerp(p1, t));
    } else {
        path.line_to(p1);
        let t = if l2 > 0.0 {
            ((drawn - l1) / l2).min(1.0)
        } else {
            1.0
        };
        path.line_to(p1.lerp(p2, t));
    }
    path
}

/// Whether local point `pos` (relative to the widget's own origin) lies
/// inside `size`.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// A view-held, typed change callback (erased on build) — always
/// `Option<bool>` internally; [`checkbox`]'s binary wrapper unwraps it back
/// to `bool` for its own callback signature (`None` never reaches a
/// non-tristate caller — see [`next_value`]).
type OnChanged<State> = Rc<dyn Fn(&mut State, Option<bool>)>;

/// A declarative M3(E) checkbox. See the [module docs](self).
pub struct CheckboxView<State: 'static> {
    value: Option<bool>,
    tristate: bool,
    error: bool,
    on_changed: OnChanged<State>,
}

/// Create a binary checkbox reflecting `checked`, that fires
/// `on_changed(state, !checked)` on release inside its bounds. Mirrors
/// [`super::switch::switch`]/[`frust::Checkbox`]: a controlled component
/// that never flips its own value.
pub fn checkbox<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_changed: F,
) -> CheckboxView<State> {
    CheckboxView {
        value: Some(checked),
        tristate: false,
        error: false,
        on_changed: Rc::new(move |state, next| on_changed(state, next.unwrap_or(false))),
    }
}

/// PascalCase alias for [`checkbox`].
#[allow(non_snake_case)]
pub fn Checkbox<State: 'static, F: Fn(&mut State, bool) + 'static>(
    checked: bool,
    on_changed: F,
) -> CheckboxView<State> {
    checkbox(checked, on_changed)
}

/// Create a tristate checkbox reflecting `value` (`None` = indeterminate,
/// rendered as a dash), that fires `on_changed` with the reference's exact
/// tap-cycle order on release inside its bounds — see [`next_value`].
pub fn tristate_checkbox<State: 'static, F: Fn(&mut State, Option<bool>) + 'static>(
    value: Option<bool>,
    on_changed: F,
) -> CheckboxView<State> {
    CheckboxView {
        value,
        tristate: true,
        error: false,
        on_changed: Rc::new(on_changed),
    }
}

impl<State: 'static> CheckboxView<State> {
    /// Recolor the box/border/mark into the error flavor (`colors.error`/
    /// `colors.onError` in place of `colors.primary`/`colors.onPrimary`) —
    /// mirrors the reference's `M3ECheckbox.error` flag
    /// (`m3e_checkbox_theme.dart`'s `fillColor`/`borderColor`/`markColor`/
    /// `stateLayerColor`, each keying on `error` first).
    pub fn error(mut self, error: bool) -> Self {
        self.error = error;
        self
    }
}

/// The retained widget for a [`CheckboxView`].
pub struct CheckboxWidget {
    /// The app-confirmed value (source of truth; adopted on `rebuild`).
    /// `None` is the indeterminate state (only reachable when `tristate`).
    value: Option<bool>,
    tristate: bool,
    error: bool,
    /// Drives the box/mark `0.0` (inactive) .. `1.0` (active) reveal
    /// fraction. See the module docs' Check-mark draw section.
    anim: AnimationController,
    /// The [`is_active`] value the animation is currently driving toward (or
    /// has already settled at) — compared against the live value at paint
    /// time to decide whether a fresh transition needs to start (mirrors
    /// [`super::switch::SwitchWidget::anim_target`]).
    anim_target: bool,
    interaction: InteractionState,
    /// The pressed *visual* state; follows the cursor in/out while captured.
    pressed: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on `Up`/`Cancel`.
    captured: bool,
    on_changed: frust::authoring::ErasedArgCallback<Option<bool>>,
}

/// Build a duration+curve [`AnimationController`] already **settled** at
/// `1.0` if `active`, `0.0` otherwise — no mount-time fade-in (see the
/// module docs). A single `animate_to` call can't itself snap a non-zero-duration
/// controller (only a zero-duration one settles instantly — see
/// `AnimationController::animate_to`'s doc), and a lone `advance` call right
/// after can't either: the first `advance` after starting a motion always
/// reports a zero delta, since it only seeds the clock (see
/// `AnimationController::advance`'s doc). Two throwaway `advance` calls
/// settle it correctly instead: the first seeds `last_time`, the second —
/// far enough ahead — reports an elapsed exceeding the duration.
fn seeded_anim(active: bool) -> AnimationController {
    let mut anim =
        AnimationController::new(MaterialMotion::SHORT_3).with_curve(MaterialMotion::STANDARD);
    if active {
        anim.animate_to(1.0);
        anim.advance(FrameTime::ZERO);
        anim.advance(FrameTime::from_nanos(
            MaterialMotion::SHORT_3.as_nanos() as u64 * 2,
        ));
    }
    anim
}

impl<State: 'static> View<State> for CheckboxView<State> {
    type Element = CheckboxWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CheckboxWidget {
        let active = is_active(self.value);
        CheckboxWidget {
            value: self.value,
            tristate: self.tristate,
            error: self.error,
            anim: seeded_anim(active),
            anim_target: active,
            interaction: InteractionState::new(),
            pressed: false,
            captured: false,
            on_changed: frust::authoring::erase_callback_arg(&self.on_changed),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CheckboxWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_changed = frust::authoring::erase_callback_arg(&self.on_changed);
        let mut flags = ChangeFlags::NONE;
        if prev.value != self.value || prev.tristate != self.tristate || prev.error != self.error {
            // The app is the source of truth: adopt the new value on
            // rebuild. The transition itself starts lazily in `paint` (see
            // the module docs' Check-mark draw section).
            element.value = self.value;
            element.tristate = self.tristate;
            element.error = self.error;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for CheckboxWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(HIT_SIZE, HIT_SIZE))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Resolved before the animation advance/request_frame block below —
        // `theme` borrows `ctx` immutably, so every read through it must
        // finish before `ctx.request_frame()`'s mutable borrow (mirrors
        // `switch.rs`'s identical ordering).
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme, self.error);
        let radius = resolve_radius(theme);

        let active = is_active(self.value);
        if active != self.anim_target {
            self.anim.animate_to(if active { 1.0 } else { 0.0 });
            self.anim_target = active;
        }
        if self.anim.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        let progress = self.anim.value_clamped();

        // The pod's hover link is authoritative; re-synced every paint (see
        // `list_item`'s identical pattern, cited in the module docs).
        self.interaction.set_hovered(ctx.is_hovered());

        let o = ctx.origin();
        let center = Point::new(o.x + HIT_SIZE / 2.0, o.y + HIT_SIZE / 2.0);
        let layer_base = state_layer_base(&colors, active, self.error);
        paint_state_layer(
            scene,
            center,
            STATE_LAYER_SIZE,
            self.interaction,
            layer_base,
        );

        let box_abs = Point::new(o.x + BOX_OFFSET, o.y + BOX_OFFSET);
        let fill = lerp_color_exact(Color::TRANSPARENT, colors.active, progress);
        scene.fill_rounded_rect(box_abs, Size::new(BOX, BOX), radius, fill);

        let half = BORDER_WIDTH / 2.0;
        let border = lerp_color_exact(colors.border_inactive, colors.active, progress);
        let border_rect = RoundedRect::new(
            BOX_OFFSET + half,
            BOX_OFFSET + half,
            BOX_OFFSET + BOX - half,
            BOX_OFFSET + BOX - half,
            (radius - half).max(0.0),
        );
        scene.stroke_path(
            o,
            &border_rect.to_path(PATH_TOLERANCE),
            BORDER_WIDTH,
            &Brush::Solid(border),
        );

        if progress > 0.0 {
            match self.value {
                Some(true) => {
                    let mark_origin = Point::new(MARK_OFFSET, MARK_OFFSET);
                    let path = check_mark_path(mark_origin, MARK_SIZE, progress);
                    scene.stroke_path(o, &path, CHECK_STROKE_WIDTH, &Brush::Solid(colors.mark));
                }
                None if self.tristate => {
                    let w = INDETERMINATE_WIDTH * progress.min(1.0);
                    let rect_origin = Point::new(
                        o.x + BOX_OFFSET + (BOX - w) / 2.0,
                        o.y + BOX_OFFSET + (BOX - INDETERMINATE_HEIGHT) / 2.0,
                    );
                    scene.fill_rect(rect_origin, Size::new(w, INDETERMINATE_HEIGHT), colors.mark);
                }
                _ => {}
            }
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
                self.pressed = true;
                self.captured = true;
                self.interaction.set_pressed(true);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    // No capture: this is the hover pass — claim whenever
                    // the pointer is inside, mirroring `list_item`'s
                    // reference hover-claim implementation (see the module
                    // docs).
                    let over = inside(p.position, ctx.size());
                    if over {
                        ctx.claim_hover();
                    }
                    if self.interaction.set_hovered(over) {
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
                let inside_now = inside(p.position, ctx.size());
                self.pressed = inside_now;
                self.interaction.set_pressed(inside_now);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    // Report the *requested* value; never self-toggle.
                    let next = next_value(self.value, self.tristate);
                    (self.on_changed)(ctx, next);
                }
                self.pressed = false;
                self.captured = false;
                self.interaction.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                self.interaction.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A single CheckBox node; `Toggled::Mixed` reports the
        // indeterminate state (mirrors baseline `frust::Checkbox`'s
        // Role::CheckBox + Toggled convention, extended with the tristate
        // third value accesskit's own `Toggled` enum already carries).
        ctx.push_node(Role::CheckBox, |node| {
            node.set_toggled(match self.value {
                Some(true) => Toggled::True,
                Some(false) => Toggled::False,
                None => Toggled::Mixed,
            });
            node.add_action(Action::Click);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    // ---- Pure logic: is_active / next_value ------------------------------

    #[test]
    fn is_active_matches_reference_active_formula() {
        assert!(!is_active(Some(false)));
        assert!(is_active(Some(true)));
        assert!(is_active(None), "indeterminate is active");
    }

    #[test]
    fn next_value_binary_cycle_matches_reference_handle_tap() {
        assert_eq!(next_value(Some(false), false), Some(true));
        assert_eq!(next_value(Some(true), false), Some(false));
    }

    #[test]
    fn next_value_tristate_cycle_matches_reference_handle_tap() {
        assert_eq!(next_value(Some(false), true), Some(true));
        assert_eq!(next_value(Some(true), true), None);
        assert_eq!(next_value(None, true), Some(false));
    }

    // ---- check_mark_path ---------------------------------------------------

    #[test]
    fn check_mark_path_is_empty_at_zero_progress() {
        let path = check_mark_path(Point::ZERO, MARK_SIZE, 0.0);
        assert!(path.elements().is_empty());
    }

    #[test]
    fn check_mark_path_has_two_segments_at_full_progress() {
        let path = check_mark_path(Point::ZERO, MARK_SIZE, 1.0);
        // MoveTo + two LineTo.
        assert_eq!(path.elements().len(), 3);
    }

    #[test]
    fn check_mark_path_partial_progress_stops_along_the_first_segment() {
        let path = check_mark_path(Point::ZERO, MARK_SIZE, 0.1);
        // A very small progress hasn't reached the first vertex yet: one
        // MoveTo + one partial LineTo, not the full two-segment path.
        assert_eq!(path.elements().len(), 2);
    }

    // ---- View/Widget plumbing ---------------------------------------------

    #[derive(Default)]
    struct ChangeState {
        last: Option<Option<bool>>,
        changes: u32,
    }

    fn binary_widget(checked: bool) -> CheckboxWidget {
        let view = checkbox::<ChangeState, _>(checked, |s: &mut ChangeState, v: bool| {
            s.last = Some(Some(v));
            s.changes += 1;
        });
        let mut counter = 0u64;
        View::<ChangeState>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn tristate_widget(value: Option<bool>) -> CheckboxWidget {
        let view =
            tristate_checkbox::<ChangeState, _>(value, |s: &mut ChangeState, v: Option<bool>| {
                s.last = Some(v);
                s.changes += 1;
            });
        let mut counter = 0u64;
        View::<ChangeState>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut CheckboxWidget, state: &mut ChangeState, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(HIT_SIZE, HIT_SIZE));
        w.event(&mut ctx, event);
    }

    #[test]
    fn layout_is_always_the_forty_dp_hit_target_regardless_of_the_box() {
        // HIT_SIZE (40dp) > BOX (18dp) is a compile-time fact of these two
        // constants, asserted structurally here rather than at runtime: the
        // hit target really is layout()'s returned size, strictly larger
        // than the 18dp visual box painted inside it.
        let mut w = binary_widget(false);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)));
        assert_eq!(size, Size::new(HIT_SIZE, HIT_SIZE));
        assert!(size.width >= 40.0 && size.height >= 40.0);
        assert!(size.width > BOX && size.height > BOX);
    }

    #[test]
    fn binary_unchecked_fires_true_and_does_not_self_mutate() {
        let mut w = binary_widget(false);
        let mut state = ChangeState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 20.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 20.0, 20.0));
        assert_eq!(state.last, Some(Some(true)));
        assert_eq!(state.changes, 1);
        assert_eq!(w.value, Some(false), "must not mutate its own value");
    }

    #[test]
    fn binary_checked_fires_false() {
        let mut w = binary_widget(true);
        let mut state = ChangeState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 20.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 20.0, 20.0));
        assert_eq!(state.last, Some(Some(false)));
    }

    #[test]
    fn tristate_checked_fires_indeterminate() {
        let mut w = tristate_widget(Some(true));
        let mut state = ChangeState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 20.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 20.0, 20.0));
        assert_eq!(state.last, Some(None));
    }

    #[test]
    fn tristate_indeterminate_fires_false() {
        let mut w = tristate_widget(None);
        let mut state = ChangeState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 20.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 20.0, 20.0));
        assert_eq!(state.last, Some(Some(false)));
    }

    #[test]
    fn up_outside_does_not_fire() {
        let mut w = binary_widget(false);
        let mut state = ChangeState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 20.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 500.0, 20.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 500.0, 20.0));
        assert_eq!(state.changes, 0);
    }

    #[test]
    fn hover_move_without_down_is_ignored_noop() {
        let mut w = binary_widget(false);
        let mut state = ChangeState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(HIT_SIZE, HIT_SIZE));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 20.0, 20.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!w.pressed);
        assert_eq!(state.changes, 0);
    }

    #[test]
    fn hover_move_inside_claims_hover_and_requests_redraw_once() {
        let mut w = binary_widget(false);
        let mut state = ChangeState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(HIT_SIZE, HIT_SIZE));
        w.event(&mut ctx, &ev(PointerPhase::Move, 20.0, 20.0));
        assert!(ctx.needs_redraw(), "hover gain must request a redraw");
        assert!(w.interaction.hovered);
    }

    #[test]
    fn cancel_clears_armed_state() {
        let mut w = binary_widget(false);
        let mut state = ChangeState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 20.0, 20.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 20.0, 20.0));
        assert!(!w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 20.0, 20.0));
        assert_eq!(state.changes, 0);
    }

    #[test]
    fn rebuild_adopts_new_value_without_self_mutation() {
        let mut counter = 0u64;
        let prev = checkbox::<ChangeState, _>(false, |_s, _v| {});
        let mut w = View::<ChangeState>::build(&prev, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.value, Some(false));
        let next = checkbox::<ChangeState, _>(true, |_s, _v| {});
        let flags =
            View::<ChangeState>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(
            w.value,
            Some(true),
            "the confirmed value adopts immediately"
        );
        assert!(
            !w.anim_target,
            "the animation target only updates lazily in paint, once this frame's active value is read"
        );
        assert!(flags.needs_paint());
    }

    // ---- Animation seeding / pacing ---------------------------------------

    #[test]
    fn build_already_checked_settles_instantly_with_no_mount_fade_in() {
        let w = binary_widget(true);
        assert!(!w.anim.is_animating());
        assert_eq!(w.anim.value_clamped(), 1.0);
        assert!(w.anim_target);
    }

    #[test]
    fn build_unchecked_starts_settled_at_zero() {
        let w = binary_widget(false);
        assert!(!w.anim.is_animating());
        assert_eq!(w.anim.value_clamped(), 0.0);
        assert!(!w.anim_target);
    }

    #[test]
    fn paint_starts_a_real_transition_when_the_value_disagrees_with_anim_target() {
        let mut w = binary_widget(false);
        w.value = Some(true); // simulate the confirmed value a rebuild adopts
        assert!(!w.anim.is_animating());
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(HIT_SIZE, HIT_SIZE));
        let mut rec = RRectRecorder::default();
        w.paint(&mut ctx, &mut rec);
        assert!(w.anim_target, "paint syncs the animation target");
        assert!(w.anim.is_animating(), "a real transition started");
        assert!(
            ctx.needs_frame(),
            "an in-flight transition needs another frame"
        );
    }

    // ---- Paint: colors per state ------------------------------------------

    /// Records each rounded rect's `(origin, size, radius, color)` and each
    /// stroked path's `(width, color)`, mirroring `switch.rs`'s
    /// `RRectRecorder` pattern.
    #[derive(Default)]
    struct RRectRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(f64, Color)>,
        fills: Vec<(Point, Size, Color)>,
    }

    impl PaintScene for RRectRecorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.fills.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, width: f64, brush: &Brush) {
            let Brush::Solid(color) = brush else {
                return;
            };
            self.strokes.push((width, *color));
        }
    }

    fn paint_rec(w: &mut CheckboxWidget, theme: Option<&Theme>) -> RRectRecorder {
        let mut rec = RRectRecorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, Size::new(HIT_SIZE, HIT_SIZE)).with_theme(t),
            None => PaintCtx::new(Point::ZERO, Size::new(HIT_SIZE, HIT_SIZE)),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    /// The box fill is always the second rounded rect (the state layer, when
    /// active, is the first — but at rest with no interaction it's absent).
    fn box_fill(rec: &RRectRecorder) -> Color {
        rec.rrects.last().expect("box fill painted").3
    }

    fn border_color(rec: &RRectRecorder) -> Color {
        rec.strokes.first().expect("border stroked").1
    }

    #[test]
    fn unthemed_unchecked_paints_transparent_fill_and_outline_variant_border() {
        let mut w = binary_widget(false);
        let rec = paint_rec(&mut w, None);
        assert_eq!(box_fill(&rec), Color::TRANSPARENT);
        assert_eq!(border_color(&rec), ON_SURFACE_VARIANT);
    }

    #[test]
    fn unthemed_checked_paints_active_fill_and_border() {
        let mut w = binary_widget(true);
        let rec = paint_rec(&mut w, None);
        assert_eq!(box_fill(&rec), FILL_ACTIVE);
        assert_eq!(border_color(&rec), FILL_ACTIVE);
        // Two strokes: border + check mark.
        assert_eq!(rec.strokes.len(), 2);
        assert_eq!(rec.strokes[1].1, ON_PRIMARY);
    }

    #[test]
    fn unthemed_error_recolors_fill_border_and_mark_regardless_of_active() {
        let mut counter = 0u64;

        let error_off = checkbox::<ChangeState, _>(false, |_s, _v| {}).error(true);
        let mut w = View::<ChangeState>::build(&error_off, &mut BuildCtx::new(&mut counter));
        let rec = paint_rec(&mut w, None);
        assert_eq!(
            box_fill(&rec),
            Color::TRANSPARENT,
            "inactive fill stays transparent even in error"
        );
        assert_eq!(border_color(&rec), ERROR);

        let error_on = checkbox::<ChangeState, _>(true, |_s, _v| {}).error(true);
        let mut w = View::<ChangeState>::build(&error_on, &mut BuildCtx::new(&mut counter));
        let rec = paint_rec(&mut w, None);
        assert_eq!(box_fill(&rec), ERROR);
        assert_eq!(border_color(&rec), ERROR);
        assert_eq!(rec.strokes[1].1, ON_ERROR);
    }

    #[test]
    fn unthemed_indeterminate_paints_active_fill_and_a_centered_dash() {
        let mut w = tristate_widget(None);
        let rec = paint_rec(&mut w, None);
        assert_eq!(box_fill(&rec), FILL_ACTIVE);
        // Border stroke only (no check-mark stroke for the dash — it's a
        // filled rect).
        assert_eq!(rec.strokes.len(), 1);
        let (origin, size, color) = rec.fills.first().expect("dash painted");
        assert_eq!(*size, Size::new(INDETERMINATE_WIDTH, INDETERMINATE_HEIGHT));
        assert_eq!(*color, ON_PRIMARY);
        let expected_x = BOX_OFFSET + (BOX - INDETERMINATE_WIDTH) / 2.0;
        assert!((origin.x - expected_x).abs() < 1e-9);
    }

    #[test]
    fn themed_paint_resolves_color_scheme_roles() {
        let theme = crate::baseline();
        let scheme = theme.scheme();

        let mut off = binary_widget(false);
        let rec = paint_rec(&mut off, Some(&theme));
        assert_eq!(box_fill(&rec), Color::TRANSPARENT);
        assert_eq!(border_color(&rec), scheme.on_surface_variant);

        let mut on = binary_widget(true);
        let rec = paint_rec(&mut on, Some(&theme));
        assert_eq!(box_fill(&rec), scheme.primary);
        assert_eq!(border_color(&rec), scheme.primary);
        assert_eq!(rec.strokes[1].1, scheme.on_primary);
    }

    #[test]
    fn themed_paint_resolves_error_roles() {
        let theme = crate::baseline();
        let scheme = theme.scheme();
        let view = checkbox::<ChangeState, _>(true, |_s, _v| {}).error(true);
        let mut counter = 0u64;
        let mut w = View::<ChangeState>::build(&view, &mut BuildCtx::new(&mut counter));
        let rec = paint_rec(&mut w, Some(&theme));
        assert_eq!(box_fill(&rec), scheme.error);
        assert_eq!(border_color(&rec), scheme.error);
        assert_eq!(rec.strokes[1].1, scheme.on_error);
    }

    // ---- Semantics ----------------------------------------------------------

    #[test]
    fn semantics_reports_checkbox_role_and_toggled_state() {
        fn logic(_s: &mut ChangeState) -> CheckboxView<ChangeState> {
            checkbox::<ChangeState, _>(true, |_s, _v| {})
        }
        let mut root: frust_core::RenderRoot<ChangeState, CheckboxView<ChangeState>> =
            frust_core::RenderRoot::new();
        let mut state = ChangeState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::CheckBox)
            .expect("checkbox contributes a Role::CheckBox node");
        assert_eq!(node.toggled(), Some(Toggled::True));
        assert!(node.supports_action(Action::Click));
    }

    #[test]
    fn semantics_reports_mixed_toggled_for_indeterminate() {
        fn logic(_s: &mut ChangeState) -> CheckboxView<ChangeState> {
            tristate_checkbox::<ChangeState, _>(None, |_s, _v| {})
        }
        let mut root: frust_core::RenderRoot<ChangeState, CheckboxView<ChangeState>> =
            frust_core::RenderRoot::new();
        let mut state = ChangeState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::CheckBox)
            .expect("checkbox contributes a Role::CheckBox node");
        assert_eq!(node.toggled(), Some(Toggled::Mixed));
    }
}
