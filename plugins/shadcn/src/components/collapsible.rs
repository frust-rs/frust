//! `collapsible`: a trigger plus the content it reveals — the accordion's
//! mechanics with none of its chrome.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/collapsible.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17). Upstream is
//! three `data-slot`-tagged Radix primitives with **no classes at all**: the look
//! is entirely the caller's (a shadcn `button` trigger, say), and the component
//! contributes the disclosure behavior. This port keeps exactly that split —
//! arbitrary `trigger` and `content` views, no borders, no padding, no fills.
//!
//! # What it does contribute
//!
//! - **A controlled `open`.** Activating the trigger reports the *requested*
//!   state through `on_open_change`; the widget never flips its own flag.
//! - **The reveal.** Same shape as [`crate::components::accordion`]'s: the content
//!   is laid out at full height and revealed by clipping an animated band
//!   (200ms, eased out), collapsing to a jump under
//!   `Theme.motion.reduce_motion`. Because the reveal changes the widget's height,
//!   a running animation asks for `request_layout`, not a bare `request_frame`.
//! - **Keyboard and pointer activation.** A press on the trigger row captures and
//!   fires on release inside it; Space/Enter toggle while the widget holds focus.
//! - **A focus ring** around the trigger row — the one deliberate addition to an
//!   unstyled upstream component. Focus here is per-*pod* and this widget is the
//!   pod that takes it, so a focusable child (a shadcn button used as the trigger)
//!   never sees focus itself and would paint no ring; a keyboard user would
//!   otherwise have no indication of where they are.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Role,
    SemanticsCtx, Size, View, Widget, any, build_child, erase_callback_arg, rebuild_child,
    route_event, teardown_child, visit_children,
};
use frust::{AnimationController, Curve, Theme};

use crate::components::native_select::activates;
use crate::hit::inside;
use crate::style;
use crate::tokens::ShadcnTokens;

/// Reveal duration, matching [`crate::components::accordion`]'s (the source's own
/// `duration-200`).
const REVEAL_MS: u64 = 200;
/// Progress difference below which a reveal counts as settled.
const PROGRESS_EPSILON: f64 = 1e-4;

/// A view-held, typed open-state callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative collapsible. See the [module docs](self).
pub struct CollapsibleView<State: 'static> {
    trigger: AnyView<State>,
    content: AnyView<State>,
    open: bool,
    disabled: bool,
    on_open_change: OnOpenChange<State>,
}

/// Build a controlled collapsible: `trigger` reveals `content` while `open`, and
/// every activation reports the requested state through
/// `on_open_change(state, next_open)`.
pub fn collapsible<State: 'static, T, C, F>(
    trigger: T,
    content: C,
    open: bool,
    on_open_change: F,
) -> CollapsibleView<State>
where
    T: View<State>,
    C: View<State>,
    F: Fn(&mut State, bool) + 'static,
{
    CollapsibleView {
        trigger: any(trigger),
        content: any(content),
        open,
        disabled: false,
        on_open_change: Rc::new(on_open_change),
    }
}

impl<State: 'static> CollapsibleView<State> {
    /// Disable the trigger: inert, [`style::DISABLED_CURSOR`], and no ring. The
    /// trigger view dims itself if it has a disabled state of its own — this
    /// widget paints nothing, so it has nothing to dim.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The retained widget for a [`CollapsibleView`].
///
/// The reveal drives the same way [`crate::components::accordion`]'s does: the
/// [`AnimationController`] runs a normalized `0 → 1` ramp and `progress`
/// interpolates `from → target` across it, because a controller always starts at
/// `0.0` and a collapsible built *open* must start revealed rather than animate
/// up from nothing.
pub struct CollapsibleWidget {
    /// `[trigger, content]` — a flat pod list so the pair routes through
    /// [`route_event`] rather than a hand-rolled dispatch.
    pods: Vec<ChildPod>,
    open: bool,
    disabled: bool,
    /// The progress the running reveal started from.
    from: f64,
    /// The reveal's target (`1.0` open, `0.0` closed).
    target: f64,
    /// The reveal progress `layout` last used, updated in `paint`.
    progress: f64,
    anim: AnimationController,
    /// Resolved trigger-row height.
    trigger_height: f64,
    /// Resolved full content height.
    content_height: f64,
    hovered: bool,
    pressed: bool,
    captured: bool,
    on_open_change: ErasedArgCallback<bool>,
}

