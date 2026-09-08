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
- `?case=<slug>&embed=1` renders the same case with that header (and all
  other chrome) stripped away — see "Embedding" below.
- No `?case=` (or an unknown slug) falls back to the **index page**: a
  live-filtered, keyboard-searchable, scrollable list of every case in the
  registry, plus a ticking counter line that proves a `frust::RwSignal`
  write alone — no pointer or key event — drives a repaint.

Query parameters, all read in `src/main.rs` / `index.html`:

| Param | Values | Effect |
|---|---|---|
| `?case=<slug>` | any registry slug | render that one case |
| `?theme=<variant>` | `light`/`dark` | force that appearance end-to-end (`frust::set_app_theme`), overriding the host's own reported light/dark appearance; any other value (including absent) leaves the shell following the host, unchanged from before this parameter existed |
| `?embed=1` | exact string `1` | strip this app's own chrome down to exactly the hosted case — no "‹ Index" header, no debug title, no outer `scroll_view`/padding — and make it inescapable from inside the frame (for a slug the registry carries; an unknown slug still falls back to the chromed index, see "Embedding"); any other value (including absent) leaves the existing chromed `?case=` page unchanged. **This is the exact spelling and semantics the website's `WidgetPreview` component's iframe `src` must use** (`?case=<slug>&embed=1`) — see "Embedding" below |
| `?log=<level>` | `off`/`error`/`warn`/`info`/`debug`/`trace` | raise the console log ceiling above the facade's `Warn` default |
| `?arm=webgl` | present/absent | force the WebGL2 backend arm (see "The WebGL2 arm" below) |

See "Embedding" below for the `?theme=`/`?embed=1` overrides' rationale and
the iframe height-report contract this task adds.

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
`index.html`/`frust_web.js`) — see "Serve" below for why it is
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
two drive the identical underlying pipeline: same crate, same
`[profile.release]`, the identical `cargo build`/`wasm-bindgen`/`wasm-opt`
invocation (compare "Primary recipe" above against "Manual recipe" below —
`crates/frust-drive/src/web_build/mod.rs`'s `WASM_OPT_FEATURES` list is the
same seven flags the manual recipe passes by hand); only the file
name/host page differ. That does not make their *output* byte-identical,
and it should not be read as such — see the "snapshot, not a constant" note
below the table, which records two builds of this identical pipeline, from
unchanged source, producing different byte counts at every stage.
`[profile.release]` is this crate's own (`lto = "fat"`, `codegen-units = 1`,
`strip = "symbols"`, `panic = "abort"`, copied from the root profile since a
standalone workspace never sees it):

| Stage | Size |
|---|---|
| `wasm-bindgen` output (unoptimized) | 11,524,399 B ≈ 10.99 MiB |
| after `wasm-opt -O` with the feature list above | 10,724,099 B ≈ 10.23 MiB |
| gzip `-9` of the `wasm-opt`'d file | 4,466,947 B ≈ 4.26 MiB |

