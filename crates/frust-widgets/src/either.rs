//! A two-arm view: [`Either`] and the lazy [`either`] helper.
//!
//! A helper whose body is an `if`/`else` (or a two-arm `match`) over two
//! different view types can return `impl View<State>` through [`either`]
//! instead of erasing both arms into an [`AnyView`](frust_core::AnyView). Both
//! arms stay statically typed; only the arm that is live is built.
//!
//! # When to use it
//!
//! * **Two arms** of different concrete types: [`either`] / [`Either`].
//! * **Three or more arms**: stay with [`any`](frust_core::any) /
//!   [`AnyView`](frust_core::AnyView) (nest `Either`s only if the nesting reads
//!   better than the erasure).
//! * **An optional child** inside a container: use the container's `.when(..)` /
//!   `.when_some(..)` sugar rather than an `Either` with an empty arm.
//!
//! # Swap semantics
//!
//! Switching arms tears the old arm's element down through the old arm's view,
//! builds the new arm's element from scratch and reports
//! `LAYOUT | PAINT`: the previous arm's retained state is dropped, exactly as
//! an `AnyView` concrete-type swap does. Rebuilding the *same* arm reconciles in
//! place and preserves its state.
//!
//! # Focus and capture bookkeeping
//!
//! [`EitherWidget`] is an enum widget delegating every pass to the live arm; it
//! holds no `AnyView` or `Box<dyn Widget>` field, so it is not the swap-blind
//! wrapper shape (`focus-wrapper-erasure-swap-blind`). One gap remains: both
//! arms share the single element type `EitherWidget<L, R>`, so a parent
//! reconciler comparing element type ids across the rebuild cannot see an arm
//! swap. `Either` therefore does the focus half of that bookkeeping itself: when
//! an arm swap happens while the focus chain runs through this view
//! ([`BuildCtx::has_focus`]), the focused widget just stopped existing and the
//! root is told to release the focus/IME session
//! ([`mark_focus_orphaned`](frust_core::mark_focus_orphaned)).

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, SemanticsCtx, View, Widget, mark_focus_orphaned,
};
use kurbo::Size;

/// A view that is one of two statically-typed arms. See the [module docs](self).
pub enum Either<L, R> {
    /// The left arm.
    Left(L),
    /// The right arm.
    Right(R),
}

/// Pick an arm by `cond`: `l` when true, `r` when false. Only the chosen
/// closure runs, so the unused arm is never constructed.
///
/// Use it for exactly two arms; for three or more, erase with
/// [`any`](frust_core::any). For an optional child use `.when(..)` /
/// `.when_some(..)` on the container.
pub fn either<L, R>(cond: bool, l: impl FnOnce() -> L, r: impl FnOnce() -> R) -> Either<L, R> {
    if cond {
        Either::Left(l())
    } else {
        Either::Right(r())
    }
}

/// Retained widget for [`Either`]: the live arm's widget, delegated to for every
/// pass.
pub enum EitherWidget<L, R> {
    /// The left arm's widget.
    Left(L),
    /// The right arm's widget.
    Right(R),
}

