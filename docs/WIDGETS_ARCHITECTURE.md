# Frust - WIDGETS Architecture

## Overview

WIDGETS covers `frust-widgets`, the baseline widget set (layout, controls, text, gestures,
navigation, platform-view slots), built over `frust-core`/`frust-scene`/`frust-text`/`frust-theme`
through a shared authoring toolkit; and its sibling `frust-theme`, a design-token crate bundling
color/type/shape/elevation/motion/glass values into a `Theme` that widgets recover from context and
app code reads reactively. The three built-in design systems (Material, Cupertino, Glyph) are no
longer part of this unit — they are sibling plugin crates in PLUGINS (`frust-glyph`/
`frust-material`/`frust-cupertino`, see [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)), built
on the same public authoring toolkit this doc describes.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how WIDGETS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-widgets::authoring` | Public container/callback toolkit every widget in the crate builds from, instead of touching `frust-core` primitives directly — reachable by app code as `frust::authoring` (the facade's re-export, CORE unit); also carries the `VisitPods` trait / `visit_children!` macro, the crate's introspection seam (see *Data Flow*) |
| `frust-widgets` (baseline) | Baseline layout containers and interactive leaf widgets (text, forms, gestures, scrolling, a virtualized `ListView`) |
| `frust-widgets::motion` | Implicit-animation and transition-pattern vocabulary |
| `frust-widgets::nav` | Imperative page-stack navigator, declarative router, and shared-element hero transitions |
| `frust-widgets::platform_view` | Native-sibling compositing slot and input-shield wrapper for translucent surfaces |
| `frust-theme` | `Theme` aggregate and its token tables (color, type, shape, elevation, motion, glass); carries no design-language token module of its own — only the neutral/language-free floor (`Theme::neutral()` and friends) |

The three built-in catalogs (`frust-material::*`, `frust-cupertino::*`, `frust-glyph::*` — PLUGINS
unit) live outside this crate's module tree entirely now; see *External Design-System Contract*
below and [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md) for their module structure.

## Layer Dependencies

`frust-widgets` depends on `frust-core` (`View`/`Widget` traits, layout, paint/event context,
semantics), `frust-scene` (renderer-agnostic draw commands), `frust-text` (text layout/editing
engine backing `frust-theme`'s typography tokens), `frust-theme` (design tokens bundled into
`Theme`), `peniko`/`kurbo` (shared color/geometry primitives), and the `image` crate (image decode
path). `frust-theme` is `frust-widgets`' primary consumer.

The one reverse edge is `frust-theme` depending on `frust-core`, but only for context-accessor
sugar (reading the active `Theme` from a context handle) — this does not create a cycle back into
`frust-widgets`.

Every container and interactive widget in `frust-widgets` is built through the crate's own public
authoring toolkit rather than touching `frust-core` primitives directly. The boundary is now
enforced structurally by the crate graph rather than a source-scan test: the three built-in design
systems are separate crates (`frust-glyph`/`frust-material`/`frust-cupertino`, PLUGINS unit) whose
only production dependency is the `frust` facade (`default-features = false`) plus `kurbo`/
`peniko` — they physically cannot reach a `frust-core` primitive `frust` doesn't re-export, the
same seam any third-party design system builds against (see *External Design-System Contract*
below). `frust::authoring` is the toolkit both the baseline set and every design-system plugin
build from.

## Data Flow

- Theme delivery (detail; the cross-unit summary lives in [ARCHITECTURE.md](ARCHITECTURE.md)): a
  shell owns the active `Theme`; widgets recover it type-erased from paint/layout context so
  `frust-core` stays theme-agnostic, while app code reads a cloned `Theme` via reactive context.
- Widget resolution precedence: explicit builder value > theme token > unthemed-fallback constant,
  generally re-resolved every paint; `Text`/`TextInput` instead bake the resolved color at layout
  time.
- Container plumbing: every container and interactive widget is built through the shared public
  authoring toolkit rather than touching `frust-core` primitives directly.
- Introspection flow: a container implements `Widget::visit_children` (CORE unit) via the
  authoring toolkit's `VisitPods` trait and `visit_children!` macro — one line naming its
  `ChildPod`-holding fields — so `WidgetTree::inspect`/`RenderRoot::inspect()` can enumerate its
  children; a hand-rolled child list (a row/slot struct behind an enum) implements `VisitPods`
  by hand instead and stays on the same seam. Both the baseline set and the three design-system
  plugins use it; see WIDGETS_CODE_STANDARDS.md for the authoring convention this obliges.
- Design-system layering: the three built-in design systems (PLUGINS unit) sit above the baseline
  set and the authoring seam, each an ordinary sibling crate assembling its own `Theme` via
  `ThemeBuilder`'s editors over `frust-theme`'s neutral floor rather than consuming a token module
  `frust-theme` ships for it.
- Glass/Cupertino chrome flow: `frust-cupertino` owns its own "Liquid Glass" `GlassScale` recipe
  (moved out of `frust-theme` with the rest of the catalog) and paints from it when supported,
  degrading to `frust-theme`'s neutral `GlassScale::opaque_material()` otherwise.
- Motion flow: implicit-animation and transition widgets resolve default timing from `Theme.motion`,
  collapsing to a short crossfade under `reduce_motion`; decorative loops request paced frames so
  the mobile frame gate can throttle them. `reduce_motion` is a floor, not an assignment: every
  baseline defaults it `false`, and a mobile shell OR's the OS accessibility preference over whatever
  the active theme authored (`effective = authored || os`, see SHELLS_ARCHITECTURE.md for the
  sensors) — an authored `true` is never un-reduced by an OS report of `false`.
- Navigation flow: `Navigator`/`Router` manage a page stack and declarative routes over the same
  container plumbing; `hero()` morphs a tagged child between pages during transitions.
  - **Navigator observation seams:** `NavigatorController::transition()` publishes a `Copy` `TransitionState`
    (active, progress, direction, depth pair, generation) on an `Rc<Cell<_>>` so chrome outside the
    subtree can observe transitions; reads during paint from a widget painted *after* the navigator
    are frame-exact, reads during `Component::build` are one frame stale. `PushOptions::on_visibility`
    and `PageVisibility` (Current/Visible/Covered) provide page-level visibility observation; the opt-in
    `cull_covered_builds` (default false) skips rebuilds for covered pages, but `on_cleanup` does **not**
    fire under a covering push (retained-state guarantee).
  - **Pop-result delivery:** `push_for_result`'s `on_result` callback needs `&mut State`, which the
    state-free view diff doesn't carry, so a pop still queues it — but the queuing rebuild also
    raises CORE's pending-result-flush flag, which the same rebuild drains via a `Housekeeping`
    broadcast (see CORE_ARCHITECTURE.md). An eager pop delivers its result within the rebuild that
    applied it; an interactive edge-swipe pop delivers on its settle frame. No further input event
    is required either way.
  - **RouteNavigator:** A thread-safe (`Arc<Mutex<Vec<NavRequest>>>`) navigation handle carrying no closures
    or reactive types, making it `Send + Sync` and usable via `provide_context` from any thread; off-thread
    requests wake the shell and apply in the next rebuild before the navigator reconciles.
  - **Route params:** Query parameters merge **under** path segment captures; path captures take precedence
    over query params of the same name, and named-route `path_for_name` round-trips unused params as query.
  - **overlay_host() constructor:** A `NavigatorView` with pop_swipe disabled and no-op transition
    (not a new widget). Back-button interest is arbitrated by **rank taken from the wiring entry
    point**, never from when or how often a controller wires: an overlay host always outranks a plain
    navigator, and among navigator peers the order is **innermost-first** (most-recently-wired wins,
    LIFO) — mirroring Android's `OnBackPressedDispatcher` dispatch order and UIKit/SwiftUI's
    pop-from-top behavior. Host-first is the same topmost-layer rule, not an exception to it —
    go_router's outermost-first order on Android is the counter-example this deliberately avoids. A
    host with no overlays reports no interest and defers to the inner navigator. Registrants are
    released by mount liveness (`NavigatorController::is_mounted()`), not by a wire-count heuristic.
  - **Back reach follows input routing (R23):** ranking (above) decides who wins *among* claimants;
    a separate reach check decides who may claim at all. A navigator whose hosting page is not in
    its host navigator's `input_routed_pages()` reports `back_interest() == false` outright,
    whatever its own stack looks like — so a press can never pop an off-screen nested stack while
    the visible page stays put. Reach is input routing, not painting: a page under a *transparent*
    overlay is still `PageVisibility::Visible` but claims no back. A top-level navigator has no
    hosting page and is unconditionally reachable, so single-navigator apps are unaffected.
  - **Controller binding is structural:** a `NavigatorView`'s published cells (depth, back interest,
    transition, mount count) bind to the `NavigatorController` it was last built or rebuilt against;
    a controller swap is detected by identity (not just view type) and re-bound in the same rebuild
    pass, so ops and published state can never target different controllers and a swapped-away-from
    controller reliably reports unmounted.
  - **NavigatorWidget semantics (R23):** Now implements `Widget::semantics`, forwarding via
    `ChildPod::semantics_child` to exactly the pages input routing can reach (covered pages and pages
    under modals omitted outright), honoring the input-parity invariant documented in
    CODE_STANDARDS.md. Back arbitration's reach check (above) derives from this same
    `input_routed_pages()` set, so it is the single reach definition shared by input, semantics, and
    back.
- Platform-view flow: `platform_view()`/`shield()` publish native-compositing slots and input-shield
  rects each frame for the shell layer to reconcile against native views.

## Key Types

| Type | Purpose |
|------|---------|
| `Theme` / `DesignLanguage` / `ThemeBuilder` | `frust-theme`'s aggregate design-token bundle, tagged by baseline (Material3/Cupertino/Glyph), plus a fluent editor |
| `ColorScheme`, `TypeScale`, `ShapeScale`, `Elevation`, `MotionScheme`, `GlassScale`, `StatusPalette` | Individual token-group types composed into `Theme`; `frust-theme` ships only each one's `neutral()` floor — a design system assembles its own values via `ThemeBuilder`'s per-group editors |
| `NativeTypefaces` / `FontFace` | A `ThemeExtensions` payload carrying a design system's own native-control font bytes (button/body face pair); no baseline `frust-theme` value attaches it — `frust-glyph::baseline()` is the one built-in that does — see NATIVE_WIDGETS_ARCHITECTURE.md's theme ladder |
| authoring module (`build_child`/`rebuild_child`/`teardown_child`/`rebuild_children`, `route_event`, `VisitPods`/`visit_children!`) | The sanctioned seam for authoring any widget against `frust-core`, including child-introspection |
| `Navigator` / `Router` / `hero()` | Page-stack and declarative routing plus shared-element transitions |
| `ButtonStyle`, `ScrollInfo`, `IconData`/`IconSource`, `ImageSource`/`ImageFit` | Small per-widget config/state types shared across the baseline widget set |
| `ListView` / `ListViewWidget` | Baseline virtualized list: windowed rebuild-time materialization, positional or keyed (`ChildKey`) row identity, optional variable extents, refresh/overscroll parity with `ScrollView` |
| `NavigatorController::transition()` / `TransitionState` / `PageVisibility` | Navigation state observation seams |
| `RouteNavigator` / `NavRequest` | Off-thread-safe navigation handle (Arc-backed plain data, no reactive types) |

## Architectural Facts & Constraints

### External Design-System Contract
The three built-in design systems — `frust-glyph`/`frust-material`/`frust-cupertino`
(`plugins/{glyph,material,cupertino}`, PLUGINS unit) — are themselves proof of this contract: each
is an ordinary sibling crate depending on `frust` (`default-features = false`) plus `kurbo`/
`peniko` only, built entirely on the public authoring/theme seam a third party gets too, no
special-cased access. Each follows the same conventions: its catalog is flat re-exported at the
crate root (`frust_glyph::app_bar`, `frust_material::AppBar`, `frust_cupertino::CupertinoButton`),
its token constructors are free functions (`baseline()`, `color_scheme_light()`/`_dark()`,
`type_scale()`, `shape_scale()`, `elevation()`, `motion_scheme()`) rather than an in-crate module
`frust-theme` used to ship, and `install()` seeds the theme with `frust::set_default_theme(baseline())`
from an `app!` `setup` block. `frust-glyph::baseline()` additionally attaches the
`NativeTypefaces` theme extension (its bundled monospace faces) — the one built-in that does, see
Key Types and NATIVE_WIDGETS_ARCHITECTURE.md's theme ladder.

`DesignLanguage` is `#[non_exhaustive]` with a `Custom(&'static str)` variant (compared by string
content, not interning identity) so a third-party design system can tag its identity without a
breaking enum change; every built-in `==` branch site treats an unrecognized tag as the
neutral/System path by construction. The public seam an external design system builds against is
three-fold: the `frust_widgets::authoring` toolkit (container/callback plumbing, `visit_children!`,
re-exported as `frust::authoring`), `Theme`'s token bus (`ThemeBuilder`'s per-group editors,
`Theme::neutral()` as a language-free baseline), and `ThemeExtensions` for typed, no-lock-in
payloads a baseline doesn't carry (e.g. `NativeTypefaces`, see Key Types) — plus `IconData::resolve`/
`same`, promoted `pub` so an out-of-tree catalog can paint its own icon glyphs the same way a
built-in one does. `examples/design-system-sample` is the reference proof for a genuinely external
crate (outside this repo's own workspace, unlike the three built-ins above) — a themed catalog plus
installer built on `frust`'s public API alone (see ARCHITECTURE.md's Examples table); its one
finding, that `Widget::semantics` cannot be exercised from out of tree, is registered in
LIMITATIONS.md. `plugins/shadcn` (`frust-shadcn`, PLUGINS unit) is the production-scale companion
proof: a 48-component port of a real third-party design system (shadcn/ui) built on this same
seam, not a sample — see PLUGINS_ARCHITECTURE.md's Design-System Plugins.

