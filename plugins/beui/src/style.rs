//! The shared beUI style vocabulary: the metrics and state treatments every
//! component in this catalog reads instead of re-deriving them.
//!
//! beUI expresses its design entirely in Tailwind utility classes, so a ported
//! component's "design" is a class list. This module is where the recurring
//! classes are translated **once**: `h-10` becomes [`HEIGHT_MD`],
//! `rounded-full` becomes [`RADIUS_CONTROL`], `disabled:opacity-50` becomes
//! [`DISABLED_OPACITY`], `ring-2 ring-ring/40` becomes [`FOCUS_RING_WIDTH`] and
//! [`FOCUS_RING_OPACITY`]. A component that needs a number a class list gives it
//! looks here first; a value used by exactly one component stays in that
//! component.
//!
//! **Sources**, all retrieved 2026-09-01:
//! - Component class lists: beUI rev
//!   `10c283e433a8f4f0ac0736684d4426ab612b9f55` — `components/motion/button/`
//!   (`base.tsx`'s `SIZE_CLASS`/`VARIANT_CLASS` tables) and
//!   `components/motion/input.tsx` are the two representative controls this
//!   ladder is transcribed from, plus `app/globals.css`'s own utility classes
//!   (`.press`, `.glass*`).
//! - The spacing/size/radius scales those classes resolve against: Tailwind
//!   CSS's own defaults. beUI overrides **no** scale — it authors no `--radius`
//!   custom property at all — so every number below is a Tailwind default,
//!   unlike `frust-shadcn`, which resolves its radii against a shadcn override.
//!
//! # The house style: pills and generous heights
//!
//! Two things separate this ladder from the sibling catalog's. beUI's controls
//! are **pill-shaped** — `rounded-full` outnumbers every other radius class
//! upstream by better than two to one — and they are **taller**: the default
//! button is `h-10` (40px) against shadcn's `h-9`, and the input field is `h-11`
//! (44px), which lands exactly on the mobile tap-target convention rather than
//! below it. This is a desktop-first web system whose metrics happen to be
//! touch-friendly; that is the source's, not an adaptation made here.
//!
//! # What is deliberately *not* here
//!
//! - **No state-layer overlay.** beUI does not tint a control on hover the way
//!   M3 does (`frust_material::state_layer`); it *swaps or fades the fill token*
//!   — `hover:bg-primary/90` on a solid control, `hover:bg-primary/5` on a ghost
//!   one. The alphas those forms need are [`HOVER_SOLID_ALPHA`] and
//!   [`HOVER_WASH_ALPHA`]; the token swap itself is each component's own
//!   business.
//! - **No elevation table.** beUI's shadows are Tailwind's flat CSS ladder,
//!   picked per component, not levels derived from a dp value — so the one
//!   shadow the stylesheet itself authors travels with the glass tier that uses
//!   it ([`glass_scale`](crate::tokens::glass_scale)) rather than becoming a
//!   `Theme::elevation` fold.
//! - **No motion constants.** Curves, springs and the durations paired with
//!   them live in [`crate::tokens::motion`], which is where a component reads
//!   them.

use frust::{Color, CursorIcon};

// ---- Spacing ---------------------------------------------------------------

/// One step of Tailwind's spacing scale, in logical px (`--spacing: 0.25rem` at
/// the CSS default 16px root font size). `p-4` is `4 × SPACING_UNIT`.
pub const SPACING_UNIT: f64 = 4.0;

/// Resolve a Tailwind spacing step (`gap-2`, `px-3`, `size-4`) to logical px.
///
/// Fractional steps are legal upstream and legal here (`pl-3.5` is
/// `spacing(3.5)`).
pub fn spacing(steps: f64) -> f64 {
    steps * SPACING_UNIT
}

// ---- Control heights -------------------------------------------------------

