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
//!   the box). Chain `.fill(...)`/`.radius(...)`/`.border(...)`.
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
//! own child-hugging paths behave. Combining a fixed/expanded box with an
//! *un*-hugged, centered child (`SizedBox`'s "grow past the child" shape) is
//! deliberately deferred — see `docs/LIMITATIONS.md`/the arc's own task
//! record for the full v1 knob boundary (also deferred: dashed border,
//! per-corner radius, glow/shadow, left-accent, bottom-rule, bleed).
//!
//! # Paint discipline
//!
//! Fill paints first, then the border strokes *inside* the container's own
//! bounds — inset by half the stroke width, since a stroke is centered on its
//! path — mirroring [`crate::button::ButtonWidget`]'s and
//! `material::card`'s outlined variant's identical discipline. Both use the
//! shared uniform-radius [`frust_core::PaintScene::fill_rounded_rect`]. A
//! container with neither `.fill` nor `.border` set paints nothing of its
//! own — the paint pass is a pure pass-through to the child in that case.
//!
//! # Semantics
//!
//! Purely decorative: [`Widget::semantics`] forwards the child's nodes
//! (`ChildPod::semantics_child`) and contributes none for the box itself —
//! unlike `material::card`, which wraps its child in a `Role::GenericContainer`
//! group node. A childless container has nothing to forward.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

/// Flattening tolerance for the border's rounded-rect stroke path (mirrors
/// `material::card`'s / `Button`'s identical precedent).
const BORDER_TOLERANCE: f64 = 0.1;

/// Declared larger than any real viewport (logical px) so
/// [`BoxConstraints::constrain`] always clamps a childless `.expand()`ed
/// container down to the incoming max — the same trick
/// `examples/glyph-catalog`/`examples/playground`'s hand-rolled
/// `AppBackground` used before this widget existed (see the module docs).
const EXPAND_INTRINSIC: f64 = 1.0e7;

/// How a childless [`ContainerView`] sizes itself — ignored once a child is
/// attached (see the module docs' "Sizing + a child" section).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sizing {
    /// Collapse to the incoming minimum constraint — the default, matching
    /// `SizedBox`'s childless-spacer precedent.
    Hug,
    /// Fill the incoming max constraint on both axes (the `AppBackground`
    /// case) via [`EXPAND_INTRINSIC`].
    Expand,
    /// A fixed `(width, height)`, clamped into the incoming constraints (the
    /// `FillBox`/swatch case).
    Fixed(f64, f64),
}

/// A declarative decorated box. See the [module docs](self).
pub struct ContainerView<State: 'static> {
    child: Option<AnyView<State>>,
    fill: Option<Color>,
    radius: f64,
    border: Option<(Color, f64)>,
    sizing: Sizing,
}

/// Wrap `child` in a [`ContainerView`] — the single-child wrapper family. See
/// the [module docs](self).
pub fn container<State: 'static, V: View<State>>(child: V) -> ContainerView<State> {
    ContainerView {
        child: Some(any(child)),
        fill: None,
        radius: 0.0,
        border: None,
        sizing: Sizing::Hug,
    }
}

/// A childless [`ContainerView`] — the leaf/background family. See the
/// [module docs](self).
pub fn colored_box<State: 'static>() -> ContainerView<State> {
    ContainerView {
        child: None,
        fill: None,
        radius: 0.0,
        border: None,
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

    /// Uniform corner radius for the fill and border, in logical px. `0.0`
    /// (square corners) by default.
    pub fn radius(mut self, radius: f64) -> Self {
        self.radius = radius;
        self
    }

    /// Paint a `width`-px border in `color`, stroked fully inside the
    /// container's own bounds — see the module docs' "Paint discipline"
    /// section. No border by default.
    pub fn border(mut self, color: Color, width: f64) -> Self {
        self.border = Some((color, width));
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
    /// constraints — the `FillBox`/swatch case. Childless only; see the
    /// module docs' "Sizing + a child" section.
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.sizing = Sizing::Fixed(width, height);
        self
    }
}

/// The retained widget for a [`ContainerView`]. See the [module docs](self).
pub struct ContainerWidget {
    child: Option<ChildPod>,
    fill: Option<Color>,
    radius: f64,
    border: Option<(Color, f64)>,
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
        if prev.fill != self.fill || prev.radius != self.radius || prev.border != self.border {
            element.fill = self.fill;
            element.radius = self.radius;
            element.border = self.border;
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

impl Widget for ContainerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        match &mut self.child {
            Some(pod) => {
                // Always hug the child exactly — decoration never grows the
                // box (see the module docs' "Sizing + a child" section).
                let child_size = pod.layout_child(ctx, bc);
                pod.set_origin(Point::ZERO);
                bc.constrain(child_size)
            }
            None => {
                let intrinsic = match self.sizing {
                    Sizing::Hug => bc.min(),
                    Sizing::Expand => Size::new(EXPAND_INTRINSIC, EXPAND_INTRINSIC),
                    Sizing::Fixed(w, h) => Size::new(w, h),
                };
                bc.constrain(intrinsic)
            }
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        if let Some(fill) = self.fill {
            scene.fill_rounded_rect(origin, size, self.radius, fill);
        }
        if let Some((color, width)) = self.border {
            // Inset by half the stroke width so the border paints fully
            // inside the container's own bounds (a stroke is centered on its
            // path) — mirrors `Button`'s/`material::card`'s outlined-variant
            // precedent.
            let half = width / 2.0;
            let rr = RoundedRect::new(
                half,
                half,
                size.width - half,
                size.height - half,
                (self.radius - half).max(0.0),
            );
            let path = rr.to_path(BORDER_TOLERANCE);
            scene.stroke_path(origin, &path, width, &Brush::Solid(color));
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