Three more seams are part of the same public authoring surface: opt-in hover claiming
(`EventCtx::claim_hover`/`PaintCtx::is_hovered`) for state-layer-style interaction chrome, cursor
requests (`EventCtx::set_cursor`/`CursorIcon`, also flat-re-exported as `frust::CursorIcon`) for a
design system's own hover/drag affordances, and the absolute-window-space `PaintCtx::origin`
contract that an anchored-overlay pattern positions against (see CORE_ARCHITECTURE.md's Hover and
Cursor section and Data Flow).

### Reactive-Free Design
`frust-widgets` contains no `reactive_graph` symbols crate-wide — the crate is entirely signal-free.
Reactive bridging lives in the CORE unit's facade (`router_glue.rs`, `back_glue.rs`), where observer
callbacks and context providers can reach the reactive runtime. This boundary keeps the widget set
reusable and decouples it from the reactive layer; state machine state in nav/navigator is held in
plain `Rc<Cell<_>>`/`Arc<Mutex<_>>` instead.

### Icon Generation
`crates/frust-widgets/src/icons/mod.rs` is **generated** by `scripts/gen_icons.py` from a hardcoded
`STARTER_SET`. Hand-edits to this file are destroyed on the next `gen_icons.py` run; expand the icon
set by modifying the script's source list, not the generated output.

