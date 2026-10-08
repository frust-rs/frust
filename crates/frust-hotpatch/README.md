# frust-hotpatch

The in-app half of Frust hot patching: a native-only runtime that detours calls through a jump
table, so a patch library built from edited code takes effect in a running process without a
restart. It is a frust-owned port of the [subsecond](https://crates.io/crates/subsecond) 0.7.10
runtime and stays wire-compatible with `dx` (dioxus-cli) 0.7.10, which still builds the patches.

The crate is published with the workspace so that `frust-core`'s optional dependency on it
resolves from crates.io. It is reached only through `frust-core`'s opt-in `hotpatch` feature and is
absent from every default dependency graph.

## Scope

- `HotFn` / `HotFunction` (`call`, `try_call`, `try_call_with_ptr`, `ptr_address`) and the free
  `call` helper. Every call is one jump-table lookup keyed on the address of the function's
  `call_it` monomorphisation, which is sound for any `FnMut` (fn item, closure of any size, fn
  pointer). `HotFn::from_fn_ptr` keys a plain `fn(A, ..) -> R` on its own value instead; its sealed
  `FnPointer` bound is the only way into that path. There is no stale-call detection and no retry:
  a panic propagates untouched, and code already running when a patch lands finishes as old code.
- Sound dispatch means the key names the right function, not that the patched function is safe to
  call. `apply_patch`'s contract covers that: the table must be built against this exact
  executable, **and** every mapped function's argument, return and capture types must keep their
  layout between the running image and the patch. Symbol names do not encode layout, so a field
  added to a component's `State` keeps the entry and breaks the second condition (RESULTS.md row
  D2 in the spike); checking it is the patch builder's job (the spike's PORT.md, section 2(c)).
- `apply_patch`, `get_jump_table`, `register_handler`, and `load_patch_library` (the platform
  loader `apply_patch` uses: `libloading` on desktop, memfd + `android_dlopen_ext` on Android).
- The app-owned anchor: the app defines `#[unsafe(no_mangle)] pub extern "C" fn
  __frust_hotpatch_anchor() {}` and registers it once with `set_anchor(__frust_hotpatch_anchor as
  usize)` (a function-pointer address, no `dlsym`). The builder sends the symbol's link-time
  address as `aslr_reference` and exports the same symbol from every patch, so `apply_patch`
  resolves the patch's anchor by that name (`ANCHOR_SYMBOL`), not `main`: an Android cdylib has no
  `main`. A patch library without the symbol is refused with `PatchError::Dlopen`.
