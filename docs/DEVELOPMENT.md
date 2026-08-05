# Frust - Development Guide

The canonical test-tier, golden-image, headless GPU, and Android emulator runbook is
`docs/TESTING.md`. This guide retains the concise build/test commands and platform
prerequisites used during ordinary development.

## Prerequisites

- Rust 1.88+ (workspace `rust-version`), edition 2024.
- A real Metal or Vulkan adapter for the GPU smoke gate (`frust-render`'s `--ignored`
  test). Headless Vulkan needs no display server; see `docs/TESTING.md`. Other hosts can
  build and run the non-GPU suite.
- **Android** (only needed for `frust run`/`build`/`create`'s Android output): `rustup
  target add aarch64-linux-android`; `cargo install cargo-ndk`; JDK 17+ on `JAVA_HOME`
  (Android Studio's bundled JBR auto-detected on macOS); `ANDROID_HOME`/`ANDROID_SDK_ROOT`
  and `ANDROID_NDK_HOME` set — `frust doctor` checks all of these.
- **iOS** (only needed for `frust run`/`build`/`create`'s iOS output; macOS host only):
  Xcode 26+ resolved by `xcode-select -p`; `rustup target add aarch64-apple-ios-sim
  aarch64-apple-ios`. A booted Simulator suffices for a debug `frust run`; a signed build
  needs a codesigning identity (`frust` auto-detects `DEVELOPMENT_TEAM`, or set
  `FRUST_IOS_TEAM`/`[ios] team`), and a physical iPhone run also needs iOS 17+,
  unlocked/paired/trusted with Developer Mode on (`frust doctor` checks Rust targets on
  macOS hosts only).
- **clean-signals-rs**: not required to build. `clean-signals` is git+rev-pinned to its
  public repo (see *Version-Pin Policy*), consumed by `examples/huddle`,
  `plugins/clean-signals-frust`, and `templates/app`'s clean-signals scaffold variant.
  Cloning it as a sibling directory (`../clean-signals-rs`) is still useful for local
  iteration on `clean-signals` itself, via a `[patch]` override in the consuming
  workspace — not needed for ordinary development.
- No Docker, CI config, or `.env` setup exists in this repo yet.

## Build

```bash
cargo build --workspace --locked
```

`--locked` must always pass — it is part of the verify gate below and is how
manifest/lockfile drift is caught (see *Version-Pin Policy*). **Dev-profile shader-stack
overrides.** The root `Cargo.toml`, `templates/app/Cargo.toml.tmpl`, and
`examples/huddle/Cargo.toml` each carry a `[profile.dev.package.*]` override (`opt-level
= 2`) for the shader/render crates: debug-profile (`opt-level = 0`) shader
compilation/translation is slow enough on mobile CPUs to trip the iOS launch watchdog.
The three manifests are hand-synced (`cargo test -p frust-cli --test profile_sync` is
the tripwire); a `[profile.dev.package."*"]` wildcard (`opt-level = 1`) widens every
other dependency's debug optimization the same way.

**Release-profile hardening.** `[profile.release]` (root, template, huddle) sets `lto =
"fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "abort"`, at the default
`opt-level = 3` — chosen over `"s"`/`"z"` after a smaller-opt-level win didn't clear a
5% bar against a render-stack CPU-perf carve-out (measured via `scripts/size-report.sh`
below). **Design-system feature gating.** `frust` default-enables
`glyph`/`material`/`cupertino` (each `frust-widgets` catalog) plus `glyph-fonts`;
`default-features = false` drops all four. Cargo silently ignores `default-features =
false` on an *inherited* dependency (defaults stay on) — set it on the
`[workspace.dependencies]` entry instead (root `Cargo.toml`'s
`frust-widgets`/`frust-theme` edges); `frust = { workspace = true, default-features =
false }` in a member is a hard Cargo error, so `examples/no-catalogs` uses a plain
`path` dependency instead. **Widget-authoring test fixtures.** `frust-widgets`'
non-default `test-support` feature compiles in the crate's GPU-free container-widget
fixtures (`frust_widgets::test_support`), so a design system built outside this crate
can test its own containers the same way; off by default in a normal app build.

## Run

```bash
# Huddle: the repo's sole example (own [workspace]/Cargo.lock, excluded
# from the root workspace — run/gate from its own directory, not `-p`).
(cd examples/huddle && cargo run)
```

`(cd examples/huddle && cargo run)` is the manual visual gate for
rendering/interaction/theme/navigation/text-input changes (no automated pixel-diff test
yet, so a person must look at the window) — including the check that a background-thread
wake (e.g. a timer-driven completion) renders content with zero mouse movement, not only
on an input-triggered redraw. `examples/huddle`'s own verify gate (`cargo test` plus
clippy, from its own directory) is part of *Test* below.

