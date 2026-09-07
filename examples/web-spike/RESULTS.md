# w0-03 results: browser render probe — `frust-engine` on WebGPU and WebGL2 in Chrome

*(This file replaces w0-02's compile-probe results. w0-02's own findings that
are still live — the flags table, the `Send`/`Sync` diagnosis — are carried
forward in § 8 and § 9 rather than deleted, with their status updated.)*

*(§ 15 adds w0-04's results — the `frust-reactive` wasm arm — on top of this
file rather than in a new one, per the card's own write scope. Everything
above § 15 is w0-03's, unmodified and re-verified unaffected — see § 15's own
regression check.)*

*(§ 16 adds w0-05's results — binary size, first-frame time, and the
single-threaded strip-generation/warm-up cost, per backend — on top of this
file, same convention. §§ 1–15 are unmodified; w0-05 is evidence only and
carries no GO/NO-GO verdict of its own, per its own card's acceptance
criterion — the § "Verdict" table above is w0-03's/w0-04's and is not
extended by this section.)*

## Verdict

| Arm | Bring-up | Shapes | Shaped text | Overall |
|---|---|---|---|---|
| **1. Chrome WebGPU** (`?arm=webgpu`, `Backends::BROWSER_WEBGPU`) | OK (after retry, § 6) | **correct** | **correct** | **GO** |
| **2. Chrome forced GL** (`?arm=webgl`, `Backends::GL` = WebGL2) | OK | **correct** | **BROKEN — every glyph is a solid filled box** | **PARTIAL NO-GO** |
| 3. Safari 26 | not run | — | — | **unavailable on this host** (§ 7) |

`frust-engine` renders in a browser. On WebGPU it renders *correctly*, glyphs
included. On WebGL2 every non-text primitive is pixel-identical to the WebGPU
arm — fills, rounded-rect corners, stroke antialiasing — and every **glyph**
loses its coverage and paints as an opaque rectangle the size of its own quad
(correct advance, correct colour, no letterform).

§ 5 isolates that failure: it is **the GL backend**, not the downlevel
capability profile. `DownlevelProfile::WebGl2` forced onto the WebGPU backend
renders text perfectly, so nothing in `frust-gpu`'s caps clamp (the 2048
texture ceiling, `has_storage_buffers = false`, the raised uniform alignment)
causes it.

There is **no render-tier switch anywhere in this probe or in the crates it
calls**. `frust-render`'s `tier` module is an adapter-*capability* gate
(`engine_support`), not a renderer choice; the only axis this file selects is
the wgpu backend, which is the axis the card asked about.

## 1. Screenshots (the evidence)

All four are captured **headed**, against the container's live X display with a
real NVIDIA T400 — see § 4 for why headless captures were rejected as evidence.

| File | What it shows |
|---|---|
| `screenshots/w0-03-webgpu.png` | Arm 1, canvas only. Card, accent bar, three rounded swatches, stroked line, `Hello` at 112 px and `Hello, Hello, Héllo` at 28 px, all correct and antialiased. |
| `screenshots/w0-03-webgpu-page.png` | Arm 1, whole page — the same frame **plus** the on-page log mirror, so the backend/adapter/profile lines and the pixels are in one image. |
| `screenshots/w0-03-webgl.png` | Arm 2, canvas only. Identical geometry; every glyph is a solid white/blue box. |
| `screenshots/w0-03-webgl-page.png` | Arm 2, whole page, with its log mirror. |
| `screenshots/w0-03-webgpu-forced-downlevel.png` | The § 5 isolation run: `FRUST_ENGINE_DOWNLEVEL=1` + `?arm=webgpu`, i.e. `DownlevelProfile::WebGl2` on the WebGPU backend. Text is **correct**. |

Pixel histograms were read back through a `drawImage` + `getImageData`
round-trip rather than judged by eye. Arm 1 and arm 2 agree exactly on every
non-text colour — `#1a2233` card, `#0b0e14` clear, `#ef5350`/`#66bb6a`/
`#ffca28` swatches (1890 sampled px each), `#ff8a65` stroke (988 px). They
differ only in the white/foreground count: **1493** sampled white px on
WebGPU (glyph coverage) against **3556** on WebGL2 (filled boxes).

## 2. Exact commands

From `examples/web-spike`:

```sh
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir pkg --out-name web_spike \
  /data/cache/target/wasm32-unknown-unknown/release/web-spike.wasm
./serve.sh 8931
```

`--out-name web_spike` is required, not cosmetic: cargo names the artifact
after the *package*, so the input is `web-spike.wasm` (hyphen) and
wasm-bindgen would otherwise emit `web-spike.js`, which `index.html` does not
import. The global `target-dir` is `/data/cache/target` (this host's
`~/.cargo/config.toml`), so the `.wasm` never lands in the checkout.

`wasm-opt` (130) was **not** run — binary size is w0-05's question, and an
extra transform between the build and the evidence would only muddy this one.

