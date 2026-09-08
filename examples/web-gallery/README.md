# web-gallery

The web-shell plan's **Phase-1 gate vehicle** (task w1-06). An ordinary Frust
app — authored through the `frust` facade's `web_app!` entry point (w1-05),
the same vocabulary a desktop/Android/iOS app author uses — that renders the
shared `frust-gallery` case registry (`examples/gallery`) in the browser and
exercises every Phase-1 host-signal milestone: pointer/touch/wheel/keyboard
input, dark/light theme follow, resize + DPR, and a signal-driven repaint
with zero input events.

Standalone workspace (own `[workspace]` root), excluded from the root
workspace graph the same way `examples/web-spike`/`examples/huddle` are (see
`Cargo.toml`'s header comment) — a `wasm32`-only browser app has no business
in the native workspace's lockfile or `target/` dir. `frust-gallery`
(`examples/gallery`) is a root-workspace member, reached here as an ordinary
relative path dependency.

## Pages

- `?case=<slug>` (e.g. `?case=button`, `?case=material/button`) renders
  exactly that case, full-screen, with a small "‹ Index" header.
- No `?case=` (or an unknown slug) falls back to the **index page**: a
  live-filtered, keyboard-searchable, scrollable list of every case in the
  registry, plus a ticking counter line that proves a `frust::RwSignal`
  write alone — no pointer or key event — drives a repaint.

Query parameters, all read in `src/main.rs` / `index.html`:

| Param | Values | Effect |
|---|---|---|
| `?case=<slug>` | any registry slug | render that one case |
| `?theme=<variant>` | `light`/`dark` | force that appearance end-to-end (`frust::set_app_theme`), overriding the host's own reported light/dark appearance; any other value (including absent) leaves the shell following the host, unchanged from before this parameter existed |
| `?log=<level>` | `off`/`error`/`warn`/`info`/`debug`/`trace` | raise the console log ceiling above the facade's `Warn` default |
| `?arm=webgl` | present/absent | force the WebGL2 backend arm (see "The WebGL2 arm" below) |

See "Embedding" below for the `?theme=` override's rationale and the iframe
height-report contract this task adds.

## Build

### Primary recipe: `frust build web --release`

From this directory, once `frust` itself is built from this checkout (an
installed `frust` predating the web-shell plan's CLI work has no `web`
subcommand at all — check `frust build web --help` before assuming an
already-installed binary is new enough):

```sh
frust build web --release
```

This is `crates/frust-drive::web_build::build` (`BuildTarget::Web` in
`crates/frust-cli/src/commands/build.rs`), and it always builds in release
mode regardless of the flag (every `frust build` target does — matching
`macos`/`windows`/`linux`/`apk`/`appbundle`/`ios`/`ipa`); `--release` is
written above because it is the invocation the CLI documents for every
target, not because it changes anything here. In order: `cargo build
--target wasm32-unknown-unknown --release`, `wasm-bindgen --target web`
straight into `<out-dir>/pkg/` (`build/web/pkg/` by default), then
`wasm-opt` — release always runs it, using the exact named-proposal list
`WASM_OPT_FEATURES` in `crates/frust-drive/src/web_build/mod.rs` carries
(see "wasm-opt feature flags" below; kept in lockstep with the manual
recipe's own flags), then stages a host page over the result.

**This crate has no `web/` host-page directory of its own** (`[web]
host-dir`'s default), only an `index.html` at its project root — the exact
shape `frust-drive::web_build::bundle::resolve_embedder`'s own test names
this crate by: *"No app host page at all (`examples/web-gallery`'s shape: an
`index.html` at the project root, not under `web/`) falls back to the
framework embedder."* So `frust build web` **always** stages
`platform/web/index.html` + `platform/web/frust_web.js` verbatim, named
`pkg/app.js`/`pkg/app_bg.wasm` (`BINDGEN_OUT_NAME`) — never this directory's
own `index.html`, whatever it says. That framework page already implements
the height-report contract (see "Embedding" below), so this is the recipe
the binary-size table below and the browser verification were measured
against — it needed no change from this task at all, which is this task's
own justification for leaving `Cargo.toml`/the CLI's embedder choice alone
and only touching this crate's own `index.html` for the *second*, independent
embedding route (below).

Output lands in `build/web/` (`pkg/app.js`, `pkg/app_bg.wasm`, the staged
`index.html`/`frust_web.js`) — see "Generated output" below for why it is
**not** committed.

### Manual recipe (no `frust` build required)

From this directory:

```sh
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir pkg --out-name web_gallery \
  target/wasm32-unknown-unknown/release/web-gallery.wasm
wasm-opt -O \
  --enable-bulk-memory --enable-bulk-memory-opt \
  --enable-nontrapping-float-to-int --enable-sign-ext \
  --enable-mutable-globals --enable-multivalue --enable-reference-types \
  -o pkg/web_gallery_bg.opt.wasm pkg/web_gallery_bg.wasm
```

This produces the exact `pkg/web_gallery.js`/`pkg/web_gallery_bg.wasm` this
directory's own `index.html` imports by that fixed `--out-name`. Unlike the
primary recipe, this one **does** serve this directory's own `index.html` —
see "Serve" below for the one requirement that recipe adds (serving from the
repository root, not this directory, so `index.html`'s
`../../platform/web/frust_web.js` import resolves).

Host tool versions the sizes below were measured with: `wasm-bindgen-cli`
0.2.128 (must equal the crate's own `=0.2.128` pin — see `Cargo.toml`),
`wasm-opt` 132, `cargo` 1.98.1 (the repo's pinned toolchain).

### `wasm-opt` feature flags

`wasm-opt` must be told which proposals to accept — bare `-O` refuses to
parse this `rustc`/`wasm-opt` combination's output, the finding
`examples/web-spike/RESULTS.md` § 16.1 records — and must run **after**
`wasm-bindgen`, never before. Name them, as above, rather than reaching for
`--all-features`: that flag means "everything this binaryen knows about", and
since binaryen 131 that includes the compact import section, which makes the
optimizer emit an import kind no browser will instantiate —

```
CompileError: WebAssembly.instantiateStreaming(): Invalid import kind 126,
enable with --experimental-wasm-compact-imports
```

— while still exiting 0, so the breakage shows up only when the page loads.
`frust build web` runs the same named list (`WASM_OPT_FEATURES` in
`crates/frust-drive/src/web_build/mod.rs`); keep the two together. To actually *ship* the optimized artifact
(rather than only measure it), copy it over the unoptimized one afterward —
`wasm-bindgen`'s generated `pkg/web_gallery.js` always imports
`./pkg/web_gallery_bg.wasm` by that fixed name:

```sh
cp pkg/web_gallery_bg.opt.wasm pkg/web_gallery_bg.wasm
```

### Binary size — the number that decides whether live previews ship
default-on or opt-in

Measured against the **primary recipe** (`frust build web --release`,
`build/web/pkg/app_bg.wasm`) — the artifact this task's browser verification
actually loaded (see "Embedding" below) — not the manual recipe, though the
two produce byte-identical `.wasm` (same crate, same `[profile.release]`,
same `wasm-bindgen`/`wasm-opt` invocation under the hood; only the file
name/host page differ). `[profile.release]` is this crate's own (`lto =
"fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "abort"`, copied
from the root profile since a standalone workspace never sees it):

| Stage | Size |
|---|---|
| `wasm-bindgen` output (unoptimized) | 11,523,550 B ≈ 10.99 MiB |
| after `wasm-opt -O` with the feature list above | 10,723,407 B ≈ 10.23 MiB |
| gzip `-9` of the `wasm-opt`'d file | 4,466,224 B ≈ 4.26 MiB |

Measured 2026-09-08 on this task's own branch (`feature/widget-previews-p3`),
toolchain `cargo`/`wasm-bindgen-cli`/`wasm-opt` versions as recorded above,
via `frust build web --release` itself (built from this checkout — see
"Primary recipe" above) followed by `gzip -9 -c build/web/pkg/app_bg.wasm |
wc -c`. This table's immediately-prior measurement (11,535,811 / 10,730,424
/ 4,465,581 B, from the base commit this task started from, before the
`?theme=` override landed) is close enough that the delta is entirely that
one small `resolve_theme_override_from_query` addition — not noise, but not
a meaningful shift either. The earlier rows this table carried before that
(9.34 / 8.72 / 3.69 MiB) predate the web shell's IME bridge and CLI web
build landing.

**Verdict for the live-preview default:** unchanged by this task — a ~10.2
MiB (~4.3 MiB gzipped) module is still too large to load eagerly per
preview instance; g3-02's lazy-iframe-with-PNG-poster design (load only on
demand, one shared module cached across every embedded case on a page) is
the right shape, not a default-on embed. This binary carries the *whole*
35-case registry (see "A discrepancy with the card's '107 cases'" below) —
per-design-system splitting, so a single Material-only preview page does not
pay for Cupertino/Glyph/Shadcn/beUI too, remains the lever actually worth
pulling before "opt-in" becomes "affordable by default"; that split is
future work, not something this task's `write_files` can do (it would mean
restructuring `frust-gallery`'s own case registry, `examples/gallery`).

One binary contains the whole 35-case registry plus the JS glue
(`app.js`/`web_gallery.js`, 162,358–162,382 B depending on `--out-name`,
uncompressed) — separate and tiny by comparison. For scale,
`examples/web-spike`'s own five-crate render-engine probe (no widgets, no
`frust-gallery`, no design-system plugins) measured 5.44 MiB / 1.93 MiB
gzipped at `wasm-opt`; the delta here is the widget set, the reactive
runtime's full surface, and the five design-system plugins' own font/token
data pulled in transitively through `frust-gallery`'s dependency on all five
(even though, at this base commit, `cases()` only actually walks the `Base`
module — see below).

