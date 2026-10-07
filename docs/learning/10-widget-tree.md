# Lab 10 — How the Widget Tree Works (`frust-core` + `frust-widgets`' authoring kit)

**Concept:** Lab 2 made you a widget author; lab 3 watches a whole frame. In between sits the
machinery that decides *which* widget object survives from one frame to the next. It has three
layers — a throwaway `View` value, a retained `Widget` object, and a tree that owns the root —
and most of what surprises people (why the root is a `Box<dyn Widget>`, why a child can need two
downcasts, why `ChangeFlags::PAINT` skips nothing) follows from how those layers meet. Read it
after lab 2 and before lab 3.

## Where it lives

| Thing | File | Symbol ≈line |
|---|---|---|
| Declarative layer | `crates/frust-core/src/view.rs` | `View` ≈191, `ChangeFlags` ≈24 (`PAINT` ≈30, `LAYOUT` ≈32), `alloc_id` ≈127, `with_focus_link` ≈176 |
| Type erasure | `crates/frust-core/src/view.rs` | `ErasedView` ≈227 (blanket impl ≈248), `AnyView` ≈297 (its `View` impl ≈316) |
| Retained layer | `crates/frust-core/src/widget.rs` | `pub trait Widget: Any` ≈2015, `visit_children` ≈2091, `downcast_mut` ≈2099, `impl Widget for Box<dyn Widget>` ≈2114 |
| A container's child | `crates/frust-core/src/widget.rs` | `ChildPod` ≈2163, `holds_live_focus` ≈2561, `layout_child` ≈2624, `paint_child` ≈2637, `event_child` ≈2807 |
| The arena | `crates/frust-core/src/tree.rs` | `WidgetPod` ≈52, `new_typed` ≈90, `WidgetTree` ≈214, `inspect` ≈300 |
| The driver | `crates/frust-core/src/app.rs` | `RenderRoot` ≈292, `rebuild` ≈1189, `rebuild_view` ≈1334, `layout_with_text` ≈1398, `paint` ≈1469, `event` ≈2122 |
| Focus-orphan side channel | `crates/frust-core/src/event.rs` | `FOCUS_ORPHANED` ≈744, `mark_focus_orphaned` ≈776 |
| Reconcilers | `crates/frust-widgets/src/authoring.rs` | `build_child` ≈278, `rebuild_child` ≈308, `teardown_child` ≈393, `rebuild_children_positional` ≈635 |
| Routing + introspection | `crates/frust-widgets/src/authoring.rs` | `route_event` ≈978, `route_event_single` ≈1039, `VisitPods` ≈803, `macro_rules! visit_children` ≈870 |
| A real container | `crates/frust-widgets/src/flex.rs` | `FlexView` (its `rebuild` ≈271) |
| Components | `crates/frust-core/src/component.rs` | `Component` ≈56, `ComponentWidget` ≈109 |
| Overlay portal | `crates/frust-core/src/overlay.rs` | `OverlayEntry` ≈230, `register_overlay` (defined in widget.rs ≈1551) |
| The one layout-skip consumer | `crates/frust-shell-android/src/app/frame.rs` | `needs_layout` ≈422 |

## The five ideas

1. **Three layers, one direction of ownership.** A **View** is a `'static` value the root component's
   `build` re-creates every frame. `View<State>` has `type Element: Widget` plus `build` (make the widget),
   `rebuild(prev, element, ctx) -> ChangeFlags` (mutate the live widget to match), and `teardown`.
   Its `BuildCtx` does two jobs. `alloc_id` mints a `WidgetId`; the only production caller is the
   root's first build (`app.rs` ≈1367). It also threads the **focus chain**
   (`has_focus`/`set_has_focus`/`with_focus_link`), an AND of every focus link from the root down
   to the node being diffed. A **Widget** is the retained object:
   `layout`/`paint`/`event`/`semantics`, a `visit_children` that visits nothing by default, and
   `type_name`. The **Tree** is `WidgetTree`: a `tree_arena` 0.2.0 `TreeArena<WidgetPod>` plus
   insertion-order indices, because the arena iterates in `HashMap` order. It is
   **single-root on purpose**. As `ChildPod`'s doc puts it (`widget.rs` ≈2143–2162): *"This is a
   deliberate divergence from the masonry 'everything in the arena' model: it needs zero
   global-id plumbing and no disjoint-borrow gymnastics for the container features this crate
   ships."* Containers own their children as `Vec<ChildPod>` or named fields.

