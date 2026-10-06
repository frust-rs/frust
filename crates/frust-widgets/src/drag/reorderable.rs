//! [`reorderable_list()`]: a drag-to-reorder list built entirely from the
//! existing session primitives — a composition, not a new layout algorithm.
//!
//! # Shape
//!
//! `N` rows produce `N + 1` thin gap targets, one before the first row and
//! one after every row, interleaved into a single [`crate::FlexView`] column
//! (`gap, row, gap, row, …, row, gap`) built once per call. Each row is
//! wrapped in [`super::draggable()`] carrying its own index (into the list as
//! given to this call) as the payload; each gap is a
//! [`super::drag_target::<usize, _, _>()`] over that same index type, so a
//! row and every gap share one coordinator session with no type mismatch.
//! Both halves are the unmodified widgets the rest of the module ships — the
//! ghost is [`super::draggable()`]'s default child snapshot, the keyboard
//! lift/cycle/drop chord is answered by the row's own focus handling, and
//! cycling walks the gaps in registration order, which is this column's
//! build order, which is visual order (see the [module docs](super#target-resolution)
//! and [`mod@super::draggable`]'s *Lift, cycle, drop*). Every row keeps the
//! [`crate::ChildKey`] it was given, so [`crate::keyed`] reconciliation
//! carries each row's retained state — scroll position, focus, whatever a
//! custom row builds — through a reorder instead of reattaching it to
//! whatever index now sits there.
//!
//! # Index convention
//!
//! A gap's own position, `g`, is its index among the `N + 1` gaps in
//! registration/visual order (`0` is before the first row). Dropping a
//! session carrying source index `i` onto gap `g` reports
//! [`ReorderableListView::on_reorder`]`(state, i, to)` where `to` is `g`
//! adjusted for the source's own removal — the index `i` would land at in
//! the list *after* it is taken out, which is what a caller applying the
//! move to its own backing storage (a `Vec::remove` + `Vec::insert` pair)
//! wants directly:
//!
//! * `g <= i` — nothing between the gap and the list start shifts when `i`
//!   is removed, so `to = g`.
//! * `g > i` — removing `i` shifts everything after it down by one, so
//!   `to = g - 1`.
//!
//! The two gaps touching the source itself — immediately before it (`g ==
//! i`) and immediately after it (`g == i + 1`) — both resolve to `to == i`,
//! the source's own current position: dropping there is a no-op and
//! [`ReorderableListView::on_reorder`] does not fire, matching the drop
//! target's own cancel-on-reject precedent of never completing a session
//! that changed nothing. Every other gap reports a real move.
//!
//! # Limits
//!
//! * Vertical only: this helper always builds a vertical column. A
//!   horizontal reorderable row is out of scope here — compose
//!   [`super::draggable()`]/[`super::drag_target()`] directly for that, the
//!   way this module does internally.
//! * The gap's own highlight is [`super::target::DragTargetView`]'s default
//!   (a themed inset stroke) rather than a bespoke insertion-line treatment;
//!   override per gap is not exposed — a design system wanting its own gap
//!   affordance composes the two primitives directly instead of this helper.
//! * [`ReorderableListView::auto_scroll`] builds its own [`crate::ScrollView`]
//!   bound to the controller it is given — the list itself becomes the
//!   scroll surface, not something a caller wraps around it. Bound the
//!   result's height from outside (a fixed-size ancestor, the way every
//!   other scroll surface in this crate needs one) to get a real viewport to
//!   scroll within; do not also put the list inside a separate
//!   `scroll_view(...)` bound to the same controller, which would give one
//!   handle two competing surfaces.
//! * Row identity during a session: each row's drag payload is the plain
//!   index [`reorderable_list()`] captured when it built the column, not a
//!   stable id. A caller that mutates `items`' order or length while a
//!   session on this list's coordinator is in flight can have
//!   [`ReorderableListView::on_reorder`] fire against a pair computed for a
//!   list that no longer matches what is on screen — this helper does not
//!   guard against it. Keep the list's order and length fixed for the
//!   duration of any session on its coordinator.

