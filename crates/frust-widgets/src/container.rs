//! `Container`/`colored_box`: the baseline decorated box every app currently
//! hand-rolls its own copy of (`examples/huddle`'s `FillBox`/`FilledBox`,
//! muxr's three panels + `term_box`, `examples/glyph-catalog` and
//! `examples/playground`'s `AppBackground`).
//!
//! One retained widget ([`ContainerWidget`]) backs two constructor front
//! doors — a single-child wrapper and a childless leaf — because the knob set
//! and paint discipline are identical; only whether a child is attached
//! differs.
//!
//! - [`container`] — the **single-child wrapper** family (replaces
//!   `FilledBox`, a panel, `term_box`): wraps `child`, hugging it exactly (the
//!   container's own size is always the child's size — decoration never grows
//!   the box) unless `.size_centered(...)` overrides that (see "Sizing + a
//!   child" below). Chain `.fill(...)`/`.radius(...)`/`.border(...)`/
//!   `.border_style(...)`/`.glow(...)`.
//! - [`colored_box`] — the **childless leaf/background** family (replaces
//!   `FillBox`, `AppBackground`): no content, so it needs `.expand()` (fill
//!   the available space — the full-bleed background case) or `.size(w, h)`
//!   (a fixed swatch, the `FillBox` case) to have any extent beyond the
//!   incoming minimum constraint.
//!
//! `.child(...)` switches either front door into the wrapper family
//! (`colored_box().child(x)` and `container(x)` build identical widgets), so
//! there is exactly one type ([`ContainerView`]) either way.
//!
//! # Padding composition
//!
//! `Container` carries **no** padding knob. Compose it with the existing
//! [`crate::Padding`] instead: `container(Padding(insets, child))` insets
//! the child before decoration measures it, so the fill/border/radius still
//! trace the *outer* (padded) box exactly, matching `material::card`'s own
//! content-inset shape without duplicating `Padding`'s inset math on this
//! type. A childless `colored_box()` has no child for a knob to inset in the
//! first place. This mirrors the framework's general stance
//! (`docs/CODE_STANDARDS.md`): a container widget owns one concern, and
//! composes with its siblings rather than accreting their knobs.
//!
//! # Sizing + a child
//!
//! `.expand()`/`.size(...)` only take effect while the container is
//! childless — once `.child(...)` attaches content, layout always hugs it
//! (the [`container`]/`FilledBox` case), the same way `Padding`/`SizedBox`'s
//! own child-hugging paths behave. [`ContainerView::size_centered`] is the one
//! exception, added for the `Panel::fixed` shape: it forces the container to a
//! fixed `(width, height)` **with a child attached**, loosening the child's own
//! constraint to that box (mirroring [`crate::Align`]'s `bc.loosen()` child
//! pass) and centering it in the free space — `SizedBox`'s general "grow past
//! the child" case for an arbitrary alignment stays deferred; this is the one
//! fixed-size-plus-centered-child shape this module picks up. See
//! `docs/LIMITATIONS.md`/the arc's own task record for the remaining v1 knob
//! boundary (still deferred: left-accent, bottom-rule, bleed — compose those
//! as app-side layering over a plain `Container` instead).
//!
//! ## `.expand()` under an unbounded axis
//!
//! A childless `.expand()`ed container fills the incoming max **per axis**,
//! not as a single "fill everything" gesture: a bounded axis fills to that
//! bound as expected, but an *unbounded* axis (`max == f64::INFINITY`) —
//! the shape `Flex`'s inflexible-child intrinsic-probe pass, `ScrollView`'s
//! content layout, and `ListView`'s row layout all hand a child when
//! measuring its natural extent along that axis — **collapses to
//! `bc.min()`** on that axis instead of growing without bound.
//! [`frust_core::BoxConstraints::constrain`]'s clamp cannot be relied on to
//! cap this case: `x.clamp(min, f64::INFINITY)` never lowers `x`, so any
//! finite "large enough" sentinel size would leak through unclamped and
//! report a fictitious intrinsic size to the caller (a `Flex` probing an
//! `.expand()`ed inflexible child would allocate room for it as if it were
//! ~10,000,000px wide). Collapsing to `bc.min()` — typically `0` under a
//! loose probe — is the sane floor: an unbounded expand has no real
//! "available space" to fill, so it reports the smallest size the incoming
//! constraint still allows, exactly like [`crate::sized::SizedBox`]'s
//! childless-spacer default. See [`ContainerWidget::layout`]'s
//! `Sizing::Expand` arm and `tests/container_layout.rs`'s
//! `childless_expand_collapses_to_min_under_an_unbounded_axis`/
//! `colored_box_expand_inside_a_flex_intrinsic_pass_does_not_report_the_old_sentinel`.
//!
//! With a child attached, `.expand()` has no effect at all (layout always
//! hugs the child regardless of `sizing`, per the section above) — so the
//! child's own measured size is what a `Flex`/`ScrollView`/`ListView`
//! intrinsic probe already sees in that case; there is no separate
//! expand-with-child collapse rule to apply.
//!
//! # Paint discipline
//!
//! Paint order is glow, then fill, then border, then the child — an
//! elevation-style shadow reads from *beneath* the shape it casts, mirroring
//! `material::card`'s `Elevated` variant's `draw_shadow`-then-`fill` order.
//! The border then strokes *inside* the container's own bounds — inset by
//! half the stroke width, since a stroke is centered on its path — mirroring
//! [`crate::button::ButtonWidget`]'s and `material::card`'s outlined variant's
//! identical discipline. Fill uses
//! [`frust_core::PaintScene::fill_rounded_rect_radii`] (per-corner, with the
//! uniform case its `From<f64>` case of [`CornerRadii`]); the border strokes a
//! `kurbo::RoundedRect` built from the *same* per-corner radii (inset by half
//! the stroke width per corner) so a per-corner fill and its border trace
//! identical corners — see [`ContainerView::radius`]. A container with none of
//! `.fill`/`.border`/`.glow` set paints nothing of its own — the paint pass is
//! a pure pass-through to the child in that case.
//!
//! # Semantics
//!
//! Purely decorative: [`Widget::semantics`] forwards the child's nodes
//! (`ChildPod::semantics_child`) and contributes none for the box itself —
//! unlike `material::card`, which wraps its child in a `Role::GenericContainer`
//! group node. A childless container has nothing to forward.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CornerRadii, DashPattern, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