2. **Why the root is `Box<dyn Widget>`, and where double boxing comes from.** The arena slot and
   every pass driver work on `&mut dyn Widget`. `RenderRoot<State, V>` keeps `prev_view: Option<V>`
   typed. On the first build it stores the element with `WidgetPod::new_typed(id, element)` (≈1372),
   which records `core::any::type_name::<W>()`. `WidgetPod::new` takes an already-erased box and
   records `ERASED_TYPE_NAME` instead. On every rebuild, the root gets the concrete element back with
   `downcast_mut::<V::Element>()` (≈1350). That downcast works because of `Widget: Any`.
   `AnyView` uses the same trick one level down. `dyn_rebuild` downcasts the *previous view* to
   `V`. If that succeeds, it rebuilds in place. If not, it tears down through the old view, builds
   fresh, and returns `LAYOUT | PAINT`. `AnyView::Element` is `Box<dyn Widget>`, which is only
   legal because of the blanket `impl Widget for Box<dyn Widget>`: it forwards every pass *and*
   `type_name`. **The catch:** `build_child` (≈278) and `ComponentView::build` store that element
   as `ChildPod::new(Box::new(element))`, so the pod holds a `Box<Box<dyn Widget>>`. The outer box
   lets a rebuild recover `&mut Box<dyn Widget>`, the type `AnyView::rebuild` needs to *replace*
   the widget. The `new_typed` doc warns about the same trap: pass it an already-boxed widget and
   it double-boxes, which breaks the downcast. To reach a concrete child, you need **two hops**:

   ```rust
   // component.rs ≈915 `inner_widget`; authoring.rs `nest_in` and
   // plugins/beui/src/agents/chat_app.rs ≈625 `pod_widget` do the same two hops.
   w.child.widget_mut()
       .downcast_mut::<Box<dyn Widget>>().expect("double-boxed AnyView element")
       .downcast_mut::<W>().expect("inner concrete widget")
   ```

   `pod_widget` also accepts a single-boxed pod: it falls through when the first hop fails.
   Tooling never sees the extra box, because `type_name` resolves through both layers.

3. **`ChildPod` holds a container's per-child state.** It carries parent-relative `origin`/`size`,
   `active` (this child captured the pointer), `focused` + `focus_epoch`/`focus_root`, and
   `hover_epoch`/`hover_root`. It also has lazily assigned `semantics_id`/`inspect_id` `Cell`s and a
   `debug_label`. A set `focused` flag proves nothing by itself: `holds_live_focus` also requires
   the stamp to name the session the root has *now*. `paint_child` builds the child's `PaintCtx`
   at the **absolute** origin (`ctx.origin() + self.origin`) and passes down frame time, theme,
   insets, and the epochs. It composes the child's focus as
   `self.focused && self.focus_epoch == ctx.focus_epoch() && ctx.has_focus()` and bubbles frame
   requests up by tick class. `event_child` translates, dispatches, and *records*
   `active`/`focused` from what bubbles back. It does **not** route: its doc sends containers to
   `route_event`/`route_event_single`. In order, they send broadcasts (`InputEvent::Housekeeping`,
   `InputEvent::Overlay`) to every child, `Key`/`Ime` to the child that `holds_live_focus`, and a
   pointer to the `is_active` child with no hit test; only then do they hit-test topmost-first. A
   `Down` also runs the container-side blur sweep (`set_focused(false)`). `RenderRoot::event` only
   pre-routes overlays (`route_overlay` ≈2616), advances the focus epoch, and calls the root widget.

