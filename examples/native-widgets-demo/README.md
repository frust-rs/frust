# native-widgets-demo

**The `frust-native-widgets` showcase and device-gate vehicle: one page per
native-widget family, every real OS control (`android.widget` / UIKit /
AppKit) rendered from pure Rust beside its frust-drawn peer, under one `Theme`
and driven by one signal.** It is not the general plugin testing ground (see
`examples/playground`) and not a design-system catalog (see
`examples/glyph-catalog`).

The app seeds the Glyph baseline theme, so the native controls' typeface ladder
has a design system's bundled faces to adopt (`NativeTypefaces`, ladder L3) —
the brightness toggle in the app bar re-themes the native controls and their
frust-drawn peers together.

## What it shows

A Glyph `app_bar` (brand mark, title, brightness toggle) sits above the
`pattern_switcher`-hosted section body, with a bottom section navigation of
frust-drawn baseline buttons (two rows of four) — deliberately not the native
tab bar, which off iOS/iPadOS renders as a refusal banner and would leave the
app unnavigable on Android and macOS.

## Pages

One module per section under `src/pages/` (shared chrome, the pair row, the
demo image and `AnchorProbe` live in `src/pages/common.rs`). Every page header
names the platform, reads `live_slot_count()` back against the page's
documented **native slots at rest** (the gate constant — `AT_REST_SLOTS` in
`common.rs`), and offers a "Re-read" chip (a new slot is created after the
paint that published it, so the first frame can still count the previous
page's slots).

| # | Label | Module | What it proves | Slots at rest |
|---|---|---|---|---|
| 1 | Controls | `controls.rs` | The six base builders (button, label, switch, slider, progress, image), each REAL control beside its frust-drawn peer in the same cell, theme and signal; the native → signal → frust readout (`Taps:`/`Switch:`/`Slider:`); the REJECT write-back affordance (a test-only refusal count in the accessibility label, so a refusal reaches the wire); a light/dark toggle re-theming both columns live. Each pair names its class per platform. | 6 |
| 2 | macOS | `macos.rs` | The AppKit arm: what each builder mounts on macOS, the documented AppKit gaps (with their `docs/LIMITATIONS.md` ids), and a live `Cover`/`Contain` image pair (iOS/Android crop `Cover`; macOS letterboxes both). A reference page off macOS. | 2 |
| 3 | New | `new_controls.rs` | Spinner (animating toggle) vs Glyph `dots_loader`; segmented (three segments, controlled, REFUSE toggle) vs Glyph `segmented_control`; stepper (range 0..=10, step 1/2, wraps) vs baseline `−`/`+` buttons; date picker (compact ↔ inline, bounded 2026–2027, always mounted through the same pair-row shape so the switch is an in-place style update, never a remount — the switch applies immediately on iOS/macOS, and is a no-op on Android until the picker remounts) vs a `Text` readout — Glyph has no stepper or date picker. | 4 on iOS/macOS, 2 on Android |
| 4 | Alerts | `alerts.rs` | `show_native_alert_into`: a three-role alert with a cancelable toggle; an action sheet anchored to its own button's painted window rect (`AnchorProbe`); two requests from one tap, the second refused `Busy`; a programmatic `present::dismiss` 2 s after presenting; the outcome readout. | 0 |
| 5 | TabBar | `tab_bar.rs` | A bare `native_tab_bar` at the page bottom (`.safe_area(false)` — the page is not docked to the window edge): a byte icon, an SF Symbol with a selected symbol, a badged item; selection drives a page-local index, not the app router (routing from here would navigate away from the control under test); a reselect counter; Glyph `tabs` on the same selection. | 1 on iOS, 0 elsewhere |
| 6 | Sheet | `sheet.rs` | `show_native_sheet_into`: medium + large with grabber, non-dismissible, a custom 0.4 detent, a programmatic run (expand to large after 1 s, dismiss 2 s later); the outcome and user-detent readouts. | 0 |
| 7 | Composite | `composite.rs` | The plugin's `DemoCard` — one `NativeComponent`, one slot, three native children — behind a default-off toggle; its Primary button bumps a counter through `.on_event`, and a "Rename" chip drives its update path. The FFI wall still applies (an app crate cannot implement `NativeComponent`). | 0 (1 with the card on) |
| 8 | Stress | `stress.rs` | The gate harness: the at-rest table for every page, a mount/unmount cycler over its own six-control group (x5 / x100 at 150 ms per half-step), and a 50-slot `native_label` stress toggle. | 0 (+6 cycling, +50 stress) |

### Per-platform expectations

✓ = the real platform control/presentation; **banner** = the plugin's
frust-drawn refusal banner in place of a slot; **Unsupported** = the request
is refused `PresentError::Unsupported` and the page's status line says so;
**alert** = presented as an ordinary alert (style/anchor ignored).

| Page / family | iOS / iPadOS | Android | macOS | Linux/Windows preview |
|---|---|---|---|---|
| Controls (six builders) | ✓ UIKit | ✓ `android.widget` | ✓ AppKit | empty slots (no host) |
| macOS image pair | ✓ (Cover crops) | ✓ (Cover crops) | ✓ (Cover letterboxes) | empty slots |
| New: spinner, date picker | ✓ | ✓ | ✓ | empty slots |
| New: segmented, stepper | ✓ | banner | ✓ | banner |
| Alerts: alert | ✓ `UIAlertController` | ✓ `AlertDialog` | ✓ `NSAlert` window sheet | Unsupported |
| Alerts: action sheet | ✓ (iPad: popover at the anchor) | alert | alert | Unsupported |
| TabBar | ✓ bare `UITabBar` | banner | banner | banner |
| Sheet | ✓ `UISheetPresentationController` | Unsupported | Unsupported | Unsupported |
| Composite | ✓ | ✓ | ✓ | recorded stand-in, no view |
| Stress | ✓ | ✓ | ✓ | empty slots |

A Mode B iOS surface whose translucency the platform refused renders every
builder as a frust-drawn placeholder (no slot), so the at-rest counts read 0
there.

A `native-widgets-demo://section/<label>` deep link (case-insensitive against
`SECTION_LABELS`) routes straight to a section; an unrecognized `<label>`
shows a toast instead — the raw label is untrusted external text, so it is
echoed into the toast only when it is ASCII alphanumerics/`-`, 32 characters
or fewer (otherwise a fixed "Unknown section in deep link" message; either
way the raw label still reaches the log, `{:?}`-escaped). The toast queue
holds at most 8 messages at once, oldest dropped first. Deliveries dedupe by
`DeepLink::sequence`, not URL text. On Android:

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
  `FRUST_DEMO_SECTION=<label>` (a `SECTION_LABELS` entry, case-insensitive)
  once at startup to open that section — e.g.
  `env -u DYLD_LIBRARY_PATH FRUST_DEMO_SECTION=alerts cargo run`. Unset,
  empty or unknown values are ignored; release builds never read it.
- **Android:** `frust run android` (device or emulator), or
  `frust build apk --profile` for an installable profile APK.
- **iOS / iPadOS:** `frust run ios` (a connected device, or a simulator — see
  below).

### iOS surface mode: `FRUST_DEMO_OPAQUE`

`ios/Runner/SceneDelegate.swift` runs the app in Mode B (translucent surface)
by default, like `examples/playground`. The iOS Simulator renders a frust
surface only in Mode A, so for simulator runs set `FRUST_DEMO_OPAQUE=1` in the
Xcode scheme's environment (Product > Scheme > Edit Scheme > Run > Arguments >
Environment Variables); any value selects Mode A. Leave it unset on a device.

