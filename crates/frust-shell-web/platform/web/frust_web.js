// frust_web.js — vanilla-JS host-page glue for embedding a Frust
// `wasm-bindgen`-built browser app (a `frust::web_app!` binary). This is the
// browser embedder's counterpart to the Android and iOS embeddings shipped in
// `crates/frust-shell-android` and `crates/frust-shell-ios`: an app
// author's own host page (`index.html` here, or a page they write themselves)
// imports this module and hands it their app's generated `init` function; it
// does the rest of the host-page wiring the shell itself has no browser-DOM
// seam to do on its own. See README.md for the full contract this module
// implements and the parts of it that are still evidence-only pending a
// Rust-side hook.
//
// No framework, no bundler dependency: plain ES module `import`/`export`,
// loaded directly by a `<script type="module">` tag.

/** Default host element selector `mount()` binds winit's canvas into. */
export const DEFAULT_HOST_SELECTOR = "#frust-host";

/** Default element `mount()`/`reportFailure()` mirror a load failure into. */
export const DEFAULT_LOG_SELECTOR = "#frust-log";

function resolveElement(selectorOrElement, fallbackSelector) {
  if (selectorOrElement instanceof Element) {
    return selectorOrElement;
  }
  return document.querySelector(selectorOrElement || fallbackSelector);
}

/**
 * Report a load failure the same way a Rust-side panic already is (via the
 * facade's `console_error_panic_hook` install): into `document.title`, an
 * on-page log element, and `console.error`. Exported so a host page's own
 * `import(moduleUrl)` failure (before `mount()` is ever reached — a bad
 * `?module=` path, a 404) can be reported through the identical path a wasm
 * init failure uses.
 */
export function reportFailure(err, options = {}) {
  const message = `FAIL: ${err}`;
  document.title = `${document.title} | ${message}`;
  const logElement = resolveElement(options.log, DEFAULT_LOG_SELECTOR);
  if (logElement) {
    logElement.textContent += `${message}\n`;
  }
  console.error("[frust-web]", err);
}

// --- canvas <-> host binding -------------------------------------------
//
// winit's web backend creates its own `<canvas>` and appends it straight to
// `<body>` (`with_append(true)` — see `crates/frust-shell-web/src/
// app_handler.rs`'s `resumed`); it has no seam yet for adopting a host
// page's own element instead. It also pins that canvas's inline
// `style.width`/`style.height`, in pixels, at creation
// (`DEFAULT_CANVAS_SIZE`, 800x600) — and an inline declaration always
// outranks an ordinary stylesheet rule in the CSS cascade, `!important` or
// not, regardless of selector. So a page that wants the canvas to track a
// host element's own box (rather than sitting fixed at 800x600 forever)
// cannot do it with CSS alone; it has to overwrite that inline declaration
// with a plain JS property assignment, which — unlike a stylesheet rule —
// actually replaces winit's own inline value. Once replaced, winit's own
// `ResizeObserver` (already attached to the canvas) picks up the new box as
// an ordinary observed resize and drives `WindowEvent::Resized` exactly as
// it would for any other box change; nothing here talks to wgpu, winit, or
// the shell directly.
//
// This is the exact mechanism `examples/web-gallery/index.html`'s own
// `MutationObserver` proved out (see that file's "Milestone 6" finding);
// this module generalizes it from a fixed `100vw`/`100vh` target to an
// arbitrary host element's own box, kept in sync live via `ResizeObserver`.

function bindCanvasToHost(host, onCanvasReady) {
  const existing = document.body.querySelector("canvas");
  if (existing) {
    adopt(existing);
    return;
  }
  // The canvas does not exist yet: winit's surface bring-up is asynchronous,
  // running well after this module's own top-level code (and even after
  // `init()` has returned — see `mount()`'s own comment), so a
  // `MutationObserver` on `<body>` is what catches it the moment it
  // actually appears.
  const observer = new MutationObserver(() => {
    const canvas = document.body.querySelector("canvas");
    if (!canvas) {
      return;
    }
    observer.disconnect();
    adopt(canvas);
  });
  observer.observe(document.body, { childList: true });

  function adopt(canvas) {
    // Reparenting a live `<canvas>` element (a plain DOM move, not a
    // `width`/`height` IDL-attribute write) preserves its bitmap and its
    // rendering context — only the IDL attributes reset it, which nothing
    // here ever touches — so moving it under `host` is safe whether or not
    // wgpu has already bound a surface to it by the time this runs.
    host.appendChild(canvas);
    canvas.style.display = "block";
    canvas.style.border = "none";
    canvas.style.padding = "0";
    canvas.style.margin = "0";
    syncCanvasToHost(canvas, host);
    watchHostResize(canvas, host);
    onCanvasReady(canvas);
  }
}

function syncCanvasToHost(canvas, host) {
  const rect = host.getBoundingClientRect();
  // A plain property assignment, in CSS pixels off the host's own box — the
  // load-bearing overwrite of winit's pinned inline declaration. Re-run on
  // every host resize (see `watchHostResize`) and on every observed
  // `devicePixelRatio` change (see `watchDevicePixelRatio`), so a live
  // resize keeps reaching the shell for the whole life of the page, not
  // only once at startup.
  canvas.style.width = `${rect.width}px`;
  canvas.style.height = `${rect.height}px`;
}

function watchHostResize(canvas, host) {
  const observer = new ResizeObserver(() => {
    syncCanvasToHost(canvas, host);
    console.debug(
      `[frust-web] host resized to ${host.clientWidth}x${host.clientHeight}` +
        " - canvas inline style re-synced",
    );
  });
  observer.observe(host);
  return observer;
}

