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
`pattern_switcher`-hosted section body, with a bottom `navigation_bar`
selecting between ten sections:

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
   at a named breakpoint.
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
