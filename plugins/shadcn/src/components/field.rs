//! `field`: the form-row composition — label, control, description, error
//! message — in one of three orientations.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/field.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17). The
//! `fieldVariants` cva has one base (`flex w-full gap-3
//! data-[invalid=true]:text-destructive`) and the three orientation arms this
//! module's [`FieldOrientation`] ports.
//!
//! # What this ports, and what it collapses
//!
//! Upstream ships nine composable pieces (`FieldSet`, `FieldLegend`,
//! `FieldGroup`, `Field`, `FieldContent`, `FieldLabel`, `FieldTitle`,
//! `FieldDescription`, `FieldError`, `FieldSeparator`) whose whole job in the DOM
//! is to carry the classes a *flex layout* needs. This port keeps the piece with
//! behavior — the field row itself, with its label/description/error slots and
//! their invalid/disabled treatments — and expresses the rest as layout inside
//! this one widget. An app that wants a fieldset stacks fields in a `Column`.
//!
//! # Orientation
//!
//! - [`FieldOrientation::Vertical`] — `flex-col`, every slot full width, `gap-3`
//!   between slots (Tailwind's `gap-*` applies between *all* flex children, which
//!   is why the label/description spacing is the same 12px as the control's).
//! - [`FieldOrientation::Horizontal`] — `flex-row items-center`: the label takes
//!   the remaining width (`[&>[data-slot=field-label]]:flex-auto`) and the control
//!   sits at its natural width on the trailing edge, vertically centered.
//! - [`FieldOrientation::Responsive`] — vertical below
//!   [`RESPONSIVE_BREAKPOINT`], horizontal at or above it. Upstream's
//!   `@md/field-group:flex-row` is a *container* query against the enclosing
//!   field group, and a widget's incoming constraints are exactly that: the width
//!   its container is giving it.
//!
//! **One divergence:** in a horizontal field the description and error do not
//! join the row (a four-item flex row squeezes prose into a column of two words);
//! they stay full-width rows underneath, which is what upstream's own
//! `FieldContent` composition produces for the same content.
//!
//! # Invalid and disabled
//!
//! `data-[invalid=true]:text-destructive` cascades to the field's own text, so an
//! [`error`](FieldView::error) message tints the **label** destructive along with
//! itself. The description keeps `text-muted-foreground` — its own class wins over
//! the cascade in the source, and it does here too. Disabled dims the label and
//! description (`group-data-[disabled=true]/field:opacity-50`) through a paint
//! layer, leaving the control to dim itself through its own builder.

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, ThemeTextColor,
    ThemeTextType, View, Widget, any, build_child, rebuild_child, route_event, teardown_child,
    text::FontWeight, visit_children,
};
use frust::{TextView, text};

use crate::style;

/// The container width at which a [`FieldOrientation::Responsive`] field turns
/// horizontal: Tailwind's `@md` container-query breakpoint, `28rem` at the CSS
/// default 16px root font size.
///
/// Source: `tailwindcss@4.3.0`'s `--container-md`, the scale
/// `@md/field-group:flex-row` resolves against (the same pin
/// [`crate::style`] takes its spacing/shadow scales from).
pub const RESPONSIVE_BREAKPOINT: f64 = 448.0;

/// How a field arranges its label and control — upstream's `fieldVariants`
/// `orientation` axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FieldOrientation {
    /// `flex-col` — label above control. The default.
    #[default]
    Vertical,
    /// `flex-row items-center` — label beside control.
    Horizontal,
    /// Vertical below [`RESPONSIVE_BREAKPOINT`], horizontal at or above it.
    Responsive,
}

impl FieldOrientation {
    /// Whether this orientation lays the label beside the control at `width`.
    pub fn is_row(self, width: f64) -> bool {
        match self {
            FieldOrientation::Vertical => false,
            FieldOrientation::Horizontal => true,
            FieldOrientation::Responsive => width >= RESPONSIVE_BREAKPOINT,
        }
    }
}

