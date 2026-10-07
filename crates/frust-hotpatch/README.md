# frust-hotpatch

The in-app half of Frust hot patching: a native-only runtime that detours calls through a jump
table, so a patch library built from edited code takes effect in a running process without a
restart. It is a frust-owned port of the [subsecond](https://crates.io/crates/subsecond) 0.7.10
runtime and stays wire-compatible with `dx` (dioxus-cli) 0.7.10, which still builds the patches.

`publish = false`: this crate ships only on the hot-patch spike branch. It is reached through
`frust-core`'s opt-in `hotpatch` feature and is absent from every default dependency graph.

## Scope

- `HotFn` / `HotFunction` (`call`, `try_call`, `try_call_with_ptr`, `ptr_address`) and the free
  `call` helper, which retries its closure when a stale inner call unwinds with `HotFnPanic`.
- `apply_patch`, `get_jump_table`, `register_handler`, and `load_patch_library` (the platform
  loader `apply_patch` uses: `libloading` on desktop, memfd + `android_dlopen_ext` on Android).
- `JumpTable` / `AddressMap`: the same serde shape as `subsecond-types` 0.7.10. The
  `wire_compat_with_subsecond_types` test round-trips a `subsecond_types::JumpTable` through JSON
  into this crate's type and back.
- The aarch64 Android pointer-tag handling: lookups strip the top byte and re-apply it.

A runner hands a table received from `dx` to this crate by a serde round-trip (see
`examples/hotpatch-spike/runner/src/main.rs`).

## Debug-only, like subsecond

The jump table is consulted only under `cfg!(debug_assertions)`, exactly as in subsecond 0.7.10: a
release-profile build calls every `HotFn` directly and never reads the table, even with the
`hotpatch` feature on. `apply_patch` itself is not gated; callers keep the devserver connection
debug-only. Moving the gate onto the cargo feature is follow-up work.

## Differences from subsecond 0.7.10

- Native only: all wasm32 code and dependencies (`wasm-bindgen`, `js-sys`, `web-sys`) are gone;
  a wasm32 build fails with a `compile_error!`.
- `load_patch_library` is public, so the Android loader can be probed on its own.
- Fall-through diagnostics: `last_call_fell_through()` (this thread's most recent `HotFn` call
  found a table installed but no entry for its own address) and `fall_through_count()` (misses on
  any thread since the last patch). Dispatch never reads them. They expose the case where a patch
  changes a generic call's type parameters — e.g. a component's `State` type — so the patched
  monomorphisation has an address the running binary never calls and the old code silently keeps
  running. A miss is also normal for any hot function the patch did not recompile.
- No `unwrap()` outside tests: a poisoned handler lock is recovered, and a patch library without
  the `main` anchor returns `PatchError::Dlopen` instead of panicking. `HotFn::call` raises a stale
  call's `HotFnPanic` with `panic_any`, so the payload is the `HotFnPanic` the free `call` catches
  (subsecond's `unwrap()` wrapped it in a string). `try_call` never returns `Err` in either crate,
  so observed dispatch is unchanged.
- ASLR offsets use wrapping arithmetic, and the `main` anchor is cached in an atomic rather than a
  `static mut`.
- The memfd and its pseudo-path are named `frust-hotpatch` instead of `subsecond-patch`.

## Attribution

Every source file is ported from subsecond 0.7.10 and subsecond-types 0.7.10 by DioxusLabs
(Jonathan Kelley), <https://github.com/DioxusLabs/dioxus/tree/main/packages/subsecond>, licensed
MIT OR Apache-2.0 (the published crates' `license` field; the crate packages ship no LICENSE file).
This crate keeps that dual license.
