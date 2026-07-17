//! `ListItem` rows (Phase 6c, PLAN.md D4/D5, task 12): 1/2/3-line variants at
//! 56/72/88dp heights, meant to pair with [`super::list_view`].
//!
//! # Anatomy
//!
//! A row is `[ leading? | headline (+ supporting?) | trailing? ]` with 16dp
//! horizontal padding, an 8dp vertical inset, and a 16dp gap between the slots
//! and the text column. The `headline` and optional `supporting` text are child
//! [`crate::text`] widgets (M3 `on_surface` / `on_surface_variant` roles) laid
//! out during the layout pass — [`forgekit_core::PaintCtx`] has no text-shaping
//! context, so all text sizing happens at layout time (C1 in PLAN.md). The
//! `leading`/`trailing` slots are arbitrary [`forgekit_core::AnyView`]s (an
//! icon, avatar, switch, …), vertically centered.
//!
//! # Height (ledger-verified)
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
//! [`super::card`]: the row owns capture, fires on release inside its bounds,
//! and paints the shared [`super::state_layer`] overlay tinted `on_surface`. A
//! non-interactive row routes pointer events to its slot children (so a trailing
//! control stays live).

use std::rc::Rc;

use forgekit_core::accesskit::{Action, Role};
use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use forgekit_theme::Theme;
use kurbo::{Point, Rect, Size};
use peniko::Color;

use super::state_layer::StateLayer;
use crate::text::{ThemeTextColor, text};

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
/// [`crate::route_event`] and recurse uniformly for paint/semantics.
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
    on_press: Option<crate::ErasedCallback>,
}

/// Build the ordered child window `[leading?, headline, supporting?, trailing?]`
/// and the [`Slots`] index map, from a [`ListItem`] view.
fn build_children<State: 'static>(
    view: &ListItem<State>,
    ctx: &mut BuildCtx<'_>,
) -> (Vec<ChildPod>, Slots) {
    let mut children = Vec::new();
    let leading = view.leading.as_ref().map(|v| {
        children.push(crate::build_child(v, ctx));
        children.len() - 1
    });
    children.push(crate::build_child(&view.headline_view(), ctx));
    let headline = children.len() - 1;
    let supporting = view.supporting_view().map(|v| {
        children.push(crate::build_child(&v, ctx));
        children.len() - 1
    });
    let trailing = view.trailing.as_ref().map(|v| {
        children.push(crate::build_child(v, ctx));
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
            on_press: self.on_press.as_ref().map(crate::erase_callback),
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
                flags |= crate::rebuild_child(pi, ni, &mut element.children[idx], ctx);
            }
            flags |= crate::rebuild_child(
                &prev.headline_view(),
                &self.headline_view(),
                &mut element.children[slots.headline],
                ctx,
            );
            if let (Some(pv), Some(nv)) = (prev.supporting_view(), self.supporting_view())
                && let Some(idx) = slots.supporting
            {
                flags |= crate::rebuild_child(&pv, &nv, &mut element.children[idx], ctx);
            }
            if let (Some(pi), Some(ni)) = (prev.trailing.as_ref(), self.trailing.as_ref())
                && let Some(idx) = slots.trailing
            {
                flags |= crate::rebuild_child(pi, ni, &mut element.children[idx], ctx);
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
        element.on_press = self.on_press.as_ref().map(crate::erase_callback);
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
            crate::teardown_child(v, pod, ctx);
        } else if index == slots.headline {
            crate::teardown_child(&view.headline_view(), pod, ctx);
        } else if Some(index) == slots.supporting
            && let Some(v) = view.supporting_view()
        {
            crate::teardown_child(&v, pod, ctx);
        } else if Some(index) == slots.trailing
            && let Some(v) = view.trailing.as_ref()
        {
            crate::teardown_child(v, pod, ctx);
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
            return crate::route_event(&mut self.children, ctx, event);
        }
        // Non-pointer events (Key, Ime, focus-routed) must be forwarded to
        // children, even when interactive. Only pointer events drive the
        // interactive row's own capture/press behavior.
        let InputEvent::Pointer(p) = event else {
            return crate::route_event(&mut self.children, ctx, event);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf;
    use forgekit_core::RenderRoot;
    use forgekit_text::TextContext;
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
        InputEvent::Pointer(forgekit_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: forgekit_core::PointerButton::Primary,
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
        let key_event = InputEvent::Key(forgekit_core::KeyEvent {
            key: forgekit_core::Key::Named(forgekit_core::NamedKey::Backspace),
            modifiers: forgekit_core::Modifiers::default(),
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