## Tests

```
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

`tests/smoke.rs` mounts the full shell and every section page headless (no GPU,
no window) at a phone and a desktop size under both brightnesses, asserts each
page paints text and chrome, that each publishes exactly its documented at-rest
number of native slots for the target the test runs on, and that
`AnchorProbe` records the window rect its child painted at. It proves
structural soundness only; how the native controls actually look and behave is
the on-device gate below.

## Assets

`assets/demo-image.png` (128×128, 2103 bytes) is the image every image demo and
the tab bar's byte icon show — a byte-for-byte copy of
`examples/playground/assets/logo.png`, the bytes the playground's native image
pair used, so the image pair stays comparable with earlier gates. It is frust
project artwork (committed with the playground's scaffold) covered by the
repository licence, MIT OR Apache-2.0; no third-party licence applies and the
playground carried no separate notice. `assets/logo.png` is this app's own icon from
`frust create` (`frust.toml`'s `icon`), not used by any page.

The `[profile.release]` and cold-set `[profile.release.package.*]` blocks in
`Cargo.toml` are byte-synced with the root `Cargo.toml`;
`crates/frust-cli/tests/profile_sync.rs` (run by `cargo test -p frust-cli` at the
repo root) fails if they drift.

## Device status

<!-- Filled in by the on-device gate: device, OS, date, result per section. -->
