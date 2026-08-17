//! `ListItem` rows: 1/2/3-line variants at 56/72/88dp heights, meant to pair
//! with [`super::list_view`].
//!
//! # Anatomy
//!
//! A row is `[ leading? | headline (+ supporting?) | trailing? ]` with 16dp
//! horizontal padding, an 8dp vertical inset, and a 16dp gap between the slots
//! and the text column. The `headline` and optional `supporting` text are child
//! [`frust::text`] widgets (M3 `on_surface` / `on_surface_variant` roles) laid
//! out during the layout pass — [`frust::authoring::PaintCtx`] has no text-shaping
//! context, so all text sizing happens at layout time. The
//! `leading`/`trailing` slots are arbitrary [`frust::authoring::AnyView`]s (an
//! icon, avatar, switch, …), vertically centered.
//!
//! # Height
//!
//! | Variant | Height | Constructed by |
//! |---|---|---|
//! | One-line   | 56dp | [`list_item`] |
//! | Two-line   | 72dp | [`ListItem::supporting`] |
//! | Three-line | 88dp | [`ListItem::three_line`] |
//!
//! # Interactivity
//!
//! [`ListItem::on_press`] makes the whole row one interactive target, mirroring
//! [`mod@super::card`]: the row owns capture, fires on release inside its bounds,
//! and paints the shared [`super::state_layer`] overlay tinted `on_surface`. A
//! non-interactive row routes pointer events to its slot children (so a trailing
//! control stays live).
//!
//! An interactive row is also the catalog's **hover** reference consumer: it
//! claims the hover link from its uncaptured `Move` arm
//! ([`frust::authoring::EventCtx::claim_hover`]) and paints the state layer's 8%
//! hover overlay for it. Because a pointer *leaving* the row routes its next move
//! to whatever it moved onto, the row never hears about the departure — so
//! [`frust::authoring::PaintCtx::is_hovered`] is the authoritative read and paint
//! re-syncs [`super::state_layer::StateLayer::set_hovered`] from it every frame.
//! A press wins visually while it lasts (pressed 10% > hover 8%, the
//! max-of-active-states rule), and a captured drag paints no hover at all — the
//! framework refuses a claim from a captured pointer, so a touch gesture produces
//! none either.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::{Action, Role};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Rect, Size};
use peniko::Color;

use super::state_layer::StateLayer;
use frust::authoring::ThemeTextColor;
use frust::text;

/// Horizontal padding on the leading and trailing edges (M3 list spec, 16dp).
const HPAD: f64 = 16.0;
/// Gap between a leading/trailing slot and the text column (16dp).
const GAP: f64 = 16.0;
/// Supporting-text font size (M3 body-medium, 14sp); the headline keeps the
/// default 16sp body-large size.
const SUPPORTING_SIZE: f32 = 14.0;

/// One-line row height, in logical px (M3 list spec).
pub const ONE_LINE_HEIGHT: f64 = 56.0;
/// Two-line row height, in logical px (M3 list spec).
pub const TWO_LINE_HEIGHT: f64 = 72.0;
/// Three-line row height, in logical px (M3 list spec).
pub const THREE_LINE_HEIGHT: f64 = 88.0;

/// Unthemed-fallback state-layer content color for an interactive row (a theme
/// resolves this from `colors.on_surface`).
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

/// The number of text lines a [`ListItem`] reserves height for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListItemLines {
    /// Headline only — 56dp.
    One,
    /// Headline + one supporting line — 72dp.
    Two,
    /// Headline + two supporting lines — 88dp.
    Three,
}

impl ListItemLines {
    /// The fixed row height for this line count.
    pub fn height(self) -> f64 {
        match self {
            ListItemLines::One => ONE_LINE_HEIGHT,
            ListItemLines::Two => TWO_LINE_HEIGHT,
            ListItemLines::Three => THREE_LINE_HEIGHT,
        }
    }
}

/// A view-held, typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// A declarative M3 list row. See the [module docs](self).
pub struct ListItem<State: 'static> {
    headline: String,
    supporting: Option<String>,
    lines: ListItemLines,
    leading: Option<AnyView<State>>,
    trailing: Option<AnyView<State>>,
    on_press: Option<OnPress<State>>,
}

