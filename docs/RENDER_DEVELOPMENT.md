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
| `vello_hybrid =0.2.0` exact | Hybrid-tier measurement spike (`frust-render`'s non-default `hybrid-tier` feature) — see Hybrid Tier below | `cargo build -p frust-render --features hybrid-tier --locked` and `cargo test -p frust-render --features hybrid-tier` |
| `vello_common =0.2.0` / `glifo =0.3.0` exact | The `hybrid-tier` feature's vendored sparse-strips core (the engine plan's rendering core); pinned exactly like `vello_cpu` | same tripwire as `vello_hybrid` above, plus `cargo tree -d` |

The `vello`/`wgpu` pin is also what pins the iOS Simulator's render gap and the vello
bitmap-emoji decode caveat — both recorded in [DEVELOPMENT.md](DEVELOPMENT.md)'s Known Issues.

## Hybrid Tier

All three companion pins (`vello_hybrid`/`vello_common`/`glifo`) live in the root
`[workspace.dependencies]` beside `vello_cpu`; `frust-render` declares them optional under
`hybrid-tier = ["dep:vello_hybrid", "dep:vello_common", "dep:glifo"]`
(`crates/frust-render/Cargo.toml`), and the facade forwards `hybrid-tier` to the three shells
exactly like `perf-trace` (`crates/frust/Cargo.toml`).

**Sanctioned duplicate.** `vello_common` resolves twice — `=0.0.9` via `vello_cpu`'s `cpu-tier`
feature, `=0.2.0` via `hybrid-tier` — until the engine plan's swap phase retires `cpu-tier`.
`cargo tree -d` is expected to list both versions whenever a build enables both features; this is
not drift.

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
| `FRUST_AA_MODE` / `FRUST_RENDER_SCALE` | Render-cost measurement instruments — not policy: both exist to A/B vello's fine-stage cost on device ([RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s Data Flow), and nothing selects either automatically. `FRUST_AA_MODE` picks vello's anti-aliasing method (`area`/`msaa8`/`msaa16`, case-insensitive; an unrecognised value warns once and falls back to `area`), requested at every render-params site including cached snapshot pages, with the vello renderer built for exactly that one mode's pipelines; msaa8/msaa16 render corrupted on Adreno 620, unusable there (see [LIMITATIONS.md](LIMITATIONS.md)). `FRUST_RENDER_SCALE` renders the whole frame into an intermediate that fraction of the surface (`0.25..=1.0`; out of range clamps, unparsable/non-finite falls back to `1.0`, each with one warn) and lets the blit pass upscale it — while it is below `1.0` the surface is pinned onto the blit arm and the snapshot-layer cache is refused (see [LIMITATIONS.md](LIMITATIONS.md)). Each logs its effective value once per process in any build, no `perf-trace` needed (`frust-render aa-mode=<mode>`, `frust-render render-scale=<s> blit-target=<w>x<h>`), and a non-default value additionally emits one `log::warn!` naming the knob in effect, so a capture read hours later does not have to infer it. **Both are vello-classic instruments the hybrid tier ignores** (`context::parse_aa_mode` feeds vello's `AaSupport`; the scale knob forces vello's blit arm) — a hybrid build logs a one-time note if either is set, and neither has a hybrid counterpart. | `area` (byte-identical to an untouched build) / `1.0` (full surface resolution) |
| `FRUST_RENDER_TIER=hybrid` | Selects the experimental `vello_hybrid` render tier (`hybrid-tier`-featured builds only) — override-only, exactly like `cpu`/`gpu`: no probe ever selects it (`tier.rs`'s `select_render_tier`). Compile-time via `frust build|run --define FRUST_RENDER_TIER=hybrid` (the only path into an Android app process) or a runtime env var for a desktop `cargo run`/`frust run --render-tier hybrid`; runtime wins when both are set (`tier.rs`'s `render_tier_override_from_sources`, via `context::env_str`'s shared precedence). A build **without** the `hybrid-tier` feature refuses the override with a diagnosis naming the missing feature, rather than silently running vello under a hybrid label. | override-only (unset ⇒ GPU-probed as usual) |
| `FRUST_HYBRID_ATLAS_CACHE=1` | Turns on `glifo`'s experimental glyph-atlas cache for every glyph run the hybrid tier draws (upstream calls it "not recommended for external use" — a knob, not a default). Logs `frust-render hybrid atlas-cache=on|off` once per process in any hybrid-tier build, plus one `log::warn!` naming the knob when on. Same compile-time-or-runtime shape as the others. | off |
| hybrid per-frame trace line | Under `perf-trace` + `FRUST_TRACE`, the hybrid tier emits one `frust-perf hybrid strip_us=<n> record_us=<n>` line per frame: `strip_us` is the CPU half (scene reset through the shared command walk's sparse-strip rasterization), `record_us` is the renderer's GPU command-**record** CPU time only — GPU *execution* is not in either window, it lands inside the frame's `submit_us`, same as this tier's whole `HybridDirect` cost (see [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s Data Flow). **Compare whole-frame totals across tiers, never `encode_us`**: on the classic tier's blit arm the GPU render stays in `encode_us` (small `submit_us`), on its direct arm the render moves into `submit_us` instead — a third, incompatible split from the hybrid tier's own. | off (needs `perf-trace` + `FRUST_TRACE`) |

**Planned, not implemented.** `FRUST_ENGINE_DOWNLEVEL=1` — a future Phase-2 knob to request the
WebGL2 downlevel limit profile against a desktop backend, as a browser stand-in — does not exist at
this SHA (`benchmarks/harness/webgl2_arm.md`); do not treat it as a shipped instrument.

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md) — the unit's design, incl. the render-path
  data flows the knobs above switch between