**These three numbers are a snapshot, not a constant — re-measure before
citing one anywhere else.** All three rows above are from one build's own
log — the deployed artifact, measured 2026-09-09 against
`feature/widget-previews-p3` — never mixed across builds; `wasm-opt` shrank
that build's own unoptimized module from 11,524,399 B to the 10,724,099 B
row above it. Same recipe as always: `frust build web --release` (see
"Primary recipe" above) followed by `gzip -9 -c build/web/pkg/app_bg.wasm |
wc -c`. This table's immediately-prior row (11,523,553 / 10,723,170 /
4,466,842 B, measured 2026-09-08 on `task/p3-r1-embed-mode`, this same base,
no source change since) differs from today's row at every stage — 846 B
larger unoptimized, 929 B larger optimized, 105 B larger gzipped. That is
the observation: two builds of unchanged source through the identical
pipeline did not produce byte-identical output. This document does not know
why — nothing here diagnoses a cause, and none should be assumed — only
that it happened, which is reason enough that a second document
(`docs/TESTING.md`) copying one of these numbers goes stale the moment
either file rebuilds, independent of any app-code change; that is
why `docs/TESTING.md` no longer quotes a byte count at all (see its own
`examples/web-gallery` entry). The prior-prior measurement (11,523,550 /
10,723,407 / 4,466,224 B, the base commit review-round-1 started from — the
exact pair a previous draft of this document, and `docs/TESTING.md`,
both went on quoting as if still current) and the rows before that
(11,535,811 / 10,730,424 / 4,465,581 B before `?theme=`; 9.34 / 8.72 / 3.69
MiB predating the web shell's IME bridge and CLI web build landing) remain
useful history for the same reason — read as history, never as today's
number.

**Every row above predates the design-system font registration** that
`src/main.rs` now performs, and that registration is not free: it grows the
module by ~1.22 MB uncompressed and ~0.53 MiB gzipped, measured before and
after on the manual recipe in one sitting. The "Text on `wasm32`" section
below carries that before/after pair and explains why the bytes were not
already linked in; read it together with this table rather than assuming
either number still describes the other's build.

**Verdict for the live-preview default:** unchanged by this task — a ~10.2
MiB (~4.3 MiB gzipped) module is still too large to load eagerly per
preview instance; g3-02's lazy-iframe-with-PNG-poster design (load only on
demand, one shared module cached across every embedded case on a page) is
the right shape, not a default-on embed. This binary carries the *whole*
107-case registry — Base plus all five design systems (see "Registry size"
below) — per-design-system splitting, so a single Material-only preview page
does not pay for Cupertino/Glyph/Shadcn/beUI too, remains the lever actually
worth pulling before "opt-in" becomes "affordable by default"; that split is
future work, not something this task's `write_files` can do (it would mean
restructuring `frust-gallery`'s own case registry, `examples/gallery`).

One binary contains the whole 107-case registry plus the JS glue
(`app.js`/`web_gallery.js`, 162,358–162,382 B depending on `--out-name`,
uncompressed) — separate and tiny by comparison. For scale,
`examples/web-spike`'s own five-crate render-engine probe (no widgets, no
`frust-gallery`, no design-system plugins) measured 5.44 MiB / 1.93 MiB
gzipped at `wasm-opt`; the delta here is the widget set, the reactive
runtime's full surface, and the five design-system plugins' own font/token
data, which `cases()` now pulls in directly, not merely transitively — see
"Registry size" below.

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