4. **`RenderRoot` drives, and `ChangeFlags` are a report, not a skip list.** `rebuild` calls
   the root component's `build` closure. The reactive `TrackedScope`/`Owner` wrap is the *shell's*
   (`crates/frust-shell-desktop/src/app_handler.rs`, `scope.track(|| root.rebuild` ≈2771). It diffs
   (idea 2), runs at most `MAX_PENDING_RESULT_FLUSH_PASSES` = 3 (≈163) `Housekeeping` flush/re-diff
   passes, drains `take_focus_orphaned` into `release_focus_session` (≈1274–1276), then ORs the
   flags into `pending`. `layout_with_text` passes down loose window `BoxConstraints`, a type-erased
   text context, the theme, and insets. `paint` paints the main tree, then the overlays. `inspect`
   (≈1151) returns an owned `InspectNode` snapshot.
   **The rule people get wrong:** a View rebuild cascades to **every child, every frame**.
   `rebuild_children_positional` rebuilds every common-index child whatever the flags say, and
   `docs/CORE_ARCHITECTURE.md` says "there is no per-component skipping". The flags are ORed upward
   purely as a **report**. Inside a run, the only pass they gate is the whole-tree
   `RenderRoot::layout`, and only in a shell that reads them — but flags left *undrained* by a
   rebuild are also read a second way, before that decision is even made: mobile's whole-frame gate
   takes pending flags as one of its Run/Skip inputs. `FrameInputs::change_flags_pending`
   (`crates/frust-shell-common/src/frame_gate.rs` ≈292, ORed into `any_set` ≈343) is filled from
   `RenderRoot::has_pending_change_flags()` (`crates/frust-core/src/app.rs` ≈1122) by both mobile
   shells (`crates/frust-shell-android/src/app/frame.rs` ≈250,
   `crates/frust-shell-ios/src/app/frame.rs` ≈203; lab 7 lists it among the gate inputs). So there
   are two consumers, not one: the frame gate reads pending flags on Android and iOS to decide
   whether the frame runs at all, and, inside a run that does happen, the layout skip reads the
   drained flags on Android only. The contract is rebuild → layout *iff*
   `take_change_flags().needs_layout()` (or first frame/resize) → always paint
   (`crates/frust-shell-common/src/frame_gate.rs` module doc ≈26–38; `app_tree.rs`'s
   `drive_frame` ≈914). Android wires it (`frame.rs` ≈422). iOS drains and drops the flags, and
   desktop discards them (`let _flags`); both relayout every frame. `LAYOUT` "implies a subsequent
   paint" (`view.rs` ≈18) only because paint always runs. So a container that changes geometry but
   reports only `PAINT` goes stale *on Android only*. `FlexView::rebuild` shows the discipline: it
   adds `LAYOUT` when `direction`/`cross`/`main` or the `flex` sidecar changes, and
   `rebuild_children` adds `LAYOUT | PAINT` on any length change.

5. **Reconciliation keeps identity; components add a state boundary.** `rebuild_child` wraps
   `rebuild_child_tracked`, which rebuilds under `with_focus_link(pod flag)` and detects a swap by
   `TypeId`. On a swap it drops `active`/`focused` and calls `mark_orphan_if_live` (≈432). That sets
   the thread-local `Cell<bool>` behind `frust_core::mark_focus_orphaned`, but **only if** the link
   was focused *and* the chain above it was live. `teardown_child` first sends a synthetic `Cancel`
   to a captured pod. The positional reconciler computes the **stable prefix** `k`: the first
   type-swap index, or the common length if there is none. Pods before `k` keep their paths, and
   `cancel_active_children` (≈533) clears the rest. That is Flutter's focus-retention invariant.
   Keyed lists (`ChildKey`, `crates/frust-widgets/src/lib.rs` ≈129) are all-or-nothing per list:
   a mixed list falls back to positional in `rebuild_children` ≈581. They *move* whole pods, flags
   included. A container exposes its pods with one `visit_children!(fields…)` line over `VisitPods`
   (`ChildPod`/`Option<T>`/`Vec<T>`/`[T]`); a container that skips that line reads as a leaf.
   **Components:** `Component` has exactly `type State`, `init` (once, under its own `Owner`), and
   `build` (every rebuild, returns `impl View<Self::State>`, which the hosting `ComponentWidget` erases with `AnyView::new`). It has no teardown. Teardown is
   `View<Outer>::teardown` on `ComponentView` (≈254), which disposes the owner. `ComponentWidget`
   holds `state`, `prev`, `child: ChildPod`, `owner`, `next_id`, and `disposed`, and its rebuild
   takes the double-box hop from idea 2 (≈208–215).