/// Control height for the `sm` button size: `h-8`.
pub const HEIGHT_SM: f64 = 32.0;
/// Control height for the `md` button size: `h-10` — beUI's default, and the
/// size a plain `<Button>` renders at.
pub const HEIGHT_MD: f64 = 40.0;
/// Control height for the `lg` button size: `h-12`.
pub const HEIGHT_LG: f64 = 48.0;
/// Edge length of an `icon`-size button: `h-8 w-8` — square, and the one size
/// upstream gives a rectangular radius instead of a pill
/// ([`RADIUS_ICON_BUTTON`]).
pub const SIZE_ICON_BUTTON: f64 = 32.0;
/// Height of the text input field: `h-11` (`input.tsx`) — a rung above the
/// default button, and the only control that lands on the 44px mobile
/// tap-target convention exactly.
pub const HEIGHT_INPUT: f64 = 44.0;

/// Horizontal padding of an `sm` button: `px-3`.
pub const PADDING_X_SM: f64 = 12.0;
/// Horizontal padding of an `md` button: `px-5` — roomier than the height step
/// alone would suggest, which is what gives a beUI pill its wide look.
pub const PADDING_X_MD: f64 = 20.0;
/// Horizontal padding of an `lg` button: `px-6`.
pub const PADDING_X_LG: f64 = 24.0;
/// Horizontal padding inside an input field with no icon: `pl-3.5` / `pr-3.5`.
pub const PADDING_X_INPUT: f64 = 14.0;
/// Horizontal padding inside an input field on a side that carries an icon:
/// `pl-10` / `pr-10`.
pub const PADDING_X_INPUT_ICON: f64 = 40.0;

/// Gap between a control's icon and its label at the `sm` size: `gap-1.5`.
pub const GAP_SM: f64 = 6.0;
/// Gap between a control's icon and its label at the `md`/`lg` sizes: `gap-2`.
pub const GAP_MD: f64 = 8.0;

/// Default icon edge inside a control: `[&_svg]:h-4 [&_svg]:w-4` (`input.tsx`'s
/// icon slots, and the size every inline icon inherits).
pub const ICON_SIZE: f64 = 16.0;
/// Icon edge for the larger in-field affordances: `h-5 w-5` (`input.tsx`'s
/// success check).
pub const ICON_SIZE_LG: f64 = 20.0;

// ---- Text ------------------------------------------------------------------

/// `text-xs`: `sm`-button and badge text.
pub const TEXT_XS: f64 = 12.0;
/// `text-sm`: `md`-button and label text — the catalog's default control size.
pub const TEXT_SM: f64 = 14.0;
/// `text-base`: `lg`-button and input text.
pub const TEXT_BASE: f64 = 16.0;
/// `leading-6`: the input field's line height, the one line-height value the
/// representative controls state outright.
pub const LINE_HEIGHT_INPUT: f64 = 24.0;

// ---- Radii -----------------------------------------------------------------
//
// Tailwind's default radius ladder, in logical px at the CSS default 16px root
// font size. beUI authors no `--radius` override, so these are the framework
// defaults its `rounded-*` classes resolve against — and the ladder
// `crate::tokens::shape_scale` folds onto `ShapeScale`.

/// `rounded-sm` — `0.25rem`.
pub const RADIUS_SM: f64 = 4.0;
/// `rounded-md` — `0.375rem`.
pub const RADIUS_MD: f64 = 6.0;
/// `rounded-lg` — `0.5rem`. The icon button's radius, and the smallest step the
/// catalog uses with any frequency.
pub const RADIUS_LG: f64 = 8.0;
/// `rounded-xl` — `0.75rem`. The workhorse panel radius.
pub const RADIUS_XL: f64 = 12.0;
/// `rounded-2xl` — `1rem`. Cards.
pub const RADIUS_2XL: f64 = 16.0;
/// `rounded-3xl` — `1.5rem`. Sheets and large surfaces.
pub const RADIUS_3XL: f64 = 24.0;
/// `rounded-4xl` — `2rem`.
pub const RADIUS_4XL: f64 = 32.0;