impl<L: Widget, R: Widget> Widget for EitherWidget<L, R> {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        match self {
            Self::Left(w) => w.layout(ctx, bc),
            Self::Right(w) => w.layout(ctx, bc),
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        match self {
            Self::Left(w) => w.paint(ctx, scene),
            Self::Right(w) => w.paint(ctx, scene),
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match self {
            Self::Left(w) => w.event(ctx, event),
            Self::Right(w) => w.event(ctx, event),
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        match self {
            Self::Left(w) => w.semantics(ctx),
            Self::Right(w) => w.semantics(ctx),
        }
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        match self {
            Self::Left(w) => w.visit_children(visitor),
            Self::Right(w) => w.visit_children(visitor),
        }
    }

    /// Report the live arm's name: the enum is a storage detail, never a tree
    /// element in its own right.
    fn type_name(&self) -> &'static str {
        match self {
            Self::Left(w) => w.type_name(),
            Self::Right(w) => w.type_name(),
        }
    }
}

impl<State: 'static, L: View<State>, R: View<State>> View<State> for Either<L, R> {
    type Element = EitherWidget<L::Element, R::Element>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        match self {
            Self::Left(v) => EitherWidget::Left(v.build(ctx)),
            Self::Right(v) => EitherWidget::Right(v.build(ctx)),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        match (self, prev, &mut *element) {
            (Self::Left(v), Self::Left(p), EitherWidget::Left(w)) => v.rebuild(p, w, ctx),
            (Self::Right(v), Self::Right(p), EitherWidget::Right(w)) => v.rebuild(p, w, ctx),
            _ => {
                // Arm swap: tear the old arm down through its own view, then
                // build the new arm fresh.
                prev.teardown(element, ctx);
                // Both arms share one element type, so the parent's type-id
                // comparison cannot see this swap; raise the focus-orphan
                // signal here when the live chain ran through this view.
                if ctx.has_focus() {
                    mark_focus_orphaned();
                }
                *element = self.build(ctx);
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
        }
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        match (self, element) {
            (Self::Left(v), EitherWidget::Left(w)) => v.teardown(w, ctx),
            (Self::Right(v), EitherWidget::Right(w)) => v.teardown(w, ctx),
            _ => {}
        }
    }

    fn message(&self, element: &mut Self::Element, state: &mut State) {
        match (self, element) {
            (Self::Left(v), EitherWidget::Left(w)) => v.message(w, state),
            (Self::Right(v), EitherWidget::Right(w)) => v.message(w, state),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use frust_core::accesskit::Role;
    use frust_core::{DiscardScene, FrameTime, RenderRoot, SemanticsUpdate, take_focus_orphaned};
    use frust_text::TextContext;

    use super::*;
    use crate::{ButtonView, CheckboxView, button, checkbox};

    type Log = Rc<RefCell<Vec<String>>>;

    /// A fixed-size arm that logs build / teardown / drop under its tag.
    struct Arm {
        tag: &'static str,
        size: Size,
        log: Log,
    }

    struct ArmWidget {
        tag: &'static str,
        size: Size,
        log: Log,
        painted: u32,
    }

    impl Drop for ArmWidget {
        fn drop(&mut self) {
            self.log.borrow_mut().push(format!("drop {}", self.tag));
        }
    }

    impl View<()> for Arm {
        type Element = ArmWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ArmWidget {
            self.log.borrow_mut().push(format!("build {}", self.tag));
            ArmWidget {
                tag: self.tag,
                size: self.size,
                log: Rc::clone(&self.log),
                painted: 0,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            el: &mut ArmWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            el.size = self.size;
            ChangeFlags::NONE
        }
        fn teardown(&self, _el: &mut ArmWidget, _ctx: &mut BuildCtx<'_>) {
            self.log.borrow_mut().push(format!("teardown {}", self.tag));
        }
    }

    impl Widget for ArmWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            self.painted += 1;
            self.log
                .borrow_mut()
                .push(format!("paint {} #{}", self.tag, self.painted));
        }
    }

    fn arm(tag: &'static str, w: f64, h: f64, log: &Log) -> Arm {
        Arm {
            tag,
            size: Size::new(w, h),
            log: Rc::clone(log),
        }
    }

    fn new_log() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }

    fn drain(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.borrow_mut())
    }

    type TwoArms = Either<Arm, Arm>;

    #[test]
    fn helper_is_lazy_and_picks_the_arm() {
        let built = Rc::new(RefCell::new(Vec::new()));
        let (b1, b2) = (Rc::clone(&built), Rc::clone(&built));
        let v = either(
            true,
            move || {
                b1.borrow_mut().push("l");
                1u8
            },
            move || {
                b2.borrow_mut().push("r");
                2u8
            },
        );
        assert!(matches!(v, Either::Left(1)));
        assert_eq!(*built.borrow(), vec!["l"], "only the chosen arm runs");
        assert!(matches!(either(false, || 1u8, || 2u8), Either::Right(2)));
    }

    #[test]
    fn layout_and_paint_delegate_to_the_active_arm() {
        let log = new_log();
        let mut root: RenderRoot<(), TwoArms> = RenderRoot::new();
        let mut state = ();
        let mut logic = |_: &mut ()| {
            either(
                false,
                || arm("a", 10.0, 10.0, &log),
                || arm("b", 30.0, 20.0, &log),
            )
        };
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        root.paint(&mut DiscardScene, FrameTime::from_nanos(0));
        let nodes = root.inspect();
        assert!(
            nodes[0].type_name.contains("EitherWidget"),
            "root is the either widget, got {}",
            nodes[0].type_name
        );
        assert_eq!(nodes[0].bounds.size(), Size::new(30.0, 20.0));
        let seen = drain(&log);
        assert!(seen.contains(&"build b".to_string()));
        assert!(seen.contains(&"paint b #1".to_string()));
        assert!(
            !seen.iter().any(|e| e.contains(" a")),
            "inactive arm untouched: {seen:?}"
        );
    }

    #[test]
    fn same_arm_rebuild_keeps_state_swap_resets_it() {
        let log = new_log();
        let mut root: RenderRoot<(), TwoArms> = RenderRoot::new();
        let mut state = ();
        let left = std::cell::Cell::new(true);
        let mut logic = |_: &mut ()| {
            either(
                left.get(),
                || arm("a", 10.0, 10.0, &log),
                || arm("b", 10.0, 10.0, &log),
            )
        };
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut DiscardScene, FrameTime::from_nanos(0));
        root.paint(&mut DiscardScene, FrameTime::from_nanos(1));
        assert_eq!(drain(&log), ["build a", "paint a #1", "paint a #2"]);

