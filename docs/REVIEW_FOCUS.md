<!-- Starter draft seeded by agent; curate freely. -->
# Frust - Review Focus

## Review Priorities

1. **Cross-thread + FFI correctness in the shells and render split.**
   `frust-shell-common::render_split`, and the sanctioned-unsafe zones in
   `frust-shell-android::jni_glue` / `frust-shell-ios::ffi_glue`, hand frame state across the
   render-thread boundary. A race near surface destroy/recreate is the costliest regression class
   in this codebase — weight any touch to these paths heavily regardless of diff size.
2. **Feature-flag matrix integrity.** The three design-system catalogs are opt-out via
   `frust-widgets`/`frust-theme` default features, and platform FFI deps are target-gated. A
   workspace-wide build silently re-unifies features back on, so the only real catalog-off gate is
   `cargo build -p no-catalogs` as its own standalone build. Check default-feature wiring changes
   against that gate, not against `cargo build --workspace`.
3. **Version-pin and hand-sync discipline in manifests.** The `vello`/`wgpu` pins are coupled and
   must move together. The `[profile.*]` blocks are hand-synced across the root `Cargo.toml`,
   `templates/app/Cargo.toml.tmpl`, and `examples/huddle/Cargo.toml` — there is no single source of
   truth, only the tripwire test `cargo test -p frust-cli --test profile_sync`. Flag any manifest
   edit that touches one copy without the others.
4. **Plugin backend parity.** A new public API surface on the shared-preferences, secure-storage,
   or camera plugin must extend the shared cross-platform conformance suite, not land against a
   single platform backend. A one-sided addition is a parity gap, not a complete feature.
5. **Frame-gate/pacing semantics.** `frust-shell-common::frame_gate` decides whether a frame runs
   or skips; a wrong decision presents as a silent stuck UI or a battery regression, not a crash.
   Every gating seam must preserve its kill-switch env var revert path (e.g.
   `FRUST_NO_RENDER_THREAD`) — removing one is a regression even if the normal path still works.

## Known Hot Spots

| Area | Why fragile |
|------|-------------|
| `frust-shell-common::render_split` + `frust-render` surface/present handoff | The surface can be destroyed at any point in the Ack/barrier handoff; lifecycle edges (backgrounding, rotation, view recreation) are where deadlocks and races actually surface. |
| `frust-shell-common::frame_gate` + animation pacing | Skip decisions aren't recorded across the render-split channel, so skip-sensitive measurements need `FRUST_NO_RENDER_THREAD` to be trustworthy; a wrong skip looks like a hang, not a test failure. |
| `frust-shell-common::surface_mode` | Single-writer contract enforced only by a source-scan conformance test, not the type system. Translucency refusal on the blit path is a shipped, accepted limitation (see `docs/LIMITATIONS.md` `cam-blit-opaque`) — don't re-flag the refusal itself, only regressions to the surrounding contract. |
| Widget layout-skip handling (`frust-widgets`) | Containers combining cached layout with conditional children have a history of skip-related bugs. Any change to a container's child-conditional logic needs its layout-skip path re-verified, not just its happy path. |
| Reactive tracking (`frust-reactive`, `TrackedScope` consumers) | An untracked read of a render-relevant signal is a silent wake hazard — the UI doesn't update until some unrelated write happens to wake it. New rebuild-path reads need explicit `TrackedScope` coverage, not just a passing host test. |
| `frust-native-widgets` (`plugins/native-widgets`) FFI surface | JNI export symbol names and Kotlin package/class names are frozen once shipped; evolution must be additive only. There is exactly one generic factory/listener per platform — a new per-control glue path is an architectural regression, not a convenience. |
| `frust-drive` cores + `frust-mcp` | Print-free contract: a stray `print!`/`println!` garbles the TUI's raw-mode terminal, since both crates are linked into `frust-tui`. Each crate carries its own `print_free_cores` tripwire test (`crates/frust-drive/tests/`, `crates/frust-mcp/tests/`); all process spawns must pipe stdout/stderr, never inherit. |
| `frust-widgets` authoring boundary | Baseline widgets must never import `material`/`cupertino`/`glyph`; all container plumbing goes through the authoring module, enforced by the `authoring_only_conformance` source-scan test. A baseline widget reaching into a catalog is a boundary violation regardless of how small. |
| Theme resolution (`frust-theme` / `Theme` recovery) | Precedence is explicit builder value > theme token > unthemed fallback. A hardcoded metric or color where a token already exists is a defect. `frust-core`/`frust-widgets` must never call `Instant::now()` directly — time enters only as shell-supplied `FrameTime`. |
| `frust-database`'s turso bridge (`plugins/database/src/turso.rs`) | The tier's first plugin-owned OS thread + tokio runtime; a caller-thread `block_on` would panic under `panic="abort"`, so the bridge sends futures across a channel instead. The `AsyncContext` guard is a provable-subset detector (`try_current`+`try_id`), not exhaustive — a change to how it's computed can silently widen or narrow what it catches without any test failing outside the two cases already pinned. |

## Severity Calibration

- **Critical:** on-device visual or input regressions — device gates are the arbiter here, a green
  host-side run does not clear a device-gated change. Independent version bumps of the coupled
  rendering-stack pins (`vello`/`wgpu`). A renamed or removed frozen FFI symbol in `native-widgets`.
  Data loss in the shared-preferences or secure-storage plugin. Any blocking or parking call on the
  platform UI thread (typed fail-fast + `spawn_blocking` is the contract). A `render_split`
  deadlock.
- **Major:** a catalog/feature-gating regression only catchable by a standalone gate (e.g.
  `no-catalogs`), not by the normal workspace build. Removing or bypassing a kill-switch seam.
  Weakening a source-scan conformance test (`authoring_only_conformance`, `print_free_cores`,
  `profile_sync`, `surface_mode`'s single-writer check) instead of fixing the underlying violation.
  Target-gated code that no longer compiles under the Android or iOS compile gate.
- **Minor:** style/naming nits. Doc drift that's still within its budget. A gap covered only by a
  host-side test where a device gate already exists for the same behavior.

## Out of Scope

- Benchmark median deltas without tail data — in this project the tails carry the regression
  signal; a median-only perf claim is not a finding.
- The `clean-signals`/`clean-signals-frust` strings and the sibling-checkout path dependencies
  around them — load-bearing compatibility surface, not typos to "fix".
- `workflow/` — a separate nested repo, not part of this build; out of scope entirely.
- Generated Android/iOS project trees under `examples/*` — scaffold output. Review the templates
  in `frust-drive` instead of the generated artifacts.
- Anything already entered in `docs/LIMITATIONS.md` — accepted, measured degrades. Do not re-flag
  them; cite the id if a change is adjacent to one.
