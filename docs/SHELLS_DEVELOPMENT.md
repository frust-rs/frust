# Frust - SHELLS Development

Device/emulator gates owned by the SHELLS unit (`frust-shell-common`, `frust-shell-desktop`,
`frust-shell-macos`, `frust-shell-windows`, `frust-shell-linux`, `frust-shell-android`,
`frust-shell-ios`, plus the in-repo platform embedding modules). Shared
prerequisites, build/run commands, the standard verify gate, the mobile compile gates, and the
version-pin policy live in [DEVELOPMENT.md](DEVELOPMENT.md); the unit's design lives in
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md).

No gate below has an automated counterpart — each is a person-driven check against an installed
app (`frust run -d <device>` on mobile, a real desktop session otherwise).

The embedding modules a gate exercises live under `platform/` in `frust-shell-android`,
`frust-shell-ios` and `frust-shell-web`; a project reaches them through `packages::locate` plus
`platform_wiring::sync` (Android `frust.embedding.dir` in `android/local.properties`, the
`ios/FrustEmbedding` symlink, the web embedder lookup). `frust run`/`frust build` with `-d
android|ios` refresh the wiring before building, so after editing a checkout's embedding in path
mode just rebuild; the mechanism is in [CLI_DEVELOPMENT.md](CLI_DEVELOPMENT.md).

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

## Overlay and surface-mode manual test (Android)

A device gate for the overlay host ([SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) § Native-view
hosting) on `examples/native-widgets-demo`'s `sandwich` page. Overlays need the opaque (Mode A)
surface and `FRUST_OVERLAYS` (default off; knob table in [DEVELOPMENT.md](DEVELOPMENT.md)): build
with `frust run -d <device> --define FRUST_OVERLAYS=1`, then open the page. A **debuggable** build
accepts the boolean intent extra `dev.frust.translucentSurface` to force the surface mode for one
launch; the mode latches process-wide at the first surface, so force-stop between modes:

```bash
adb shell am force-stop it.f0x.native_widgets_demo
adb shell am start -a android.intent.action.VIEW -d native-widgets-demo://section/sandwich \
  --ez dev.frust.translucentSurface false it.f0x.native_widgets_demo
```

Run it with `false` (Mode A: overlays show the popover and badge over the native views) and, after a
force-stop, with `true` (Mode B: the overlay ops are ignored and the main surface shows the same
content). The "Interleaved" checkbox shows the known limit (`android-overlays-cover-later-slots`).
Device outcome (Pixel 5): the overlay, rotation and Mode B legs pass; a newly attached or recreated
overlay surface gets no frame while the app is idle (`android-overlays-opt-in-pending-device-leg`).

## Apple overlay manual test (macOS + iOS)

The Apple overlay hosts ([SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) § Native-view hosting) on
`examples/native-widgets-demo`'s `sandwich` page, behind `FRUST_OVERLAYS` (default off). Perf lines
below need a `perf-trace` build plus `FRUST_TRACE=1`.

```bash
# macOS (run inside examples/native-widgets-demo):
env -u DYLD_LIBRARY_PATH FRUST_OVERLAYS=1 FRUST_DEMO_SECTION=sandwich cargo run
# iPhone: build with the knob, launch with FRUST_DEMO_OPAQUE=1, open the page by deep link:
frust build ios --profile --define FRUST_OVERLAYS=1
#   native-widgets-demo://section/sandwich
# Simulator: set the knobs through simctl's child environment:
SIMCTL_CHILD_FRUST_DEMO_OPAQUE=1 SIMCTL_CHILD_FRUST_OVERLAYS=1
```

Overlays are Mode A only; the demo's `FRUST_DEMO_OPAQUE` selects the opaque surface on iOS. The
frust card must sit above the native controls it overlaps and, under "Interleaved", below the later
native slider. Toggling it, backgrounding and returning, and rotating must all settle with no
further input. The proving log lines: `overlay surfaces on (one per differ cut, Mode A)`,
`overlay N view above slot S`, `first segmented frame submitted/presented: main + K overlay(s)`,
`frust-overlay skew frames=120 … armed=`, `frust-overlay cost overlay=N gpu_main_us=…`,
`overlay N refused …` and `content kept on main …`. Outcome: iPhone SE and iOS Simulator pass with
0 uncaptured wgpu errors in Mode A and Mode B; macOS passes in both paint orders (a withdraw can
stall the UI thread up to 2 s).

