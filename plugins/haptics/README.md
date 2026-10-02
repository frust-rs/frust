# frust-haptics

A minimal, general-purpose **haptic-feedback** plugin for frust apps — Android
`Vibrator`/`VibrationEffect` over plain JNI, iOS `UISelectionFeedbackGenerator`/
`UIImpactFeedbackGenerator`/`UINotificationFeedbackGenerator` via `objc2-ui-kit`,
desktop (macOS/Linux/Windows) unavailable by design.

**Platform support:** Android and iOS. On macOS, Linux and Windows, and on any other target, every call returns `HapticsError::NotAvailable(Unavailability::UnsupportedPlatform)`.

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

---

## 1. Add the dependency

```toml
# app Cargo.toml — [dependencies]
frust-haptics = { path = "<frust>/plugins/haptics" }  # or the crates.io release, e.g. version = "0.5"
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote.

```rust
use frust_haptics::{Haptics, HapticEffect};

// Fire-and-forget: ignore the Result in the common case (a haptic tick is
// cosmetic feedback, never load-bearing UI state).
let _ = Haptics::perform(HapticEffect::SelectionClick);
```

### Android: the `VIBRATE` permission arrives via Add Plugin

Unlike `frust-clipboard`, this plugin *does* need one Android manifest
addition: the `android.permission.VIBRATE` `<uses-permission>` (a **normal**
permission — granted automatically at install, no runtime prompt). The
`frust` TUI's **Add Plugin** dialog (or `frust-drive`'s plugin registry
applied manually) inserts it into your generated project's
`android/app/src/main/AndroidManifest.xml` idempotently. No Gradle module, no
iOS plist key, no Swift package — reading/writing haptic feedback needs
neither on either mobile platform.

---

## 2. `HapticEffect` — the vocabulary

A closed, seven-value vocabulary — the lowest common denominator both mobile
platforms ship a first-class API for:

| `HapticEffect` | Android (`android.os.VibrationEffect`) | iOS (`objc2_ui_kit`) |
|---|---|---|
| `SelectionClick` | `EFFECT_TICK` (API 29+; falls back to `EFFECT_CLICK` at API 30+ when the platform reports TICK unsupported); a short `createOneShot` amplitude below API 29 | `UISelectionFeedbackGenerator.selectionChanged()` |
| `ImpactLight` | `EFFECT_CLICK` (API 29+); `createOneShot` fallback below API 29 | `UIImpactFeedbackGenerator(style: .light)` |
| `ImpactMedium` | `createOneShot` amplitude (no predefined "medium" effect exists at any API level) | `UIImpactFeedbackGenerator(style: .medium)` |
| `ImpactHeavy` | `EFFECT_HEAVY_CLICK` (API 29+); `createOneShot` fallback below API 29 | `UIImpactFeedbackGenerator(style: .heavy)` |
| `Success` | A short composed `createWaveform` pattern | `UINotificationFeedbackGenerator(.success)` |
| `Warning` | A short composed `createWaveform` pattern | `UINotificationFeedbackGenerator(.warning)` |
| `Error` | A short composed `createWaveform` pattern | `UINotificationFeedbackGenerator(.error)` |

### Android API-level fallbacks

`VibrationEffect.createPredefined(int)` (the `EFFECT_TICK`/`EFFECT_CLICK`/
`EFFECT_HEAVY_CLICK` constants) requires API 29. This workspace's Android
floor is API 26 (`frust-embedding`'s `minSdk`), so `SelectionClick`/
`ImpactLight`/`ImpactHeavy` fall back to `VibrationEffect.createOneShot(long,
int)` (API 26) below API 29 — a duration/amplitude pair standing in for the
missing predefined effect, tuned so the three impact intensities stay
distinguishable from each other on hardware with amplitude control (and
degrade to one fixed strength, never an error, on hardware without it).
`ImpactMedium` and the three notification-style effects use `createOneShot`/
`createWaveform` (both API 26) unconditionally — Android has no built-in
"medium click", so there was never a predefined effect to prefer there in the
first place.

`SelectionClick` additionally queries `Vibrator.areEffectsSupported(int...)`
(API 30) before committing to `EFFECT_TICK`, falling back to `EFFECT_CLICK`
when the platform doesn't report it definitely supported. Below API 30 that
query doesn't exist, so this backend always tries `EFFECT_TICK` directly —
worst case a no-op single tick on hardware that lacks it, never a crash.

---

## 3. Platform caveats

- **No vibrator hardware is a silent no-op, not an error.** `Vibrator.hasVibrator()`
  is checked first; a device with no vibration motor (a small class of
  tablets/TVs, and every emulator without one configured) reports `Ok(())`
  having done nothing further.
- **Desktop is unavailable by design, in v1.** macOS trackpads expose
  `NSHapticFeedbackManager` and some Windows/Linux hardware exposes force
  feedback, but none of it maps onto this crate's mobile-first vocabulary — a
  future desktop backend would be additive, never a breaking change, since
  every call site already treats `Haptics::perform` as fire-and-forget.
- **iOS dispatches asynchronously to the main thread.** Every
  `UI*FeedbackGenerator` class is UIKit's `MainThreadOnly`; `Haptics::perform`
  returns immediately (`Ok(())`) after handing the construct-and-trigger
  sequence to `dispatch_get_main_queue()` — it never blocks the caller, even
  when called from a background thread, and there is nothing to report back
  on this path once dispatched.
- **Android's `Vibrator.vibrate(VibrationEffect)` call is synchronous and
  quick** (the vibration itself plays asynchronously on the vibrator HAL, not
  inside the call) — this backend calls it directly from the caller's thread,
  no `frust_reactive::spawn_blocking` needed.

---

## 4. Fire-and-forget contract

`Haptics::perform` returns a `Result<(), HapticsError>` for the caller that
wants to distinguish *why* nothing happened (an old scaffold predating
`nativeInitPlatform`, a genuinely unavailable platform, a backend failure) —
but a haptic tick is cosmetic feedback, never load-bearing UI state, so the
overwhelmingly common call site ignores it entirely (`let _ =
Haptics::perform(...)`).

---

## 5. Caveats

- **v1 is a fixed, seven-value vocabulary.** `HapticEffect` is not
  `#[non_exhaustive]` — it is a considered, closed set matching the
  Android/iOS lowest common denominator, not a placeholder pending growth.
  Widening it (a custom-intensity impact, a raw waveform escape hatch) is a
  deliberate future decision, not accidental drift.
- **`frust create --overwrite` is a non-issue here**, aside from the one
  manifest permission — Add Plugin's idempotent apply handles a repeat run
  the same way every other registry entry does.

## Links and license

Documentation: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`)
or the MIT license (`LICENSE-MIT`) at your option (SPDX: `MIT OR Apache-2.0`).
Both license files are included beside this README.