impl<State: 'static> View<State> for CollapsibleView<State> {
    type Element = CollapsibleWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CollapsibleWidget {
        // A collapsible built open starts revealed (idle controller, settled
        // progress) rather than animating in.
        let settled = if self.open { 1.0 } else { 0.0 };
        CollapsibleWidget {
            pods: vec![
                build_child(&self.trigger, ctx),
                build_child(&self.content, ctx),
            ],
            open: self.open,
            disabled: self.disabled,
            from: settled,
            target: settled,
            progress: settled,
            anim: reveal_controller(),
            trigger_height: 0.0,
            content_height: 0.0,
            hovered: false,
            pressed: false,
            captured: false,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CollapsibleWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.trigger, &self.trigger, &mut element.pods[0], ctx);
        flags |= rebuild_child(&prev.content, &self.content, &mut element.pods[1], ctx);
        if element.open != self.open {
            // The app decided: reveal toward whatever it confirmed, from wherever
            // the last reveal had reached.
            element.open = self.open;
            element.start_reveal(if self.open { 1.0 } else { 0.0 });
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.disabled != self.disabled {
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
        }
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        flags
    }

    fn teardown(&self, element: &mut CollapsibleWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.trigger, &mut element.pods[0], ctx);
        teardown_child(&self.content, &mut element.pods[1], ctx);
    }
}

/// A fresh `0 → 1` reveal ramp at the source's own `duration-200`, eased out.
fn reveal_controller() -> AnimationController {
    AnimationController::new(Duration::from_millis(REVEAL_MS)).with_curve(Curve::EaseOut)
}

impl CollapsibleWidget {
    /// Whether widget-local `pos` is over the trigger row.
    fn over_trigger(&self, pos: Point, size: Size) -> bool {
        inside(pos, size) && pos.y < self.trigger_height
    }

    /// Start a reveal from the current progress toward `target`.
    fn start_reveal(&mut self, target: f64) {
        self.from = self.progress;
        self.target = target;
        self.anim = reveal_controller();
        self.anim.forward();
    }

    /// Report the open state an activation asks for. The widget never flips its
    /// own flag — the next rebuild does, from the app.
    fn toggle(&mut self, ctx: &mut EventCtx) {
        if self.disabled {
            return;
        }
        let next = !self.open;
        (self.on_open_change)(ctx, next);
    }
}

