//! Indexed stack: keeps every child mounted, shows and routes to one of them.
//!
//! [`IndexedStackView`]/[`IndexedStackWidget`] hold a list of children and an
//! optional selected index. Every child is built, rebuilt and laid out on every
//! pass, so each one keeps its widget state (a tab's scroll offset, a tab's own
//! navigator stack) while it is not selected. Only the selected child is
//! painted, receives input, appears in the accessibility tree, and may claim a
//! back press. This is the container for per-tab navigation stacks: each tab
//! holds its own navigator, and only the visible tab's navigator answers back.
//!
//! See [`IndexedStack`] for the reach rule (R23), the page-switch contract, and
//! the layout choice.

use std::cell::Cell;
use std::rc::Rc;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EditingState, EventCtx, EventResult,
    ImeState, InputEvent, LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, ViewSeq, Widget,
    any,
};
use kurbo::{Point, Size};

use crate::nav::{ambient_page_reach, with_page_reach};

/// A declarative indexed stack. See [`IndexedStack`] and the
/// [module docs](self).
pub struct IndexedStackView<State: 'static> {
    index: Option<usize>,
    children: Vec<AnyView<State>>,
}

/// Keep every child of `children` mounted and show only the one at `index`.
///
/// `children` is any [`ViewSeq`]: a `Vec`/array of views, a tuple of mixed view
/// types (positional, up to 12 and nestable), an `Option` (`None` drops the
/// slot), or `views(iter)` for an iterator. Every element is erased once, here.
/// An `index` that is `None` or out of range selects nothing: the stack paints
/// nothing, routes no input and exposes no accessibility node, while every child
/// stays mounted.
///
/// # Reach follows the selection (R23)
///
/// Only the selected child is reachable. Input events go to it alone, the
/// accessibility tree contains it alone, and a navigator anywhere inside an
/// unselected child reports
/// [`back_interest`](crate::NavigatorController::back_interest)` == false`, the
/// same rule that silences a navigator on a covered navigator page. The stack
/// does this by installing one page-reach cell per child, `true` only for the
/// selected child, and only while the stack itself is reachable. A stack built
/// on a navigator page that later gets covered therefore leaves every tab
/// navigator unreachable, at any nesting depth. Cells are rewritten on every
/// rebuild, before the children are reconciled, so a nested navigator always
/// reads the value for the selection it is being rebuilt under.
///
/// Broadcast events (housekeeping, overlay and drop-end notifications) are not
/// user input and go to every child, as they do for the navigator's pages: a
/// navigator in an unselected tab may hold a queued pop result that must still
/// flush on the frame that produced it. The one exception is
/// [`InputEvent::Shortcut`]: it reaches the selected child only, so a shortcut
/// an unselected child declared is never answered and the root falls through
/// to its next candidate.
///
/// # Page-switch contract
///
/// When the selected index changes, the outgoing child is handled the way a
/// navigator handles a covered page, in this order: an in-flight pointer
/// capture receives a synthetic `Cancel` (so it never fires on a later `Up`),
/// its focus flag is cleared, and, if that child held focus on the live focus
/// chain, the next paint publishes a cleared IME surface so the platform
/// keyboard hides. A child that never held focus publishes nothing, so a text
/// field elsewhere in the tree keeps its session. The incoming child is not
/// focused automatically.
///
/// # Layout
///
/// Every child is laid out under the stack's loosened constraints at the stack
/// origin, and the stack takes the size of its largest child, clamped to its
/// constraints. This matches Flutter's `IndexedStack`: the stack does not
/// change size when the selection changes.
///
/// ```
/// use frust_widgets::{IndexedStack, IndexedStackView, Stack, indexed_stack, text};
/// # fn demo() -> (IndexedStackView<()>, IndexedStackView<()>) {
/// // Mixed child types: a tuple, no `any(..)` needed.
/// let tabs = IndexedStack(Some(1), (text("home"), Stack((text("feed"), text("badge")))));
/// // The fluent form.
/// let fluent = indexed_stack(Some(0)).child(text("home")).child(text("search"));
/// # (tabs, fluent)
/// # }
/// # let _ = demo();
/// ```
#[allow(non_snake_case)]
pub fn IndexedStack<State: 'static, M>(
    index: Option<usize>,
    children: impl ViewSeq<State, M>,
) -> IndexedStackView<State> {
    indexed_stack(index).children(children)
}

/// An empty indexed stack selecting `index`, ready for fluent children (no
/// `any(..)` wrapping). See [`IndexedStack`].
pub fn indexed_stack<State: 'static>(index: Option<usize>) -> IndexedStackView<State> {
    IndexedStackView {
        index,
        children: Vec::new(),
    }
}