**Case study — the overlay portal (`overlay.rs` module doc).** It is **owner-hosted**: the owner
builds a plain `ChildPod` and lays it out loosely against `LayoutCtx::window_size`. It is
**root-painted**: the owner calls `PaintCtx::register_overlay(OverlayEntry { .. })` from its own
`paint`, and the registry is per pass (cleared when `RenderRoot::paint` opens it, drained after the
main tree), so the owner re-registers every frame. It is **root-routed**: `event` hit-tests overlay
rects first, then broadcasts `InputEvent::Overlay`. The floated pod sits behind an `Rc` that no
container's blur sweep visits, so the root must ask the pod itself (`holds_live_focus`,
`retire_stale_focus_link` in `paint_overlays` ≈1824). That is why the stamps in idea 3 exist.

## Experiments

All GPU-free; run from the repo root.

### 10.1 — Walk the arena and the pods

`cargo test -p frust-core --lib tree::` → 11 pass. In `tree.rs`, read `insert_root_and_find`,
then `inspect_walks_pre_order_with_absolute_bounds` (relative origins accumulate:
`(10,5)+(2,3)` → `Rect::new(12.0, 8.0, …)`), then `inspect_descends_through_child_pods` (only the
root is in the arena; pod ids start at `ChildPod::INSPECT_ID_BASE` = 2^48), and finally
`a_widget_that_ignores_the_seam_reads_as_a_leaf` (`len() == 1` even though it owns a pod).

### 10.2 — Type swaps, the stable prefix, and the orphan mark

```bash
cargo test -p frust-widgets --lib -- rebuild_child_clears_active_on_type_swap \
  a_truncated_focused_child_marks_the_focus_orphan a_keyed_type_swap_marks_only_on_a_live_chain \
  the_focus_chain_composes_through_nested_containers a_cleared_tail_marks_only_on_a_live_chain \
  focus_on_child_survives_append_after
```

6 pass. The first four pin the swap and orphan arms; the stable-prefix rule itself is in the last
two. In `a_cleared_tail_marks_only_on_a_live_chain`, index 0 swaps, so `k = 0`, and
`assert!(!pods[1].is_focused(), "the cleared tail drops the flag")` shows the tail being cleared.
`flex.rs`'s `focus_on_child_survives_append_after` is the other half: a pod before `k` keeps its
link. In `the_focus_chain_composes_through_nested_containers`, the two `drop_inner_leaf` calls use
the same leaf flag with a different outer link and get opposite marks.

### 10.3 — Routing lives in the container

`cargo test -p frust-widgets --lib -- blur_on_outside_tap_clears_focus_chain
key_reaches_focused_leaf_through_nested_container capture_and_focus_paths_are_independent` → 3 pass
(`authoring::focus_tests`). In the last test, a `Move` at `y = 999` still reaches the captured
child (`vec![201]`), because the capture path has no hit test.

### 10.4 — Components across the state boundary

`cargo test -p frust-core --lib -- local_state_survives_parent_driven_rebuild
anyview_type_swap_disposes_component_owner_once component_content_only_rebuild_keeps_focus_and_marks_nothing`
→ 3 pass. Local state survives a prop change (`count == 3`), a type swap runs `on_cleanup` exactly
once even after `drop`, and the every-frame same-type rebuild marks no orphan.