/// Flattening tolerance for the border's rounded-rect stroke path (mirrors
/// `material::card`'s / `Button`'s identical precedent).
const BORDER_TOLERANCE: f64 = 0.1;

/// A border's stroke style, set via [`ContainerView::border_style`]. Solid by
/// default; [`ContainerView::border`] alone (with no `.border_style` call)
/// keeps the pre-existing solid-only behavior unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum BorderStyle {
    /// A continuous stroke — [`frust_core::PaintScene::stroke_path`].
    #[default]
    Solid,
    /// A dashed stroke — [`frust_core::PaintScene::stroke_path_dashed`],
    /// which breaks the same rounded-rect path this module builds for
    /// [`BorderStyle::Solid`] into the pattern's on/off runs.
    Dashed(DashPattern),
}

/// How a childless [`ContainerView`] sizes itself, or (via
/// [`Sizing::FixedCentered`] only) a `.size_centered`-forced size with a child
/// attached — see the module docs' "Sizing + a child" section.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sizing {
    /// Collapse to the incoming minimum constraint — the default, matching
    /// `SizedBox`'s childless-spacer precedent.
    Hug,
    /// Fill the incoming max constraint on both axes (the `AppBackground`
    /// case) — per-axis, so a bounded axis fills to its max while an
    /// unbounded one (`max == f64::INFINITY`) collapses to `bc.min()` on
    /// that axis instead of growing without limit. See
    /// [`ContainerWidget::layout`]'s `Sizing::Expand` arm.
    Expand,
    /// A fixed `(width, height)`, clamped into the incoming constraints (the
    /// `FillBox`/swatch case). With a child attached, layout still hugs the
    /// child instead (see the module docs) — this variant only takes effect
    /// childless.
    Fixed(f64, f64),
    /// A fixed `(width, height)`, clamped into the incoming constraints, with
    /// a child centered inside the free space — [`ContainerView::size_centered`].
    /// Unlike [`Sizing::Fixed`], this variant takes effect *with* a child
    /// attached (the `Panel::fixed` case); childless it behaves like `Fixed`.
    FixedCentered(f64, f64),
}

