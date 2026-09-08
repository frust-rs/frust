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
| `?log=<level>` | `off`/`error`/`warn`/`info`/`debug`/`trace` | raise the console log ceiling above the facade's `Warn` default |
| `?arm=webgl` | present/absent | force the WebGL2 backend arm (see "The WebGL2 arm" below) |

## Build

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

Host tool versions the sizes below were measured with: `wasm-bindgen-cli`
0.2.128 (must equal the crate's own `=0.2.128` pin — see `Cargo.toml`),
`wasm-opt` 132, `cargo` 1.98.1 (the repo's pinned toolchain).

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

### Binary size (this build, `[profile.release]` from this crate's own
`Cargo.toml` — `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`,
`panic = "abort"`, copied from the root profile since a standalone workspace
never sees it):

| Stage | Size |
|---|---|
| `wasm-bindgen` output (`web_gallery_bg.wasm`, unoptimized) | 11,535,811 B ≈ 11.00 MiB |
| after `wasm-opt -O` with the feature list above | 10,730,424 B ≈ 10.23 MiB |
| gzip `-9` of the `wasm-opt`'d file | 4,465,581 B ≈ 4.26 MiB |

Measured 2026-09-08 on `dev` after the web shell's IME bridge and CLI web
build landed; the earlier rows this table carried (9.34 / 8.72 / 3.69 MiB)
predate both.

One binary contains the whole 35-case registry (see "A discrepancy with the
card's '107 cases'" below) plus the JS glue (`web_gallery.js`, 162,382 B,
uncompressed) is separate and tiny by comparison. For scale,
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
wanted:

```sh
python3 -m http.server 8931 --bind 127.0.0.1
# open http://127.0.0.1:8931/index.html
```

`pkg/` and `target/` are generated and **must not be committed** — this
standalone workspace needs its own `.gitignore` for that, which is also
outside `write_files`; the conductor adds it (see the task card's own "if the
standalone workspace needs its own .gitignore ... report it" instruction).

## Rig used for the milestone evidence below

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
| `src/main.rs` | The app. `fn main() {}` (required for a `[[bin]]` target; the real entry is `wasm_bindgen(start)`) plus a `wasm32`-only `mod app`: `AppState`, the font-registration/case-resolution/log-level startup sequence, `label`/`nav_row` (the font-defect workaround), `CaseHost` (the `Component` state-boundary bridge into a `()`-state `Case`), `case_view`/`index_view`, and the `frust::web_app!` invocation. |
| `index.html` | Host page. No `<canvas>` of its own (the shell creates one) — carries the `?arm=webgl` WebGPU-removal script, the canvas-resize `MutationObserver`, and the load-failure `<pre id="log">` mirror. |
| `README.md` | This file. |
| `pkg/` | `wasm-bindgen`/`wasm-opt` output. Generated; **not committed** (see "Serve" above). |
| `target/` | Cargo build output. Generated; **not committed**. |