use std::cell::RefCell;
use std::rc::Rc;

use frust_core::{AnyView, BuildCtx, ChangeFlags, View, Widget, any};

use super::auto_scroll::auto_scroll_zone;
use super::coordinator::DragCoordinator;
use super::draggable::draggable;
use super::target::drag_target;
use crate::flex::{Axis, CrossAxisAlignment, FlexChild, FlexView};
use crate::{ChildKey, ScrollController, SizedBox, keyed, scroll_view};

/// Thickness, in logical px, of the drop-target band between two rows (and
/// before the first/after the last).
///
/// **Community-approximate**: thin enough not to read as a row of its own,
/// wide enough to land a long-press finger on; no platform publishes a
/// reorder gap metric.
const GAP_PX: f64 = 8.0;

/// A reorder completion callback: the source's original index and the index
/// it lands at after the move (see the [module docs](self#index-convention)).
type ReorderCallback<State> = Rc<dyn Fn(&mut State, usize, usize)>;

/// The cell every gap's `on_drop` reads from and
/// [`ReorderableListView::on_reorder`] writes to — shared so the callback
/// can be attached after the column (which needs no payload-adjustment logic
/// of its own) is already built.
type ReorderCell<State> = Rc<RefCell<Option<ReorderCallback<State>>>>;

/// A declarative drag-to-reorder list. See the [module docs](self).
pub struct ReorderableListView<State: 'static> {
    inner: AnyView<State>,
    coordinator: DragCoordinator,
    on_reorder: ReorderCell<State>,
}

/// Build a reorderable list over `items`, each an `(identity, content)` pair
/// — the same [`crate::ChildKey`] a [`crate::keyed`] [`crate::Column`] row
/// would carry. See the [module docs](self) for the composition and the
/// index convention [`ReorderableListView::on_reorder`] reports through.
///
/// `items` is any iterable of one `(key, view)` pair type; each view is
/// erased here, so a homogeneous list of rows needs no `any()`.
///
/// ```
/// use frust_widgets::{ChildKey, DragCoordinator, ReorderableListView, reorderable_list, text};
/// # fn demo(coordinator: DragCoordinator) -> ReorderableListView<()> {
/// let names = ["alpha", "beta", "gamma"];
/// reorderable_list(coordinator, names.map(|name| (ChildKey::new(name), text(name))))
/// # }
/// # let _ = demo;
/// ```
pub fn reorderable_list<State, K, V>(
    coordinator: DragCoordinator,
    items: impl IntoIterator<Item = (K, V)>,
) -> ReorderableListView<State>
where
    State: 'static,
    K: Into<ChildKey>,
    V: View<State>,
{
    let on_reorder: ReorderCell<State> = Rc::new(RefCell::new(None));
    let items = items.into_iter();
    let count = items.size_hint().0;
    let mut children: Vec<FlexChild<State>> = Vec::with_capacity(count * 2 + 1);
    children.push(gap_child(0, &coordinator, &on_reorder));
    for (index, (key, view)) in items.enumerate() {
        let key: ChildKey = key.into();
        let row = draggable(view, coordinator.clone(), move |_: &State| index);
        children.push(keyed(key, row));
        children.push(gap_child(index + 1, &coordinator, &on_reorder));
    }
    let column = FlexView::new(Axis::Vertical, children).cross_axis(CrossAxisAlignment::Stretch);
    ReorderableListView {
        inner: any(column),
        coordinator,
        on_reorder,
    }
}

/// Build the gap target at position `gap_index` (see the [module
/// docs](self#index-convention)), keyed by its own position so the
/// reconciler matches it across rebuilds (the gap count never changes
/// independently of the row count, so this stays stable across reorders).
fn gap_child<State: 'static>(
    gap_index: usize,
    coordinator: &DragCoordinator,
    on_reorder: &ReorderCell<State>,
) -> FlexChild<State> {
    let on_reorder = Rc::clone(on_reorder);
    let target =
        drag_target::<usize, State, _>(SizedBox::<State>(None, Some(GAP_PX)), coordinator.clone())
            .on_drop(move |state: &mut State, from: usize| {
                let to = if gap_index <= from {
                    gap_index
                } else {
                    gap_index - 1
                };
                if to == from {
                    // The gap immediately before or after the source: dropping
                    // there is the source's own current slot.
                    return;
                }
                let callback = on_reorder.borrow().clone();
                if let Some(callback) = callback {
                    callback(state, from, to);
                }
            });
    keyed(gap_key(gap_index), target)
}

