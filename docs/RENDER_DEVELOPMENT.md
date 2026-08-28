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
| `vello 0.9.0` / `wgpu 29.0.3` (resolves 29.0.4) | `vello` requires `wgpu ^29.0.3`; bumping `wgpu` independently (30.x is ecosystem-latest) breaks the build | `cargo build --workspace --locked` |
| `image =0.25.10` exact (`Image` widget's PNG/JPEG decoder, `png`/`jpeg` only) | Only 0.25.x release whose MSRV equals the workspace `rust-version` (1.88) | fresh MSRV check before bumping, not just `cargo update` |
| `vello_cpu =0.0.9` exact | Experimental CPU render tier (`frust-render`'s non-default `cpu-tier` feature), pre-1.0 unstable API, isolated behind the `SceneSink` encode seam so a breaking bump never reaches the default GPU path | the `cpu-tier` command in [DEVELOPMENT.md](DEVELOPMENT.md)'s Test section |

The `vello`/`wgpu` pin is also what pins the iOS Simulator's render gap and the vello
bitmap-emoji decode caveat — both recorded in [DEVELOPMENT.md](DEVELOPMENT.md)'s Known Issues.

## Instrumentation (render path)

The render-path environment variables, split out of [DEVELOPMENT.md](DEVELOPMENT.md)'s
Instrumentation table (which keeps the cross-unit ones and points here for these). All five share
`FRUST_TRACE`'s compile-time-or-runtime parsing shape — a compile-time `option_env!` define or a
runtime env var, resolved once and cached — and the three kill switches share one caveat: on the
direct-to-surface arm the GPU render moves into `submit_us` and `acquire_us` precedes rather than
follows it, so account for that remap before comparing timings across arms
([RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s Data Flow has the full field mapping).

| Variable | Purpose | Default |
|---|---|---|
| `FRUST_NO_DIRECT_SURFACE` / `FRUST_NO_SHADER_EFFECTS` / `FRUST_NO_SNAPSHOT_LAYERS` | Render-path A/B kill switches, one per GPU pre-pass ([RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s Data Flow): `FRUST_NO_DIRECT_SURFACE` pins a direct-capable surface onto the blit fallback arm; `FRUST_NO_SHADER_EFFECTS` disables the shader-quad pre-pass, so `Command::ShaderQuad` falls back to its placeholder fill; `FRUST_NO_SNAPSHOT_LAYERS` disables the snapshot-layer cache, so every `PushSnapshot` bracket lowers through `convert.rs`'s inline emulation. | off (path auto-probed) / off (pre-pass active) / off (cache active) |
| `FRUST_AA_MODE` / `FRUST_RENDER_SCALE` | Render-cost measurement instruments — not policy: both exist to A/B vello's fine-stage cost on device ([RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s Data Flow), and nothing selects either automatically. `FRUST_AA_MODE` picks vello's anti-aliasing method (`area`/`msaa8`/`msaa16`, case-insensitive; an unrecognised value warns once and falls back to `area`), requested at every render-params site including cached snapshot pages, with the vello renderer built for exactly that one mode's pipelines; msaa8/msaa16 render corrupted on Adreno 620, unusable there (see [LIMITATIONS.md](LIMITATIONS.md)). `FRUST_RENDER_SCALE` renders the whole frame into an intermediate that fraction of the surface (`0.25..=1.0`; out of range clamps, unparsable/non-finite falls back to `1.0`, each with one warn) and lets the blit pass upscale it — while it is below `1.0` the surface is pinned onto the blit arm and the snapshot-layer cache is refused (see [LIMITATIONS.md](LIMITATIONS.md)). Each logs its effective value once per process in any build, no `perf-trace` needed (`frust-render aa-mode=<mode>`, `frust-render render-scale=<s> blit-target=<w>x<h>`), and a non-default value additionally emits one `log::warn!` naming the knob in effect, so a capture read hours later does not have to infer it. | `area` (byte-identical to an untouched build) / `1.0` (full surface resolution) |

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md) — the unit's design, incl. the render-path
  data flows the knobs above switch between