## Serve

Any static server that sends `.wasm` as `application/wasm` works (verified
with `python3 -m http.server`, whose `mimetypes` module has mapped `.wasm`
to `application/wasm` since 3.9 — the same fact `examples/web-spike/serve.sh`
documents). A `serve.sh` matching that spike's own script was **not** added
— it is outside this task's `write_files` (`Cargo.toml`, `README.md`,
`index.html`, `src/main.rs`); the conductor can add one from this recipe if
wanted.

**Primary recipe's artifact** (`build/web/`, self-contained — `frust_web.js`
is staged *into* it, so nothing outside it is ever read):

```sh
python3 -m http.server 8931 --bind 127.0.0.1 --directory build/web
# open http://127.0.0.1:8931/index.html?case=button
```

**Manual recipe's `index.html`** — this directory's own page now imports
`../../platform/web/frust_web.js` directly rather than carrying a copy (see
"Embedding" below for why), so the server root must be the **repository
root**, not this directory, or that relative import 404s:

```sh
python3 -m http.server 8931 --bind 127.0.0.1 --directory ../..
# open http://127.0.0.1:8931/examples/web-gallery/index.html?case=button
```

`--directory` needs Python 3.7+; both invocations were verified against the
`python3` this task's own host carries.

`pkg/` and `target/` are generated and **must not be committed** — this
standalone workspace's own `.gitignore` already covers both (`efe979c5`).
`build/`, which the `frust build web` recipe writes, is covered the same way
(`066e9c30`) — the `.gitignore` line was added separately because
`.gitignore` was outside this task's own `write_files`.

