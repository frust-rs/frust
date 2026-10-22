# native-widgets-demo

**The `frust-native-widgets` showcase and device-gate vehicle, shaped like the
GitHub app: native chrome (the top bar, the tab bar and a detail toolbar), native
main controls, and a frust-drawn body.** Every real OS control
(`android.widget` / UIKit / AppKit) renders from pure Rust beside its
frust-drawn peer, under one `Theme` and driven by one signal. It is not the
general plugin testing ground (see `examples/playground`) and not a
design-system catalog (see `examples/glyph-catalog`).

The app seeds the Glyph baseline theme, so the native controls' typeface ladder
has a design system's bundled faces to adopt (`NativeTypefaces`, ladder L3).
"Toggle brightness" in the bar's overflow menu re-themes the native controls,
the chrome and the frust-drawn peers together.

## What it shows

The principle: **native chrome + native main controls + frust body.** One
`NativeWidgetsDemoApp` whose `build` is a single `native_scaffold` tree over a
reach-gated `indexed_stack` of four tabs, each its own `navigator`:

- The **chrome** is the scaffold's: a top bar (avatar at Home's root, the tab's
  title, a back item and a title/subtitle on a pushed page, pull-down menus and a
  search action), a bottom tab bar (Dialogs carries an unseen-outcome badge)
  and, on a Home detail page, a toolbar with a primary Comment pill. It is never
  a native-widget slot.
- The **main controls** are native widgets in the body: the Favorites card's
  button on Home, every control on Controls.
- The **body** is frust: lists, cards, text, the detail page's table and composer.

