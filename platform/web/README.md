# platform/web

The browser embedder — the third host tier consumed by path, alongside
`platform/android`'s `frust-embedding` and `platform/ios`'s `FrustEmbedding`.
Unlike those two (native platform-project libraries), this one is a static
host-page pair: `index.html` (a template an app author drops their own
`wasm-bindgen`-built app behind) and `frust_web.js` (the framework-free
vanilla-JS glue that page loads). It carries no app logic, no build step, and
no dependency of its own — no npm, no bundler.

This document is the host-page contract an app embedder must honour: what
`frust_web.js` does automatically, what it forwards as console evidence only
(because `crates/frust-shell-web` has no Rust-side hook for it yet), and the
CSS constraints the canvas needs to keep working.

## Files

| File | Purpose |
|---|---|
| `index.html` | Host page template. Defines the `#frust-host` canvas-host element and the `#frust-log` failure mirror, resolves `?module=` (default `./pkg/app.js`), and hands the resulting `init` function to `frust_web.js`'s `mount()`. |
| `frust_web.js` | The glue module: canvas-host binding, resize/DPR sync, visibility/history/iframe-height forwarding, and wasm-init failure reporting. Exports `mount(init, options)`, `reportFailure(err, options)`, `DEFAULT_HOST_SELECTOR`, `DEFAULT_LOG_SELECTOR`. |

## Consuming this embedder

Build your app to `wasm32-unknown-unknown`, run `wasm-bindgen --target web`
over it, and place `index.html`/`frust_web.js` next to the resulting `pkg/`
directory (or serve them from a fixed location and pass `?module=` to point
at wherever `pkg/` lives) — see "Build" below for the exact recipe, taken
from `examples/web-gallery/README.md`. No edit to either file is required for
an ordinary app: point `?module=` at your own build's `--out-name`.

```
index.html?module=./pkg/my_app.js
```

## `mount(init, options)`

`frust_web.js`'s one public entry point. `init` is the `default` export of a
`wasm-bindgen --target web` build (`import init from "./pkg/app.js"`).
`options.host` (a `#selector` string or an `Element`, default
`#frust-host`) is the element the canvas is bound to; `options.log` (default
`#frust-log`) is where a load failure is mirrored. Returns a promise that
settles once `init()` has resolved or its failure has been reported — never
an unhandled rejection.

