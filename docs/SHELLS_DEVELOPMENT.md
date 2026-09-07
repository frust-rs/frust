# Frust - SHELLS Development

Device/emulator gates owned by the SHELLS unit (`frust-shell-common`, `frust-shell-desktop`,
`frust-shell-macos`, `frust-shell-windows`, `frust-shell-linux`, `frust-shell-android`,
`frust-shell-ios`, plus the in-repo platform embedding modules). Shared
prerequisites, build/run commands, the standard verify gate, the mobile compile gates, and the
version-pin policy live in [DEVELOPMENT.md](DEVELOPMENT.md); the unit's design lives in
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md).

No gate below has an automated counterpart — each is a person-driven check against an installed
app (`frust run -d <device>` on mobile, a real desktop session otherwise).

## Deep-link manual test (Android)

A device/emulator gate for `nativeOnDeepLink` ([SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s
cross-cutting host-signal flow, deep-link) on a project scaffolded with `--deeplink-scheme
<scheme> [--deeplink-host <host>]`, installed via `frust run -d <device>`:

```bash
# Cold start (app not running; queues until the native handle exists):
adb shell am force-stop <package>
adb shell am start -a android.intent.action.VIEW -d "<scheme>://<path>" <package>

# Warm (already foregrounded; singleTop routes via onNewIntent):
adb shell am start -a android.intent.action.VIEW -d "<scheme>://<other-path>" <package>
```

Confirm the app navigates to the linked route both times. iOS (`frust_on_deep_link`) has
a Simulator-only CLI trigger (`xcrun simctl openurl booted "<scheme>://<path>"`); a
physical device has none — tap a registered `CFBundleURLSchemes` link instead.
`frust.toml`'s `[deeplink]` section is informational only, so use `--overwrite` or edit
the platform files directly to change the scheme.

## Safe-area / keyboard / back manual test (Android + iOS)

A device/emulator gate for the inset and back contracts (see
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s cross-cutting host-signal flow), against an
installed app (`frust run -d <device>`) — no CLI trigger like the deep-link gate above, so each
is a person-driven check:

- **Safe-area:** rotate the device; confirm top/bottom-anchored content reflows around
  the status bar/notch/gesture-nav insets in both orientations.
- **Keyboard:** focus a bottom text field; confirm content shifts clear of the on-screen
  keyboard, then dismiss it and confirm the layout returns.
- **Back:** press hardware/gesture back on a pushed route; confirm it pops one level,
  and falls through to the platform's own default at the root.
- **Density refresh:** move a running app between displays of different density; confirm
  layout rescales rather than sticking to launch-time density.

## Per-OS desktop shell gate (macOS + Windows + Linux)

A hardware gate for the native integration the shared winit core cannot cover, run against an app
configured with an app name, reverse-DNS id, window icon and menu spec. macOS and Windows are
closed (Windows 2026-08-19, via `examples/shadcn-demo` built natively with MSVC); Linux is still
owed (see [LIMITATIONS.md](LIMITATIONS.md) `desktop-shells-runtime-unverified` for exactly which
checks remain). Per host:

- **macOS:** menu bar shows the app-named application menu; ⌘Q and the menu Quit both exit;
  Hide/Show All work; closing with `quit_on_last_window_closed = false` hides the window and a
  Dock click from the *inactive* state brings it back; the zero-config preview is unchanged.
- **Windows:** titlebar and taskbar icons; taskbar grouping under the configured AppUserModelID;
  titlebar brightness follows an app-forced theme flip; the native menu bar and each declared
  accelerator activate their item.
- **Linux:** the window pairs with its `.desktop` entry (Wayland `app_id` / X11 `WM_CLASS`) and
  the X11 window icon shows; run non-headless, on both session types where available.

## Browser manual gate (web)

A gate for `crates/frust-shell-web`, run against `examples/web-gallery` per its own README's
Build/Serve sections (`cargo build --release --target wasm32-unknown-unknown`, then
`wasm-bindgen --target web ...` and `wasm-opt -O --all-features ...`, served by anything that
sends `.wasm` as `application/wasm`) — the README has the full recipe, including the
ship-the-optimized-artifact copy step; do not restate it here. Open the served page in a
Chrome 15x with WebGPU and check:

- pointer, wheel, touch-drag scroll, and keyboard text entry (typing into the index page's
  filter field) all reach the app
- dark/light theme flips live, with the app already running and no reload
- resize and DPR 2 track live and stay pixel-correct
- the index page's ticking counter advances with zero pointer/keyboard events (proves a signal
  write alone drives the repaint, not a poll loop)

