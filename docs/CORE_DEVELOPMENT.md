# Frust - CORE Development

Version pins owned by the CORE unit (`frust-core`, `frust-scene`, `frust-reactive`,
`frust-paths`, and the `frust` facade). Shared prerequisites, build/run commands, the standard
verify gate, and the version-pin *policy* live in [DEVELOPMENT.md](DEVELOPMENT.md); the unit's
design lives in [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md).

## Version Pins

The pins this unit owns, under [DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy (pins are
LAW; re-run the row's tripwire after touching it, and never run a blind `cargo update`):

| Pin | Why | Tripwire |
|---|---|---|
| `reactive_graph 0.2` / `any_spawner 0.3` / `tokio 1` (no default features), minor | `frust-reactive` substrate, pre-1.0 Leptos-ecosystem churn expected; never enable `reactive_graph`'s `effects` feature — the frame path is a custom subscriber, not `RenderEffect` (see [ARCHITECTURE.md](ARCHITECTURE.md)'s Key Types) | `cargo test -p frust-reactive` |
| `clean-signals` (crates.io, `"0.1"`) | Consumed by `examples/huddle`, `plugins/clean-signals-frust` (dep + `test-fixtures` dev-dep), and `crates/frust-drive/templates/app`'s clean-signals scaffold variant — all sites must declare the identical version requirement (the type-identity rule in [DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy). Never enable its `effects` feature (`reactive_graph/effects`, same prohibition as the `reactive_graph` row above) — confirm it stays disabled before bumping | `cargo generate-lockfile` + `cargo build --locked` in each of `examples/huddle`/`plugins/clean-signals-frust` |
| `accesskit 0.25` minor + adapters (`accesskit_winit 0.34`, `accesskit_android 0.9` minor, `accesskit_ios =0.2.2` exact) | `frust-core`'s semantics-pass vocabulary; `accesskit_ios` is younger/less proven, compile-gate only ([DEVELOPMENT.md](DEVELOPMENT.md)'s Test section) | `cargo test -p frust-core semantics` |
| `libloading 0.8.8` (`frust-hotpatch`, `cfg(any(unix, windows))`) | The dlopen/`LoadLibrary` patch loader; the patch library's initialisers run on load | `cargo test -p frust-hotpatch` and `bash scripts/ci/hotpatch-feature-gate.sh` |
| `memmap2 0.9.8` / `memfd 0.6.4` (`frust-hotpatch`, `cfg(target_os = "android")`) | The memfd + `android_dlopen_ext` patch loader; versions match subsecond 0.7.10's own requirements, so they move together with the subsecond/dx version `frust-hotpatch` is ported from | `cargo ndk -t arm64-v8a build -p frust-hotpatch` |
| `subsecond-types =0.7.10` (`frust-hotpatch` dev-dependency, exact) | The wire-compat test round-trips this crate's JSON jump-table shape, which dx 0.7.10 speaks; bumps in lockstep with the `memmap2`/`memfd` row | `cargo test -p frust-hotpatch wire_compat` |

`kurbo`/`peniko` reach app code through the facade's whole-crate valves rather than a
re-declared dependency ([CODE_STANDARDS.md](CODE_STANDARDS.md)'s State & Reactivity
Conventions); they are pinned once in the root `[workspace.dependencies]`.

## Layout-Skip and Retained-Layer Knobs

Read by the mobile/desktop/web shells (`frust-shell-common::frame_gate::LayoutKnobs`), compile-time
or runtime; each shell logs one info line `layout skip=.. audit=..` at startup. Design:
[CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) Data Flow.

| Variable | Effect |
|---|---|
| `FRUST_NO_LAYOUT_SKIP` | Any value calls `RenderRoot::set_layout_skip(false)`: every pass lays the whole tree out. First A/B valve for a suspected stale-geometry report |
| `FRUST_LAYOUT_AUDIT` | Any value other than `0` calls `set_layout_audit(true)`: each skipped pass is compared against a forced one and mismatches log `layout-audit:` warnings (debug aid, doubles layout cost) |
| `FRUST_NO_RETAINED_LAYERS` | Any value calls `RenderRoot::set_retained_layers(false)`: `PaintCtx::subtree_clean` answers `false` everywhere, so `repaint_boundary`, navigator pages and cached opacity paint plain bodies and the scene never carries `PushRetained`/`DrawRetained`. Engine-side counterpart: `FRUST_ENGINE_NO_LAYERS` ([RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)) |

