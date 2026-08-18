//! The shared shadcn style vocabulary: the metrics, state treatments, and paint
//! helpers every component in this catalog reads instead of re-deriving them.
//!
//! shadcn/ui expresses its design entirely in Tailwind utility classes, so a
//! ported component's "design" is a class list. This module is where the
//! recurring classes are translated **once**: `h-9` becomes
//! [`HEIGHT_DEFAULT`], `focus-visible:ring-[3px] focus-visible:ring-ring/50`
//! becomes [`draw_focus_ring`], `disabled:opacity-50` becomes
//! [`DISABLED_OPACITY`], `shadow-xs` becomes [`SHADOW_XS`]. A component that
//! needs a number a class list gives it looks here first; a value used by
//! exactly one component stays in that component.
//!
//! **Sources**, all retrieved 2026-08-17:
//! - Component class lists: shadcn/ui rev
//!   `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`,
//!   `apps/v4/registry/new-york-v4/ui/*.tsx` (each constant below names the file
//!   it came from).
//! - The spacing/size/shadow scales those classes resolve against:
//!   `tailwindcss@4.3.0`'s own `theme.css` (the version `apps/v4/package.json`
//!   pins) — shadcn overrides only the radius scale (see
//!   [`ShadcnRadius`](crate::ShadcnRadius)), so every other scale is the
//!   Tailwind default.
//!
//! # What is deliberately *not* here
//!
//! - **No state-layer overlay.** shadcn does not tint a control on hover the way
//!   M3 does (`frust_material::state_layer`); it *swaps the fill token* —
//!   `hover:bg-accent` on a ghost/outline control, `hover:bg-primary/90` on a
//!   solid one. The two alphas that second form needs are
//!   [`HOVER_SOLID_ALPHA`]/[`HOVER_SECONDARY_ALPHA`]; the token swap itself is
//!   each component's own business.
//! - **No elevation table.** shadcn's shadows are Tailwind's flat CSS ladder,
//!   picked per component (`shadow-xs` on an input, `shadow-md` on a popover),
//!   not levels derived from a dp value — so they are constants here rather than
//!   a `Theme.elevation` fold.

use frust::{
    Brightness, Color, CursorIcon, Theme,
    authoring::{Brush, PaintScene, Point, RoundedRect, Shape, Size},
};

use crate::tokens::extension::ShadcnTokens;

// ---- Spacing, sizes, metrics ---------------------------------------------

/// One step of Tailwind's spacing scale, in logical px (`--spacing: 0.25rem` at
/// the CSS default 16px root font size). `p-4` is `4 × SPACING_UNIT`.
pub const SPACING_UNIT: f64 = 4.0;

/// Resolve a Tailwind spacing step (`gap-2`, `px-3`, `size-4`) to logical px.
///
/// Fractional steps are legal upstream and legal here (`py-1.5` is
/// `spacing(1.5)`).
pub fn spacing(steps: f64) -> f64 {
    steps * SPACING_UNIT
}

/// Control height for the `default` size: `h-9` (button, input, native-select,
/// select trigger).
///
/// **Below the 44–48px mobile tap-target convention by design** — this is
/// shadcn's desktop metric and the port keeps it. A touch-first screen should
/// pick [`HEIGHT_LG`] or give the control a generous hit area of its own; no
/// density mechanism is invented here.
pub const HEIGHT_DEFAULT: f64 = 36.0;
/// Control height for the `sm` size: `h-8` (`button` size `sm`, `select`
/// `data-[size=sm]`).
pub const HEIGHT_SM: f64 = 32.0;
/// Control height for the `xs` size: `h-6` (`button` size `xs`).
pub const HEIGHT_XS: f64 = 24.0;
/// Control height for the `lg` size: `h-10` (`button` size `lg`) — the tallest
/// rung of the ladder, and still below the 44px mobile tap-target convention
/// [`HEIGHT_DEFAULT`] documents. The whole ladder is desktop-first; `lg` is the
/// roomiest choice available, not a touch-sized one.
pub const HEIGHT_LG: f64 = 40.0;

/// Default icon edge: `[&_svg:not([class*='size-'])]:size-4` — the size every
/// component's inline icon inherits unless it overrides it.
pub const ICON_SIZE: f64 = 16.0;
/// Icon edge inside an `xs`-size control: `size-3`.
pub const ICON_SIZE_SM: f64 = 12.0;