/// Which slot each entry of [`FieldWidget::children`] holds. The control is
/// always present; the other three are optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slots {
    label: Option<usize>,
    control: usize,
    description: Option<usize>,
    error: Option<usize>,
}

/// A declarative form field. See the [module docs](self).
pub struct FieldView<State: 'static> {
    control: AnyView<State>,
    label: Option<String>,
    description: Option<String>,
    error: Option<String>,
    orientation: FieldOrientation,
    disabled: bool,
}

/// Wrap `control` in a field row; add the text slots with
/// [`label`](FieldView::label)/[`description`](FieldView::description)/
/// [`error`](FieldView::error).
pub fn field<State: 'static, V: View<State>>(control: V) -> FieldView<State> {
    FieldView {
        control: any(control),
        label: None,
        description: None,
        error: None,
        orientation: FieldOrientation::default(),
        disabled: false,
    }
}

impl<State: 'static> FieldView<State> {
    /// Set the field's label (`text-sm font-medium`, destructive while the field
    /// carries an error).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the helper text below the control (`text-sm text-muted-foreground`).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the error message (`role="alert"`, `text-sm text-destructive`). Its
    /// presence is what puts the field in its invalid state.
    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Set the orientation (default [`FieldOrientation::Vertical`]).
    pub fn orientation(mut self, orientation: FieldOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Dim the label and description. The control dims through its own builder
    /// (see the [module docs](self)).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The label's text view: destructive ink while the field is invalid.
    fn label_view(&self) -> Option<AnyView<State>> {
        let role = if self.error.is_some() {
            ThemeTextColor::Error
        } else {
            ThemeTextColor::OnSurface
        };
        self.label.as_ref().map(|label| {
            any(field_text(label)
                .weight(FontWeight::MEDIUM)
                .themed_family(ThemeTextType::LabelLarge)
                .themed_role(role))
        })
    }

    /// The description's text view (always `muted-foreground`).
    fn description_view(&self) -> Option<AnyView<State>> {
        self.description
            .as_ref()
            .map(|text| any(field_text(text).themed_role(ThemeTextColor::OnSurfaceVariant)))
    }

    /// The error message's text view.
    fn error_view(&self) -> Option<AnyView<State>> {
        self.error
            .as_ref()
            .map(|text| any(field_text(text).themed_role(ThemeTextColor::Error)))
    }

    /// The presence-of-slots shape; a change forces a full child rebuild.
    fn shape(&self) -> (bool, bool, bool) {
        (
            self.label.is_some(),
            self.description.is_some(),
            self.error.is_some(),
        )
    }
}

/// A field slot's base text style: `text-sm`, in the live theme's
/// `BodyMedium` family (the label names its own role over it).
fn field_text(label: &str) -> TextView {
    text(label.to_string())
        .size(style::TEXT_SM as f32)
        .themed_family(ThemeTextType::BodyMedium)
}

/// The retained widget for a [`FieldView`].
pub struct FieldWidget {
    children: Vec<ChildPod>,
    slots: Slots,
    orientation: FieldOrientation,
    disabled: bool,
}

/// Build the ordered child window `[label?, control, description?, error?]` and
/// its [`Slots`] index map.
fn build_children<State: 'static>(
    view: &FieldView<State>,
    ctx: &mut BuildCtx<'_>,
) -> (Vec<ChildPod>, Slots) {
    let mut children = Vec::new();
    let label = view.label_view().map(|v| {
        children.push(build_child(&v, ctx));
        children.len() - 1
    });
    children.push(build_child(&view.control, ctx));
    let control = children.len() - 1;
    let description = view.description_view().map(|v| {
        children.push(build_child(&v, ctx));
        children.len() - 1
    });
    let error = view.error_view().map(|v| {
        children.push(build_child(&v, ctx));
        children.len() - 1
    });
    (
        children,
        Slots {
            label,
            control,
            description,
            error,
        },
    )
}

