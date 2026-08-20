<!-- Starter draft seeded by agent; curate freely. -->
# Frust - Review Focus

## Review Priorities

1. **Cross-thread + FFI correctness in the shells and render split.**
   `frust-shell-common::render_split`, and the sanctioned-unsafe zones in
   `frust-shell-android::jni_glue` / `frust-shell-ios::ffi_glue`, hand frame state across the
   render-thread boundary. A race near surface destroy/recreate is the costliest regression class
   in this codebase — weight any touch to these paths heavily regardless of diff size.
2. **Feature-flag matrix integrity.** `frust` carries `default = []` — the three design systems are
   ordinary sibling plugin crates (`frust-glyph`/`frust-material`/`frust-cupertino`), not a catalog
   feature to opt out of. Platform FFI deps are still target-gated, and a workspace-wide build still
   silently re-unifies any Cargo feature back on across every member — the remaining live risk is a
   *new* cargo feature reintroducing that hazard, not the design-system catalogs (which no longer
   have one). Check any new default-feature wiring against a package-scoped
   `cargo build -p <crate> --no-default-features`, not against `cargo build --workspace`.
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
| Widget layout-skip handling (`frust-widgets`) | Containers combining cached layout with conditional children have a history of skip-related bugs. Any change to a container's child-conditional logic needs its layout-skip path re-verified, not just its happy path. The same class recurs in design-system plugins: an animated-height widget (`frust_glyph`'s accordion; `frust_material`'s `expandable_list`/`dismissible`) must call `request_layout` on *every* in-flight spring frame, not just at the animation's start/end, or its container's cached layout goes stale mid-motion — the fix pattern is now pinned in both crates, but a new animated-height widget that skips it reintroduces the bug silently. |
| `frust-widgets::nav` (`crates/frust-widgets/src/nav/`) | Just decomposed from a single ~8,700-line `navigator.rs` into six modules (2,044-line `navigator.rs` core plus `ambient.rs`/`options.rs`/`controller.rs`/`view.rs`/`edge_swipe.rs`, all private and re-exported through `navigator.rs`) — the split has no review track record yet, so weight a touch to any of the six as if to the old monolith. Three recurring fragile classes now map to named files: layout-skip in `NavigatorWidget`'s cached-child layout (`navigator.rs`), back-press arbitration and reach (`navigator.rs` + `controller.rs`), and interactive edge-swipe gesture/claim state (`edge_swipe.rs`). |
| Reactive tracking (`frust-reactive`, `TrackedScope` consumers) | An untracked read of a render-relevant signal is a silent wake hazard — the UI doesn't update until some unrelated write happens to wake it. New rebuild-path reads need explicit `TrackedScope` coverage, not just a passing host test. |
| `frust-native-widgets` (`plugins/native-widgets`) FFI surface | JNI export symbol names and Kotlin package/class names are frozen once shipped; evolution must be additive only. There is exactly one generic factory/listener per platform — a new per-control glue path is an architectural regression, not a convenience. |
| `frust-drive` cores + `frust-mcp` + `frust-dap` | Print-free contract, one family, three tripwires (`crates/{frust-drive,frust-mcp,frust-dap}/tests/print_free_cores.rs`): all three link into `frust-tui`, so a stray `print!`/`println!` garbles the workbench's raw-mode terminal — `frust-dap`'s is the strictest, zero-allowlist, since it is *embedded-only* (no `frust dap` process, no stdout of its own to fall back on at all) and its host owns the terminal outright. All process spawns must pipe stdout/stderr, never inherit. |
| `frust-widgets` authoring boundary | Baseline widgets must never depend on a design-system plugin (`frust-glyph`/`frust-material`/`frust-cupertino`); all container plumbing goes through the authoring module. Structurally enforced now (the three are separate crates depending only on `frust`, so `frust-widgets` cannot name them without a manifest edit) — a manifest change adding one as a `frust-widgets` dependency is itself the violation to flag, since no source-scan test remains to catch it after the fact. |
| Theme resolution (`frust-theme` / `Theme` recovery) | Precedence is explicit builder value > theme token > unthemed fallback. A hardcoded metric or color where a token already exists is a defect. `frust-core`/`frust-widgets` must never call `Instant::now()` directly — time enters only as shell-supplied `FrameTime`. |
| `frust-database`'s turso bridge (`plugins/database/src/turso.rs`) | The tier's first plugin-owned OS thread + tokio runtime; a caller-thread `block_on` would panic under `panic="abort"`, so the bridge sends futures across a channel instead. The `AsyncContext` guard is a provable-subset detector (`try_current`+`try_id`), not exhaustive — a change to how it's computed can silently widen or narrow what it catches without any test failing outside the two cases already pinned. |
| `plugins/material`'s newest, densest surfaces (`button/`, `slider/`, `carousel/`, `date_picker/`, `time_picker/`, plus Phase 4's nav tier: `appbar/`, `toolbar/`, `split_button.rs`, the liquid-indicator family in `navbar.rs`/`navigation_rail.rs`/`navigation_drawer.rs`) | The button family shares one press-morph driver (`RadiusPaddingMotion`) across every variant plus a native gradient-decoration seam and three overflow strategies; the slider owns its own frame-clock haptic scheduler and wavy-phase paint math. Phase 3 added `carousel/`'s weighted-slot layout solver + snap-physics drag machine and the pickers' in-crate calendar/dial geometry math (an `atan2` `(0,0)` NaN trap). Phase 4 added the nav tier: `toolbar/`'s scroll-hide visibility controller + FAB 80→56 morph choreography (icon-centering degrade tracked in `docs/LIMITATIONS.md` `material-fab-fixed-tier-icon-centering`), `split_button.rs`'s two focus-separated child pods routing input independently, and the two-spring liquid selection indicator `navbar.rs` establishes and `navigation_rail.rs`/`navigation_drawer.rs` both cite. None of it has a regression history yet — weight a touch to any of it heavily until one builds up. |
| `frust-material`/`frust-shadcn` overlay `modal`/`anchored` hosts — staged vs. unstaged dismiss routes | Every host-chrome dismiss gesture (scrim tap, Escape, close button, drag, and — material only — Android back) stages a reverse-ramp exit before firing the app's dismissal; a panel's own content-owned close control that pops via `NavigatorController::pop()` directly instead — the only route open to it, since the staged `dismiss_signal` is private to `show_overlay_modal` — dismisses **unstaged**, with no ramp (`docs/LIMITATIONS.md` `material-modal-staged-dismiss-private-to-host`). The recurring shape to watch for: a new modal-family component wiring its own in-content close affordance down the unstaged path while the rest of the same panel stages — easy to get right for chrome, easy to miss for content. Two more recurring shapes the Phase-3 closure round caught in both hosts: (a) any leg that ramps the surface back **open** (a refused staged pop, a caught mid-exit drag) must itself clear the exit-staging flag — leaving it set across a reopening leg permanently locks the surface out of ever dismissing again; (b) a barrier must gate its input swallow on `progress > epsilon`, never on open/closed state alone — an invisible surface (progress ~0, still mounted) that keeps absorbing input is the same defect class regardless of which host it's in. |

## Severity Calibration

- **Critical:** on-device visual or input regressions — device gates are the arbiter here, a green
  host-side run does not clear a device-gated change. Independent version bumps of the coupled
  rendering-stack pins (`vello`/`wgpu`). A renamed or removed frozen FFI symbol in `native-widgets`.
  Data loss in the shared-preferences or secure-storage plugin. Any blocking or parking call on the
  platform UI thread (typed fail-fast + `spawn_blocking` is the contract). A `render_split`
  deadlock.
- **Major:** a feature-gating regression only catchable by a package-scoped
  `--no-default-features` build, not by the normal workspace build. Removing or bypassing a
  kill-switch seam. Weakening a source-scan conformance test (`print_free_cores`, `profile_sync`,
  `surface_mode`'s single-writer check) instead of fixing the underlying violation. A `frust-widgets`
  manifest edit adding a dependency on any of the three design-system plugin crates. Target-gated
  code that no longer compiles under the Android or iOS compile gate.
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
