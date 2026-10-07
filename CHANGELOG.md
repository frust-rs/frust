# Changelog

All notable changes to Frust are recorded here, newest first, in the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) shape. GitHub release notes are copied
from the section for the tag (see [docs/RELEASING.md](docs/RELEASING.md)).

## Unreleased

## 0.6.0 — 2026-10-07

Type erasure moved from the call site to the API boundary: public builders take generic views and
erase internally, and views are composed with fluent builders instead of `any()` everywhere.

### Breaking

- `Component::build` returns `impl View<Self::State>` instead of `AnyView<Self::State>`. An impl
  that keeps `-> AnyView<..>` warns (`refining_impl_trait`) and fails under `-D warnings`.
- `Component` is no longer dyn-compatible: `dyn Component` and `Box<dyn Component>` do not
  compile. Use generics, or store the `AnyView`s a component produces.
- Driver closures must erase the build result with `AnyView::new(root.build(state))`; the opaque
  return captures the `&self` and `&mut State` borrows and cannot escape the closure otherwise.
  `frust::run`, `frust::app!` and the templates already do this.
- Public parameters that were typed `AnyView` are generic (`impl View<S>`, `Option<V>`,
  `impl IntoIterator<Item = impl View<S>>`, `impl Fn(..) -> V`), with two exceptions that keep
  erasure because they store or lazily produce views: `frust_material::list_view`'s item builder
  (`impl Fn(usize) -> AnyView<State>`) and beui's `ChatConversationView::extra_rows`
  (`Fn() -> Vec<AnyView<State>>`). Callers passing `any(..)` still compile, but a bare `None`, an
  empty `vec![]`, an explicit turbofish call, or a `.collect()` straight into a list parameter now
  needs a type.
- Wrapper views with `type Element = Box<dyn Widget>` that forward to an inner `AnyView` still hide
  inner type swaps from focus bookkeeping (see `focus-wrapper-erasure-swap-blind` in
  `docs/LIMITATIONS.md`).
- Keyed flex lists are all-or-nothing, and the check is a `debug_assert`: a mixed list silently
  falls back to positional reconciliation in release builds (see
  `keyed-list-all-or-nothing-debug-only` in `docs/LIMITATIONS.md`).

### Added

- `AnyView::new` and `any()` are idempotent: erasing an `AnyView` returns it unchanged.
- Fluent builders: `column()` and `row()` with `child`, `flex`, `keyed`, `children`, `push`,
  `when` and `when_some`; `stack()` with `child`, `children`, `when` and `when_some`.
- `scripts/codemod/frust_any_codemod.py` migrates an app to the new idiom, including the opt-in
  rule `--t5` that drops `any()` from homogeneous `vec![..]` list arguments.
- `scripts/ci/erasure-check.sh` guards the repository against redundant erasure (rules T1–T5 in
  strict mode) and runs as a CI hygiene step; a site that must keep `any()` carries a
  `// erasure: keep <why>` comment the tool honours.
- `docs/PERFORMANCE_BASELINES.md` collects the size and build-time measurements.

### Changed

- Templates, README and rustdoc present erasure at the API boundary and the fluent builders.
- Measured cost of the change (medians of 3 on one host, main 79686b68 → f79b739c; details in
  `docs/PERFORMANCE_BASELINES.md`): stripped arm64 `.so` huddle +0.12 %, material3-demo +0.34 %;
  clean release rebuild frust-gallery +1.6 % (an earlier run at the pull-request head measured
  +3.4 %), material3-demo −2.9 %. Build-time deltas of this size sit inside the host's run-to-run
  noise; the size figures are exact.

### Migration

1. Change every `impl Component` whose `build` returns `AnyView<..>` to `-> impl View<Self::State>`;
   the body may keep `any(..)` for now.
2. Where the app writes its own driver closure, erase the build result there:
   `move |s| AnyView::new(root.build(s))` (`frust::run`, `frust::app!` and the templates already
   do).
3. Run `python3 -I scripts/codemod/frust_any_codemod.py --write <src dirs>` (add `--t5` to also
   unwrap homogeneous `vec![any(..), ..]` list arguments), then `cargo fmt`.
4. Give a type to any bare `None`, empty `vec![]`, turbofish call or `.collect()` the compiler now
   rejects; a same-head list whose elements differ in type keeps `any()` with a
   `// erasure: keep <why>` comment.
5. Replace `dyn Component` with generics or stored `AnyView`s; see `docs/LIMITATIONS.md` for the
   two behaviours listed under Breaking.