// --- devicePixelRatio -------------------------------------------------
//
// A browser fires no `devicepixelratiochange` event; `matchMedia` against
// the current ratio, re-armed after every fire, is the standard vanilla-JS
// idiom for observing it. `crates/frust-shell-web/src/app_handler.rs`'s
// `WindowEvent::ScaleFactorChanged` handler already documents that winit
// "guarantees a following `Resized`" for a real DPR change, so the shell
// needs no help acting on one once it is detected — this listener's own job
// is evidence (a console line proving the browser reported the change at
// all) plus a defensive re-sync of the canvas's pixel geometry, in case a
// DPR-only change (no host-box size change) would otherwise leave
// `syncCanvasToHost`'s last write stale.

function watchDevicePixelRatio(onChange) {
  function attach() {
    const query = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    query.addEventListener(
      "change",
      () => {
        onChange(window.devicePixelRatio);
        attach();
      },
      { once: true },
    );
  }
  attach();
}

// --- visibility (pause/resume) -----------------------------------------
//
// There is no Rust-side pause/resume export this can call into today —
// `frust-shell-web`'s frame loop is entirely `requestAnimationFrame`-driven
// under `ControlFlow::Wait`, and a browser already stops delivering rAF
// callbacks to a hidden tab (throttling it to near-zero) without any page
// script asking it to, which is the "already degrades gracefully" behaviour
// the task names. This listener makes that implicit degrade explicit, as
// console evidence, rather than calling a hook that does not exist; see
// README.md's "Known gaps" for the Rust-side hook a real pause (releasing
// GPU resources on hidden, say) would need.

function watchVisibility() {
  document.addEventListener("visibilitychange", () => {
    console.info(`[frust-web] visibility: ${document.visibilityState}`);
  });
}

// --- popstate / URL deep link / back ------------------------------------
//
// Like visibility above, there is no Rust-side router hook to forward into:
// `frust-shell-web` reads nothing from `location`/`history` itself. An app
// that wants real deep-linking can already read `location.pathname`/
// `.search` and listen for `popstate` directly through `web-sys` from its
// own Rust code (`web_sys::window().unwrap().location()`), exactly the way
// `examples/web-gallery`'s `?case=`/`?log=` query parameters are read today
// (that app resolves them from a plain `&str`, not through this module).
// This listener's job is the same as visibility's: console evidence that a
// browser back/forward action reaches the page at all, satisfying the
// milestone's own "console-log evidence is fine" acceptance.

function watchHistory() {
  console.info(`[frust-web] initial location: ${location.href}`);
  window.addEventListener("popstate", (event) => {
    console.info(`[frust-web] popstate -> ${location.href}`, event.state);
  });
}

// --- iframe height postMessage ------------------------------------------
//
// When this page is embedded inside an `<iframe>` (the sibling previews use
// case this milestone names), the parent page has no way to size the frame
// to its content on its own — the frame's own document has no intrinsic
// height a parent's layout can read across the origin boundary. Posting the
// content height explicitly is the documented, work-anywhere way around
// that; `"*"` as the target origin is deliberate here (a preview host is not
// assumed to be same-origin), and the message shape (`{ type, height }`) is
// this module's own tiny contract — document it on the consuming page.

function postHeightToParent() {
  if (window.parent === window) {
    return; // not embedded — nothing to post, and no parent to post to.
  }
  const height = document.documentElement.scrollHeight;
  window.parent.postMessage({ type: "frust:height", height }, "*");
  console.debug(`[frust-web] posted height ${height}px to parent frame`);
}

// --- public entry point --------------------------------------------------

/**
 * Wire up one Frust browser app's host page: bind the canvas winit creates
 * to a host element and keep it in sync on resize/DPR change, forward
 * visibility/history/iframe-height signals as documented above, then await
 * the app's own wasm-bindgen `init` and report a failure the way a Rust
 * panic already is.
 *
 * `init` is the `default` export of a `wasm-bindgen --target web` build
 * (e.g. `import init from "./pkg/app.js"`) — its own generated glue already
 * implements `WebAssembly.instantiateStreaming` with an `arrayBuffer`
 * fallback for a server that answers a `.wasm` request with the wrong
 * `Content-Type` (its `__wbg_load`), so this function awaits it rather than
 * re-implementing that fallback a second time.
 *
 * `options.host` — a `#selector` string or an `Element` — defaults to
 * `DEFAULT_HOST_SELECTOR`. `options.log` likewise defaults to
 * `DEFAULT_LOG_SELECTOR`.
 *
 * Returns a promise that resolves once `init()` has settled (successfully
 * or not — a failure is reported, not rethrown, rather than leaving an
 * unhandled rejection for a caller who did not ask for one).
 */
export function mount(init, options = {}) {
  const host = resolveElement(options.host, DEFAULT_HOST_SELECTOR);
  if (!host) {
    throw new Error(
      `frust_web.mount: host element not found (${options.host || DEFAULT_HOST_SELECTOR})`,
    );
  }

  // Wired before `init()` is ever called: winit's canvas can appear at any
  // point after the wasm module starts running (bring-up is asynchronous),
  // so every observer here must already be listening by the time it does.
  watchHistory();
  watchVisibility();
  bindCanvasToHost(host, () => postHeightToParent());
  watchDevicePixelRatio((dpr) => {
    console.info(`[frust-web] devicePixelRatio changed to ${dpr}`);
    const canvas = host.querySelector("canvas");
    if (canvas) {
      syncCanvasToHost(canvas, host);
    }
  });
  window.addEventListener("resize", () => postHeightToParent());

  return Promise.resolve()
    .then(() => init())
    .then(() => {
      console.info("[frust-web] wasm module initialized");
      postHeightToParent();
    })
    .catch((err) => reportFailure(err, options));
}
