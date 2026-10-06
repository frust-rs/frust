# playground

**Playground is the plugin-functionality, platform-views, responsiveness, and
general testing showcase. It is NOT a theme showcase (see
`examples/glyph-catalog`) and NOT a benchmark (see `benchmarks/`).**

Every section here exists to exercise a real OS-facing capability end to end —
on a real device, through the public app-facing surface only (`frust` plus
plugin crates, custom widgets via `frust::authoring`). Nothing in this app
demonstrates a design language; the Material 3 baseline it seeds is just a
neutral backdrop for the capabilities under test.

## What it shows

A Material `app_bar` (a plug brand mark, the title, and the
brightness / reduce-motion / animations toggles) sits above the
`pattern_switcher`-hosted section body, selecting between thirteen sections
through one of two responsive navigation shapes (`use_context::<WindowMetrics>()`
against a 600px width breakpoint — `pages::responsive`'s own named-breakpoint
convention, applied to the shell itself):

- **Narrow** (< 600px, e.g. a phone): a bottom `navigation_bar` shows the
  first five sections directly, plus a trailing "More" destination that opens
  an overlay listing every section.
- **Wide** (≥ 600px, e.g. a desktop window): a side rail lists all thirteen
  sections directly, in a column beside the page body — nothing is ever
  behind an overflow menu at this width.

The sections, in order:

1. **Platform Views** — the real `frust::platform_view` embedding contract: a
   300×400 `dev.frust.DemoStreamFactory` Mode B slot with chrome overlapping
   its edges (an alpha-over-alpha fringing probe), a self-update proof strip,
   dispose/create + `updateParams` + multi-slot stress toggles, and enough
   filler to scroll the slot fully off-screen and back (paint-time visible-rect
   culling).
2. **Camera** — the `frust-camera` device-gate vehicle: permission flow, a live
   Mode B preview slot, lens switching, still capture, an image-stream readout,
   and a two-mode barcode scan strip (policy consumer shape / hand-timed
   `decode_frame` measurement rig) plus torch.
3. **Responsive** — `frust::WindowMetrics` driving a **structural** layout
   switch between a single scrolling column and a two-pane master/detail split
   at a named breakpoint; also displays a live readout of window-control
   corner-insets (iPadOS 26+ windowed apps only; zero elsewhere) for gate
   verification.
4. **Terminal** — a checked-in byte stream replayed through a **real** VT
   emulator (the pinned `vt100` crate) into a batched, painted 80×45 character
   grid: five on-page profiles (idle / typing / build-log / htop / firehose),
   30 Hz app-side repaint coalescing (one signal write per tick, never one per
   chunk), width-derived cell geometry, and a live runs-per-frame readout.
5. **Keys** — an IME keystroke probe: a hand-rolled `View`/`Widget` pair that
   claims focus, publishes a sentinel `ImeState`, and recovers per-keystroke
   bytes by diffing every platform snapshot against that sentinel — with the
   derived stream and an event log rendered on screen (and logged; grep
   `playground keys`).
6. **i18n** — `frust-i18n`'s whole vertical slice: system locale detection,
   live language switching across the three shipped locales (English/German/
   Japanese, `locales/en|de|ja`), a CLDR plural (`cart-items`), a Fluent
   select expression (`theme-choice`), compile-checked typed-key call sites,
   and the ICU4X-backed decimal/currency/date/time formatting matrix
   (`frust_i18n::fmt`) exercised directly alongside the FTL messages.
7. **Video** — the `frust-video-player` device-gate vehicle, on Android, iOS
   **and** the macOS desktop shell: a source picker (a streamed https MP4, an
   HLS stream, and a bundled local asset), a real `video_view` slot in a
   fixed 320×180 box, and transport/scrubber/state chrome below it —
   play/pause, ±10s, rate, volume, loop, fit, and close.
8. **URL** — the `frust-url-launcher` device-gate page:
   `UrlLauncher::open_external` opening a valid `https` URL versus a rejected
   `javascript:` one, plus the `frustplay` custom-scheme return leg a
   round-trip device gate drives back in through `frust::deep_links`.
