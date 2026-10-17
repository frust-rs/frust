//! Statically-typed branching views: the two-arm [`Either`] with its lazy
//! [`either`] helper, and the three-to-eight-arm [`EitherOf3`] ..
//! [`EitherOf8`].
//!
//! A helper whose body is an `if`/`else` (or a `match`) over several different
//! view types can return `impl View<State>` through these enums instead of
//! erasing the arms into an [`AnyView`](frust_core::AnyView). Every arm stays
//! statically typed; only the arm that is live is built.
//!
//! # When to use it
//!
//! * **Two arms** of different concrete types: [`either`] / [`Either`].
//! * **Three to eight arms** of different concrete types: [`EitherOf3`] ..
//!   [`EitherOf8`] (variants `A`, `B`, `C`, ..) built from a `match`.
//! * **More than eight arms, or a stored page table**: stay with
//!   [`any`](frust_core::any) / [`AnyView`](frust_core::AnyView).
//! * **The same concrete type in every arm**: build that one type
//!   conditionally instead. `Either` / `EitherOfN` remount on an arm swap, so
//!   they would discard state a single conditionally-configured view keeps.
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
//! [`EitherWidget`] (and every `EitherOfNWidget`) is an enum widget delegating
//! every pass to the live arm; it holds no `AnyView` or `Box<dyn Widget>`
//! field, so it is not the swap-blind wrapper shape
//! (`focus-wrapper-erasure-swap-blind`). Every arm shares the single element
//! type (`EitherWidget<L, R>`, `EitherOf3Widget<A, B, C>`, ..), so a parent
//! reconciler comparing element type ids across the rebuild cannot see an arm
//! swap by itself. Each enum therefore **reports** the swap
//! ([`BuildCtx::mark_element_replaced`]), and the enclosing reconciler (the
//! `ChildPod` that holds it, through `rebuild_child` / `rebuild_children`, or a
//! component's child pod) drops the pod's capture and focus link exactly as for
//! an `AnyView` concrete-type swap — and, in a positional list, cancels the
//! recorded paths of every later sibling. An enum nested inline inside another
//! enum propagates the report to the outer one's pod; a pod inside the swapped
//! arm consumes its own.
//!
//! The enum also raises the focus orphan itself
//! ([`mark_focus_orphaned`](frust_core::mark_focus_orphaned)) when the swap
//! happens while the focus chain runs through it ([`BuildCtx::has_focus`]): a
//! root-level enum, or one under a container that does not reconcile through a
//! pod, has no enclosing reconciler to do it. The orphan flag is idempotent, so
//! the two marks never double-release.

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, SemanticsCtx, SliverWidget, View, Widget, mark_focus_orphaned,
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
/// Use it for exactly two arms of different types; for three to eight arms use
/// [`EitherOf3`] .. [`EitherOf8`], and for more than eight (or a stored page
/// table) erase with [`any`](frust_core::any). When every arm is the same
/// concrete type, build that one type conditionally instead (a swap remounts).
/// For an optional child use `.when(..)` / `.when_some(..)` on the container.
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

    /// The live arm's sliver face: a sliver arm stays a sliver through the enum,
    /// and a box arm keeps answering `None`.
    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        match self {
            Self::Left(w) => w.as_sliver(),
            Self::Right(w) => w.as_sliver(),
        }
    }

    /// The live arm's answer: the enum's pod is the arm's Tab stop.
    fn is_focusable(&self) -> bool {
        match self {
            Self::Left(w) => w.is_focusable(),
            Self::Right(w) => w.is_focusable(),
        }
    }

    /// The live arm's traversal children, so a narrowing arm (a navigator, an
    /// `IndexedStack`) stays narrowed through the enum.
    fn visit_focus_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        match self {
            Self::Left(w) => w.visit_focus_children(visitor),
            Self::Right(w) => w.visit_focus_children(visitor),
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
                // comparison cannot see this swap: report it, so the enclosing
                // reconciler drops the pod's capture and focus link as for an
                // `AnyView` swap. Raise the focus orphan here too when the live
                // chain ran through this view — a root-level or pod-less parent
                // has no reconciler to do it (the flag is idempotent).
                ctx.mark_element_replaced();
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

/// Generates `EitherOfN`, `EitherOfNWidget` and their `Widget` / `View` impls
/// with exactly the semantics of [`Either`].
macro_rules! either_of {
    ($(#[$meta:meta])* $arity:literal, $E:ident, $W:ident; $($V:ident),+ $(,)?) => {
        $(#[$meta])*
        #[doc = ""]
        #[doc = concat!("A view that is one of ", $arity, " statically-typed arms. Arm swaps behave as for [`Either`]: the old arm is torn down, the new one built fresh, and `LAYOUT | PAINT` is reported. See the [module docs](self).")]
        pub enum $E<$($V),+> {
            $(
                #[doc = concat!("Arm `", stringify!($V), "`.")]
                $V($V),
            )+
        }

        #[doc = concat!("Retained widget for [`", stringify!($E), "`]: the live arm's widget, delegated to for every pass.")]
        pub enum $W<$($V),+> {
            $(
                #[doc = concat!("Arm `", stringify!($V), "`'s widget.")]
                $V($V),
            )+
        }

        impl<$($V: Widget),+> Widget for $W<$($V),+> {
            fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                match self { $(Self::$V(w) => w.layout(ctx, bc),)+ }
            }

            fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
                match self { $(Self::$V(w) => w.paint(ctx, scene),)+ }
            }

            fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
                match self { $(Self::$V(w) => w.event(ctx, event),)+ }
            }

            fn semantics(&self, ctx: &mut SemanticsCtx) {
                match self { $(Self::$V(w) => w.semantics(ctx),)+ }
            }

            fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
                match self { $(Self::$V(w) => w.visit_children(visitor),)+ }
            }

            /// Report the live arm's name: the enum is a storage detail, never
            /// a tree element in its own right.
            fn type_name(&self) -> &'static str {
                match self { $(Self::$V(w) => w.type_name(),)+ }
            }

            /// The live arm's sliver face, forwarded as for [`EitherWidget`].
            fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
                match self { $(Self::$V(w) => w.as_sliver(),)+ }
            }

            /// The live arm's answer, forwarded as for [`EitherWidget`].
            fn is_focusable(&self) -> bool {
                match self { $(Self::$V(w) => w.is_focusable(),)+ }
            }

            /// The live arm's traversal children, forwarded as for [`EitherWidget`].
            fn visit_focus_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
                match self { $(Self::$V(w) => w.visit_focus_children(visitor),)+ }
            }
        }

        impl<State: 'static, $($V: View<State>),+> View<State> for $E<$($V),+> {
            type Element = $W<$($V::Element),+>;

            fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
                match self { $(Self::$V(v) => $W::$V(v.build(ctx)),)+ }
            }

            fn rebuild(
                &self,
                prev: &Self,
                element: &mut Self::Element,
                ctx: &mut BuildCtx<'_>,
            ) -> ChangeFlags {
                match (self, prev, &mut *element) {
                    $((Self::$V(v), Self::$V(p), $W::$V(w)) => v.rebuild(p, w, ctx),)+
                    _ => {
                        // Arm swap: see `Either::rebuild`.
                        prev.teardown(element, ctx);
                        ctx.mark_element_replaced();
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
                    $((Self::$V(v), $W::$V(w)) => v.teardown(w, ctx),)+
                    _ => {}
                }
            }

            fn message(&self, element: &mut Self::Element, state: &mut State) {
                match (self, element) {
                    $((Self::$V(v), $W::$V(w)) => v.message(w, state),)+
                    _ => {}
                }
            }
        }
    };
}