/// A declarative decorated box. See the [module docs](self).
pub struct ContainerView<State: 'static> {
    child: Option<AnyView<State>>,
    fill: Option<Color>,
    radius: CornerRadii,
    border: Option<(Color, f64)>,
    border_style: BorderStyle,
    /// Ambient glow/shadow: `(color, std_dev, spread)` — see
    /// [`ContainerView::glow`].
    glow: Option<(Color, f64, f64)>,
    sizing: Sizing,
}

/// Wrap `child` in a [`ContainerView`] — the single-child wrapper family. See
/// the [module docs](self).
pub fn container<State: 'static, V: View<State>>(child: V) -> ContainerView<State> {
    ContainerView {
        child: Some(any(child)),
        fill: None,
        radius: CornerRadii::default(),
        border: None,
        border_style: BorderStyle::default(),
        glow: None,
        sizing: Sizing::Hug,
    }
}

/// A childless [`ContainerView`] — the leaf/background family. See the
/// [module docs](self).
pub fn colored_box<State: 'static>() -> ContainerView<State> {
    ContainerView {
        child: None,
        fill: None,
        radius: CornerRadii::default(),
        border: None,
        border_style: BorderStyle::default(),
        glow: None,
        sizing: Sizing::Hug,
    }
}

impl<State: 'static> ContainerView<State> {
    /// Attach (or replace) the single child, switching this container into
    /// the wrapper family — the `.child(...)` counterpart to [`container`]'s
    /// direct-argument form. See the [module docs](self).
    pub fn child<V: View<State>>(mut self, child: V) -> Self {
        self.child = Some(any(child));
        self
    }

    /// Paint a solid fill behind the child (or, childless, behind whatever
    /// extent `.expand()`/`.size(...)` gives it). No fill by default.
    pub fn fill(mut self, color: Color) -> Self {
        self.fill = Some(color);
        self
    }

    /// Corner radius for the fill and border, in logical px. Accepts
    /// `impl Into<`[`CornerRadii`]`>` rather than a second `.radius_corners(...)`
    /// method: [`CornerRadii`] already implements `From<f64>` for the uniform
    /// case, so the existing `.radius(8.0)` call keeps working exactly as
    /// before while `.radius(CornerRadii::new(tl, tr, br, bl))` picks up the
    /// per-corner case (a bottom-anchored sheet with only its top corners
    /// rounded, a segmented control's end caps) for free — one method, no
    /// ergonomics lost either way. Square corners
    /// (`CornerRadii::default()`) by default.
    pub fn radius(mut self, radius: impl Into<CornerRadii>) -> Self {
        self.radius = radius.into();
        self
    }

    /// Paint a `width`-px border in `color`, stroked fully inside the
    /// container's own bounds — see the module docs' "Paint discipline"
    /// section. No border by default. Solid unless overridden with
    /// [`ContainerView::border_style`].
    pub fn border(mut self, color: Color, width: f64) -> Self {
        self.border = Some((color, width));
        self
    }

    /// Set the border's stroke style — [`BorderStyle::Solid`] (the default,
    /// so this call is only needed for [`BorderStyle::Dashed`]) or
    /// [`BorderStyle::Dashed`] with an explicit [`DashPattern`]. Has no
    /// effect without a `.border(...)` call — there is no stroke to style.
    pub fn border_style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Paint a gaussian-blurred ambient glow/shadow beneath the fill and
    /// border, via [`frust_core::PaintScene::draw_shadow`] — an
    /// elevation-style knob, not a directional drop shadow (see below). No
    /// glow by default.
    ///
    /// # Mapping to CSS `box-shadow` terms
    ///
    /// `.glow(color, std_dev, spread)` maps loosely onto
    /// `box-shadow: 0 0 <blur> <spread> <color>` (an un-offset, centered
    /// shadow — see the note on offset below):
    ///
    /// - `color` — the shadow's color, alpha included (CSS `<color>`).
    /// - `std_dev` — the Gaussian's standard deviation. CSS's `blur-radius` is
    ///   *twice* the standard deviation (CSS Backgrounds and Borders 3
    ///   § 7.2.1), so a blur token translates in as `std_dev = blur / 2.0` —
    ///   the same halving `frust_material::card`/`frust_shadcn::style::draw_shadow`
    ///   already apply at their own call sites.
    /// - `spread` — grows (positive) or shrinks (negative) the shadow's rect
    ///   symmetrically on every side *before* blurring. `draw_shadow` itself
    ///   has no spread parameter (`frust_shadcn::style::draw_shadow`'s own doc
    ///   drops it for the same reason), so this module computes the inflated
    ///   rect directly — `origin` shifts by `-spread` on each axis, `size`
    ///   grows by `2.0 * spread`, clamped to non-negative before reaching
    ///   `draw_shadow`.
    /// - `border-radius` — a per-corner [`ContainerView::radius`] lowers
    ///   through [`CornerRadii::largest`] for this call (`draw_shadow` takes a
    ///   single `f64` radius, the same fallback
    ///   [`Command::BlurredRoundedRect`](frust_scene::Command::BlurredRoundedRect)'s
    ///   own doc names for a per-corner shadow caster) — but never
    ///   *unclamped*: kurbo silently caps a `RoundedRect`'s own corner radius
    ///   at half its shorter side, so the *fill's* true rendered corner is
    ///   `radius.largest().min(size.min_side() / 2.0)`, not the raw builder
    ///   value. Feeding an unclamped radius (e.g. a `.radius(f64::MAX)`-style
    ///   pill/circle idiom) straight into the blurred-rect primitive on a
    ///   small box is exactly the CSS `box-shadow` `spread` + `border-radius`
    ///   interaction: per CSS Backgrounds and Borders 3 § 7.2.1, spreading a
    ///   shadow grows its corner radii by the spread distance too (clamped at
    ///   0), so this module adds `spread` on top of the *clamped* fill
    ///   radius — `shadow_radius = clamped_fill_radius + spread.max(0.0)` —
    ///   rather than to the raw builder value or to a radius re-clamped
    ///   against the already-inflated shadow rect. This is what keeps a
    ///   circular fill's halo circular instead of degenerating into the
    ///   oversized dark ring a naive `radius.largest()` passthrough painted
    ///   around a small pulsing-dot circle.
    /// - offset-x/offset-y are **not modeled**: `.glow` is a centered ambient
    ///   glow, not a directional drop shadow like `material::card`'s
    ///   `Elevated` variant (which offsets `origin.y` by the shadow rung's
    ///   `y_offset`). A caller wanting a directional shadow composes an
    ///   explicit `PaintScene::draw_shadow` call instead, following that
    ///   precedent.
    pub fn glow(mut self, color: Color, std_dev: f64, spread: f64) -> Self {
        self.glow = Some((color, std_dev, spread));
        self
    }

    /// Fill the available space on both axes (the incoming constraint's
    /// max) — the `AppBackground` full-bleed-background case. Childless
    /// only; see the module docs' "Sizing + a child" section.
    pub fn expand(mut self) -> Self {
        self.sizing = Sizing::Expand;
        self
    }

    /// Force a fixed `(width, height)`, clamped into the incoming
    /// constraints — the `FillBox`/swatch case. Childless only (a child stays
    /// hugged tight); use [`ContainerView::size_centered`] for a fixed size
    /// with a centered child. See the module docs' "Sizing + a child" section.
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.sizing = Sizing::Fixed(width, height);
        self
    }

    /// Force a fixed `(width, height)`, clamped into the incoming
    /// constraints, **with an attached child centered inside the free
    /// space** — the `Panel::fixed` case (a settings panel/dialog/card that
    /// wants a fixed footprint but lets its content size itself rather than
    /// being stretched to fill it). Unlike [`ContainerView::size`], this
    /// loosens the child's own constraint to `(width, height)` (mirroring
    /// [`crate::Align`]'s child pass) instead of hugging it tight, then
    /// centers the child in whatever free space remains — see the module
    /// docs' "Sizing + a child" section. Childless, this behaves exactly like
    /// [`ContainerView::size`].
    pub fn size_centered(mut self, width: f64, height: f64) -> Self {
        self.sizing = Sizing::FixedCentered(width, height);
        self
    }
}