## Embedding (this task, g3-01)

This task's objective: make the gallery embeddable — `?case=`/`?theme=`
routing (done; `?case=` already existed from w1-06, `?theme=` is new here)
plus reporting the embedded page's own rendered height to a parent frame, so
a preview host (g3-02's lazy iframe) can size itself without guessing.

**The height contract is the framework's, not this crate's own code.**
`platform/web/frust_web.js`'s `mount()` (read-only evidence for this task —
outside `write_files`) posts `{type: "frust:height", height}` to
`window.parent` via `postMessage`, on canvas-ready, after the wasm module's
own `init()` resolves, and on every `window resize` event — see that file's
`postHeightToParent`/`mount` doc comments for the exact mechanism (a plain
`document.documentElement.scrollHeight` read, `"*"` as the target origin
since a preview host is not assumed same-origin). Two independent routes
put this crate's own page on that contract, both inside this task's
`write_files`, and both are now live:

1. **`frust build web --release` stages the framework's own
   `platform/web/index.html` verbatim**, because this crate has no `web/`
   host-page directory of its own (see "Primary recipe" above) — no code
   change needed at all; the framework page already calls `mount()`.
2. **This directory's own `index.html` now imports `mount` from
   `../../platform/web/frust_web.js` directly**, replacing its former
   hand-rolled canvas-binding/resize script (the "Milestone 6" fix below) —
   the general version of the identical technique, plus the height-report
   calls that script never made. Chosen over copying `frust_web.js` into
   this directory (the shape `templates/app/web.tmpl/` uses for a *real*
   scaffolded app's own `<host-dir>`) because a copy is a second identity to
   keep in sync by hand the moment the framework's contract changes, and
   this task's `write_files` has no `web/` subdirectory to put one in
   anyway — a plain relative import is `mount()`'s own read-only evidence,
   never duplicated.

Both were verified end-to-end this task (see "Browser verification" below)
to actually post the message with the correct payload; the "Binary size"
table above records route 1's own measurement, since that is what `frust
build web --release`'s own recipe produces and what a real embedding is
expected to load.

**`?theme=light`/`?theme=dark`** (`src/main.rs`'s
`resolve_theme_override_from_query`) is the other half: an embedding host's
explicit request to pin one appearance end-to-end
(`frust::set_app_theme(frust_gallery::theme(design, variant))`), overriding
whatever the host/browser itself reports — orthogonal to and layered above
the existing per-design-system-case seeding (`set_default_theme`), which
still runs unchanged when `?theme=` is absent. Host-follow with no query
parameter needs no app code at all, exactly as before this task.

## Browser verification (this task, g3-01)

This Mac carries no automated WebDriver rig (the prior milestone evidence
below was gathered from a Linux Docker container this task's host does not
have) — but it does have `/Applications/Google Chrome.app` installed, and
Playwright's `chromium.launch({channel: "chrome"})` drives an **already
installed** Chrome over CDP directly, no `chromedriver`/Selenium download
needed. Verified this way, headed (not headless — this Mac's headless Chrome
GPU behaviour was not characterized, and headed matches the milestone
evidence below's own precedent), against `frust build web --release`'s own
`build/web/` output served locally and embedded in a plain cross-origin
`<iframe>` (a second local static server on a different port — the real
preview-embedding shape, not same-origin):

- **Chrome, WebGPU (`?case=button`):** loads; a canvas is created and
  correctly reparented under `#frust-host`
  (`canvasParentId === "frust-host"`); the parent frame receives **two**
  `{"type":"frust:height","height":700}` messages (canvas-ready, then
  post-`init()`) matching the iframe's own CSS box height exactly; the
  button case's shapes and (on this Chrome/macOS combination) its text both
  render.
- **Chrome, forced WebGL2 (`?case=button&arm=webgl`):** loads through the
  GL arm (`ANGLE (Apple, ANGLE Metal Renderer: Apple M4, ...)` in the
  adapter name, Naga-generated GLSL in the console under `?log=debug`,
  matching the WebGL2-arm mechanism described below); canvas adoption and
  the height-message pair both behave identically to the WebGPU arm.
- **`?theme=dark` / `?theme=light` (`?case=button`):** both load and post
  height correctly; screenshots confirm a genuinely different rendered
  background (near-black vs. near-white) for the two, with no OS/browser
  dark-mode emulation touched — the override is real, not host-follow
  coincidentally agreeing.
- **The manual recipe's own `index.html`** (served from the repository root
  per "Serve" above): posts the identical two height messages, confirming
  route 2 above works as well as route 1.