### Text Widget Alignment
`TextInput`'s live-edited text is always start-aligned, regardless of `TextStyle::align`, because
parley's `PlainEditor` exposes no text-alignment hook. This is a parley limitation, not a frust
design decision; users cannot work around it per-field. `TextView` does honor `align`.

### Virtualized ListView (baseline)
`ListView`/`ListViewWidget`/`list_view()` live in `frust-widgets` proper (`list_view.rs`), not a
design-system catalog — the facade re-exports them unconditionally regardless of which (if any)
design-system plugin an app depends on. `frust_material::list_view` (moved with the rest of the
Material catalog to `plugins/material`) remains only as a deprecated compatibility shim
(individually `#[deprecated]` type aliases/fn, not a re-exported module) so an existing
`material::list_view::…` call site keeps resolving; new code uses the baseline path.

Each frame's `rebuild` reads the retained widget's own scroll offset and cached viewport to
materialize only the visible window (plus a small buffer) — the only `View` in the framework that
reads retained element state during `rebuild`. Layout only measures pods that already exist and
paint builds nothing, so the row-builder closure never runs outside `rebuild` — the same invariant
every other widget upholds, exercised here against a windowed rather than fixed child set.

**Two row-identity models.** `ListView::builder` reconciles rows by raw index — correct for
append-only, truncate-only, or full-replace data, but a mid-list insert/remove/reorder silently
reattaches a row's retained state to whatever now sits at that index. `ListView::builder_keyed`
adds a `ChildKey` per row (the same identity `keyed()` uses for `Flex`, see CODE_STANDARDS.md) so
state follows the row through a mutation instead; a keyed list additionally re-anchors its scroll
offset in `rebuild`, before windowing, so a prepend/removal above the viewport never visibly jumps
the content the user is looking at **when the mutation is single-sided** — a same-frame mutation on
both sides of the anchor (e.g. a prepend above *and* an append below in one frame) can still miss
the anchor-shift correction itself, a documented, deferred gap (`list_view.rs`'s module doc, *Cache
hygiene*). Unlike `Flex`'s keyed reconciler, a duplicate key on `ListView`
`debug_assert!`s but has **no positional fallback** in release — the first slot claiming a key wins
deterministically, later duplicates rebuild fresh.