- Anchor consistency: `apply_patch` checks that `aslr_reference` and `new_base_address` really are
  the link-time addresses of `__frust_hotpatch_anchor` in the base and patch images (the offset
  each implies must equal the image's slide) and otherwise refuses with
  `PatchError::AnchorMismatch`, installing nothing. Today's `dx` 0.7.10 tables are anchored on
  `main`, so this runtime refuses them; a builder must anchor on the symbol. The patch-side
  refusal happens after the library is mapped, which is never unloaded.
- `apply_from_devtools(bytes_path, table) -> ApplyReport`: the safe entry the in-app devtools
  service calls, so the one `unsafe` call stays in this crate. `bytes_path` must be a file the app
  wrote from bytes received on the authenticated devtools connection, never a wire path. It
  refuses while any layout-mismatch record is unreported: nothing is loaded and the report says
  `applied: false` with the records. The refusal lives in `apply_patch` itself, under its lock and
  before any load (`PatchError::LayoutMismatchPending(records)`), so every apply entry, the raw one
  included, refuses until the host calls `mark_layout_mismatches_reported`.
- `report_layout_mismatch(type_name, stored, own)`: records a layout disagreement found at run
  time and keeps it until reported. `pending_layout_mismatches()` reads the list (for
  `hotpatch_info`); `mark_layout_mismatches_reported(&records)` removes the records a
  `PatchOutcome` or `hotpatch_info` answer carried (records reported meanwhile stay pending).
- `seam_hits()` and `missed_keys()`: hits and distinct missed keys since the last patch (see
  below).
- `JumpTable` / `AddressMap`: the same serde shape as `subsecond-types` 0.7.10. The
  `wire_compat_with_subsecond_types` test round-trips a `subsecond_types::JumpTable` through JSON
  into this crate's type and back.
- The aarch64 Android pointer-tag handling: lookups strip the top byte and re-apply it to the
  patched address (the masking helper is compiled and tested on every host).

A runner hands a table received from `dx` to this crate by a serde round-trip (see
`examples/hotpatch-spike/runner/src/main.rs`).

## The patch boundary

Nothing rewrites process memory: the table only redirects calls that enter through a `HotFn`.
Patch code is reached without one too. A trait object, stored closure or fn pointer created while
patched code ran points straight into the patch library, so calling it later runs that code with no
`HotFn` in between, even after a newer patch. Such values created before a patch keep running the
old code until something rebuilds them. A patch that changes a generic call's type parameters (e.g.
a component's `State`) produces a monomorphisation the running binary never calls, so the old code
keeps running; the fall-through diagnostics below expose that.

## Debug-only, like subsecond

The jump table is consulted only under `cfg!(debug_assertions)`, exactly as in subsecond 0.7.10: a
release-profile build calls every `HotFn` directly and never reads the table, even with the
`hotpatch` feature on (`try_call_with_ptr`, which calls the address it is given, is the one call
that honours its pointer in every profile). `apply_patch` is gated too: a release-profile
build returns `PatchError::ReleaseBuild` before touching anything, so it refuses to load a patch
whatever the callers do.

## Differences from subsecond 0.7.10

- Native only: all wasm32 code and dependencies (`wasm-bindgen`, `js-sys`, `web-sys`) are gone;
  a wasm32 build fails with a `compile_error!`.
- No pointer-size heuristic. subsecond treats any `F` as large as a fn pointer as one and
  transmutes its bytes, which misdispatches a pointer-sized capturing closure; here only
  `HotFn::from_fn_ptr` keys on a pointer's value, and `HotFunction` has no `call_as_ptr`.
- No `HotFnPanic` and no retry loop: nothing in either crate ever produced one. `try_call` and
  `try_call_with_ptr` return `Result<_, Infallible>`.
- Ordering: the table is published with a Release store and read with an Acquire load, so a
  reader on another thread sees a fully built map (subsecond uses `Relaxed`). The anchor
  is an atomic rather than a `static mut`.
- Serialisation: `apply_patch` holds a lock end to end (load, rebase, publish, handlers), so
  concurrent patches apply one at a time. A handler must not apply a patch itself.
- Fail-closed anchor: while no anchor is set, `apply_patch` returns `PatchError::AnchorUnresolved`
  before loading the library; nothing is installed, no handler runs. The anchor is the app's
  `__frust_hotpatch_anchor`, not `main`; `aslr_reference` reads the registered value and retries
  an unset one rather than caching it.
- Handlers run under `catch_unwind`: a panicking handler is logged to stderr, the patch stays
  installed, and the remaining handlers still run.
- The Android memfd stays owned until `android_dlopen_ext` succeeds, so a failed patch closes it
  instead of leaking a descriptor.
- `load_patch_library` is public, so the Android loader can be probed on its own.
- Fall-through diagnostics: `last_call_fell_through()` (this thread's most recent `HotFn` call
  found a table installed but no entry for its own key) and `fall_through_count()` (misses on
  any thread since the last patch, reset by every patch). Dispatch never reads them. Beside them
  `seam_hits()` counts table hits, and `missed_keys()` lists each distinct missed key once as
  `MissedKey { image, link_address }`: `image` 0 is the base executable and `n` the n-th patch
  loaded (found from the address ranges of the images this crate loaded; an address in none is
  `UNKNOWN_IMAGE`), `link_address` the key minus that image's slide. This crate only records; the
  host classifies. Both reset with every patch. A miss is
  also normal for any hot function the patch did not recompile, and for every call made from
  patch-image code: the key is the calling image's own `call_it` address and the table's keys are
  base-image addresses, so a nested component's hot call from the newest patch's rebuild misses
  although it already runs the newest code. The count stays a plain count; a restart rule reads the missed keys.
- No `unwrap()` outside tests: a poisoned handler or apply lock is recovered, and a patch library
  without the anchor symbol returns `PatchError::Dlopen` instead of panicking.
- ASLR offsets use wrapping arithmetic.
- The memfd and its pseudo-path are named `frust-hotpatch` instead of `subsecond-patch`.

## Attribution

Every source file is ported from subsecond 0.7.10 and subsecond-types 0.7.10 by DioxusLabs
(Jonathan Kelley), <https://github.com/DioxusLabs/dioxus/tree/main/packages/subsecond>, licensed
MIT OR Apache-2.0 (the published crates' `license` field; their packages ship no LICENSE file).
This crate keeps that dual license; the dispatch, ordering, serialisation and fail-closed changes
listed above are frust's.