## Migrating an already-scaffolded app to edge-to-edge

**Symptom:** black bands behind the status bar and gesture/nav bar, and `frust-insets` reporting a
zeroed `view_padding` — on Android 14 (API 34) and below only (API 35+ enforces edge-to-edge
regardless of the app's theme).

**Fix:** change `android:theme` on both the `<application>` and `<activity>` elements in
`android/app/src/main/AndroidManifest.xml` from the legacy `@android:style/Theme.NoTitleBar` to
`@android:style/Theme.Material.NoActionBar` (the current scaffold template's theme).

Rebuilding against the current embedding already fixes the bars and the insets even on the legacy
theme — `FrustActivity.onCreate` adds `FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS` itself on API < 35 (see
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s cross-cutting host-signal flow) — but it keeps
logging one `frust`-tagged warning naming this section until the manifest theme actually moves.

**Verify** (against a debug APK — `frust-insets` logs only under `cfg!(debug_assertions)`, at Info
since the Android logger caps below that):

```bash
adb logcat -s frust               # frust-insets view_padding ... t=<nonzero> ... b=<nonzero> ...
adb shell dumpsys window windows  # the app's window fl= carries DRAWS_SYSTEM_BAR_BACKGROUNDS
```

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
- **Bar region (Android):** confirm no black band shows behind the status bar or gesture/nav bar
  in either orientation — `scripts/testing/android-smoke.sh` is the automated equivalent of this
  leg (see [TESTING.md](TESTING.md) § Android Emulator GPU Lab).
- **iPad (iPadOS 26+):** run the playground on the iOS 27.0 iPad Simulator with a local, uncommitted
  opaque-surface flip (translucent Mode B rendered black on earlier Simulator runs and no longer
  reproduces; the opaque flip stays the deterministic recipe): set
  `translucentSurface` to `false` in `examples/playground/ios/Runner/SceneDelegate.swift`. Open
  Responsive: in a windowed scene the corner-insets line shows a non-zero TL (TR under RTL) and the
  home bar's brand slot sits clear of the window control; full screen reads zeros.

## Clipboard manual test (Android + iOS)

A device gate for the two mobile clipboard routes, against an installed app (`frust run -d
<device>`) with a `TextInput` on screen. **Not run yet on either platform** — no clipboard behaviour
has been observed on a physical Android device or on an iPhone, so nothing below is known to pass;
record the outcome here once a run happens.

The two platforms are gated separately because they take opposite selection-toolbar policies: the
Android shell leaves the framework's own toolbar in place and drains the tree's clipboard slots
itself, while the iOS shell declares the native policy and presents UIKit's own edit menu at the
anchor the focused field publishes (a paste is exempt from the iOS 14+ banner and the iOS 16+
permission alert only while it is system-initiated).

- **Android — one toolbar, not two:** long-press a word; the framework's toolbar appears over the
  selection and no second, native `ActionMode` bar competes with it or steals the gesture.
- **Android — round trip:** Copy from that bar, paste into another app, then copy in that app and
  Paste back into the field. One insertion each way, content identical.
- **Android — the IME's own verbs:** drive Copy/Paste from Gboard's clipboard chip / its own paste
  action rather than the framework bar; both must reach the focused field.
- **Android — hardware chords:** with a physical keyboard attached, `Ctrl`+`C`/`X`/`V`/`A` act on
  the focused field, and with no field focused they fall through untouched (the chords are
  intercepted only while an editable holds focus).
- **Android — empty clipboard:** with nothing on the clipboard, Paste inserts nothing and clears no
  selection.
- **Android — a URI-backed clip:** copy a *photo or file* in another app (not text), focus a field
  and paste. It resolves off the UI thread, so watch for two things: the app must not freeze while
  the source app answers, and the text must land in the field you asked from. Then repeat and switch
  to a different field while it is still resolving — the paste must be discarded, not delivered to
  the field you moved to.
- **iOS — the system menu:** long-press or double-tap a word; UIKit's own edit menu appears at the
  selection and the framework floats no toolbar of its own. The menu offers Copy/Cut/Paste/Select
  All and nothing else, and offers only the verbs the field allows.
- **iOS — the exemption, which is the whole point of this route:** paste from that menu and from a
  hardware `Cmd`+`V`. Expect **no** "pasted from" banner and **no** per-app paste-permission alert;
  either one appearing means the pasteboard was read outside the system-initiated path.
- **iOS — Paste is offered only when there is something to paste:** with no string on the
  pasteboard, the item is absent from the menu.
- **iOS — hardware chords, with NO menu on screen:** tap a field to focus it, do not long-press,
  and press `Cmd`+`V` on a hardware keyboard. It must paste. Then `Cmd`+`A`, `Cmd`+`C`, `Cmd`+`X`.
  Run this leg *before* any leg that opens the menu: the verbs are published as a level by a focused
  field, and a bug that gated them on a menu being open passes every menu-first test and fails only
  this one. `Cmd`+`V` is also one of just two paste routes iOS exempts from its per-app permission
  alert, so this leg doubles as the check that no alert appears.
- **iOS — hardware chords with the menu up:** the same four chords match the menu's own verbs.
- **iOS — the menu goes away:** collapsing the selection, blurring the field, or any gesture that
  withdraws the field's request (a scroll, a text change) puts it away; it never stands over a
  field that no longer has a selection.
- **iOS 15 (separate device):** the menu comes up through the older `UIMenuController` route rather
  than `UIEditMenuInteraction` — run the legs above again on a 15 device if one is available.
- **Both — obscured fields:** a field built `obscured(true)` offers no Copy/Cut anywhere (bar, menu,
  or assistive-technology action) and puts nothing on the host clipboard.

## Pinch / pan-zoom manual test (Android + iOS + desktop)

A device/emulator and desktop gate for the multi-contact pointer routing, touch ABI change, and
desktop scale-gesture mapping (see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s Touch
contacts and resampling / Desktop scale gestures), using `examples/playground`'s Graph page (the
"Graph" section — a `pan_zoom`-wrapped `canvas`) as the vehicle. **iOS: passed 2026-10-02**
on an iPhone SE (iOS 26.7, human-run, all legs) and the iOS Simulator (iOS 26.2, single-contact
legs only; pinch not run), against `8a263d79` built with Xcode 27.0 — per-leg results in
`examples/playground/README.md`'s gate checklist. The iPhone SE re-ran the pinch legs at
`f47a9828` (after the multi-contact scroll veto) and passed: a pinch that starts on a node keeps
the gesture through vertical travel, and a one-finger drag still scrolls the page. **Android: passed
2026-10-02** on a Pixel 5 (Android 14; human-run pinch legs, `adb`-driven single-contact legs;
Xiaomi 12 not run) — same checklist. **Desktop: not run yet.**

- **Android (Pixel 5, Xiaomi 12):** two-finger pinch zooms the graph about the gesture's midpoint
  and drag-pans it with one finger; after lifting to one finger mid-gesture, panning continues with
  no jump.
- **Android — single-finger regression:** on an unrelated screen with an ordinary tap target and a
  `ScrollView`, confirm tap and single-finger scroll are unaffected by the touch-ABI change
  (`nativeOnTouch`'s added `pointerId` argument) — both devices.
- **iOS (device or Simulator):** a two-finger pinch zooms the graph about the gesture's midpoint; a
  single-finger drag pans it; a plain tap elsewhere is unaffected. Xcode 27 ships no Simulator
  app, so Option-drag is unavailable; `idb` drives single contacts only, and on Xcode 27 it needs
  `DEVELOPER_DIR` pointed at a copy whose `Library/PrivateFrameworks` links
  `SharedFrameworks/SimulatorKit.framework`. Pinch therefore needs a physical device.
- **Desktop (Linux, macOS, Windows):** ctrl+wheel (Linux/Windows) or ⌘+wheel (macOS) zooms the
  graph about the cursor; a plain unmodified wheel still scrolls where applicable; drag-pans with
  the primary button. macOS additionally: a two-finger trackpad pinch zooms the graph (winit's
  `PinchGesture`); Linux/Windows have no trackpad-pinch source yet (`desktop-pinch-linux-windows-unavailable`
  in [LIMITATIONS.md](LIMITATIONS.md)) — ctrl/⌘+wheel is the only route there.

## Per-OS desktop shell gate (macOS + Windows + Linux)

A hardware gate for the native integration the shared winit core cannot cover, run against an app
configured with an app name, reverse-DNS id, window icon and menu spec. macOS, Windows, and
Linux/X11 are closed (Windows 2026-08-19, `examples/shadcn-demo` on MSVC; Linux 2026-09-13 on
X11); Wayland remains owed (see [LIMITATIONS.md](LIMITATIONS.md)
`desktop-shells-runtime-unverified`). Per host:

- **macOS:** menu bar shows the app-named application menu; ⌘Q and the menu Quit both exit;
  Hide/Show All work; closing with `quit_on_last_window_closed = false` hides the window and a
  Dock click from the *inactive* state brings it back; the zero-config preview is unchanged.
- **Windows:** titlebar and taskbar icons; taskbar grouping under the configured AppUserModelID;
  titlebar brightness follows an app-forced theme flip; the native menu bar and each declared
  accelerator activate their item.
- **Linux:** `WM_CLASS` matches the app id and the `.desktop` pairing via `StartupWMClass`, plus
  the X11 window icon — done on X11; the Wayland `app_id` pairing is still owed.

## Desktop window geometry (macOS + Windows + Linux)

`DesktopConfig`'s geometry fields — `initial_size`, `min_size`, `max_size` (logical pixels) and the
opt-in `persist_window_state` — shape the shared core's window
([SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) § Configuration vocabulary); `min_size`/`max_size`
are the window's resize limits. Creation geometry resolves lowest to highest precedence:

1. the shell default, 800×600;
2. `initial_size` (an invalid size is ignored);
3. the saved state (size, position, maximized), only when persistence is on;
4. `FRUST_WINDOW_SIZE` (size only) and `FRUST_WINDOW_MAXIMIZED` (maximized only; `0` un-maximizes
   a saved maximized state) — [DEVELOPMENT.md](DEVELOPMENT.md) § Instrumentation;
5. the extension's pre-create hook, which may override any attribute.

The state file is `<data_dir>/frust/window_state_<app_stem>.txt`: versioned `key=value` lines
(`version=1`, `width`, `height`, `maximized`, optional `x`/`y`/`monitor`). Another version or a
malformed line is ignored; a failed read logs at debug, the first failed write per process at warn
and later ones at debug, and none fails startup. A run with a valid `FRUST_WINDOW_SIZE` or any
`FRUST_WINDOW_MAXIMIZED` set reads and applies the saved state but never writes it; window creation
logs `frust-shell-desktop window persist=on`, `off (knob)` or (not opted in) `off` once. Saves are
debounced 500 ms after the last resize or move, with a final save on close request and on exit. The
normal (un-maximized) geometry is seeded from the creation geometry and tracked on every
un-maximized change, so a window maximized before its first save still records it; a minimized or
fullscreen window records nothing, and fullscreen is never restored.
A restored state is fitted to the connected monitors first: the position is dropped (the OS places
the window) when the saved monitor is gone or the rectangle overlaps no monitor, the size is fitted
to that monitor (the primary once the position is dropped), then `max_size` and `min_size` apply in
that order, so the minimum wins. Wayland reports no position, so only size and maximized state
restore there.

Gate, against `examples/playground` (480×320 minimum, persistence on; its README's gate table is
the record):

- **Linux — passed** (2026-10-18, linux-native rig): resize and move saved after the debounce, a
  300×200 request held at the 480×320 minimum, close wrote the final state, relaunch restored size and position.
- **macOS — owed:** relaunch restore and the minimum size.
- **Windows — passed** (2026-10-18, dell_mini_pc over RDP): relaunch restored a resized and moved
  window exactly, a corner drag stopped at the 480×320 minimum, a position edited to 5000,5000
  relaunched on-screen, a maximized close relaunched maximized. Open: after a maximize the saved
  normal geometry is the maximized rect, not the last un-maximized one (action item).
- **Any OS — owed (manual):** multi-monitor hot-unplug while running (saved monitor disconnected
  before close).

The same README table records the Linux re-run of the Shortcuts page's F2, `?`, single-Escape and
press-focus legs (2026-10-18) and still owes them as part of the macOS and Windows Shortcuts rows.

### Decorations and window commands

API (all desktop-only; `examples/custom-titlebar` is the worked app): `DesktopConfig::with_decorations`
(`Decorations::{Full, None, NoTitleBar}`) and `with_title_buttons(TitleButtons::new(leading,
band_height))` (macOS `NoTitleBar` only); `frust::push_window_command` with `WindowCommand`
(`BeginMove`, `BeginResize(ResizeEdge)`, `SetMaximized`, `ToggleMaximized`, `Minimize`,
`SetFullscreen`, `RequestClose`); the `WindowState` context (`maximized`, `fullscreen`, `focused`,
`minimized`); and the `window_drag_region` widget, which turns a primary press its child ignored
into `BeginMove` (a second one into `ToggleMaximized`) or, with `.resize(edge)`, `BeginResize`.

- **Creation-time only:** `decorations` and `title_buttons` are read when the window is built;
  there is no runtime setter (`desktop-decorations-creation-time-only`).
- **The app owns the controls:** under `None`/`NoTitleBar` it must draw at least a drag region and a
  close control, or the window can be neither moved nor closed. A refused drag logs once per kind.
- **Drain points:** after every input dispatch (macOS starts a move drag only from the event being
  handled, so a move pushed outside one is ignored) and once per frame, top of redraw, for pushes
  from timers and tasks. `RequestClose` takes the OS close-button path (`on_close_requested`).
- **Push contract:** one desktop window, no window id; push from UI-thread contexts. A push does
  not wake the loop: it runs at the end of the current dispatch or the next frame. Off desktop
  nothing drains the queue (`desktop-window-commands-off-desktop-noop`).
- **Per OS:** macOS `BeginResize` is a no-op (`desktop-macos-begin-resize-unsupported`; hide resize
  strips there and while maximized or fullscreen) and the traffic-light placement is best effort
  (`desktop-title-buttons-macos-best-effort`); Windows adds the DWM shadow to an undecorated window
  and warns once about a menu bar on one (`desktop-windows-menu-bar-undecorated-unverified`); Linux
  uses X11 Motif hints or Wayland client-side mode.
- **`WindowState.minimized` is unreliable** on Wayland (always `false`: the compositor reports no
  minimization) and on macOS relies on the `Occluded` event.

Gates, against `examples/custom-titlebar` (each gate card flips its bullet):

- **macOS — passed 2026-10-24** (macOS 27, Apple M4, 2x built-in plus an external display
  switched to 1x): under `NoTitleBar` the traffic lights sit at leading 16 / band 40
  (pixel-measured) after edge resize, two fullscreen round-trips, zoom and a 2x→1x→2x display
  move, and AppKit shows its own buttons while fullscreen. Drag from the bar or the title works; a
  press-release does not move the window; double-click zooms to the visible frame and restores;
  controls in the bar never start a drag. AppKit edge resize, the focus readout and close
  (`quit_on_last_window_closed(false)` hides, ⌘Q quits) all pass, and so does the playground row.
  `None`: no frame, shadow present; drag, double-click and edge resize pass. An undecorated
  window reads `maximized` from its frame, because winit's style-mask probe cancels AppKit's
  resize.
- **Windows — passed 2026-10-24** (Windows 11, 96 dpi): no caption; DWM shadow and rounded
  corners; drag and Aero Snap (the OS move loop); double-click maximize/restore (80–250 ms
  press to press); all eight strips; maximize to the work area, minimize, fullscreen round-trip,
  close and the focus readout pass; playground Window row passes.
  A menu bar on an undecorated window is installed but not drawn
  (`desktop-windows-menu-bar-undecorated-unverified`).
- **Linux X11 — passed 2026-10-24** (KWin, Xorg, NVIDIA T400): Motif hints remove the frame; drag
  (`_NET_WM_MOVERESIZE`), double-click maximize/restore, all eight strips, maximize to the work area,
  minimize, fullscreen round-trip, close and the focus readout act; no shadow is drawn for the
  frameless window. A press released within a few ms can leave KWin in a sticky move (synthetic
  input only). Playground Window row passes.
- **Linux Wayland — passed 2026-10-24** (nested KWin 6.7): no frame; drag (`xdg_toplevel.move`),
  double-click, five strips (`xdg_toplevel.resize`), maximize, minimize, fullscreen, close and
  focus act; `minimized` stays `false` (`desktop-linux-wayland-minimized-unreported`). Playground
  Window row passes. GNOME not run.

## Browser manual gate (web)

A gate for `crates/frust-shell-web`, run against `examples/web-gallery` per its own README's
Build/Serve sections (`cargo build --release --target wasm32-unknown-unknown`, then
`wasm-bindgen --target web ...` and `wasm-opt -O` with the named feature list, served by
anything that sends `.wasm` as `application/wasm`) — the README has the full recipe, including
the feature flags (never `--all-features`, which on binaryen 131+ emits an import kind browsers
refuse) and the ship-the-optimized-artifact copy step; do not restate it here. No automated
pixel gate exists for this surface — a person must look.

**Where this gate actually stands:** it has been run on Safari only, and only partway through
the list. The Chrome legs are unconfirmed, and Firefox is not installed on the development
machine, so its legs cannot be run there at all. Treat every unticked box below as unobserved
rather than as passing-by-analogy with a browser that was checked. Check:

- [ ] Chrome, default (WebGPU): pointer, wheel, touch-drag scroll, and keyboard text entry
      (typing into the index page's filter field) all reach the app; theme flips live with the
      app already running and no reload; resize and DPR 2 track live and stay pixel-correct; the
      index page's ticking counter advances with zero pointer/keyboard events (proves a signal
      write alone drives the repaint, not a poll loop).
- [ ] Chrome, forced to WebGL2 (`?arm=webgl` — Chrome's `--disable-features=WebGPU` flag does
      **not** reach this fallback on this stack, since `requestAdapter` fails outright instead of
      falling through to GL, so the query param is the only way; per the README's own record of
      that finding): shapes, layout, backgrounds, theme colours, text, gradients, blur and layers
      all stay pixel-correct — the atlas two-layer floor (see [LIMITATIONS.md](LIMITATIONS.md)
      `engine-webgl2-atlas-target`) is what makes glyphs render instead of painting as solid
      opaque boxes; a regression here means that floor broke.
- [ ] Safari 26 and Firefox on Windows/macOS (WebGPU): same checks as the Chrome/WebGPU row
      above. Firefox on Linux (its own WebGL2 fallback): same checks as the Chrome/WebGL2 row.
- [ ] IME baseline: focus the gallery search box, type ASCII, compose Japanese `nihongo` →
      にほんご → 日本語 (Enter commits once, Escape cancels); blurring the field removes
      `#frust-ime-overlay` from the DOM.
- [ ] Safari predictive commit: compose CJK text and let Safari's predictive text commit it
      instead of Enter — expect exactly one commit and no lost or duplicated text (exercises the
      compose latch's empty-`compositionend` grace window, `PendingEmptyEnd`).
- [ ] Password field: focus a field publishing `ImeContentType::Password` (an obscured
      `TextInput`) — expect the overlay element to be `type="password"` and the on-screen
      keyboard to offer no suggestions; note whether CJK composition is available on it at all
      (it may not be — see [LIMITATIONS.md](LIMITATIONS.md) `web-ime-residual-gaps`). Then land
      focus on it by a signal rather than a tap (a button that focuses the field) — expect the
      element to be replaced by a `type="password"` one on the next frame, still focused.
- [ ] Clipboard, keyboard route: select text in the gallery's filter field, `Ctrl`/`Cmd`+`C`, and
      paste into another app — the text arrives exactly once. Then copy in that app and
      `Ctrl`/`Cmd`+`V` back into the field: exactly one insertion, and the `v` itself never typed
      (a paste gesture is withheld from the canvas re-dispatch; a copy/cut gesture keeps its key
      path and hands its text to the DOM callback instead). `Ctrl`/`Cmd`+`X` must never delete text
      it did not write out — after a cut, the removed text is on the host clipboard.
- [ ] Clipboard, no-DOM-event route: copy through the field's own selection toolbar, a gesture no
      `copy` event follows — the write goes out through `navigator.clipboard.writeText`, which is
      undefined outside a secure context, so serve over `localhost`/`https` for this leg and expect
      a plain-`http://` origin to refuse the copy rather than lose it silently.
- [ ] Clipboard, obscured field: with a field publishing `ImeContentType::Password` focused, copy
      and cut must leave the host clipboard untouched — the field refuses the verb, and the DOM
      callback refuses to derive a write of its own from a secret field, so neither the real text
      nor whatever the overlay element happens to hold can reach the clipboard.
- [ ] Android Chrome soft keyboard: with no hardware keyboard attached, type on the soft
      keyboard — expect each letter exactly once, never doubled (the `Unidentified`-keystroke
      carry through the `input` path); then press its Backspace — expect no deletion, which is
      the recorded gap; a deletion that does land is evidence to note against that gap.

## Version Pins

The pins this unit owns, under [DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy (pins are
LAW; re-run the row's tripwire after touching it, and never run a blind `cargo update`):

| Pin | Why | Tripwire |
|---|---|---|
| `muda 0.19` minor | `frust-shell-macos`'/`frust-shell-windows`'s native menu-bar bindings (`NSMenu`/`HMENU` built from `DesktopConfig::menu_spec`). Pins `objc2 0.6.1` + `objc2-app-kit 0.3.x`, unifying with this workspace's existing `objc2` family; muda's own `gtk`/`libxdo` deps live behind a Linux/BSD-only target table internally, so this pin resolves no GTK dependency on macOS/Windows — `frust-shell-linux` deliberately never depends on it | `cargo check --target aarch64-apple-darwin -p frust-shell-macos && cargo check --target x86_64-pc-windows-gnu -p frust-shell-windows`, plus `cargo tree -d` (watch for muda bumping its `objc2` lineage); on a bump, re-verify two Windows contracts this pin's internals carry (`frust-shell-windows/src/menu.rs`): submenu accelerators register into the root `HACCEL` only while the submenu is attached (attach-then-fill order), and the predefined quit stays unused (it calls `PostQuitMessage`, inert under winit) |
| `windows-sys 0.61` minor | `frust-shell-windows`'s Win32 bindings (`SetCurrentProcessExplicitAppUserModelID`, `TranslateAcceleratorW`/`HACCEL`/`MSG`). Resolved empirically, not guessed: muda 0.19.3's own `>=0.60, <=0.61` constraint picks `0.61.2`, the minor already dominant in this lockfile. No features declared at the workspace level — each consuming crate selects its own list (`frust-shell-windows/Cargo.toml`: `Win32_UI_Shell`, `Win32_UI_WindowsAndMessaging`) | `cargo check --target x86_64-pc-windows-gnu -p frust-shell-windows`; `cargo tree -d --target x86_64-pc-windows-msvc` — watch for a third `windows-sys` minor appearing beyond the pre-existing `0.52.0`/`0.61.2` pair |
| `wasm-bindgen =0.2.128` exact | The crate's own glue-generation ABI schema, which must equal the host `wasm-bindgen-cli` — a stricter contract than semver, so a mismatch fails at glue-generation time, not at `cargo check`. `crates/frust-shell-web`'s `[target.'cfg(target_arch = "wasm32")'.dependencies]` table is where the pin is anchored (a workspace row only takes effect where a member depends on it, and this crate is the browser tier's only entry point); `crates/frust`'s own wasm32 table re-exports it unconditionally for `web_app!`'s start shim. The exact pin forces `wasm-bindgen-futures`/`js-sys`/`web-sys` into lockstep below, since each declares its own exact `wasm-bindgen` requirement. `wasm-bindgen-macro-support`'s `syn` dependency resolves onto the same `syn 3.0.3` this lockfile already carries via `async-trait`/`bytemuck_derive` — no second `syn 3.x` identity | `cargo check --target wasm32-unknown-unknown -p frust-ui --tests` (docs/DEVELOPMENT.md), plus `examples/web-gallery`'s packaging step (`wasm-bindgen --target web ...`, its README's Build section) erroring on a CLI/crate schema mismatch; `cargo tree -i wasm-bindgen` — watch for a second `wasm-bindgen` identity or a `syn` split |
| `wasm-bindgen-futures 0.4.77` / `js-sys 0.3.104` / `web-sys 0.3.104` caret | Forced to lockstep by the `wasm-bindgen` exact pin above (this lockfile currently resolves `0.4.78`/`0.3.105`/`0.3.105`). `web-sys` features are declared per crate, not at the workspace level, and trimmed to what each caller actually touches — `frust-shell-web` takes `Window` (the `setTimeout` half of its surface-bring-up retry wait) plus the DOM rows its hidden-input IME overlay needs (`src/ime.rs`): document/element/input-element (`Document`/`Node`/`Element`/`EventTarget`/`HtmlElement`/`HtmlInputElement`), composition/input/keyboard events (`Event`/`CompositionEvent`/`InputEvent`/`KeyboardEvent`/`KeyboardEventInit`), style/rect (`CssStyleDeclaration`/`HtmlCanvasElement`/`DomRect`), and the clipboard route that rides that same element: `ClipboardEvent`/`DataTransfer` — what the overlay's `paste`/`copy`/`cut` listeners narrow their `Event` to, `getData("text/plain")` for a paste and `setData` for the synchronous write a `copy`/`cut` callback must finish before it returns (Safari honours a clipboard write from nowhere else) — plus `Clipboard`/`Navigator`, the asynchronous `navigator.clipboard` `writeText`/`readText` half, reached only for the slots the tree fills with no DOM event behind them (a toolbar tap, an app-driven copy); `crates/frust`'s own wasm32 table pulls no `web-sys`/`js-sys` row at all. `wasm-bindgen-futures` supplies `spawn_local` (no blocking executor exists on a target that must never block) and the `JsFuture` half of that same retry wait; `js-sys` supplies `Promise`, the object the `setTimeout` callback resolves | `cargo check --target wasm32-unknown-unknown -p frust-shell-web -p frust-gallery` (docs/DEVELOPMENT.md); `cargo tree -i js-sys -i web-sys -i wasm-bindgen-futures` — watch for a second minor entering the lockstep |
| `web-time 1.1.0` | `std::time::Instant::now()` panics on `wasm32-unknown-unknown` (no clock syscall on that target); `web-time` is the drop-in replacement winit 0.30.13 itself already resolves internally, so this pin introduces no new identity. Scope is every crate `frust-shell-web` compiles — `frust-shell-web`, `frust-shell-common`, `frust-text`, `frust-render`, `frust-engine`, `frust-gpu` declare it directly — and each reads time through `web_time::Instant` (`SystemTime` stays the std type); winit types `ControlFlow::WaitUntil` over it on this target, so anything else also mistypes that contract | `scripts/ci/wasm-time-check.sh` (clippy `disallowed-*` over the web graph; registry crates and feature-gated paths are `scripts/ci/web-smoke.sh`'s job); `cargo check --target wasm32-unknown-unknown -p frust-shell-web -p frust-gallery`; `cargo tree -i web-time` — expect those direct dependents plus winit's own entry |
| `console_log 1.0.0` / `console_error_panic_hook 0.1.7` caret | (this lockfile currently resolves `console_log 1.1.0` from the `1.0.0` caret; `console_error_panic_hook 0.1.7` exact-matches) stderr is a silent no-op in a browser. The facade's `web_app!` bootstrap (`frust::__web_bootstrap`, the start shim's first call) installs both unconditionally — `console_error_panic_hook::set_once()` and `console_log::init_with_level(log::Level::Warn)` — before `frust_shell_web::logging::install` runs. A second `install` call is treated as "someone got here first" and swallowed rather than erroring (`log` allows exactly one sink per process), so an app or the gallery's own `?log=` query param re-levelling the sink afterward is safe and idempotent | `cargo check --target wasm32-unknown-unknown -p frust-ui --tests`; `cargo test -p frust-shell-web` (the `logging` module's own unit tests) — watch for `frust-shell-web`'s `DEFAULT_LEVEL` (`Warn`) drifting from the facade bootstrap's own default |
| `wasm-bindgen-test =0.3.78` exact | `crates/frust-testing`'s `tests/wasm_goldens.rs`/`tests/wasm_binary_invariants.rs` browser harness (`wasm_bindgen_test_configure!(run_in_browser)`, `#[wasm_bindgen_test]`), declared under that crate's `[target.'cfg(target_arch = "wasm32")'.dev-dependencies]` behind its non-default `webgl` feature — crate-local, with no root `[workspace.dependencies]` row. Pinned exactly because it is one half of a two-part schema contract, not an ordinary semver dependency: `0.3.78` is the release paired with the `wasm-bindgen =0.2.128` exact pin above, and the host `wasm-bindgen-test-runner` (shipped by that same `wasm-bindgen-cli 0.2.128`) refuses a binary whose schema hash does not match. Bump it only together with the `wasm-bindgen` pin and the installed CLI, never on its own | `CHROMEDRIVER_REMOTE=http://localhost:9517 CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner cargo test -p frust-testing --target wasm32-unknown-unknown --features webgl --release --test wasm_goldens`; `cargo tree -i wasm-bindgen-test` — watch for a second identity or a drift from the `wasm-bindgen` pin's own version |

`arboard =3.6.1` is the other pin this unit consumes without owning: `frust-shell-desktop`'s
clipboard route and `frust-clipboard`'s desktop backend resolve one shared row, kept with the plugin
that owns it ([PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md)'s Version Pins) rather than split in
two here. That row carries a tripwire for each consumer and a bump must clear both — including this
unit's, since the two hold arboard differently (one long-lived instance on the shell's clipboard
worker thread against the plugin's per-call ones).

`objc2-app-kit 0.3` (the `NSApplication`/`NSResponder` slice `frust-shell-macos` names directly,
for the Dock-reopen observer, plus `NSWindow`/`NSButton`/`NSControl` for `title_buttons`) joins the objc2 pin family owned by
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