/// `text-xs`: badge/kbd/`xs`-button text.
pub const TEXT_XS: f64 = 12.0;
/// `text-sm`: the catalog's default text size (button, select, table, menus).
pub const TEXT_SM: f64 = 14.0;
/// `text-base`: `input`/`textarea` text below the `md` breakpoint (they drop to
/// `text-sm` above it — a responsive step this port does not model, so a
/// component picks one).
pub const TEXT_BASE: f64 = 16.0;

/// Border width for every bordered control: Tailwind's `border` = 1px.
pub const BORDER_WIDTH: f64 = 1.0;

/// Flattening tolerance for every path this catalog strokes or fills — the
/// same value the other catalogs' stroked outlines use.
///
/// Curves reach `Shape::to_path` as polylines accurate to this many logical px,
/// which is well under a device pixel at any sane scale factor; a component
/// flattening a rounded rect, an arc, or a glyph path reads this rather than
/// naming its own tolerance.
pub(crate) const PATH_TOLERANCE: f64 = 0.1;

// ---- Interaction-state treatments ----------------------------------------

/// Opacity of a disabled control: `disabled:opacity-50`, applied uniformly to
/// every part a control paints (fill, border, text, icon).
pub const DISABLED_OPACITY: f32 = 0.5;

/// The cursor a disabled control asks for: `disabled:cursor-not-allowed`.
///
/// Requested from the widget's own `Move` arm like any other cursor (never from
/// `Down` — see `docs/CODE_STANDARDS.md`'s cursor rule). A control that is
/// `disabled:pointer-events-none` upstream (`button`) shows no cursor of its own
/// at all; one that keeps pointer events (`input`, `checkbox`, `select`) shows
/// this.
pub const DISABLED_CURSOR: CursorIcon = CursorIcon::NotAllowed;

/// The cursor an enabled, activatable control asks for.
///
/// shadcn/Tailwind's reset leaves a `<button>` at the platform default, but the
/// catalog's own site and every shadcn app in practice set `cursor-pointer` on
/// interactive controls; frust has no CSS reset to inherit, so this is the
/// catalog's stated choice rather than a class translation.
pub const ACTIVE_CURSOR: CursorIcon = CursorIcon::Pointer;

/// Alpha a **solid** fill drops to on hover: `hover:bg-primary/90`,
/// `hover:bg-destructive/90` (`button.tsx`).
pub const HOVER_SOLID_ALPHA: f32 = 0.9;
/// Alpha a **secondary** fill drops to on hover: `hover:bg-secondary/80`
/// (`button.tsx`).
pub const HOVER_SECONDARY_ALPHA: f32 = 0.8;

/// Width of the focus ring: `focus-visible:ring-[3px]` — 3 logical px, painted
/// **outside** the control's own bounds (Tailwind's `ring` is a non-inset
/// box-shadow at offset 0).
pub const FOCUS_RING_WIDTH: f64 = 3.0;
/// Opacity the ring color is painted at: `focus-visible:ring-ring/50`.
pub const FOCUS_RING_OPACITY: f32 = 0.5;

/// Return `color` with its alpha channel replaced by `alpha`.
///
/// The catalog's one alpha helper — shadcn's class lists express a great many
/// treatments as `token/NN` (a hover fill, a ring, a destructive wash), and every
/// one of them is this operation.
pub const fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// Multiply `color`'s existing alpha by `factor` — `token/50` applied to a
/// color that may already be translucent (a dark-mode `border`, say), where
/// [`with_alpha`] would throw the existing alpha away.
pub const fn scale_alpha(color: Color, factor: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], c[3] * factor])
}

/// Apply the disabled treatment to a resolved color: `opacity-50` when
/// `disabled`, untouched otherwise.
///
/// Applied per painted color rather than through a `push_layer` group so a
/// component can leave one part (a checked checkbox's mark, say) at full opacity
/// if its class list does.
pub const fn disabled_tint(color: Color, disabled: bool) -> Color {
    if disabled {
        scale_alpha(color, DISABLED_OPACITY)
    } else {
        color
    }
}

// ---- Two-state precedence --------------------------------------------------

/// The fill a two-state control paints under the current state, if any: the
/// `on` state wins over hover, and a resting control paints nothing
/// (`bg-transparent`).
///
/// Shared by `toggle` (the precedence's originating fix) and `toggle_group`,
/// which resolves the same rule per item over a palette of its own, so the two
/// components share the decision rather than restating it.
pub(crate) fn precedence_fill(
    on: bool,
    hovered: bool,
    on_fill: Color,
    hover_fill: Color,
) -> Option<Color> {
    if on {
        Some(on_fill)
    } else if hovered {
        Some(hover_fill)
    } else {
        None
    }
}