/// `rounded-full` — the pill sentinel, resolved against the box a component is
/// painting (half its shorter edge), never a literal radius.
///
/// Infinite rather than a large number so a consumer that clamps against the
/// box gets a true pill at every size; this is the same sentinel
/// `ShapeScale::full` carries, which is what
/// [`shape_scale`](crate::tokens::shape_scale) binds it to.
pub const RADIUS_FULL: f64 = f64::INFINITY;

/// The radius a beUI control paints: [`RADIUS_FULL`].
///
/// Named separately from the raw ladder step because it is a *house rule*, not a
/// class translation: `rounded-full` is what `button`'s `sm`/`md`/`lg` sizes and
/// `input`'s field both carry, and it is the single most recognizable thing
/// about the system's look. A component reaching for "the control radius" should
/// name this rather than picking a number.
pub const RADIUS_CONTROL: f64 = RADIUS_FULL;

/// The radius an `icon`-size button paints: `rounded-lg` — the one control
/// upstream deliberately squares off instead of making a pill (a circular icon
/// button would read as an avatar).
pub const RADIUS_ICON_BUTTON: f64 = RADIUS_LG;

/// Border width for every bordered control: Tailwind's `border` = 1px.
pub const BORDER_WIDTH: f64 = 1.0;

/// Flattening tolerance for every path this catalog strokes or fills — the same
/// value the other catalogs' stroked outlines use.
///
/// Curves reach `Shape::to_path` as polylines accurate to this many logical px,
/// which is well under a device pixel at any sane scale factor; a component
/// flattening a rounded rect, an arc, or a glyph path reads this rather than
/// naming its own tolerance.
pub const PATH_TOLERANCE: f64 = 0.1;

// ---- Interaction-state treatments ------------------------------------------

/// Opacity of a disabled control: `disabled:opacity-50` (`button`), applied
/// uniformly to every part a control paints (fill, border, text, icon).
pub const DISABLED_OPACITY: f32 = 0.5;

/// Opacity of a disabled **input field**: `disabled:opacity-60` (`input.tsx`).
///
/// Deliberately a second constant rather than folding into
/// [`DISABLED_OPACITY`]: the two representative controls disagree upstream, and
/// collapsing them would silently change one of them.
pub const DISABLED_OPACITY_INPUT: f32 = 0.6;

/// The cursor a disabled control asks for: `disabled:cursor-not-allowed`
/// (`input.tsx`).
///
/// Requested from the widget's own `Move` arm like any other cursor (never from
/// `Down` — see `docs/CODE_STANDARDS.md`'s cursor rule). A control that is
/// `disabled:pointer-events-none` upstream (`button`) shows no cursor of its own
/// at all; one that keeps pointer events (`input`) shows this.
pub const DISABLED_CURSOR: CursorIcon = CursorIcon::NotAllowed;

/// The cursor an enabled, activatable control asks for.
///
/// Tailwind's reset leaves a `<button>` at the platform default, but beUI's own
/// site sets `cursor-pointer` on interactive controls; frust has no CSS reset to
/// inherit, so this is the catalog's stated choice rather than a class
/// translation.
pub const ACTIVE_CURSOR: CursorIcon = CursorIcon::Pointer;

/// Alpha a **solid** fill drops to on hover: `hover:bg-primary/90`
/// (`button.tsx`'s `primary` variant).
pub const HOVER_SOLID_ALPHA: f32 = 0.9;

/// Alpha of the **wash** a transparent control fills with on hover:
/// `hover:bg-primary/5` (`button.tsx`'s `ghost` and `outline` variants).
///
/// Note this is a wash of `primary` — the ink color — not of an accent, which is
/// what makes a beUI ghost hover read as a faint grey rather than a tint.
pub const HOVER_WASH_ALPHA: f32 = 0.05;

