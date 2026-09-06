# Frust - RENDER Development

Version pins owned by the RENDER unit (`frust-render`, `frust-text`). Shared prerequisites, the
GPU smoke gate, the non-default-feature test commands, and the version-pin *policy* live in
[DEVELOPMENT.md](DEVELOPMENT.md); the unit's design lives in
[RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md).

## Version Pins

The pins this unit owns, under [DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy (pins are
LAW; re-run the row's tripwire after touching it, and never run a blind `cargo update`):

| Pin | Why | Tripwire |
|---|---|---|
| `wgpu` caret `30.0.1` — frust-owned, no `vello` constraint above it (`vello` is fully deleted; nothing in this workspace depends on it) | The engine's substrate (`frust-gpu`); vertex/fragment-only, no compute — bump deliberately, after the full engine gate suite (device smokes across backends), never casually | `cargo build --workspace --locked` plus a device smoke on each shipping backend — the retired "wgpu 30.0.1 device smoke" section of RESULTS.md (git history, `git show f64be636:benchmarks/RESULTS.md`) records the p8-07 arms (Pixel 5 Vulkan matrix; iPhone SE + Simulator Metal gates; macOS window smoke; Windows DX12 smoke on the Dell rig) — all four shipping-backend arms recorded |
| `image =0.25.10` exact (`Image` widget's PNG/JPEG decoder, `png`/`jpeg` only) | Only 0.25.x release whose MSRV equals the workspace `rust-version` (1.88) | fresh MSRV check before bumping, not just `cargo update` |
| `vello_common =0.2.0` / `glifo =0.3.0` exact | `frust-engine`'s vendored sparse-strips rasterizer core — the engine is a plain dependency of `frust-render`, not a cargo feature; see Engine below | `cargo build -p frust-render --locked` and `cargo test -p frust-engine`, plus `cargo tree -d` |
| `vello_cpu_oracle (=0.2.0, package vello_cpu, features std+text+u8_pipeline)` — dev-only | `frust-testing`'s CPU oracle arm rasterizes with the engine's own core (`vello_common`/`glifo` 0.2.0/0.3.0), the P1 reference against `EngineOracle`; the published crate hits a `compile_error!` on `std`+`text` alone, so `u8_pipeline` is required, not decorative | `cargo test -p frust-testing --test dup_identities` (guard G6: exactly ONE `vello_common` identity (`0.2.0`), ONE `glifo` identity (`0.3.0`), `vello_cpu` resolving to `{0.2.0}`) |

`effective_limits` (`frust-gpu::context`) still forces the iOS Simulator's device request back up to
a 256-byte uniform alignment (the Simulator misreports its own) — kept until upstream
[gfx-rs/wgpu#10189](https://github.com/gfx-rs/wgpu/pull/10189) ships; see
[DEVELOPMENT.md](DEVELOPMENT.md)'s Known Issues.

**The wgpu pin's per-target backend routing is not a size lever — do not re-open it as one.**
[DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy documents the routing itself (backend
features added per target in `crates/frust-gpu/Cargo.toml`'s `[target.'cfg(...)'.dependencies]`
tables rather than one unioned `[workspace.dependencies]` row). Measured under this workspace's
`lto = "fat"`/`codegen-units = 1` release profile, that routing changes ~0 bytes of output: the
unused naga shader writers the old unioned row dragged in were already dead-stripped by fat LTO.
The real, measured size lever lives in `frust_bench` instead: its `db` feature (default-on)
shrinks arm64 release `libfrustbench.so` from 11.35 MB to 9.41 MB under `--no-default-features`
(build recipe in `benchmarks/frust_bench/Cargo.toml`'s `db` feature comment; the resulting APK is
a debug-signed measurement artifact only — [DEVELOPMENT.md](DEVELOPMENT.md)'s Release Builds
section).

## Engine

`frust-engine`/`frust-gpu` are plain, unconditional dependencies of `frust-render`
(`crates/frust-render/Cargo.toml`) — not a cargo feature. `frust-engine`'s strip pipeline is the
only renderer this crate contains, so every build of `frust-render`, `--no-default-features`
included, is an engine build; the facade carries no forwarding feature for it either, since there
is no longer a choice to forward. `perf-trace` remains an ordinary opt-in feature (gates the
`FRUST_TRACE`-driven probe log lines plus, through `frust-gpu`'s own `perf-trace` row, the device's
`TIMESTAMP_QUERY` request).

## Instrumentation (render path)

The render-path environment variables, split out of [DEVELOPMENT.md](DEVELOPMENT.md)'s
Instrumentation table (which keeps the cross-unit ones and points here for this one). Follows
`FRUST_TRACE`'s compile-time-or-runtime parsing shape — a compile-time `option_env!` define or a
runtime env var, resolved once and cached.

| Variable | Purpose | Default |
|---|---|---|
| GPU timestamps (`gpu_q`, `gpu_prepass_us`/`gpu_main_us`/`gpu_composite_us`/`gpu_blit_us` — `FRUST_TRACE_RAW`'s v4 fields) | Real per-pass GPU time via pass-boundary `timestamp_writes` (`wgpu::Features::TIMESTAMP_QUERY`) (`frust_gpu::diag::TimestampRing`, spans named by `frust_engine::diag::EngineSpan`). `gpu_q=1` only on a `perf-trace` build whose adapter actually offers the feature; every other case — no `perf-trace`, no adapter feature, or a frame ahead of the ring's first completed readback — reports the ordinary `gpu_q=0`, not a failure; a refused frame abandons its ring slot rather than mapping it. Full raw-line field format: `benchmarks/PROTOCOL.md`. | `gpu_q=0` (no reading) |

**`FRUST_ENGINE_*` knobs (`frust-gpu`/`frust-engine`-owned, distinct from the GPU-timestamps row
above):**

| Variable | Purpose | Status |
|---|---|---|
| `FRUST_ENGINE_NO_DEPTH` | Disables the engine's depth attachment/test (`config::depth_disabled`); the two draw passes collapse into one blended painter-order pass. | Wired |
| `FRUST_ENGINE_NO_ATLAS` | Routes every glyph/image atlas resolution to a logged skip instead. | Wired |
| `FRUST_ENGINE_ATLAS_SIZE=<W>x<H>` | Overrides the capability-chosen per-layer atlas extent (e.g. `2048x2048`); runtime wins over compile-time. | Wired |
| `FRUST_ENGINE_DOWNLEVEL=1` | Rehearses the WebGL2/GLES3.0 limit ceiling (`downlevel_webgl2_defaults()`) against a desktop adapter — `frust-gpu`'s `TierCaps`/device-request path only (see GPU Substrate below); no browser/wasm measurement exists — the arm was cancelled, not merely unimplemented (`engine-webgl2-unhosted` in LIMITATIONS.md). | Wired |
| `FRUST_ENGINE_NO_LAYERS` / `FRUST_ENGINE_NO_POOL` / `FRUST_ENGINE_MAX_TEX=<n>` | Parsed and cached (`crates/frust-engine/src/config.rs`) but consulted by nothing yet — reserved for the layer-cache/resource-pool/texture-ceiling subsystems that will read them. | Reserved (no effect) |

Each follows `FRUST_TRACE`'s compile-time-`option_env!`-or-runtime-`std::env::var` shape, runtime
winning.

**Adaptive-refresh devices can demote an untouched session's refresh class mid-A/B.** An
Android `DisplayModeDirector` can drop an idle/cheap-frame cell to 60 Hz while a heavier arm
self-promotes to its panel's peak rate, so an unpinned tier A/B measures the display-policy
decision, not the render tier — pin `min`/`peak_refresh_rate` for the cell under measurement
(`adb shell settings put system min_refresh_rate <hz>` / `peak_refresh_rate <hz>`, restored after)
and verify the Choreographer period in the raw logs; the measured before/after is in the retired
Phase-7 A/B section of `benchmarks/RESULTS.md` (git history, `git show f64be636:benchmarks/RESULTS.md`).

## GPU Substrate (`frust-gpu`, `frust-engine`)

`frust-gpu` is the wgpu substrate under the frust-owned render engine (device/surface/
pipeline lifecycle, pipeline cache, shader loading, resource pool/arena, encoder, headless
testing). It carries its own test/adapter-pin split, same shape as `frust-testing` above:

```bash
# Host-only arm, no GPU/environment needed:
cargo test -p frust-gpu

# Real-adapter arm, pinned runner (a multi-adapter host must pin explicitly, e.g. the
# reference Linux rig also enumerates an Intel iGPU alongside the discrete T400; on a Mac
# the pin is Metal's default adapter):
WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 cargo test -p frust-gpu -- --ignored
```

Every device-suite fixture — this crate's own `--ignored` arm, `frust-engine`'s, and
`frust-testing`'s `EngineOracle::new` — requests its device limits through
`frust_gpu::test_device_limits(&adapter, &caps)`, the same derivation `create_device` uses for the
production device request (WebGL2 downlevel defaults on that profile, else the adapter's own
limits), never a bare `wgpu::Limits::default()`. This is what lets the seam suites run on the iOS
Simulator's 15-inter-stage-variable Apple2 adapter; see the iOS Simulator engine gate below.

`bytemuck` (`derive` feature) is a `[workspace.dependencies]` row for `frust-gpu`'s
plain-old-data GPU vertex/uniform types (`Pod`/`Zeroable`); `derive` is the only feature the
workspace needs.

`crates/frust-gpu/src/lint.rs` plus `tests/downlevel_rules.rs` are downlevel (WebGL2/GLES3.0)
design-rule lints — a WGSL source scan for disallowed compute/storage/bit-intrinsic/
depth-textureLoad usage, plus pipeline-layout bounds and WebGL2 limits checks — that run inside
`cargo test -p frust-gpu`, i.e. the ordinary `cargo test --workspace` gate; they run against
`frust-engine`'s shipped `shaders/` directory (`strip.wgsl`, `filter.wgsl`, `blend.wgsl`, and the
rest) and are load-bearing today.

`frust-engine` is the render engine built on that substrate (scene compile, paint/gradient
encoding, GPU strip layouts, the ported WGSL pipelines, `EngineRenderer`). Same adapter-pin
split as `frust-gpu`, mandatory on a multi-adapter host:

```bash
# Host-only arm, no GPU/environment needed:
cargo test -p frust-engine

# Host-only arm with perf-trace instrumentation tests:
cargo test -p frust-engine --features perf-trace

# Real-adapter arm, pinned runner (adapter pin mandatory on a multi-adapter host):
WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=<gpu> cargo test -p frust-engine -- --ignored

# Goldens: engine output compared against the reference renderer, same adapter pin plus an
# expect-adapter fail-fast:
WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=<gpu> FRUST_GOLDEN_EXPECT_ADAPTER=<gpu> \
  cargo test -p frust-testing --test engine_goldens --test alpha_polarity -- --ignored

# Diagnostic only — never a gate:
cargo bench -p frust-engine -- --quick
```

Engine-owned instrumentation — the `frust-perf img`/`frust-perf atlas`/`frust-perf enc` counter lines
and the `EncodeTrace` type — compiles only under the `perf-trace` feature (an island, absent entirely
otherwise) and is deliberately NOT gated by the shell's runtime `FRUST_TRACE` dial — the engine cannot
observe shell-side instrumentation state and cannot condition its own on it. `PhaseClock`
(`compile/mod.rs`) is not one of these islands: the type compiles unconditionally on every build —
only its clock field and reads are `perf-trace`-only, so without the feature it is zero-sized and every
lap answers `Duration::ZERO` with no clock ever read — inert, not absent.

`bash scripts/testing/engine-lean-check.sh` is the engine's lean-weight gate (same SKIP≠FAIL exit
shape as `scripts/release-lean-check.sh`; manual gate, no CI): a single default arm builds
`examples/material3-demo`'s ordinary release build and asserts `frust-engine`/`frust_engine`
markers ARE present (the inverted expectation from the pre-swap gate, which asserted their
absence) while the classic-only crate names `vello_shaders`/`vello_encoding` are ABSENT — deliberately
not a bare `vello` check, since `vello_common`/`glifo` are the engine's own vendored core and
legitimately remain in the binary. There is no second, feature-on arm any more: the engine stopped
being a cargo feature, so there is nothing left for an ON/OFF distinction to distinguish.

## Golden / Oracle Tests

`frust-testing`'s CPU-arm gate rides the standard `cargo test --workspace` chain in
[DEVELOPMENT.md](DEVELOPMENT.md); the commands below isolate it and cover the GPU-gated arms:

```bash
# CPU arm only, no GPU/environment needed — includes tests/goldens.rs and
# tests/adversarial.rs, the unit/adversarial corpora's `cpu/`-class gate:
cargo test -p frust-testing

# material3-demo's own standalone page-golden gate (CPU arm, no GPU needed):
(cd examples/material3-demo && cargo test --test page_goldens)
```

The unit/adversarial corpora have no separate GPU arm of their own any more: `tests/goldens.rs` and
`tests/adversarial.rs` are entirely CPU-oracle gates (`cpu/`, baseline-required), and their GPU
coverage lives in `tests/engine_goldens.rs` above instead, over the engine's own
`engine-`-prefixed golden classes.

The filter golden family (`crates/frust-testing/src/corpus/filters.rs`'s `FilterCase`s, exercised
by `crates/frust-testing/tests/engine_goldens.rs`) is engine-only: `frust_scene` carries no filter
command, so a case is not a `CorpusCase` and is rendered directly on each arm rather than through
`EngineOracle`. The engine arm drives `push_filter_layer`/`Schedule`/`FilterResources` the same way
`frust-engine`'s own `tests/filters.rs` does; the CPU arm is a REAL `vello_cpu` 0.2.0 render through
`RenderContext::push_layer`'s own `filter` parameter, cropped to the layer's own device-space
bounds so the two arms compare pixel-for-pixel. `cpu`-class baselines are committed after visual
inspection; the pinned T400 rig's engine class is recording-only until a reviewed baseline is
promoted. Each case's measured engine-vs-`vello_cpu` tolerance is documented in
`engine_goldens.rs`.

**iOS Simulator engine gate** (gate: `engine-p6-ios-simulator-renders`;
`crates/frust-testing/tests/ios_sim.rs`, `#![cfg(target_os = "ios")]`): the one automated proof
that the engine tier renders correct, non-black pixels on exactly the adapter vello classic was
black on by construction (the Simulator's Apple2 Metal feature set lacks `INDIRECT_EXECUTION`).
Drives `EngineRenderer` directly against the Simulator's own real adapter and compares seven
unit-corpus cases against the embedded `testing/goldens/cpu/` baseline (`include_bytes!`, not a
filesystem read — the Simulator process has no path back to this checkout); no new golden class.
Its own device request already uses the same `test_device_limits` derivation the GPU Substrate
section above describes, which is why it never over-asks the Simulator's constrained adapter the
way the `scene_texture`/`shader_quad`/`shared_encoder` seam suites' fixtures did before that fix —
see [LIMITATIONS.md](LIMITATIONS.md) `engine-ios-sim-seam-suites-unrun`.
Run recipe: [DEVELOPMENT.md](DEVELOPMENT.md)'s Manual/gated tests.

`docs/TESTING.md` is the canonical golden-image/oracle/class runbook; the commands above are the
render-stack-specific reproduction recipes for the pins and knobs this spoke owns.

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md) — the unit's design, incl. the render-path
  data flows the knobs above switch between
