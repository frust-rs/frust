# Lab 7 — Mobile Loops & the Frame Gate

**Concept:** Desktop waits for dirt; mobile vsync callbacks
(`Choreographer` / `CADisplayLink`) fire every tick no matter what. So the
mobile shells run the *inverse* battery strategy: a per-tick **frame gate**
decides `Run` or `Skip` from an OR-list of dirtiness signals, and a `Skip`
does zero rebuild/layout/paint/encode work. This lab reads the gate, then
flips its kill switch on a device and measures the difference.

Requires a physical device for the fun parts. In the engine era the iOS Simulator renders
correctly — `frust-engine` needs neither `COMPUTE_SHADERS` nor `INDIRECT_EXECUTION`
(`ENGINE_REQUIRED_DOWNLEVEL_FLAGS`, `crates/frust-render/src/tier.rs`, is deliberately empty), though
its Metal validation misreports its own uniform-buffer alignment (`docs/DEVELOPMENT.md` Known
Issues). An Apple-Silicon Android emulator's hardware GPU path segfaults inside its
gfxstream/MoltenVK Vulkan driver during adapter enumeration — `crates/frust-gpu/src/context.rs`'s
`is_android_emulator` detects it and strips the `DEBUG`/`VALIDATION` instance flags to keep bring-up
alive; a physical device, or booting with `-gpu swiftshader` (software Vulkan), sidesteps the crash
outright (`docs/DEVELOPMENT.md` Known Issues).

## Where it lives

| Thing | File | Anchor |
|---|---|---|
| `FrameGate::decide` — the OR-list | `crates/frust-shell-common/src/frame_gate.rs` | ≈509 |
| `FrameInputs` — all twelve signals | same | ≈216–330 |
| Kill switch `FRUST_NO_FRAME_GATE` | same | ≈89 (const), ≈436 (`FrameGate::new`) |
| Android entry: `nativeOnFrame` JNI → `app.frame()` | `crates/frust-shell-android/src/jni_glue.rs` → `.../app/frame.rs` | ≈1471 → ≈60 |
| Android gate consult | `crates/frust-shell-android/src/app/frame.rs` | ≈311 |
| Android **layout skip within a Run** (`needs_layout \|\| force_layout`) | same | ≈276 (`force_layout`), ≈422–428 |
| iOS entry: `frust_render_frame` (from `CADisplayLink`, ns timestamp) | `crates/frust-shell-ios/src/lib.rs` → `ffi_glue.rs` → `app/frame.rs` | ≈202 → ≈1249 → ≈48 |
| iOS gate consult (early-return skips the whole frame) | `crates/frust-shell-ios/src/app/frame.rs` | ≈260 |
| iOS `FrameTime::from_nanos(timestamp_ns)` | same | ≈255 |
| `AppTree` type erasure (how a non-generic FFI handle drives any app) | `crates/frust-shell-common/src/app_tree.rs` | ≈1–30 |
| `PointerResampler` + `FRUST_NO_RESAMPLE` | `crates/frust-shell-common/src/resample.rs` | ≈54, ≈125 |

The twelve `FrameInputs` fields (read them with the doc comments —
`frame_gate.rs` ≈216–330): `signals_dirty`, `events_since_last_frame`,
`pointer_capture_active`, `focus_or_ime_changed`, `last_needs_frame`,
`last_needs_frame_paced_only`, `change_flags_pending`, `deferred_callbacks_pending`,
`theme_or_appearance_changed`, `surface_changed_or_resized`, `a11y_action_performed`,
`resumed_recently`. Any `true` ⇒ `Run` — except `focus_or_ime_changed`, which is an **edge**
(focus/IME session moved since the last tick), not a level: it forces one frame per transition
but, unlike every other field, does not also disqualify pacing, so a steady focus session can
still throttle to a paced loop's cadence (a blinking caret, say). The convention behind the rest
(`docs/CODE_STANDARDS.md`): **a signal with no precise source defaults to
must-run** — over-running wastes a frame; over-skipping drops real work.

Precision worth keeping: **both** platforms gate whole frames identically.
The platform *difference* is inside a `Run`: Android additionally skips the
layout pass unless `needs_layout || force_layout` (≈422–428); iOS currently
relayouts on every `Run` (the finer skip isn't wired there yet). Don't
conflate the two skips.

## Experiments

### 7.1 — Watch the gate think (device)

```bash
cd examples/huddle
FRUST_TRACE=1 frust run -d <device-id>      # or: frust run --define FRUST_TRACE=1
```

Leave the app idle: the periodic `frust-perf frame` summaries show
`skipped=` climbing — those are gate `Skip`s counted by `FrameStats`.
Interact: skips pause (input, capture, animations all force `Run`). Now the
control run:

```bash
FRUST_TRACE=1 FRUST_NO_FRAME_GATE=1 frust run -d <device-id>
```

Every tick runs. Compare idle `total_p50_ms` and skipped counts between the
two runs — you're measuring what the gate saves. (This is also the first
diagnostic dial for any "UI stuck on mobile" report: if the bug vanishes
under `FRUST_NO_FRAME_GATE=1`, some `FrameInputs` signal is under-reporting.)

### 7.2 — Starve the gate on purpose (the untracked-read trap)

The nastiest real bug class this gate creates: a signal read via
`get_untracked()` inside `build` never subscribes, so a later write never
sets `signals_dirty`, so the gate happily skips forever — a *frozen UI*,
not a stale value. Reproduce it in a scratch huddle/`frust_bench` component:
change one `get()` powering visible text to `get_untracked()`, run on
device, trigger the write from a timer. The UI updates only when you touch
the screen (input forces a `Run`). Revert, and you'll never mis-diagnose
this again. Desktop note: the same bug manifests there too (no redraw
requested) — the mobile gate just makes it stark.

### 7.3 — Feel the resampler

`PointerResampler` interpolates buffered touch moves to the frame boundary
(Down/Up/Cancel pass through losslessly). On a 120Hz-capable device, scroll
a huddle list normally, then with the kill switch:

```bash
FRUST_NO_RESAMPLE=1 frust run -d <device-id>
```

Judder during slow drags is the raw, off-phase touch delivery the resampler
exists to hide. Then read `resample.rs` ≈125–156 — it's pure logic, fully
unit-tested on desktop.

### 7.4 — Trace one frame across the FFI

Pick Android. Read, in order, with the files open side by side:
Kotlin `Choreographer` callback (generated app / huddle's `android/` dir) →
`nativeOnFrame` (`jni_glue.rs` ≈1471) → `AndroidAppHandle::frame` (`app/frame.rs` ≈60) →
gate (≈311) → rebuild (≈394) → conditional layout (≈428) → paint (≈468) → encode/present (≈505).
It's the same chapter-3 loop wearing a JNI coat — the type-erased `AppTree`
(≈1–30 in `app_tree.rs`) is what lets one exported symbol drive *your*
`State` type.

## What to notice before moving on

- `FrameStats.skipped` ties the gate to chapter 8's measurements: skips are
  *recorded*, so benchmark idle numbers reflect gate behavior, not luck.
- The theme/inset contracts from chapter 6 (`ChangeFlags::LAYOUT` forced on
  theme/inset change) exist precisely so Android's layout-skip can be
  aggressive without going stale.