/// Tear every child down through the view it was built from.
fn teardown_children<State: 'static>(
    view: &FieldView<State>,
    slots: Slots,
    children: &mut [ChildPod],
    ctx: &mut BuildCtx<'_>,
) {
    for (index, pod) in children.iter_mut().enumerate() {
        if Some(index) == slots.label
            && let Some(v) = view.label_view()
        {
            teardown_child(&v, pod, ctx);
        } else if index == slots.control {
            teardown_child(&view.control, pod, ctx);
        } else if Some(index) == slots.description
            && let Some(v) = view.description_view()
        {
            teardown_child(&v, pod, ctx);
        } else if Some(index) == slots.error
            && let Some(v) = view.error_view()
        {
            teardown_child(&v, pod, ctx);
        }
    }
}

impl<State: 'static> View<State> for FieldView<State> {
    type Element = FieldWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FieldWidget {
        let (children, slots) = build_children(self, ctx);
        FieldWidget {
            children,
            slots,
            orientation: self.orientation,
            disabled: self.disabled,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FieldWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if self.shape() != prev.shape() {
            // Slot presence changed: tear the whole set down against the previous
            // view (whose shape the live children still match) and rebuild.
            teardown_children(prev, element.slots, &mut element.children, ctx);
            let (children, slots) = build_children(self, ctx);
            element.children = children;
            element.slots = slots;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            let slots = element.slots;
            if let (Some(p), Some(n)) = (prev.label_view(), self.label_view())
                && let Some(i) = slots.label
            {
                flags |= rebuild_child(&p, &n, &mut element.children[i], ctx);
            }
            flags |= rebuild_child(
                &prev.control,
                &self.control,
                &mut element.children[slots.control],
                ctx,
            );
            if let (Some(p), Some(n)) = (prev.description_view(), self.description_view())
                && let Some(i) = slots.description
            {
                flags |= rebuild_child(&p, &n, &mut element.children[i], ctx);
            }
            if let (Some(p), Some(n)) = (prev.error_view(), self.error_view())
                && let Some(i) = slots.error
            {
                flags |= rebuild_child(&p, &n, &mut element.children[i], ctx);
            }
        }
        if element.orientation != self.orientation {
            element.orientation = self.orientation;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.disabled != self.disabled {
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut FieldWidget, ctx: &mut BuildCtx<'_>) {
        teardown_children(self, element.slots, &mut element.children, ctx);
    }
}

impl Widget for FieldWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let gap = style::spacing(3.0);
        let row = self.orientation.is_row(width);
        let full = BoxConstraints::new(Size::ZERO, Size::new(width, f64::INFINITY));

        let mut y = 0.0;
        if row {
            // `flex-row items-center`: the label is `flex-auto` (it takes the width
            // the control leaves), the control keeps its natural width.
            let control_natural = self.children[self.slots.control].layout_child(ctx, &full);
            let label_width = (width - control_natural.width - gap).max(0.0);
            let label_size = self.slots.label.map(|i| {
                self.children[i].layout_child(
                    ctx,
                    &BoxConstraints::new(Size::ZERO, Size::new(label_width, f64::INFINITY)),
                )
            });
            let row_height = control_natural
                .height
                .max(label_size.map_or(0.0, |s| s.height));
            if let (Some(i), Some(size)) = (self.slots.label, label_size) {
                self.children[i].set_origin(Point::new(0.0, (row_height - size.height) / 2.0));
            }
            self.children[self.slots.control].set_origin(Point::new(
                width - control_natural.width,
                (row_height - control_natural.height) / 2.0,
            ));
            y = row_height;
        } else {
            // `flex-col`: label, then control, each full width, `gap-3` apart.
            if let Some(i) = self.slots.label {
                let size = self.children[i].layout_child(ctx, &full);
                self.children[i].set_origin(Point::new(0.0, y));
                y += size.height + gap;
            }
            let size = self.children[self.slots.control].layout_child(ctx, &full);
            self.children[self.slots.control].set_origin(Point::new(0.0, y));
            y += size.height;
        }

        // Description and error are always full-width rows below the control (see
        // the module docs' divergence note).
        for i in [self.slots.description, self.slots.error]
            .into_iter()
            .flatten()
        {
            let size = self.children[i].layout_child(ctx, &full);
            self.children[i].set_origin(Point::new(0.0, y + gap));
            y += gap + size.height;
        }

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The field paints no chrome of its own — it is a layout plus the text
        // slots' own ink. Disabled dims the *text* slots only: the control carries
        // its own disabled treatment, and dimming it twice would compound.
        let dim = self.disabled;
        for (index, pod) in self.children.iter_mut().enumerate() {
            let text_slot = index != self.slots.control;
            if dim && text_slot {
                scene.push_layer(
                    ctx.origin() + pod.origin().to_vec2(),
                    pod.size(),
                    style::DISABLED_OPACITY,
                );
                pod.paint_child(ctx, scene);
                scene.pop_layer();
            } else {
                pod.paint_child(ctx, scene);
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // No interaction of its own: the control (and any interactive slot
        // content) owns every event, including the focus/capture fast paths the
        // helper handles.
        route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let error = self.slots.error;
        ctx.push_container(
            Role::Group,
            |_node| {},
            |ctx| {
                for (index, pod) in self.children.iter().enumerate() {
                    if Some(index) == error {
                        // `role="alert"` upstream: the message is a live
                        // announcement, not just labelling text.
                        ctx.push_container(Role::Alert, |_node| {}, |ctx| pod.semantics_child(ctx));
                    } else {
                        pod.semantics_child(ctx);
                    }
                }
            },
        );
    }

    visit_children!(children);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Theme;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Brush, Color, PointerButton, PointerEvent, PointerPhase};
    use frust::{Brightness, FrameTime};
    use frust_core::RenderRoot;
    use frust_widgets::test_support::leaf;
    use std::any::Any;

    /// A recording scene capturing the glyph colors the text slots bake in, plus
    /// the `push_layer` alphas the disabled treatment uses.
    #[derive(Default)]
    struct Recorder {
        glyph_colors: Vec<Color>,
        layers: Vec<(Point, Size, f32)>,
        rects: Vec<(Point, Size, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            // A text slot's ink is baked into its shaped run's brush at layout
            // time — which is exactly what these tests assert on.
            if let Brush::Solid(color) = run.brush {
                self.glyph_colors.push(color);
            }
        }
        fn push_layer(&mut self, origin: Point, size: Size, alpha: f32) {
            self.layers.push((origin, size, alpha));
        }
        fn pop_layer(&mut self) {}
        fn stroke_path(
            &mut self,
            _origin: Point,
            _path: &frust::authoring::BezPath,
            _width: f64,
            _brush: &Brush,
        ) {
        }
    }

    const WIDE: Size = Size::new(600.0, 400.0);
    const NARROW: Size = Size::new(320.0, 400.0);

    fn build<S: 'static>(view: &FieldView<S>) -> FieldWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut FieldWidget, size: Size) -> Size {
        layout_themed(w, size, None)
    }

    /// Lay out against `theme`. A text slot's ink is resolved and baked into its
    /// shaped run at **layout** time (`Text`'s documented layout-baked
    /// resolution), so a themed color assertion has to thread the theme here, not
    /// only into the paint pass.
    fn layout_themed(w: &mut FieldWidget, size: Size, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::loose(size))
    }

    fn paint(w: &mut FieldWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        if let Some(theme) = theme {
            let mut ctx = ctx.with_theme(theme as &dyn Any);
            w.paint(&mut ctx, &mut rec);
        } else {
            w.paint(&mut ctx, &mut rec);
        }
        rec
    }

    /// A field with all four slots over a fixed-size control.
    fn full_field(orientation: FieldOrientation) -> FieldView<()> {
        field(leaf(120.0, 36.0))
            .label("Email")
            .description("We never share it.")
            .orientation(orientation)
    }

    #[test]
    fn a_vertical_field_stacks_every_slot_full_width_with_gap_3() {
        let mut w = build(&full_field(FieldOrientation::Vertical));
        let size = layout(&mut w, WIDE);
        let gap = style::spacing(3.0);
        let label = w.slots.label.expect("label");
        let control = w.slots.control;
        let description = w.slots.description.expect("description");

        assert_eq!(w.children[label].origin(), Point::ORIGIN);
        assert_eq!(
            w.children[control].origin().y,
            w.children[label].size().height + gap
        );
        assert_eq!(
            w.children[description].origin().y,
            w.children[control].origin().y + w.children[control].size().height + gap
        );
        // Every stacked slot starts at the leading edge, and the field is as tall
        // as its content.
        assert_eq!(w.children[description].origin().x, 0.0);
        assert_eq!(
            size.height,
            w.children[description].origin().y + w.children[description].size().height
        );
        assert_eq!(size.width, WIDE.width);
    }

    #[test]
    fn a_horizontal_field_puts_the_control_on_the_trailing_edge_center_aligned() {
        let mut w = build(&full_field(FieldOrientation::Horizontal));
        let size = layout(&mut w, WIDE);
        let label = w.slots.label.expect("label");
        let control = w.slots.control;

        assert_eq!(w.children[control].size(), Size::new(120.0, 36.0));
        assert_eq!(
            w.children[control].origin().x,
            WIDE.width - 120.0,
            "the control sits at the trailing edge"
        );
        assert_eq!(w.children[label].origin().x, 0.0);
        // items-center: both are centered on the row.
        let row_height = 36.0f64.max(w.children[label].size().height);
        assert_eq!(
            w.children[label].origin().y,
            (row_height - w.children[label].size().height) / 2.0
        );
        assert_eq!(w.children[control].origin().y, 0.0);
        // The label is `flex-auto`: it gets the width the control leaves.
        assert!(w.children[label].size().width <= WIDE.width - 120.0 - style::spacing(3.0));
        // The description still rides below the row.
        let description = w.slots.description.expect("description");
        assert!(w.children[description].origin().y >= row_height);
        assert!(size.height > row_height);
    }

    #[test]
    fn a_responsive_field_switches_at_the_container_breakpoint() {
        assert!(!FieldOrientation::Responsive.is_row(RESPONSIVE_BREAKPOINT - 1.0));
        assert!(FieldOrientation::Responsive.is_row(RESPONSIVE_BREAKPOINT));
        assert!(!FieldOrientation::Vertical.is_row(f64::MAX));
        assert!(FieldOrientation::Horizontal.is_row(0.0));

        // Narrow: stacked (the control starts below the label).
        let mut w = build(&full_field(FieldOrientation::Responsive));
        layout(&mut w, NARROW);
        let label = w.slots.label.expect("label");
        assert!(w.children[w.slots.control].origin().y > 0.0);
        assert_eq!(w.children[label].origin().y, 0.0);

        // Wide: the same widget lays out as a row on the next pass.
        layout(&mut w, WIDE);
        assert_eq!(
            w.children[w.slots.control].origin().x,
            WIDE.width - 120.0,
            "at or above the breakpoint the control moves beside the label"
        );
    }

    #[test]
    fn an_error_tints_the_label_destructive_and_leaves_the_description_muted() {
        let theme = crate::theme().with_brightness(Brightness::Light);
        let scheme = theme.scheme();

        let clean: FieldView<()> = field(leaf(120.0, 36.0))
            .label("Email")
            .description("We never share it.");
        let mut w = build(&clean);
        let size = layout_themed(&mut w, WIDE, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(
            rec.glyph_colors,
            vec![scheme.on_surface, scheme.on_surface_variant],
            "label in foreground, description in muted-foreground"
        );

        let invalid: FieldView<()> = field(leaf(120.0, 36.0))
            .label("Email")
            .description("We never share it.")
            .error("Enter a valid address.");
        let mut w = build(&invalid);
        let size = layout_themed(&mut w, WIDE, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(
            rec.glyph_colors,
            vec![scheme.error, scheme.on_surface_variant, scheme.error],
            "the invalid cascade tints the label and the message, never the description"
        );
    }

    #[test]
    fn a_disabled_field_dims_only_its_text_slots() {
        let disabled: FieldView<()> = field(leaf(120.0, 36.0))
            .label("Email")
            .description("Locked")
            .disabled(true);
        let mut w = build(&disabled);
        let size = layout(&mut w, WIDE);
        let rec = paint(&mut w, size, None);
        assert_eq!(rec.layers.len(), 2, "the label and the description");
        assert!(
            rec.layers
                .iter()
                .all(|(_, _, alpha)| *alpha == style::DISABLED_OPACITY)
        );
        // The control painted outside any layer (it dims itself).
        assert_eq!(rec.rects.len(), 1, "the leaf control's own fill");
    }

    #[test]
    fn the_control_still_receives_events_through_the_field() {
        #[derive(Default)]
        struct AppState {
            text: String,
        }
        let mut root: RenderRoot<AppState, FieldView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let mut logic = |state: &mut AppState| {
            field(crate::input(state.text.clone(), |s: &mut AppState, t| {
                s.text = t
            }))
            .label("Email")
            .orientation(FieldOrientation::Vertical)
        };
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(WIDE, &mut tcx as &mut dyn Any);
        // A press inside the control's band focuses it, and a key edit round-trips
        // to the app through the field.
        let control_y = WIDE.height / 8.0;
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            root.event(
                &mut state,
                &InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(100.0, control_y),
                    button: PointerButton::Primary,
                }),
            );
        }
        root.event(
            &mut state,
            &InputEvent::Key(frust::authoring::KeyEvent {
                key: frust::authoring::Key::Character("z".to_string()),
                modifiers: frust::authoring::Modifiers::default(),
                repeat: false,
            }),
        );
        assert_eq!(state.text, "z");
    }

    #[test]
    fn changing_the_slot_shape_rebuilds_the_child_set() {
        let mut counter = 0u64;
        let view: FieldView<()> = field(leaf(10.0, 10.0)).label("A");
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.children.len(), 2);
        let next: FieldView<()> = field(leaf(10.0, 10.0)).label("A").error("Nope");
        let flags = View::<()>::rebuild(&next, &view, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(flags.needs_layout());
        assert_eq!(w.children.len(), 3, "the error slot was added");
        assert!(w.slots.error.is_some());
    }

    #[test]
    fn semantics_is_a_group_whose_error_is_an_alert() {
        let mut root: RenderRoot<(), FieldView<()>> = RenderRoot::new();
        let mut state = ();
        let mut logic = |_s: &mut ()| {
            field(leaf(120.0, 36.0))
                .label("Email")
                .error("Enter a valid address.")
        };
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WIDE, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        assert!(
            update.nodes.iter().any(|(_, n)| n.role() == Role::Group),
            "role=group"
        );
        assert!(
            update.nodes.iter().any(|(_, n)| n.role() == Role::Alert),
            "the error message is announced"
        );
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Label && n.value() == Some("Email")),
            "the label text reaches the tree"
        );
    }

    #[test]
    fn visit_children_publishes_every_slot() {
        let w = build(&full_field(FieldOrientation::Vertical));
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 3, "label + control + description");
    }

    // ---- Typeface: the label, description and error follow the live theme

    /// The label and description, and the invalid field's label and error.
    /// The control is a text-free leaf, since it is the caller's own view.
    #[cfg(feature = "bundled-fonts")]
    fn slots(_: &mut ()) -> frust::FlexView<()> {
        frust::column()
            .child(full_field(FieldOrientation::Vertical))
            .child(
                field(leaf(120.0, 36.0))
                    .label("Password")
                    .error("Too short."),
            )
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_label_description_and_error_paint_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "a field's label, description and error",
            slots,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_label_description_and_error_follow_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "a field's label, description and error",
            slots,
        );
    }
}
