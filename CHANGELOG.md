# Changelog

All notable changes to Frust are recorded here, newest first, in the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) shape. GitHub release notes are copied
from the section for the tag (see [docs/RELEASING.md](docs/RELEASING.md)).

## Unreleased

### Fixed

- The CLI resolves the app's `frust-ui` dependency key, so both `frust-ui = ".."` and
  `frust = { package = "frust-ui", .. }` work for `run`, `build` and hot sessions: every cargo
  argument list, Android `-Pfrust.cargoFeatures`, iOS `FRUST_FEATURES` and hot-patch fat build
  spells features with the app's key, a missing-feature cargo failure on a debug or profile build
  carries a one-line hint naming both forms, and release lanes refuse devtools through either key
  (c801185c, 5fb139c6, fcbb1e91).
- Codemod (`scripts/codemod/frust_any_codemod.py`): the name-list callees and the
  `column`/`row`/`stack` builders resolve by import origin (a strict `frust*` path head), so an
  app fn or an app-imported builder that shares a listed name is skipped with a note (412dc49a);
  non-private T7 helpers land in a T7-visible bucket and are rewritten only with `--t7-visible`, a
  `return` inside a closure is not an early return, a first-body-line keep marker that rustfmt
  moved is fn-scoped for T7 and rule U (40b6f35f), and a helper whose same-file call is taken by
  `&` is excluded as "taken by reference" (46be57ae).
- Codemod T7 emits a type-only `+ use<..>` whenever the helper (or its impl) has a lifetime
  parameter or an elided reference parameter, in any edition, so a stored borrow of a temporary no
  longer fails E0716; an argument-position `impl Trait` still skips with a note. T7-ref counts a
  use in another file only when it is path-qualified to the helper's module or imported from it,
  `let x = <expr>; any(x)` tails collapse to `<expr>` (no `clippy::let_and_return`), and "taken by
  reference" also covers a same-file call bound by an untyped `let` and later borrowed as `&x` or
  `&mut x` in the same block (bd02b8b7, f3e36a12).
- Codemod: a local fn shadows a builder only in its own module and descendants, so a `fn row` in a
  nested `#[cfg(test)] mod` no longer blocks T2/T3 in the parent; a reasoned `// erasure: keep`
  marker on an accumulator `let` of `AnyView`s covers that vec's `push`/`insert`/`extend` calls in
  the same fn, and a reasoned own-line marker inside an erasure call's parentheses (the form rustfmt
  produces for a multi-line `any(..)`) covers that call for rule U and the T1-T7 keep exemption
  (1b015a7b).

### Changed

- Migration guidance: use `Either` or `either(..)` only when the two arms have different concrete
  view types. When both arms produce the same type, build it conditionally (compute the differing
  props or children in an `if`, construct once): `Either` reconciles a Left/Right swap as a
  teardown and remount, which drops in-flight gestures and can orphan focus
  ([docs/WIDGETS_CODE_STANDARDS.md](docs/WIDGETS_CODE_STANDARDS.md)).

## 0.6.0 — 2026-10-11

Type erasure moved from the call site to the API boundary: public builders take generic views and
erase internally, and views are composed with fluent builders, child sequences (`ViewSeq<State, M>`)
and `Either<L, R>` instead of `any()` everywhere; helpers return `impl View<S>` instead of
`AnyView<S>` wherever the result is not stored erased. Debug builds hot-patch a saved edit into the
running app on the desktop, an Android device and the iOS simulator, keeping the process and every
component's State.

### Breaking

- `Component::build` returns `impl View<Self::State>` instead of `AnyView<Self::State>`. An impl
  that keeps `-> AnyView<..>` warns (`refining_impl_trait`) and fails under `-D warnings`.
- `Component` is no longer dyn-compatible: `dyn Component` and `Box<dyn Component>` do not
  compile. Use generics, or store the `AnyView`s a component produces.
- Driver closures must erase the build result with `AnyView::new(root.build(state))`; the opaque
  return captures the `&self` and `&mut State` borrows and cannot escape the closure otherwise.
  `frust::run`, `frust::app!` and the templates already do this.