impl<State: 'static> IndexedStackView<State> {
    /// Append a child after the existing ones. Accepts any [`View`].
    pub fn child<V: View<State>>(mut self, view: V) -> Self {
        self.children.push(any(view));
        self
    }

    /// Append every element of `children` after the existing ones, in order.
    ///
    /// Accepts any [`ViewSeq`] (`Vec`, array, tuple, `Option`, nested, or
    /// `views(iter)`); tuples are positional and a `None` element drops its slot.
    pub fn children<M>(mut self, children: impl ViewSeq<State, M>) -> Self {
        children.extend_views(&mut self.children);
        self
    }

    /// Apply `f` to the builder only when `cond` is true.
    pub fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }

    /// Apply `f` with the contained value when `opt` is `Some`.
    pub fn when_some<T>(self, opt: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        match opt {
            Some(value) => f(self, value),
            None => self,
        }
    }
}

/// The retained widget for an [`IndexedStackView`].
pub struct IndexedStackWidget {
    children: Vec<ChildPod>,
    /// One page-reach cell per child, installed around that child's
    /// build/rebuild/teardown. `true` only for the selected child while the
    /// stack itself is reachable.
    reach: Vec<Rc<Cell<bool>>>,
    index: Option<usize>,
    /// Set when a selection change took focus away from the outgoing child on
    /// the live chain; the next paint publishes a cleared IME surface.
    needs_ime_clear: bool,
}

/// Whether the ambient page this stack lives on is reachable: `true` at the
/// top level (no hosting page).
fn parent_reachable() -> bool {
    ambient_page_reach().is_none_or(|cell| cell.get())
}

impl IndexedStackWidget {
    /// The selected pod, if the index names an existing child.
    fn selected_mut(&mut self) -> Option<&mut ChildPod> {
        self.index.and_then(|i| self.children.get_mut(i))
    }

    fn selected(&self) -> Option<&ChildPod> {
        self.index.and_then(|i| self.children.get(i))
    }

    /// Apply the page-switch contract to the currently selected child, which is
    /// about to stop being selected: cancel its capture, clear its focus flag,
    /// and queue a cleared IME publish when it held focus on the live chain.
    ///
    /// The focus flag is read before anything clears it, as the navigator's
    /// `apply_pop` does; reading it afterwards would always report `false`.
    fn switch_away(&mut self, ctx: &BuildCtx<'_>) {
        let Some(pod) = self.selected_mut() else {
            return;
        };
        let outgoing_focused = pod.is_focused();
        if pod.is_active() {
            crate::authoring::cancel_pod(pod);
            pod.set_active(false);
        }
        if outgoing_focused {
            pod.set_focused(false);
        }
        if outgoing_focused && ctx.has_focus() {
            self.needs_ime_clear = true;
        }
    }

    /// Rewrite every reach cell for the current selection.
    fn publish_reach(&self, parent_reachable: bool) {
        for (i, cell) in self.reach.iter().enumerate() {
            cell.set(parent_reachable && Some(i) == self.index);
        }
    }
}