**One incidental observation, not chased further (out of this task's
scope):** on this Chrome/macOS/M4 combination, case-label text rendered
legibly on **both** the WebGPU and forced-WebGL2 arms — screenshots show
"Save"/"Cancel"/"Delete"/"Locked" clearly, which does not match the "A font
defect" section below's "every label renders as a blank/solid box"
finding from the prior task's Linux rig. This could be a platform/GPU
difference (a real system font resolving through some path this task did
not trace) rather than the defect being fixed; it is reported here as an
honest observation, not a claim that the documented `fontique`
`wasm32`/`SystemUi` limitation no longer exists — that would need the same
Linux rig re-run to confirm either way, and doing so is outside this task's
`write_files`.

**Not run, and not claimable from this Mac:** Safari 26. There is no Safari
automation available in this environment (Playwright's `webkit` channel is
a bundled WebKit build, not shipping Safari, and was not substituted for
it here since that would misrepresent the leg as covered). The conductor
should bill a real Safari 26 pass to Ed or a macOS device gate, the same way
the card's own acceptance names it.

## Rig used for the milestone evidence below (prior task, w1-06)

`docker exec frust-linux-native ...` — Chrome 151.0.7922.108 at
`/usr/bin/google-chrome`, `chromedriver` 151.0.7922.108 at
`/home/user/apps/chromedriver-linux64/chromedriver` (already running on port
9517 in the container), `DISPLAY=:20` (**headed** — headless canvas readback
is blank on this rig, the same finding `examples/web-spike/RESULTS.md`
records), host networking so the host's own `python3 -m http.server 8931`
is reachable at `http://localhost:8931/` from inside the container. Driven
over the raw W3C WebDriver HTTP protocol (no Selenium installed on this
host) — `goog:chromeOptions.args` per session as noted per milestone below.
Every session that touches WebGPU needs `--no-sandbox --disable-dev-shm-usage
--enable-unsafe-webgpu` (the same flags `examples/web-spike/RESULTS.md` § 2
records); a cold page's first `navigator.gpu.requestAdapter()` sometimes
resolves `null` once, which `frust_render`'s own `SurfaceRenderer` retry
already absorbs (`crates/frust-render`), so no app-level retry was needed
here the way `examples/web-spike`'s own probe hand-rolled one.