**Glyph design-language gate.** `examples/huddle`'s appearance settings expose a
four-way `System`/`Material3`/`Cupertino`/`Glyph` toggle; a Glyph dark+light check
(desktop, Android, iOS) plus a reduced-motion pass round out the manual gate above — no
automated check exists for either. `examples/huddle` seeds its own Glyph first-launch
default via `frust::glyph_theme::install()` (`app!`'s setup block); a shell with no
design system installed falls back to `Theme::neutral()`.

`examples/huddle` additionally builds and runs on Android and iOS, from its own
directory (its own `frust.toml`, package `it.f0x.huddle`):

```bash
cd examples/huddle
/path/to/frust build apk --debug   # debug APK via Gradle + cargo-ndk
frust run -d <device-id>           # build/install/launch/stream (Android or iOS sim/device)
```

The template `frust create` scaffolds is its own demo (a notes app,
`templates/app/src/lib.rs.tmpl`) with no example counterpart to run directly in this
repo; check scaffold changes via *Template development* below or the scaffold end-to-end
test in *Test*.

In a generated project, `frust run [-d <device>] [--release|--profile] [--flavor
<name>]` builds and launches on a connected Android device/emulator (preflight →
variant-aware `gradlew assemble<Flavor><Mode>` via cargo-ndk → `adb install`/`launch` →
streamed `logcat`; release needs the signing keystore, see Prerequisites), a booted
iOS Simulator (`xcodebuild build` → `simctl install`/`launch --console-pty`), or a
physical iPhone (iOS 17+: a signed build → `xcrun devicectl device install
app`/`process launch --console --terminate-existing` — failures hint at
unlocking/pairing/Developer Mode). `run` defaults to debug (`build` defaults to
release); with no device selected it falls back to a streamed `cargo run` (desktop
preview, or `--watch` below) — first Android/iOS builds take a few minutes.

`frust run --render-tier <gpu|cpu>` forces the desktop preview's render tier
(`FRUST_RENDER_TIER` for a manual `cargo run`); `gpu` fails fast on an incapable
adapter, `cpu` can always be forced (desktop-preview-only today). **High refresh-rate
hints.** A generated app's iOS `CADisplayLink` requests a 30–120Hz
`preferredFrameRateRange`; Android calls `Surface.setFrameRate()` (API 30+) with the
display's max rate — both hints, unverifiable on the iOS Simulator or most Android
emulators (60Hz-only).

**TUI workbench.** Bare `frust` in an interactive terminal, or the explicit `frust tui`, opens the
ratatui workbench (scaffold/build/doctor/clean as supervised sessions, a fuzzy command palette, a
bootstrap wizard, Add Plugin — `?` opens the full keybinding help overlay). Opening the workbench
immediately runs device discovery, the doctor preflight, and the bootstrap report in the invoking
directory, and records the active project — the cwd-detected one, or (when the invoking directory
isn't a Frust project) a re-stamp of the persisted most-recently-opened project, with no entry
recorded if neither exists — in the recent-projects store (`~/.config/frust/tui.toml`), the
implicit side effects of the new default entry point. Both stdin and stdout must be TTYs: a non-interactive
bare `frust` prints help and exits 2, while a non-interactive `frust tui` returns a clean error
instead of touching terminal state. A per-session perf sparkline parses `frust-perf` lines once
`FRUST_TRACE` is set. `cargo test -p frust-tui` covers engine/render logic; live-terminal gestures
and a fresh-machine bootstrap walk are a **manual gate** for a person at a real desk, not CI.

## Dev Loop

```bash
frust run --watch
```

Desktop only: watches `src/` and `Cargo.toml`, killing and relaunching (`cargo run`,
incremental) on change, debouncing a save-burst into one relaunch. Kill/relaunch and
Ctrl-C exit both group-kill on Unix (Windows stays direct-child-only). A **relaunch
loop, not state-preserving hot reload** — app state resets every rebuild. `--watch` +
`-d <device>` is a hard error (device-side watch isn't implemented).

**Measured baseline**: default-config incremental `cargo build` medians 0.89s; edit-to-first-frame medians
~263ms once built. An alternate linker and `cranelift` both measured worse, so **no
fast-dev template recipe ships** — re-run `scripts/devloop-measure.sh` if that changes.

## Release Builds

```bash
# Android: signed release APK (needs resolvable signing material, not just a file's
# existence — see the `[signing]` gate in docs/CLI_ARCHITECTURE.md)
frust build apk --release

# Other Android artifact shapes
frust build apk --debug
frust build apk --profile
frust build apk --release --split-per-abi --target-platform android-arm64,android-x64
frust build appbundle --release --build-name 1.2.3 --build-number 7

# iOS: signed device build, unsigned device build, App Store archive
frust build ios
frust build ios --no-codesign
frust build ipa --export-method app-store-connect

# Remove build output (cargo clean + the Android build/.gradle directories)
frust clean
```

`--flavor <name>` needs a matching Gradle flavor or Xcode scheme+configuration already
declared in the project. **Android release minification.** A generated app's
`release`/`profile` Gradle build type runs R8 (`isMinifyEnabled`/`isShrinkResources =
true`) against `proguard-rules.pro` (keeps the Frust JNI surface and the vendored
`accesskit_android` delegate); `--debug` is unaffected. NDK r27+ already 16KB-aligns
`.so` LOAD segments, so Android 15's page-size rule needs no linker-flag change.
**Release-lean mode.** `--release` also compiles out all `perf-trace` instrumentation
and enables the app's `lean` feature (`log/release_max_level_warn`), matching Flutter's
release log-level parity; `scripts/release-lean-check.sh` is the manual strings-absence
gate confirming both before shipping.

## Test

```bash
# Standard verify gate (run before considering any change done):
cargo build --workspace --locked \
  && cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check \
  && cargo build -p no-catalogs
```

`frust-drive`/`frust-tui` ride this gate automatically (root workspace members).

**The `no-catalogs` step is load-bearing.** `cargo build`/`test --workspace` alone does
NOT exercise the catalog-off configuration — Cargo unifies `frust`'s features across
every root-workspace member (`plugins/native-widgets` has its own legitimate default-on
`frust` dependency), so only the package-scoped build above actually compiles
`--no-default-features` (`examples/no-catalogs/src/main.rs`'s doc comment has the full
evidence). Skip it and a catalog opt-out regression ships behind a green `--workspace`
run.

Additionally run:

```bash
(cd examples/huddle && cargo test) \
  && (cd examples/huddle && cargo clippy --all-targets -- -D warnings) \
  && (cd plugins/clean-signals-frust && cargo test) \
  && (cd plugins/clean-signals-frust && cargo clippy --all-targets -- -D warnings)
```

`examples/huddle` and `plugins/clean-signals-frust` each gate from their own directory
rather than `-p` from the repo root because both are standalone workspaces excluded from
the root one (*Version-Pin Policy*); both git+rev-pin `clean-signals` to its public repo,
so no local sibling checkout is required to run this gate. This gate is separate from
`frust build apk`/`run`'s pipeline gate (*Run*).

**Non-default features are not compiled by the chain above.** `frust-render`'s
`cpu-tier` (experimental `vello_cpu` render backend — see *Version-Pin Policy*) is
headless and needs no GPU: run `cargo test -p frust-render --features cpu-tier` when
touching `frust-render`. `frust-native-widgets`' `demo-components` is a composite
`NativeComponent` demo of real JNI/UIKit view construction, shipped inside the plugin
rather than in `examples/glyph-catalog` (which merely switches it on) because an app
crate cannot implement that trait without raw `jni`/`objc2-ui-kit` deps the plugin does
not re-export; the mobile compile gates below are the only thing that builds it.

**Manual/gated tests** (not part of the default `cargo test --workspace` run — each
requires local hardware or is slow, and is marked `#[ignore]` with a reason):

```bash
# GPU smoke test (frust-render): needs a real Metal/Vulkan device.
cargo test -p frust-render -- --ignored

# Scaffold end-to-end test (frust-cli): compiles a freshly generated project's full
# dependency graph (winit/vello/wgpu) — ~30s cold.
cargo test -p frust-cli --test create_e2e -- --ignored

# iOS scaffold test (frust-cli): scaffolds a project and runs `xcodebuild -list` + `plutil
# -lint` against the generated Xcode project (parse-only; needs Xcode on macOS).
cargo test -p frust-cli --test create_ios -- --ignored

# Build pipeline e2e test (frust-cli): scaffolds a project, generates a throwaway keystore,
# and runs `build apk --release` via a real Gradle build — needs Android SDK/NDK; ~1 minute.
cargo test -p frust-cli --test build_e2e -- --ignored

# Android compile gate (no device needed): the whole facade graph must compile for Android.
cargo check --target aarch64-linux-android \
  -p frust -p frust-plugin -p frust-shared-preferences -p frust-secure-storage \
  -p frust-camera -p frust-native-widgets -p frust-clipboard -p frust-haptics
cargo check --target aarch64-linux-android -p frust-native-widgets --features demo-components

# --all-targets additionally compiles cfg(test) — the plain checks above never do, so a
# mobile shell's own test module otherwise goes uncompiled by anything:
cargo check --all-targets --target aarch64-linux-android -p frust-shell-android

# iOS compile gate (a type-check, no device/Xcode needed — runs on Linux too; only
# building/running an iOS app needs macOS, see Prerequisites): the whole facade graph
# must compile for the Simulator target (frust-secure-storage also gates the device target).
cargo check --target aarch64-apple-ios-sim \
  -p frust -p frust-shared-preferences -p frust-secure-storage -p frust-camera \
  -p frust-native-widgets -p frust-clipboard -p frust-haptics
cargo check --target aarch64-apple-ios -p frust-secure-storage
cargo check --target aarch64-apple-ios-sim -p frust-native-widgets --features demo-components

# Same --all-targets rationale as Android above:
cargo check --all-targets --target aarch64-apple-ios-sim  -p frust-shell-ios
```

The iOS compile gate above is also the only check of the `accesskit_ios` adapter today —
uncompiled on any host in this repo's history. Screen-reader verification
(TalkBack/VoiceOver) and the `cpu-tier` tier's visual behavior on real hardware are
unverified — both need a device/Simulator or physical GPU this headless host cannot provide.

**Neither compile gate above touches Kotlin or Swift.** `cargo check --target
aarch64-linux-android`/`aarch64-apple-ios*` only type-checks the Rust `frust-*` graph —
a Kotlin type error in the embedding module or a plugin's Gradle module ships through a
fully green cargo gate undetected. The only gate that actually compiles Kotlin is a real
Gradle build (`frust build apk --debug`, or the ignored `build_e2e`/scaffold tests
above); compiling Swift needs an Xcode build (macOS only).

### Deep-link manual test (Android)

A device/emulator gate for `nativeOnDeepLink` (`docs/SHELLS_ARCHITECTURE.md`'s cross-cutting host-signal flow, deep-link) on
a project scaffolded with `--deeplink-scheme <scheme> [--deeplink-host <host>]`, installed
via `frust run -d <device>`:

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

### Safe-area / keyboard / back manual test (Android + iOS)

A device/emulator gate for the inset and back contracts (see `docs/SHELLS_ARCHITECTURE.md`'s
cross-cutting host-signal flow), against an installed app (`frust run -d <device>`) — no CLI
trigger like the deep-link gate above, so each is a person-driven check:

- **Safe-area:** rotate the device; confirm top/bottom-anchored content reflows around
  the status bar/notch/gesture-nav insets in both orientations.
- **Keyboard:** focus a bottom text field; confirm content shifts clear of the on-screen
  keyboard, then dismiss it and confirm the layout returns.
- **Back:** press hardware/gesture back on a pushed route; confirm it pops one level,
  and falls through to the platform's own default at the root.
- **Density refresh:** move a running app between displays of different density; confirm
  layout rescales rather than sticking to launch-time density.

### Shared-preferences manual test (desktop + Android + iOS)

A kill-and-relaunch persistence gate for `frust-shared-preferences`
(`plugins/shared-preferences`), against the scaffolded notes app template (`frust
create`'s default `lib.rs.tmpl`, persisting its notes list + draft):

- **Persistence:** add a note (and/or edit the draft), kill the app, relaunch it, and
  confirm it survived — desktop preview, an installed Android device, and an installed
  iPhone.
- **Old-scaffold graceful error:** a project scaffolded *before* the plugin existed must
  still boot with empty state rather than crash (`PrefsError::PlatformNotInitialized`,
  caught by the template's load path).
- **macOS storage-location caveat:** the unbundled desktop preview (no
  `CFBundleIdentifier`) writes `NSUserDefaults` to the global defaults domain rather
  than an app-specific plist — a storage-location difference, not a behavioral one.

### Secure-storage manual test (desktop + Android + iOS)

A device/emulator gate for `frust-secure-storage`, against an app that depends on the
plugin per **only** `plugins/secure-storage/README.md`:

- **Persistence:** store a value, kill/relaunch the app, confirm it reads back —
  desktop, an installed Android device, and an installed iPhone.
- **Biometric round-trip (physical device only):** open a store with
  `AuthPolicy::Required` following *only* the README's steps; confirm Face ID/Touch ID
  (iOS) or `BiometricPrompt` (Android API 28+) gates each call.
- **Old-scaffold graceful error:** a pre-plugin scaffold must surface a typed
  `PlatformNotInitialized`, never crash.
- **Add Plugin dialog:** clean scaffold builds with zero hand edits.
- **`--overwrite` caveat:** `frust create --overwrite` re-renders the project and drops
  every addition above; recovery is re-running Add Plugin (idempotent).

### Camera manual test (Android + iOS)

A device gate for `frust-camera` (`plugins/camera`), against an app depending on
the plugin per `plugins/camera/README.md`:

- **Permission + preview:** grant, deny, then re-grant via settings; confirm a live Mode B preview inside app chrome in both orientations.
- **Capture + stream:** still capture produces an orientation-correct JPEG; the stream toggle shows a live fps readout (`docs/LIMITATIONS.md`'s `cam-bgra-apple-only`).
- **A6 keep-alive:** scroll the preview slot off-screen and back; confirm it resumes without reopening the camera.
- **Forced-blit degrade:** `FRUST_NO_DIRECT_SURFACE=1` on Android makes the preview invisible (`docs/LIMITATIONS.md`'s `cam-blit-opaque`) — expected.
- **Torch:** toggle on/off on the back lens from the catalog camera page; confirm `torch_available()` is false on the front lens; confirm torch survives starting/stopping the barcode scan strip.
- **Scan:** policy mode detects the dense muxr:// screen-QR once (NoDuplicates), timing mode shows decode ms + attempts/s for MEASUREMENTS.md.
- **Add Plugin dialog:** clean scaffold, both platforms build with zero hand edits.

### Template development

`frust create` embeds `templates/app/` into the binary at compile time; the hidden,
development-only `--template-dir <path>` flag iterates on template files without
rebuilding the embedded copy. Every scaffold also gets a default launcher icon set and
the platform-specific edge-to-edge/safe-area/keyboard-inset and back-navigation glue
the generated app needs — see `docs/SHELLS_ARCHITECTURE.md`'s cross-cutting host-signal flow.

`--arch clean-signals` scaffolds a clean-architecture variant (controller + use-case +
`async_view` over `clean-signals-frust`) instead of the default notes-app template;
`clean-signals` is git+rev-pinned to its public GitHub repo (see *Version-Pin Policy*), so
the scaffold builds on any machine with no sibling checkout required.

**Platform embedding modules ship in-repo, not templated.**
`platform/android/frust-embedding` and `platform/ios/FrustEmbedding` are consumed by a
scaffolded project by path — edit the module in place and rebuild the consuming app
directly, no re-scaffold needed. A scaffolded project's embedding path is
machine-specific: moving it means editing `gradle.properties`'s `frust.embedding.dir`
line (Android) or the local package reference in `project.pbxproj` (iOS); `frust clean`
also removes the redirected Gradle build output.

## Benchmarks

`benchmarks/` is a paired Frust-vs-Flutter measurement suite, out-of-tree from the crate
workspace: `frust_bench/` (standalone Cargo package, gate from its own directory like
`examples/huddle`) and `flutter_bench/` (Flutter SDK, tested against 3.44.2 stable)
implement the same eight timed scenarios (S1–S8), driven by `harness/`'s shared scripts.
`frust_bench` additionally carries S9 (terminal-grid) and S10 (an IME capability probe, never
timed and never recorded in RESULTS.md); S9 is paired with a Flutter counterpart only at the
protocol level today — the Flutter side remains on the spike branch, not in this tree.

```bash
./benchmarks/harness/run.sh <scenario> --app frust|flutter --device <serial>
```

`benchmarks/PROTOCOL.md` is the published methodology (device matrix, run counts,
statistics, fairness gates, raw-line formats); `benchmarks/RESULTS.md` is the filled
record of actual device runs (never placeholder/projected numbers), with every
methodology deviation labeled per run. **macOS `mktemp` caveat.** `harness/run.sh`'s
`mktemp` only expands correctly under GNU mktemp — BSD/macOS returns the template
unexpanded, silently colliding runs; alias `mktemp` to `gmktemp` (`brew install
coreutils`) until fixed.

## Instrumentation

| Variable | Purpose | Default |
|---|---|---|
| `FRUST_TRACE` | Enables `frust-perf` frame/startup logging (`frust-shell-common::perf`); requires a `perf-trace` build (debug/profile compile it in by default) — release compiles the instrumentation out entirely, no code or strings. Runtime env var; `frust run --profile`/`frust build --profile` auto-inject `--define FRUST_TRACE=1` unless already set — opt out with `--define FRUST_TRACE=0`. | off |
| `FRUST_NO_FRAME_GATE` | Kill switch for the mobile whole-frame skip gate (`docs/SHELLS_ARCHITECTURE.md`'s `frame_gate` module) — forces every Choreographer/`CADisplayLink` tick to run, restoring pre-gate behavior. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Reach for this first when diagnosing a suspected stuck-UI report. | off (gate active) |
| `FRUST_NO_ANIM_PACING` | Kill switch for animation-loop pacing only (`docs/SHELLS_ARCHITECTURE.md`'s `frame_gate` module) — a paced (`TickClass::CosmeticLoop`) frame request runs on its vsync as before; the whole-frame skip gate (`FRUST_NO_FRAME_GATE` row above) stays active regardless. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Narrower A/B valve than `FRUST_NO_FRAME_GATE` — reach for this when isolating pacing from skip-gate behavior. | off (pacing active) |
| `FRUST_LOG` | Desktop-only stderr log level override (`frust-shell-desktop::logger`) — the sink `perf`'s `log::info!` lines print through; Android/iOS use their platform loggers instead. The logger suppresses only known-noisy vello Error/Warn messages below `debug` (see *Known Issues*' vello bitmap-emoji note); unknown vello errors still surface at the default level. Pass `FRUST_LOG=debug` to see all vello log lines when debugging the render stack. | `info` |
| `FRUST_RENDER_TIER` | Forces the desktop preview's render tier (`gpu`/`cpu`) — see *Run* above. | adapter-probed |
| `FRUST_TRACE_RAW` | A second dial beside `FRUST_TRACE`, requiring the same `perf-trace` build: with both set, `FrameStats` emits one parseable `frust-perf raw ...` line per frame (instead of periodic summaries), plus `bench-scenario-start/end <name>` marker lines for a benchmark harness to slice by. Setting `FRUST_TRACE_RAW` alone does nothing — `FRUST_TRACE` must also be on. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Raw line format is v3 (`acquire_us`/`submit_us` as separate fields, superseding v2's single `present_us`); `stats.py` parses key=value so v1/v2/v3 logs stay parseable. In the default render-thread split, a gate-skipped frame never reaches this line — see the `FRUST_NO_RENDER_THREAD` row below for skip-sensitive series. See `benchmarks/PROTOCOL.md`'s raw-format changelog for the full field history. | off |
| `FRUST_NO_RENDER_THREAD` | Kill switch for the render-thread split (`docs/ARCHITECTURE.md`'s frame pipelines) — restores the pre-split single-thread path (rebuild/layout/paint/encode/acquire/present all on the UI/main thread), the fallback if the split needs to be ruled out. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Also the skip-count fix: a `FrameGate` `Skip` sends nothing across the split's UI→render channel, so it is never recorded in `FrameStats`/the raw line — build with this set when a skip-sensitive series (skip counts/rates) needs every skip counted. | off (split active) |
| `FRUST_NO_RESAMPLE` | Kill switch for the mobile pointer-event resampler (`docs/SHELLS_ARCHITECTURE.md`'s `frust-shell-common` kill-switch data flow) — forces raw per-touch delivery with no frame-boundary interpolation/prediction. Same compile-time-or-runtime parsing as `FRUST_TRACE`. | off (resampler active) |
| `FRUST_NO_DIRECT_SURFACE` | Kill switch pinning a direct-capable surface onto the blit fallback arm (`docs/RENDER_ARCHITECTURE.md`'s direct-to-surface/blit data flow) — the A/B valve for comparing the direct-to-surface and blit render paths on the same hardware. Same compile-time-or-runtime parsing as `FRUST_TRACE`. **A/B caveat:** on the direct arm the GPU render moves into `submit_us` (out of `encode_us`) and `acquire_us` now precedes it rather than follows — account for this remap before comparing `submit_us` across arms (`SurfaceRenderer::submit`'s doc comment has the full v3 field mapping). | off (path auto-probed) |
| `FRUST_NO_SHADER_EFFECTS` | Kill switch for the shader-quad pre-pass (`docs/RENDER_ARCHITECTURE.md`'s shader pre-pass data flow) — disables it entirely, so `Command::ShaderQuad` falls back to the built-in placeholder fill instead of running the pre-pass. Same compile-time-or-runtime parsing as `FRUST_TRACE`. | off (pre-pass active) |

`FRUST_TRACE=1 (cd examples/huddle && cargo run)` prints a `frust-perf startup ...`
line, then periodic `frust-perf frame ...` summaries; on a platform-view page it
also emits a rate-limited `frust-perf platform-view tail depth=...` line from the
Android scroll-sync tail. `frust-camera`'s backends separately `log::debug!` the
platform's own expected frame-rate range at session configure (`FRUST_LOG=debug`),
so a stream's fps reading can be compared against it.

`scripts/size-report.sh [--app <dir>]` (default `examples/huddle`) builds the arm64-v8a
release `.so` via `cargo ndk`, reporting unstripped/stripped size, an APK/AAB per-ABI
`.so`+dex breakdown when Gradle output already exists, and a desktop `cargo bloat
--release -n 20` breakdown when installed — every missing-tool/artifact path degrades to
a printed note.

## Version-Pin Policy

The rendering stack's versions in `[workspace.dependencies]` are pinned deliberately,
not floating. Each row's tripwire must be re-run after touching that pin:

| Pin | Why | Tripwire |
|---|---|---|
| `vello 0.9.0` / `wgpu 29.0.3` (resolves 29.0.4) | `vello` requires `wgpu ^29.0.3`; bumping `wgpu` independently (30.x is ecosystem-latest) breaks the build | `cargo build --workspace --locked` |
| `image =0.25.10` exact (`Image` widget's PNG/JPEG decoder, `png`/`jpeg` only) | Only 0.25.x release whose MSRV equals the workspace `rust-version` (1.88) | fresh MSRV check before bumping, not just `cargo update` |
| `reactive_graph 0.2` / `any_spawner 0.3` / `tokio 1` (no default features), minor | `frust-reactive` substrate, pre-1.0 Leptos-ecosystem churn expected; never enable `reactive_graph`'s `effects` feature — the frame path is a custom subscriber, not `RenderEffect` (see ARCHITECTURE's Key Types) | `cargo test -p frust-reactive` |
| `clean-signals` (git, `rev = "910f626"` on `master`, not published to crates.io) | Consumed by `examples/huddle`, `plugins/clean-signals-frust` (dep + `test-fixtures` dev-dep), and `templates/app`'s clean-signals scaffold variant — all four sites must pin the identical git+rev spec (type-identity rule below). Never enable its `effects` feature (`reactive_graph/effects`, same prohibition as the `reactive_graph` row above) — absent at this rev, confirm it stays that way before bumping | `cargo generate-lockfile` + `cargo build --locked` in each of `examples/huddle`/`plugins/clean-signals-frust` |
| `accesskit 0.24` minor + adapters (`accesskit_winit 0.33`, `accesskit_android 0.7` minor, `accesskit_ios =0.1.2` exact) | `frust-core`'s semantics-pass vocabulary; `accesskit_ios` is younger/less proven, compile-gate only (*Test*) | `cargo test -p frust-core semantics` |
| `vello_cpu =0.0.9` exact | Experimental CPU render tier (`frust-render`'s non-default `cpu-tier` feature), pre-1.0 unstable API, isolated behind the `SceneSink` encode seam so a breaking bump never reaches the default GPU path | tripwire in *Test* |
| `ndk-context 0.1` minor | `frust-plugin`'s Android platform-handle slot (written by `nativeInitPlatform`, read by every plugin) | `cargo check --target aarch64-linux-android -p frust-plugin` |
| `objc2 0.6` / `objc2-foundation 0.3` minor | Apple ObjC bridge (`frust-shared-preferences`'s `NSUserDefaults` backend; `frust-secure-storage`'s apple arm also pulls `objc2-foundation` for `NSString`/`NSError`; `frust-camera`'s apple arm pulls the full `objc2-av-foundation`/`objc2-core-media`/`objc2-core-video`/`objc2-quartz-core`/`dispatch2`/`block2` stack, each pinned `0.3`/`0.6` minor — `objc2-av-foundation 0.3.2` itself permits `objc2 >=0.6.2, <0.8.0`, wider than this workspace's own `0.6` caret) | `cargo check --target aarch64-apple-ios-sim -p frust-shared-preferences && cargo check --target aarch64-apple-ios-sim -p frust-camera` |
| `androidx.camera:camera-{core,camera2,lifecycle} 1.6.1` (Gradle, not a Cargo pin) minor | `frust-camera`'s Android CameraX session/preview stack (`plugins/camera/platform/android/build.gradle.kts`); deliberately not `camera-view` (no `PreviewView`) | `(cd examples/glyph-catalog/android && ./gradlew :frust-camera:compileReleaseKotlin)` |
| `objc2-security 0.3` / `objc2-local-authentication 0.3` minor | `frust-secure-storage`'s Apple Keychain backend + biometric gate (`SecAccessControl`/`LAContext`) | the secure-storage mobile compile gates above |
| `objc2-ui-kit` / `objc2-quartz-core` / `objc2-core-text` / `objc2-core-foundation` 0.3 minor | `frust-native-widgets`'s (`plugins/native-widgets`) UIKit binding, plus its theme-ladder L2 (`objc2-quartz-core`'s `CALayer.cornerRadius`) and L3 (`objc2-core-text`/`objc2-core-foundation` resolving the embedded Glyph font bytes to a `CTFont`) direct pins — each already resolved transitively before this crate named it directly, so no lockfile version change. Also consumed by `frust-clipboard` (iOS `UIPasteboard`) and `frust-haptics` (the three `UI*FeedbackGenerator` classes). `cargo tree -i objc2-ui-kit` legitimately shows two versions — `0.2.2` pulled transitively by `accesskit_ios`/`winit`, `0.3.2` consumed by `frust-native-widgets`/`frust-clipboard`/`frust-haptics` — an expected split, not `cargo tree -d` drift; do not force-align them | `cargo check --target aarch64-apple-ios-sim -p frust-native-widgets -p frust-clipboard -p frust-haptics` |
| `keyring-core 1.0` / `zbus-secret-service-keyring-store 1.0` (`rt-async-io-crypto-rust` feature, keeps the plugin tokio-free) / `windows-native-keyring-store 1.1` minor | `frust-secure-storage`'s desktop Linux/Windows backend | `cargo test -p frust-secure-storage` |
| `arboard =3.6.1` exact | `frust-clipboard`'s desktop (macOS/Linux/Windows) text-clipboard backend; `default-features = false` drops the default `image-data` feature; `wl-clipboard-rs`'s native-Wayland `wayland-data-control` feature is deliberately not enabled | `cargo check -p frust-clipboard` |
| `notify 8` minor (`frust-cli`-only) | `frust run --watch`'s filesystem watcher (`ctrlc` floats, shared by `frust-drive`/`frust-cli`, unpinned; `frust-drive`'s copy now enables the `termination` feature so `frust-drive::interrupt` also catches SIGTERM/SIGHUP — needed to scrub the plaintext release-signing file on a CI runner's kill, not just Ctrl-C. Visible consequence: interrupting a `--release` `frust run` before logcat streaming begins now exits 130 for SIGTERM/SIGHUP, where an unhandled signal previously exited 143/129; the logcat phase's own Ctrl-C-means-stop override still exits 0 for all three signals) | `cargo test -p frust-cli` |
| `ratatui 0.30` / `crossterm 0.29` / `ansi-to-tui 8.0.1` minor | `frust-tui`'s render/terminal/log stack, pre-1.0 churn expected | `cargo test -p frust-tui` |
| `toml_edit 0.25` minor | `frust-tui`'s config persistence and `frust-drive::plugin`'s format-preserving Cargo.toml/manifest edits | `cargo test -p frust-tui` && `cargo test -p frust-drive` |

- `examples/huddle` and `plugins/clean-signals-frust` are each a **standalone package**
  (own `[workspace]` root/`Cargo.lock`, excluded from the root `[workspace]`),
  git+rev-pinning the `clean-signals` core crate to its public repo (see the table row
  above) rather than a path dep — every consumer, including `templates/app`'s
  clean-signals scaffold variant, must resolve the identical git+rev spec, or Cargo
  builds two distinct crate identities. Neither manifest can use `{ workspace = true }`;
  gate each from its own directory (`cargo test` + `cargo clippy --all-targets -- -D
  warnings`).
- **Never run a blind `cargo update`.** After any pinned-dependency manifest change, run
  `cargo generate-lockfile` then confirm `cargo build --workspace --locked` still
  succeeds before committing.
- Use `cargo tree -d` to check for duplicate/divergent versions of a crate across the
  dependency graph after any manifest change.
- Android deps (`jni`, `ndk`, `ndk-sys`, `android_logger`) are target-gated (they only
  compile for `--target *-linux-android`) but legitimately appear in `Cargo.lock` on
  every platform — expected, not drift. `libc` (unpinned `0.2`) is the same shape but
  two-platform: android's/ios's render-thread priority self-boosts
  (`docs/CODE_STANDARDS.md`'s sanctioned-unsafe zones).

## Platform-Support Policy

The minimum supported platform is a **project-wide constant, not a per-module choice**. Every module
must declare the same floor; the numbers are repeated in an in-file comment at each site.

**A mismatch fails in one of two ways, depending on direction** (both verified empirically):

- **App below library → hard build error.** AGP's manifest merger refuses it:
  `uses-sdk:minSdkVersion 24 cannot be smaller than version 26 declared in library
  [:frust-embedding] … as the library might be using APIs not available in 24`.
  `:app:processDebugMainManifest` fails; nothing is produced.
- **Library below app → silent behaviour change.** No error; the library just misses APIs it could
  have used, exactly as `IME_FLAG_NO_PERSONALIZED_LEARNING` did (below).

| Platform | Floor | Declared in |
|----------|-------|-------------|
| Android | **`minSdk = 26`** (Android 8.0) · `compileSdk = 36` | 10 Gradle files: `platform/android/frust-embedding`, `plugins/{camera,native-widgets,secure-storage}/platform/android`, `templates/app/android.tmpl/app`, and the 5 example/benchmark apps |
| iOS | **15.0** | `platform/ios/FrustEmbedding/Package.swift` (`.iOS(.v15)`) and each app's `IPHONEOS_DEPLOYMENT_TARGET` |

**Adding a new Android module?** Copy the floor and the lockstep comment. **Adding a new iOS
target?** `Package.swift`'s `platforms:` must stay **at or below** every consumer's
`IPHONEOS_DEPLOYMENT_TARGET` — a package minimum above the app's is a compile error.

### Why Android is 26 and must not go lower

Raised from 24 in `db6827b`. API 24/25 (Android 7.x) were dropped as too old to carry, and the floor
was actively costing correctness: **`EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING` is API 26+**, so
the keyboard-learning half of FINDINGS #31's mitigation was a no-op below it. It is an `imeOptions`
bit, so the constant inlines at compile time and an older IME simply ignores it — no compile error,
no runtime crash. At 26 the flag is unconditionally honoured and `frust-core/src/event.rs`'s IME
contract table holds at the floor.

> **Lint *did* catch this, and we were not running lint.** Re-tested by setting the module back to
> `minSdk = 24`: `lintDebug` reports it twice as **`InlinedApi`** (the rule specifically for inlined
> constants) — *"Field requires API level 26 (current min is 24):
> `android.view.inputmethod.EditorInfo#IME_FLAG_NO_PERSONALIZED_LEARNING`"*. Its default severity is
> **warning**, so the build still exits 0. The defect was not invisible to tooling; it was invisible
> because **no gate ran Android Lint at all**. Wiring `lintDebug` into the Android gate — and
> deciding whether `InlinedApi` should be an error here — is open work, not something this bump
> settled.

**Lowering the floor below 26 re-opens that hole silently.** If it is ever lowered, restore the
API-caveat entry in `docs/LIMITATIONS.md` in the same change.

**One** API becomes available at the new floor and is deliberately **not** adopted:
`SeekBar.setMin` (API 26) — `plugins/native-widgets/src/controls/slider.rs` keeps its Rust-side
range mapping; see that module doc for why.

Two things the bump does **not** unlock, and which still need their existing workarounds:
`Font.Builder(ByteBuffer)` is **API 29**, so `typeface.rs` still writes a cache file; and
`BiometricPrompt` is **API 28+**, so `secure-storage`'s gate stays and
`NotAvailable(UnsupportedApiLevel)` remains reachable on 26 and 27.

### Migrating an already-scaffolded app

Apps generated before `db6827b` carry `minSdk = 24`. **The bump is mandatory, not optional** — a
scaffolded app consumes the embedding as a Gradle *project* dependency
(`implementation(project(":frust-embedding"))`), so an app at 24 against the 26 library fails the
manifest merger outright (verified: `:app:processDebugMainManifest` exits 1 with the
`cannot be smaller than version 26` error quoted above). Set `minSdk = 26` in the app's
`app/build.gradle.kts`; there is no other migration step.

**Verifying a floor change:** `cargo` cannot see any of this. Run, from an app dir:

```
./gradlew :app:processDebugMainManifest   # catches an app-below-library mismatch (hard error)
./gradlew compileDebugKotlin lintDebug    # catches API usage above the floor, as InlinedApi/NewApi
```

`lintDebug` reports API-above-floor usage at **warning** severity, so a green exit code does **not**
mean clean — read the SARIF/HTML report under `build/reports/`, or raise the severity, before
concluding anything.

## Known Issues

### Formatting

The codebase is formatted with `rustfmt` using default settings (no `rustfmt.toml`);
`cargo fmt --check` is part of the verify gate above.

### Android emulator GPU on Apple Silicon

An Apple-Silicon Android emulator's default (hardware) GPU path segfaults on
`vkQueueSubmit` in the emulator's gfxstream/MoltenVK Vulkan driver (an emulator/driver
limitation, not a Frust bug). Use a physical device, or boot with `-gpu swiftshader`
(software Vulkan; slower but correct).

### iOS Simulator cannot render (vello 0.9 / wgpu 29)

The iOS Simulator's GPU only exposes the Apple2 Metal feature family, lacking
`wgpu::DownlevelFlags::INDIRECT_EXECUTION`, which vello 0.9's renderer unconditionally
requires — a wgpu-hal-29/vello-0.9 limitation, not fixable under the version pin.
`frust-render` detects it and fails fast with a clear diagnostic instead of a per-frame
panic; `frust run` still builds/installs/launches, but the window stays black.
**Physical iOS devices are unaffected** (verified on an iPhone 13 mini and iPhone SE)
— use one for a pixel-accurate check until a future wgpu/vello upgrade closes the gap.

### Android release build may not pick up `--define`

A `--release` Android build threads `--define`s into the `cargo ndk` compile via
Gradle's `environment(...)`, but has been observed to drop them (e.g. `FRUST_TRACE=1`
missing from the artifact). Workaround: export the same key/value pairs in the build
shell's environment first; fix pending.

### vello bitmap color-emoji decode (desktop confirmed safe; Android CBDT at risk)

vello 0.9's bitmap-glyph decode path (`sbix`/COLR strikes) errors and skips any glyph
whose PNG isn't `(RGBA, 8-bit)` — pinned, unfixed upstream
([linebender/vello#1031](https://github.com/linebender/vello/issues/1031)). **Safe** for
huddle's desktop emoji set (uniformly RGBA8); **unverified, at-risk**: Android's CBDT
strikes may use palette-indexed PNGs at smaller sizes. The desktop logger suppresses
this noise below `debug` (*Instrumentation*'s `FRUST_LOG` row).
