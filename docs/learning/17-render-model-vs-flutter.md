# Lab 17 — Frust's Render Model, Mapped onto Flutter's

**Concept:** A fair critique of an early frust read: "no layer tree, no repaint boundaries, no
slivers, no hybrid composition, no texture widget." Each of those now has a counterpart, and none
is a copy of Flutter's. This chapter is for an engineer who knows Flutter's render tree: it maps
each Flutter concept to the frust mechanism that does the job, says *why* the shape differs, lists
what Flutter still does that frust does not (each with the `docs/LIMITATIONS.md` entry that records
it), and gives three labs you can run today. Read it after labs 10 (widget tree) and 12 (clips,
layers, punch); lab 9 covers the native-view half in more depth.

Every Apple (macOS/iOS) cell of the platform-view matrix reads *pending*, and every Android device
outcome reads *device leg pending*. This chapter says the same and no more.

## The mapping

Flutter citations are paths under `packages/flutter/lib/src/` in `flutter/flutter`, or a
docs.flutter.dev page. Frust paths are in this repository.

| Flutter | Frust | Where (frust) / reference (Flutter) | Why it differs |
|---|---|---|---|
| `Widget` (immutable config) | `View` | `crates/frust-core/src/view.rs` / `widgets/framework.dart` | Same role: a cheap value rebuilt every frame and diffed. |
| `Element` + `RenderObject` | `Widget` behind a `ChildPod` | `crates/frust-core/src/widget.rs` (`ChildPod`) / `widgets/framework.dart`, `rendering/object.dart` | One retained object does layout, paint and events; the pod carries what Element/RenderObject carry in Flutter: identity, geometry, dirtiness. No parent pointers; containers own `ChildPod`s (lab 10). |
| `BoxConstraints`, `Size` | `BoxConstraints`, `Size` | `crates/frust-core/src/widget.rs` / `rendering/box.dart` | Same protocol: constraints down, size up. |
| `RenderSliver`, `SliverConstraints`, `SliverGeometry` | `SliverWidget`, reached through `Widget::as_sliver` | `crates/frust-core/src/sliver.rs`, `crates/frust-widgets/src/slivers/` / `rendering/sliver.dart` | Additive beside the box protocol: `as_sliver` defaults to `None`, so no box widget changed. Making every widget speak a second protocol would have touched every widget in the framework for the benefit of the few that scroll. |
| `CustomScrollView` | `custom_scroll_view` | `crates/frust-widgets/src/viewport.rs` / `widgets/scroll_view.dart` | Same composition: a list of slivers in one viewport. `sliver_box`, `sliver_list`, `sliver_grid`, `sliver_fixed_extent_list`, `sliver_padding`, `sliver_fill_remaining`, `sliver_persistent_header`. |
| `NestedScrollView` | not yet | `LIMITATIONS.md` `sliver-v1-scope` / `widgets/nested_scroll_view.dart` | There is no overlap absorber to coordinate a header with an inner scroller. |
| Layer tree (`ContainerLayer`, `OffsetLayer`, `EngineLayer`) | `PushRetained` / `DrawRetained` commands plus the engine's resident-key ledger | `crates/frust-scene/src/`, `crates/frust-engine/src/cache/retained.rs`, `crates/frust-engine/src/compile/layers.rs`, `crates/frust-core/src/retained.rs` / `rendering/layer.dart` | Frust has no framework-side layer tree. The scene stays a flat display list; the *renderer* owns a budgeted LRU store of rasterized pages and publishes which keys are resident (the ledger). The widget tree asks "is my page resident at this extent and scale?" and either draws it or paints the body again. A refusal always degrades to a plain paint, never to a missing frame. |
| `RepaintBoundary` | `repaint_boundary(child)`, `.alpha()`, `AnimatedOpacity::cached` | `crates/frust-widgets/src/repaint_boundary.rs`, `crates/frust-widgets/src/motion/animated.rs` / `rendering/object.dart` (`isRepaintBoundary`), `widgets/basic.dart` | Explicit, like Flutter's widget, but never automatic: nothing inserts boundaries for you. A body that publishes a paint-time side channel is repainted every frame instead of retained. |
| Relayout boundary | per-pod constraint-equality relayout skipping | `crates/frust-core/src/widget.rs` (`layout_child`), `crates/frust-core/src/app/layout.rs` (`RenderRoot::layout_with_text`) / `rendering/object.dart` (`_relayoutBoundary`) | A pod whose constraints equal last pass's and which is not dirty returns its cached size. Without parent pointers, a parent on a dirty path still runs its own layout; only clean subtrees below it are skipped. |
| Layout assertions | `FRUST_LAYOUT_AUDIT` | `crates/frust-core/src/app/layout.rs` (`LayoutAudit`, `RenderRoot::set_layout_audit`), `docs/CORE_DEVELOPMENT.md` | After a skipping pass the audit runs a forced full pass and logs every pod whose geometry differs: it finds a widget that changes layout state without saying so. A debug aid; it doubles layout cost. |
| `ClipPath`, `BackdropFilter` | `PushClipPath`, `BackdropBlur` | `crates/frust-scene/src/`, `crates/frust-engine/src/compile/clip.rs` / `widgets/basic.dart` | Path clips are coverage masks; at most four backdrop blurs render per frame (`backdrop-blur-bound`). |
| Partial repaint / damage | measured, rejected, removed | `LIMITATIONS.md` `partial-repaint-outcome` | Flutter repaints only the damaged region when the embedder reports one (`engine/src/flutter/shell/common/rasterizer.h`, `FrameDamage`); frust re-rasterizes the surface each rendered frame. A scissored partial path was built and measured: it lost on mobile-shaped workloads, so it was removed. |
| `PlatformView` hybrid composition | overlay surfaces plus the Mode A / Mode B matrix | `crates/frust-shell-common/src/platform_view.rs` (`ingest_scene`), `crates/frust-render/src/overlay.rs` (`# Segments`), `docs/SHELLS_ARCHITECTURE.md`, `docs/NATIVE_WIDGETS_ARCHITECTURE.md` / docs.flutter.dev/platform-integration/android/platform-views | Mode A: opaque frust surface, native view above. Mode B: translucent surface, native view below, hole-punched. To paint frust content *above* a native view the display list is cut at the view's slot and each cut is drawn on its own overlay surface. Android ships this behind `FRUST_OVERLAYS` (default off); Apple is pending. |
| `Texture` widget | `texture_view` fed by a `TextureSink` | `crates/frust-widgets/src/texture_view.rs`, `crates/frust-plugin/src/texture.rs`, `crates/frust-render/src/texture_bus.rs`, `frust::texture` / `widgets/texture.dart`, `rendering/texture.dart` | A producer pushes frames into a latest-frame mailbox; the renderer imports them as a scene texture. Producers: video-player `VideoOutput::Texture`, camera `preview_texture`. Android hardware-buffer import (`crates/frust-gpu/src/import/android.rs`) is behind `FRUST_ANDROID_HWBUFFER_IMPORT`. |