9. **Auth** — the `frust-auth-session` device-gate page (Android Custom Tabs /
   Apple `ASWebAuthenticationSession`): four buttons exercise a successful
   callback round trip, a user-cancelled session, a busy-session rejection,
   and a cookie-persistence check against committed static pages under
   `plugins/auth-session/gate`.
10. **DB** — the `frust-database` device-gate vehicle: opens
    `<filesDir>/databases/playground.db`, inserts, counts; proves
    `frust_paths::data_dir()` resolves on Android without any HOME/XDG env
    var. Gated 2026-09-26 on Pixel 5 / Android 14 (and Xiaomi 12 / Android 16,
    API 35 x86_64 emulator): `db ok rows=1` then `rows=2` at
    `/data/user/0/it.f0x.playground/files/databases/playground.db`, bare
    scaffold `MainActivity`, no env var set. A `frustplay://section/<label>`
    deep link (case-insensitive against `SECTION_LABELS`) routes straight to a
    section; an unrecognized `<label>` shows a toast instead of routing.
    Deliveries dedupe by `DeepLink::sequence`, not URL text, so two deliveries
    of an identical link each apply. A cold `am start -a
    android.intent.action.VIEW -d frustplay://section/db it.f0x.playground`
    lands on this section and auto-runs the smoke once — a debug-build-only
    behavior (a shipped release build never auto-runs a DB write from an
    external deep link) — logging `playground deep-link: section db -> 9`
    then `frust-database smoke: ok rows=N` — `scripts/testing/android-smoke.sh`
    passed all 5 assertions against the same Pixel 5. That same 2026-09-26 gate
    also exercised the Android edge-to-edge fix: on the Pixel 5 (gesture nav),
    the Mode A `material3-demo` sampled pure black in 165/168 status-bar and
    72/144 gesture-bar pixels with no `DRAWS_SYSTEM_BAR_BACKGROUNDS` flag before
    the fix; after, the window carries that flag, logcat shows `frust-insets
    view_padding l=0.0 t=49.5 r=0.0 b=24.0 … scale=2.75`, and both bands sample
    zero pure-black pixels. A legacy-theme APK still gets the flag/insets and
    logs the migration warning (see
    `docs/SHELLS_DEVELOPMENT.md`). Xiaomi 12 / Android 16 shows no regression
    (`t=39.3, b=0.0` — its gesture bar is hidden).
11. **Graph** — a `CanvasView`/`PanZoomView` widget demo, not a plugin gate: a
    toy node-and-edge graph (12 nodes, a spiral layout, 13 edges) painted by a
    `frust::canvas` closure (edges via `stroke_line`, nodes as
    `fill_rounded_rect` circles) hosted inside a `frust::pan_zoom` viewport.
    Drag pans, ctrl/⌘+wheel or trackpad-pinch (desktop) or a touch pinch
    zooms, both focal-point-correct; tapping a node selects it (highlighted on
    the next repaint via `CanvasView::repaint_key`) through `on_hit`/
    `on_pointer` rather than `on_tap`, since only the raw event carries the
    tap's position; a readout below shows the live scale/offset, and "Fit"
    drives the attached `PanZoomController` to frame the whole graph.
12. **Scroll** — a `ScrollController` bound to a keyed, variable-extent
    `ListView` (~300 rows, every 7th one taller than the rest): "Jump to 0" /
    "Jump to end" move instantly, "Animate to 50%" eases to half of the
    current max offset, and "Item #150 (Start)" / "Item #42 (Center)" /
    "Animated #280 (End)" scroll straight to a named row by key (the last one
    animated). A live offset/max-offset readout below the list updates on
    every drag, fling, or programmatic move via `ScrollController::on_change`.
    Dragging the list while an animated move is in flight interrupts it —
    user input always wins; under reduce-motion (or with animations off)
    every animated move collapses to an instant jump instead.