## Milestone evidence

All screenshots below were captured during this task and are **not**
committed (per the task's instruction — they live under this session's
scratchpad); this section describes what each one showed. Re-run the WebDriver
recipe above to reproduce.

### 1. Mouse click / hover (index navigation)

Clicking a row in the index list's `nav_row` (see "A font defect and how
this app works around it" below for why rows are hand-rolled instead of
`frust::button`) navigated to that case's `?case=` page; clicking "‹ Index"
on a case page navigated back. Verified round-trip on both the WebGPU and
the forced-WebGL2 arm.

### 2. Mouse wheel scroll

A combined WebDriver `pointerMove` + `wheel` action
(`{"type":"scroll","deltaY":1200}`) over the list scrolled it from the first
page of cases (`any-view`, `container`, `safe-area`, ...) to a later page
(`scroll-view`, `animated-opacity`, `animated-scale`, ...) — the
`list_view`'s own virtualized scroll/fling handling, unmodified by this app.
(A `wheel` action alone, with no preceding `pointerMove` in the *same*
actions call, did **not** register — worth recording as a WebDriver quirk on
this chromedriver, not an app or engine finding.)

### 3. Touch drag scroll

A multi-waypoint `pointerMove` sequence (five intermediate points over
~500ms, `pointerType: "touch"`) starting on a list row and ending higher up
scrolled the list, both in a mobile-emulation session
(`goog:chromeOptions.mobileEmulation`, 400×800 @2x, `touch: true`) and in a
plain desktop session with a touch-type pointer. A single-jump
`pointerDown`→`pointerMove`→`pointerUp` (one waypoint) did **not** register
as a scroll — winit's touch-phase state machine on this backend appears to
want more than one `Moved` phase before treating the gesture as a drag
rather than a tap; recorded as an evidence caveat for whoever next touches
`crates/frust-shell-web`'s touch mapping, not a defect this task's write
scope can investigate further.

### 4. Keyboard text entry

Typing `flex` into the index page's filter `text_input` (WebDriver key
actions, one `keyDown`/`keyUp` pair per character) live-filtered the
35-case list down to the single `flex — Row and Column` row, confirmed by
screenshot. The typed value round-tripped through the widget's controlled
`on_change`/`value` contract exactly as `TextInputView`'s own doc describes.

### 5. Dark/light theme follow — **live flip**

