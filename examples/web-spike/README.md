# web-spike

Throwaway spike for the web-shell plan's Phase 0 (tasks w0-02 through w0-05).
**Read [`RESULTS.md`](RESULTS.md) first** — this file is a map into it, not a
substitute for it.

Not a shipping example: a standalone workspace (own `[workspace]` root, own
`Cargo.lock`), excluded from the root workspace graph via the root
`Cargo.toml`'s `[workspace]` `exclude` list, so its wasm32-only probe never
resolves into the native workspace's lockfile — same shape as
`examples/huddle`.

## What this spike has proven, task by task

| Task | Question | Answer | RESULTS.md |
|---|---|---|---|
| w0-02 | Does the frust render-engine graph (`frust-scene`, `frust-text`, `frust-gpu`, `frust-engine`, `frust-render`) compile for `wasm32-unknown-unknown`? | Yes, once `frust-gpu`'s `fragile-send-sync-non-atomic-wasm` `wgpu` feature landed (now on this task's base). No RUSTFLAGS/`--cfg`/feature needed for the graph itself. | § 8, § 9 |
| w0-03 | Does `frust-engine` render correctly in a real browser, on WebGPU and on WebGL2? | **WebGPU: yes**, shapes and shaped text both correct. **WebGL2: shapes yes, text no** — every glyph paints as a solid opaque box (§ 5's isolated finding: the wgpu GL backend's coverage-texture read, not `frust-gpu`'s caps or `frust-text`'s shaping). | § 1–§ 11 |
| w0-04 | Does a `frust_reactive` signal write wake exactly one repaint through a real `wasm32` `ReactiveRuntime`, with zero input events involved? | Yes — proven with the continuous redraw loop gated off so the signal path is the only possible cause. | § 15 |
| w0-05 | `.wasm` size (debug/release/`wasm-opt`), first-frame time, and single-threaded strip/warm-up cost, per backend — evidence only, no verdict. | Release + `wasm-opt`: 5.44 MiB raw / 1.93 MiB gzipped. First-frame: WebGPU 113ms median, WebGL2 2236ms median (bring-up cost, not strip cost — see below). Steady-state strip/submit cost: 0.6–1.4ms on both backends. | § 16 |

**The known defect that still stands:** WebGL2 cannot render text on this
rig (w0-03 § 5). It is a **correctness** blocker, not (per w0-05 § 16.3) a
performance one — steady-state strip/submit cost is within ~1ms of WebGPU's
on the same scene. A WebGL2 fallback is not usable for a text-bearing UI
until that lands, and it needs its own task (a shader-level GL capture into
`crates/frust-engine/shaders/strip.wgsl`'s `textureLoad` path) — this spike's
write scope (`examples/web-spike`, no `crates/` edits) cannot fix it.

**The other w0-05 finding worth knowing before you drive this rig again:**
the WebGL2 arm's bring-up (context + surface + font + scene, before any
frame renders) took ~2.1 **seconds** longer than WebGPU's on every single
cold run measured, and it is not the § 6 adapter-acquisition retry (both
arms needed exactly 1 attempt every run) — RESULTS.md § 16.3 point 2 has the
full reasoning and flags it as an open `crates/frust-gpu` question.

## Running the probe

Native, render-less compile check only (what w0-02 originally proved, still
green):

```sh
cd examples/web-spike
cargo check
```

Wasm32 compile check (no browser, no server):

```sh
cargo check --target wasm32-unknown-unknown
```

Full browser build + serve (debug or release — see RESULTS.md § 16.1 for why
release should always be built with this crate's own `[profile.release]`
applied, which happens automatically once it is in `Cargo.toml`):

```sh
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir pkg --out-name web_spike \
  /data/cache/target/wasm32-unknown-unknown/release/web-spike.wasm
./serve.sh 8931
```

`--out-name web_spike` is required, not cosmetic — see RESULTS.md § 2 for
why. Then open (or drive via WebDriver — RESULTS.md § 2/§ 6/§ 16.2 document
the exact chromedriver recipe this rig uses, headed on `DISPLAY=:20`;
headless WebGPU canvas readback is blank on this rig, § 4):

```
http://localhost:8931/?arm=webgpu                # WebGPU (default)
http://localhost:8931/?arm=webgl                 # WebGL2 (forced GL backend)
http://localhost:8931/?arm=webgpu&signal=timer   # w0-04's signal->repaint proof, either arm
```

Optionally, run `wasm-opt` on the `wasm-bindgen` output afterward — **after**
`wasm-bindgen`, never before, and with `--all-features` (`-all`) on this
`rustc`/`wasm-opt` combination (RESULTS.md § 16.1 explains why the bare `-O`
flag refuses to parse the input):

```sh
wasm-opt -O --all-features -o pkg/web_spike_bg.opt.wasm pkg/web_spike_bg.wasm
```

## `pkg/` is generated and gitignored

`.gitignore` (this directory) ignores `pkg/` — the `wasm-bindgen --target
web` output above. Never commit it; regenerate it with the commands above
whenever you need it.

## `Cargo.lock`

Committed, following the `examples/huddle` precedent (`git ls-files
examples/huddle/Cargo.lock` shows it tracked there too): a standalone
workspace's lockfile is part of its own reproducible build, not the root
workspace's.

## Files

| File | Purpose |
|---|---|
| `Cargo.toml` | Standalone workspace manifest: five engine-graph path deps, `kurbo`/`peniko`, the `wasm32`-gated browser rows (winit/wasm-bindgen/wgpu/web-sys/etc., w0-03), the `frust-reactive`/`reactive_graph` rows (w0-04), and this crate's own `[profile.release]` (w0-05 — see RESULTS.md § 16.1 for why a standalone workspace needs its own copy of the root's release-profile flags). |
| `src/main.rs` | The probe. Target-independent scene/compile half (w0-02's native check, still green), plus a `wasm32`-only `web` module: canvas, winit loop, backend-restricted context, retrying bring-up, frame loop, logging, the w0-04 `?signal=timer` proof mode, and the w0-05 `performance.now()` timing instrumentation (`timing:` log lines). |
| `index.html` | Host page. One page serves every arm/mode via `?arm=`/`?signal=`; carries the `<pre id="log">` evidence mirror every `timing:`/`frame N`/`verdict:` line is mirrored into. |
| `serve.sh` | `python3 -m http.server` on 127.0.0.1, free high port or an explicit one. `.wasm` is served as `application/wasm` (verified with `curl -I`). |
| `screenshots/` | The w0-03/w0-04 PNGs (RESULTS.md §§ 1, 15). w0-05 added no new screenshots — it is a log/console-transcript measurement, not a visual one. |
| `README.md` | This file. |
| `RESULTS.md` | Full findings for all four tasks: the compile-probe flags table (w0-02), the render/text findings and the browser-driving recipe (w0-03), the signal-wake proof (w0-04), and the size/timing tables (w0-05). |
| `Cargo.lock` | Committed, per the `examples/huddle` precedent above. |
| `pkg/` | `wasm-bindgen` output. Generated and gitignored. |
