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
app unnavigable on Android and macOS. The eight sections:

1. **Controls** — the six base controls (button, label, switch, slider,
   progress, image), native beside frust-drawn.
2. **macOS** — the same controls as AppKit views.
3. **New** — spinner, segmented control, stepper, date picker.
4. **Alerts** — native alert and action sheet.
5. **TabBar** — the native tab bar (UITabBar on iOS/iPadOS).
6. **Sheet** — the native page sheet (UISheetPresentationController on
   iOS/iPadOS).
7. **Composite** — a native component subtree and its events.
8. **Stress** — a 50-slot stress harness.

Each section is currently a placeholder (its label and a one-line summary);
the catalog pages replace them one module at a time under `src/pages/`.

A `native-widgets-demo://section/<label>` deep link (case-insensitive against
`SECTION_LABELS`) routes straight to a section; an unrecognized `<label>` shows
a toast instead. Deliveries dedupe by `DeepLink::sequence`, not URL text. On
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

- **macOS desktop:** `cargo run`
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
no window) at a phone and a desktop size under both brightnesses. It proves
structural soundness only; how the native controls actually look and behave is
the on-device gate below.

The `[profile.release]` and cold-set `[profile.release.package.*]` blocks in
`Cargo.toml` are byte-synced with the root `Cargo.toml`;
`crates/frust-cli/tests/profile_sync.rs` (run by `cargo test -p frust-cli` at the
repo root) fails if they drift.

## Device status

<!-- Filled in by the on-device gate: device, OS, date, result per section. -->