/// The retained widget for a [`ContainerView`]. See the [module docs](self).
pub struct ContainerWidget {
    child: Option<ChildPod>,
    fill: Option<Color>,
    radius: CornerRadii,
    border: Option<(Color, f64)>,
    border_style: BorderStyle,
    glow: Option<(Color, f64, f64)>,
    sizing: Sizing,
}

impl<State: 'static> View<State> for ContainerView<State> {
    type Element = ContainerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ContainerWidget {
        ContainerWidget {
            child: self
                .child
                .as_ref()
                .map(|view| crate::authoring::build_child(view, ctx)),
            fill: self.fill,
            radius: self.radius,
            border: self.border,
            border_style: self.border_style,
            glow: self.glow,
            sizing: self.sizing,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ContainerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.fill != self.fill
            || prev.radius != self.radius
            || prev.border != self.border
            || prev.border_style != self.border_style
            || prev.glow != self.glow
        {
            element.fill = self.fill;
            element.radius = self.radius;
            element.border = self.border;
            element.border_style = self.border_style;
            element.glow = self.glow;
            flags |= ChangeFlags::PAINT;
        }
        if prev.sizing != self.sizing {
            element.sizing = self.sizing;
            flags |= ChangeFlags::LAYOUT;
        }
        match (&prev.child, &self.child, &mut element.child) {
            (Some(prev_view), Some(next_view), Some(pod)) => {
                flags |= crate::authoring::rebuild_child(prev_view, next_view, pod, ctx);
            }
            (None, Some(next_view), _) => {
                element.child = Some(crate::authoring::build_child(next_view, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(prev_view), None, Some(pod)) => {
                crate::authoring::teardown_child(prev_view, pod, ctx);
                element.child = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            _ => {}
        }
        flags
    }

    fn teardown(&self, element: &mut ContainerWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(view), Some(pod)) = (&self.child, &mut element.child) {
            crate::authoring::teardown_child(view, pod, ctx);
        }
    }
}

/// Insets each of `radii`'s four corners by `inset` (e.g. half a border's
/// stroke width), clamped to non-negative — the per-corner generalization of
/// the uniform `(self.radius - half).max(0.0)` inset the border path always
/// applied, so a per-corner fill and its border stroke agree on every corner.
fn inset_radii(radii: CornerRadii, inset: f64) -> CornerRadii {
    CornerRadii::new(
        (radii.top_left - inset).max(0.0),
        (radii.top_right - inset).max(0.0),
        (radii.bottom_right - inset).max(0.0),
        (radii.bottom_left - inset).max(0.0),
    )
}

/// One axis of a childless `.expand()`ed container's intrinsic size: `max`
/// when it is a real bound, else `min` — the fallback for an unbounded axis
/// (`max == f64::INFINITY`), where `BoxConstraints::constrain`'s clamp is a
/// no-op (`x.clamp(min, INFINITY)` never lowers `x`) and can't be relied on
/// to cap an arbitrary sentinel the way it caps a genuinely bounded axis. See
/// [`Sizing::Expand`]'s doc for why `min` (not e.g. zero) is the right floor.
fn collapse_or_fill(max: f64, min: f64) -> f64 {
    if max.is_finite() { max } else { min }
}

impl Widget for ContainerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        match &mut self.child {
            Some(pod) => {
                if let Sizing::FixedCentered(w, h) = self.sizing {
                    // Force the fixed size, then loosen the child's own
                    // constraint to it (mirrors `Align`'s `bc.loosen()` child
                    // pass) and center the child in the free space — the
                    // `Panel::fixed` case (see the module docs).
                    let own_size = bc.constrain(Size::new(w, h));
                    let child_size = pod.layout_child(ctx, &BoxConstraints::loose(own_size));
                    let x = ((own_size.width - child_size.width) / 2.0).max(0.0);
                    let y = ((own_size.height - child_size.height) / 2.0).max(0.0);
                    pod.set_origin(Point::new(x, y));
                    own_size
                } else {
                    // Always hug the child exactly — decoration never grows
                    // the box (see the module docs' "Sizing + a child"
                    // section).
                    let child_size = pod.layout_child(ctx, bc);
                    pod.set_origin(Point::ZERO);
                    bc.constrain(child_size)
                }
            }
            None => {
                let intrinsic = match self.sizing {
                    Sizing::Hug => bc.min(),
                    // Fill the incoming max, per axis — except an unbounded
                    // axis (`max == INFINITY`, the intrinsic-probe shape
                    // `Flex`'s inflexible-child pass/`ScrollView`/`ListView`
                    // hand a childless expanding box), which collapses to
                    // `bc.min()` on that axis instead of reporting an
                    // arbitrary sentinel. `bc.constrain` below is then a
                    // no-op on a bounded axis (the chosen value already sits
                    // at `bc.max()`) and only does real clamping work when
                    // `bc.min() > 0` on the collapsed axis.
                    Sizing::Expand => Size::new(
                        collapse_or_fill(bc.max().width, bc.min().width),
                        collapse_or_fill(bc.max().height, bc.min().height),
                    ),
                    Sizing::Fixed(w, h) | Sizing::FixedCentered(w, h) => Size::new(w, h),
                };
                bc.constrain(intrinsic)
            }
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        // Glow paints first — an elevation-style shadow reads from beneath
        // the shape it casts (see the module docs' "Paint discipline"
        // section).
        if let Some((color, std_dev, spread)) = self.glow {
            let shadow_origin = Point::new(origin.x - spread, origin.y - spread);
            let shadow_size = Size::new(
                (size.width + 2.0 * spread).max(0.0),
                (size.height + 2.0 * spread).max(0.0),
            );
            // Clamp to the *fill's own* rendered corner — kurbo already caps
            // a `RoundedRect`'s radius at half its shorter side, so an
            // unclamped `radius.largest()` (e.g. a `.radius(999.0)`
            // pill/circle idiom) fed straight into the blurred-rect
            // primitive on a small box paints a dark blob far larger than
            // the shape it's supposed to halo (see `.glow`'s doc). `spread`
            // then adds onto this *clamped* radius, not the raw builder
            // value and not a radius re-clamped against the already-inflated
            // `shadow_size` — the CSS `box-shadow` spread+border-radius rule
            // this mirrors grows the caster's own corner, so a circular fill
            // keeps a circular halo instead of the shadow's roundness being
            // capped by its own inflated box.
            let clamped_fill_radius = self.radius.largest().min(size.width.min(size.height) / 2.0);
            let shadow_radius = clamped_fill_radius + spread.max(0.0);
            scene.draw_shadow(shadow_origin, shadow_size, shadow_radius, std_dev, color);
        }
        if let Some(fill) = self.fill {
            scene.fill_rounded_rect_radii(origin, size, self.radius, fill);
        }
        if let Some((color, width)) = self.border {
            // Inset by half the stroke width so the border paints fully
            // inside the container's own bounds (a stroke is centered on its
            // path) — mirrors `Button`'s/`material::card`'s outlined-variant
            // precedent, generalized per corner via `inset_radii` so the
            // border agrees with a per-corner fill.
            let half = width / 2.0;
            let radii = inset_radii(self.radius, half);
            let rr = RoundedRect::new(
                half,
                half,
                size.width - half,
                size.height - half,
                (
                    radii.top_left,
                    radii.top_right,
                    radii.bottom_right,
                    radii.bottom_left,
                ),
            );
            let path = rr.to_path(BORDER_TOLERANCE);
            match self.border_style {
                BorderStyle::Solid => scene.stroke_path(origin, &path, width, &Brush::Solid(color)),
                BorderStyle::Dashed(dash) => {
                    scene.stroke_path_dashed(origin, &path, width, dash, &Brush::Solid(color))
                }
            }
        }
        if let Some(pod) = &mut self.child {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match &mut self.child {
            Some(pod) => crate::authoring::route_event_single(pod, ctx, event),
            None => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Purely decorative: forward the child's nodes, contribute none for
        // the box itself (see the module docs' "Semantics" section).
        if let Some(pod) = &self.child {
            pod.semantics_child(ctx);
        }
    }

    crate::authoring::visit_children!(child);
}