**Variable extents are keyed-only.** `.estimated_item_extent(px)` switches a keyed list from the
closed-form uniform path to a measured-by-key extent cache (an estimate for unvisited rows, the
row's own layout height once visited); calling it on a positional list `debug_assert!`s and is
inert in release. A measurement that revises a row's height above the viewport's top edge is
corrected through the same rebuild-time anchoring path — recorded at layout, committed on the
following rebuild so the anchor row never visibly moves. A correction observed mid-fling
accumulates and commits only once the fling settles, so it never fights the fling pump.

**Refresh/overscroll parity with ScrollView.** `on_refresh_release` and drag rubber-band overscroll
share `scroll.rs`'s `pub(crate)` resistance/trigger/settle constants with `ScrollView` verbatim, so
`ScrollView` is no longer the framework's only pull-to-refresh-capable widget.

Virtualization exists because eager materialization doesn't scale: an in-repo host bench
(`crates/frust-widgets/tests/list_virtualization_bench.rs`) shows `ListView`'s rebuild+layout cost
staying flat against item count while an eagerly-built `ScrollView`+`Column` scales roughly
linearly; a CI-durable structural assertion in the same file pins the windowed-materialization fact
itself (not the timing) at `N = 10,000`.

### Recent Additions (Batch 2)
**Navigator observation:** `TransitionState` and `PageVisibility` seams; `overlay_host()` constructor;
R23 semantics forwarding. **Routing:** route params now merge query under path captures, and
`RouteNavigator` allows off-thread navigation. **Widgets:** `frust_glyph::sheet` (modal overlay with
staged dismiss and scrim fade, now `plugins/glyph`); `button` disabled state; `TextInput` read-only
mode and `content_type` IME hints; `TextView` alignment control; `EmptyStateView` and `MenuEntry`
icon slots; badge `Info` variant with warning border.