13. **Drag** — three demos sharing one `DragCoordinator` (`frust::drag`):
    a three-column kanban board (`Todo` / `Doing` / `Done`) whose columns
    are scrollable drop targets with edge auto-scroll near the top/bottom
    and a visible highlight while hovered, cards dragged by pointer
    (desktop) or long-press (touch), `Escape` cancels; a ten-row
    keyboard-reorderable list (`Enter`/Space lifts a focused row, arrow
    keys cycle the drop gap, `Enter`/Space drops) with a live
    `from -> to` readout; and a desktop-only OS file-drop zone listing
    dropped files by name (never their contents).

The Terminal fixtures live in [`fixtures/terminal/`](fixtures/terminal/README.md)
(bytes + the deterministic generator that produced them, embedded with
`include_bytes!`); `examples/rows_profile.rs` is a host-only analysis tool that
replays them offline and reports the changed-rows-per-generation distribution
(`cargo run --example rows_profile`).

### Mode B

The generated Android/iOS glue turns `translucentSurface` **ON**
process-wide — playground is the framework's Mode B testbed. The shell
therefore paints an explicit surface-colored `AppBackground` as the root
stack's bottom-most layer; the Platform Views slot is the one deliberate hole
punched through it. See `src/pages/platform_views.rs`'s module docs.

## Running

```bash
# Desktop preview (no native host exists there — platform-view slots render a
# debug fill in debug builds and nothing in release):
cargo run

# On a device (Android / iOS) — where every section above is actually
# meaningful:
frust run -d <device-id>
```

This is a standalone package (its own `[workspace]` root and `Cargo.lock`,
excluded from the Frust root workspace — the same shape as
`examples/glyph-catalog`), so build/test/gate it from **this directory**, never
with `-p` from the repo root:

```bash
cargo build --locked && cargo test \
  && cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

## Coverage

`tests/smoke.rs` is the automated half: it drives the real `RenderRoot`
rebuild→layout→paint seam headless (no GPU, no window) over every section at
two viewport sizes and both brightnesses, and mounts the whole shell through
its root `Component`. It proves structural soundness and theme-resolution
liveness — it cannot prove any of the on-device behaviour these pages exist
for (a real native view compositing, a real camera session delivering frames,
real platform controls responding to touch). That remains a human-run device
gate.

## Gate checklist (human-run, hardware-owed)

The headless suite above proves the Graph page builds/lays out/paints and
that the node-hit/selection math is correct; it cannot drive a real pointer,
trackpad, or touchscreen. Rows marked OWED have not been run:

| Gate | Platform | Status |
|------|----------|--------|
| Desktop ctrl/⌘+wheel zoom + drag pan | Linux (local) | OWED |
| Touch pinch zoom + drag pan | Pixel 5 (redfin), Android 14, 2026-10-02 | PASS (all legs below) |
| Touch pinch zoom + drag pan, plus tap/scroll/focus regression | iPhone SE (iPhone12,8), iOS 26.7, 2026-10-02 | PASS (all legs below; human-run) |
| Same legs, single contact only | iOS Simulator (iPhone 17), iOS 26.2, 2026-10-02 | PASS except pinch: NOT RUN |
| Scroll: every button (Jump to 0 / Jump to end / Animate to 50% / #150 Start / #42 Center / Animated #280 End) | Linux (local) | OWED |
| Scroll: a touch drag mid-`animate_to`/animated `scroll_to_item` interrupts the move | Android device | OWED |
| Drag: desktop pointer drag (kanban card between columns, edge auto-scroll, highlight) | Linux desktop (X11, NVIDIA T400), 2026-10-06 | PASS (card lands in Doing/Done incl. a drag with a vertical leg; ghost, source dim, column highlight; edge auto-scroll not exercisable — the demo columns never overflow) |
| Drag: Escape cancels a live pointer drag | Linux desktop (X11, NVIDIA T400), 2026-10-06 | PASS (ghost + highlight clear with the button still held) |
| Drag: keyboard reorder (lift/cycle/drop) on the reorderable list | Linux desktop (X11, NVIDIA T400), 2026-10-06 | PASS (Enter / ArrowDown ×4 / Enter → "Last move: 0 → 2") |
| Drag: OS file drop onto the file-drop zone | Linux desktop (X11, NVIDIA T400), 2026-10-06 | PASS (XDND v5 source; hover highlight, two names listed, hover+leave clears) |
| Drag: desktop pointer drag / Esc cancel / keyboard reorder / OS file drop | macOS | OWED |
| Drag: desktop pointer drag / Esc cancel / keyboard reorder / OS file drop | Windows | OWED (binary builds and launches on the Windows 11 rig; the leg needs an interactive desktop session) |
| Drag: long-press drag (kanban card between columns) | Pixel 5 (redfin), Android 14, 2026-10-06 | PASS (long-press lift, ghost, column highlight; drop lands both directions incl. a ~300 px vertical leg; reorder onto a gap → "Last move: 0 -> 2"; short tap / quick swipe unaffected) |

### Drag gate notes

- **Android injection**: `adb shell input motionevent DOWN x y`, hold ≥ 0.9 s, `MOVE` steps, `UP`; `input draganddrop` needs ≥ 15 s so the 500 ms long-press arms before the slop is crossed; the Done column is off-screen at 1080 px (known layout limit).
- **Linux**: the playground runs on the host and presents on the container desktop, so dropped files must exist on the HOST filesystem; a drop aimed at a reorder row rather than its 8 px gap cancels by design.
- **winit 0.30 on X11**: silently discards a whole drop whose uri-list contains a path that does not canonicalize on the app's filesystem while still answering XdndStatus/XdndFinished accepted — the app sees no event at all.

iOS legs (Phase A touch-ABI gate, build at `8a263d79`). The Simulator legs were
driven by `idb` with screenshots. Pinch could not run there: Xcode 27 ships no
Simulator app for Option-drag, and `idb` injects one contact only.

| Leg | iPhone SE | Simulator |
|-----|-----------|-----------|
| Platform: tap "Bump params" | PASS | PASS (count 0 to 1) |
| Platform: one-finger page scroll | PASS | PASS |
| Keys: tap the zone, keyboard focus | PASS (typing registered) | PASS (`active=1`; `idb` text did not register) |
| Graph: tap a node selects and highlights it | PASS | PASS (`selected: N0`) |
| Graph: one-finger drag on empty space pans | PASS | PASS (offset moved by the drag) |
| Graph: vertical drag starting on a node | PASS (no anomaly reported) | No pan, by design: the canvas handles the press, so the child owns the gesture (same at `5c078079`, after the veto fix) |
| Graph: two-finger pinch zooms about the fingers; lifting one keeps panning | PASS | NOT RUN |
| Graph: Fit reframes the whole graph | PASS | PASS |

Android legs, Pixel 5 (redfin), Android 14. The first legs below ran at build
`8a263d79`; the node-first pinch leg, exercising the live
multi-contact veto fix, ran at build `2e084ef5`.

| Leg | Pixel 5 |
|-----|---------|
| Gradle/Kotlin build + install | PASS |
| Deep link to Graph | PASS |
| Fit reframes the graph | PASS |
| Graph: tap a node selects and highlights it | PASS |
| Graph: one-finger drag on empty space pans, page does not scroll | PASS |
| Graph: vertical drag starting on a node: selects, no pan by design, page does not scroll | PASS |
| Graph: two-finger pinch zooms about the fingers and a two-finger drag pans | PASS (human-run) |
| Graph: pinch with the first finger on a node, after the veto fix (build `2e084ef5`) | PASS (human-run, no page scroll) |

On a phone-width viewport the Graph page opens at 1.0x over an empty corner of
the 1400x1000 content, so press Fit first.

Multi-contact veto re-check (pinch survives the page scroll), iPhone SE, iOS 26.7,
2026-10-02, build `f47a9828`, human-run. The Graph page overflows the SE screen,
so the page body scrolls vertically. `pinch_detector` has no consumer in
playground, so only `pan_zoom`'s half of the fix runs on a device here.

| Leg | Result |
|-----|--------|
| A: pinch starting with one finger on a node, both fingers travelling vertically, zooms throughout and the page does not scroll | PASS |
| B: the same pinch starting on empty canvas | PASS |
| C: one finger on a node, vertical drag, scrolls the page | PASS |
| D: right after a pinch ends, leg C scrolls the page again | PASS |
| E: button tap, page scroll, Keys focus at this build | PASS |