`init()`'s own generated glue already implements
`WebAssembly.instantiateStreaming` with an `arrayBuffer` fallback for a
server that answers a `.wasm` request with the wrong `Content-Type`
(`wasm-bindgen`'s own `__wbg_load`); `mount()` awaits it rather than
re-implementing that fallback a second time, matching
`examples/web-gallery/index.html`'s identical `init().catch(...)` precedent
for reporting a failure that happens before Rust's own panic hook is
installed.

## Canvas host / resize / DPR contract

**winit pins the canvas's inline `style.width`/`style.height`, in pixels, at
creation** (`DEFAULT_CANVAS_SIZE`, 800x600 — see
`crates/frust-shell-web/src/app_handler.rs`'s `resumed`), and an inline
declaration always outranks an ordinary stylesheet rule in the CSS cascade,
`!important` or not, regardless of selector. A page that only adds a
`canvas { width: ...; }` rule therefore never sees a live resize reach the
shell — `examples/web-gallery/index.html`'s own "Milestone 6" section records
this exact finding, empirically, against a fixed `100vw`/`100vh` target; this
embedder generalizes the fix to an arbitrary host element:

1. winit appends its own `<canvas>` straight to `<body>` (it has no
   canvas-adoption seam yet); a `MutationObserver` on `<body>` catches it the
   moment it appears (bring-up is asynchronous, so this can be well after
   `init()` itself has returned).
2. `frust_web.js` reparents that canvas into `#frust-host`. Moving a live
   `<canvas>` in the DOM (a plain node move, not a `width`/`height`
   IDL-attribute write) preserves its bitmap and its rendering context, so
   this is safe whether or not wgpu has already bound a surface to it.
3. It then **overwrites** the canvas's inline `style.width`/`style.height`
   with the host element's own `getBoundingClientRect()`, in CSS pixels — a
   plain JS property assignment, which (unlike a stylesheet rule) actually
   replaces winit's inline declaration rather than trying to outrank it.
4. A `ResizeObserver` on `#frust-host` repeats step 3 on every host-box
   change, for the whole life of the page — not only once at startup.
   Winit's own `ResizeObserver` (already attached to the canvas) then picks
   the new box up as an ordinary observed resize and drives
   `WindowEvent::Resized` exactly as it would for any other box change;
   nothing in `frust_web.js` talks to wgpu, winit, or the shell directly.

Sizing `#frust-host` is entirely the host page's own job — `100vw`/`100vh` by
default in `index.html`'s stylesheet (an ordinary full-viewport app), a fixed
box, a flex child, whatever the embedding page wants. Whatever it resolves
to, the `ResizeObserver` above keeps the canvas in sync with it live.

DevicePixelRatio: a browser fires no `devicepixelratiochange` event;
`frust_web.js` uses the standard `matchMedia`-against-the-current-ratio,
re-armed after every fire, idiom to detect one. `crates/frust-shell-web`'s
own `WindowEvent::ScaleFactorChanged` handler documents that winit
"guarantees a following `Resized`" once a real DPR change is detected, so
the shell needs no help acting on it — the watcher's job is console evidence
that the browser reported a change at all, plus a defensive re-sync of the
canvas's pixel geometry for the case where a DPR-only change left no CSS-box
change for the `ResizeObserver` above to catch on its own.

## CSS constraints on the canvas

No `border`, `padding`, or `transform` on the canvas element (`frust_web.js`
clears `border`/`padding`/`margin` itself on adoption; `transform` is the
embedding page's own responsibility to avoid). `frust-shell-web`'s pointer
mapping (`physical_to_logical` in `crates/frust-shell-web/src/
app_handler.rs`) assumes the canvas's CSS box, divided by
`window.devicePixelRatio`, *is* the logical coordinate space the widget tree
lays out and hit-tests in. A border or padding shifts the content box away
from what a click event reports its position relative to; a CSS `transform`
scales or skews the rendered pixels without moving where the browser reports
pointer events — either one desyncs where the user clicked from where the
tree thinks they clicked.

## Lifecycle, history, and iframe height — what is real vs. evidence-only

Two of this milestone's forwarding paths have **no Rust-side hook to forward
into today** — `frust-shell-web` reads nothing from `document.visibilityState`
or from `location`/`history`. Rather than invent one on this task's own
authority, `frust_web.js` documents the gap and forwards what it can as
console evidence, satisfying the acceptance criterion's own "console-log
evidence is fine" allowance:

- **Visibility (pause/resume).** `frust-shell-web`'s frame loop is entirely
  `requestAnimationFrame`-driven under `winit::event_loop::ControlFlow::Wait`
  — a browser already stops delivering rAF callbacks to a hidden tab
  (throttling it to near-zero) with no page script asking it to, which is the
  "already degrades gracefully" behaviour the task names. `frust_web.js`'s
  `visibilitychange` listener makes that implicit degrade explicit as a
  console line; it calls no Rust export, because none exists. **Missing
  hook, for a follow-up card:** an explicit `pause()`/`resume()` the shell
  exported would let a hidden page actively release GPU resources (today it
  merely stops drawing) — a real optimization a JS-only glue cannot add on
  its own.
- **`popstate` / URL / back.** Same story: no router hook exists to forward
  into. An app that wants real deep-linking can already read
  `location.pathname`/`.search` and listen for `popstate` directly through
  `web-sys` from its own Rust code — exactly how `examples/web-gallery`'s own
  `?case=`/`?log=` query parameters are read today (via a plain `&str`
  parser, not through this module). `frust_web.js`'s listener logs the
  initial URL and every `popstate` for evidence only.

**iframe height postMessage is a real, complete implementation** (no missing
hook): `document.documentElement.scrollHeight` posted as
`{ type: "frust:height", height }` to `window.parent` with `"*"` as the
target origin. The wildcard origin is used because the payload contains only
a non-sensitive height value and the parent page is not assumed to be
same-origin (a preview host may be served from a different origin). This is
fired once `init()` settles and again on every `window` `resize` — which
Chrome fires inside an iframe's own window when the embedding `<iframe>`
element's box size changes from the parent page, confirmed during verification
below.

## Build

Same recipe `examples/web-gallery/README.md` documents, run from your own
app's crate directory:

```sh
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir pkg --out-name app \
  target/wasm32-unknown-unknown/release/<your-crate-name>.wasm
# wasm-opt is optional (needs --all-features on this toolchain combination):
wasm-opt -O --all-features -o pkg/app_bg.opt.wasm pkg/app_bg.wasm
cp pkg/app_bg.opt.wasm pkg/app_bg.wasm   # only if you ran wasm-opt
```

`--out-name app` matches `index.html`'s own `?module=` default
(`./pkg/app.js`); pick any name and pass the matching `?module=` instead.
Serve with anything that answers a `.wasm` request `application/wasm`
(`python3 -m http.server`'s `mimetypes` module has mapped it since 3.9 — see
`examples/web-spike/serve.sh`). `pkg/` and `target/` are build output; do not
commit them.

## Module parameter validation

The `?module=` query parameter must point to a same-origin, relative module
under the `./pkg/` directory. `index.html` validates the parameter before
importing to prevent cross-origin script injection:

- **Same-origin only:** The parameter's origin must match `location.origin`.
  Cross-origin URLs (e.g., `https://example.com/x.js`) and data URLs are
  rejected.
- **No schemes:** The raw parameter must not contain a URL scheme
  (e.g., `https://`, `file://`, `data:`).
- **No network paths:** The parameter must not start with `//`.
- **No directory traversal:** The parameter must not contain `..` segments.
- **Under `./pkg/`:** The resolved pathname must be within the `pkg/`
  subdirectory of the page's directory.

If validation fails, `index.html` displays a failure message in the app's
load-failure area (`#frust-log`) and does not attempt to import an invalid
module URL. This applies whether a default module is configured or not — a bad
override is always an error, never silently ignored.

## Verification performed (this task)

Rig: headed Chrome 151.0.7922.108 inside the `frust-linux-native` container
(`DISPLAY=:20`, host networking), `chromedriver` at
`/home/user/apps/chromedriver-linux64/chromedriver` on port 9517, driven over
the raw W3C WebDriver HTTP protocol. Served from a staging directory outside
this repository (`platform/web`'s three files plus `examples/web-gallery`'s
own `pkg/` output, built unmodified from this task's base commit) via
`python3 -m http.server 8932 --bind 127.0.0.1`, reachable from the container
at `http://localhost:8932/`.

- **The examples/web-gallery app runs unchanged through this embedder.**
  `examples/web-gallery`'s own `Cargo.toml`/`src/main.rs`/`index.html` were not
  touched; only its `wasm-bindgen` output was staged behind this embedder's
  `index.html?module=./pkg/web_gallery.js`. The index page (case list,
  live-filtered search, ticking signal-driven counter) rendered and
  functioned identically to `examples/web-gallery/README.md`'s own recorded
  behaviour.
- **Canvas host binding.** On load, winit's canvas was reparented into
  `#frust-host` and its inline style rewritten from the 800x600 default to
  the host's own box (`992px`/`753px` on a 1000x900 window).
- **Live resize — the review-finding fix.** A WebDriver `window/rect` resize
  from 1000x900 to 700x650 moved the canvas's CSS box to `692px`/`503px` and
  its backing store to 692x503 (DPR 1), with the in-app `WindowMetrics` line
  updating to `window: 692x503 @ 1.00x scale — Landscape` and the whole
  layout reflowing on the very next frame — console evidence:
  `[frust-web] host resized to 692x503 - canvas inline style re-synced`.
- **DPR.** A fresh session launched with `--force-device-scale-factor=2`
  produced a canvas CSS box of 960x375 against a backing store of exactly
  1920x750 (2x); resizing that same session to 700x650 produced a CSS box of
  692x503 against a 1384x1006 backing store (2x) — DPR and live resize
  compose correctly. (Matching `examples/web-gallery/README.md`'s own
  finding, a live in-session zoom does not fire `ScaleFactorChanged` on this
  rig — DPR is exercised via a fresh session's launch flag instead, the same
  workaround that app's own verification used.)
- **Visibility (pause/resume evidence).** A WebDriver `window/minimize`
  flipped `document.visibilityState` to `hidden` and logged
  `[frust-web] visibility: hidden`; a following `window/maximize` flipped it
  back to `visible`, logged `[frust-web] visibility: visible`, and the app
  kept tracking further resizes correctly afterward — proving it resumed,
  not merely that the event fired.
- **`popstate` / back.** `history.pushState` followed by a WebDriver `back`
  command restored the previous URL and logged
  `[frust-web] popstate -> <restored URL>`.
- **iframe height postMessage.** A separate parent page embedded this
  page's `index.html` in a 500x300 `<iframe>` and received
  `{ type: "frust:height", height: 300 }` twice on load (once from the
  canvas-ready callback, once from `init()` settling); resizing the
  `<iframe>` element itself to 500px tall from the parent page produced a
  third message with the updated height (500) — confirming both the "on
  load" and "on resize" halves of the acceptance criterion.
- **Load-failure reporting.** Pointing `?module=` at a nonexistent path
  produced `FAIL: TypeError: Failed to fetch dynamically imported module:
  ...` in both `document.title` and `#frust-log`, via `reportFailure`.

Screenshots and the full WebDriver session transcript were captured under
this task's scratch directory (not committed — see the task's own
instruction); this section describes what each one showed, mirroring
`examples/web-gallery/README.md`'s own evidence-section shape.

## Known gaps / follow-ups

- **No Rust-side pause/resume hook.** See "Lifecycle, history, and iframe
  height" above — worth its own follow-up card against `crates/
  frust-shell-web` if a browser deployment ever needs to actively release
  GPU resources on a hidden tab rather than merely stop drawing to it.
- **No Rust-side router/history hook.** Same section — an app wanting real
  deep-linking reads `location`/`popstate` itself via `web-sys`; nothing in
  this embedder or the shell reads the URL today.
- **No canvas-adoption seam in `frust-shell-web` itself.** This embedder
  works around winit always appending its own canvas to `<body>` by
  reparenting it after the fact; a Rust-side seam that let the shell create
  its canvas inside a caller-supplied host element directly would make the
  `MutationObserver`/reparent step in `frust_web.js` unnecessary. Not
  attempted here — out of this task's `write_files` (`crates/` is read-only
  for this task).
- **Documentation rows.** `docs/SHELLS_ARCHITECTURE.md` and
  `docs/SHELLS_DEVELOPMENT.md` (doc-maintainer-owned, not edited by this
  task) should gain a row for `platform/web` alongside the existing
  `platform/android`/`platform/ios` entries: this embedder's canvas
  host/resize/DPR contract, and the two documented no-hook gaps above, are
  facts a future web-shell task should not have to rediscover from this
  README alone.
