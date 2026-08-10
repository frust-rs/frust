# Frust - Architecture

## Overview

Frust is a Rust-native, declarative, mobile-first UI framework: apps author a `View` tree each
frame from `Component` state, which diffs into a retained `Widget` tree driving layout, paint,
and a vello/wgpu GPU render pipeline. Signals-based reactivity (`frust-reactive`) wakes the host
shell on state change. Android, iOS, and desktop shells embed the same core and render pipeline;
a plugin tier bridges OS capabilities and native controls; standalone CLI/TUI tooling scaffolds,
builds, and drives real devices.

This is the root index. It holds only the shared, cross-unit shape; each unit's internals live in
its own spoke — read this to orient, then follow one link.

## Doc Map

| Unit | Packages | Responsibility | Spoke |
|------|----------|-----------------|-------|
| CORE | `frust-core`, `frust-scene`, `frust-reactive`, `frust-paths`, `frust` (facade) | View/Widget lifecycle, layout, event routing, the Component state boundary; renderer-agnostic scene display-list seam; leaf signals/tasks executor; leaf data/cache-dir resolution; facade curating all of the above into one app!/Component/View API | [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) |
| RENDER | `frust-render`, `frust-text` | wgpu+Vello GPU backend encoding the scene display list and presenting to a surface, owning render-tier selection and per-surface shader effects; Parley-based text shaping/layout/IME editing engine | [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md) |
| WIDGETS | `frust-widgets`, `frust-theme` | Baseline widget set (layout, controls, text, gestures, navigation, platform-view slots) plus three feature-gated design-system catalogs (Material, Cupertino, Glyph) over a shared authoring toolkit; sibling design-token crate bundled into the `Theme` widgets recover from context | [WIDGETS_ARCHITECTURE.md](WIDGETS_ARCHITECTURE.md) |
| SHELLS | `frust-shell-common`, `frust-shell-desktop`, `frust-shell-android`, `frust-shell-ios` | The seam to each host: owns the event loop/frame callback, drives rebuild→layout→paint→encode→present, and translates native input/lifecycle/theme/insets/IME/deep-link/back/platform-view signals | [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) |
| PLUGINS | `frust-plugin`, `plugins/shared-preferences`, `plugins/secure-storage`, `plugins/camera`, `plugins/clipboard`, `plugins/haptics`, `plugins/iap`, `plugins/clean-signals-frust`, `plugins/database` | Shared Android platform-handle substrate; six OS-capability plugins behind a shared conformance suite; facade-tier glue for an external clean-architecture core; a pure-Rust embedded-SQL plugin | [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md) |
| NATIVE_WIDGETS | `frust-native-widgets` (`plugins/native-widgets`) | A platform plugin rendering real OS controls plus plugin-authored native view hierarchies, driven through exactly one generic factory/listener per platform; its **theme ladder** folds `Theme` into control props every frame (diff-gated) and degrades bundled fonts to the platform system font when unavailable | [NATIVE_WIDGETS_ARCHITECTURE.md](NATIVE_WIDGETS_ARCHITECTURE.md) |
| CLI | `frust-cli`, `frust-drive`, `frust-mcp`, `frust-dap` | Thin clap front-end plus the framework-free drive library: scaffolds projects, validates toolchain, discovers devices, drives Android/iOS run/build/clean pipelines; `frust-mcp` exposes the same driving/diagnosis surface to AI agents over an MCP Streamable HTTP server; `frust-dap` exposes launch orchestration to a DAP-speaking IDE over stdio/loopback-TCP | [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md) |
| TUI | `frust-tui` | Mouse-first ratatui TEA terminal workbench supervising `frust-drive` sessions (scaffold/build/run/doctor/clean); can embed an `frust-mcp` server so an AI agent drives the same sessions | [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md) |
| DEVTOOLS | `frust-devtools-protocol`, `frust-devtools` | Sanctioned dependency-free wire-protocol leaf plus the in-app loopback debug service (widget-tree inspection, frame stats, input injection) a shell hosts for `frust-drive`/`frust-tui` | [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md) |