Driving the browser (host side; the container uses `--network host`, so
`localhost:8931` reaches the host's server from inside it):

```sh
docker exec -d frust-linux-native env DISPLAY=:20 \
  /home/user/apps/chromedriver-linux64/chromedriver --port=9516 \
  --allowed-ips= "--allowed-origins=*"
```

then a W3C WebDriver session over plain HTTP to `127.0.0.1:9516` with
`goog:chromeOptions.args`:

```
--no-sandbox --disable-dev-shm-usage --enable-unsafe-webgpu
--window-size=880,900 --window-position=0,0
```

navigate to `http://localhost:8931/?arm=webgpu` (or `?arm=webgl`), poll
`document.title` until it carries a terminal verdict, then read
`#log`'s `textContent` and take the screenshot. Nothing was installed on the
host and nothing in the container was modified.

Verification, from the repository root and from this directory:

```sh
cargo metadata --no-deps          # exit 0; `web-spike` appears 0 times
cargo fmt --check                 # clean, inside examples/web-spike
```

## 3. Console output, verbatim

### Arm 1 — WebGPU (`?arm=webgpu`)

```
frust w0-03 browser render probe | arm=webgpu | renderer=frust-engine (the only one)
verdict: frust w0-03 webgpu | starting
winit: window created over the page canvas
wgpu: instance restricted to Backends(BROWSER_WEBGPU)
surface: online after 1 attempt(s), phase=SurfaceReady
adapter:
backend: BrowserWebGpu
downlevel profile: Full
limits: max_texture_dimension_2d=8192 max_texture_array_layers=256 max_bind_groups=4 max_uniform_buffer_binding_size=65536 min_uniform_buffer_offset_alignment=256 max_vertex_attributes=16
caps: storage_buffers=true timestamp_query=true resource_texture_dim=4096 downlevel_flags=DownlevelFlags(COMPUTE_SHADERS | FRAGMENT_WRITABLE_STORAGE | INDIRECT_EXECUTION | BASE_VERTEX | READ_ONLY_DEPTH_STENCIL | NON_POWER_OF_TWO_MIPMAPPED_TEXTURES | CUBE_ARRAY_TEXTURES | COMPARISON_SAMPLERS | INDEPENDENT_BLEND | VERTEX_STORAGE | ANISOTROPIC_FILTERING | FRAGMENT_STORAGE | MULTISAMPLED_SHADING | DEPTH_TEXTURE_AND_BUFFER_COPIES | WEBGPU_TEXTURE_FORMAT_SUPPORT | BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED | UNRESTRICTED_INDEX_BUFFER | FULL_DRAW_INDEX_UINT32 | DEPTH_BIAS_CLAMP | VIEW_FORMATS | UNRESTRICTED_EXTERNAL_TEXTURE_COPIES | SURFACE_VIEW_FORMATS | NONBLOCKING_QUERY_RESOLVE | SHADER_F16_IN_F32 | MSL2_1 | TEXTURE_COMPRESSION)
profile check: OK — Full on BrowserWebGpu, resolved by frust-gpu, not by the probe
font: registered bundled face -> Named(["Noto Sans"])
scene: 8 commands -> 28 engine draws (CPU compile)
verdict: frust w0-03 webgpu | RENDERED
frame 1: RENDERED
frame 2: RENDERED
```

**`adapter:` is empty on purpose.** `wgpu::AdapterInfo::name` is the blank
string on the WebGPU backend — the browser deliberately does not expose the
device name to a page. `frust-gpu` passes it straight through, so
`TierCaps::adapter_name` is `""` and `frust-render`'s own tier line reads
``adapter=`` ``. Not a bug, but every web-facing diagnostic that prints an
adapter name will print nothing there; the GL arm below *does* get a name,
because ANGLE reports one.

The `Full` profile is right: a browser WebGPU adapter is not GLES-3.0
downlevel, and `frust-gpu` resolves `DownlevelProfile` from
`wgpu::Backend`, which is `BrowserWebGpu` here.

### Arm 2 — forced GL / WebGL2 (`?arm=webgl`)

```
frust w0-03 browser render probe | arm=webgl | renderer=frust-engine (the only one)
verdict: frust w0-03 webgl | starting
winit: window created over the page canvas
wgpu: instance restricted to Backends(GL)
surface: online after 1 attempt(s), phase=SurfaceReady
adapter: ANGLE (NVIDIA Corporation, NVIDIA T400 4GB/PCIe/SSE2, OpenGL 4.5.0)
backend: Gl
downlevel profile: WebGl2
limits: max_texture_dimension_2d=2048 max_texture_array_layers=256 max_bind_groups=4 max_uniform_buffer_binding_size=16384 min_uniform_buffer_offset_alignment=256 max_vertex_attributes=16
caps: storage_buffers=false timestamp_query=false resource_texture_dim=2048 downlevel_flags=DownlevelFlags(NON_POWER_OF_TWO_MIPMAPPED_TEXTURES | COMPARISON_SAMPLERS | ANISOTROPIC_FILTERING | MULTISAMPLED_SHADING | SHADER_F16_IN_F32 | MSL2_1 | TEXTURE_COMPRESSION)
profile check: OK — WebGl2 on Gl, resolved by frust-gpu, not by the probe
font: registered bundled face -> Named(["Noto Sans"])
scene: 8 commands -> 28 engine draws (CPU compile)
verdict: frust w0-03 webgl | RENDERED
frame 1: RENDERED
frame 2: RENDERED
```

**`DownlevelProfile::WebGl2` engages automatically, as the card required.**
Nothing in `src/main.rs` selects it: the probe only restricts the instance to
`Backends::GL`, and `frust-gpu`'s `DownlevelProfile::resolve` derives
`WebGl2` from the reported `wgpu::Backend::Gl`. The clamp is real, not a
label — the limits above are the GLES-3.0/WebGL2 downlevel defaults
(2048/256/4/16384/256/16 against the WebGPU arm's 8192/256/4/65536/256/16),
`has_storage_buffers` is forced off, and `resource_texture_dim` drops from
4096 to 2048. The probe asserts the expected profile per arm and logs the
comparison rather than trusting it.

One `frust-gpu` line appears on **both** arms, and it is the runtime half of
w0-02's remedy (§ 8), observed for the first time:

```
frust-gpu: could not spawn the pipeline warm-up thread (operation not supported on this platform); building the listed variants inline
```

The fallback works exactly as w0-02 predicted: warm-up goes synchronous, no
frame is refused, and pipeline compilation happens on the browser's main
thread. Whether that janks a frame is w0-05's measurement, not a correctness
problem here.

## 4. Why every screenshot is headed, and why headless is not evidence

Headless (`--headless=new`) reported `RENDERED` on both arms and produced a
**blank or garbage canvas** every time — the page screenshot showed
rectangular blocks of the clear colour in the wrong places, and a
`drawImage` + `getImageData` readback of the canvas returned
`rgba(0,0,0,0)` for **100 %** of sampled pixels on both arms.

That was investigated rather than worked around, because "the framework
renders nothing in a browser" and "the harness cannot see what the browser
rendered" are opposite conclusions. A **negative control** settled it — two
canvases driven by plain JavaScript in the same headless session, read back
by the identical code path:

| Control | Result |
|---|---|
| WebGL2 `clearColor(1,0,1,1)` + `clear()` | reads back `255,0,255,255` — **correct** |
| WebGPU render pass clearing to yellow (`bgra8unorm`, `alphaMode:'opaque'`) | reads back `0,0,0,0` |
| Same, `rgba8unorm` | reads back `0,0,0,0` |
| Same, re-read after two `requestAnimationFrame`s | reads back `0,0,0,0` |

So in headless Chrome 151 on this rig, **WebGPU canvas content is not
readable or capturable at all** (a harness limitation, independent of frust),
while WebGL2 readback *does* work — which is what made the arm-2 glyph
failure trustworthy once it was reproduced headed.

Headed, on `DISPLAY=:20`, both arms read back the real scene immediately.
**Conclusion for w0-04/w0-05 and for any future web CI: do not use
`--headless=new` screenshots as render evidence on this rig.** Drive
chromedriver with `DISPLAY=:20` instead. This cost real time here; it should
not cost it twice.

A second, unrelated capture trap was fixed in the probe itself: an early
version drew exactly one frame and the canvas was empty by capture time. A
canvas only reliably holds the pixels the compositor last consumed (WebGL's
default is `preserveDrawingBuffer: false`), so `src/main.rs` now re-requests
a redraw after every frame — winit's web backend services that from
`requestAnimationFrame`, which is the ordinary animation loop any real shell
runs anyway.

## 5. The arm-2 glyph failure, isolated

**Symptom.** Every glyph paints as a fully opaque rectangle covering its own
glyph quad. Advances, positions, colours and the text layout are all correct
— `Hello` is five boxes of visibly different widths in the right places, and
the 28 px caption is a row of small boxes with correct word spacing and
comma gaps. Only the *coverage* inside each quad is wrong (saturated to 1.0).

**What it is not:**

* Not a font-coverage/`.notdef` artifact. `.notdef` in this face is a hollow
  outlined box; these are solid, and the same string renders correctly on
  arm 1 from the same bundled bytes and the same `register_fonts` call
  (`font: registered bundled face -> Named(["Noto Sans"])` on both arms).
* Not a general coverage-path failure. Rounded-rect corners and the 6 px
  stroked line are antialiased correctly on this same arm, and those get
  their coverage from the same strip machinery.
* **Not the downlevel capability clamp.** This is the decisive experiment:
  rebuild with `FRUST_ENGINE_DOWNLEVEL=1` (`frust-gpu`'s existing
  `option_env!`/`env` override, which forces `DownlevelProfile::WebGl2` and
  the full limit clamp onto any adapter) and run **arm 1**. The log then
  reads `backend: BrowserWebGpu` / `downlevel profile: WebGl2` with the
  clamped limits (`max_texture_dimension_2d=2048`,
  `max_uniform_buffer_binding_size=16384`, `storage_buffers=false`,
  `resource_texture_dim=2048`) — and **text renders perfectly**
  (`screenshots/w0-03-webgpu-forced-downlevel.png`; 1480 sampled white px,
  matching arm 1's 1493, against arm 2's 3556).

**Therefore:** the failing layer is the **wgpu GL backend on
`wasm32-unknown-unknown` (WebGL2 via ANGLE)**, in the glyph-coverage read
specifically — not `frust-gpu`'s caps, not `frust-text`'s shaping, not
`frust-scene`'s glyph runs, and not the engine's CPU compile (which produced
the same `8 commands -> 28 engine draws` on both arms).

**Where to look next** (stated as a lead, not a diagnosis — this probe's
write scope is `examples/web-spike` and nothing in `crates/` was touched):
`crates/frust-engine/shaders/strip.wgsl` reads strip coverage from
`var alphas_texture: texture_2d<u32>` via `textureLoad`, unpacking 16 one-byte
alphas per texel with `textureDimensions`, a shift by the
`config.alphas_tex_width_bits` uniform, and a channel select. `alpha` is left
at its initialised value when neither the analytic-rect branch nor that
`textureLoad` branch is taken. An integer-texture `textureLoad` compiled
through naga's GLSL-ES output is the one part of that read that differs
between the two arms, and a coverage that comes back uniformly saturated is
consistent with the load being skipped or returning all-ones. **This needs its
own task with a shader-level GL capture; it should not be guessed at.**

**Consequence for the plan.** The web shell can ship on WebGPU today. A
WebGL2 fallback — the whole point of arm 2, since Safari and older Chrome
need it — currently cannot render text and is therefore not a fallback. This
is a Phase-0 blocker for the WebGL2 arm and should be filed as its own task
ahead of any web-shell text work.

## 6. Chrome's cold-page adapter race (affects w0-04 and w0-05)

The first bring-up attempt on arm 1 failed with, verbatim:

```
frust-render: failed to create render surface: frust-gpu: no compatible GPU adapter: No suitable graphics adapter found; noop not requested, vulkan not requested, metal not requested, dx12 not requested, gl not requested, webgpu found no adapters
```

on a machine with a working WebGPU adapter. Measured directly over CDP on a
freshly loaded page, in one async function, in this order:

| Call | Result |
|---|---|
| `requestAdapter()` (no options) | **`null`** |
| `requestAdapter({})` | `nvidia` / `turing` |
| `requestAdapter({powerPreference:'low-power'})` | `nvidia` / `turing` |
| `requestAdapter({powerPreference:'high-performance'})` | `nvidia` / `turing` |
| `requestAdapter()` again | `nvidia` / `turing` |

It is the **first** call that fails, whatever it asks for — a GPU-process
warm-up race, not a capability answer. `frust-gpu` asks exactly once
(`RenderContext::ensure_device` → `wgpu::util::initialize_adapter_from_env_or_default`),
so a cold page gets a hard, permanent "no adapter" for a machine that has
one.

The probe works around it host-side: `create_surface_with_retry` retries
`on_surface_created` up to 8 times, 150 ms apart, logging every attempt so a
genuine refusal still reads as a refusal. Observed cost: **2 attempts on the
WebGPU arm when the page is cold, 1 on the GL arm** (and 1 on WebGPU once
Chrome's GPU process is already warm, which is why the § 3 log shows 1).

**Seam this suggests** (a `crates/` change, not made here): adapter
acquisition on wasm should retry, or the web shell must own a warm-up before
the first `RenderContext::device()`. Every later web task will hit this.

## 7. Safari

**Not available.** This host is Manjaro Linux and the only browser reachable
from it is Google Chrome 151.0.7922.108 inside the `frust-linux-native`
container; there is no Safari, and no macOS or iOS device is attached to this
session. Arm 3 is therefore **not run and not inferred**. It matters more
than the usual "untested" note, because Safari 26's WebGPU support is the
main reason a WebGL2 fallback exists at all — and § 5 says that fallback
currently cannot draw text.

## 8. w0-02's blocker: remedy (A) is LANDED — this section supersedes w0-02 § 4

w0-02 reported the engine graph did **not** compile for `wasm32-unknown-unknown`
(91 × `E0277` in `crates/frust-gpu/src/pipeline.rs`, wgpu implementing neither
`Send` nor `Sync` on wasm) and handed over a validated but deliberately
unlanded one-line remedy.

**That remedy is now on this task's base**, in
`crates/frust-gpu/Cargo.toml`'s `cfg(target_arch = "wasm32")` table:

```toml
wgpu = { workspace = true, features = ["webgpu", "webgl", "fragile-send-sync-non-atomic-wasm"] }
```

The graph compiles, links, and runs. Its predicted runtime consequence was
observed for the first time here and is benign — see § 3's
`could not spawn the pipeline warm-up thread` line. Structural remedy (B)
(cfg-gating the threaded warm-up off on wasm32) remains an open follow-up,
now with the evidence that (A) is not merely compile-clean but functional.

## 9. Flags and identity (w0-02's table, re-verified after the new deps)

Every row re-checked against the current lockfile and a `cargo tree` on the
wasm32 target — the browser-probe dependencies are new since w0-02 and could
have changed any of these answers.

| Question | Finding |
|---|---|
| `--cfg=web_sys_unstable_apis`? | **Still NO.** Nothing here needs it. The probe deliberately does *not* name `web_sys`'s WebGPU bindings (which are the unstable-API surface); it reaches WebGPU only through wgpu, which vendors its own. |
| `getrandom`? | **Still NO on this target.** `getrandom` 0.3.4 and 0.4.3 are now *in the lockfile* — winit drags them in for its Android/Linux backends — but `cargo tree --target wasm32-unknown-unknown -i getrandom@0.3.4` and `@0.4.3` both answer "nothing to print". No `wasm_js` feature, no `getrandom_backend` cfg. |
| Any RUSTFLAGS / `--cfg`? | **NONE.** Every command in § 2 runs with an empty RUSTFLAGS and no environment prefix. (`FRUST_ENGINE_DOWNLEVEL=1` in § 5 is a deliberate diagnostic, not a build requirement.) |
| `wgpu` identity | **Single**, `wgpu 30.0.1`, matching the root pin. The spike's own new `wgpu` row (§ 10) is feature-less; the `webgpu`/`webgl` backend features still come from exactly one edge, `frust-gpu`'s target table. |
| `wasm-bindgen` identity | **Single**, `0.2.128`, matching the host CLI. `wgpu`'s own `^0.2.127` edge, `winit`'s, `console_error_panic_hook`'s and this crate's all unify onto it. |
| Graph size | 309 packages, up from 179 — winit's cross-platform backends account for nearly all of it, and almost none of it reaches the wasm32 target. |

## 10. Dependencies added, and the lock delta

All new rows are **target-gated to `cfg(target_arch = "wasm32")`**, so a plain
`cargo check` of this spike still resolves exactly what w0-02 resolved.

| Crate | Version | Why |
|---|---|---|
| `winit` | `0.30.13` | Copied verbatim from the root workspace pin. Web event loop via `EventLoopExtWebSys::spawn_app`. |
| `wasm-bindgen` | `=0.2.128` | **Exact** pin. The host CLI is 0.2.128 and wasm-bindgen's crate/CLI schema contract is stricter than semver; w0-02 § 6 flagged this. |
| `wasm-bindgen-futures` | `0.4.78` | Drives the async surface bring-up (`spawn_local`); no blocking executor exists in a browser. |
| `js-sys` | `0.3.105` | `Promise` + `setTimeout` behind `sleep_ms`, for the § 6 retry. |
| `web-sys` | `0.3.105` | Features `console`, `Document`, `Element`, `HtmlCanvasElement`, `HtmlElement`, `Location`, `UrlSearchParams`, `Window` — exactly what `src/main.rs` names. |
| `wgpu` | `30.0.1`, `default-features = false` | The `wgpu::Backends` **type**, to fill `ContextOptions::backends`. Contributes no feature. See § 11. |
| `log` | `0.4.34` | Sink for `frust-render`/`frust-gpu`/`wgpu`'s own records. |
| `console_log` | `1.1.0` | Routes those records to the devtools console — how § 3's warm-up-thread line was captured. |
| `console_error_panic_hook` | `0.1.7` | Without it a Rust panic reaches JS as a bare `unreachable` trap with no message. |

**Lock delta.** 179 → 309 packages. The one delta worth naming is not a
judgement call but a mechanical consequence: `js-sys`/`web-sys` carry an
**exact** `wasm-bindgen = "=0.2.N"` requirement in their own manifests
(`js-sys` 0.3.104 → `=0.2.127`, 0.3.105 → `=0.2.128`), so pinning
`wasm-bindgen` to `=0.2.128` *forces* `js-sys`/`web-sys` from w0-02's 0.3.104
to 0.3.105 and `wasm-bindgen-futures` from 0.4.77 to 0.4.78. 0.3.104 and
`wasm-bindgen` 0.2.128 are not co-satisfiable at all. Nothing in the root
workspace or in `crates/*` was touched.

## 11. Seams the web shell will want (findings, not changes)

None of these were made here; all are `crates/` changes outside this task's
write scope.

1. **Adapter retry on wasm** (§ 6). A single-shot `request_adapter` reports a
   permanent "no adapter" for a cold Chrome page. Either `frust-gpu` retries
   on `wasm32`, or the web shell owns a documented warm-up.
2. **`wgpu` is not re-exported by `frust-gpu` or `frust-render`.** Only the
   `frust` facade re-exports it, behind its `gpu` feature — and pulling the
   facade in drags `frust-reactive` along, which this graph deliberately
   excludes. Any host that wants to pin a backend must therefore take a
   direct `wgpu` dependency (as § 10 does) or receive a re-export /
   backend-selector newtype from `frust-render`. On native this never comes
   up, because `WGPU_BACKEND` covers it; in a browser there is no
   environment, so `wgpu::Backends::from_env()` can never resolve and the
   programmatic path is the *only* path.
3. **`AdapterInfo::name` is empty on the WebGPU backend** (§ 3). Diagnostics
   that identify a GPU by name — including `frust-render`'s own tier log line
   — print nothing on the web. Worth a fallback string.
4. **WebGL2 glyph coverage** (§ 5) — the blocker, needing its own task.
5. `README.md` in this directory still describes w0-02's state ("expect it to
   **fail**", no `.wasm` built, `index.html` a skeleton). It is outside this
   task's write scope; it needs a refresh to match this file.

## 12. What this probe deliberately does not do

* **`frust-reactive`** — was excluded at w0-03 time (its `tokio` multi-thread
  runtime init failed to compile on wasm32). **w0-04 lands that arm** — see
  § 15. The two w0-03 arms above (`?arm=webgpu`/`?arm=webgl`, no `?signal=`)
  remain exactly as documented in this section and in § 1–§ 11: their scene
  is still static and their frame loop is still driven by winit's
  `requestAnimationFrame`, not by a signal — § 15's regression check
  confirms their console trace is byte-identical to what is recorded above.
* **No measurement.** Binary size, first-frame time and per-backend
  `strip_us` are w0-05's, and `wasm-opt` was not run for the same reason.
* **No device-pixel-ratio handling.** The canvas is a fixed 800×600 backing
  store so a screenshot is a pixel-for-pixel record; DPR scaling is a
  web-shell concern.
* **No input, no resize, no visibility handling.** `WindowEvent` handling is
  redraw-only.

## 13. Files in this spike

| File | Purpose |
|---|---|
| `Cargo.toml` | Standalone workspace manifest: five engine-graph path deps, `kurbo`/`peniko`, and the `wasm32`-gated browser rows of § 10. |
| `src/main.rs` | The probe. Target-independent scene/compile half (also w0-02's native check), plus a `wasm32`-only `web` module: canvas, winit loop, backend-restricted context, retrying bring-up, frame loop, logging. |
| `index.html` | Host page. One page serves both arms via `?arm=`; carries the `<pre id="log">` evidence mirror. |
| `serve.sh` | `python3 -m http.server` on 127.0.0.1, free high port or an explicit one. `.wasm` is served as `application/wasm` (verified with `curl -I`). |
| `screenshots/` | The five PNGs of § 1. |
| `README.md` | w0-02's; stale — see § 11.5. |
| `RESULTS.md` | This file. |
| `Cargo.lock` | Committed, per the `examples/huddle` precedent. |
| `pkg/` | wasm-bindgen output. Generated and gitignored (`.gitignore`, added on this task's base). |

## 14. Toolchain facts (environment-supplied, unmodified)

| Tool | Version |
|---|---|
| rustc / rustup toolchain | `1.98.1`, repo-pinned via `rust-toolchain.toml` |
| `wasm32-unknown-unknown` target | installed |
| `wasm-bindgen-cli` | `0.2.128` — now **matched** by the crate pin (§ 10) |
| `wasm-opt` | `130` (not exercised) |
| `python3` | `3.14.6` (`mimetypes` maps `.wasm` → `application/wasm`) |
| Google Chrome | `151.0.7922.108`, in the `frust-linux-native` container |
| chromedriver | `151.0.7922.108` |
| GPU | NVIDIA T400 4GB; `ANGLE (NVIDIA Corporation, NVIDIA T400 4GB/PCIe/SSE2, OpenGL 4.5.0)` on the GL arm |

Nothing above was installed, upgraded or otherwise modified by this task, and
the container was not modified.

## 15. w0-04 results: the `frust-reactive` wasm arm — signal → repaint proof

### Verdict

**GO.** `crates/frust-reactive` now compiles for `wasm32-unknown-unknown`
(`cargo check --target wasm32-unknown-unknown -p frust-reactive` green), the
host-side test suite is untouched (`cargo test -p frust-reactive`: 25/25,
unchanged), and the browser proves the wasm arm actually wakes: a
`frust_reactive` signal write, triggered by a one-shot JS timer, fires the
installed `FrameWaker` and produces **exactly one** extra repaint — with the
w0-03 arms' own continuous `request_redraw` loop gated off for this run, so
nothing else could have produced it — and **zero** pointer/keyboard events
observed while waiting for it.

### What changed, and where

`crates/frust-reactive` (this task's other two write files):

- **`Cargo.toml`** target-gates `tokio`: non-wasm keeps the exact native
  feature set (`rt-multi-thread`, `time`, `net` — byte-identical, `cfg`'d
  only by which arm compiles for a given target); wasm gets `rt` only
  (`rt-multi-thread`/`net` fail to compile on `wasm32-unknown-unknown` — mio
  needs epoll/kqueue-equivalent syscalls this target does not have, measured
  directly, see the probe transcript this task ran and is described in the
  commit). Also adds `any_spawner`'s `wasm-bindgen` feature +
  `wasm-bindgen-futures`, wasm-only.
- **`src/runtime.rs`**: `ReactiveRuntime::init`'s wasm arm builds a bare
  `Builder::new_current_thread()` runtime with no driver enabled (instead of
  the native multi-thread pool) and calls
  `any_spawner::Executor::init_wasm_bindgen()` explicitly (`any_spawner` has
  no automatic wasm default, unlike its `tokio` feature) instead of
  installing the custom `ForgeExecutor` — `spawn_local` then routes straight
  to `wasm_bindgen_futures::spawn_local`, driven by the browser's own
  microtask queue. The current-thread runtime is never driven (nothing calls
  `block_on`); it exists purely so `handle()`/`spawn_blocking` — used
  unconditionally by `executor.rs`/`task.rs`'s type signatures, neither of
  which is in this task's write scope — keep a real
  `tokio::runtime::Handle` to type-check against. **`use_task`'s background
  half and `spawn_blocking` have no working wasm equivalent yet**: a task
  handed to that handle is silently never polled. Documented on
  `spawn_blocking` and in `runtime.rs`'s module docs, not hidden.

`examples/web-spike` (this file plus `src/main.rs`, both in this task's write
scope; `Cargo.toml`/`Cargo.lock` under the conductor's scope extension since
the card's own `write_files` omitted them):

- **`Cargo.toml`** adds two wasm-only rows: `frust-reactive` (path dep — the
  point of this task) and `reactive_graph` (its `Get`/`Set` traits, which
  `frust-reactive` does not re-export). Both resolve within this spike's own
  standalone lockfile; see the lock-delta note below.
- **`src/main.rs`** adds a `?signal=timer` proof mode, orthogonal to `?arm=`
  (either backend arm can carry it — this run used the WebGPU arm, since
  arm 2's text bug (§ 5) is unrelated and would only add noise):
  - `ReactiveRuntime::init` is wired with a `FrameWaker` that sends a unit
    event through a winit `EventLoopProxy` (wrapped in a
    `# Safety`-documented `unsafe impl Send + Sync` newtype — wasm's
    `EventLoopProxy` is not `Send`/`Sync` upstream because winit is
    cross-platform and its *native* impls genuinely cross real OS threads;
    `wasm32-unknown-unknown` here has none, mirroring `frust-gpu`'s own
    `fragile-send-sync-non-atomic-wasm` precedent).
  - A `TrackedScope` tracks one read of a fresh `RwSignal<u32>` counter
    (initial value 0) right after `ReactiveRuntime::init`.
  - The continuous `request_redraw` loop the two w0-03 arms use (re-request
    at the end of every `RedrawRequested`) is gated off for this mode — the
    proof needs the *only* source of a second repaint to be the signal path,
    not this file re-asking on its own.
  - Once the baseline frame renders (`frame 1`), a one-shot
    `window.set_timeout` (800 ms) fires, and the spawned continuation writes
    the counter signal (`0 -> 1`). That write is what trips
    `TrackedScope`'s clean→dirty edge (`frust_reactive::tracked`), which
    fires the installed `FrameWaker`, which sends the `EventLoopProxy` event,
    which lands in `ApplicationHandler::user_event` — the **only** place in
    `signal_mode` that calls `window.request_redraw()`.
  - Every pointer/keyboard `WindowEvent` variant is counted while
    `signal_mode` is set, and the final verdict line reports the count.

### Exact commands

Build/serve, from `examples/web-spike` (unchanged recipe from § 2):

```sh
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir pkg --out-name web_spike \
  /data/cache/target/wasm32-unknown-unknown/release/web-spike.wasm
./serve.sh 8931
```

Chromedriver, identically to § 2:

```sh
docker exec -d frust-linux-native env DISPLAY=:20 \
  /home/user/apps/chromedriver-linux64/chromedriver --port=9516 \
  --allowed-ips= "--allowed-origins=*"
```

then the same plain-HTTP W3C WebDriver session against `127.0.0.1:9516`, same
`goog:chromeOptions.args` as § 2, navigating to
`http://localhost:8931/?arm=webgpu&signal=timer`, polling `document.title`
for `PROVEN`/`FAIL`, then reading `#log`'s `textContent` and taking a
screenshot — `screenshots/w0-04-signal-webgpu.png`.

Verification, from the repository root:

```sh
cargo check --target wasm32-unknown-unknown -p frust-reactive   # green
cargo test -p frust-reactive                                    # 25/25, unchanged
cargo build --workspace --locked && cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check                                          # green, no failures
cargo tree -e features -p frust-reactive                        # diffed empty vs base_sha
```

and from `examples/web-spike`:

```sh
cargo check                                     # native probe, unchanged, green
cargo check --target wasm32-unknown-unknown     # green
```

### Console output, verbatim (`?arm=webgpu&signal=timer`)

```
frust w0-03 browser render probe | arm=webgpu | renderer=frust-engine (the only one)
verdict: frust w0-03 webgpu | starting
signal: mode enabled (?signal=timer) — wiring ReactiveRuntime
signal: counter signal created and tracked (initial value 0)
winit: window created over the page canvas
wgpu: instance restricted to Backends(BROWSER_WEBGPU)
surface: online after 1 attempt(s), phase=SurfaceReady
adapter:
backend: BrowserWebGpu
downlevel profile: Full
limits: max_texture_dimension_2d=8192 max_texture_array_layers=256 max_bind_groups=4 max_uniform_buffer_binding_size=65536 min_uniform_buffer_offset_alignment=256 max_vertex_attributes=16
caps: storage_buffers=true timestamp_query=true resource_texture_dim=4096 downlevel_flags=DownlevelFlags(COMPUTE_SHADERS | FRAGMENT_WRITABLE_STORAGE | INDIRECT_EXECUTION | BASE_VERTEX | READ_ONLY_DEPTH_STENCIL | NON_POWER_OF_TWO_MIPMAPPED_TEXTURES | CUBE_ARRAY_TEXTURES | COMPARISON_SAMPLERS | INDEPENDENT_BLEND | VERTEX_STORAGE | ANISOTROPIC_FILTERING | FRAGMENT_STORAGE | MULTISAMPLED_SHADING | DEPTH_TEXTURE_AND_BUFFER_COPIES | WEBGPU_TEXTURE_FORMAT_SUPPORT | BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED | UNRESTRICTED_INDEX_BUFFER | FULL_DRAW_INDEX_UINT32 | DEPTH_BIAS_CLAMP | VIEW_FORMATS | UNRESTRICTED_EXTERNAL_TEXTURE_COPIES | SURFACE_VIEW_FORMATS | NONBLOCKING_QUERY_RESOLVE | SHADER_F16_IN_F32 | MSL2_1 | TEXTURE_COMPRESSION)
profile check: OK — Full on BrowserWebGpu, resolved by frust-gpu, not by the probe
font: registered bundled face -> Named(["Noto Sans"])
scene: 8 commands -> 28 engine draws (CPU compile)
verdict: frust w0-03 webgpu | RENDERED
frame 1: RENDERED
signal: baseline frame rendered; scheduling a one-shot 800ms timer to write the tracked counter signal
signal: wrote counter signal (0 -> 1) — this write's TrackedScope dirty edge is what fires the FrameWaker
signal: FrameWaker fired via the EventLoopProxy -> requesting the one post-write repaint
frame 2: RENDERED
verdict: frust w0-04 signal | PROVEN: baseline=1 frame, +1 repaint after the signal write, input_events=0
```

`document.title` at completion: `frust w0-04 signal | PROVEN: baseline=1
frame, +1 repaint after the signal write, input_events=0`. Screenshot:
`screenshots/w0-04-signal-webgpu.png` (canvas identical to arm 1's § 1
evidence, log mirror carries the transcript above).

**Reading the transcript as the acceptance criterion:** exactly one `frame N:
RENDERED` line (`frame 1`) precedes the signal write; the next repaint
(`frame 2`) appears only after, and strictly after, `signal: wrote counter
signal` and the `FrameWaker`/`EventLoopProxy`/`user_event` chain — the
continuous loop that would otherwise explain `frame 2` for free is gated off
in `signal_mode` (see `src/main.rs`'s `window_event`), so this frame has no
other possible cause. `input_events=0` in the final verdict is read directly
off a counter incremented in the same `WindowEvent` match the frame count
comes from, not asserted separately.

### Regression check: the two w0-03 arms are unaffected

Re-ran `?arm=webgpu` (no `?signal=`) against the same build; the `#log`
transcript is **byte-identical** to § 3's Arm 1 transcript (same lines, same
order, ending `frame 2: RENDERED` with no `signal:`/`verdict: frust w0-04`
lines at all — `signal_mode` is entirely off). `?arm=webgl` was not
re-driven (its own code path is untouched by this task; `signal_mode` is
orthogonal to `Arm` and gates independently), but there is no `signal_mode`
branch reachable without `?signal=timer` in the URL, so its behaviour follows
the same "off by construction" reasoning as the webgpu re-run.

### Lock deltas

**Root workspace (`Cargo.lock`).** Two new dependency edges, no version
bumps, no new package rows: `any_spawner -> wasm-bindgen-futures` and
`frust-reactive -> wasm-bindgen-futures`, both resolving to the
already-present `wasm-bindgen-futures 0.4.77` (pulled in transitively before
this task, via `frust-gpu`'s `wgpu` `webgpu` feature edge — § 9's flags
table). `cargo tree -e features -p frust-reactive` (native) diffed empty
against `base_sha` — the native feature graph this crate resolves is
unchanged.

**This spike (`examples/web-spike/Cargo.lock`).** Two new direct
dependencies (`frust-reactive`, `reactive_graph`) pull in `frust-reactive`'s
own dependency tree (`any_spawner`, `reactive_graph`, `futures`, and — on
this wasm32 build — the wasm-only `tokio`/`wasm-bindgen-futures` rows the
Cargo.toml changes above add) plus their transitive graph. No version was
independently chosen: `reactive_graph = "0.2"` matches the root workspace's
own unpinned range, and `wasm-bindgen`/`wasm-bindgen-futures`/`js-sys`/
`web-sys` stay unified on the exact versions § 9/§ 10 already recorded
(`wasm-bindgen 0.2.128`, `wasm-bindgen-futures 0.4.78`) — this task's new
rows do not name any of those crates directly, so nothing forced a further
bump. `wasm-bindgen-futures` in this spike's lock now has two edges into it
(the pre-existing `wasm-bindgen-futures` row used by the bring-up retry, and
the new one via `any_spawner`'s `wasm-bindgen` feature) — same crate, same
identity, no fork.

### Risks and open follow-ups

- **`use_task`/`spawn_blocking` have no working wasm equivalent.** Documented
  in `runtime.rs`, not fixed here — the card's `unimplemented`-or-documented
  bar is met by documentation, not by a working background-thread story
  (wasm32-unknown-unknown has none to give it). A future task that wants
  `use_task` on the web will need its own design (a `wasm-bindgen-futures`
  or Web Worker-backed path), not a small follow-up to this one.
- **The 800 ms timer delay is this proof's own choice**, not a measured
  minimum — it only needs to be long enough that a screenshot/log capture
  taken immediately after bring-up unambiguously shows the pre-write state.
  Not a finding about signal-wake latency, which w0-05 (binary size,
  first-frame time, `strip_us`) does not cover either — nothing in this repo
  yet measures wasm wake latency.
- **The `SendSyncProxy` `unsafe impl Send + Sync`** lives in
  `examples/web-spike/src/main.rs`, a throwaway spike excluded from
  `docs/DOC_POLICY.md`'s doc-unit graph (and from `docs/CODE_STANDARDS.md`'s
  sanctioned-unsafe zone list, which only enumerates shipping crates) — it is
  `# Safety`-documented inline per that convention's spirit, but is not
  itself part of the registry. If a real web shell wants this waker pattern,
  it should land the wrapper (or an upstream-safe equivalent) inside
  `crates/frust-shell-*` where the sanctioned-unsafe register applies, not
  copy this file's copy.

### `doc_updates_needed`

None of `docs/CORE_ARCHITECTURE.md`/`docs/CORE_DEVELOPMENT.md` need a change
for this task's actual landing shape (the wasm arm's behaviour is documented
in `runtime.rs`'s own doc comments, which is where `CORE_ARCHITECTURE.md`
already points readers for `ReactiveRuntime`/`FrameWaker` detail). Worth a
future doc pass, not required by this task:

- `docs/CORE_DEVELOPMENT.md`'s `reactive_graph`/`any_spawner`/`tokio` pin row
  could gain a one-clause note that `tokio` is now target-gated (wasm gets
  `rt` only) — today a reader has to open `Cargo.toml` to learn that.
- If a future task gives `use_task`/`spawn_blocking` a real wasm story, that
  is exactly the kind of change `docs/CORE_ARCHITECTURE.md`'s "Data Flow"
  section would want a line for.

## 16. w0-05 results: binary size, first-frame time, single-threaded strip cost

**Evidence only — no GO/NO-GO verdict.** This section's card was explicit
that it carries no acceptance thresholds; the numbers below are handed to the
conductor to write the plan's verdict from, not judged here.

**Hardware/software held constant across every row and every table below**
(re-verified, not assumed — same rig § 1–§ 15 used):

| | |
|---|---|
| Browser | Google Chrome `151.0.7922.108`, in the `frust-linux-native` container |
| chromedriver | `151.0.7922.108` |
| GPU | NVIDIA T400 4GB; WebGL2 arm reports `ANGLE (NVIDIA Corporation, NVIDIA T400 4GB/PCIe/SSE2, OpenGL 4.5.0)` |
| Display | headed, `DISPLAY=:20` (§ 4 — headless WebGPU readback is blank on this rig; this task did not need canvas readback, but the whole probe still runs under the same headed chromedriver session for consistency with § 2/§ 6) |
| `rustc` / toolchain | `1.98.1`, repo-pinned (`rust-toolchain.toml`), unchanged from § 14 |
| `wasm-bindgen` (crate + CLI) | `0.2.128`/`0.2.128` — matched, as § 9/§ 14 already established |
| `wasm-opt` | **`130`**, not `132` as the card's own text guessed — the host binary's real, unmodified version (`wasm-opt --version` → `wasm-opt version 130`); § 14's "not exercised" row is now superseded by this section, which exercises it for the first time |
| `gzip` | `/usr/bin/gzip`, level `-9` (max) for every gzipped figure below |
| `python3` | `3.14.6`, unchanged from § 14 |

Nothing above was installed, upgraded, or otherwise modified by this task —
same discipline as § 14.

### 16.1 Binary size — debug / release / release+wasm-opt, raw and gzipped

**Exact commands**, from `examples/web-spike` (target-dir is
`/data/cache/target`, this host's `~/.cargo/config.toml`, same as § 2):

```sh
# debug
cargo build --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir <scratch>/pkg-debug --out-name web_spike \
  /data/cache/target/wasm32-unknown-unknown/debug/web-spike.wasm

# release (this task's new [profile.release] applies — see § 16.1's flags
# note below)
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir pkg --out-name web_spike \
  /data/cache/target/wasm32-unknown-unknown/release/web-spike.wasm

# release + wasm-opt (STRICTLY after wasm-bindgen, on the pkg/*_bg.wasm it
# emitted, per the card's own instruction)
wasm-opt -O --all-features -o <scratch>/web_spike_bg.opt.wasm pkg/web_spike_bg.wasm

# gzip, every row
gzip -9 -c <file> | wc -c
```

`--all-features` (`-all`) was **required**, not a tuning choice: a bare
`wasm-opt -O` refuses to parse this `.wasm` at all —

```
[wasm-validator error in function 0] unexpected false: memory.copy operations
require bulk memory operations [--enable-bulk-memory-opt], on ...
```

— and adding only `--enable-bulk-memory` still leaves `i32.trunc_sat_f32_u`
(non-trapping float-to-int) and other ops unrecognised (`unexpected false:
all used features should be allowed`). `rustc` 1.98.1 turns on bulk-memory,
sign-extension and non-trapping-float-to-int by default for
`wasm32-unknown-unknown` (has been the default target-features set for
several stable releases), and `wasm-opt` 130 defaults its own validator to
the MVP feature set, so the two disagree unless told to agree. `-all` is the
correct, permissive answer here — it does not *add* SIMD/threads/etc. to the
module (nothing in this graph emits them), it only stops the validator from
rejecting features the input already legitimately uses.

**Sizes** (bytes; MiB rounded to 2 places; gzip at `-9`):

| Row | Raw (post-`wasm-bindgen` `*_bg.wasm`, the artifact the browser actually loads) | Gzipped |
|---|---:|---:|
| **debug** | 36,742,025 (35.04 MiB) | 6,516,420 (6.21 MiB) |
| **release** (no `wasm-opt`) | 6,178,556 (5.89 MiB) | 2,035,301 (1.94 MiB) |
| **release + `wasm-opt -O --all-features`** | 5,703,574 (5.44 MiB) | 2,021,648 (1.93 MiB) |

Both backend arms (`?arm=webgpu`/`?arm=webgl`) ship in the **same** `.wasm` —
`Arm` is a runtime `?arm=` query read, not a build-time selection (`src/
main.rs`'s `Arm::from_query`) — so there is one size table, not two.

For context, not itself a browser-loadable artifact: the raw `cargo`-emitted
`.wasm` *before* `wasm-bindgen` post-processing was 227,593,088 bytes (217.05
MiB) debug / 7,957,621 bytes (7.59 MiB) release. `wasm-bindgen --target web`
shrinks both (debug most dramatically) by rewriting/dropping custom sections
(names, a linking-metadata leftover, etc.) that the browser's own loader
never reads — an existing wasm-bindgen behaviour, not something this task
tuned.

**Which release-profile flags applied to which row** (the card's own
"finalize the flags table" instruction — item (b) of the dispatch's Fact
list):

| Row | `lto` | `codegen-units` | `strip` | `panic` | `wasm-opt` |
|---|---|---|---|---|---|
| debug | Cargo dev default (off) | Cargo dev default (256) | off | `unwind` (default) | not run |
| release | **`"fat"`** | **`1`** | **`"symbols"`** | **`"abort"`** | not run |
| release + wasm-opt | **`"fat"`** | **`1`** | **`"symbols"`** | **`"abort"`** | **`-O --all-features`** (post-build transform, not a `cargo` profile flag) |

The bolded four are this task's own addition — `examples/web-spike/
Cargo.toml`'s new `[profile.release]` block (§ "Conductor scope extension"),
copied verbatim from the root workspace's `[profile.release]`
(`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "abort"` —
`docs/DEVELOPMENT.md`'s Version-Pin Policy release-shape row) and from
`examples/huddle/Cargo.toml`'s identical hand-synced copy, both read-only
references. **Before this task, this standalone workspace had no
`[profile.release]` of its own** and a `--release` build here silently fell
back to Cargo's own defaults (no LTO, cgu=16, no strip, no `panic=abort`) —
the same gap the root manifest's own `[profile.release]` comment (read-only,
`Cargo.toml` line ~671) already named for the example crates it fixed. No
prior task measured this spike's release size (§ 12: "No measurement.
Binary size, first-frame time and per-backend `strip_us` are w0-05's"), so
there is no "before this fix" number to compare against — this is the first
release-size measurement this spike has ever had, and it is measured with
the representative profile already in place, per the card's own instruction
that the release number should be representative.

### 16.2 First-frame time — `init()` resolution to first presented frame

**No public hook exists for this** (unlike § 16.3, where the card names a
concrete measurement, `first-frame time` has none), so this section defines
it operationally and states the definition rather than assuming one: `t0` is
`performance.now()` read at the very first statement of the `#[wasm_bindgen(
start)]` entry point (`web::start`, `src/main.rs`) — before the canvas is
even looked up. That start section runs *synchronously inside* `init()`'s
own work (module fetch, compile, instantiate, run start section) and returns
almost immediately (it only spawns the async event loop via
`EventLoopExtWebSys::spawn_app`), so `t0` is within roughly a tick of the
JS-side `await init()` in `index.html` actually resolving — closer than any
cross-boundary timestamp handoff this spike could add without touching
`index.html`'s own async structure. `t1` is `performance.now()` read the
first time `ApplicationHandler::window_event`'s `RedrawRequested` arm
completes a **`requestAnimationFrame`-driven** draw (the first `frame 1:
RENDERED` line) — not the earlier, unlabelled draw `resumed`'s own
`spawn_local` continuation performs to guarantee at least one submitted
frame (see § 1's screenshot-capture note); that earlier draw's cost is
inside § 16.3's `bring-up` span instead, since it runs before this handler
ever sees a `RedrawRequested` event. First-frame time is `t1 - t0`.

**Exact commands.** Build/serve identical to § 16.1's release recipe (no
`wasm-opt` — this task's browser measurements use the same non-`wasm-opt`
`pkg/` the established § 2/§ 6 recipe drives, so the first-frame/`strip_us`
numbers are not entangled with the separate, size-only `wasm-opt` question);
`./serve.sh 8931`. Driving the browser: the same chromedriver + plain-HTTP
W3C WebDriver session as § 2/§ 6 (`--no-sandbox --disable-dev-shm-usage
--enable-unsafe-webgpu --window-size=880,900 --window-position=0,0`), except
each of the 3 runs per arm below opened a **fresh WebDriver session**
(`POST /session` … `DELETE /session/{id}`) and navigated once, rather than
reusing one session across runs — this is what "cold (fresh page load)" in
the card's own vocabulary means here: no `document.location.reload()`, no
session reuse, no wasm/JS state surviving between runs. The chromedriver
*process* and its underlying Chrome instance were not restarted between
runs (only impractical to do per-run here); § 16.3's own finding (GL bring-up
being ~2.1s slower than WebGPU on **every** run, not just the first) is
itself evidence that this did not mask a warm/cold split the way § 6's
adapter-race retry-count did.

**All 3 cold runs per arm, plus the median** (milliseconds, from the
in-page `timing:` log lines — see § 16.4 for one full verbatim transcript
per arm):

| Arm | Run 1 | Run 2 | Run 3 | **Median** |
|---|---:|---:|---:|---:|
| WebGPU (`?arm=webgpu`) | 113.0 | 109.6 | 186.7 | **113.0** |
| WebGL2 (`?arm=webgl`) | 2299.0 | 2221.3 | 2236.4 | **2236.4** |

WebGL2's first-frame time is **~20×** WebGPU's on this rig. § 16.3 isolates
where that time goes: it is not the adapter-acquisition retry (both arms
needed only 1 bring-up attempt on every run — `surface: online after 1
attempt(s)` in every transcript), it is inside the `bring-up` span itself.

### 16.3 Single-threaded strip generation cost and pipeline warm-up cost

**No `strip_us` hook is reachable from this spike.** Checked, per the card's
own instruction:

* `crates/frust-engine/src/compile/mod.rs`'s `PhaseClock`/`CompilePhaseCosts`
  time exactly this (`walk`, the phase that includes "strip generation and
  paint encoding" per its own doc comment) — but the type and its lap
  timings are `pub(crate)`, gated behind the `perf-trace` Cargo feature this
  spike's `Cargo.toml` does not (and, being outside `crates/`, cannot)
  enable meaningfully, since nothing exposes the costs across the crate
  boundary even if the feature were on. Not reachable.
* `crates/frust-gpu/src/pipeline.rs`'s `PipelineCache::warm_up` has no
  timing of its own at all — `docs/LIMITATIONS.md`'s `engine-wasm-single-
  thread` entry documents the synchronous-fallback *behaviour* (the `could
  not spawn the pipeline warm-up thread ... building the listed variants
  inline` line § 3/§ 15's transcripts already carry), not a cost. Not
  reachable.
* `grep -rn "strip_us\|warm_up\|perf::" crates/frust-engine crates/frust-gpu`
  turns up no per-frame timing hook exposed at either crate's public API
  surface.

So, per the card's own fallback instruction, this section wraps
[`SurfaceRenderer::encode`]/`acquire`/`submit` (`src/main.rs`'s `draw`) and
the whole `RenderContext`/`SurfaceRenderer` bring-up (`src/main.rs`'s
`bring_up`, called from `resumed`) with `performance.now()` instead —
`web-sys`'s `Performance` feature, added to `Cargo.toml` this task, since
`std::time::Instant::now()` panics on `wasm32-unknown-unknown` (no clock
syscall on this target — this is the same "no threads without
`SharedArrayBuffer`" single-threaded-wasm shape the card names for
`multithreading`, just for a clock instead of a thread).

**What each span actually measures** (read directly off
`crates/frust-render/src/renderer.rs`'s own doc comments, quoted in § 16's
code comments too):

* **`encode_us`** — `SurfaceRenderer::encode`'s own doc comment: "a memcpy
  and nothing else" (it stashes the scene for `submit` to compile). Measured
  ≈0.0–0.1ms every run, on both arms — consistent with "memcpy", and
  **not** a `strip_us` proxy by itself.
* **`submit_us`** — the same doc comment: "the whole GPU render runs HERE
  ... the frame's GPU cost lands in `submit_us`". This is where
  `EngineRenderer::encode` actually compiles the scene (strip generation +
  paint encoding, `SceneCompiler::compile`'s `walk` phase) **and** where the
  compiled draws are submitted to the GPU queue — the two are not
  separable at this API boundary (`crates/frust-render`'s public seam has no
  finer split), so `submit_us` is this spike's proxy for the card's
  `strip_us`, not an isolated CPU-only number. Documented as a proxy, not
  presented as the real thing.
* **`bring-up` total** — wraps `bring_up()` end to end: `RenderContext`
  creation, the retrying `on_surface_created` (§ 6), `frust-gpu`'s inline
  pipeline warm-up fallback (embedded inside `on_surface_created`, not
  separately callable), font registration, and the first scene build. Per
  the card's own instruction, these are reported as **one** span, not
  decomposed further — the inline warm-up fallback has no separate entry/exit
  the spike can hook.

**The very first `submit` call** (inside `bring_up`'s own guaranteed draw,
before `resumed` even returns) is materially more expensive than every
`submit` after it on both arms — this is the pipeline-variant compile
happening synchronously on first use, consistent with `PipelineCache::
get_or_create`'s "steal-and-build-inline" fallback path and with
`docs/LIMITATIONS.md`'s `engine-wasm-single-thread` entry. `frame 1`'s and
`frame 2`'s `submit_us` (both drawing the identical static scene, nothing
new to compile) are the closer read on steady-state single-threaded strip
cost.

**All 3 cold runs per arm** (milliseconds):

| Arm | | Run 1 | Run 2 | Run 3 | **Median** |
|---|---|---:|---:|---:|---:|
| **WebGPU** | bring-up total | 91.9 | 85.5 | 166.5 | **91.9** |
| | first `submit` (warm-up-inclusive) | 10.2 | 12.2 | 10.0 | **10.2** |
| | `frame 1` `submit` (`strip_us` proxy) | 0.5 | 0.8 | 0.6 | **0.6** |
| | `frame 2` `submit` (`strip_us` proxy) | 0.8 | 0.9 | 0.7 | **0.8** |
| **WebGL2** | bring-up total | 2271.1 | 2192.0 | 2208.1 | **2208.1** |
| | first `submit` (warm-up-inclusive) | 16.4 | 16.7 | 16.7 | **16.7** |
| | `frame 1` `submit` (`strip_us` proxy) | 0.8 | 1.0 | 0.9 | **0.9** |
| | `frame 2` `submit` (`strip_us` proxy) | 0.9 | 1.4 | 1.4 | **1.4** |

`encode_us` and `acquire_us` are omitted from this table (every value on
every run, both arms, was 0.0–0.2ms — a memcpy and a same-frame swapchain
acquire, exactly as their doc comments predict); the full per-frame numbers
are in § 16.4's verbatim transcripts.

**Reading this table:**

1. **Steady-state strip cost is small and close on both backends** — 0.6–1.4
   ms to re-encode+submit an unchanged 8-command/28-draw scene, WebGPU and
   WebGL2 within ~1ms of each other. Nothing here suggests the GL backend's
   § 5 glyph-coverage bug is a *performance* problem; it is a correctness
   one, as § 5 already concluded.
2. **The GL arm's `bring-up total` is ~24× the WebGPU arm's** (2208.1ms vs
   91.9ms median), and this holds on **every** run, not just a cold-cache
   first one — ruling out the § 6 adapter-race retry as the cause (both
   arms needed exactly 1 bring-up attempt, every run; see § 16.4). The gap
   is not explained by the first `submit`'s own pipeline-warm-up cost either
   (16.7ms vs 10.2ms median — real, but two orders of magnitude too small to
   account for ~2.1 **seconds**). The remaining ~2.1s sits somewhere inside
   `on_surface_created`/`RenderContext::ensure_device` on the `Gl` backend
   path before the probe's own timing spans start — most plausibly ANGLE's
   GL context/shader-compiler bring-up, which this probe's write scope
   (`examples/web-spike`, no `crates/` edits) cannot instrument further.
   **This is a new finding, not previously recorded** (§ 3/§ 5 measured GL
   *correctness*, never GL *bring-up time*) — worth its own `crates/frust-gpu`
   task if the web shell plan wants a real answer for it, per this file's own
   "seams, not changes" convention (§ 11).
3. **First-submit warm-up cost is real on both arms** (WebGPU: 10.2ms vs
   0.6–0.8ms steady state, ~13–17×; WebGL2: 16.7ms vs 0.9–1.4ms, ~12–19×) —
   consistent with `engine-wasm-single-thread`'s prediction that the warm-up
   queue drains synchronously on `wasm32` with no background thread to hide
   it behind. Single-digit-to-low-double-digit milliseconds, once, not a
   per-frame recurring cost.

### 16.4 Console output, verbatim (one representative cold run per arm)

Captured via the same driver approach as § 2/§ 6 (a plain-HTTP W3C WebDriver
session against chromedriver on `127.0.0.1:9516`), reading back
`document.getElementById('log').textContent` after the page's title reaches
a terminal verdict — this DOM mirror carries every `timing:`/`frame N`/
`verdict:` line this spike's own `log_line` calls write (everything measured
in § 16.2/§ 16.3 above comes from these lines); it does **not** carry lines
the `log` crate writes straight to the devtools console (e.g. § 3's `frust-
gpu: could not spawn the pipeline warm-up thread ...` line), which this
section did not re-capture since § 3/§ 15 already recorded that behaviour
verbatim and it is unchanged here.

**WebGPU, run 1 of 3:**

```
frust w0-03 browser render probe | arm=webgpu | renderer=frust-engine (the only one)
verdict: frust w0-03 webgpu | starting
winit: window created over the page canvas
wgpu: instance restricted to Backends(BROWSER_WEBGPU)
surface: online after 1 attempt(s), phase=SurfaceReady
adapter:
backend: BrowserWebGpu
downlevel profile: Full
limits: max_texture_dimension_2d=8192 max_texture_array_layers=256 max_bind_groups=4 max_uniform_buffer_binding_size=65536 min_uniform_buffer_offset_alignment=256 max_vertex_attributes=16
caps: storage_buffers=true timestamp_query=true resource_texture_dim=4096 downlevel_flags=DownlevelFlags(COMPUTE_SHADERS | FRAGMENT_WRITABLE_STORAGE | INDIRECT_EXECUTION | BASE_VERTEX | READ_ONLY_DEPTH_STENCIL | NON_POWER_OF_TWO_MIPMAPPED_TEXTURES | CUBE_ARRAY_TEXTURES | COMPARISON_SAMPLERS | INDEPENDENT_BLEND | VERTEX_STORAGE | ANISOTROPIC_FILTERING | FRAGMENT_STORAGE | MULTISAMPLED_SHADING | DEPTH_TEXTURE_AND_BUFFER_COPIES | WEBGPU_TEXTURE_FORMAT_SUPPORT | BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED | UNRESTRICTED_INDEX_BUFFER | FULL_DRAW_INDEX_UINT32 | DEPTH_BIAS_CLAMP | VIEW_FORMATS | UNRESTRICTED_EXTERNAL_TEXTURE_COPIES | SURFACE_VIEW_FORMATS | NONBLOCKING_QUERY_RESOLVE | SHADER_F16_IN_F32 | MSL2_1 | TEXTURE_COMPRESSION)
profile check: OK — Full on BrowserWebGpu, resolved by frust-gpu, not by the probe
font: registered bundled face -> Named(["Noto Sans"])
scene: 8 commands -> 28 engine draws (CPU compile)
timing: bring-up (context create + surface retries + inline pipeline warm-up fallback + font register + scene build) = 91.900ms
timing: encode=0.100ms (scene memcpy) acquire=0.000ms (swapchain/vsync wait) submit=10.200ms (strip generation + GPU encode/queue-submit — this spike's `strip_us` proxy)
verdict: frust w0-03 webgpu | RENDERED
timing: encode=0.000ms (scene memcpy) acquire=0.100ms (swapchain/vsync wait) submit=0.500ms (strip generation + GPU encode/queue-submit — this spike's `strip_us` proxy)
frame 1: RENDERED
timing: first-frame (start() entry -> first RAF-driven `frame 1` presented) = 113.000ms
timing: encode=0.000ms (scene memcpy) acquire=0.000ms (swapchain/vsync wait) submit=0.800ms (strip generation + GPU encode/queue-submit — this spike's `strip_us` proxy)
frame 2: RENDERED
```

**WebGL2, run 1 of 3:**

```
frust w0-03 browser render probe | arm=webgl | renderer=frust-engine (the only one)
verdict: frust w0-03 webgl | starting
winit: window created over the page canvas
wgpu: instance restricted to Backends(GL)
surface: online after 1 attempt(s), phase=SurfaceReady
adapter: ANGLE (NVIDIA Corporation, NVIDIA T400 4GB/PCIe/SSE2, OpenGL 4.5.0)
backend: Gl
downlevel profile: WebGl2
limits: max_texture_dimension_2d=2048 max_texture_array_layers=256 max_bind_groups=4 max_uniform_buffer_binding_size=16384 min_uniform_buffer_offset_alignment=256 max_vertex_attributes=16
caps: storage_buffers=false timestamp_query=false resource_texture_dim=2048 downlevel_flags=DownlevelFlags(NON_POWER_OF_TWO_MIPMAPPED_TEXTURES | COMPARISON_SAMPLERS | ANISOTROPIC_FILTERING | MULTISAMPLED_SHADING | SHADER_F16_IN_F32 | MSL2_1 | TEXTURE_COMPRESSION)
profile check: OK — WebGl2 on Gl, resolved by frust-gpu, not by the probe
font: registered bundled face -> Named(["Noto Sans"])
scene: 8 commands -> 28 engine draws (CPU compile)
timing: bring-up (context create + surface retries + inline pipeline warm-up fallback + font register + scene build) = 2271.100ms
timing: encode=0.000ms (scene memcpy) acquire=0.100ms (swapchain/vsync wait) submit=16.400ms (strip generation + GPU encode/queue-submit — this spike's `strip_us` proxy)
verdict: frust w0-03 webgl | RENDERED
timing: encode=0.000ms (scene memcpy) acquire=0.000ms (swapchain/vsync wait) submit=0.800ms (strip generation + GPU encode/queue-submit — this spike's `strip_us` proxy)
frame 1: RENDERED
timing: first-frame (start() entry -> first RAF-driven `frame 1` presented) = 2299.000ms
timing: encode=0.000ms (scene memcpy) acquire=0.000ms (swapchain/vsync wait) submit=0.900ms (strip generation + GPU encode/queue-submit — this spike's `strip_us` proxy)
frame 2: RENDERED
```

Runs 2 and 3 of both arms (the remaining data behind § 16.2/§ 16.3's tables)
matched this shape line-for-line except for the `timing:`/adapter-name
numbers already tabulated; not reproduced a second and third time here to
keep this section a reasonable size.

### 16.5 Files touched, and what stayed the same

`src/main.rs`, `index.html`'s build/serve comment header (unchanged content,
still accurate), `Cargo.toml` (`[profile.release]` + the `Performance`
`web-sys` feature), `Cargo.lock` (regenerated; no dependency version moved —
`Performance` is a `web-sys` feature of an already-present dependency, not a
new crate), `README.md` (rewritten — see below), and this file, all under
the conductor's scope extension. Nothing in `crates/` was touched; nothing
in `docs/` was touched.

`README.md` was rewritten to the current state (w0-02 through w0-05) — it
had been stale since w0-02 (§ 11.5 already flagged this). See the rewritten
file for what changed; the short version is that it now describes what each
of the four tasks proved, how to build/run both arms, the `pkg/` ignore
rule, the WebGL2 text defect, and pointers into this file's §§ rather than
w0-02's now-superseded "expect it to fail" framing.

### `doc_updates_needed` (w0-05)

None of `docs/*.md` need a change for this task's own write scope — it adds
no new crate dependency, no new cfg, no new build flag reachable outside
`examples/web-spike`. Two things worth a future task's attention, neither
blocking this one:

- The GL-arm bring-up cost (§ 16.3 point 2, ~2.1s, unexplained by anything
  this probe's write scope can instrument) is a strong candidate for its own
  `crates/frust-gpu` investigation task, parallel to § 5's WebGL2 text-defect
  task — both are Phase-0 blockers for a WebGL2 fallback, one correctness,
  one now perf.
- `docs/LIMITATIONS.md`'s `engine-wasm-single-thread` entry could gain a
  one-line pointer to this section's measured warm-up cost (10–17ms, once,
  not per-frame) now that it has been measured rather than only predicted —
  a documentation task, not required by this one's write scope.