/// The label ink under the same precedence, falling back to the `resting` ink a
/// control inherits when it is neither on nor hovered.
pub(crate) fn precedence_ink(
    on: bool,
    hovered: bool,
    on_ink: Color,
    hover_ink: Color,
    resting: Color,
) -> Color {
    if on {
        on_ink
    } else if hovered {
        hover_ink
    } else {
        resting
    }
}

// ---- Focus ring -----------------------------------------------------------

/// The ring color for the pass's theme, under the documented ladder
/// (**explicit > theme extension > fallback**) — see
/// [`ShadcnTokens::resolve_ring`].
pub fn ring_color(explicit: Option<Color>, theme: Option<&Theme>) -> Color {
    ShadcnTokens::resolve_ring(explicit, theme)
}

/// Paint shadcn's focus ring around a control: a [`FOCUS_RING_WIDTH`]-wide ring
/// in the theme's `--ring` color at [`FOCUS_RING_OPACITY`], sitting immediately
/// **outside** `origin`/`size` (Tailwind `ring` semantics: offset 0, non-inset).
///
/// `radius` is the control's own corner radius; the ring's radius grows with its
/// outward offset so the two stay concentric.
///
/// # Two-part treatment
///
/// shadcn's focus-visible state is *two* changes, not one:
/// `focus-visible:ring-[3px] focus-visible:ring-ring/50` **and**
/// `focus-visible:border-ring`. This function paints the ring; a bordered
/// control must also swap its 1px border to the ring color, which is what
/// [`focus_border`] is for.
///
/// # Painting outside your own bounds
///
/// The ring lands in the 3px band outside the widget's bounds — legal (frust
/// applies no automatic clip) but visible only if nothing else paints over it, so
/// a focusable control inside a tight container should reserve room. Call it from
/// `paint`, gated on `PaintCtx::has_focus()` (the authoritative paint-time focus
/// read), never on a widget-internal focus flag.
pub fn draw_focus_ring(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    radius: f64,
    ring: Color,
) {
    // A stroke is centered on its path, so a ring occupying the band
    // `[0, FOCUS_RING_WIDTH]` outside the bounds rides a path offset outward by
    // half its width.
    let half = FOCUS_RING_WIDTH / 2.0;
    let rr = RoundedRect::new(
        -half,
        -half,
        size.width + half,
        size.height + half,
        radius + half,
    );
    scene.stroke_path(
        origin,
        &rr.to_path(PATH_TOLERANCE),
        FOCUS_RING_WIDTH,
        &Brush::Solid(with_alpha(ring, FOCUS_RING_OPACITY)),
    );
}

/// The border color a bordered control paints: the ring color while focused
/// (`focus-visible:border-ring`), `unfocused` otherwise.
///
/// The border half of the two-part focus treatment [`draw_focus_ring`]
/// documents; a control calls both from the same `has_focus()` branch.
pub fn focus_border(unfocused: Color, focused: bool, theme: Option<&Theme>) -> Color {
    if focused {
        ring_color(None, theme)
    } else {
        unfocused
    }
}

// ---- Shadow ladder --------------------------------------------------------

/// One rung of shadcn's (i.e. Tailwind's) shadow ladder, translated to
/// [`PaintScene::draw_shadow`]'s parameters.
///
/// # Translating a CSS `box-shadow`
///
/// A Tailwind shadow is one or two `offset-x offset-y blur spread color` layers;
/// `draw_shadow` takes a single `(origin, size, radius, std_dev, color)`. Three
/// deliberate simplifications bridge them:
///
/// 1. **Blur → standard deviation, halved.** The CSS spec defines a shadow's
///    blur radius as *twice* the standard deviation of the Gaussian
///    (CSS Backgrounds and Borders 3 § 7.2.1), so `std_dev = blur / 2`.
/// 2. **Only the first (dominant) layer is painted.** Every two-layer Tailwind
///    shadow pairs a wide soft layer with a tight contact layer; `draw_shadow`
///    is a single primitive, and the wide layer is the one that reads. The same
///    simplification `frust_material::card` makes.
/// 3. **Spread is dropped.** The negative spreads Tailwind uses (`-1px`,
///    `-3px`) shrink the shadow rect slightly; `draw_shadow` has no spread
///    parameter, and the visual difference at these radii is under a pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadcnShadow {
    /// Vertical offset in logical px (CSS `offset-y`; every Tailwind shadow's
    /// `offset-x` is 0).
    pub y_offset: f64,
    /// Gaussian standard deviation — half the CSS blur radius (see the type
    /// docs).
    pub std_dev: f64,
    /// Alpha of the black shadow color (CSS `rgb(0 0 0 / α)`).
    pub alpha: f32,
}

