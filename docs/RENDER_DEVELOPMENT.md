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
| `vello 0.9.0` / `wgpu` caret `^29.0.3` (resolves 29.0.4) | `vello` requires `wgpu ^29.0.3`; bumping `wgpu` independently (30.x is ecosystem-latest) breaks the build | `cargo build --workspace --locked` |
| `image =0.25.10` exact (`Image` widget's PNG/JPEG decoder, `png`/`jpeg` only) | Only 0.25.x release whose MSRV equals the workspace `rust-version` (1.88) | fresh MSRV check before bumping, not just `cargo update` |
| `vello_cpu =0.0.9` exact | Experimental CPU render tier (`frust-render`'s non-default `cpu-tier` feature), pre-1.0 unstable API, isolated behind the `SceneSink` encode seam so a breaking bump never reaches the default GPU path | the `cpu-tier` command in [DEVELOPMENT.md](DEVELOPMENT.md)'s Test section |
| `vello_common =0.2.0` / `glifo =0.3.0` exact | `frust-engine`'s vendored sparse-strips rasterizer core (`frust-render`'s non-default `engine-tier` feature) — see Engine Tier below; pinned exactly like `vello_cpu` | `cargo build -p frust-render --features engine-tier --locked` and `cargo test -p frust-engine`, plus `cargo tree -d` |
| `vello_cpu_oracle (=0.2.0, package vello_cpu, features std+text+u8_pipeline)` — dev-only | `frust-testing`'s CPU oracle arm rasterizes with the engine's own core (`vello_common`/`glifo` 0.2.0/0.3.0) rather than the legacy `cpu-tier` fallback's 0.0.9 identities; the published crate hits a `compile_error!` on `std`+`text` alone, so `u8_pipeline` is required, not decorative | `cargo test -p frust-testing --test dup_identities` (guard G6: exactly two `vello_common` identities, `0.0.9`/`0.2.0`, and two `glifo` identities, `0.1.1`/`0.3.0`) |

The `vello`/`wgpu` pin is also what pins the iOS Simulator's render gap and the vello
bitmap-emoji decode caveat — both recorded in [DEVELOPMENT.md](DEVELOPMENT.md)'s Known Issues.

## Engine Tier

`frust-engine`/`frust-gpu` are optional dependencies of `frust-render`
(`{ workspace = true, optional = true }`), pulled in only by its non-default
`engine-tier = ["dep:frust-engine", "dep:frust-gpu"]` feature (`crates/frust-render/Cargo.toml`).
The facade re-exports the same feature (`engine-tier = ["frust-render/engine-tier"]`,
`crates/frust/Cargo.toml`) but forwards it no further — unlike `perf-trace`/`devtools`, no shell
crate carries an `engine-tier` feature of its own to reach, so no app in this repo can turn the
tier on through the facade yet; `frust-render`'s own feature is the nearest reachable target
(see `scripts/testing/engine-lean-check.sh`'s ON arm, below).

**Sanctioned duplicate.** `vello_common` resolves twice — `=0.0.9` via `vello_cpu`'s `cpu-tier`
feature, `=0.2.0` via `frust-engine`'s `engine-tier` feature — until `cpu-tier` retires; `glifo`
duplicates exactly the same way (`0.1.1` via `vello_cpu`, `0.3.0` via `frust-engine`). `cargo tree
-d` is expected to list both versions of each whenever a build enables both features; this is not
drift.

## Instrumentation (render path)

The render-path environment variables, split out of [DEVELOPMENT.md](DEVELOPMENT.md)'s
Instrumentation table (which keeps the cross-unit ones and points here for these). All share
`FRUST_TRACE`'s compile-time-or-runtime parsing shape — a compile-time `option_env!` define or a
runtime env var, resolved once and cached — and the three kill switches share one caveat: on the
direct-to-surface arm the GPU render moves into `submit_us` and `acquire_us` precedes rather than
follows it, so account for that remap before comparing timings across arms
([RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s Data Flow has the full field mapping).

| Variable | Purpose | Default |
|---|---|---|
| `FRUST_NO_DIRECT_SURFACE` / `FRUST_NO_SHADER_EFFECTS` / `FRUST_NO_SNAPSHOT_LAYERS` | Render-path A/B kill switches, one per GPU pre-pass ([RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s Data Flow): `FRUST_NO_DIRECT_SURFACE` pins a direct-capable surface onto the blit fallback arm; `FRUST_NO_SHADER_EFFECTS` disables the shader-quad pre-pass, so `Command::ShaderQuad` falls back to its placeholder fill; `FRUST_NO_SNAPSHOT_LAYERS` disables the snapshot-layer cache, so every `PushSnapshot` bracket lowers through `convert.rs`'s inline emulation. | off (path auto-probed) / off (pre-pass active) / off (cache active) |
| `FRUST_AA_MODE` / `FRUST_RENDER_SCALE` | Render-cost measurement instruments — not policy: both exist to A/B vello's fine-stage cost on device ([RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s Data Flow), and nothing selects either automatically. `FRUST_AA_MODE` picks vello's anti-aliasing method (`area`/`msaa8`/`msaa16`, case-insensitive; an unrecognised value warns once and falls back to `area`), requested at every render-params site including cached snapshot pages, with the vello renderer built for exactly that one mode's pipelines; msaa8/msaa16 render corrupted on Adreno 620, unusable there (see [LIMITATIONS.md](LIMITATIONS.md)). `FRUST_RENDER_SCALE` renders the whole frame into an intermediate that fraction of the surface (`0.25..=1.0`; out of range clamps, unparsable/non-finite falls back to `1.0`, each with one warn) and lets the blit pass upscale it — while it is below `1.0` the surface is pinned onto the blit arm and the snapshot-layer cache is refused (see [LIMITATIONS.md](LIMITATIONS.md)). Each logs its effective value once per process in any build, no `perf-trace` needed (`frust-render aa-mode=<mode>`, `frust-render render-scale=<s> blit-target=<w>x<h>`), and a non-default value additionally emits one `log::warn!` naming the knob in effect, so a capture read hours later does not have to infer it. **Both are vello-classic instruments the engine tier ignores** (`context::parse_aa_mode` feeds vello's `AaSupport`; the scale knob forces vello's blit arm) — an engine-tier build logs a one-time warning if either is set, and renders at full surface resolution regardless (see [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s Data Flow). | `area` (byte-identical to an untouched build) / `1.0` (full surface resolution) |
| `FRUST_RENDER_TIER=engine` | Selects the frust-owned `frust-engine` render tier (`engine-tier`-featured builds only) — override-only, exactly like `cpu`/`gpu`: no probe ever selects it (`tier.rs`'s `select_render_tier`; `ENGINE_REQUIRED_DOWNLEVEL_FLAGS` is deliberately empty). Compile-time via `frust build|run --define FRUST_RENDER_TIER=engine` (the only path into an Android app process) or a runtime env var for a desktop `cargo run`/`frust run --render-tier engine`; runtime wins when both are set (`tier.rs`'s `render_tier_override_from_sources`, via `context::env_str`'s shared precedence). A build **without** the `engine-tier` feature refuses the override with a diagnosis naming the missing feature, rather than silently running vello under an engine label. | override-only (unset ⇒ GPU-probed as usual) |

**Shipped in `frust-gpu`, absent in `frust-render`.** `FRUST_ENGINE_DOWNLEVEL=1` now exists as
`frust-gpu`'s WebGL2 rehearsal knob (clamped `TierCaps` + clamped device request — see the GPU
Substrate section below); the vello-classic tier documented in this table ignores it, and the
browser/wasm measurement arm (`benchmarks/harness/webgl2_arm.md`) remains unimplemented.

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

`bytemuck` (`derive` feature) is a `[workspace.dependencies]` row for `frust-gpu`'s
plain-old-data GPU vertex/uniform types (`Pod`/`Zeroable`); `derive` is the only feature the
workspace needs.

`crates/frust-gpu/src/lint.rs` plus `tests/downlevel_rules.rs` are downlevel (WebGL2/GLES3.0)
design-rule lints — a WGSL source scan for disallowed compute/storage/bit-intrinsic/
depth-textureLoad usage, plus pipeline-layout bounds and WebGL2 limits checks — that run inside
`cargo test -p frust-gpu`, i.e. the ordinary `cargo test --workspace` gate; they are green
against the currently empty shader set and become load-bearing once the engine plan starts
adding shaders.

`frust-engine` is the render engine built on that substrate (scene compile, paint/gradient
encoding, GPU strip layouts, the ported WGSL pipelines, `EngineRenderer`). Same adapter-pin
split as `frust-gpu`, mandatory on a multi-adapter host:

```bash
# Host-only arm, no GPU/environment needed:
cargo test -p frust-engine

# Real-adapter arm, pinned runner (adapter pin mandatory on a multi-adapter host):
WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=<gpu> cargo test -p frust-engine -- --ignored

# Goldens: engine output compared against the reference renderer, same adapter pin plus an
# expect-adapter fail-fast:
WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=<gpu> FRUST_GOLDEN_EXPECT_ADAPTER=<gpu> \
  cargo test -p frust-testing --test engine_goldens --test alpha_polarity -- --ignored

# Diagnostic only — never a gate:
cargo bench -p frust-engine -- --quick
```

`bash scripts/testing/engine-lean-check.sh` is the engine's lean-weight gate (same SKIP≠FAIL exit
shape as `scripts/release-lean-check.sh`; manual gate, no CI): the OFF arm builds
`examples/material3-demo`'s ordinary release build (it depends on neither `frust-engine` nor a
`frust-render` `engine-tier` feature today) and fails on a `frust-engine`/`frust_engine` marker
leak; the ON arm is the positive control, building `frust-render` itself with `--features
engine-tier` from the root workspace and failing if the markers are ABSENT despite the feature
being on — the facade's `engine-tier` feature forwards to no shell (see Engine Tier above), so no
app in this repo can reach it yet, and `frust-render` is the nearest reachable ON target until one
can.

## Golden / Oracle Tests

`frust-testing`'s CPU-arm gate rides the standard `cargo test --workspace` chain in
[DEVELOPMENT.md](DEVELOPMENT.md); the commands below isolate it and cover the GPU-gated arms:

```bash
# CPU arm only, no GPU/environment needed:
cargo test -p frust-testing

# Classic (GPU) golden + adversarial arms, pinned runner (adapter pin required — the
# reference box also enumerates an Intel iGPU, so an unpinned run can silently land there):
WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
  cargo test -p frust-testing --test goldens --test adversarial -- --ignored

# Calibration reproduction (same pinned runner):
WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
  cargo test -p frust-testing --test calibration -- --ignored

# material3-demo's own standalone page-golden gate (CPU arm, no GPU needed):
(cd examples/material3-demo && cargo test --test page_goldens)
```

`docs/TESTING.md` is the canonical golden-image/oracle/class runbook; the commands above are the
render-stack-specific reproduction recipes for the pins and knobs this spoke owns.

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md) — the unit's design, incl. the render-path
  data flows the knobs above switch between