## What Flutter does that frust still does not

| Gap | Entry in `docs/LIMITATIONS.md` |
|---|---|
| No partial repaint; each rendered frame re-rasterizes the surface | `partial-repaint-outcome` |
| Residual render-model gaps (a parent on a dirty path still lays out; a non-default `BlendMode` on a layer is refused; Apple pending) | `render-model-gaps` |
| Retained pages are a budgeted, refusable cache; the ledger is process-global; an eviction between publish and the next scene can refuse a frame | `retained-pages-budget-refusals`, `retained-ledger-single-renderer`, `retained-stale-ledger-eviction-thrash` |
| `BackdropBlur` has five limits; `PushClipPath` re-rasterizes its mask per push | `backdrop-blur-bound`, `path-clip-mask-rerasterized` |
| No `NestedScrollView`, no reverse growth, no snapping or stretching headers | `sliver-v1-scope` |
| `ScrollView` and `ListView` are not built on the sliver viewport | `scroll-view-list-view-not-viewport-based` |
| Overlays are off by default on Android although the host passes its device leg | `android-overlays-opt-in-pending-device-leg` |
| An Android overlay covers every native view, including later slots | `android-overlays-cover-later-slots` |
| Content an overlay cannot draw (retained pages, scene textures, backdrop blur) stays under the native view | `overlay-unservable-content-stays-on-main` |
| Desktop hosts a native view in Mode A only | `desktop-platform-view-mode-a-only` |
| A platform view leads at the start of a scroll on Android | `cam-gesture-onset-lead` |
| Android hardware-buffer import is opt-in and RGBA8-only | `android-hwbuffer-import-opt-in` |
| Android video textures copy through GLES; camera preview textures are CPU-copied | `video-texture-android-gles-copy`, `camera-texture-cpu-copy-only` |
| A native frame's release can lag by a submit; a full backlog drops the newest frame | `texture-bus-native-release-delay` |
| An external texture never joins the opaque depth-writing pass | `engine-scene-texture-always-blended` |

