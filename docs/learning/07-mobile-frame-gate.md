# Lab 7 — Mobile Loops & the Frame Gate

**Concept:** Desktop waits for dirt; mobile vsync callbacks
(`Choreographer` / `CADisplayLink`) fire every tick no matter what. So the
mobile shells run the *inverse* battery strategy: a per-tick **frame gate**
decides `Run` or `Skip` from an OR-list of dirtiness signals, and a `Skip`
does zero rebuild/layout/paint/encode work. This lab reads the gate, then
flips its kill switch on a device and measures the difference.

Requires a physical device for the fun parts (iOS Simulator can't render
under the vello 0.9 pin; Apple-Silicon Android emulators need
`-gpu swiftshader` — `docs/DEVELOPMENT.md` Known Issues).

## Where it lives

| Thing | File | Anchor |
|---|---|---|
| `FrameGate::decide` — the OR-list | `crates/frust-shell-common/src/frame_gate.rs` | ≈257–272 |
| `FrameInputs` — all ten signals | same | ≈109–157 |
| Kill switch `FRUST_NO_FRAME_GATE` | same | ≈63, 285–300 |
| Android entry: `nativeOnFrame` JNI → `app.frame()` | `crates/frust-shell-android/src/jni_glue.rs` ≈742 → `app.rs` ≈776 |
| Android gate consult | `crates/frust-shell-android/src/app.rs` | ≈903 |
| Android **layout skip within a Run** (`needs_layout \|\| force_layout`) | same | ≈885, 976–988 |
| iOS entry: `frust_render_frame` (from `CADisplayLink`, ns timestamp) | `crates/frust-shell-ios/src/lib.rs` ≈157 → `ffi_glue.rs` ≈410 → `app.rs` ≈642 |
| iOS gate consult (early-return skips the whole frame) | `crates/frust-shell-ios/src/app.rs` | ≈763 |
| iOS `FrameTime::from_nanos(timestamp_ns)` | same | ≈865 |
| `AppTree` type erasure (how a non-generic FFI handle drives any app) | `crates/frust-shell-common/src/app_tree.rs` | ≈1–28 |
| `PointerResampler` + `FRUST_NO_RESAMPLE` | `crates/frust-shell-common/src/resample.rs` | ≈54, 121–156 |

The ten `FrameInputs` fields (read them with the doc comments —
`frame_gate.rs` ≈109–157): `signals_dirty`, `events_since_last_frame`,
`pointer_capture_active`, `focus_or_ime_active`, `last_needs_frame`,
`change_flags_pending`, `theme_or_appearance_changed`,
`surface_changed_or_resized`, `a11y_action_performed`, `resumed_recently`.
Any `true` ⇒ `Run`. The convention behind them
(`docs/CODE_STANDARDS.md`): **a signal with no precise source defaults to
must-run** — over-running wastes a frame; over-skipping drops real work.

Precision worth keeping: **both** platforms gate whole frames identically.
The platform *difference* is inside a `Run`: Android additionally skips the
layout pass unless `needs_layout || force_layout` (≈976–988); iOS currently
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
exists to hide. Then read `resample.rs` ≈121–156 — it's pure logic, fully
unit-tested on desktop.

### 7.4 — Trace one frame across the FFI

Pick Android. Read, in order, with the files open side by side:
Kotlin `Choreographer` callback (generated app / huddle's `android/` dir) →
`nativeOnFrame` (`jni_glue.rs` ≈742) → `AppHandle::frame` (`app.rs` ≈776) →
gate (≈903) → rebuild → conditional layout (≈976) → paint → encode/present.
It's the same chapter-3 loop wearing a JNI coat — the type-erased `AppTree`
(≈1–28 in `app_tree.rs`) is what lets one exported symbol drive *your*
`State` type.

## What to notice before moving on

- `FrameStats.skipped` ties the gate to chapter 8's measurements: skips are
  *recorded*, so benchmark idle numbers reflect gate behavior, not luck.
- The theme/inset contracts from chapter 6 (`ChangeFlags::LAYOUT` forced on
  theme/inset change) exist precisely so Android's layout-skip can be
  aggressive without going stale.