### 10.5 — Write your own: ids are stable across rebuilds

Add a scratch test file under `crates/frust-widgets/tests/` (e.g. `lab10_scratch.rs`; delete it
afterwards), then run `cargo test -p frust-widgets --test lab10_scratch`:

```rust
use frust_core::RenderRoot;
use frust_widgets::{SizedBox, column};
use kurbo::Size;

#[test]
fn child_ids_are_stable_across_rebuilds() {
    let mut app = |w: &mut f64| column()
        .child(SizedBox(Some(*w), Some(10.0)))
        .child(SizedBox(Some(20.0), Some(10.0)));
    let mut root = RenderRoot::new();
    let mut width = 10.0;
    root.rebuild(&mut app, &mut width);
    root.layout(Size::new(200.0, 200.0));
    let first = root.inspect();
    width = 50.0; // a real content change: child 0 gets wider
    root.rebuild(&mut app, &mut width);
    root.layout(Size::new(200.0, 200.0));
    let second = root.inspect();
    assert_eq!(first.len(), 3, "root Flex + two SizedBox pods");
    assert_eq!(first.iter().map(|n| n.id).collect::<Vec<_>>(), second.iter().map(|n| n.id).collect::<Vec<_>>());
    assert_ne!(first[1].bounds, second[1].bounds, "same id, new geometry");
}
```

It passes. The root is `WidgetId(1)` (from `alloc_id`); the pods are `281474976710656` and `…657`.
Ids belong to pods, and pods outlive views. Follow-up: make the second child `if swap {
Either::Left(Column(())) } else { Either::Right(SizedBox(..)) }` and flip `swap` — or more idiomatically, `either(swap, || Column(()), || SizedBox(..))`. The id stays; `type_name` goes
`SizedBoxWidget` → `FlexWidget`, because the swap happens *inside* the pod. (The `either()` form erases the type at the API boundary, whereas `any()` stays only for stored tables, accumulators into `Vec<AnyView>`, and three-plus-arm bodies; use `either(cond, || a, || b)` for two-arm bodies instead.)

## Child Sequences and Type Erasure

The container APIs that take children accept `impl ViewSeq<State, M>`, erased once inside. Choose the form that fits:

- **Tuples for mixed-type children:** `Column((app_bar, body, footer))` when each child is a different type. Tuples nest beyond 12 items; a 13-child list nests as `Column(((a, b, c), (d, e, f), ..., (x, y, z)))`.
- **`Vec<V>` for homogeneous or dynamic lists:** `Column(vec![item1, item2, ...])` when every child shares a type, or the count or order changes at runtime. Keyed lists always stay `Vec` via `keyed(id, view)`.
- **`either(cond, || left, || right)` for two-arm conditionals:** Returns `impl View<State>` and erases at the boundary. The two arms stay typed, an arm swap destroys and rebuilds state.
- **`any()`/`AnyView` only for irreducible cases:** stored view tables, accumulators pushing into `Vec<AnyView>`, recursive helpers, three-plus-arm bodies (which `any()` handles; anything with exactly two arms uses `either()` instead), trait extension points, and helpers feeding a `ChildPod` directly.

Helper functions return `impl View<State>` (or `impl View<State> + use<>` when the result is stored); the framework erases once at the API boundary, never at the call site.

## What to notice before moving on

- Nothing skips a subtree. A frame costs the whole diff, which is why `Component::build` must stay cheap
  (lab 3, experiment 3.3).
- A `ChildPod` is a *slot*. Identity, geometry, and interaction paths hang off the slot, not the
  widget. Keys decide which slot a child lands in; type swaps decide whether the slot's paths
  survive.
- `ChangeFlags` are only as good as their least careful producer, and the one shell that trusts
  them for layout is the one you test least on a desktop.