/// Create a one-line list row with the given `headline` text.
pub fn list_item<State: 'static>(headline: impl Into<String>) -> ListItem<State> {
    ListItem {
        headline: headline.into(),
        supporting: None,
        lines: ListItemLines::One,
        leading: None,
        trailing: None,
        on_press: None,
    }
}

impl<State: 'static> ListItem<State> {
    /// Add supporting text below the headline, promoting a one-line row to the
    /// two-line (72dp) variant (a three-line row set via
    /// [`ListItem::three_line`] keeps its height).
    pub fn supporting(mut self, supporting: impl Into<String>) -> Self {
        self.supporting = Some(supporting.into());
        if self.lines == ListItemLines::One {
            self.lines = ListItemLines::Two;
        }
        self
    }

    /// Force the three-line (88dp) variant (for a supporting line that wraps to
    /// two visual lines).
    pub fn three_line(mut self) -> Self {
        self.lines = ListItemLines::Three;
        self
    }

    /// Set the leading slot (an icon/avatar/control), vertically centered.
    pub fn leading<V: View<State>>(mut self, leading: V) -> Self {
        self.leading = Some(any(leading));
        self
    }

    /// Set the trailing slot (an icon/metadata/control), vertically centered.
    pub fn trailing<V: View<State>>(mut self, trailing: V) -> Self {
        self.trailing = Some(any(trailing));
        self
    }

    /// Make the whole row interactive, firing `on_press` on release inside its
    /// bounds (see the module docs' Interactivity section).
    pub fn on_press<F: Fn(&mut State) + 'static>(mut self, on_press: F) -> Self {
        self.on_press = Some(Rc::new(on_press));
        self
    }

    /// The presence-of-slots shape; a change forces a full child rebuild.
    fn shape(&self) -> (bool, bool, bool) {
        (
            self.leading.is_some(),
            self.supporting.is_some(),
            self.trailing.is_some(),
        )
    }

    /// The headline text view (`on_surface`, default body-large size).
    fn headline_view(&self) -> AnyView<State> {
        any(text(self.headline.clone()))
    }

    /// The supporting text view (`on_surface_variant`, body-medium size), or
    /// `None` when this row has no supporting line.
    fn supporting_view(&self) -> Option<AnyView<State>> {
        self.supporting.as_ref().map(|s| {
            any(text(s.clone())
                .size(SUPPORTING_SIZE)
                .themed_role(ThemeTextColor::OnSurfaceVariant))
        })
    }
}

/// Which entry of [`ListItemWidget::children`] each logical slot occupies. The
/// headline is always present; the others are optional. Storing every child in
/// one `Vec` lets a non-interactive row route events through
/// [`frust::authoring::route_event`] and recurse uniformly for paint/semantics.
#[derive(Clone, Copy)]
struct Slots {
    leading: Option<usize>,
    headline: usize,
    supporting: Option<usize>,
    trailing: Option<usize>,
}

/// The retained widget for a [`ListItem`].
pub struct ListItemWidget {
    children: Vec<ChildPod>,
    slots: Slots,
    lines: ListItemLines,
    interactive: bool,
    pressed: bool,
    captured: bool,
    state_layer: StateLayer,
    on_press: Option<frust::authoring::ErasedCallback>,
}

/// Build the ordered child window `[leading?, headline, supporting?, trailing?]`
/// and the [`Slots`] index map, from a [`ListItem`] view.
fn build_children<State: 'static>(
    view: &ListItem<State>,
    ctx: &mut BuildCtx<'_>,
) -> (Vec<ChildPod>, Slots) {
    let mut children = Vec::new();
    let leading = view.leading.as_ref().map(|v| {
        children.push(frust::authoring::build_child(v, ctx));
        children.len() - 1
    });
    children.push(frust::authoring::build_child(&view.headline_view(), ctx));
    let headline = children.len() - 1;
    let supporting = view.supporting_view().map(|v| {
        children.push(frust::authoring::build_child(&v, ctx));
        children.len() - 1
    });
    let trailing = view.trailing.as_ref().map(|v| {
        children.push(frust::authoring::build_child(v, ctx));
        children.len() - 1
    });
    (
        children,
        Slots {
            leading,
            headline,
            supporting,
            trailing,
        },
    )
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// The resolved state-layer content color for an interactive row. Themed:
/// `colors.on_surface`. Unthemed: [`ON_SURFACE`] exactly.
fn resolve_content_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface,
        None => ON_SURFACE,
    }
}