either_of! {
    /// Pick one of three view types from a `match` while keeping every arm
    /// statically typed (no [`AnyView`](frust_core::AnyView) erasure).
    ///
    /// ```
    /// use frust_core::View;
    /// use frust_widgets::{EitherOf3, button, checkbox, text};
    ///
    /// enum Phase {
    ///     Idle,
    ///     Busy,
    ///     Done,
    /// }
    ///
    /// struct Screen;
    ///
    /// impl Screen {
    ///     fn body(phase: Phase) -> impl View<()> {
    ///         match phase {
    ///             Phase::Idle => EitherOf3::A(text("Idle")),
    ///             Phase::Busy => EitherOf3::B(button::<(), _>("Cancel", |_| {})),
    ///             Phase::Done => EitherOf3::C(checkbox::<(), _>(true, "Ok", |_, _| {})),
    ///         }
    ///     }
    /// }
    ///
    /// let _ = Screen::body(Phase::Idle);
    /// let _ = Screen::body(Phase::Busy);
    /// let _ = Screen::body(Phase::Done);
    /// ```
    "three", EitherOf3, EitherOf3Widget; A, B, C
}
either_of! { "four", EitherOf4, EitherOf4Widget; A, B, C, D }
either_of! { "five", EitherOf5, EitherOf5Widget; A, B, C, D, E }
either_of! { "six", EitherOf6, EitherOf6Widget; A, B, C, D, E, F }
either_of! { "seven", EitherOf7, EitherOf7Widget; A, B, C, D, E, F, G }
either_of! { "eight", EitherOf8, EitherOf8Widget; A, B, C, D, E, F, G, H }

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use frust_core::accesskit::Role;
    use frust_core::{DiscardScene, FrameTime, RenderRoot, SemanticsUpdate, take_focus_orphaned};
    use frust_text::TextContext;

    use super::*;
    use crate::{ButtonView, CheckboxView, button, checkbox, text};

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

    /// The behaviour matrix, instantiated per enum so `Either` and `EitherOfN`
    /// cannot drift. `$a` / `$b` are the first two variants.
    macro_rules! swap_matrix {
        ($m:ident, $E:ident, $W:ident, $a:ident, $b:ident, $arms:ty, $ui:ty) => {
            mod $m {
                use super::*;

                type Arms = $arms;
                type Ui = $ui;

                fn pick(first: bool, log: &Log, sa: (f64, f64), sb: (f64, f64)) -> Arms {
                    if first {
                        $E::$a(arm("a", sa.0, sa.1, log))
                    } else {
                        $E::$b(arm("b", sb.0, sb.1, log))
                    }
                }

                #[test]
                fn layout_and_paint_delegate_to_the_live_variant() {
                    let log = new_log();
                    let mut root: RenderRoot<(), Arms> = RenderRoot::new();
                    let mut state = ();
                    let mut logic = |_: &mut ()| pick(false, &log, (10.0, 10.0), (30.0, 20.0));
                    root.rebuild(&mut logic, &mut state);
                    root.layout(Size::new(200.0, 200.0));
                    root.paint(&mut DiscardScene, FrameTime::from_nanos(0));
                    let nodes = root.inspect();
                    assert!(
                        nodes[0].type_name.contains(stringify!($W)),
                        "root is the enum widget, got {}",
                        nodes[0].type_name
                    );
                    assert_eq!(nodes[0].bounds.size(), Size::new(30.0, 20.0));
                    let seen = drain(&log);
                    assert!(seen.contains(&"build b".to_string()));
                    assert!(seen.contains(&"paint b #1".to_string()));
                    assert!(
                        !seen.iter().any(|e| e.contains(" a")),
                        "inactive variant untouched: {seen:?}"
                    );
                }

                #[test]
                fn same_variant_rebuild_keeps_state_swap_resets_it() {
                    let log = new_log();
                    let mut root: RenderRoot<(), Arms> = RenderRoot::new();
                    let mut state = ();
                    let first = std::cell::Cell::new(true);
                    let mut logic =
                        |_: &mut ()| pick(first.get(), &log, (10.0, 10.0), (10.0, 10.0));
                    root.rebuild(&mut logic, &mut state);
                    root.layout(Size::new(100.0, 100.0));
                    root.paint(&mut DiscardScene, FrameTime::from_nanos(0));
                    root.paint(&mut DiscardScene, FrameTime::from_nanos(1));
                    assert_eq!(drain(&log), ["build a", "paint a #1", "paint a #2"]);

                    root.rebuild(&mut logic, &mut state);
                    root.layout(Size::new(100.0, 100.0));
                    root.paint(&mut DiscardScene, FrameTime::from_nanos(2));
                    assert_eq!(drain(&log), ["paint a #3"]);

                    first.set(false);
                    root.rebuild(&mut logic, &mut state);
                    root.layout(Size::new(100.0, 100.0));
                    root.paint(&mut DiscardScene, FrameTime::from_nanos(3));
                    assert_eq!(
                        drain(&log),
                        ["teardown a", "build b", "drop a", "paint b #1"]
                    );

                    first.set(true);
                    root.rebuild(&mut logic, &mut state);
                    root.paint(&mut DiscardScene, FrameTime::from_nanos(4));
                    assert_eq!(
                        drain(&log),
                        ["teardown b", "build a", "drop b", "paint a #1"]
                    );
                }

                #[test]
                fn swap_reports_full_rebuild_flags_and_same_variant_does_not() {
                    let log = new_log();
                    let mut counter = 0u64;
                    let mut ctx = BuildCtx::new(&mut counter);
                    let prev = pick(true, &log, (1.0, 1.0), (1.0, 1.0));
                    let mut el = prev.build(&mut ctx);
                    let same = pick(true, &log, (1.0, 1.0), (1.0, 1.0));
                    assert_eq!(same.rebuild(&prev, &mut el, &mut ctx), ChangeFlags::NONE);
                    let next = pick(false, &log, (1.0, 1.0), (1.0, 1.0));
                    let flags = next.rebuild(&same, &mut el, &mut ctx);
                    assert_eq!(flags, ChangeFlags::LAYOUT | ChangeFlags::PAINT);
                    assert!(matches!(el, $W::$b(_)));
                }

                #[test]
                fn an_arm_swap_reports_element_replaced_and_same_variant_does_not() {
                    let log = new_log();
                    let mut counter = 0u64;
                    let mut ctx = BuildCtx::new(&mut counter);
                    let prev = pick(true, &log, (1.0, 1.0), (1.0, 1.0));
                    let mut el = prev.build(&mut ctx);
                    let same = pick(true, &log, (2.0, 2.0), (1.0, 1.0));
                    let (_, replaced) =
                        ctx.track_element_replaced(|ctx| same.rebuild(&prev, &mut el, ctx));
                    assert!(!replaced, "a same-variant rebuild keeps the widget");
                    let next = pick(false, &log, (1.0, 1.0), (1.0, 1.0));
                    let (_, replaced) =
                        ctx.track_element_replaced(|ctx| next.rebuild(&same, &mut el, ctx));
                    assert!(replaced, "an arm swap reports the replaced widget");
                }

                #[test]
                fn swap_under_a_live_focus_chain_orphans_focus_only_then() {
                    let log = new_log();
                    let mut counter = 0u64;
                    let mut ctx = BuildCtx::new(&mut counter);
                    let a = pick(true, &log, (1.0, 1.0), (1.0, 1.0));
                    let b = pick(false, &log, (1.0, 1.0), (1.0, 1.0));
                    let _ = take_focus_orphaned();

                    ctx.set_has_focus(false);
                    let mut el = a.build(&mut ctx);
                    b.rebuild(&a, &mut el, &mut ctx);
                    assert!(!take_focus_orphaned(), "unfocused swap marks nothing");

                    ctx.set_has_focus(true);
                    let b2 = pick(false, &log, (1.0, 1.0), (1.0, 1.0));
                    b2.rebuild(&b, &mut el, &mut ctx);
                    assert!(!take_focus_orphaned(), "same-variant rebuild marks nothing");

                    a.rebuild(&b2, &mut el, &mut ctx);
                    assert!(take_focus_orphaned(), "focused swap orphans focus");
                }

                fn ui(show_button: bool) -> Ui {
                    if show_button {
                        $E::$a(button::<(), _>("Go", |_| {}))
                    } else {
                        $E::$b(checkbox::<(), _>(true, "Agree", |_, _| {}))
                    }
                }

                fn roles(u: &SemanticsUpdate) -> Vec<Role> {
                    u.nodes.iter().map(|(_, n)| n.role()).collect()
                }

                #[test]
                fn semantics_reflect_the_live_variant_and_follow_a_swap() {
                    let mut root: RenderRoot<(), Ui> = RenderRoot::new();
                    let mut state = ();
                    let show_button = std::cell::Cell::new(true);
                    let mut logic = |_: &mut ()| ui(show_button.get());
                    let mut tcx = TextContext::new();
                    root.rebuild(&mut logic, &mut state);
                    root.layout_with_text(
                        Size::new(300.0, 300.0),
                        &mut tcx as &mut dyn std::any::Any,
                    );
                    let before = root.semantics();
                    let top = &before
                        .nodes
                        .iter()
                        .find(|(id, _)| *id == before.root)
                        .unwrap()
                        .1;
                    assert_eq!(
                        top.children().len(),
                        1,
                        "exactly the live variant contributes"
                    );
                    show_button.set(false);
                    root.rebuild(&mut logic, &mut state);
                    root.layout_with_text(
                        Size::new(300.0, 300.0),
                        &mut tcx as &mut dyn std::any::Any,
                    );
                    let after = root.semantics();
                    assert!(roles(&before).contains(&Role::Button));
                    assert!(!roles(&before).contains(&Role::CheckBox));
                    assert!(roles(&after).contains(&Role::CheckBox));
                    assert!(!roles(&after).contains(&Role::Button));
                }
            }
        };
    }

    swap_matrix!(
        either_matrix,
        Either,
        EitherWidget,
        Left,
        Right,
        Either<Arm, Arm>,
        Either<ButtonView<()>, CheckboxView<()>>
    );
    swap_matrix!(
        either_of_3_matrix,
        EitherOf3,
        EitherOf3Widget,
        A,
        B,
        EitherOf3<Arm, Arm, Arm>,
        EitherOf3<ButtonView<()>, CheckboxView<()>, ButtonView<()>>
    );
    swap_matrix!(
        either_of_8_matrix,
        EitherOf8,
        EitherOf8Widget,
        A,
        B,
        EitherOf8<Arm, Arm, Arm, Arm, Arm, Arm, Arm, Arm>,
        EitherOf8<
            ButtonView<()>,
            CheckboxView<()>,
            ButtonView<()>,
            ButtonView<()>,
            ButtonView<()>,
            ButtonView<()>,
            ButtonView<()>,
            ButtonView<()>,
        >
    );

    /// A leaf that is a Tab stop when `focusable`, owning two pods of which
    /// traversal may enter only the first.
    struct FocusProbe {
        focusable: bool,
        pods: [ChildPod; 2],
    }

    impl Widget for FocusProbe {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.min()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
            for pod in &self.pods {
                visitor(pod);
            }
        }
        fn is_focusable(&self) -> bool {
            self.focusable
        }
        fn visit_focus_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
            visitor(&self.pods[0]);
        }
    }

    fn probe(focusable: bool) -> FocusProbe {
        let log = new_log();
        let leaf = |tag| {
            let widget = arm(tag, 1.0, 1.0, &log).build(&mut BuildCtx::new(&mut 0u64));
            ChildPod::new(Box::new(widget))
        };
        FocusProbe {
            focusable,
            pods: [leaf("x"), leaf("y")],
        }
    }

    /// `(is_focusable, children visited, focus children visited)` for `widget`.
    fn focus_face(widget: &dyn Widget) -> (bool, usize, usize) {
        let (mut all, mut focus) = (0, 0);
        widget.visit_children(&mut |_| all += 1);
        widget.visit_focus_children(&mut |_| focus += 1);
        (widget.is_focusable(), all, focus)
    }

    #[test]
    fn focus_queries_forward_to_the_live_arm() {
        type Two = EitherWidget<FocusProbe, FocusProbe>;
        assert_eq!(focus_face(&Two::Left(probe(true))), (true, 2, 1));
        assert_eq!(focus_face(&Two::Right(probe(false))), (false, 2, 1));
        type Three = EitherOf3Widget<FocusProbe, FocusProbe, FocusProbe>;
        assert_eq!(focus_face(&Three::C(probe(true))), (true, 2, 1));
        assert_eq!(focus_face(&Three::A(probe(false))), (false, 2, 1));
        type Eight = EitherOf8Widget<
            ArmWidget,
            ArmWidget,
            ArmWidget,
            ArmWidget,
            ArmWidget,
            ArmWidget,
            ArmWidget,
            FocusProbe,
        >;
        assert_eq!(focus_face(&Eight::H(probe(true))), (true, 2, 1));
    }

    #[test]
    fn tab_reaches_a_control_behind_either() {
        let mut root: RenderRoot<u32, crate::FlexView<u32>> = RenderRoot::new();
        let mut logic = |_: &mut u32| {
            let arm = either(
                true,
                || button::<u32, _>("Go", |n: &mut u32| *n += 1),
                || text("idle"),
            );
            crate::column().child(arm)
        };
        let mut presses = 0u32;
        let mut tcx = TextContext::new();
        root.rebuild(&mut logic, &mut presses);
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn std::any::Any);
        root.paint(&mut DiscardScene, FrameTime::from_nanos(0));
        let key = |key| {
            InputEvent::Key(frust_core::KeyEvent {
                key,
                modifiers: frust_core::Modifiers::default(),
                repeat: false,
            })
        };
        root.event(
            &mut presses,
            &key(frust_core::Key::Named(frust_core::NamedKey::Tab)),
        );
        assert!(
            root.is_focus_active(),
            "the enum's pod is the button's stop"
        );
        root.event(&mut presses, &key(frust_core::Key::Character(" ".into())));
        assert_eq!(presses, 1);
    }

    #[test]
    fn eight_variant_instance_swaps_a_to_h() {
        type Eight = EitherOf8<Arm, Arm, Arm, Arm, Arm, Arm, Arm, Arm>;
        let log = new_log();
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let a: Eight = EitherOf8::A(arm("a", 1.0, 1.0, &log));
        let h: Eight = EitherOf8::H(arm("h", 5.0, 5.0, &log));
        let mut el = a.build(&mut ctx);
        assert!(matches!(el, EitherOf8Widget::A(_)));
        let flags = h.rebuild(&a, &mut el, &mut ctx);
        assert_eq!(flags, ChangeFlags::LAYOUT | ChangeFlags::PAINT);
        assert!(matches!(el, EitherOf8Widget::H(_)));
        assert_eq!(drain(&log), ["build a", "teardown a", "build h", "drop a"]);
        h.teardown(&mut el, &mut ctx);
        assert_eq!(drain(&log), ["teardown h"]);
    }
}