        // Same arm: reconciled in place, the paint counter survives.
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut DiscardScene, FrameTime::from_nanos(2));
        assert_eq!(drain(&log), ["paint a #3"]);

        // Swap: old arm torn down and dropped, new arm built fresh.
        left.set(false);
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root.paint(&mut DiscardScene, FrameTime::from_nanos(3));
        assert_eq!(
            drain(&log),
            ["teardown a", "build b", "drop a", "paint b #1"]
        );

        // Swap back: b's state is gone, a starts at #1 again.
        left.set(true);
        root.rebuild(&mut logic, &mut state);
        root.paint(&mut DiscardScene, FrameTime::from_nanos(4));
        assert_eq!(
            drain(&log),
            ["teardown b", "build a", "drop b", "paint a #1"]
        );
    }

    #[test]
    fn swap_reports_full_rebuild_flags_and_same_arm_does_not() {
        let log = new_log();
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let prev: TwoArms = Either::Left(arm("a", 1.0, 1.0, &log));
        let mut el = prev.build(&mut ctx);
        let same: TwoArms = Either::Left(arm("a", 1.0, 1.0, &log));
        assert_eq!(same.rebuild(&prev, &mut el, &mut ctx), ChangeFlags::NONE);
        let next: TwoArms = Either::Right(arm("b", 1.0, 1.0, &log));
        let flags = next.rebuild(&same, &mut el, &mut ctx);
        assert_eq!(flags, ChangeFlags::LAYOUT | ChangeFlags::PAINT);
        assert!(matches!(el, EitherWidget::Right(_)));
    }

    #[test]
    fn swap_under_a_live_focus_chain_orphans_focus_only_then() {
        let log = new_log();
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let a: TwoArms = Either::Left(arm("a", 1.0, 1.0, &log));
        let b: TwoArms = Either::Right(arm("b", 1.0, 1.0, &log));
        let _ = take_focus_orphaned();

        // No live chain: a swap raises nothing.
        ctx.set_has_focus(false);
        let mut el = a.build(&mut ctx);
        b.rebuild(&a, &mut el, &mut ctx);
        assert!(!take_focus_orphaned(), "unfocused swap marks nothing");

        // Live chain, same arm: nothing.
        ctx.set_has_focus(true);
        let b2: TwoArms = Either::Right(arm("b", 1.0, 1.0, &log));
        b2.rebuild(&b, &mut el, &mut ctx);
        assert!(!take_focus_orphaned(), "same-arm rebuild marks nothing");

        // Live chain, swap: the session is released.
        a.rebuild(&b2, &mut el, &mut ctx);
        assert!(take_focus_orphaned(), "focused swap orphans focus");
    }

    type Ui = Either<ButtonView<()>, CheckboxView<()>>;

    fn semantics_for(show_button: bool) -> SemanticsUpdate {
        let mut root: RenderRoot<(), Ui> = RenderRoot::new();
        let mut state = ();
        let mut logic = |_: &mut ()| -> Ui {
            either(
                show_button,
                || button::<(), _>("Go", |_| {}),
                || checkbox::<(), _>(true, "Agree", |_, _| {}),
            )
        };
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 300.0), &mut tcx as &mut dyn std::any::Any);
        root.semantics()
    }

    #[test]
    fn semantics_tree_reflects_the_active_arm() {
        let role_of_child = |u: &SemanticsUpdate| {
            let root = &u.nodes.iter().find(|(id, _)| *id == u.root).unwrap().1;
            let kids = root.children();
            assert_eq!(kids.len(), 1, "exactly the live arm contributes");
            u.nodes
                .iter()
                .find(|(id, _)| *id == kids[0])
                .unwrap()
                .1
                .role()
        };
        assert_eq!(role_of_child(&semantics_for(true)), Role::Button);
        assert_eq!(role_of_child(&semantics_for(false)), Role::CheckBox);
    }

    #[test]
    fn semantics_follow_a_live_swap() {
        let mut root: RenderRoot<(), Ui> = RenderRoot::new();
        let mut state = ();
        let show_button = std::cell::Cell::new(true);
        let mut logic = |_: &mut ()| -> Ui {
            either(
                show_button.get(),
                || button::<(), _>("Go", |_| {}),
                || checkbox::<(), _>(true, "Agree", |_, _| {}),
            )
        };
        let mut tcx = TextContext::new();
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(300.0, 300.0), &mut tcx as &mut dyn std::any::Any);
        let before = root.semantics();
        show_button.set(false);
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(300.0, 300.0), &mut tcx as &mut dyn std::any::Any);
        let after = root.semantics();
        let roles = |u: &SemanticsUpdate| u.nodes.iter().map(|(_, n)| n.role()).collect::<Vec<_>>();
        assert!(roles(&before).contains(&Role::Button));
        assert!(!roles(&before).contains(&Role::CheckBox));
        assert!(roles(&after).contains(&Role::CheckBox));
        assert!(!roles(&after).contains(&Role::Button));
    }
}