impl ShadcnShadow {
    /// The shadow color to paint with: the theme's `shadow` role (black on every
    /// shadcn theme) at this rung's alpha, or plain black at it when no theme is
    /// threaded.
    pub fn color(&self, theme: Option<&Theme>) -> Color {
        let base = theme.map_or(Color::BLACK, |t| t.scheme().shadow);
        with_alpha(base, self.alpha)
    }
}

/// `shadow-2xs`: `0 1px rgb(0 0 0 / 0.05)` — a hairline contact shadow, no blur.
pub const SHADOW_2XS: ShadcnShadow = ShadcnShadow {
    y_offset: 1.0,
    std_dev: 0.0,
    alpha: 0.05,
};
/// `shadow-xs`: `0 1px 2px 0 rgb(0 0 0 / 0.05)` — the catalog's most-used rung
/// (inputs, outline/secondary buttons, switch, checkbox, select trigger).
pub const SHADOW_XS: ShadcnShadow = ShadcnShadow {
    y_offset: 1.0,
    std_dev: 1.0,
    alpha: 0.05,
};
/// `shadow-sm`: `0 1px 3px 0 rgb(0 0 0 / 0.1)` (+ a dropped contact layer) —
/// `card`.
pub const SHADOW_SM: ShadcnShadow = ShadcnShadow {
    y_offset: 1.0,
    std_dev: 1.5,
    alpha: 0.1,
};
/// `shadow-md`: `0 4px 6px -1px rgb(0 0 0 / 0.1)` (+ dropped layer) — the
/// anchored overlays (popover, dropdown-menu, select content, hover-card).
pub const SHADOW_MD: ShadcnShadow = ShadcnShadow {
    y_offset: 4.0,
    std_dev: 3.0,
    alpha: 0.1,
};
/// `shadow-lg`: `0 10px 15px -3px rgb(0 0 0 / 0.1)` (+ dropped layer) — the
/// modal family (dialog, alert-dialog, sheet) and menu submenus.
pub const SHADOW_LG: ShadcnShadow = ShadcnShadow {
    y_offset: 10.0,
    std_dev: 7.5,
    alpha: 0.1,
};
/// `shadow-xl`: `0 20px 25px -5px rgb(0 0 0 / 0.1)` (+ dropped layer).
pub const SHADOW_XL: ShadcnShadow = ShadcnShadow {
    y_offset: 20.0,
    std_dev: 12.5,
    alpha: 0.1,
};
/// `shadow-2xl`: `0 25px 50px -12px rgb(0 0 0 / 0.25)` — a single-layer rung.
pub const SHADOW_2XL: ShadcnShadow = ShadcnShadow {
    y_offset: 25.0,
    std_dev: 25.0,
    alpha: 0.25,
};

/// Paint one rung of the shadow ladder under a rounded rect at `origin`/`size`.
///
/// The shadow is offset down by the rung's `y_offset`, matching CSS's
/// downward-only shadows; `theme` supplies the `shadow` color role (see
/// [`ShadcnShadow::color`]).
pub fn draw_shadow(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    radius: f64,
    shadow: ShadcnShadow,
    theme: Option<&Theme>,
) {
    scene.draw_shadow(
        Point::new(origin.x, origin.y + shadow.y_offset),
        size,
        radius,
        shadow.std_dev,
        shadow.color(theme),
    );
}