## Async work in an app

Frust runs its own multi-threaded tokio runtime on every non-wasm target, Android and iOS
included. The facade does not expose its handle. Use these entry points instead:

- `frust::use_task`: run work off the UI thread and deliver the result to a signal on the UI
  thread. This is the supported way to load data.
- `frust::spawn`: run a `Send` future on the runtime.
- `frust::spawn_blocking`: run blocking or CPU-bound work.
- `frust::spawn_local` and `frust::spawn_local_scoped`: run a `!Send` future on the UI thread.

Signals are written and read on the UI thread. Results cross threads through `use_task` or a
channel.

A scoped future or `frust::time::Timer` started in a component's event handler is bound to that
component and stops when the component is removed (see
[CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md), "Contexts are visible inside both build and event
passes"). Work that must outlive a page is started under a longer-lived owner or with
`frust::spawn`.

An app that needs its own runtime, for example for a long-lived gRPC channel, may create one on
its own thread. The framework does not pause or resume it. Register an `AppLifecycleListener`
(re-exported by `frust`; `frust::app_lifecycle()` reads the state) and close or re-establish
connections in `on_hide` and `on_resume`. The shell calls these on the UI thread, so hand the work
to your runtime rather than blocking there. Timers do not fire while the app is backgrounded
(`time-no-firing-while-backgrounded` in [LIMITATIONS.md](LIMITATIONS.md)), and iOS never reports
`Detached` (`app-lifecycle-ios-no-detached`).

```rust
let _listener = frust::AppLifecycleListener::new()
    .on_hide(|| { /* close the channel */ })
    .on_resume(|| { /* reconnect */ });
```

Keep the listener alive: dropping it unregisters it.

## Start-up work before the first screen

`app!` setup and `Component::init` run synchronously on the UI thread, before the first frame.
Opening a database, running migrations or reading a store there blocks that thread, and the user
sees nothing until it returns. Do that work behind `frust::StartGate`: it runs an async task, shows
a splash view while the task is pending, an error view if it fails, and builds the real UI from the
task's value once it succeeds.

```rust
use frust::{Component, StartGate, View, spawn_blocking, text};

#[derive(Default)]
pub struct App;

impl Component for App {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        StartGate::new(|| async {
            // Blocking plugin calls belong on the blocking pool.
            spawn_blocking(open_and_migrate)
                .await
                .map_err(std::io::Error::other)?
                .map_err(|e| std::io::Error::other(e.to_string()))
        })
        .splash(text("Starting"))
        .ready(|_db: &frust_database::Database| text("Ready"))
    }
}
```

`open_and_migrate` is the app's own function returning
`Result<frust_database::Database, frust_database::DatabaseError>`. It calls
`Database::open(name)` and then `db.migrate(&[Migration { to_version, statements }])`; both are
synchronous. The secure-storage store's `open(name)` (`frust-secure-storage`) is synchronous in the same way.

- `StartGate::new` takes a closure returning a future of `Result<T, E>`, where `E` is a
  `std::error::Error + Send + Sync`. The task starts when the gate is mounted and is aborted when
  the gate is dropped.
- `.splash(view)`, `.error(|message: &str, retry: Retry| view)` and `.ready(|value: &T| view)`
  each produce a `View<()>`, so they have no app state of their own. A nested `component(..)`
  carries state.
  Without `.error`, the gate shows the message and a Retry button. `retry.run()` restarts the task
  and returns to the splash.
- The `ready` closure runs once per success, and the subtree it returns is retained across
  unrelated rebuilds. State created inside it is disposed when the gate swaps arm or is dropped.
- The gate does not hold the system launch screen. The splash is the app's first frame, so give it
  the launch screen's background colour to avoid a visible flash.
- Create the `Router` inside the ready view, in the `init` of a nested component, with
  `router_with_deep_links`. A cold-start link pushed with `push_deep_link` while the gate is still
  loading wins over the router's default route, and later links navigate normally. The tests push
  the link after the test runtime exists and before the first pump, so they do not exercise a
  link delivered earlier in real process start-up.

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) — the unit's design