impl Widget for CollapsibleWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let loose = BoxConstraints::new(Size::ZERO, Size::new(width, f64::INFINITY));
        let trigger = self.pods[0].layout_child(ctx, &loose);
        self.pods[0].set_origin(Point::ORIGIN);
        let content = self.pods[1].layout_child(ctx, &loose);
        self.pods[1].set_origin(Point::new(0.0, trigger.height));
        self.trigger_height = trigger.height;
        self.content_height = content.height;
        bc.constrain(Size::new(
            width,
            trigger.height + content.height * self.progress,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.hovered = ctx.is_hovered();
        let (origin, size) = (ctx.origin(), ctx.size());
        let now = ctx.frame_time();
        // One scope for every theme read: the `&Theme` borrows the context, and
        // `request_layout` plus the child paints below need it mutably.
        let (reduce_motion, radius, ring) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                ShadcnTokens::resolve_radius(None, theme).md,
                style::ring_color(None, theme),
            )
        };

        let next = if reduce_motion {
            // `reduce_motion` collapses the reveal to a jump, and stops asking for
            // frames.
            if self.anim.is_animating() {
                self.anim.stop();
            }
            self.target
        } else if self.anim.is_animating() {
            if self.anim.advance(now) {
                // Layout-affecting motion: `request_layout` implies a frame, and a
                // bare `request_frame` would leave the panel unresized on mobile.
                ctx.request_layout();
            }
            let ramp = self.anim.value_clamped();
            self.from + (self.target - self.from) * ramp
        } else {
            self.progress
        };
        if (next - self.progress).abs() > PROGRESS_EPSILON {
            self.progress = next;
            ctx.request_layout();
        }

        if ctx.has_focus() && !self.disabled {
            style::draw_focus_ring(
                scene,
                origin,
                Size::new(size.width, self.trigger_height),
                radius,
                ring,
            );
        }
        self.pods[0].paint_child(ctx, scene);

        // `overflow-hidden` plus the height animation: the content is clipped to
        // the revealed band.
        let revealed = self.content_height * self.progress;
        if revealed > 0.0 {
            scene.push_clip(
                Point::new(origin.x, origin.y + self.trigger_height),
                Size::new(size.width, revealed),
            );
            self.pods[1].paint_child(ctx, scene);
            scene.pop_clip();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Children first: a control inside the trigger or the revealed content owns
        // its own events, and this widget's hover claim comes after the routing.
        let routed = route_event(&mut self.pods, ctx, event);
        if event.is_broadcast() || routed == EventResult::Handled {
            return routed;
        }
        if let InputEvent::Key(key) = event {
            if self.disabled || !activates(&key.key) {
                return EventResult::Ignored;
            }
            self.toggle(ctx);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        match p.phase {
            PointerPhase::Move => {
                if !self.captured {
                    let over = self.over_trigger(p.position, size);
                    if over {
                        ctx.claim_hover();
                        ctx.set_cursor(if self.disabled {
                            style::DISABLED_CURSOR
                        } else {
                            style::ACTIVE_CURSOR
                        });
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                }
                ctx.set_cursor(style::ACTIVE_CURSOR);
                let over = self.over_trigger(p.position, size);
                if self.pressed != over {
                    self.pressed = over;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Down => {
                if self.disabled || !self.over_trigger(p.position, size) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                // Focus is what paints the ring and routes Space/Enter here.
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                let armed = self.pressed;
                self.pressed = false;
                if armed && self.over_trigger(p.position, size) {
                    self.toggle(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // Flags and a redraw only: a Cancel arm never touches app state.
                self.captured = false;
                self.pressed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Button,
            |node| {
                node.set_expanded(self.open);
                if self.disabled {
                    node.set_disabled();
                } else {
                    node.add_action(Action::Click);
                }
            },
            |ctx| self.pods[0].semantics_child(ctx),
        );
        // A collapsed panel is unreachable, and offering a screen reader a control
        // the user cannot get to is worse than omitting it — the same input-parity
        // rule the navigator follows.
        if self.open {
            self.pods[1].semantics_child(ctx);
        }
    }

    visit_children!(pods);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        Brush, Color, Key, KeyEvent, Modifiers, PointerButton, PointerEvent, Rect, Shape,
    };
    use frust::{Brightness, CursorIcon, FrameTime};
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        clips: Vec<(Point, Size)>,
        strokes: Vec<(Rect, f64, Color)>,
        rects: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {
            self.rects += 1;
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {}
        fn stroke_path(
            &mut self,
            origin: Point,
            path: &frust::authoring::BezPath,
            width: f64,
            brush: &Brush,
        ) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes
                .push((path.bounding_box() + origin.to_vec2(), width, color));
        }
    }

    impl Recorder {
        fn ring(&self) -> Option<Rect> {
            self.strokes
                .iter()
                .find(|(_, w, _)| *w == style::FOCUS_RING_WIDTH)
                .map(|(bbox, _, _)| *bbox)
        }

        fn revealed(&self) -> f64 {
            self.clips.first().map_or(0.0, |(_, size)| size.height)
        }
    }

    const WINDOW: Size = Size::new(300.0, 300.0);
    const TRIGGER_H: f64 = 32.0;
    const CONTENT_H: f64 = 60.0;

    /// A fixed-size leaf generic over the app state (the shared
    /// `test_support::leaf` is `View<()>` only).
    struct Block(Size);

    /// The retained half of [`Block`].
    struct BlockWidget(Size);

    impl<S: 'static> View<S> for Block {
        type Element = BlockWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> BlockWidget {
            BlockWidget(self.0)
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut BlockWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.0 = self.0;
            ChangeFlags::NONE
        }
    }

    impl Widget for BlockWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.0)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    #[derive(Default)]
    struct AppState {
        open: bool,
        requests: Vec<bool>,
    }

    fn view(open: bool, disabled: bool) -> CollapsibleView<AppState> {
        collapsible(
            Block(Size::new(120.0, TRIGGER_H)),
            Block(Size::new(120.0, CONTENT_H)),
            open,
            |s: &mut AppState, next| {
                s.requests.push(next);
                s.open = next;
            },
        )
        .disabled(disabled)
    }

    fn build(v: &CollapsibleView<AppState>) -> CollapsibleWidget {
        let mut counter = 0u64;
        View::<AppState>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut CollapsibleWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(WINDOW))
    }

    struct Harness {
        root: RenderRoot<AppState, CollapsibleView<AppState>>,
        state: AppState,
        tcx: TextContext,
        disabled: bool,
    }

    impl Harness {
        fn new(disabled: bool, reduce_motion: bool) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                disabled,
            };
            let mut theme = crate::theme().with_brightness(Brightness::Light);
            theme.motion.reduce_motion = reduce_motion;
            h.root.set_theme(Box::new(theme));
            h.pass();
            h
        }

        fn pass(&mut self) -> Size {
            let disabled = self.disabled;
            let mut logic = move |state: &mut AppState| view(state.open, disabled);
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any)
        }

        fn frame(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            rec
        }

        fn pointer(&mut self, phase: PointerPhase, y: f64) -> bool {
            self.root
                .event(
                    &mut self.state,
                    &InputEvent::Pointer(PointerEvent {
                        phase,
                        position: Point::new(40.0, y),
                        button: PointerButton::Primary,
                    }),
                )
                .needs_redraw
        }

        fn key(&mut self, key: Key) {
            self.root.event(
                &mut self.state,
                &InputEvent::Key(KeyEvent {
                    key,
                    modifiers: Modifiers::default(),
                    repeat: false,
                }),
            );
        }
    }

    #[test]
    fn a_closed_collapsible_is_only_its_trigger_and_paints_no_chrome() {
        let mut w = build(&view(false, false));
        let size = layout(&mut w);
        assert_eq!(size, Size::new(WINDOW.width, TRIGGER_H));
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        assert!(rec.strokes.is_empty(), "no border, no ring at rest");
        assert_eq!(rec.rects, 1, "only the trigger child painted");
        assert_eq!(rec.revealed(), 0.0, "nothing revealed");
    }

    #[test]
    fn an_open_collapsible_reserves_and_reveals_its_content() {
        let mut w = build(&view(true, false));
        let size = layout(&mut w);
        assert_eq!(size.height, TRIGGER_H + CONTENT_H);
        assert_eq!(w.pods[1].origin().y, TRIGGER_H, "the panel is flush");
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.revealed(), CONTENT_H);
        assert_eq!(rec.clips[0].0, Point::new(0.0, TRIGGER_H));
    }

    #[test]
    fn the_trigger_reports_the_requested_state_and_never_flips_its_own() {
        let mut h = Harness::new(false, false);
        h.pointer(PointerPhase::Down, 10.0);
        assert!(h.state.requests.is_empty(), "never on down");
        h.pointer(PointerPhase::Up, 10.0);
        assert_eq!(h.state.requests, vec![true]);
        h.pass();
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, 10.0);
        assert_eq!(h.state.requests, vec![true, false]);

        // A release outside the trigger row, and a cancelled press, fire nothing.
        h.pass();
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, WINDOW.height - 1.0);
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Cancel, 10.0);
        assert_eq!(h.state.requests.len(), 2);
    }

    #[test]
    fn a_press_in_the_content_band_is_not_a_trigger_press() {
        let mut h = Harness::new(false, false);
        h.state.open = true;
        h.pass();
        h.frame(0.0);
        let content_y = TRIGGER_H + 10.0;
        h.pointer(PointerPhase::Down, content_y);
        h.pointer(PointerPhase::Up, content_y);
        assert!(
            h.state.requests.is_empty(),
            "only the trigger row toggles the panel"
        );
    }

    #[test]
    fn the_reveal_animates_then_settles_and_reduce_motion_jumps() {
        let mut h = Harness::new(false, false);
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, 10.0);
        h.pass();
        h.frame(0.0); // seeds the clock
        let mid = h.frame(REVEAL_MS as f64 / 2.0).revealed();
        assert!(mid > 0.0 && mid < CONTENT_H, "mid-reveal: {mid}");
        let done = h.frame(REVEAL_MS as f64 * 2.0).revealed();
        assert!((done - CONTENT_H).abs() < 1e-6);

        // Reduce motion: the very first paint is already fully revealed.
        let mut h = Harness::new(false, true);
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, 10.0);
        h.pass();
        assert!((h.frame(0.0).revealed() - CONTENT_H).abs() < 1e-6);
    }

    #[test]
    fn a_focused_trigger_rings_and_toggles_on_space_or_enter() {
        let mut h = Harness::new(false, false);
        assert!(h.frame(0.0).ring().is_none());
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, 10.0);
        h.pass();
        let ring = h.frame(0.0).ring().expect("the trigger holds focus");
        assert!(ring.y0 < 0.0, "the ring sits outside the trigger row");
        assert!(
            ring.height() <= TRIGGER_H + style::FOCUS_RING_WIDTH + 1.0,
            "and hugs the trigger, not the whole widget"
        );

        let before = h.state.requests.len();
        h.key(Key::Character(" ".to_string()));
        h.pass();
        h.key(Key::Named(frust::authoring::NamedKey::Enter));
        assert_eq!(h.state.requests.len(), before + 2);
        h.key(Key::Named(frust::authoring::NamedKey::Tab));
        assert_eq!(h.state.requests.len(), before + 2);
    }

    #[test]
    fn hovering_the_trigger_asks_for_the_pointer_cursor() {
        let mut h = Harness::new(false, false);
        h.pointer(PointerPhase::Move, 10.0);
        assert_eq!(h.root.cursor(), CursorIcon::Pointer);
        h.pointer(PointerPhase::Move, WINDOW.height - 1.0);
        assert_eq!(h.root.cursor(), CursorIcon::Default);
    }

    #[test]
    fn a_disabled_collapsible_is_inert_asks_not_allowed_and_never_rings() {
        let mut h = Harness::new(true, false);
        h.pointer(PointerPhase::Move, 10.0);
        assert_eq!(h.root.cursor(), CursorIcon::NotAllowed);
        h.pointer(PointerPhase::Down, 10.0);
        h.pointer(PointerPhase::Up, 10.0);
        assert!(h.state.requests.is_empty());
        h.key(Key::Named(frust::authoring::NamedKey::Enter));
        assert!(h.state.requests.is_empty());
        assert!(h.frame(0.0).ring().is_none());
    }

    #[test]
    fn visit_children_publishes_the_trigger_and_the_content() {
        let w = build(&view(true, false));
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 2);
    }

    #[test]
    fn semantics_is_an_expandable_button_that_omits_a_collapsed_panel() {
        let mut h = Harness::new(false, false);
        let update = h.root.semantics();
        let closed = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("trigger node");
        assert_eq!(closed.1.is_expanded(), Some(false));
        assert!(closed.1.supports_action(Action::Click));
        let closed_children = closed.1.children().len();

        h.state.open = true;
        h.pass();
        let update = h.root.semantics();
        let open = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("trigger node");
        assert_eq!(open.1.is_expanded(), Some(true));
        // The revealed panel joins the tree (as a sibling of the trigger node).
        assert!(update.nodes.len() > closed_children + 1);
    }
}