impl<State: 'static> View<State> for ListItem<State> {
    type Element = ListItemWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ListItemWidget {
        let (children, slots) = build_children(self, ctx);
        ListItemWidget {
            children,
            slots,
            lines: self.lines,
            interactive: self.on_press.is_some(),
            pressed: false,
            captured: false,
            state_layer: StateLayer::new(),
            on_press: self.on_press.as_ref().map(frust::authoring::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ListItemWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        if self.shape() != prev.shape() {
            // Slot presence changed: tear the whole child set down (against the
            // previous view, whose shape the live children still match) and
            // rebuild fresh.
            teardown_children(prev, element.slots, &mut element.children, ctx);
            let (children, slots) = build_children(self, ctx);
            element.children = children;
            element.slots = slots;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            // Same shape: reconcile each child in place against its prev view.
            let slots = element.slots;
            if let (Some(pi), Some(ni)) = (prev.leading.as_ref(), self.leading.as_ref())
                && let Some(idx) = slots.leading
            {
                flags |= frust::authoring::rebuild_child(pi, ni, &mut element.children[idx], ctx);
            }
            flags |= frust::authoring::rebuild_child(
                &prev.headline_view(),
                &self.headline_view(),
                &mut element.children[slots.headline],
                ctx,
            );
            if let (Some(pv), Some(nv)) = (prev.supporting_view(), self.supporting_view())
                && let Some(idx) = slots.supporting
            {
                flags |= frust::authoring::rebuild_child(&pv, &nv, &mut element.children[idx], ctx);
            }
            if let (Some(pi), Some(ni)) = (prev.trailing.as_ref(), self.trailing.as_ref())
                && let Some(idx) = slots.trailing
            {
                flags |= frust::authoring::rebuild_child(pi, ni, &mut element.children[idx], ctx);
            }
        }

        if element.lines != self.lines {
            element.lines = self.lines;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        let now_interactive = self.on_press.is_some();
        if element.interactive != now_interactive {
            element.interactive = now_interactive;
            if !now_interactive && element.captured {
                element.pressed = false;
                element.captured = false;
                element.state_layer.set_pressed(false);
            }
            flags |= ChangeFlags::PAINT;
        }
        element.on_press = self.on_press.as_ref().map(frust::authoring::erase_callback);
        flags
    }

    fn teardown(&self, element: &mut ListItemWidget, ctx: &mut BuildCtx<'_>) {
        teardown_children(self, element.slots, &mut element.children, ctx);
    }
}

/// Tear down every child pod through the view it was built from, per the
/// [`Slots`] index map. `teardown_child` cancels an in-flight capture regardless
/// of view type; the reconstructed text views only need to match the pod's
/// concrete `TextWidget` type for the (usually no-op) `teardown` dispatch.
fn teardown_children<State: 'static>(
    view: &ListItem<State>,
    slots: Slots,
    children: &mut [ChildPod],
    ctx: &mut BuildCtx<'_>,
) {
    for (index, pod) in children.iter_mut().enumerate() {
        if Some(index) == slots.leading
            && let Some(v) = view.leading.as_ref()
        {
            frust::authoring::teardown_child(v, pod, ctx);
        } else if index == slots.headline {
            frust::authoring::teardown_child(&view.headline_view(), pod, ctx);
        } else if Some(index) == slots.supporting
            && let Some(v) = view.supporting_view()
        {
            frust::authoring::teardown_child(&v, pod, ctx);
        } else if Some(index) == slots.trailing
            && let Some(v) = view.trailing.as_ref()
        {
            frust::authoring::teardown_child(v, pod, ctx);
        }
    }
}

impl Widget for ListItemWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let height = self.lines.height();
        let slot_bc = BoxConstraints::new(Size::ZERO, Size::new(width, height));

        // Leading slot, vertically centered at the left inset.
        let mut text_left = HPAD;
        if let Some(idx) = self.slots.leading {
            let s = self.children[idx].layout_child(ctx, &slot_bc);
            self.children[idx].set_origin(Point::new(HPAD, ((height - s.height) / 2.0).max(0.0)));
            text_left = HPAD + s.width + GAP;
        }

        // Trailing slot, vertically centered at the right inset.
        let mut text_right = width - HPAD;
        if let Some(idx) = self.slots.trailing {
            let s = self.children[idx].layout_child(ctx, &slot_bc);
            let x = width - HPAD - s.width;
            self.children[idx].set_origin(Point::new(x, ((height - s.height) / 2.0).max(0.0)));
            text_right = x - GAP;
        }

        // Text column between the slots.
        let text_w = (text_right - text_left).max(0.0);
        let text_bc = BoxConstraints::new(Size::ZERO, Size::new(text_w, height));
        let hi = self.slots.headline;
        let head_size = self.children[hi].layout_child(ctx, &text_bc);
        let supp_size = if let Some(si) = self.slots.supporting {
            self.children[si].layout_child(ctx, &text_bc)
        } else {
            Size::ZERO
        };

        // Vertically center the headline+supporting block within the row (the
        // fixed row heights already bake in the 8dp vertical padding, so a block
        // that fits leaves ≥8dp above and below).
        let block = head_size.height + supp_size.height;
        let text_top = ((height - block) / 2.0).max(0.0);
        self.children[hi].set_origin(Point::new(text_left, text_top));
        if let Some(si) = self.slots.supporting {
            self.children[si].set_origin(Point::new(text_left, text_top + head_size.height));
        }

        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.interactive {
            // The pod's hover link is authoritative, not the flag the `Move` arm
            // set: a pointer that left the row routed its next move elsewhere, so
            // no event ever told this row it stopped being hovered. Re-syncing here
            // is what makes the overlay drop on the very frame the pointer moves
            // onto a sibling.
            self.state_layer.set_hovered(ctx.is_hovered());
            let content_color = resolve_content_color(Theme::from_paint_ctx(ctx));
            self.state_layer.paint(
                ctx,
                scene,
                Rect::from_origin_size(ctx.origin(), ctx.size()),
                0.0,
                content_color,
            );
        }
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.interactive {
            return frust::authoring::route_event(&mut self.children, ctx, event);
        }
        // Non-pointer events (Key, Ime, focus-routed) must be forwarded to
        // children, even when interactive. Only pointer events drive the
        // interactive row's own capture/press behavior.
        let InputEvent::Pointer(p) = event else {
            return frust::authoring::route_event(&mut self.children, ctx, event);
        };
        let on_press = self
            .on_press
            .as_mut()
            .expect("on_press is set whenever interactive is true");
        match p.phase {
            PointerPhase::Down => {
                self.pressed = true;
                self.captured = true;
                self.state_layer.set_pressed(true);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    // No capture: this is the hover pass. Claim the link whenever
                    // the pointer is inside the row — every qualifying move, not
                    // just the first, since a claim covers only its own pass. The
                    // redraw is gated on the setter's changed-return so a pointer
                    // wandering *within* the row costs nothing after the first move.
                    // A claim made while some pointer is captured (this row's or
                    // anyone's) is refused by the framework, so no drag or touch
                    // gesture can reach this arm and tint the row.
                    let over = inside(p.position, ctx.size());
                    if over {
                        ctx.claim_hover();
                    }
                    if self.state_layer.set_hovered(over) {
                        ctx.request_redraw();
                    }
                    // Still `Ignored`: watching a move is not consuming it.
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
                    (on_press)(ctx);
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
        ctx.push_container(
            Role::ListItem,
            |node| {
                if self.interactive {
                    node.add_action(Action::Click);
                }
            },
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(children);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use frust_widgets::test_support::leaf;
    use kurbo::Point;
    use std::any::Any;

    fn build<S: 'static>(view: &ListItem<S>) -> ListItemWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ListItemWidget, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(width, 400.0)))
    }

    #[test]
    fn one_line_row_is_56dp() {
        let view: ListItem<()> = list_item("Title");
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        assert_eq!(size.height, ONE_LINE_HEIGHT);
    }

    #[test]
    fn supporting_promotes_to_two_line_72dp() {
        let view: ListItem<()> = list_item("Title").supporting("Subtitle");
        assert_eq!(view.lines, ListItemLines::Two);
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        assert_eq!(size.height, TWO_LINE_HEIGHT);
    }

    #[test]
    fn three_line_is_88dp() {
        let view: ListItem<()> = list_item("Title").supporting("Sub").three_line();
        assert_eq!(view.lines, ListItemLines::Three);
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        assert_eq!(size.height, THREE_LINE_HEIGHT);
    }

    #[test]
    fn leading_and_trailing_slots_are_placed_and_inset_the_text() {
        // A 24x24 leading and a 16x16 trailing leaf, so the text column starts
        // after the leading + gap and ends before the trailing + gap.
        let view: ListItem<()> = list_item("Title")
            .leading(leaf(24.0, 24.0))
            .trailing(leaf(16.0, 16.0));
        let mut w = build(&view);
        layout(&mut w, 300.0);

        let leading_i = w.slots.leading.expect("leading present");
        let trailing_i = w.slots.trailing.expect("trailing present");
        let headline_i = w.slots.headline;

        // Leading sits at the 16dp inset, vertically centered in the 56dp row.
        assert_eq!(w.children[leading_i].origin().x, HPAD);
        assert_eq!(
            w.children[leading_i].origin().y,
            (ONE_LINE_HEIGHT - 24.0) / 2.0
        );
        // Trailing sits at the right inset.
        assert_eq!(w.children[trailing_i].origin().x, 300.0 - HPAD - 16.0);
        // The headline is pushed right of the leading + gap.
        assert_eq!(w.children[headline_i].origin().x, HPAD + 24.0 + GAP);
    }

    #[test]
    fn build_orders_children_leading_headline_supporting_trailing() {
        let view: ListItem<()> = list_item("H")
            .supporting("S")
            .leading(leaf(10.0, 10.0))
            .trailing(leaf(10.0, 10.0));
        let w = build(&view);
        assert_eq!(w.children.len(), 4);
        assert_eq!(w.slots.leading, Some(0));
        assert_eq!(w.slots.headline, 1);
        assert_eq!(w.slots.supporting, Some(2));
        assert_eq!(w.slots.trailing, Some(3));
    }

    // --- Interactivity ---

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    fn dispatch<S: 'static>(
        w: &mut ListItemWidget,
        state: &mut S,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(300.0, ONE_LINE_HEIGHT));
        w.event(&mut ctx, event)
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Primary,
        })
    }

    #[test]
    fn interactive_row_fires_on_up_inside() {
        let view: ListItem<Counter> =
            list_item("Tap me").on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.presses, 1);
    }

    #[test]
    fn interactive_row_up_outside_does_not_fire() {
        let view: ListItem<Counter> =
            list_item("Tap me").on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 999.0, 999.0));
        assert_eq!(state.presses, 0);
    }

    // --- Interactive with focus-routed child events ---

    /// A minimal widget that handles Key events for testing focus routing.
    struct FocusConsumer;

    impl FocusConsumer {
        fn new() -> Self {
            FocusConsumer
        }
    }

    struct FocusConsumerWidget;

    impl View<Counter> for FocusConsumer {
        type Element = FocusConsumerWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> FocusConsumerWidget {
            FocusConsumerWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut FocusConsumerWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for FocusConsumerWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            // Handle any key event (focus-routed events reach here only if the
            // pod is focused, so a Key event here proves the routing worked).
            if let InputEvent::Key(_) = event {
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn interactive_row_with_trailing_control_forwards_key_events() {
        let view: ListItem<Counter> = list_item("Item")
            .trailing(FocusConsumer::new())
            .on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();

        // Manually set the trailing control as focused to simulate a prior focus state.
        // (In real usage, the control would be focused by a pointer-down event,
        // but here we're directly testing the event-routing path.)
        let trailing_idx = w.slots.trailing.expect("trailing is present");
        w.children[trailing_idx].set_focused(true);

        // Send a Key event — it should be forwarded to the focused child.
        let key_event = InputEvent::Key(frust::authoring::KeyEvent {
            key: frust::authoring::Key::Named(frust::authoring::NamedKey::Backspace),
            modifiers: frust::authoring::Modifiers::default(),
            repeat: false,
        });
        let result = dispatch(&mut w, &mut state, &key_event);

        // Verify the key event was handled (forwarded to and handled by child).
        assert_eq!(
            result,
            EventResult::Handled,
            "key event must be forwarded to focused child"
        );

        // The row press should not have fired (the row only fires on pointer Up).
        assert_eq!(
            state.presses, 0,
            "row press callback does not fire on key event"
        );
    }

    // --- Hover ---

    /// A recording scene that captures each rounded rect's `(origin, size,
    /// radius, color)` — the state-layer overlay's shape (see `state_layer.rs`'s
    /// own `RRectRecorder`).
    #[derive(Default)]
    struct RRectRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for RRectRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
    }

    /// Two interactive rows in a `Column` under a real `RenderRoot` — the only
    /// harness that can exercise hover at all, since the hover link is recorded by
    /// the root's event pass and read back through `PaintCtx::is_hovered`. It is
    /// also the out-of-tree reachability proof: everything here is reached through
    /// the `frust` facade, exactly as a third-party catalog would.
    struct HoverHarness {
        root: frust_core::RenderRoot<Counter, frust::FlexView<Counter>>,
        state: Counter,
        tcx: TextContext,
    }

    /// Window/row geometry: two stacked 56dp rows in a 300x200 window.
    const HOVER_WINDOW: Size = Size::new(300.0, 200.0);

    impl HoverHarness {
        fn new() -> Self {
            let mut h = HoverHarness {
                root: frust_core::RenderRoot::new(),
                state: Counter::default(),
                tcx: TextContext::new(),
            };
            h.rebuild_layout();
            h
        }

        fn rebuild_layout(&mut self) {
            let mut app = |_s: &mut Counter| {
                frust::Column(vec![
                    frust::authoring::any(
                        list_item::<Counter>("first").on_press(|s: &mut Counter| s.presses += 1),
                    ),
                    frust::authoring::any(
                        list_item::<Counter>("second").on_press(|s: &mut Counter| s.presses += 1),
                    ),
                ])
            };
            self.root.rebuild(&mut app, &mut self.state);
            self.root
                .layout_with_text(HOVER_WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn dispatch(
            &mut self,
            phase: PointerPhase,
            x: f64,
            y: f64,
        ) -> frust::authoring::EventOutcome {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(frust::authoring::PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: frust::authoring::PointerButton::Primary,
                }),
            )
        }

        /// The y-centre of row `index` (rows are `ONE_LINE_HEIGHT` tall).
        fn row_y(index: usize) -> f64 {
            ONE_LINE_HEIGHT * index as f64 + ONE_LINE_HEIGHT / 2.0
        }

        /// Paint and report which rows painted a state-layer overlay, by the
        /// overlay rect's y origin.
        fn overlay_rows(&mut self) -> Vec<usize> {
            let mut rec = RRectRecorder::default();
            self.root.paint(&mut rec, frust::FrameTime::ZERO);
            rec.rrects
                .iter()
                .map(|(origin, _, _, _)| (origin.y / ONE_LINE_HEIGHT).round() as usize)
                .collect()
        }

        /// The alpha of the single overlay painted this frame.
        fn overlay_alpha(&mut self) -> f32 {
            let mut rec = RRectRecorder::default();
            self.root.paint(&mut rec, frust::FrameTime::ZERO);
            let (_, _, _, color) = rec.rrects.first().copied().expect("one overlay painted");
            color.components[3]
        }
    }

    #[test]
    fn hovering_a_row_paints_the_hover_overlay() {
        let mut h = HoverHarness::new();
        assert!(h.overlay_rows().is_empty(), "no overlay at rest");

        h.dispatch(PointerPhase::Move, 150.0, HoverHarness::row_y(0));
        assert_eq!(h.overlay_rows(), vec![0], "the hovered row tints");
        assert_eq!(
            h.overlay_alpha(),
            crate::state_layer::HOVER_OPACITY,
            "at the documented M3 hover opacity"
        );
    }

    #[test]
    fn hover_moves_to_the_row_under_the_pointer() {
        let mut h = HoverHarness::new();
        h.dispatch(PointerPhase::Move, 150.0, HoverHarness::row_y(0));
        assert_eq!(h.overlay_rows(), vec![0]);

        // The row the pointer left never receives an event about it — the move
        // routes to its sibling — so this is the case a widget cannot handle on
        // its own, and exactly one row may end up tinted.
        h.dispatch(PointerPhase::Move, 150.0, HoverHarness::row_y(1));
        assert_eq!(h.overlay_rows(), vec![1]);

        // Off both rows: nothing tints, and the row that lost hover gets the
        // repaint it could not ask for itself.
        let outcome = h.dispatch(PointerPhase::Move, 150.0, 180.0);
        assert!(outcome.needs_redraw);
        assert!(h.overlay_rows().is_empty());
    }

    #[test]
    fn a_captured_drag_paints_no_hover_overlay() {
        let mut h = HoverHarness::new();
        // Press row 0: the row captures, and the press overlay (10%) is what
        // shows — never the hover one.
        h.dispatch(PointerPhase::Down, 150.0, HoverHarness::row_y(0));
        assert_eq!(h.overlay_rows(), vec![0]);
        assert_eq!(h.overlay_alpha(), frust::authoring::PRESSED_OPACITY);

        // Drag off the row: the captured row keeps receiving moves and its own
        // `Move` arm calls `claim_hover()`, which must record nothing.
        h.dispatch(PointerPhase::Move, 150.0, HoverHarness::row_y(1));
        assert!(
            h.overlay_rows().is_empty(),
            "dragged outside: neither pressed nor hovered"
        );

        // Release outside: no press fires, and no hover was left behind.
        h.dispatch(PointerPhase::Up, 150.0, HoverHarness::row_y(1));
        assert_eq!(h.state.presses, 0);
        assert!(h.overlay_rows().is_empty());
    }

    #[test]
    fn hover_survives_a_rebuild_and_clears_on_press() {
        let mut h = HoverHarness::new();
        h.dispatch(PointerPhase::Move, 150.0, HoverHarness::row_y(0));
        h.rebuild_layout();
        assert_eq!(
            h.overlay_rows(),
            vec![0],
            "an in-place rebuild keeps the pod that holds the link"
        );

        // A `Down` ends the hover link outright; the press overlay takes over.
        h.dispatch(PointerPhase::Down, 150.0, HoverHarness::row_y(0));
        assert_eq!(h.overlay_alpha(), frust::authoring::PRESSED_OPACITY);
        h.dispatch(PointerPhase::Up, 150.0, HoverHarness::row_y(0));
        assert_eq!(h.state.presses, 1);
        assert!(
            h.overlay_rows().is_empty(),
            "a click leaves no hover behind until the pointer moves again"
        );
    }

    // --- Semantics ---

    #[test]
    fn semantics_row_is_a_list_item_with_the_headline_label() {
        fn logic(_s: &mut ()) -> ListItem<()> {
            list_item("Inbox")
        }
        let mut root: RenderRoot<(), ListItem<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 100.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let (_, item) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ListItem)
            .expect("row contributes a Role::ListItem node");
        assert!(
            !item.children().is_empty(),
            "the headline text is a semantics child of the row"
        );
        // The headline run's text is carried on a Label node.
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Label && n.value() == Some("Inbox")),
            "the headline text is announced"
        );
    }

    #[test]
    fn interactive_row_semantics_carries_a_click_action() {
        fn logic(_s: &mut ()) -> ListItem<()> {
            list_item("Go").on_press(|_s: &mut ()| {})
        }
        let mut root: RenderRoot<(), ListItem<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 100.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, item) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ListItem)
            .expect("row node");
        assert!(
            item.supports_action(Action::Click),
            "an interactive row exposes the Click action"
        );
    }
}