- Public parameters that were typed `AnyView` are generic (`impl View<S>`, `Option<V>`,
  `impl IntoIterator<Item = impl View<S>>`, `impl Fn(..) -> V`, `ViewSeq<S, M>`), with one exception
  that keeps erasure because it lazily produces views: the deprecated `frust_material::list_view`
  item builder (`impl Fn(usize) -> AnyView<State>`; `frust::list_view` is its generic replacement).
  The material button-group icon family (`button_group_icon_action`, `ButtonGroupAction::icon` and
  `checked_icon`, `ButtonGroupView::overflow_icon`), `SplitButtonView::leading_icon`, beui's
  `ChatConversationView::extra_rows` and clean-signals-frust's `async_view` take generic views too.
  Callers passing `any(..)` (or a closure returning it) still compile, but a bare `None`, an empty
  `vec![]`, an explicit turbofish call, or a `.collect()` straight into a list parameter now needs a
  type: a turbofish caller of a plugin list function writes `::<State, _>`, and one of
  `button_group_icon_action` needs one more `_`.
- `InputEvent` gained a `FileDrop(FileDropEvent)` variant (with the new `FileDropPhase`).
  `InputEvent` is not `#[non_exhaustive]`, so an exhaustive `match` over it needs a `FileDrop(_)`
  arm (E0004) (`crates/frust-core/src/event.rs`). The same release adds `OverlayAnchor::Window`
  (`crates/frust-widgets/src/overlay.rs`), which an exhaustive `match` over `OverlayAnchor` must
  also handle.
- Public exhaustive-enum changes in tooling crates (none is `#[non_exhaustive]`, so an exhaustive
  `match` outside the crate breaks): `frust-devtools-protocol` `Capability` gains `HotPatch` and
  `Method` gains `HotpatchInfo`, `PatchChunk`, `TableChunk` and `ApplyPatch`; `frust-tui`'s
  engine `Message` (+`HotPatchNow`, `HotPatchOutcome`, `HotSessionStarted`,
  `RunConfigToggleAutoApply`, `RunConfigToggleRunHot`; -`RunConfigToggleWatch`), `RegionId`
  (+`RunAutoApplyRow`, `RunHotRow`; -`RunWatchRow`), `RunFocus` (+`AutoApply`, `RunHot`;
  -`Watch`), `Effect` (+`HotPatch`, `HotPatchNow`, `LaunchHotSessions`;
  -`LaunchWatchedSessions`) and `SessionTarget` (+`Simulator`). `frust-shell-desktop`'s
  `ShellUserEvent` (doc-hidden) became `#[non_exhaustive]` and gained `HotPatched` (`hotpatch`
  feature only). Files: `crates/frust-devtools-protocol/src/{messages,method}.rs`,
  `crates/frust-tui/src/engine/{message,update,run_config,session_view}.rs`,
  `crates/frust-shell-desktop/src/app_handler.rs` (`git diff v0.5.2 v0.6.0 -- crates`).

### Known limitations, now documented

These behaviours already held before 0.6.0; this release documents them in `docs/LIMITATIONS.md`
rather than introducing them.

- Wrapper views with `type Element = Box<dyn Widget>` that forward to an inner `AnyView` hide
  inner type swaps from focus bookkeeping (`focus-wrapper-erasure-swap-blind`, the renamed
  `focus-double-erasure-swap-blind` entry that 0.5.1 already carried).
- Keyed flex lists are all-or-nothing, and the check is a `debug_assert` (present since 0.5.1,
  `crates/frust-widgets/src/authoring.rs:598` at v0.5.1): a mixed list silently falls back to
  positional reconciliation in release builds (`keyed-list-all-or-nothing-debug-only`).

### Added

- Hot patching (debug builds): `frust run --watch` (the desktop preview),
  `frust run -d <Android device> --watch` and `frust run -d <booted iOS simulator> --watch` patch
  each settled save into the running app. The process, its PID and every component's State are
  kept; the CLI prints `patched in N ms` and relaunches only on `restart required: <reason>`; a
  compile error leaves the app untouched; `--no-hot` keeps the kill-and-relaunch loop. It works on
  a stock `frust create` app.
- Hot-patch targets: macOS, Linux and Windows x64 (MSVC) desktop, Android arm64-v8a and the arm64
  iOS simulator. Not supported: a physical iOS device, the web, Windows on ARM64 and other Android
  ABIs.
