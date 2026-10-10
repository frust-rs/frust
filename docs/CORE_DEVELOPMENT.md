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
| `accesskit 0.24` minor + adapters (`accesskit_winit 0.33`, `accesskit_android 0.7` minor, `accesskit_ios =0.1.2` exact) | `frust-core`'s semantics-pass vocabulary; `accesskit_ios` is younger/less proven, compile-gate only ([DEVELOPMENT.md](DEVELOPMENT.md)'s Test section) | `cargo test -p frust-core semantics` |
| `libloading 0.8.8` (`frust-hotpatch`, `cfg(any(unix, windows))`) | The dlopen/`LoadLibrary` patch loader; the patch library's initialisers run on load | `cargo test -p frust-hotpatch` and `bash scripts/ci/hotpatch-feature-gate.sh` |
| `memmap2 0.9.8` / `memfd 0.6.4` (`frust-hotpatch`, `cfg(target_os = "android")`) | The memfd + `android_dlopen_ext` patch loader; versions match subsecond 0.7.10's own requirements, so they move together with the subsecond/dx version `frust-hotpatch` is ported from | `cargo ndk -t arm64-v8a build -p frust-hotpatch` |
| `subsecond-types =0.7.10` (`frust-hotpatch` dev-dependency, exact) | The wire-compat test round-trips this crate's JSON jump-table shape, which dx 0.7.10 speaks; bumps in lockstep with the `memmap2`/`memfd` row | `cargo test -p frust-hotpatch wire_compat` |

`kurbo`/`peniko` reach app code through the facade's whole-crate valves rather than a
re-declared dependency ([CODE_STANDARDS.md](CODE_STANDARDS.md)'s State & Reactivity
Conventions); they are pinned once in the root `[workspace.dependencies]`.

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) — the unit's design