## Examples

Consumers of the framework, not units — each keeps its own README, not an ARCHITECTURE.md:

| Example | What it shows |
|---------|---------------|
| `examples/huddle` | clean-signals clean-architecture showcase; the sole full example app |
| `examples/shadertoy` | Fragment-shader effects showcase |
| `examples/glyph-catalog` | Glyph design-system showcase (theme only) |
| `examples/playground` | Plugin functionality, native widgets, platform views, responsiveness, and general testing showcase; a standalone workspace |
| `examples/no-catalogs` | Compile guard proving `frust --no-default-features` builds — part of the verify gate, not a showcase |

`benchmarks/` is separate from the examples above: flutter-vs-frust comparative benchmarking
only (S1–S8, D1–D2 per `benchmarks/PROTOCOL.md`), not a framework showcase.

## Cross-Unit Layer Dependencies

- **Scene-layer purity** — `frust-scene` and `frust-text` expose only `kurbo`/`peniko` types; `vello`
  and `wgpu` are confined to `frust-render`. No widget or core code depends on a GPU crate directly.
- **`frust-reactive` is a leaf** — depends only on `reactive_graph`/`any_spawner`/`tokio`/`futures`;
  nothing in core, scene, render, or a shell may appear in its dependency graph.
- **`frust-core`'s one documented exception** — `frust-core` depends on `reactive_graph` directly
  (alongside `frust-scene`, `tree_arena`, `kurbo`, `peniko`, `accesskit`) rather than through the
  `frust-reactive` wrapper; this is the single sanctioned bypass of the reactive seam.
- **`frust-paths` leaf charter** — depends only on `log`; every shell and plugin that needs a
  data/cache directory calls into it rather than resolving paths itself.
- **Tooling isolation** — `frust-cli`, `frust-drive`, `frust-tui`, `frust-mcp`, and `frust-dap`
  depend on NO framework crate EXCEPT the `frust-devtools-protocol` leaf (`serde`/`serde_json`
  only); otherwise they only shell out to `cargo`/platform toolchains. `frust-mcp` is a fourth
  tooling front-end under this rule (an MCP server for AI agents, not `frust-devtools` — never a
  dev-dependency on it either); `frust-dap` is a fifth (a DAP server for IDEs). Tooling MAY depend
  on the `frust-mcp` tooling crate itself — `frust-tui`'s dependency set is `frust-drive` +
  `frust-devtools-protocol` + `frust-mcp`, embedding the MCP server inside the workbench (see
  [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)); `frust-dap` is the second such crossing, reusing
  `frust-mcp`'s `SessionEngine` for launch orchestration (see [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md))
  — both crossings stay entirely within tooling and drag in no framework crate. `frust-cli` itself
  has no *direct* dependency on `frust-mcp` and no `mcp` subcommand — it gains only a *transitive*
  one through `frust-dap`'s `dap` subcommand; an MCP client still reaches a session only through a
  running `frust-tui`. `frust-devtools` (the framework-side debug service a shell hosts) is the
  mirror rule: it depends on no framework or tooling crate either — the two sides of the devtools
  wire meet only at the protocol leaf. See [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md).
- **Facade/plugin boundary** — the `frust` facade never depends on or re-exports a plugin. Plugins
  (`frust-plugin` substrate, `native-widgets`, `clean-signals-frust`) sit *beside* the facade in an
  app's own dependency list, never inside it.

```
frust-reactive (leaf)              frust-paths (leaf)          tooling: frust-cli/-drive/-tui/-mcp
        │                                  │                              (no framework crate)
frust-scene/frust-text ──► frust-core ──► frust-widgets/frust-theme ──► frust (facade)
   (kurbo/peniko only)         │                                              │
        │                      └──────────► frust-render (vello/wgpu) ◄──────┘
        ▼                                                                    │
   shells (desktop/android/ios) ◄──────────────────────────────────────── uses facade
        │
        ▼
  plugins (frust-plugin, native-widgets, clean-signals-frust) — beside the facade, never inside it
```