- Hot patching in the TUI: debug desktop, Android-device and iOS-simulator sessions run hot by
  default; `r` hot-patches now, `W` toggles the watcher, `R` is a full rebuild and relaunch, and
  `o` opens the run dialog with 'Run hot' and 'Auto-apply on save' (both on by default).
- Hot-patch restart fallbacks, each printed as `restart required: <reason>` and answered with a
  fresh build and launch: a replayed type's layout change, a component State type change, a
  manifest, build script, cargo config, toolchain or `frust.toml` edit, a path-dependency edit on
  Android or the iOS simulator, a proc macro or an uncaptured package, the patch budget (64
  patches or 96 MiB per process by default, set by `[hotpatch]` in `frust.toml`) and a lost
  apply reply. See `no-hot-reload-restart-is-a-rebuild` in `docs/LIMITATIONS.md`.
- New crate `frust-hotpatch`, the in-app hot-patch runtime (a port of Dioxus subsecond 0.7.10),
  and the opt-in `frust/hotpatch` feature, which a hot run switches on for its own build only.
- `ScrollController` for `ScrollView` and `ListView`: programmatic `jump_to`, physics-driven
  `animate_to`, `scroll_to_item` with settle-and-correct, offset publishing and `on_change`; a
  command ends a live drag. shadcn's `scroll_area` thumb can be dragged.
- Drag and drop: a `DragCoordinator`, `draggable` sources and `drag_target` drop targets with
  keyboard and accessibility parity, auto-scroll zones, `reorderable_list`, and desktop file drop
  (`InputEvent::FileDrop` with `FileDropEvent`/`FileDropPhase`).
- `frust::ViewSeq<State, M>` — the child-sequence trait every list parameter now takes (`M` is an
  inference marker callers never name): implemented for a single view, tuples of 1–12 views (nest
  tuples beyond 12), `Vec<V>`, arrays `[V; N]`, `Option<V>` (`None` drops its slot, like
  `.when_some()`), and iterators wrapped in `views(iter)`; erasure happens once, inside the
  container.
- `frust::Either<L, R>` — two-arm typed view composition with `either(cond, || l, || r)` factory.
  Swapping an arm rebuilds; both arms stay typed.
- `frust::views(iter)` — wraps an iterator as a child sequence. A bare `.collect()` passed to a
  sequence API needs `.collect::<Vec<_>>()`.
- Generic view parameters for the icon and row APIs that still took erased closures:
  `button_group_icon_action`, `ButtonGroupAction::icon`/`checked_icon`,
  `ButtonGroupView::overflow_icon` and `SplitButtonView::leading_icon` take `Fn() -> V` with
  `V: View<State>`; beui's `ChatConversationView::extra_rows` takes `Fn() -> Seq` with
  `Seq: ViewSeq<State, M>`; clean-signals-frust's `async_view` takes loading, data and error
  closures returning any view and erases once inside.
- `AnyView::new` and `any()` are idempotent: erasing an `AnyView` returns it unchanged.
- Fluent builders: `column()` and `row()` with `child`, `flex`, `keyed`, `children`, `push`,
  `when` and `when_some`; `stack()` with `child`, `children`, `when` and `when_some`.
- `scripts/codemod/frust_any_codemod.py` migrates an app to the new idiom, including the opt-in
  rule `--t5` that drops `any()` from homogeneous `vec![..]` list arguments, and rules T6 and T7
  (opt-in flags, enforced by the tripwire): T6 rewrites `vec![..]` arguments of sequence APIs to
  tuples when all elements are erasure calls (1–12 of them); T7 converts helpers ending in one
  erasure call to return `impl View<S>` instead of `AnyView<S>`.
- `scripts/ci/erasure-check.sh` guards the repository against redundant erasure (rules T1–T7 in
  strict mode) and runs as a CI hygiene step; a site that must keep `any()` carries a
  `// erasure: keep <why>` comment the tool honours.
- `docs/PERFORMANCE_BASELINES.md` collects the size and build-time measurements.

### Changed