Nothing Apple-side in the platform-view and texture rows is shipped: overlay hosts, Metal import and
Apple texture producers are pending (`render-model-gaps`).

## Lab 17.1 — Read a frame with and without retained layers

The retained-layer tests record a scene and assert its command list.

```bash
cargo test -p frust-widgets --test repaint_boundary
```

`frame_one_records_and_an_unchanged_frame_two_draws_the_page` shows frame one opening with a
`PushRetained` bracket and an unchanged frame two emitting a `DrawRetained` for the resident page.
`the_kill_switch_paints_plain_bodies_with_no_bracket` runs the same widget after
`RenderRoot::set_retained_layers(false)` and sees only the plain fill under its fade, whatever the
ledger says. `FRUST_NO_RETAINED_LAYERS` (see `docs/CORE_DEVELOPMENT.md`) is the same switch for a
running app; its engine-side counterpart is `FRUST_ENGINE_NO_LAYERS`
(`docs/RENDER_DEVELOPMENT.md`).

**Try:** in the first test change the leaf's shade between frames and read which command list comes
out (`a_leaf_change_under_the_boundary_re_emits_the_body`). Where would Flutter's `OffsetLayer`
reuse sit in this picture, and why does frust ask the renderer rather than keep a tree?

## Lab 17.2 — Catch a lying widget with the layout audit

```bash
cargo test -p frust-core --test layout_audit
```

`crates/frust-core/tests/layout_audit.rs` defines `LiarWidget`, whose `layout` returns a new size on
every call without reporting a change. With `set_layout_audit(true)`, the skipping pass is followed
by a forced one, and `a_lying_widget_is_reported_with_its_type_name` finds a `layout-audit:` warning
naming the widget. `a_well_behaved_tree_reports_nothing_over_scripted_changes` is the control, and
`audit_off_is_a_single_pass` shows the cost you pay only when it is on.

**Try:** run any app with `FRUST_LAYOUT_AUDIT=1` and `RUST_LOG=warn`; a clean app prints nothing.
Then turn skipping off with `FRUST_NO_LAYOUT_SKIP=1` to confirm a suspected stale-geometry report.

## Lab 17.3 — Build a sliver

A `custom_scroll_view` takes a tuple or `Vec` of slivers; anything that is not a sliver is wrapped
in `sliver_box`.

```rust
use frust_widgets::{ChildKey, CustomScrollView, custom_scroll_view, sliver_box, sliver_list, text};

let view: CustomScrollView<()> = custom_scroll_view((
    sliver_box(text("header")),
    sliver_list(
        5_000,
        |i| ChildKey::new(i as u64),
        |i| text(format!("message {i}")),
    )
    .estimated_item_extent(64.0),
));
# let _ = view;
```

Drive it the way `crates/frust-widgets/tests/slivers_a.rs` does (a `RenderRoot`, scripted scroll
events, a ledger of live tiles) and watch only the visible rows be built. Then read how a widget
becomes a sliver: `Widget::as_sliver` in `crates/frust-core/src/widget.rs` returns
`Option<&mut dyn SliverWidget>`, and `crates/frust-core/tests/sliver_seam.rs` implements the seam on
a test widget.

**Try:** add `sliver_persistent_header` (`crates/frust-widgets/src/slivers/persistent_header.rs`)
above the list and read `SliverGeometry` from `ViewportWidget::sliver_geometry` as you scroll.
Then write down what you would need to add for a `NestedScrollView` (`sliver-v1-scope`).

## What to notice before moving on

- Frust's answer to "layers" is a cache in the renderer plus an explicit boundary widget, not a
  second tree in the framework. Every refusal falls back to plain painting.
- The sliver protocol rides beside the box protocol through one defaulted method.
- Both claims about platform views and textures hold on Android and Apple behind default-off knobs;
  the Android device legs ran on a OnePlus 9 and the Apple legs on an iPhone SE and macOS.