At a navigator's root the bar shows the tab's title. A Home row pushes a detail
page shaped like a pull request (a table, bullets, code chips, a composer that
the toolbar's Comment toggles, lifted onto the keyboard by the scaffold). The
fallback chrome draws its tab icons untinted, so they read mid-grey on purpose.

## Tabs

One module per tab under `src/pages/` (shared helpers, the pair row, the demo
image and `AnchorProbe` live in `src/pages/common.rs`). Every tab header names
the platform and reads `live_slot_count()` back against the tab's documented
**native slots at rest** (`AT_REST_SLOTS` in `common.rs`), with a "Re-read" chip
(a new slot is created after the paint that published it, so the first frame can
still count the previous tab's slots).

| Tab | Module | Sections | Slots at rest |
|---|---|---|---|
| Home | `home.rs`, `detail.rs` | My Work rows pushing the detail page, a Favorites card with a native button, Shortcuts | 1 |
| Controls | `controls_tab.rs` | **Controls** (`controls.rs`): the six base builders, each REAL control beside its frust peer, with the native → signal → frust readout and the REJECT write-back affordance. **New** (`new_controls.rs`): spinner, segmented control, stepper, date picker. **Composite** (`composite.rs`): the plugin's `DemoCard`, one `NativeComponent`, one slot, three native children, behind a default-off toggle | 10 on iOS/macOS (6 + 4), 9 on Android (6 + 3), 8 on a desktop preview (6 + 2) |
| Dialogs | `dialogs.rs` | **Alerts** (`alerts.rs`): `show_native_alert_into`, an anchored action sheet, the busy refusal, programmatic dismiss. **Sheet** (`sheet.rs`): `show_native_sheet_into` with detents. **Menus**: what the bar's pull-downs and the detail toolbar are, with the last chrome event read back | 0 |
| Lab | `lab.rs` | The at-rest table, then **Stress** (`stress.rs`), **Sandwich** (`sandwich.rs`), **macOS** (`macos.rs`), **TabBar** (`tab_bar.rs`) | 5 on iOS, 4 elsewhere |

The slot counts above are what the headless suite asserts on the target it runs
on (`tests/smoke.rs`): the segmented control and stepper add a slot only where
the platform has a native arm (segmented: iOS, macOS, Android; stepper: iOS,
macOS; the tab bar: iOS only), and a refusal banner publishes none. The
Controls tab's momentary segmented pair sits behind a default-off checkbox, so
the at-rest counts are unchanged by it. The Lab's count is the macOS image pair
(2) plus the sandwich (2) plus the bare `UITabBar` (iOS only); the stress
cycler (+6) and 50-slot toggle (+50) are default-off. Counts are with the
**whole body visible**: a slot is published only while painted, so a body
clipped by a short window publishes fewer.

The **chrome publishes zero slots on every platform.** **Hidden tabs:** every
tab stays laid out but only the showing tab paints, so from the first frame
after a switch the published set is the new tab's alone; the host then disposes
the unpublished slots at its own pace (the live count may lag a switch) and
re-creates them when the tab returns.

The Lab is the gate harness: **Stress** proves the registry's teardown-retire
path (a mount/unmount cycler over its own six-control group, x5 and x100 at
150 ms per half-step, and a 50-slot `native_label` toggle); **Sandwich** paints
frust over and under native views (an Interleaved paint-order checkbox, and the
`FRUST_OVERLAYS=1` notes on opaque surfaces); **macOS** states which AppKit
class each builder mounts and the documented AppKit gaps, with a live
`Cover`/`Contain` image pair (macOS letterboxes both; the section is a reference
elsewhere, and on macOS the fallback chrome is what draws the bars); **TabBar**
is the bare embedded `UITabBar` — compare it with the app's real tab bar at the
bottom of the window.

### Per-platform expectations

**iOS / iPadOS:** the chrome is native UIKit (a navigation bar and `UITabBar`
with Liquid Glass on iOS 26). **Android:** the chrome is Material 3, which needs
a Material theme: the Activity's theme must descend from
`Theme.Material3.DayNight.NoActionBar` (a non-Material theme is refused with one
warning and frust-drawn bars, no crash). The Android arms also carry these facts:
the native sheet is a `BottomSheetDialog`, whose detents are limited (Medium is
half height, Large expanded, a custom detent is a half ratio, and only one half
ratio is draggable at a time); `native_segmented` is a
`MaterialButtonToggleGroup`; the detail toolbar is a floating action button above
the bottom navigation; and the bottom navigation hides while the keyboard is up
(the scaffold lifts the body so a composer sits on the keyboard). **macOS and
desktop:** the chrome is the plugin's frust-drawn fallback (a title bar, a
toolbar row and a tab row), and every native control is AppKit on macOS or an
empty slot on a Linux/Windows preview.

✓ = the real platform control/presentation; **banner** = the plugin's
frust-drawn refusal banner in place of a slot; **Unsupported** = the request
is refused `PresentError::Unsupported` and the page's status line says so;
**alert** = presented as an ordinary alert (style/anchor ignored).

| Family | iOS / iPadOS | Android | macOS | Linux/Windows preview |
|---|---|---|---|---|
| App chrome | ✓ UIKit bars | ✓ Material 3 bars (Material theme required) | fallback bars | fallback bars |
| Controls (six builders) | ✓ UIKit | ✓ `android.widget` | ✓ AppKit | empty slots (no host) |
| macOS image pair | ✓ (Cover crops) | ✓ (Cover crops) | ✓ (Cover letterboxes) | empty slots |
| New: spinner, date picker | ✓ | ✓ | ✓ | empty slots |
| New: segmented | ✓ | ✓ `MaterialButtonToggleGroup` | ✓ | banner |
| New: stepper | ✓ | banner | ✓ | banner |
| Alerts: alert | ✓ `UIAlertController` | ✓ `AlertDialog` | ✓ `NSAlert` window sheet | Unsupported |
| Alerts: action sheet | ✓ (iPad: popover at the anchor) | alert | alert | Unsupported |
| TabBar | ✓ bare `UITabBar` | banner | banner | banner |
| Sheet | ✓ `UISheetPresentationController` | ✓ `BottomSheetDialog` (limited detents) | Unsupported | Unsupported |
| Composite | ✓ | ✓ | ✓ | recorded stand-in, no view |
| Stress | ✓ | ✓ | ✓ | empty slots |

A Mode B iOS surface whose translucency the platform refused renders every
builder as a frust-drawn placeholder (no slot), so the at-rest counts read 0
there.

## Deep links

`native-widgets-demo://section/<label>` opens the tab an old section label now
lives on and scrolls to that section (case-insensitive): `Controls`, `New` and
`Composite` on Controls; `Alerts` and `Sheet` on Dialogs; `macOS`, `TabBar`,
`Stress` and `Sandwich` on Lab. The new `native-widgets-demo://tab/<slug>` opens
a tab without scrolling: `home`, `controls`, `dialogs`, `lab`. An unrecognized
`<label>` shows a toast instead — the raw label is untrusted external text, so
it is echoed into the toast only when it is ASCII alphanumerics/`-`, 32
characters or fewer (otherwise a fixed "Unknown section in deep link" message;
either way the raw label's first 128 characters still reach the log,
`{:?}`-escaped after the cut — never sliced inside a multi-byte character —
plus the original byte length). The toast queue is an append-only log the toast
host walks by index: nothing is dropped from the front (that would shift entries
under its cursor) and nothing is collapsed (a repeated message is a new request
the host must show again); growth is bounded at the source by rate-limiting
unknown-label toasts, the only toast an outside process can trigger, to at most
one per ~2 seconds. Deliveries dedupe by `DeepLink::sequence`, not URL text. On
Android:

```
adb shell am start -a android.intent.action.VIEW \
  -d native-widgets-demo://section/sheet it.f0x.native_widgets_demo
```

## Plugin wiring

`frust-native-widgets` is wired through `frust-drive`'s Add Plugin applier —
the same edits `frust tui`'s Add Plugin makes to a user's app: the Cargo path
dependency, the `:frust-native-widgets` include/projectDir/build-dir block in
`android/settings.gradle.kts`, and `implementation(project(..))` in
`android/app/build.gradle.kts`. iOS needs nothing beyond the Cargo row. The
plugin's non-default `demo-components` feature is hand-added on top (the
registry has no optional feature that produces it).