/// The scale a pressed control shrinks to: `whileTap={{ scale: pressScale }}`
/// with `pressScale = 0.93` (`button/base.tsx`'s default), driven by
/// [`SPRING_PRESS`](crate::tokens::motion::SPRING_PRESS).
pub const PRESS_SCALE: f64 = 0.93;

/// The scale a hovered control grows to: `whileHover={{ scale: 1.02 }}`
/// (`button/base.tsx`).
///
/// Upstream applies this **only on a hover-capable pointer** and never under
/// reduced motion; a port must gate it the same way rather than scaling on
/// touch, where there is no hover state to leave.
pub const HOVER_SCALE: f64 = 1.02;

/// The scale the stylesheet's own `.press` utility shrinks to: `scale(0.97)`,
/// paired with [`TIMING_PRESS`](crate::tokens::motion::TIMING_PRESS).
///
/// Distinct from [`PRESS_SCALE`]: `.press` is the CSS-only press treatment
/// applied to non-button surfaces, and it is deliberately shallower than the
/// spring-driven one buttons use.
pub const PRESS_SCALE_CSS: f64 = 0.97;

/// Width of the focus ring: `ring-2` — 2 logical px, painted **outside** the
/// control's own bounds (Tailwind's `ring` is a non-inset box-shadow at offset
/// 0).
pub const FOCUS_RING_WIDTH: f64 = 2.0;

/// Opacity the ring color is painted at: `ring-ring/40` (`input.tsx`'s focused
/// state).
pub const FOCUS_RING_OPACITY: f32 = 0.4;

/// Opacity the ring is painted at on an **errored** control:
/// `ring-destructive/25` (`input.tsx`), which is deliberately fainter than the
/// focus ring because the border itself has already gone solid `destructive`.
pub const ERROR_RING_OPACITY: f32 = 0.25;

/// Alpha the input's border rises to while focused: `border-foreground/40`.
pub const FOCUS_BORDER_ALPHA: f32 = 0.4;

/// Alpha a placeholder is painted at: `placeholder:text-muted-foreground/60`
/// (`input.tsx`).
pub const PLACEHOLDER_ALPHA: f32 = 0.6;

// ---- Color helpers ---------------------------------------------------------

/// Return `color` with its alpha channel replaced by `alpha`.
///
/// The catalog's one alpha helper — beUI's class lists express a great many
/// treatments as `token/NN` (a hover fill, a ring, a destructive wash), and
/// every one of them is this operation.
pub const fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Multiply `color`'s existing alpha by `factor` — `token/50` applied to a color
/// that may already be translucent (beUI's borders always are), where
/// [`with_alpha`] would throw the existing alpha away.
pub const fn scale_alpha(color: Color, factor: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], c[3] * factor])
}

/// Apply the disabled treatment to a resolved color: `opacity` when `disabled`,
/// untouched otherwise.
///
/// Applied per painted color rather than through a `push_layer` group so a
/// component can leave one part at full opacity if its class list does. The
/// opacity is a parameter rather than baked in because the two representative
/// controls disagree — see [`DISABLED_OPACITY`] and [`DISABLED_OPACITY_INPUT`].
pub const fn disabled_tint(color: Color, disabled: bool, opacity: f32) -> Color {
    if disabled {
        scale_alpha(color, opacity)
    } else {
        color
    }
}

