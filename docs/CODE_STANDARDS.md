# ForgeKit - Code Standards

## Language Idioms

- **`unsafe` is confined to a small set of sanctioned platform-FFI boundaries.**
  Every other crate (`forgekit-core`, `forgekit-scene`, `forgekit-text`,
  `forgekit-widgets`, `forgekit-shell-common`, `forgekit-shell-desktop`,
  `forgekit`) stays `unsafe`-free — where Masonry/xilem-style code would
  reach for `unsafe` downcasting, use trait upcasting instead: bound a trait
  on `Any` (e.g. `Widget: Any`) and downcast through `&mut dyn Any`. The
  sanctioned zones are raw-pointer boundaries a GPU/platform shell cannot
  avoid, each isolated in one function/module with a `# Safety` doc comment
  stating the caller contract:
  - `forgekit-render`'s `create_android_surface`/`create_metal_surface`
    (`lifecycle.rs`) and the `on_surface_created_from_android_window`/
    `on_surface_created_from_metal_layer` renderer methods (`renderer.rs`)
    — turn a caller-owned raw `ANativeWindow*`/`CAMetalLayer*` into a
    `wgpu::Surface`.
  - `forgekit-shell-android`'s `jni_glue` module — the JNI FFI boundary
    (`extern "system"` exports, `Box::into_raw`/`from_raw` for the opaque
    native handle, `ANativeWindow_fromSurface`) — plus the
    `#[unsafe(no_mangle)]` attributes the `android_app!` macro emits on its
    generated exports.
  - `forgekit-shell-ios`'s `ffi_glue` module — the C-ABI FFI boundary
    (`extern "C"` exports, `Box::into_raw`/`from_raw` for the opaque native
    handle, the call into `on_surface_created_from_metal_layer`) — plus the
    `#[unsafe(no_mangle)]` attributes the `ios_app!` macro emits on its
    generated exports.
- **No unwind across FFI.** Every platform export both shells define routes
  through `forgekit-shell-common`'s `guard` helper, which `catch_unwind`s and
  logs, returning a benign default instead of unwinding into JVM- or
  Swift-owned stack frames — a panic crossing the FFI boundary is undefined
  behavior, not just a bug.
- **Type-erase to avoid a downstream crate dependency**, not to avoid writing
  a type. When a lower layer needs to thread a resource owned by a higher
  layer (e.g. `forgekit-core`'s `LayoutCtx` carrying the shell's
  `forgekit-text::TextContext`), pass it as `&mut dyn Any` rather than adding
  the dependency, and recover it at the one call site that knows the
  concrete type with a documented, panic-on-mismatch `downcast_mut::<T>()`
  — the panic message should say this is a wiring bug, not a runtime-data
  condition.
- **Edition-2024 `-> impl Trait` return types capture all in-scope
  lifetimes by default.** When a function returns an `impl Trait` that
  borrows nothing from its parameters (e.g. `app_logic(&mut State) -> impl
  View<State>`, where views are `'static`), opt out explicitly:

  ```rust
  fn app_logic(state: &mut AppState) -> impl forgekit::View<AppState> + use<> {
      forgekit::text(state.greeting.clone()).size(32.0)
  }
  ```

## Error Handling

- **`anyhow::Result` in binaries and integration-facing library code**
  (`forgekit-cli` commands, `forgekit-render`'s public API,
  `forgekit-shell-desktop`/`forgekit`'s `run`), with `.context()` /
  `.with_context()` at each fallible step so the error chain names what was
  being attempted (e.g. `"failed to spawn `{cmd}`"`, `"reading `{path}`"`).
- **`thiserror`-derived enums for errors callers match on**, i.e. where the
  error is part of a library's API contract rather than a leaf failure
  report:

  ```rust
  #[derive(thiserror::Error, Debug, PartialEq, Eq)]
  pub enum BuildInfoError {
      #[error("--debug, --profile, and --release are mutually exclusive")]
      ConflictingModes,
      #[error("invalid --define '{0}': expected KEY=VALUE with a non-empty key")]
      InvalidDefine(String),
  }
  ```

## Naming Conventions

| Element | Convention | Example |
|---|---|---|
| Crate names | `forgekit-<layer>` | `forgekit-scene`, `forgekit-shell-desktop` |
| Fallible constructor errors | `<Type>Error` enum, `thiserror`-derived | `BuildInfoError`, `NameError` |
| Test-only fakes | `Fake<Trait>` | `FakeProcessRunner`, `FakeEnv` |
| Widget pairs | `<Name>View` (declarative) / `<Name>Widget` (retained) | `TextView` / `TextWidget` |
| JNI exports | `Java_<fixed_package>_<FixedClass>_native<Name>` | `Java_dev_forgekit_ForgeKitSurfaceView_nativeOnFrame` |

JNI export names are LAW: the package/class (`dev.forgekit.ForgeKitSurfaceView`)
is fixed across every generated app, not app-specific, so the mangled symbol
stays stable regardless of the app's own package.

## Anti-patterns

### Shelling out directly instead of through `ProcessRunner`

**BAD:**
```rust
let out = std::process::Command::new("rustc").arg("--version").output()?;
```
Untestable without actually invoking `rustc`, and every caller re-implements
its own spawn-failure handling.

**GOOD:**
```rust
fn validate(&self, ctx: &DoctorCtx) -> Validation {
    match ctx.runner.run("rustc", &["--version"]) { /* ... */ }
}
```
Every external tool invocation (`rustc`, `adb`, `xcrun`, `cargo ndk`, …) goes
through the `ProcessRunner` trait, so `doctor`/`devices` logic is exercised
in `cargo test` with `FakeProcessRunner` and never shells out during a test
run.

### Leaking `vello`/`wgpu` types outside `forgekit-render`

**BAD:** a `forgekit-scene` or `forgekit-text` public function taking or
returning a `vello::*`/`wgpu::*` type.

**GOOD:** public APIs above `forgekit-render` speak only `kurbo`/`peniko`;
this is what lets the GPU backend be swapped later without touching widget
or text code.

## Testing Patterns

- **Fixture-driven tests for parsers/validators**: register canned
  `ProcessRunner`/`EnvLookup` responses (`FakeProcessRunner::with(...)`,
  `.missing(...)`, `FakeEnv::set(...)`) keyed by the exact invocation, then
  assert the resulting `Status`/`Validation`. This is the pattern used
  throughout `forgekit-cli`'s `doctor`/`devices` validators.
- **`#[ignore = "<reason>"]` for GPU-dependent or slow end-to-end tests.**
  The reason string must say how to run it (`cargo test -p ... --
  --ignored`) and why it's excluded by default (needs a real GPU; compiles a
  full generated dependency graph; etc.) — see
  `forgekit-render/tests/gpu_smoke.rs` and
  `forgekit-cli/tests/create_e2e.rs`.
- **Recording fakes for paint assertions**: a minimal `PaintScene`
  implementation that pushes `(origin, size)`/`(origin, text)` tuples into
  `Vec`s lets widget `layout`/`paint` behavior be asserted without any GPU
  or `forgekit-render` dependency (see `forgekit-core::widget` and `app`
  unit tests).
- **Template rendering uses `minijinja::UndefinedBehavior::Strict`**: an
  unresolved `{{ placeholder }}` is a hard render-time error rather than a
  silently emitted `undefined`, so template/context drift is caught by the
  test suite instead of shipping into a generated project.