## Run

This is a standalone workspace excluded from the root graph — run everything
from this directory.

- **macOS desktop:** `cargo run` — with `DYLD_LIBRARY_PATH` unset, or image
  slots stay empty (`native-widgets-macos-imageio-dyld-shadow`). The desktop
  shell has no launch-time deep link, so debug builds read
  `FRUST_DEMO_SECTION=<label or tab slug>` (a section label such as `alerts`, or
  a tab slug such as `lab`; case-insensitive) once at startup to open that page —
  e.g. `env -u DYLD_LIBRARY_PATH FRUST_DEMO_SECTION=alerts cargo run`. Unset,
  empty or unknown values are ignored; release builds never read it.
- **Android:** `frust run -d <device>` (a device or emulator from `frust
  devices`; the platform follows the device), or `frust build apk --profile`
  for an installable profile APK. The app's theme must be Material 3 (see
  above).
- **iOS / iPadOS:** `frust run -d <device>` (a connected device, or a
  simulator — see below).

### iOS surface mode: `FRUST_DEMO_OPAQUE`

`ios/Runner/SceneDelegate.swift` runs the app in Mode B (translucent surface)
by default, like `examples/playground`. The iOS Simulator renders a frust
surface only in Mode A, so for simulator runs set `FRUST_DEMO_OPAQUE=1` in the
Xcode scheme's environment (Product > Scheme > Edit Scheme > Run > Arguments >
Environment Variables); any value selects Mode A. Leave it unset on a device.

From the command line, `frust run -d <sim>` never reads the Xcode scheme, so
use `SIMCTL_CHILD_FRUST_DEMO_OPAQUE=1 frust run -d <sim>` instead — `xcrun
simctl launch` forwards every `SIMCTL_CHILD_<NAME>` variable in its own
environment to the app as `<NAME>`.

## Tests