**`?embed=1`** (`src/main.rs`'s `resolve_embed_from_query`, task p3-r1-01,
review round 1) is a third piece, added after this section's other two: the
website's `WidgetPreview` component builds its iframe `src` as
`?case=<slug>&embed=1`, not bare `?case=<slug>`, so a reader inside the
preview iframe sees exactly `component(CaseHost(case.build))` — no "‹ Index"
header, no debug title, no outer `scroll_view`/padding
(`src/main.rs`'s `embedded_case_view`, the chrome-free sibling of
`case_view`). Without it, the chromed `?case=` page's own header
(~55–60px) plus its 12px/16px padding squeezed the widget into whatever
remained inside a box the website sizes from the poster PNG
`frust-testing`'s CPU oracle renders **bare**, at `Case::DEFAULT_SIZE`
(`examples/gallery/src/case.rs`, 360×240 for five of the six live slugs,
400×180 for `material/tabs`) — a composition mismatch between the poster
and the live frame it cross-fades into. Worse, the chromed page's "‹ Index"
tap set `AppState::case` back to `None`, replacing the documented widget
with the full gallery index (recoverable only by tapping the slug's row
again — `src/main.rs`'s `index_view` sets `case` back to `Some`). `?embed=1`
closes both: `embedded_case_view` never renders a `nav_row`/`GestureDetector`
at all, so no tap inside an embedded frame can reach `index_view`. One
caveat the parameter cannot cover: `?embed=1` with a slug the registry does
not carry resolves to `None`, and `src/main.rs`'s view match then renders
the chromed `index_view` — so a stale or renamed slug shows the full index
inside the frame. The tap-out is closed; a bad slug is not. Any value other than the literal string `"1"`
(including the parameter's absence) leaves the existing chromed `?case=`
page unchanged — additive, not a replacement, the identical discipline
`?theme=` already established. **This exact spelling (`embed`, exact value
`"1"`) is the contract the website's `WidgetPreview` component (a separate
repository, a parallel card) must match** — see the query-parameter table
above.

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

**One observation from this task, now explained (not a Mac/Linux platform
quirk):** on this Chrome/macOS/M4 combination, case-label text rendered
legibly on **both** the WebGPU and forced-WebGL2 arms — screenshots show
"Save"/"Cancel"/"Delete"/"Locked" clearly, which did not match the "Text on
`wasm32`" section below's original "every label renders as a blank/solid box"
finding from the prior task's Linux rig. A review-round-1 fix (this pass)
traced this to `docs/LIMITATIONS.md`'s `web-generic-family-partial-fallback`
entry: `crates/frust-shell-web`'s `install_default_fonts` registers a
bundled Inter Variable face as the `SystemUi`/`SansSerif` generic-family
fallback unconditionally, before the shell builds its own `TextContext` —
in the codebase this whole crate builds against today, that mapping is
already live for every case's default-styled text. So the legible text
above is that framework-level fix actually working, not an unexplained
platform/GPU difference; this passage no longer needs the Linux rig re-run
it originally called for. See the "Text on `wasm32`" section below for the
corrected, current state (the `Monospace`/`Serif`/`Emoji` *generics* remain
unmapped; the named families in front of the `Monospace` tail — Glyph's two
faces and the shadcn/beUI mono slots — are registered by this binary itself
now, and their cases do render text, measured there case by case).
Note this rig exercised only `Base` cases, which is exactly why the
14 blank `glyph/*` cases went unobserved at the time — see that section's
own browser evidence, which reproduces the defect first and then measures
the fix.

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

Clicking a row in the index list's `nav_row` (see "Text on `wasm32`" below
for why rows are hand-rolled instead of `frust::button`) navigated to that case's `?case=` page; clicking "‹ Index"
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
actions, one `keyDown`/`keyUp` pair per character) live-filtered the case
list — 35 entries at the time of this evidence (before the design-system
batches landed; 107 today, see "Registry size" below) — down to the single
`flex — Row and Column` row, confirmed by screenshot. The typed value
round-tripped through the widget's controlled `on_change`/`value` contract
exactly as `TextInputView`'s own doc describes.

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

## Text on `wasm32`: `SystemUi`/`SansSerif` and the bundled design-system faces render; the `Monospace`/`Serif`/`Emoji` generics still do not

**The generic-slot half was discovered by a prior task and fixed at the
framework level:** every text-bearing Frust widget defaults to
`frust_text::FontFamily::SystemUi` (`TextStyle::default()`) unless a caller
sets `.family(...)` explicitly, and no case in the whole `frust-gallery`
registry does (`grep -rn '\.family(' examples/gallery/src` is empty). On
desktop that resolves through fontique's real system-font backend; on
`wasm32-unknown-unknown`, `fontique` 0.11.1 ships a backend its own source
literally comments as a "Dummy system font backend for targets like
wasm32-unknown-unknown" (`fontique-0.11.1/src/backend/mod.rs`), whose
generic-family map is empty by default. `crates/frust-shell-web`'s
`install_default_fonts` now registers a bundled Inter Variable face as the
`SystemUi`/`SansSerif` generic-family fallback
(`frust_text::register_generic_fallback`) unconditionally, before the shell
builds its own `TextContext` — no per-app opt-in, and this app does not opt
in to anything for it. `Monospace`, `Serif` and `Emoji` remain **unmapped**
by design (a proportional face substituted for `Monospace` would silently
regress `TextInput`/code-display layout, and the bundled face set has
neither a serif nor an emoji face) — see `docs/LIMITATIONS.md`'s
`web-generic-family-partial-fallback` entry for the authoritative record.

