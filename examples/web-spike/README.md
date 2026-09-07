# web-spike

Throwaway spike for the web-shell plan's Phase 0 (tasks w0-02 through w0-05,
plus w0-07's diagnosis).
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
| w0-05 | `.wasm` size (debug/release/`wasm-opt`), first-frame time, and single-threaded strip/warm-up cost, per backend — evidence only, no verdict. | Release + `wasm-opt`: 5.44 MiB raw / 1.93 MiB gzipped. First-frame: WebGPU 113ms median, WebGL2 2236ms median (bring-up cost, not strip cost — but see w0-07: most of the WebGL2 figure was this probe's own Debug logging). Steady-state strip/submit cost: 0.6–1.4ms on both backends. | § 16 |
| w0-07 | *Why* does WebGL2 paint every glyph as a solid box, and where does its ~2.1s bring-up go? | **Found and fixed (fix proposed, not landed here).** `wgpu-hal` 30.0.1's GLES backend picks a texture's GL target from the texture descriptor alone, so the engine's single-layer atlas array binds as `GL_TEXTURE_2D` under a `sampler2DArray` and reads GLES' incomplete-texture value `(0,0,0,1)` — a solid box for a glyph, an opaque black rect for an image. A two-layer floor on the atlas fixes it, verified in Chrome. And **~1.5–1.8s of the 2.1s bring-up was this probe's own `Debug` console sink** printing naga's trace. | § 17 |

**The WebGL2 text defect: diagnosed, fix verified, not yet landed.** w0-03 § 5
recorded the symptom (every glyph a solid opaque box) and named the wrong
suspect — the strip shader's `alphas_texture` `textureLoad`. w0-07 § 17
exonerates that read (naga's GLSL-ES lowering of it is correct) and finds the
real cause one layer down, in `wgpu-hal` 30.0.1's GLES backend: it chooses a
texture's GL target from the `TextureDescriptor` alone
(`src/gles/mod.rs:513`), so `frust-engine`'s atlas array — a `D2` texture with
one array layer — is bound as `GL_TEXTURE_2D` while the shader samples it as a
`sampler2DArray`. The sampler's texture is then *incomplete* and GLES 3.0
returns `(0,0,0,1)`: a solid box for a glyph (`AlphaMask` tint × alpha 1.0)
and an opaque black rect for an image. Giving the atlas array a two-layer
floor makes wgpu-hal choose `GL_TEXTURE_2D_ARRAY`, and WebGL2 then renders
text, images, gradients, blur and layers identically to WebGPU
(`screenshots/w0-07-webgl-paints-fixed.png`). The fix is three one-line hunks
in `crates/frust-engine`, verified against the host Vulkan goldens — but it is
outside this spike's write scope, so § 17.7 files it as its own card.

**And a correction to w0-05's bring-up headline.** The WebGL2 arm's ~2.1 s
bring-up is real but mostly *this probe's own doing*: at the default `Debug`
log level, `console_log` prints naga's ~8,900-line typifier trace across the
wasm/JS boundary (the WebGPU backend never runs naga, so its console is 29
lines). Re-running the same build with `?log=info` drops WebGL2's first frame
from ~2.2–2.6 s to ~0.44–0.76 s against WebGPU's ~0.11–0.27 s — roughly 3–6×,
not 20×. What remains is naga's WGSL→GLSL translation plus ANGLE's shader
compile/link, one-off at bring-up; steady-state strip/submit cost is within
~1 ms of WebGPU's on both arms and is unaffected. RESULTS.md § 17.5 has the
tables and supersedes § 16.2/§ 16.3's WebGL2 rows.

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
http://localhost:8931/?arm=webgl&probe=paints    # w0-07: + gradient, blurred rrect, layer, image
http://localhost:8931/?arm=webgl&log=info        # w0-07: same build, naga's Debug trace silenced
```

The last two are w0-07's, both off by default so a plain `?arm=` run still
renders exactly the frame w0-03 screenshotted and w0-05 timed. `?probe=paints`
adds the four commands that reach the strip shader's *other* texture bindings
(`encoded_paints_texture`, `gradient_texture`, `layer_input_texture` and — via
an ordinary image draw — the same `atlas_texture_array` a glyph reads), which
is what separated "the glyph path is broken" from "every texture binding is
broken". `?log=info` is the measurement control described above.

The naga GLSL-ES lowering inspector (RESULTS.md § 17.2) is a native, dev-only
cargo example and needs no browser:

```sh
cargo run --example lower_strip -- fs_main   # or: -- vs_main
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
| `src/main.rs` | The probe. Target-independent scene/compile half (w0-02's native check, still green), plus a `wasm32`-only `web` module: canvas, winit loop, backend-restricted context, retrying bring-up, frame loop, logging, the w0-04 `?signal=timer` proof mode, the w0-05 `performance.now()` timing instrumentation (`timing:` log lines), and w0-07's `?probe=paints` scene, `?log=info` control and five-stage bring-up breakdown. |
| `index.html` | Host page. One page serves every arm/mode via `?arm=`/`?signal=`; carries the `<pre id="log">` evidence mirror every `timing:`/`frame N`/`verdict:` line is mirrored into. |
| `serve.sh` | `python3 -m http.server` on 127.0.0.1, free high port or an explicit one. `.wasm` is served as `application/wasm` (verified with `curl -I`). |
| `examples/lower_strip.rs` | w0-07's native, dev-only naga inspector: lowers `frust-engine`'s `strip.wgsl` to GLSL ES 3.00 with the exact options `wgpu-hal`'s GLES backend uses, so RESULTS.md § 17.2 quotes the real lowered source. A cargo *example* on a `[dev-dependencies]` `naga`, so the shipped `.wasm` is byte-identical with and without it. |
| `screenshots/` | The w0-03/w0-04 PNGs (RESULTS.md §§ 1, 15) and w0-07's five (§ 17.3/§ 17.4). w0-05 added no screenshots — it is a log/console-transcript measurement, not a visual one. |
| `README.md` | This file. |
| `RESULTS.md` | Full findings for all five tasks: the compile-probe flags table (w0-02), the render/text findings and the browser-driving recipe (w0-03), the signal-wake proof (w0-04), the size/timing tables (w0-05), and the WebGL2 glyph diagnosis + proposed fix + corrected bring-up breakdown (w0-07). |
| `Cargo.lock` | Committed, per the `examples/huddle` precedent above. |
| `pkg/` | `wasm-bindgen` output. Generated and gitignored. |