/// Resolve the pill sentinel against the box a component is painting: half the
/// shorter edge, which is what `border-radius: 9999px` renders as.
///
/// A component paints [`RADIUS_CONTROL`] through this rather than passing an
/// infinite radius to a rounded-rect primitive. A finite radius passes through
/// clamped to the same ceiling, so a caller can hand any ladder step to it
/// without branching.
pub fn resolve_radius(radius: f64, width: f64, height: f64) -> f64 {
    let ceiling = width.min(height).max(0.0) / 2.0;
    if radius.is_nan() {
        return 0.0;
    }
    radius.clamp(0.0, ceiling)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacing_resolves_tailwind_steps_including_fractional_ones() {
        assert_eq!(spacing(0.0), 0.0);
        assert_eq!(spacing(3.0), 12.0, "px-3");
        assert_eq!(spacing(5.0), PADDING_X_MD, "px-5");
        assert_eq!(spacing(3.5), PADDING_X_INPUT, "pl-3.5");
        assert_eq!(spacing(10.0), PADDING_X_INPUT_ICON, "pl-10");
        assert_eq!(spacing(1.5), GAP_SM, "gap-1.5");
        assert_eq!(spacing(4.0), ICON_SIZE, "size-4");
    }

    /// Every height in the ladder is a whole Tailwind step, and the ladder
    /// climbs. The input's extra rung above the default button is beUI's own
    /// (`h-11` vs `h-10`) and is pinned here so a later edit cannot quietly
    /// level them.
    #[test]
    fn the_height_ladder_climbs_in_tailwind_steps() {
        for (name, height) in [
            ("HEIGHT_SM", HEIGHT_SM),
            ("HEIGHT_MD", HEIGHT_MD),
            ("HEIGHT_LG", HEIGHT_LG),
            ("HEIGHT_INPUT", HEIGHT_INPUT),
            ("SIZE_ICON_BUTTON", SIZE_ICON_BUTTON),
        ] {
            let steps = height / SPACING_UNIT;
            assert_eq!(steps.fract(), 0.0, "{name} must be a whole spacing step");
        }
        // Strictly increasing, in the order a caller reaches for them.
        let ladder = [HEIGHT_SM, HEIGHT_MD, HEIGHT_INPUT, HEIGHT_LG];
        assert!(ladder.windows(2).all(|w| w[0] < w[1]));
        // The house metric worth knowing: the default control is taller than
        // the sibling catalog's 36px, and the field lands on the 44px
        // tap-target convention exactly.
        assert_eq!(HEIGHT_MD, 40.0);
        assert_eq!(HEIGHT_INPUT, 44.0);
    }

    /// The radius ladder is Tailwind's default scale — doubling `rem` steps,
    /// strictly increasing — and it terminates in the pill sentinel rather than
    /// a large finite number.
    #[test]
    fn the_radius_ladder_is_the_tailwind_default_scale() {
        let ladder = [
            RADIUS_SM, RADIUS_MD, RADIUS_LG, RADIUS_XL, RADIUS_2XL, RADIUS_3XL, RADIUS_4XL,
        ];
        assert!(ladder.windows(2).all(|w| w[0] < w[1]));
        // The `rem` sources, at the CSS default 16px root font size.
        assert_eq!(RADIUS_SM, 0.25 * 16.0);
        assert_eq!(RADIUS_MD, 0.375 * 16.0);
        assert_eq!(RADIUS_LG, 0.5 * 16.0);
        assert_eq!(RADIUS_XL, 0.75 * 16.0);
        assert_eq!(RADIUS_2XL, 16.0);
        assert_eq!(RADIUS_3XL, 1.5 * 16.0);
        assert_eq!(RADIUS_4XL, 2.0 * 16.0);

        assert!(RADIUS_FULL.is_infinite());
        assert!(RADIUS_CONTROL.is_infinite(), "beUI's controls are pills");
        assert_eq!(RADIUS_ICON_BUTTON, RADIUS_LG, "the one squared-off control");
    }

    /// The pill sentinel resolves to a true pill on any box, and a finite step
    /// passes through untouched until it would exceed the box.
    #[test]
    fn resolve_radius_turns_the_sentinel_into_half_the_shorter_edge() {
        assert_eq!(resolve_radius(RADIUS_CONTROL, 120.0, HEIGHT_MD), 20.0);
        assert_eq!(resolve_radius(RADIUS_CONTROL, 20.0, 120.0), 10.0);
        // A finite step is left alone while it fits...
        assert_eq!(resolve_radius(RADIUS_XL, 120.0, HEIGHT_MD), RADIUS_XL);
        // ...and clamped when it does not.
        assert_eq!(resolve_radius(RADIUS_4XL, 120.0, 20.0), 10.0);
        // Degenerate boxes stay well-defined rather than producing a negative
        // or NaN radius a paint call would choke on.
        assert_eq!(resolve_radius(RADIUS_CONTROL, 0.0, 0.0), 0.0);
        assert_eq!(resolve_radius(RADIUS_CONTROL, -10.0, 40.0), 0.0);
        assert_eq!(resolve_radius(f64::NAN, 100.0, 40.0), 0.0);
    }

    #[test]
    fn with_alpha_replaces_and_scale_alpha_multiplies() {
        let opaque = Color::from_rgb8(0x11, 0x22, 0x33);
        assert_eq!(with_alpha(opaque, 0.5).components[3], 0.5);
        // The distinction that matters for beUI: its borders are already
        // translucent, so `token/50` must multiply, not overwrite.
        let wash = with_alpha(opaque, 0.06);
        assert_eq!(scale_alpha(wash, 0.5).components[3], 0.03);
        assert_eq!(with_alpha(wash, 0.5).components[3], 0.5);
        // Neither touches the color channels.
        assert_eq!(
            scale_alpha(wash, 0.5).components[..3],
            opaque.components[..3]
        );
    }

    #[test]
    fn disabled_tint_scales_only_when_disabled_and_takes_its_opacity() {
        let color = Color::from_rgb8(0x11, 0x22, 0x33);
        assert_eq!(disabled_tint(color, false, DISABLED_OPACITY), color);
        assert_eq!(
            disabled_tint(color, true, DISABLED_OPACITY).components[3],
            DISABLED_OPACITY
        );
        // The input's own, fainter treatment is reachable through the same
        // helper rather than a second one.
        assert_eq!(
            disabled_tint(color, true, DISABLED_OPACITY_INPUT).components[3],
            DISABLED_OPACITY_INPUT
        );
        assert_ne!(DISABLED_OPACITY, DISABLED_OPACITY_INPUT);
    }

    /// The state alphas and scales sit in the ranges their treatments require —
    /// a wash fainter than a solid fill's hover, a press that shrinks, a hover
    /// that grows.
    #[test]
    fn the_state_treatments_are_the_right_way_round() {
        // A ghost control's hover wash is far fainter than the alpha a solid
        // fill drops to, and an error ring is fainter than a focus ring.
        for (name, fainter, stronger) in [
            ("hover wash", HOVER_WASH_ALPHA, HOVER_SOLID_ALPHA),
            ("error ring", ERROR_RING_OPACITY, FOCUS_RING_OPACITY),
        ] {
            assert!(fainter < stronger, "{name} must be the fainter treatment");
        }
        // A press shrinks and a hover grows — and the CSS-only press treatment
        // is the shallower of the two presses.
        let scales = [PRESS_SCALE, PRESS_SCALE_CSS, 1.0, HOVER_SCALE];
        assert!(scales.windows(2).all(|w| w[0] < w[1]));
        for (name, alpha) in [
            ("DISABLED_OPACITY", DISABLED_OPACITY),
            ("DISABLED_OPACITY_INPUT", DISABLED_OPACITY_INPUT),
            ("HOVER_SOLID_ALPHA", HOVER_SOLID_ALPHA),
            ("HOVER_WASH_ALPHA", HOVER_WASH_ALPHA),
            ("FOCUS_RING_OPACITY", FOCUS_RING_OPACITY),
            ("ERROR_RING_OPACITY", ERROR_RING_OPACITY),
            ("FOCUS_BORDER_ALPHA", FOCUS_BORDER_ALPHA),
            ("PLACEHOLDER_ALPHA", PLACEHOLDER_ALPHA),
        ] {
            assert!((0.0..=1.0).contains(&alpha), "{name} out of range");
        }
    }
}