Then reload with `?arm=webgl` (forces the WebGL2 fallback arm): shapes, layout, backgrounds,
theme colours and text should now all stay pixel-correct — the atlas two-layer floor (see
[LIMITATIONS.md](LIMITATIONS.md) `engine-webgl2-atlas-target`) is what makes glyphs render
instead of painting as solid opaque boxes; a regression here means that floor broke. Chrome's
`--disable-features=WebGPU` flag does **not** reach this fallback on this stack —
`requestAdapter` fails outright instead of falling through to GL — so `?arm=webgl` is the only
way to exercise the WebGL2 arm (per the README's own record of that finding).

## Version Pins

The pins this unit owns, under [DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy (pins are
LAW; re-run the row's tripwire after touching it, and never run a blind `cargo update`):

| Pin | Why | Tripwire |
|---|---|---|
| `muda 0.19` minor | `frust-shell-macos`'/`frust-shell-windows`'s native menu-bar bindings (`NSMenu`/`HMENU` built from `DesktopConfig::menu_spec`). Pins `objc2 0.6.1` + `objc2-app-kit 0.3.x`, unifying with this workspace's existing `objc2` family; muda's own `gtk`/`libxdo` deps live behind a Linux/BSD-only target table internally, so this pin resolves no GTK dependency on macOS/Windows — `frust-shell-linux` deliberately never depends on it | `cargo check --target aarch64-apple-darwin -p frust-shell-macos && cargo check --target x86_64-pc-windows-gnu -p frust-shell-windows`, plus `cargo tree -d` (watch for muda bumping its `objc2` lineage); on a bump, re-verify two Windows contracts this pin's internals carry (`frust-shell-windows/src/menu.rs`): submenu accelerators register into the root `HACCEL` only while the submenu is attached (attach-then-fill order), and the predefined quit stays unused (it calls `PostQuitMessage`, inert under winit) |
| `windows-sys 0.61` minor | `frust-shell-windows`'s Win32 bindings (`SetCurrentProcessExplicitAppUserModelID`, `TranslateAcceleratorW`/`HACCEL`/`MSG`). Resolved empirically, not guessed: muda 0.19.3's own `>=0.60, <=0.61` constraint picks `0.61.2`, the minor already dominant in this lockfile. No features declared at the workspace level — each consuming crate selects its own list (`frust-shell-windows/Cargo.toml`: `Win32_UI_Shell`, `Win32_UI_WindowsAndMessaging`) | `cargo check --target x86_64-pc-windows-gnu -p frust-shell-windows`; `cargo tree -d --target x86_64-pc-windows-msvc` — watch for a third `windows-sys` minor appearing beyond the pre-existing `0.52.0`/`0.61.2` pair |
| `wasm-bindgen =0.2.128` exact | The crate's own glue-generation ABI schema, which must equal the host `wasm-bindgen-cli` — a stricter contract than semver, so a mismatch fails at glue-generation time, not at `cargo check`. `crates/frust-shell-web`'s `[target.'cfg(target_arch = "wasm32")'.dependencies]` table is where the pin is anchored (a workspace row only takes effect where a member depends on it, and this crate is the browser tier's only entry point); `crates/frust`'s own wasm32 table re-exports it unconditionally for `web_app!`'s start shim. The exact pin forces `wasm-bindgen-futures`/`js-sys`/`web-sys` into lockstep below, since each declares its own exact `wasm-bindgen` requirement. `wasm-bindgen-macro-support`'s `syn` dependency resolves onto the same `syn 3.0.3` this lockfile already carries via `async-trait`/`bytemuck_derive` — no second `syn 3.x` identity | `cargo check --target wasm32-unknown-unknown -p frust --tests` (docs/DEVELOPMENT.md), plus `examples/web-gallery`'s packaging step (`wasm-bindgen --target web ...`, its README's Build section) erroring on a CLI/crate schema mismatch; `cargo tree -i wasm-bindgen` — watch for a second `wasm-bindgen` identity or a `syn` split |
| `wasm-bindgen-futures 0.4.77` / `js-sys 0.3.104` / `web-sys 0.3.104` caret | Forced to lockstep by the `wasm-bindgen` exact pin above (this lockfile currently resolves `0.4.78`/`0.3.105`/`0.3.105`). `web-sys` features are declared per crate, not at the workspace level, and trimmed to what each caller actually touches — `frust-shell-web` takes only `Window` (the `setTimeout` half of its surface-bring-up retry wait); `crates/frust`'s own wasm32 table pulls no `web-sys`/`js-sys` row at all. `wasm-bindgen-futures` supplies `spawn_local` (no blocking executor exists on a target that must never block) and the `JsFuture` half of that same retry wait; `js-sys` supplies `Promise`, the object the `setTimeout` callback resolves | `cargo check --target wasm32-unknown-unknown -p frust-shell-web -p frust-gallery` (docs/DEVELOPMENT.md); `cargo tree -i js-sys -i web-sys -i wasm-bindgen-futures` — watch for a second minor entering the lockstep |
| `web-time 1.1.0` | `std::time::Instant::now()` panics on `wasm32-unknown-unknown` (no clock syscall on that target); `web-time` is the drop-in replacement winit 0.30.13 itself already resolves internally, so this pin introduces no new identity. Every `Instant` in `frust-shell-web` is `web_time::Instant` — winit types `ControlFlow::WaitUntil` over it on this target, so using anything else would also mistype that contract | `cargo check --target wasm32-unknown-unknown -p frust-shell-web -p frust-gallery`; `cargo tree -i web-time` — expect exactly one entry, resolved through `winit` |
| `console_log 1.0.0` / `console_error_panic_hook 0.1.7` caret | (this lockfile currently resolves `console_log 1.1.0` from the `1.0.0` caret; `console_error_panic_hook 0.1.7` exact-matches) stderr is a silent no-op in a browser. The facade's `web_app!` bootstrap (`frust::__web_bootstrap`, the start shim's first call) installs both unconditionally — `console_error_panic_hook::set_once()` and `console_log::init_with_level(log::Level::Warn)` — before `frust_shell_web::logging::install` runs. A second `install` call is treated as "someone got here first" and swallowed rather than erroring (`log` allows exactly one sink per process), so an app or the gallery's own `?log=` query param re-levelling the sink afterward is safe and idempotent | `cargo check --target wasm32-unknown-unknown -p frust --tests`; `cargo test -p frust-shell-web` (the `logging` module's own unit tests) — watch for `frust-shell-web`'s `DEFAULT_LEVEL` (`Warn`) drifting from the facade bootstrap's own default |
| `wasm-bindgen-test =0.3.78` exact | `crates/frust-testing`'s `tests/wasm_goldens.rs`/`tests/wasm_binary_invariants.rs` browser harness (`wasm_bindgen_test_configure!(run_in_browser)`, `#[wasm_bindgen_test]`), declared under that crate's `[target.'cfg(target_arch = "wasm32")'.dev-dependencies]` behind its non-default `webgl` feature — crate-local, with no root `[workspace.dependencies]` row. Pinned exactly because it is one half of a two-part schema contract, not an ordinary semver dependency: `0.3.78` is the release paired with the `wasm-bindgen =0.2.128` exact pin above, and the host `wasm-bindgen-test-runner` (shipped by that same `wasm-bindgen-cli 0.2.128`) refuses a binary whose schema hash does not match. Bump it only together with the `wasm-bindgen` pin and the installed CLI, never on its own | `CHROMEDRIVER_REMOTE=http://localhost:9517 CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner cargo test -p frust-testing --target wasm32-unknown-unknown --features webgl --release --test wasm_goldens`; `cargo tree -i wasm-bindgen-test` — watch for a second identity or a drift from the `wasm-bindgen` pin's own version |

`objc2-app-kit 0.3` (the `NSApplication`/`NSResponder` slice `frust-shell-macos` names directly,
for the Dock-reopen observer) joins the objc2 pin family owned by
[PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md) rather than getting a row here — see its Version
Pins table. `frust-shell-macos`'s own manifest declares it with a literal version and a trimmed
`default-features = false` feature list, not `{ workspace = true }`, because Cargo forbids a
member from overriding `default-features` on a workspace row that doesn't set it; the two must be
kept in step (same workaround as `plugins/clipboard`'s `objc2-ui-kit` row). Known accepted
residual: `objc2-app-kit` resolves to two versions (`0.2.2` via `accesskit_macos`/winit,
`0.3.2` via this pin/`arboard`) — pre-existing, not widened by this pin, do not force-align.

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) — the unit's design
- [TESTING.md](TESTING.md) — test tiers, golden images, emulator runbook