## Cross-Unit Data Flow

- **Rebuild wake flow** — a tracked signal write notifies the process-wide `FrameWaker`, which the
  active shell consults to schedule its next frame. See [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md).
- **Render encode flow** — a `frust-scene` `Scene` built by `Component`/widget paint is run through
  `encode_scene` into a `vello::Scene`, then `SurfaceRenderer` encodes and presents it, either
  direct-to-surface or via an intermediate-texture blit. See [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md).
- **Theme delivery** — the shell owns the active `Theme`; widgets recover it type-erased from the
  paint/layout context; app code reads a cloned `Theme` reactively. `native-widgets` folds it into
  control props every frame, diff-gated against the platform FFI. See
  [WIDGETS_ARCHITECTURE.md](WIDGETS_ARCHITECTURE.md) and [NATIVE_WIDGETS_ARCHITECTURE.md](NATIVE_WIDGETS_ARCHITECTURE.md).
- **Window metrics** — the shell publishes `WindowMetrics` (logical size, scale, derived orientation,
  and insets snapshot) as a plain `provide_context` value (not a signal) whenever the window shape
  changes, guarded to avoid unconditional per-frame re-provides. Widgets adapt layout and visuals to
  the shape without a separate reactive subscription. See [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) and
  [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md).
- **Android frame pipeline** — a Choreographer callback consults `frame_gate` to run or skip the
  frame, then rebuild → conditional layout → paint → render-thread present. See
  [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md).
- **iOS frame pipeline** — a CADisplayLink tick consults the same `frame_gate`, then
  rebuild/layout/paint/present, with an optional present-sync against platform-view geometry. See
  [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md).
- **CLI flow** — `Cli` (clap) parses an **optional** `Command`; `main` resolves the TTY-gated default
  (bare `frust` in a terminal → the TUI) before `commands::dispatch` builds the one `RealProcessRunner`.
  Cores stay print-free — only the CLI and TUI front-ends print. See [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md).
- **Platform-view** — paint-time view frames feed the `platform_view` differ; each shell's FFI
  layer polls the resulting per-frame command backlog and applies it frame-paired with present.
  See [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md).

## Key Types

| Type | Purpose |
|------|---------|
| `View` / `Widget` | The declarative tree authored each frame (`View`), diffed into the retained render tree (`Widget`) |
| `Component` | The state + `build()` boundary run inside a tracked reactive scope |
| `RenderRoot` | Owns the retained `Widget` tree; drives layout and paint from a `View` diff |
| `Scene` / `SceneBuilder` | The renderer-agnostic vector display list — the stable widget↔GPU seam (`frust-scene`) |
| `Theme` | The design-language token bundle (color/type/shape/elevation/motion) recovered from context |
| `ReactiveRuntime` / `FrameWaker` | The leaf signal/task executor and the wake signal it raises on a tracked write |
| `SurfaceRenderer` | `frust-render`'s per-surface encode/present owner (direct-to-surface or blit) |
| `RenderTier` | The GPU (`vello`) rendering path, default; the optional non-default `cpu-tier` feature substitutes a `vello_cpu` software fallback for the same surface target |

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — build, run, test, environment
- [CODE_STANDARDS.md](CODE_STANDARDS.md) — coding conventions and patterns
- [TESTING.md](TESTING.md) — test strategy and coverage
- [LIMITATIONS.md](LIMITATIONS.md) — known gaps and constraints
- [REVIEW_FOCUS.md](REVIEW_FOCUS.md) — review priorities and hot spots
- [DOC_POLICY.md](DOC_POLICY.md) — the doc structure/budget decision record