impl<State: 'static> View<State> for IndexedStackView<State> {
    type Element = IndexedStackWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> IndexedStackWidget {
        let parent = parent_reachable();
        let reach: Vec<Rc<Cell<bool>>> = (0..self.children.len())
            .map(|i| Rc::new(Cell::new(parent && Some(i) == self.index)))
            .collect();
        let children = self
            .children
            .iter()
            .zip(&reach)
            .map(|(view, cell)| with_page_reach(cell, || crate::authoring::build_child(view, ctx)))
            .collect();
        IndexedStackWidget {
            children,
            reach,
            index: self.index,
            needs_ime_clear: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut IndexedStackWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        // Re-read the ambient parent: this subtree may have moved, or its
        // hosting page may have been covered, since the last pass.
        let parent = parent_reachable();

        // The outgoing child is handled first, against the pods as they were.
        if element.index != self.index {
            element.switch_away(ctx);
            element.index = self.index;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Resize the reach cells to the new child count and rewrite them all
        // before any child is reconciled. The cells of a shrunk tail are kept
        // aside, set unreachable, so those children are torn down inside their
        // own (now false) scope.
        let next_len = self.children.len();
        let removed_reach = if element.reach.len() > next_len {
            element.reach.split_off(next_len)
        } else {
            Vec::new()
        };
        for cell in &removed_reach {
            cell.set(false);
        }
        element
            .reach
            .resize_with(next_len, || Rc::new(Cell::new(false)));
        element.publish_reach(parent);

        // Reconcile per index rather than through `rebuild_children`: each
        // child needs its own reach scope around its rebuild, and the shared
        // reconciler runs all children in one call with no per-child hook.
        // Positional matching is the right identity here anyway, since a slot
        // index is exactly what the selection addresses. `rebuild_child`
        // handles a type swap at one slot (drops its capture and focus, marks
        // a live focus orphan) without touching the other slots, so a swap in
        // one tab never cancels another tab's capture or focus.
        let IndexedStackWidget {
            children: pods,
            reach,
            ..
        } = element;
        let common = prev.children.len().min(next_len);
        for i in 0..common {
            let (prev_view, next_view, pod) = (&prev.children[i], &self.children[i], &mut pods[i]);
            flags |= with_page_reach(&reach[i], || {
                crate::authoring::rebuild_child(prev_view, next_view, pod, ctx)
            });
        }
        if next_len > common {
            for (view, cell) in self.children[common..].iter().zip(&reach[common..]) {
                pods.push(with_page_reach(cell, || {
                    crate::authoring::build_child(view, ctx)
                }));
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if prev.children.len() > next_len {
            for ((view, pod), cell) in prev.children[next_len..]
                .iter()
                .zip(pods[next_len..].iter_mut())
                .zip(&removed_reach)
            {
                with_page_reach(cell, || crate::authoring::teardown_child(view, pod, ctx));
            }
            pods.truncate(next_len);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut IndexedStackWidget, ctx: &mut BuildCtx<'_>) {
        // The whole stack is leaving: nothing in it is reachable any more.
        for cell in &element.reach {
            cell.set(false);
        }
        for ((view, pod), cell) in self
            .children
            .iter()
            .zip(element.children.iter_mut())
            .zip(&element.reach)
        {
            with_page_reach(cell, || crate::authoring::teardown_child(view, pod, ctx));
        }
    }
}

impl Widget for IndexedStackWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Every child, selected or not, so the stack keeps one size across
        // selection changes (Flutter `IndexedStack` parity).
        let loose = bc.loosen();
        let mut max = Size::ZERO;
        for pod in &mut self.children {
            let size = pod.layout_child(ctx, &loose);
            pod.set_origin(Point::ZERO);
            max = Size::new(max.width.max(size.width), max.height.max(size.height));
        }
        bc.constrain(max)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if let Some(pod) = self.selected_mut() {
            pod.paint_child(ctx, scene);
        }
        // Deterministic IME hide after a selection change took focus from the
        // outgoing child (see `switch_away`).
        if self.needs_ime_clear {
            ctx.publish_ime_state(cleared_ime_state());
            self.needs_ime_clear = false;
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A shortcut is the carve-out: only the selected child may answer one.
        if matches!(event, InputEvent::Shortcut(_)) {
            if let Some(pod) = self.selected_mut() {
                crate::authoring::route_event_single(pod, ctx, event);
            }
            return EventResult::Ignored;
        }
        if event.is_broadcast() {
            // Not user input: every child gets it, nothing consumes it (see
            // the `IndexedStack` docs).
            for pod in &mut self.children {
                crate::authoring::route_event_single(pod, ctx, event);
            }
            return EventResult::Ignored;
        }
        match self.selected_mut() {
            Some(pod) => crate::authoring::route_event_single(pod, ctx, event),
            None => EventResult::Ignored,
        }
    }

    /// Forwards the selected child only. This is the input-gating carve-out of
    /// the forward-every-child rule: the accessibility tree follows input
    /// routing (R23), and only the selected child receives input.
    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if let Some(pod) = self.selected() {
            pod.semantics_child(ctx);
        }
    }

    // Every retained child, selected or not: inspection shows what the tree
    // holds, not what input reaches.
    crate::authoring::visit_children!(children);

    /// The selected child only: Tab traversal follows input routing.
    fn visit_focus_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        if let Some(pod) = self.selected() {
            visitor(pod);
        }
    }
}

/// The cleared/inactive IME surface published after a selection change so the
/// platform keyboard hides deterministically. A copy of the navigator's and the
/// pattern switcher's private helper of the same name; `active: false` makes it
/// a focus/IME session release at the root.
fn cleared_ime_state() -> ImeState {
    ImeState {
        active: false,
        editing: EditingState {
            text: String::new(),
            selection_base: -1,
            selection_extent: -1,
            composing_base: -1,
            composing_extent: -1,
        },
        caret: None,
        content_type: Default::default(),
        suppress_soft_keyboard: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::navigator::{NavigatorController, NavigatorView, navigator};
    use crate::test_support::{RecordingScene, leaf, probe, swap_leaf};
    use frust_core::{PointerButton, PointerEvent, PointerPhase, RenderRoot};

    fn build<S: 'static>(view: &IndexedStackView<S>) -> IndexedStackWidget {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    fn rebuild<S: 'static>(
        next: &IndexedStackView<S>,
        prev: &IndexedStackView<S>,
        w: &mut IndexedStackWidget,
    ) -> ChangeFlags {
        let mut counter = 100u64;
        next.rebuild(prev, w, &mut BuildCtx::new(&mut counter))
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn down(x: f64, y: f64) -> InputEvent {
        pointer(PointerPhase::Down, x, y)
    }

    fn reach_of(w: &IndexedStackWidget) -> Vec<bool> {
        w.reach.iter().map(|c| c.get()).collect()
    }

    // --- (a) layout: largest child; paint: selected child only ---

    #[test]
    fn sizes_to_the_largest_child_and_paints_only_the_selected_one() {
        let view: IndexedStackView<()> = indexed_stack(Some(1))
            .child(leaf(30.0, 60.0))
            .child(leaf(80.0, 20.0));
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(500.0, 500.0)),
        );
        // Widest is the selected child (80), tallest the unselected one (60):
        // every child is laid out, not just the selected one.
        assert_eq!(size, Size::new(80.0, 60.0));
        assert_eq!(w.children[0].origin(), Point::ZERO);
        assert_eq!(w.children[1].origin(), Point::ZERO);

        let mut scene = RecordingScene::default();
        w.paint(&mut PaintCtx::new(Point::ZERO, size), &mut scene);
        assert_eq!(
            scene.rects,
            vec![(Point::ZERO, Size::new(80.0, 20.0))],
            "only the selected child paints"
        );
    }

    #[test]
    fn a_filling_child_fills_the_stack_under_tight_constraints() {
        let view: IndexedStackView<Vec<u32>> =
            indexed_stack(Some(0)).child(probe(0)).child(probe(1));
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::tight(Size::new(100.0, 70.0)),
        );
        assert_eq!(size, Size::new(100.0, 70.0));
        assert_eq!(w.children[0].size(), Size::new(100.0, 70.0));
        assert_eq!(w.children[1].size(), Size::new(100.0, 70.0));
    }

    // --- (b) input: only the selected child ---

    #[test]
    fn a_pointer_down_reaches_only_the_selected_child() {
        // Selecting the BOTTOM child proves selection, not z-order, decides.
        let view: IndexedStackView<Vec<u32>> =
            IndexedStack(Some(0), (probe(0), probe(1), probe(2)));
        let mut w = build(&view);
        w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::tight(Size::new(100.0, 100.0)),
        );
        let mut log: Vec<u32> = Vec::new();
        let mut ectx = EventCtx::new(&mut log, Point::ZERO, Size::new(100.0, 100.0));
        let result = w.event(&mut ectx, &down(50.0, 50.0));
        assert_eq!(result, EventResult::Handled);
        assert_eq!(log, vec![0], "only the selected child saw the Down");
    }

    #[test]
    fn a_broadcast_reaches_every_child_and_is_not_consumed() {
        let view: IndexedStackView<Vec<u32>> = IndexedStack(Some(1), (probe(0), probe(1)));
        let mut w = build(&view);
        w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::tight(Size::new(100.0, 100.0)),
        );
        let mut log: Vec<u32> = Vec::new();
        let mut ectx = EventCtx::new(&mut log, Point::ZERO, Size::new(100.0, 100.0));
        let result = w.event(&mut ectx, &InputEvent::Housekeeping);
        assert_eq!(result, EventResult::Ignored);
        assert_eq!(
            log,
            vec![0, 1],
            "housekeeping is not input: every tab gets it"
        );
    }

    #[test]
    fn a_shortcut_reaches_only_the_selected_child() {
        let view: IndexedStackView<Vec<u32>> = IndexedStack(Some(1), (probe(0), probe(1)));
        let mut w = build(&view);
        w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::tight(Size::new(100.0, 100.0)),
        );
        let mut log: Vec<u32> = Vec::new();
        let mut ectx = EventCtx::new(&mut log, Point::ZERO, Size::new(100.0, 100.0));
        let shortcut = InputEvent::Shortcut(frust_core::ShortcutId::next());
        assert_eq!(w.event(&mut ectx, &shortcut), EventResult::Ignored);
        assert_eq!(log, vec![1], "an unselected tab never answers a shortcut");
    }

    // --- (c) semantics: only the selected child ---

    struct Labelled {
        label: &'static str,
    }
    struct LabelledWidget {
        label: &'static str,
    }
    impl View<()> for Labelled {
        type Element = LabelledWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> LabelledWidget {
            LabelledWidget { label: self.label }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut LabelledWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.label = self.label;
            ChangeFlags::NONE
        }
    }
    impl Widget for LabelledWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn semantics(&self, ctx: &mut SemanticsCtx) {
            ctx.push_node(frust_core::accesskit::Role::Button, |node| {
                node.set_label(self.label)
            });
        }
    }

    fn labels(root: &RenderRoot<(), IndexedStackView<()>>) -> Vec<String> {
        root.semantics()
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label().map(str::to_string))
            .collect()
    }

    #[test]
    fn semantics_expose_only_the_selected_child() {
        let selected = Rc::new(Cell::new(Some(1usize)));
        let mut app = {
            let selected = selected.clone();
            move |_: &mut ()| {
                IndexedStack(
                    selected.get(),
                    (
                        Labelled { label: "tab-a" },
                        Labelled { label: "tab-b" },
                        Labelled { label: "tab-c" },
                    ),
                )
            }
        };
        let mut root: RenderRoot<(), IndexedStackView<()>> = RenderRoot::new();
        root.rebuild(&mut app, &mut ());
        root.layout(Size::new(100.0, 100.0));
        assert_eq!(labels(&root), vec!["tab-b".to_string()]);

        selected.set(Some(2));
        root.rebuild(&mut app, &mut ());
        root.layout(Size::new(100.0, 100.0));
        assert_eq!(labels(&root), vec!["tab-c".to_string()]);

        selected.set(None);
        root.rebuild(&mut app, &mut ());
        root.layout(Size::new(100.0, 100.0));
        assert!(labels(&root).is_empty(), "no selection, no nodes");
    }

    // --- (d) back reach: one navigator per tab ---

    #[test]
    fn only_the_selected_tabs_navigator_claims_back() {
        let a: NavigatorController<()> = NavigatorController::new();
        let b: NavigatorController<()> = NavigatorController::new();
        let selected = Rc::new(Cell::new(0usize));
        let mut app = {
            let (a, b, selected) = (a.clone(), b.clone(), selected.clone());
            move |_: &mut ()| {
                indexed_stack(Some(selected.get()))
                    .child(navigator(&a, || leaf(10.0, 10.0)))
                    .child(navigator(&b, || leaf(10.0, 10.0)))
            }
        };
        let mut root: RenderRoot<(), IndexedStackView<()>> = RenderRoot::new();
        root.rebuild(&mut app, &mut ());
        a.push(|| leaf(20.0, 20.0));
        b.push(|| leaf(20.0, 20.0));
        root.rebuild(&mut app, &mut ());
        assert_eq!((a.depth(), b.depth()), (2, 2), "both stacks were pushed");
        assert!(a.back_interest(), "the selected tab claims back");
        assert!(!b.back_interest(), "the unselected tab claims nothing");

        selected.set(1);
        root.rebuild(&mut app, &mut ());
        assert!(!a.back_interest(), "switched away: tab A claims nothing");
        assert!(b.back_interest(), "switched to: tab B claims back");
        assert!(a.is_mounted() && b.is_mounted(), "both stay mounted");
        assert_eq!((a.depth(), b.depth()), (2, 2), "both keep their stacks");
    }

    // --- (e) two-deep: tab -> navigator -> page -> navigator ---

    #[test]
    fn a_navigator_two_levels_down_claims_back_only_while_its_tab_is_selected() {
        let outer: NavigatorController<()> = NavigatorController::new();
        let inner: NavigatorController<()> = NavigatorController::new();
        let selected = Rc::new(Cell::new(0usize));
        let mut app = {
            let (outer, inner, selected) = (outer.clone(), inner.clone(), selected.clone());
            move |_: &mut ()| {
                let inner = inner.clone();
                indexed_stack(Some(selected.get()))
                    .child(navigator(&outer, move || {
                        navigator(&inner, || leaf(10.0, 10.0))
                    }))
                    .child(leaf(10.0, 10.0))
            }
        };
        let mut root: RenderRoot<(), IndexedStackView<()>> = RenderRoot::new();
        root.rebuild(&mut app, &mut ());
        inner.push(|| leaf(20.0, 20.0));
        root.rebuild(&mut app, &mut ());
        assert_eq!(inner.depth(), 2);
        assert!(
            inner.back_interest(),
            "selected tab: the inner stack claims"
        );

        selected.set(1);
        root.rebuild(&mut app, &mut ());
        assert!(!inner.back_interest(), "switched away: nothing claims");
        assert!(!outer.back_interest());
        // Stays silent across further passes (no stale republish).
        root.rebuild(&mut app, &mut ());
        assert!(!inner.back_interest(), "still switched away");

        selected.set(0);
        root.rebuild(&mut app, &mut ());
        assert!(
            inner.back_interest(),
            "switched back: the inner stack claims"
        );
        assert_eq!(inner.depth(), 2, "and kept its stack throughout");
    }

    // --- (f) composition: a stack on a navigator page that gets covered ---

    #[test]
    fn a_covered_host_page_silences_every_tab_navigator() {
        for cull in [false, true] {
            let host: NavigatorController<()> = NavigatorController::new();
            let t0: NavigatorController<()> = NavigatorController::new();
            let t1: NavigatorController<()> = NavigatorController::new();
            let mut app = {
                let (host, t0, t1) = (host.clone(), t0.clone(), t1.clone());
                move |_: &mut ()| {
                    let (t0, t1) = (t0.clone(), t1.clone());
                    navigator(&host, move || {
                        indexed_stack(Some(0))
                            .child(navigator(&t0, || leaf(10.0, 10.0)))
                            .child(navigator(&t1, || leaf(10.0, 10.0)))
                    })
                    .cull_covered_builds(cull)
                }
            };
            let mut root: RenderRoot<(), NavigatorView<()>> = RenderRoot::new();
            root.rebuild(&mut app, &mut ());
            t0.push(|| leaf(20.0, 20.0));
            t1.push(|| leaf(20.0, 20.0));
            root.rebuild(&mut app, &mut ());
            assert!(t0.back_interest(), "selected tab claims (cull={cull})");
            assert!(!t1.back_interest(), "unselected tab silent (cull={cull})");

            host.push(|| leaf(30.0, 30.0));
            root.rebuild(&mut app, &mut ());
            assert!(!t0.back_interest(), "covered: tab 0 silent (cull={cull})");
            assert!(!t1.back_interest(), "covered: tab 1 silent (cull={cull})");
            assert!(
                host.back_interest(),
                "the host claims instead (cull={cull})"
            );
            for _ in 0..3 {
                root.rebuild(&mut app, &mut ());
            }
            assert!(
                !t0.back_interest(),
                "stays silent while covered (cull={cull})"
            );

            host.pop();
            root.rebuild(&mut app, &mut ());
            assert!(t0.back_interest(), "revealed: tab 0 claims (cull={cull})");
            assert!(!t1.back_interest(), "tab 1 still silent (cull={cull})");
        }
    }

    // --- (g) no valid selection ---

    #[test]
    fn no_or_out_of_range_index_selects_nothing() {
        for index in [None, Some(2), Some(99)] {
            let view: IndexedStackView<Vec<u32>> = IndexedStack(index, (probe(0), probe(1)));
            let mut w = build(&view);
            assert_eq!(reach_of(&w), vec![false, false], "{index:?}: no reach");
            let size = w.layout(
                &mut LayoutCtx::new(),
                &BoxConstraints::tight(Size::new(50.0, 50.0)),
            );
            assert_eq!(size, Size::new(50.0, 50.0), "children are still laid out");

            let mut log: Vec<u32> = Vec::new();
            let mut ectx = EventCtx::new(&mut log, Point::ZERO, size);
            assert_eq!(w.event(&mut ectx, &down(5.0, 5.0)), EventResult::Ignored);
            assert!(log.is_empty(), "{index:?}: nothing routed");
        }

        let view: IndexedStackView<()> = indexed_stack(None)
            .child(leaf(10.0, 10.0))
            .child(leaf(20.0, 20.0));
        let mut w = build(&view);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(100.0, 100.0)),
        );
        let mut scene = RecordingScene::default();
        w.paint(&mut PaintCtx::new(Point::ZERO, size), &mut scene);
        assert!(scene.rects.is_empty(), "nothing painted");

        // A tab navigator under no selection claims nothing either.
        let nav: NavigatorController<()> = NavigatorController::new();
        let mut app = {
            let nav = nav.clone();
            move |_: &mut ()| indexed_stack(None).child(navigator(&nav, || leaf(10.0, 10.0)))
        };
        let mut root: RenderRoot<(), IndexedStackView<()>> = RenderRoot::new();
        root.rebuild(&mut app, &mut ());
        nav.push(|| leaf(20.0, 20.0));
        root.rebuild(&mut app, &mut ());
        assert_eq!(nav.depth(), 2);
        assert!(!nav.back_interest(), "no selection: no back reach");
    }

    #[test]
    fn reach_cells_follow_child_count_changes() {
        let v1: IndexedStackView<()> = indexed_stack(Some(1)).child(leaf(1.0, 1.0));
        let mut w = build(&v1);
        assert_eq!(
            reach_of(&w),
            vec![false],
            "index past the end selects nothing"
        );

        // Growing into the selected index makes the new child reachable.
        let v2: IndexedStackView<()> = indexed_stack(Some(1))
            .child(leaf(1.0, 1.0))
            .child(leaf(2.0, 2.0))
            .child(leaf(3.0, 3.0));
        let flags = rebuild(&v2, &v1, &mut w);
        assert!(flags.contains(ChangeFlags::LAYOUT | ChangeFlags::PAINT));
        assert_eq!(w.children.len(), 3);
        assert_eq!(reach_of(&w), vec![false, true, false]);

        // Shrinking drops the tail's cells; the dropped cell reads false.
        let dropped = Rc::clone(&w.reach[2]);
        let v3: IndexedStackView<()> = indexed_stack(Some(1))
            .child(leaf(1.0, 1.0))
            .child(leaf(2.0, 2.0));
        let flags = rebuild(&v3, &v2, &mut w);
        assert!(flags.contains(ChangeFlags::LAYOUT | ChangeFlags::PAINT));
        assert_eq!(w.children.len(), 2);
        assert_eq!(reach_of(&w), vec![false, true]);
        assert!(!dropped.get());

        // A plain rebuild with nothing changed reports nothing.
        let v4: IndexedStackView<()> = indexed_stack(Some(1))
            .child(leaf(1.0, 1.0))
            .child(leaf(2.0, 2.0));
        assert_eq!(rebuild(&v4, &v3, &mut w), ChangeFlags::NONE);

        // An index change alone reports layout + paint.
        let v5: IndexedStackView<()> = indexed_stack(Some(0))
            .child(leaf(1.0, 1.0))
            .child(leaf(2.0, 2.0));
        let flags = rebuild(&v5, &v4, &mut w);
        assert!(flags.contains(ChangeFlags::LAYOUT | ChangeFlags::PAINT));
        assert_eq!(reach_of(&w), vec![true, false]);
    }

    // --- (h) page-switch contract ---

    /// Captures the pointer and takes focus on `Down`; records `Cancel` and
    /// `Up` into shared cells (a `Cancel` arm touches internal flags only).
    struct Recorder {
        cancelled: Rc<Cell<bool>>,
        ups: Rc<Cell<u32>>,
    }
    struct RecorderWidget {
        cancelled: Rc<Cell<bool>>,
        ups: Rc<Cell<u32>>,
    }
    impl View<()> for Recorder {
        type Element = RecorderWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> RecorderWidget {
            RecorderWidget {
                cancelled: self.cancelled.clone(),
                ups: self.ups.clone(),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut RecorderWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for RecorderWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                match p.phase {
                    PointerPhase::Down => {
                        ctx.capture_pointer();
                        ctx.request_focus();
                        return EventResult::Handled;
                    }
                    PointerPhase::Cancel => {
                        self.cancelled.set(true);
                        return EventResult::Handled;
                    }
                    PointerPhase::Up => {
                        self.ups.set(self.ups.get() + 1);
                        return EventResult::Handled;
                    }
                    _ => {}
                }
            }
            EventResult::Ignored
        }
    }

    struct Armed {
        w: IndexedStackWidget,
        v1: IndexedStackView<()>,
        cancelled: Rc<Cell<bool>>,
        ups: Rc<Cell<u32>>,
        size: Size,
    }

    /// Tab 0 is a recorder, selected and armed (captured + focused) by a Down.
    fn armed() -> Armed {
        let cancelled = Rc::new(Cell::new(false));
        let ups = Rc::new(Cell::new(0u32));
        let v1: IndexedStackView<()> = indexed_stack(Some(0))
            .child(Recorder {
                cancelled: cancelled.clone(),
                ups: ups.clone(),
            })
            .child(leaf(10.0, 10.0));
        let mut w = build(&v1);
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::tight(Size::new(100.0, 100.0)),
        );
        let mut st = ();
        let mut ectx = EventCtx::new(&mut st, Point::ZERO, size);
        w.event(&mut ectx, &down(5.0, 5.0));
        assert!(w.children[0].is_active(), "tab 0 captured the pointer");
        assert!(w.children[0].is_focused(), "tab 0 took focus");
        Armed {
            w,
            v1,
            cancelled,
            ups,
            size,
        }
    }

    fn switch_view() -> IndexedStackView<()> {
        // Same child types as `armed`'s view; only the index differs.
        indexed_stack(Some(1))
            .child(Recorder {
                cancelled: Rc::new(Cell::new(false)),
                ups: Rc::new(Cell::new(0)),
            })
            .child(leaf(10.0, 10.0))
    }

    #[test]
    fn a_selection_change_cancels_capture_clears_focus_and_clears_the_ime() {
        let Armed {
            mut w,
            v1,
            cancelled,
            ups,
            size,
        } = armed();
        let v2 = switch_view();
        rebuild(&v2, &v1, &mut w);

        assert!(
            cancelled.get(),
            "the outgoing tab received a synthetic Cancel"
        );
        assert!(!w.children[0].is_active(), "its capture path was released");
        assert!(!w.children[0].is_focused(), "its focus flag was cleared");
        assert!(w.needs_ime_clear, "a cleared IME publish is queued");

        // A later Up never reaches the outgoing tab.
        let mut st = ();
        let mut ectx = EventCtx::new(&mut st, Point::ZERO, size);
        w.event(&mut ectx, &pointer(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(ups.get(), 0, "the cancelled tab does not fire on Up");

        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut RecordingScene::default());
        let ime = pctx
            .take_ime_state()
            .expect("the post-switch paint publishes a cleared IME surface");
        assert!(!ime.active);
        assert!(!w.needs_ime_clear, "the queued clear was consumed");

        // The incoming tab is not focused automatically.
        assert!(!w.children[1].is_focused());
    }

    #[test]
    fn a_selection_change_off_the_live_focus_chain_publishes_no_ime_clear() {
        let Armed {
            mut w,
            v1,
            cancelled,
            size,
            ..
        } = armed();
        let v2 = switch_view();
        let mut counter = 100u64;
        let mut ctx = BuildCtx::new(&mut counter);
        // The session lives elsewhere: the chain down to this stack is broken.
        ctx.with_focus_link(false, |ctx| v2.rebuild(&v1, &mut w, ctx));

        assert!(cancelled.get(), "capture is still cancelled");
        assert!(
            !w.children[0].is_focused(),
            "the stale flag is still cleared"
        );
        assert!(!w.needs_ime_clear, "but no session release is queued");
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut RecordingScene::default());
        assert!(pctx.take_ime_state().is_none(), "no IME publish");
    }

    #[test]
    fn switching_away_from_an_unfocused_tab_publishes_no_ime_clear() {
        let v1: IndexedStackView<()> = indexed_stack(Some(0))
            .child(leaf(10.0, 10.0))
            .child(leaf(10.0, 10.0));
        let mut w = build(&v1);
        let v2: IndexedStackView<()> = indexed_stack(Some(1))
            .child(leaf(10.0, 10.0))
            .child(leaf(10.0, 10.0));
        rebuild(&v2, &v1, &mut w);
        assert!(!w.needs_ime_clear, "the outgoing tab held no session");
    }

    // --- (i) builders ---

    #[test]
    fn fluent_and_tuple_builders_build() {
        let fluent: IndexedStackView<()> = indexed_stack(Some(0))
            .child(leaf(1.0, 1.0))
            .children((swap_leaf(), leaf(2.0, 2.0)))
            .when(true, |s| s.child(leaf(3.0, 3.0)))
            .when(false, |s| s.child(leaf(4.0, 4.0)))
            .when_some(Some(5.0), |s, side| s.child(leaf(side, side)))
            .when_some(None::<f64>, |s, side| s.child(leaf(side, side)));
        assert_eq!(build(&fluent).children.len(), 5);

        let tuple: IndexedStackView<()> =
            IndexedStack(Some(1), (leaf(1.0, 1.0), swap_leaf(), leaf(2.0, 2.0)));
        let w = build(&tuple);
        assert_eq!(w.children.len(), 3);
        assert_eq!(reach_of(&w), vec![false, true, false]);
    }

    #[test]
    fn traversal_enters_only_the_selected_child() {
        let view: IndexedStackView<Vec<u32>> =
            IndexedStack(Some(1), (probe(0), probe(1), probe(2)));
        let w = build(&view);
        let mut all = 0;
        w.visit_children(&mut |_| all += 1);
        let mut entered: Vec<*const ChildPod> = Vec::new();
        w.visit_focus_children(&mut |pod| entered.push(pod));
        assert_eq!(all, 3, "inspection still sees every retained child");
        assert_eq!(entered, vec![&w.children[1] as *const ChildPod]);

        let none: IndexedStackView<Vec<u32>> = IndexedStack(None, (probe(0),));
        let mut entered = 0;
        build(&none).visit_focus_children(&mut |_| entered += 1);
        assert_eq!(entered, 0, "nothing selected, nothing to traverse");
    }
}