### What this binary now registers itself

A design system's text does not go through a bare generic slot: it goes
through `FontFamily::stack_with_generic([<named family>], <generic>)`, whose
named half resolves only against faces somebody registered. Nothing
registers them here by inheritance — the registry builds its design-system
cases directly rather than through each plugin's `install()`
(`examples/gallery/src/glyph.rs` explains why), and `install()` is the only
seam that registers them. So every stack whose generic tail is `Monospace`
fell through to nothing and rendered **zero glyphs**, under CPU posters that
look perfectly correct because the poster host has the real faces.

`src/main.rs`'s `register_design_system_fonts` closes that: it hands the
bundled Glyph, shadcn and beUI faces to `frust::register_app_fonts` in
`AppState::new`, before `run_app` constructs the shell (the timing contract
`register_app_fonts`' own doc states). It deliberately does **not** call any
plugin's `install()`, which would also `set_default_theme` and fight this
app's own per-case seeding. Per design system, checked against each
plugin's own font module and manifest rather than assumed from Glyph's
shape:

| Design | Mono/named exposure | Bundling | Registered here |
|---|---|---|---|
| Glyph | **Both** type faces are `stack_with_generic([Space Mono \| IBM Plex Mono], Monospace)` (`plugins/glyph/src/tokens/scales.rs`) — all 14 `glyph/*` cases | 7 faces, unconditional `include_bytes!`, no feature gate; `font_data` re-exported at the crate root | **Yes** |
| shadcn | `mono_family()` = `[JetBrains Mono] + Monospace` (`tokens/theme.rs`), used by `kbd`/`questionnaire` shortcut badges; body text tails `SansSerif` and always rendered | 2 faces, unconditional; `font_data` at crate root | **Yes** |
| beUI | `mono_family()` = `[Geist Mono] + Monospace` (`tokens/theme.rs`), used by `code-block`, loaders, `number`, agents blocks; body tails `SansSerif` | 2 faces, unconditional; `font_data` at crate root | **Yes** |
| Material | **No `Monospace` slot at all** — its one stack is `[Roboto Flex] + SansSerif`, which the shell already maps. Roboto Mono is bundled but no type token selects it | 2 faces, unconditional — but `mod tokens;` is **private** and the crate root does not re-export `font_data`, so there is no public accessor | No — not exposed to this defect, and could not be registered if it were |
| Cupertino | Declares no font family anywhere; keeps the mapped `SystemUi` default | none bundled | No — nothing to register |

**This does not map a generic.** Registering a named face and mapping a
generic slot are different things, and only the first happens here: text
that resolves to a bare `Monospace`, `Serif` or `Emoji` generic with no
named face in front of it still renders nothing on this target. The
`docs/LIMITATIONS.md` entry stands unchanged, and its stated trigger for
removal ("a bundled monospace/serif/emoji policy, or a page-side
font-loading seam that lets an app supply those faces without paying for
them in every binary") is still the open item.

### Browser evidence: measured text, not a canvas that appeared

The reason this defect survived a full phase and three review rounds is that
the browser checks ran `Base` cases only, and the readiness probe asserts a
canvas *appeared* — which it does whether or not a single glyph resolves.
The measurement below therefore asserts on rendered text: real Chrome
(Playwright, `channel: "chrome"`, headed, DPR 1, `colorScheme: light`) loads
`?case=<slug>&embed=1&theme=light` from the manual recipe's own artifact,
screenshots the shell's canvas, and computes **horizontal-edge density**
(fraction of pixels whose luminance differs from the next pixel's by more
than 24/255 — text is overwhelmingly the source of such edges) for the live
capture and for the case's committed CPU poster in the website's
`static/preview/`.

Live and poster are not expected to reach parity: the embed view anchors a
case top-left while the poster centres it in the frame, so the `Base`
control's own live/poster edge ratio (0.386) is the ceiling this comparison
can reach, and it is the bar the design-system cases are read against.

| Case | Live edges before | Live edges after | Poster edges | Live/poster before → after |
|---|---|---|---|---|
| `glyph/term-block` | 0.0011 | 0.0139 | 0.0343 | 0.033 → **0.405** |
| `glyph/list` | 0.0019 | 0.0107 | 0.0300 | 0.062 → **0.358** |
| `beui/code-block` | 0.0010 | 0.0067 | 0.0183 | 0.054 → **0.366** |
| `shadcn/questionnaire` | 0.0097 | 0.0099 | 0.0278 | 0.350 → **0.356** |
| `button` (`Base` control) | 0.0078 | 0.0078 | 0.0203 | 0.385 → **0.386** |

Read alongside the captures themselves: before, `glyph/term-block` drew the
terminal chrome and its three window dots over an empty collapsed body,
against a poster showing four lines of shell transcript — after, all four
lines are there. `beui/code-block` went from a header strip (`TYPESCRIPT`,
`Ready` — both sans) above an empty code area to line numbers plus three
syntax-coloured lines. `shadcn/questionnaire` moves barely at all in
aggregate, correctly: only its three shortcut badges are mono, and they went
from empty rings to `A`/`B`/`C`. The `Base` control does not move at all,
which is the point of including it.

### It is not free: +1.22 MB uncompressed, +0.53 MiB gzipped

The bytes are bundled unconditionally by all three plugins, but they were
**not** all in the artifact beforehand — only the faces something reachable
still referenced. Each plugin's `native_typefaces` binding names one or two
specific faces (Glyph: Space Mono Regular + IBM Plex Mono Regular; shadcn:
Inter; beUI: Geist), and the linker dropped the rest as unreachable.
Measured on the manual recipe (`cargo build --release --target
wasm32-unknown-unknown` → `wasm-bindgen` → `wasm-opt`, `wasm-bindgen-cli`
0.2.128 / `wasm-opt` 132 / `cargo` 1.98.1), same host, same base commit,
back to back:

| Stage | Before | After | Delta |
|---|---|---|---|
| `wasm-bindgen` output | 11,535,394 B | 12,760,170 B | +1,224,776 B (+10.6%) |
| after `wasm-opt -O` | 10,730,018 B | 11,951,777 B | +1,221,759 B (+11.4%) |
| gzip `-9` (what a reader downloads) | 4,467,316 B | 5,027,473 B | +560,157 B (+12.5%) |

That delta is exactly the previously-dropped faces: the three plugins bundle
2,543,516 B of TTFs, of which 1,321,172 B were already linked through
`native_typefaces`, leaving 1,222,344 B unreachable — within 585 B of the
measured `wasm-opt`-stage growth. So the honest version of "the bytes are
already linked in" is: *a fifth of them were.* Registering the rest costs
about half a megabyte on the wire, which is a real input to the
live-preview download budget the site advertises, not a rounding error. The
lever if that proves too expensive is registering fewer faces (each family's
Regular only, accepting synthesized weights) rather than fewer families —
dropping a family reopens the blank-text defect for its whole page set.
These numbers are a snapshot of one host and one pair of builds; re-measure
before citing them elsewhere, exactly as the "Binary size" table above says
of its own.

### The app's own label face

`src/main.rs`'s `LABEL_FONT_BYTES` still `include_bytes!`es
`plugins/shadcn/fonts/inter/InterVariable.ttf` and registers it by name for
this app's own chrome (`label()`/`nav_row()` name `"Inter Variable"`
explicitly — the variable release's real name-table entry, not `"Inter"`).
It is now redundant twice over: the shell's generic fallback covers it, and
`register_design_system_fonts` registers shadcn's own copy of the identical
file. It is kept so this app's chrome does not depend on a plugin's bundle;
retiring it, and the duplicated ~0.88 MB payload behind it, is a follow-up
with a real size win attached, not part of this change.

## Registry size: 107 cases, not 35

An earlier draft of this document asserted the registry held only 35 cases,
all `Design::Base` — accurate at an earlier base commit, when
`examples/gallery/src/lib.rs`'s `cases()` still only called `base::cases()`
and its own doc comment said so explicitly. That assertion was not corrected
when the design-system case batches landed; it was instead carried forward
into text a later pass of this same document rewrote (the "Binary size"
section above), while `src/main.rs`'s own doc comment, added in that same
pass, correctly said 107 — one draft of this file contradicting itself.

As of this task's own base commit, `lib.rs`'s `cases()` (`lib.rs:80-89`)
concatenates `base::cases()` with every design-system module's own `CASES`
slice (`DESIGN_PARTS` at `lib.rs:67-73`: material, cupertino, glyph, shadcn,
beui, in that order) into one cached, `'static` slice. Counted directly
against that function: **107 cases total** — 35 `Base`, 23 `Material`, 15
`Shadcn`, 14 `Glyph`, 13 `Beui`, 7 `Cupertino` — every slug unique
(`examples/gallery/tests/registry.rs`'s `slugs_are_unique`). This app's own
`?case=`/index-page code has always read the registry *dynamically*
(`frust_gallery::cases().len()` is what the index page's own "N cases in the
registry" line reports, `src/main.rs:451`), so no app-code change was needed
here when the batches landed — only this document was wrong.

The per-case `set_default_theme` override in `AppState::new` for a
`Design != Base` case (see "Embedding" above) is likewise **not** dormant,
contrary to what an earlier draft of this section claimed: it runs for every
one of the 72 non-`Base` cases now in the registry, including two of the six
slugs the website's own catalog currently embeds (`material/tabs`,
`beui/button`) — a `?case=material/tabs` request exercises this branch on
every load.

## Files

| File | Purpose |
|---|---|
| `Cargo.toml` | Standalone workspace manifest: the `frust` facade + `frust-gallery` path deps, the `wasm32`-gated `wasm-bindgen`/`web-sys`/`js-sys`/`wasm-bindgen-futures`/`frust-shell-web` rows, and this crate's own `[profile.release]`. |
| `src/main.rs` | The app. `fn main() {}` (required for a `[[bin]]` target; the real entry is `wasm_bindgen(start)`) plus a `wasm32`-only `mod app`: `AppState`, the font-registration/case-theme-embed-resolution/log-level startup sequence (`resolve_case_from_query`, `resolve_theme_override_from_query`, `resolve_embed_from_query`), `label`/`nav_row` (the font-defect workaround, now largely redundant — see "Text on `wasm32`" below), `CaseHost` (the `Component` state-boundary bridge into a `()`-state `Case`), `case_view`/`embedded_case_view`/`index_view`, and the `frust::web_app!` invocation. |
| `index.html` | Host page for the manual recipe. No `<canvas>` of its own (the shell creates one) — carries the `?arm=webgl` WebGPU-removal script and the load-failure `<pre id="log">` mirror; canvas-host binding, resize/DPR sync, and the iframe height-report contract are now `platform/web/frust_web.js`'s `mount()` (imported directly, see "Embedding" above), not a page-local script. Not read at all by `frust build web --release`, which stages the framework's own page instead (see "Primary recipe" above). |
| `README.md` | This file. |
| `pkg/` | `wasm-bindgen`/`wasm-opt` output (manual recipe). Generated; **not committed** (`.gitignore`-covered — see "Serve" above). |
| `target/` | Cargo build output. Generated; **not committed** (`.gitignore`-covered). |
| `build/` | `frust build web --release`'s own output directory (primary recipe). Generated; **not committed**, and `.gitignore`-covered alongside `pkg/` and `target/`. |