```
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

`tests/smoke.rs` mounts the four tab bodies and the full shell headless (no
GPU, no window), at a phone and a desktop size under both brightnesses. It
asserts that every tab paints, that text colours track brightness, that each
tab publishes exactly its documented at-rest number of native slots when laid
out with its whole body visible (a tall viewport) for the target the test runs
on, that the full shell mounts with the fallback chrome (title, four tab labels
and no native slot), that the toolbar row shows on a Home detail page and not on
Home's root, that the composer row is the detail column's last child, that a
`section/sheet` deep link selects Dialogs and sets the anchor, and that
`AnchorProbe` records the window rect its child painted at. It proves
structural soundness only; how the native controls and the chrome actually look
and behave is the on-device gate below.

## Assets

`assets/demo-image.png` (128×128, 2103 bytes) is the image every image demo and
the tab bar's byte icon show — a byte-for-byte copy of
`examples/playground/assets/logo.png`, the bytes the playground's native image
pair used, so the image pair stays comparable with earlier gates. It is frust
project artwork (committed with the playground's scaffold) covered by the
repository licence, MIT OR Apache-2.0; no third-party licence applies and the
playground carried no separate notice. `assets/logo.png` is this app's own icon from
`frust create` (`frust.toml`'s `icon`), not used by any page.

`assets/icons/` holds the four tab icons (`home`, `controls`, `dialogs`, `lab`)
and the Home avatar, generated by `scripts/gen_tab_icons.py`. Apple targets show
SF Symbols instead of the PNGs; every other platform shows the PNGs.

The `[profile.release]` and cold-set `[profile.release.package.*]` blocks in
`Cargo.toml` are byte-synced with the root `Cargo.toml`;
`crates/frust-cli/tests/profile_sync.rs` (run by `cargo test -p frust-cli` at the
repo root) fails if they drift.

## Device status

Phase 7 gates of the Apple-expansion plan, build `2b3ae58b`, 2026-09-30/10-01; the overlay rows are later device runs, dated in the row.

| Platform | Device / OS | Result |
|----------|-------------|--------|
| macOS | Apple M4, macOS 27.0, debug build | All 11 lines pass (Ed-driven). The first `cargo run` launch painted nothing until relaunched — filed. |
| iOS | iPhone SE (2nd gen), iOS 26.7, debug build | Every line passes with the platform-view seam off, except VoiceOver (skipped late in the session; to be run in the next Apple gate round). The black screen seen as shipped with `synchronizesPresentWithPlatformViews` armed no longer reproduces: the sandwich page presents with present-sync armed on an iPhone 13 mini (iOS 26.7.1, 2026-10-21, row below); the playground renders under the same seam. |
| iPadOS | iPad Pro 13-inch (M5) simulator, iPadOS 26.2, built with `xcodebuild -sdk iphonesimulator` and launched with `SIMCTL_CHILD_FRUST_DEMO_OPAQUE=1 xcrun simctl launch` (because `frust run -d <booted simulator>` currently matches the simulator twice, filed as a CLI minor) | All 7 legs pass (Ed-driven). Light host appearance shows the theme toggle out of sync with the shell-applied appearance — filed. |
| Android | Pixel 5, Android 14 (UP1A.231105.001.B2), profile build | Alerts, refusal banners, Unsupported sheet and the stress cycler pass. **No native control is composited** (Mode A host, the scaffold default; filed, major), and an active accessibility client plus a rotation aborts the process inside `accesskit_android` (filed, critical). A page's native slots linger for about a minute after navigating away (filed as a minor); the Lab's at-rest table states the hidden-tab rule, and on the rewritten demo the same lag applies to a tab you leave. |
| iOS (overlays) | iPhone 13 mini, iOS 26.7.1, debug build with `FRUST_OVERLAYS=1` and `FRUST_DEMO_OPAQUE=1`, driven by an XCUITest runner; iPhone 17 Pro simulator, iOS 26.2, with Main Thread Checker loaded (2026-10-21) | Sandwich page in Mode A: toolbar, popover and badge drawn over the native field and slider; Interleaved toggled 11 times quickly, home→foreground and landscape→portrait: every disposed overlay view is removed once Rust hands its layer back, no withdraw waits on the render thread, live native slots return to 2; present skew avg 0.19 ms / max 0.41 ms with present-sync armed; Main Thread Checker reports nothing for overlay attach or resize; 0 wgpu errors. Interleaved keeps the card under the slider. |
| iOS (app chrome) | iPhone SE (2nd gen), iOS 26.7.1, debug build of a throwaway gate app (this demo inside `native_scaffold`, four tabs), Mode B, driven by an XCUITest runner (2026-10-22) | Tab bar (Liquid Glass, icons, badge), navigation bar (avatar, title, subtitle, `+` pull-down menu, search), select and reselect, back, alerts and sheets over the chrome, rotation and a running-app deep link all pass; native body controls are recreated after a tab round trip. The body scrolls under both glass bars (insets top 74, bottom 83) and the keyboard arrives as an IME inset (260) and clears on dismiss. **Scroll mirror fails in both modes**: the hidden scroll view tracks the body offset exactly, yet UIKit neither minimizes the tab bar nor draws a scroll-edge effect, and the appearance flip is invisible because iOS 26 bars are transparent by default, so content runs under the title unblurred (filed). Profile build not run. Follow-ups the same day: a frust-drawn edge fade replaced the scroll mirror, and tab switches were made native and instant without a black frame. |
| Android (app chrome) | Xiaomi 12, Android 16, and Pixel 5, Android 14: debug build of a throwaway gate app (this demo inside `native_scaffold`, four tabs, bitmap icons), Mode B; release APK minified with R8 (2026-10-22) | Material 3 toolbar (title, subtitle, actions) and bottom navigation (icons, badge) on both phones; frust gets top/bottom insets 0 under the bars and the landscape cutout; tab, `+` PopupMenu and search events arrive; the bar lifts on scroll; rotation keeps the tab and app state; the app's own dark/light flips the bars while the system stays dark; the keyboard hides the bottom navigation and the scaffold lifts its body so a composer sits on the keyboard (also on iPhone). A non-Material theme is refused with one warning and frust-drawn bars, no crash. The minified release APK binds the natives; it is 1.44 MB larger than the Phase 0 build (~0.6 MB Material resources, 0.17 MB dex, 0.14 MB arm64 native). An Activity relaunch (font-scale change) re-installs the bars once. The known accesskit rotation abort still reproduces with an accessibility client attached. |
| macOS (overlays) | Apple M4 MacBook (2x Retina) plus an external display set to a 1x mode, macOS 27.0, debug build with `FRUST_OVERLAYS=1` (2026-10-21) | Interleaved toggled 11 times quickly: every withdrawn overlay is removed, including the ones withdrawn mid-frame, with no UI-thread wait; live resize keeps the overlays aligned (present skew avg 0.02 ms / max 0.23 ms with two overlays); moving the window 2x→1x→2x keeps the overlay text as sharp as the main surface's at each scale; 0 wgpu errors. |
| Android (overlays) | OnePlus 9, Android 15, debug build with `FRUST_OVERLAYS=1` (2026-10-16) | Sandwich page in Mode A: toolbar, popover and badge drawn over the native field and slider through overlay surfaces; Interleaved toggle on/off and home→foreground each followed by an idle wait show the over-paint (or its restoration) with no further input; slider drags pass through the overlay; present skew avg 0.43 ms / max 0.77 ms with two overlays; landscape and back intact; Mode B ignores the overlay ops and composites the same content on one surface; 0 wgpu errors. The card still covers the slider when interleaved (`android-overlays-cover-later-slots`). Re-run on a Pixel 5 (Android 14, 2026-10-21) after the overlay table became shared and withdraw stopped waiting on the render thread: Interleaved toggled 10 times quickly, then home→foreground, each with a 3 s idle: the over-paint is drawn or restored with no input, no overlay surface outlives its view (SurfaceFlinger lists only the live ones), no ANR, 0 wgpu errors. |
| d5-05 (rewritten demo) | iPhone SE (2nd gen), iOS 26.7.1, Mode B; Xiaomi 12, Android 16; Pixel 5, Android 14; macOS 27 `cargo run` — debug builds of this demo (2026-10-22) | iPhone: all four tabs, the detail push (back, title, subtitle, `…` menu), scrolling under the glass bars, alerts and the page sheet, rotation, the brightness toggle and the `section/` and `tab/` deep links (cold and running) pass. On a detail page the tab bar hides and the native toolbar (Info, Comment) takes the bottom; it hides again while the composer is open. Android: Material 3 bars with bitmap action icons, the FAB toolbar, the composer on the keyboard with the bottom navigation hidden, back to Home then exit, the bottom sheet (medium, `select_detent` to large, programmatic dismiss) and the native segmented control (three equal segments, taps reach the app) pass. macOS: the fallback bars draw bitmap icons and the first launch paints at once. Three gate findings were fixed and re-checked before sign-off: the toolbar sat behind the tab bar, the segmented control was built against a context without the Material theme, and the fallback bars drew SF Symbol names as letters. Stock-theme refusal not re-run (passed in the Android app-chrome row above). |

Lines and legs are the per-platform checklists of the Phase 7 gate cards (g7-01 macOS, g7-02 iPhone/iPad, g7-03 Android); each line is a recorded gate row.