/// A stable identity for the gap at `gap_index`, distinct from any row key a
/// caller plausibly hashes from its own data (an id, a string name, a plain
/// index) by the string tag folded into the hash.
fn gap_key(gap_index: usize) -> ChildKey {
    ChildKey::new(("frust-widgets-reorderable-gap", gap_index))
}

impl<State: 'static> ReorderableListView<State> {
    /// Run `f` against the app state when a row drops onto a gap that moves
    /// it — never for a drop back onto its own current slot (see the
    /// [module docs](self#index-convention)).
    pub fn on_reorder<F: Fn(&mut State, usize, usize) + 'static>(self, f: F) -> Self {
        *self.on_reorder.borrow_mut() = Some(Rc::new(f));
        self
    }

    /// Wrap the list in its own scroll surface bound to `controller`, then
    /// enclose *that* surface in edge auto-scroll for sessions on this
    /// list's coordinator — so the zone's edge bands measure the surface's
    /// own (bounded) viewport, not the list's full, usually taller, content.
    /// See [`super::auto_scroll_zone()`] and the [module docs](self#limits)
    /// for the contract this puts on the caller: bound this view's height
    /// from outside (a fixed-size ancestor), and do not also wrap it in a
    /// second scroll surface bound to the same controller.
    pub fn auto_scroll(mut self, controller: ScrollController) -> Self {
        let surface = scroll_view(self.inner).controller(controller.clone());
        self.inner = any(auto_scroll_zone(
            surface,
            self.coordinator.clone(),
            controller,
        ));
        self
    }
}