- `frust create`'s default template is a single `src/lib.rs` holding the whole counter app: one
  `Component` whose `build` is one declarative widget tree, helpers as associated functions called
  `Self::name()`, and `frust::app!` default-constructing the root; `src/home_page.rs` is gone. The
  clean-signals template's pages pass generic `async_view` closures with no `any()`.
- `Column`, `Row`, `Stack` fluent builders and all layout/plugin list parameters (`frust-material`,
  `frust-beui`, `frust-shadcn`, `frust-glyph`) accept any child sequence; `Vec<_>` callers compile
  unchanged; iterator callers wrap in `views(..)`.
- Helpers across the examples, benchmarks and the app template return `-> impl View<S>` where the
  result is not stored erased (`-> AnyView` signatures 1,159 → 592 tree-wide in the first pass);
  the sites that must stay erased — stored view tables and page registries, accumulators into
  `Vec<AnyView>`, three-plus-arm bodies, helpers feeding a hand-written view's `ChildPod` — keep
  `AnyView` and carry `// erasure: keep <why>`.
- The example apps, benchmarks and templates carry no redundant erasure: every remaining `any()`
  in `examples/` and `benchmarks/` is marked `// erasure: keep <why>`, and the tripwire enforces it
  (rule U, the codemod's `--unmarked` check).
- Templates, README and rustdoc present erasure at the API boundary and the fluent builders;
  rustdoc examples and the crates.io READMEs are on the 0.6 idiom with 0.6 version pins.
- frust-mcp's `restart_app` description no longer claims Frust has no hot reload.
- frust-engine bounds flattening cost for every finite input: huge viewport-straddling curves are
  split at the viewport before flattening (the pre-flatten viewport split).

### Fixed

- frust-mcp: rmcp 1.8 → 2.2, fixing the Streamable-HTTP session leak.

### Measured

- Hot patch, save to patched frame against a full restart (medians): macOS desktop 1,397 ms vs
  5,218 ms (1b1ef31d); Pixel 5, stock app, 2,883 ms vs 13,458 ms (4c7e25b3); Windows x64 1,468 ms
  vs 10,861 ms (90113508); arm64 iOS simulator 1,694 ms vs 4,395 ms (6b82685e). material3-demo on
  a Pixel 5 patches in 3.5 s (median over four body-only edits), down from 58-72 s (80663b22).
- Cost of erasure at the API boundary (medians of 3 on one host, main 79686b68 → f79b739c; details
  in `docs/PERFORMANCE_BASELINES.md`): stripped arm64 `.so` huddle +0.12 %, material3-demo
  +0.34 %; clean release rebuild frust-gallery +1.6 % (an earlier run at the pull-request head
  measured +3.4 %), material3-demo −2.9 %. Build-time deltas of this size sit inside the host's
  run-to-run noise; the size figures are exact.
- Cost of child sequences and helper return types (i5-12600 Linux, main 721ab1e0 →
  feature/erasure-free-apps b2a41334; see `docs/PERFORMANCE_BASELINES.md`): stripped arm64 `.so`
  (material3-demo) 14,643,024 B → 14,658,576 B (+0.11 %; bar ≤ 2 %); clean release rebuild
  frust-gallery 14.42 s → 13.74 s (−4.7 %; bar ≤ 10 %), material3demo 129.02 s → 125.77 s
  (−2.5 %; bar ≤ 10 %).

### Migration

1. Change every `impl Component` whose `build` returns `AnyView<..>` to `-> impl View<Self::State>`;
   the body may keep `any(..)` for now.
2. Where the app writes its own driver closure, erase the build result there:
   `move |s| AnyView::new(root.build(s))` (`frust::run`, `frust::app!` and the templates already
   do).
3. Run `python3 -I scripts/codemod/frust_any_codemod.py --write --t5 --t6 --t7 <src dirs>`, then
   `cargo fmt` and `cargo check`.
4. Fix what the compiler reports: give a type to any bare `None`, empty `vec![]`, turbofish call or
   `.collect()` it rejects (a `.collect()` passed to a sequence API gets `::<Vec<_>>()`, a turbofish
   caller of a plugin list function writes `::<State, _>`, one of `button_group_icon_action` adds a
   `_`); a caller whose result now lands in a `Vec<AnyView>` or a `match` arm is re-erased with
   `any(..)` (or its list becomes a tuple); two-arm bodies use `either(cond, || a, || b)`, except a
   body whose two arms both consume one moved value, which uses `Either::Left`/`Either::Right` (or a
   `match`) instead of `either(..)`. Only a site that must stay erased (a stored view table, an
   accumulator into `Vec<AnyView>`, a three-plus-arm body, a `ChildPod`-fed helper, a same-head list
   whose elements differ in type) keeps `any()`. Re-run the codemod after each fix until
   `--check --t6 --t7` is quiet.
5. Replace `dyn Component` with generics or stored `AnyView`s; see `docs/LIMITATIONS.md` for the
   two behaviours listed under Known limitations, now documented.
6. Run `scripts/ci/erasure-check.sh`; mark required erasure with `// erasure: keep <why>`.
7. A `// erasure: keep` marker needs a non-empty reason; a bare marker covers nothing under rule U.
   Accepted placements (`docs/WIDGETS_CODE_STANDARDS.md`): on the call's own line, alone on the
   line above it, on the first line of an enclosing `vec![..]`, `let` or `match`, or on the
   enclosing fn signature (the `fn` line, its `->` line, or alone above the `fn`). rustfmt-stable
   forms: at the end of the `->` line with the `{` on the next line, at the end of the `any(..)`
   line, or as the first own-line comment in the body (where rustfmt moves a marker written after
   the signature's `{`), which counts as a signature marker.
8. An app scaffolded by 0.5.0 (yanked) also follows the 0.5.0 machine-path migration (see
   `scaffold-0.5.0-path-dependency` in `docs/LIMITATIONS.md`).

## 0.5.2 — 2026-10-05

Template, top-app-bar and wording release (`git log v0.5.1..v0.5.2`, 44 commits).

### Changed

- `Image` layout keeps its aspect ratio when the natural size exceeds the bounds (4f42b8bf, merged
  by 3d2287d4).
- The Material top app bar follows the M3 spec: a bare title starts 16dp from the leading edge,
  `AppBarView` gains `container(AppBarContainer)`, `elevation(u8)` and `safe_area(bool)`, and the
  fixed and search bars consume the top and side window insets themselves; a bar that is not at
  the window top opts out with `.safe_area(false)`. `selection_app_bar`'s contextual branch insets
  the same way (903184fa, 95c92de9, 0dce814d, f12c7da9). The examples that wrapped bars in
  `safe_area` were audited to match (2f61d997, 6d88f1e5, 2b8ee475).
- `frust create`'s default template is a Material counter starter built from components, and the
  clean-signals template is a Material counter feature; both use `InversePrimary` with elevation 2
  and self-inset (806c1814, c4d4e5da, c480f7ec). Generated READMEs describe each architecture's
  own layout and their font/theme snippets match the API (f70a92e9, 42a4d372), and the CLI help,
  wizard and scaffold tests no longer carry notes-app wording (d8e725f3).
- The app root closure is named `build` (was `app_logic`) across the shells, `frust-core`, the
  testing harness and the facade docs, which teach all UI in `Component::build` (d9108ee5,
  94689de4, 5346af2a, f2637e9c). The only public effect is renamed type and value parameters:
  `App<State, Logic>` is `App<State, Build>`, and `new_boxed_app`, `new_boxed_app_with`,
  `run_desktop` and `run_desktop_with` name their closure parameter `build`. No public item was
  added or removed, and type-parameter names are not nameable by callers.
- User-visible text no longer describes the framework as pre-release: the generated Gradle
  settings, `.gitignore`, the auth-session runtime error and README, and the TUI wizard no longer
  name a `frust plugin add` command (there never was one; the TUI's Add Plugin action does this),
  and the plugin READMEs and Gradle module headers show the generated
  `frustLocalDir("frust.plugin.<module>.dir")` form read from the gitignored `local.properties`
  instead of `file("<frust checkout>/plugins/...")` includes (d350ec1b, PR #15 / 383444e6).

### Release

- Version bump to 0.5.2, the first release published by `release.yml` through Trusted
  Publishing; the publish step retries a minute apart while crates.io's update rate limit
  applies (a28b2563, PR #16 / 4829391d).