/// Whether a theme is painting in dark mode — the one brightness read a
/// component needs, since a handful of shadcn class lists carry `dark:` variants
/// that are not just a token swap (`dark:bg-input/30` on an outline control).
pub fn is_dark(theme: Option<&Theme>) -> bool {
    theme.is_some_and(|t| t.brightness == Brightness::Dark)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::theme::theme;
    use frust::authoring::{BezPath, Rect};

    /// A recording `PaintScene`: captures the stroke and shadow calls this
    /// module emits, so geometry can be asserted with no GPU anywhere.
    #[derive(Default)]
    struct Recorder {
        strokes: Vec<(Rect, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn draw_shadow(
            &mut self,
            origin: Point,
            size: Size,
            radius: f64,
            std_dev: f64,
            color: Color,
        ) {
            self.shadows.push((origin, size, radius, std_dev, color));
        }
    }

    #[test]
    fn spacing_scale_resolves_tailwind_steps() {
        assert_eq!(spacing(1.0), 4.0);
        assert_eq!(spacing(1.5), 6.0);
        assert_eq!(spacing(4.0), 16.0);
        assert_eq!(spacing(6.0), 24.0);
    }

    #[test]
    fn control_heights_are_the_shadcn_size_ladder() {
        // h-6 / h-8 / h-9 / h-10, monotonic and spelled in logical px.
        assert_eq!(
            [HEIGHT_XS, HEIGHT_SM, HEIGHT_DEFAULT, HEIGHT_LG],
            [24.0, 32.0, 36.0, 40.0]
        );
        // The documented mobile-tap-target caveat: the default really is under
        // 44px, and `lg` really is the tallest shadcn size.
        const MOBILE_TAP_TARGET_FLOOR: f64 = 44.0;
        let ladder = [HEIGHT_XS, HEIGHT_SM, HEIGHT_DEFAULT, HEIGHT_LG];
        assert!(ladder.iter().all(|h| *h < MOBILE_TAP_TARGET_FLOOR));
        assert_eq!(
            ladder.iter().copied().fold(f64::MIN, f64::max),
            HEIGHT_LG,
            "`lg` is the tallest shadcn size"
        );
    }

    #[test]
    fn alpha_helpers_replace_and_scale_independently() {
        let opaque = Color::from_rgb8(0x10, 0x20, 0x30);
        assert_eq!(with_alpha(opaque, 0.5).components[3], 0.5);
        assert_eq!(scale_alpha(opaque, 0.5).components[3], 0.5);
        // Scaling respects an already-translucent color; replacing does not.
        let translucent = with_alpha(opaque, 0.2);
        assert!((scale_alpha(translucent, 0.5).components[3] - 0.1).abs() < 1e-6);
        assert_eq!(with_alpha(translucent, 0.5).components[3], 0.5);
        // RGB is never touched.
        assert_eq!(
            with_alpha(opaque, 0.5).components[..3],
            opaque.components[..3]
        );
    }

    #[test]
    fn disabled_tint_halves_alpha_only_when_disabled() {
        let c = Color::from_rgb8(0xFF, 0x00, 0x00);
        assert_eq!(disabled_tint(c, false), c);
        assert_eq!(
            disabled_tint(c, true).components[3],
            DISABLED_OPACITY,
            "opacity-50"
        );
        assert_eq!(DISABLED_CURSOR, CursorIcon::NotAllowed);
        assert_eq!(ACTIVE_CURSOR, CursorIcon::Pointer);
    }

    #[test]
    fn focus_ring_paints_a_3px_band_outside_the_control_bounds() {
        let mut scene = Recorder::default();
        let origin = Point::new(20.0, 30.0);
        let size = Size::new(100.0, 36.0);
        draw_focus_ring(
            &mut scene,
            origin,
            size,
            8.0,
            Color::from_rgb8(0xA1, 0xA1, 0xA1),
        );

        assert_eq!(scene.strokes.len(), 1);
        let (bbox, width, color) = scene.strokes[0];
        assert_eq!(width, FOCUS_RING_WIDTH);
        // The stroked path sits half a ring-width outside the bounds, so the
        // painted band covers exactly [0, 3px] outside them.
        assert!((bbox.x0 - (origin.x - 1.5)).abs() < 1e-6);
        assert!((bbox.y0 - (origin.y - 1.5)).abs() < 1e-6);
        assert!((bbox.x1 - (origin.x + size.width + 1.5)).abs() < 1e-6);
        assert!((bbox.y1 - (origin.y + size.height + 1.5)).abs() < 1e-6);
        // ...in the ring color at 50%.
        assert_eq!(color.components[3], FOCUS_RING_OPACITY);
    }

    /// The precedence rule itself, over the four state combinations —
    /// `toggle`/`toggle_group` both resolve their per-control/per-item look
    /// through these same two functions.
    #[test]
    fn the_precedence_rule_ranks_on_over_hover_over_rest() {
        let on_color = Color::from_rgb8(0x01, 0x01, 0x01);
        let hover = Color::from_rgb8(0x02, 0x02, 0x02);
        let resting = Color::from_rgb8(0x03, 0x03, 0x03);

        assert_eq!(
            precedence_fill(true, false, on_color, hover),
            Some(on_color)
        );
        assert_eq!(precedence_fill(true, true, on_color, hover), Some(on_color));
        assert_eq!(precedence_fill(false, true, on_color, hover), Some(hover));
        assert_eq!(
            precedence_fill(false, false, on_color, hover),
            None,
            "`bg-transparent` at rest"
        );

        assert_eq!(
            precedence_ink(true, false, on_color, hover, resting),
            on_color
        );
        assert_eq!(
            precedence_ink(true, true, on_color, hover, resting),
            on_color
        );
        assert_eq!(precedence_ink(false, true, on_color, hover, resting), hover);
        assert_eq!(
            precedence_ink(false, false, on_color, hover, resting),
            resting,
            "the inherited `foreground`"
        );
    }

    #[test]
    fn ring_color_and_focus_border_follow_the_theme_ladder() {
        let t = theme();
        let themed = ShadcnTokens::shadcn().ring(t.brightness);
        assert_eq!(ring_color(None, Some(&t)), themed);
        assert_eq!(ring_color(None, None), crate::tokens::FALLBACK_RING);

        let unfocused = Color::from_rgb8(0x01, 0x02, 0x03);
        assert_eq!(focus_border(unfocused, false, Some(&t)), unfocused);
        assert_eq!(focus_border(unfocused, true, Some(&t)), themed);
    }

    #[test]
    fn shadow_ladder_is_the_tailwind_table_halved_into_std_devs() {
        // blur / 2, per the CSS spec's own definition.
        assert_eq!(SHADOW_2XS.std_dev, 0.0);
        assert_eq!(SHADOW_XS.std_dev, 1.0); // blur 2px
        assert_eq!(SHADOW_SM.std_dev, 1.5); // blur 3px
        assert_eq!(SHADOW_MD.std_dev, 3.0); // blur 6px
        assert_eq!(SHADOW_LG.std_dev, 7.5); // blur 15px
        assert_eq!(SHADOW_XL.std_dev, 12.5); // blur 25px
        assert_eq!(SHADOW_2XL.std_dev, 25.0); // blur 50px
        // Offsets and alphas straight off the Tailwind table.
        assert_eq!(SHADOW_XS.y_offset, 1.0);
        assert_eq!(SHADOW_MD.y_offset, 4.0);
        assert_eq!(SHADOW_2XL.alpha, 0.25);
        // Monotonic in both offset and blur, so "bigger rung" always reads.
        let rungs = [
            SHADOW_2XS, SHADOW_XS, SHADOW_SM, SHADOW_MD, SHADOW_LG, SHADOW_XL, SHADOW_2XL,
        ];
        assert!(rungs.windows(2).all(|w| w[0].std_dev <= w[1].std_dev));
        assert!(rungs.windows(2).all(|w| w[0].y_offset <= w[1].y_offset));
    }

    #[test]
    fn draw_shadow_offsets_downward_and_resolves_the_shadow_role() {
        let mut scene = Recorder::default();
        let t = theme();
        draw_shadow(
            &mut scene,
            Point::new(10.0, 10.0),
            Size::new(80.0, 40.0),
            8.0,
            SHADOW_MD,
            Some(&t),
        );
        assert_eq!(scene.shadows.len(), 1);
        let (origin, size, radius, std_dev, color) = scene.shadows[0];
        assert_eq!(origin, Point::new(10.0, 10.0 + SHADOW_MD.y_offset));
        assert_eq!(size, Size::new(80.0, 40.0));
        assert_eq!(radius, 8.0);
        assert_eq!(std_dev, SHADOW_MD.std_dev);
        assert_eq!(color.components[3], SHADOW_MD.alpha);
        // Untheme'd: still black at the rung's alpha.
        assert_eq!(SHADOW_MD.color(None), with_alpha(Color::BLACK, 0.1));
    }

    #[test]
    fn is_dark_reads_the_theme_brightness_and_defaults_light() {
        assert!(!is_dark(None));
        assert!(!is_dark(Some(&theme().with_brightness(Brightness::Light))));
        assert!(is_dark(Some(&theme().with_brightness(Brightness::Dark))));
    }
}