impl<State: 'static> View<State> for ReorderableListView<State> {
    type Element = Box<dyn Widget>;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        self.inner.build(ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.inner.rebuild(&prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        self.inner.teardown(element, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drag::DragPhase;
    use frust_core::{FrameTime, InputEvent, Key, KeyEvent, Modifiers, NamedKey, RenderRoot};
    use kurbo::{Point, Size};
    use std::cell::RefCell as StdRefCell;

    const WINDOW: Size = Size::new(400.0, 600.0);
    const ROW_PX: f64 = 40.0;

    type Log = Rc<StdRefCell<Vec<(usize, usize)>>>;

    struct App {
        coordinator: DragCoordinator,
        reorders: Log,
    }

    fn build_view(state: &mut App) -> ReorderableListView<App> {
        let items: Vec<(ChildKey, AnyView<App>)> = (0..3u32)
            .map(|id| {
                (
                    ChildKey::new(id),
                    any(SizedBox::<App>(Some(WINDOW.width), Some(ROW_PX))),
                )
            })
            .collect();
        let log = Rc::clone(&state.reorders);
        reorderable_list(state.coordinator.clone(), items)
            .on_reorder(move |_: &mut App, from, to| log.borrow_mut().push((from, to)))
    }

    struct Harness {
        root: RenderRoot<App, ReorderableListView<App>>,
        state: App,
        clock_ms: f64,
    }

    impl Harness {
        fn new() -> Self {
            let mut harness = Harness {
                root: RenderRoot::new(),
                state: App {
                    coordinator: DragCoordinator::new(),
                    reorders: Rc::new(StdRefCell::new(Vec::new())),
                },
                clock_ms: 0.0,
            };
            harness.frame();
            harness
        }

        fn frame(&mut self) -> crate::test_support::RecordingScene {
            self.root.rebuild(&mut build_view, &mut self.state);
            self.root.layout(WINDOW);
            self.clock_ms += 16.0;
            let mut scene = crate::test_support::RecordingScene::default();
            self.root.paint(
                &mut scene,
                FrameTime::from_nanos((self.clock_ms * 1_000_000.0) as u64),
            );
            scene
        }

        fn mouse(&mut self, phase: frust_core::PointerPhase, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(frust_core::PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: frust_core::PointerButton::Primary,
                }),
            );
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

        fn phase(&self) -> DragPhase {
            self.state.coordinator.phase()
        }

        fn reorders(&self) -> Vec<(usize, usize)> {
            self.state.reorders.borrow().clone()
        }
    }

    // Row `i` occupies `[8 + i*48, 48 + i*48)`; gap `g` occupies
    // `[g*48, g*48 + 8)` — see `GAP_PX`/`ROW_PX` above. Gap 0 is the band
    // `[0, 8)`, row 0 is `[8, 48)`, gap 1 is `[48, 56)`, and so on.
    fn gap_y(gap_index: usize) -> f64 {
        gap_index as f64 * (ROW_PX + GAP_PX) + GAP_PX / 2.0
    }

    fn row_y(row_index: usize) -> f64 {
        GAP_PX + row_index as f64 * (ROW_PX + GAP_PX) + ROW_PX / 2.0
    }

    #[test]
    fn dragging_row_0_past_row_2_reports_the_adjusted_index() {
        use frust_core::PointerPhase;
        let mut h = Harness::new();
        h.mouse(PointerPhase::Down, 50.0, row_y(0));
        // One move past row 2, into the gap right after it (gap 3).
        h.mouse(PointerPhase::Move, 50.0, gap_y(3));
        assert_eq!(h.phase(), DragPhase::Dragging);
        h.mouse(PointerPhase::Up, 50.0, gap_y(3));
        h.frame();
        assert_eq!(h.phase(), DragPhase::Idle, "the target claimed the drop");
        assert_eq!(h.reorders(), vec![(0, 2)]);
    }

    #[test]
    fn dropping_on_the_gap_touching_the_source_reports_nothing() {
        use frust_core::PointerPhase;
        let mut h = Harness::new();
        h.mouse(PointerPhase::Down, 50.0, row_y(0));
        // Gap 1 sits right after row 0 — the source's own current slot.
        h.mouse(PointerPhase::Move, 50.0, gap_y(1));
        h.mouse(PointerPhase::Up, 50.0, gap_y(1));
        h.frame();
        assert_eq!(h.phase(), DragPhase::Idle);
        assert_eq!(
            h.reorders(),
            Vec::new(),
            "dropping at the original slot is a no-op"
        );
    }

    #[test]
    fn the_gap_indicator_tracks_the_hovered_gap_only() {
        use frust_core::PointerPhase;
        let mut h = Harness::new();
        let idle = h.frame();
        assert_eq!(idle.transforms.len(), 0, "nothing hovered, nothing drawn");

        h.mouse(PointerPhase::Down, 50.0, row_y(0));
        h.mouse(PointerPhase::Move, 50.0, gap_y(2));
        let hovering_gap = h.frame();
        assert_eq!(
            hovering_gap.transforms.len(),
            1,
            "exactly the hovered gap paints its highlight"
        );

        h.mouse(PointerPhase::Move, 50.0, row_y(1));
        let hovering_row = h.frame();
        assert_eq!(
            hovering_row.transforms.len(),
            0,
            "no gap is hovered while the pointer sits over a row"
        );

        h.mouse(PointerPhase::Move, 50.0, gap_y(3));
        let hovering_again = h.frame();
        assert_eq!(
            hovering_again.transforms.len(),
            1,
            "a later gap takes over the indicator"
        );
    }

    #[test]
    fn auto_scroll_measures_the_bounded_viewport_not_the_full_content() {
        use crate::SizedBoxView;
        use frust_core::PointerPhase;

        // 20 rows plus 21 gaps is 968 px of content (20 * ROW_PX + 21 *
        // GAP_PX) — far taller than the 300 px viewport below. A zone that
        // (wrongly) measured the full scrolled content instead of the
        // viewport would need the pointer roughly 650 px further down than
        // this to ever see a non-zero velocity.
        const ROWS: u32 = 20;
        const VIEWPORT_PX: f64 = 300.0;

        struct ScrollApp {
            coordinator: DragCoordinator,
            controller: ScrollController,
        }

        fn build_scrolling_view(state: &mut ScrollApp) -> SizedBoxView<ScrollApp> {
            let items: Vec<(ChildKey, AnyView<ScrollApp>)> = (0..ROWS)
                .map(|id| {
                    (
                        ChildKey::new(id),
                        any(SizedBox::<ScrollApp>(Some(WINDOW.width), Some(ROW_PX))),
                    )
                })
                .collect();
            let list = reorderable_list(state.coordinator.clone(), items)
                .auto_scroll(state.controller.clone());
            SizedBox::<ScrollApp>(Some(WINDOW.width), Some(VIEWPORT_PX)).child(list)
        }

        struct ScrollHarness {
            root: RenderRoot<ScrollApp, SizedBoxView<ScrollApp>>,
            state: ScrollApp,
            clock_ms: f64,
        }

        impl ScrollHarness {
            fn new() -> Self {
                let mut harness = ScrollHarness {
                    root: RenderRoot::new(),
                    state: ScrollApp {
                        coordinator: DragCoordinator::new(),
                        controller: ScrollController::new(),
                    },
                    clock_ms: 0.0,
                };
                harness.frame();
                harness
            }

            fn frame(&mut self) {
                self.root
                    .rebuild(&mut build_scrolling_view, &mut self.state);
                self.root.layout(WINDOW);
                self.clock_ms += 16.0;
                let mut scene = crate::test_support::RecordingScene::default();
                self.root.paint(
                    &mut scene,
                    FrameTime::from_nanos((self.clock_ms * 1_000_000.0) as u64),
                );
            }

            fn mouse(&mut self, phase: PointerPhase, x: f64, y: f64) {
                self.root.event(
                    &mut self.state,
                    &InputEvent::Pointer(frust_core::PointerEvent {
                        phase,
                        position: Point::new(x, y),
                        button: frust_core::PointerButton::Primary,
                    }),
                );
            }
        }

        let mut h = ScrollHarness::new();
        assert!(
            h.state.controller.max_offset() > 0.0,
            "the content must be taller than the viewport for this to be a\
             meaningful test"
        );

        // Press row 0, take it past its own slop with a small move (so the
        // draggable — not the enclosing scroll surface's own gesture
        // recognition — claims the session), then carry it down to the
        // *viewport's* own bottom edge, well short of the list's full
        // content.
        h.mouse(PointerPhase::Down, 50.0, row_y(0));
        h.mouse(PointerPhase::Move, 50.0, row_y(0) + 8.0);
        assert_eq!(h.state.coordinator.phase(), DragPhase::Dragging);
        h.mouse(PointerPhase::Move, 50.0, VIEWPORT_PX - 5.0);
        assert_eq!(h.state.coordinator.phase(), DragPhase::Dragging);

        for _ in 0..4 {
            h.frame();
        }

        assert!(
            h.state.controller.offset() > 0.0,
            "a pointer at the viewport's own bottom edge must auto-scroll"
        );
    }

    #[test]
    fn a_keyboard_lift_cycle_and_drop_reports_the_adjusted_index() {
        use frust_core::PointerPhase;
        let mut h = Harness::new();
        // Focus row 1 with a plain tap — no drag, no pointer capture.
        h.mouse(PointerPhase::Down, 50.0, row_y(1));
        h.mouse(PointerPhase::Up, 50.0, row_y(1));
        assert!(h.root.is_focus_active());

        h.key(Key::Named(NamedKey::Enter));
        assert_eq!(
            h.phase(),
            DragPhase::Dragging,
            "Enter lifts the focused row"
        );

        // Cycle forward to gap 3: gap 0, 1, 2, then 3.
        for _ in 0..4 {
            h.key(Key::Named(NamedKey::ArrowDown));
        }
        h.key(Key::Named(NamedKey::Enter));
        h.frame();
        assert_eq!(h.phase(), DragPhase::Idle);
        assert_eq!(h.reorders(), vec![(1, 2)]);
    }
}