`chromedriver`'s legacy `POST /session/{id}/chromium/send_command` endpoint
with `Emulation.setEmulatedMedia({features: [{name: "prefers-color-scheme",
value: "dark"}]})` flipped the OS-reported appearance **while the app was
already running**, with no reload. The page's background and every text
colour inverted correctly (light surface/dark text → dark surface/light
text) on the very next frame — `crates/frust-shell-web`'s
`follow_platform_brightness`, entirely the shell's own job; this app's code
contains nothing theme-related beyond the optional per-case
`set_default_theme` override (see `src/main.rs`'s `AppState::new`).

### 6. Resize — **live, after a page-level fix**

**Finding, not just evidence:** out of the box, the shell's canvas does
**not** track a live browser-window resize on this app, even though
`crates/frust-shell-web/src/app_handler.rs` demonstrably has a working
`WindowEvent::Resized` handler. Root cause, verified empirically by reading
back `canvas.width`/`getBoundingClientRect()` after a WebDriver
`window/rect` resize: winit sets the canvas's `style.width`/`style.height`
**inline**, in pixels, from `DEFAULT_CANVAS_SIZE` (800×600) when it creates
the element, and:

- an *external stylesheet* rule targeting `canvas` — even with `!important`
  — cannot override that inline declaration (inline always outranks a
  stylesheet rule in the CSS cascade, `!important` or not; verified by
  trying exactly this and observing no effect);
- but a plain **JS property assignment** (`canvas.style.width = "100vw"`)
  *replaces* winit's own inline declaration outright, and once the inline
  value itself is a viewport-relative unit, the browser's own layout engine
  recomputes it on every later resize, and winit's already-attached
  `ResizeObserver` picks that up as an ordinary observed change — no engine
  code touched.

`index.html` therefore carries a small `MutationObserver` (see its own
inline `<script>`, ahead of the module script) that catches the canvas the
moment winit appends it and does exactly that JS-property replacement. With
it in place: a WebDriver `window/rect` resize from 1000×900 to 700×650 (DPR
2) grew the canvas's *backing store* from 1920×750 to 1384×1006, its CSS box
from 960×375 to 692×503, and the in-app `WindowMetrics` line updated to
match (`window: 692x503 @ 2.00x scale — Landscape`) on the very next frame,
with the whole layout (header, filter box, list) correctly reflowing to the
new width. This is genuinely useful, in-scope evidence for the milestone —
but the underlying gap (an *unstyled* page's canvas never resizes at all)
is a `frust-shell-web` fact this app's `write_files` cannot fix at the
source; see "Recommended follow-up" below.

**Update (g3-01):** this page's own `MutationObserver` script described
above has since been removed — `platform/web/frust_web.js`'s `mount()` now
implements the identical fix, generalized from a fixed `100vw`/`100vh`
target to an arbitrary host element's own box (see "Embedding" above). The
finding and its root-cause diagnosis above remain accurate history; only
where the fix lives has moved.

### 7. Device-pixel-ratio change

`--force-device-scale-factor=2` at launch (a new session, not a live
toggle — matches the task's own note that live zoom does not fire winit's
`ScaleFactorChanged` on this rig) produced a crisp, correctly-scaled frame:
`WindowMetrics` read `window: 400x300 @ 2.00x scale`, and the canvas's
physical backing store was exactly 2× its CSS box throughout. Combined with
Milestone 6's live-resize fix, a DPR-2 session was then *also* resized live
and tracked correctly (see the 700×650 numbers above, captured under
`--force-device-scale-factor=2`).

### 8. Signal-driven update, zero input events

The index page's `ticks = N` line is written by a background
`frust::spawn_local` loop (`start_ticker` in `src/main.rs`) that
`sleep_ms(1_000).await`s (a `setTimeout`→`Promise`→`JsFuture` wait, mirroring
`examples/web-spike`'s own `sleep_ms`) and calls `RwSignal::update` — no
pointer or keyboard event anywhere near it. `index_view` reads the signal
through `Get::get`, which tracks the read, so the write's own dirty edge is
what wakes the next repaint. Screenshots taken tens of seconds apart with no
interaction in between show the counter advancing (2 → 28 → 70 → 149 → 167
across the course of this session's manual testing), proving the repaint
loop is signal-driven rather than a busy poll.

## The WebGL2 arm

`RenderContext::new()` (`crates/frust-gpu/src/context.rs`) takes
`wgpu::Backends::from_env().unwrap_or_default()`; `from_env` reads the
`WGPU_BACKEND` *process* environment variable, which does not exist on
`wasm32-unknown-unknown` (no process, no env), so it always falls through to
wgpu's own compiled-in default mask, `BROWSER_WEBGPU | GL`.
`crates/frust-shell-web/src/render.rs` calls `RenderContext::new()`
unconditionally — there is no `ContextOptions`/backend-selecting seam there,
and none in this app's own `write_files` either, without editing
`crates/frust-gpu`/`crates/frust-shell-web` (out of scope per the card: "no
render-tier switch", and this is a wgpu *backend* choice, not a second
renderer).

Two methods were tried:

1. **Chrome flag**, per the card's own suggestion:
   `--disable-features=WebGPU`. **Did not work** on this rig/Chrome
   151.0.7922.108 — `navigator.gpu` remained defined afterward, and instead
   of falling back to GL, wgpu's adapter request failed outright
   (`frust-gpu: no compatible GPU adapter: No suitable graphics adapter
   found`) across all 8 retry attempts. Recorded here so the next person
   does not re-try the same flag expecting it to work on this Chrome/rig
   combination.
2. **A page-level `navigator.gpu` removal**, gated behind `?arm=webgl`
   (`index.html`'s first inline `<script>`, ahead of the module import):
   `Object.defineProperty(navigator, "gpu", { value: undefined, configurable:
   true })` before the wasm module loads. This **worked**: wgpu's own
   WebGPU probe checks `navigator.gpu` before ever requesting an adapter
   from it, finds nothing, and falls through to the `GL` bit already in the
   default mask — the identical fallback `RenderContext::new()` already had,
   just steered by what the page reports the browser having, with no engine
   code touched and no second render tier introduced.

With `?arm=webgl` in place, the app came up correctly through the GL path
and reproduced **exactly** the `w0-03`/`w0-07` finding already on record: shapes,
layout, backgrounds and the theme's colours render identically to the
WebGPU arm; **every glyph paints as a solid opaque box** instead of text.
The browser console (`?log=debug`) showed the matching signature repeatedly:

```
wgpu-hal heuristics assumed that the view dimension will be equal to `D2`
rather than `D2Array`. `D2` textures with `depth_or_array_layers == 1` are
assumed to have view dimension `D2` ...
```

— the exact `wgpu-hal` 30.0.1 GLES-backend defect `examples/web-spike/RESULTS.md`
§ 17 diagnosed and proposed a (not-yet-landed) fix for. This app does not
attempt to fix it (out of scope, and explicitly forbidden by the task: "run
the WebGL2 arm anyway, record what you see honestly, do not try to fix the
engine"). Screenshot evidence: the index page's rounded-rectangle rows and
the "container/colored_box" case's blue rounded rect render pixel-correct;
every label on both pages is a solid-white box the same shape and size the
text would have occupied.

## A font defect and how this app works around it

**Discovered during this task, not by it:** every text-bearing Frust widget
defaults to `frust_text::FontFamily::SystemUi` (`TextStyle::default()`)
unless a caller sets `.family(...)` explicitly, and — as of this base commit
— **no case in the whole `frust-gallery` registry does**
(`grep -rn '\.family(' examples/gallery/src` is empty). On desktop that
resolves through fontique's real system-font backend; on
`wasm32-unknown-unknown`, `fontique` 0.11.1 ships a backend its own source
literally comments as a "Dummy system font backend for targets like
wasm32-unknown-unknown" (`fontique-0.11.1/src/backend/mod.rs`), whose
generic-family map is empty. So `SystemUi` — and `Theme::neutral()`'s own
type scale, which carries a generic `NamedWithGeneric([], SansSerif)` stack
rather than a named face — resolves **zero glyphs** on this target, and nothing
an app registers through `frust::register_app_fonts` changes that: verified
empirically here (registering a bundled Inter Variable face and re-rendering
with no other change left every `SystemUi`-styled label exactly as blank —
see the two "index" screenshots taken before/after that one-line change).

This app works around it for its **own** authored chrome only:

- `src/main.rs`'s `label()` helper wraps `frust::text` with an explicit
  `.family(FontFamily::named("Inter Variable"))` (the bundled face's real
  name-table entry — not `"Inter"`; same fact `plugins/shadcn`'s
  `tokens::theme::INTER_FAMILY` documents, not depended on directly since it
  is not re-exported past that crate's `tokens` module).
- `frust_widgets::button`'s internal label has no family-override builder at
  all, so a `button()` call would still render an invisible (though still
  clickable) label under the same defect. This app therefore never calls
  `button()` — `nav_row()` in `src/main.rs` hand-rolls the same affordance
  out of `GestureDetector` + `container` + `label()`, entirely within this
  crate's own write scope.
- The bundled face itself
  (`plugins/shadcn/fonts/inter/InterVariable.ttf`, `include_bytes!`'d — a
  *read* of an existing repo asset, not a new write-scope file) is
  registered once via `frust::register_app_fonts` in `AppState::new`,
  before the shell constructs (matching the timing contract
  `frust::register_app_fonts`'s own doc and `frust_shadcn::install`'s
  identical pattern both spell out).

It **cannot** work around this for a hosted `Case`'s own internal text (the
overwhelming majority of the registry's actual content) — that is
`crates/frust-text`, `crates/frust-widgets`, and `examples/gallery`, all
outside this task's `write_files`. A case's shapes, images, layout and theme
colours all render correctly; its labels do not, on **either** backend arm
(this is a `frust-text`/`fontique` defect, orthogonal to the WebGL2-specific
glyph-atlas defect above — it reproduces identically on the WebGPU arm too).

**Recommended follow-up (for the conductor to file, mirroring the w0-07 →
its-own-landing-card precedent):** either give `frust-text`'s `TextContext`
a way to populate fontique's generic-family map from an app-registered face
on `wasm32` (so `SystemUi`/`Theme::neutral()`'s generic stack resolves
without every widget author having to name a family explicitly), or give
`frust-gallery`'s case-building helpers (`base::framed`/`framed_in`, or
`Case` itself) a way to carry/request a fallback family so a browser host can
thread one through without editing every one of the 35 (eventually 107)
case modules by hand.

## A discrepancy with the card's "107 cases"

This task's dispatch text states `frust_gallery::cases()` returns "107 cases
incl. five design systems". At this task's actual `base_sha`,
`frust_gallery::cases()` returns **35** — every one tagged
`Design::Base` — because `examples/gallery/src/lib.rs`'s own `cases()`
still only calls `base::cases()`; its doc comment says so explicitly
("Currently the whole `Base` page set... later phases add each design
system's own case modules and concatenate them here"). This app's own
`?case=`/index-page code reads the registry *dynamically* (`frust_gallery::cases().len()`
is what the index page's own "N cases in the registry" line reports, and it
correctly says 35, not a hard-coded 107), so no change is needed here when
the design-system case batches land — this app will pick them up
automatically, including the per-case `set_default_theme` override in
`AppState::new` for a `Design != Base` case, written and compiling today but
not yet exercised by any case in the registry.

## Files

| File | Purpose |
|---|---|
| `Cargo.toml` | Standalone workspace manifest: the `frust` facade + `frust-gallery` path deps, the `wasm32`-gated `wasm-bindgen`/`web-sys`/`js-sys`/`wasm-bindgen-futures`/`frust-shell-web` rows, and this crate's own `[profile.release]`. |
| `src/main.rs` | The app. `fn main() {}` (required for a `[[bin]]` target; the real entry is `wasm_bindgen(start)`) plus a `wasm32`-only `mod app`: `AppState`, the font-registration/case-and-theme-resolution/log-level startup sequence (`resolve_case_from_query`, `resolve_theme_override_from_query`), `label`/`nav_row` (the font-defect workaround), `CaseHost` (the `Component` state-boundary bridge into a `()`-state `Case`), `case_view`/`index_view`, and the `frust::web_app!` invocation. |
| `index.html` | Host page for the manual recipe. No `<canvas>` of its own (the shell creates one) — carries the `?arm=webgl` WebGPU-removal script and the load-failure `<pre id="log">` mirror; canvas-host binding, resize/DPR sync, and the iframe height-report contract are now `platform/web/frust_web.js`'s `mount()` (imported directly, see "Embedding" above), not a page-local script. Not read at all by `frust build web --release`, which stages the framework's own page instead (see "Primary recipe" above). |
| `README.md` | This file. |
| `pkg/` | `wasm-bindgen`/`wasm-opt` output (manual recipe). Generated; **not committed** (`.gitignore`-covered — see "Serve" above). |
| `target/` | Cargo build output. Generated; **not committed** (`.gitignore`-covered). |
| `build/` | `frust build web --release`'s own output directory (primary recipe). Generated; **not committed**, and `.gitignore`-covered alongside `pkg/` and `target/`. |
