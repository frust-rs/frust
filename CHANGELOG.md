# Changelog

All notable changes to Frust are recorded here, newest first, in the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) shape. GitHub release notes are copied
from the section for the tag (see [docs/RELEASING.md](docs/RELEASING.md)).

## Unreleased

## 0.6.0 — unreleased

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
- Every public parameter that was typed `AnyView` is now generic (`impl View<S>`, `Option<V>`,
  `impl IntoIterator<Item = impl View<S>>`, `impl Fn(..) -> V`). Callers passing `any(..)` still
  compile, but a bare `None`, an empty `vec![]`, an explicit turbofish call, or a `.collect()`
  straight into a list parameter now needs a type.
- Wrapper views with `type Element = Box<dyn Widget>` that forward to an inner `AnyView` still hide
  inner type swaps from focus bookkeeping (see `focus-wrapper-erasure-swap-blind` in
  `docs/LIMITATIONS.md`).
- Keyed flex lists are all-or-nothing, and the check is a `debug_assert`: a mixed list silently
  falls back to positional reconciliation in release builds (see
  `keyed-list-all-or-nothing-debug-only` in `docs/LIMITATIONS.md`).

### Added

- `AnyView::new` and `any()` are idempotent: erasing an `AnyView` returns it unchanged.
- Fluent `column()`, `row()` and `stack()` builders with `child`, `flex`, `keyed`, `children`,
  `push`, `when` and `when_some`.
- `scripts/codemod/frust_any_codemod.py` migrates an app to the new idiom, including the opt-in
  rule `--t5` that drops `any()` from homogeneous `vec![..]` list arguments.
- `scripts/ci/erasure-check.sh` guards the repository against redundant erasure.
- `docs/PERFORMANCE_BASELINES.md` collects the size and build-time measurements.

### Changed

- Templates, README and rustdoc present erasure at the API boundary and the fluent builders.
- Measured cost of the change: stripped huddle arm64 `.so` +0.12 %, clean release rebuild of
  `frust-gallery` +3.4 %.

### Migration

See the conductor guide "Migrating an app to frust 0.6 boundary erasure (impl View everywhere)"
(PR #27).
