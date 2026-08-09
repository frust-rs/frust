# Frust - Development Guide

The canonical test-tier, golden-image, headless GPU, and Android emulator runbook is
`docs/TESTING.md`. This guide retains the concise build/test commands and platform
prerequisites used during ordinary development.

This is the shared index — prerequisites, build/run/test gates, benchmarks, instrumentation,
the version-pin *policy*, platform-support floors, known issues. Per-unit device gates,
template work, and the version-pin rows each unit owns live in its spoke:

| Unit | Development spoke | Holds |
|------|-------------------|-------|
| CORE | [CORE_DEVELOPMENT.md](CORE_DEVELOPMENT.md) | `reactive_graph`/`any_spawner`/`tokio`, `clean-signals`, `accesskit` pins |
| RENDER | [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md) | `vello`/`wgpu`, `image`, `vello_cpu` pins |
| SHELLS | [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md) | deep-link and safe-area/keyboard/back manual tests |
| PLUGINS | [PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md) | shared-preferences, secure-storage, camera, and IAP manual tests; `ndk-context`, `objc2*`, CameraX, OpenIAP, `keyring-core`, `arboard` pins |
| CLI | [CLI_DEVELOPMENT.md](CLI_DEVELOPMENT.md) | template development; `notify` pin |
| TUI | [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md) | `ratatui`/`crossterm`/`ansi-to-tui`, `toml_edit` pins |

WIDGETS and NATIVE_WIDGETS have no development spoke — everything they need is here.

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
  public repo (*Version-Pin Policy*; row in [CORE_DEVELOPMENT.md](CORE_DEVELOPMENT.md)),
  consumed by `examples/huddle`,
  `plugins/clean-signals-frust`, and `templates/app`'s clean-signals scaffold variant.
  Cloning it as a sibling directory (`../clean-signals-rs`) is still useful for local
  iteration on `clean-signals` itself, via a `[patch]` override in the consuming
  workspace — not needed for ordinary development.
- **`frust-database --features engine-turso`**: needs libclang on the host (pulls
  `bindgen`/`clang-sys` as a build dependency). The default (`engine-sqlite`) build does
  not — `rusqlite`'s `bundled` feature only needs a `cc`-compatible C toolchain.
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
repo; check scaffold changes via *Template development*
([CLI_DEVELOPMENT.md](CLI_DEVELOPMENT.md)) or the scaffold end-to-end test in *Test*.

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
  && (cd examples/playground && cargo test) \
  && (cd examples/playground && cargo clippy --all-targets -- -D warnings) \
  && (cd examples/playground && cargo fmt --check) \
  && (cd plugins/clean-signals-frust && cargo test) \
  && (cd plugins/clean-signals-frust && cargo clippy --all-targets -- -D warnings)
```

`examples/huddle`, `examples/playground`, and `plugins/clean-signals-frust` each gate from
their own directory rather than `-p` from the repo root because all three are standalone
workspaces excluded from the root one (*Version-Pin Policy*) — the same shape
`examples/shadertoy` and `examples/glyph-catalog` gate under, from their own directories,
per their own READMEs. `huddle` and `clean-signals-frust` git+rev-pin `clean-signals` to
its public repo, so no local sibling checkout is required to run this gate. This gate is
separate from `frust build apk`/`run`'s pipeline gate (*Run*).

**Non-default features are not compiled by the chain above.** `frust-render`'s
`cpu-tier` (experimental `vello_cpu` render backend —
[RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)'s pins) is
headless and needs no GPU: run `cargo test -p frust-render --features cpu-tier` when
touching `frust-render`. `frust-native-widgets`' `demo-components` is a composite
`NativeComponent` demo of real JNI/UIKit view construction, shipped inside the plugin
rather than in `examples/playground` (which merely switches it on) because an app
crate cannot implement that trait without raw `jni`/`objc2-ui-kit` deps the plugin does
not re-export; the mobile compile gates below are the only thing that builds it.
`frust-database`'s `engine-turso` needs its own gate too: `cargo test -p frust-database
--features engine-turso` (libclang required — see *Prerequisites*).

```bash
# frust-database mobile compile gates: run separately from the facade-graph gates below
# (this crate's C/bindgen build needs cargo-ndk's CC_*/AR_* passthrough, which a plain
# `cargo check --target` does not supply; the iOS gate needs xcrun, so macOS-only).
cargo ndk -t arm64-v8a check -p frust-database
cargo check -p frust-database --target aarch64-apple-ios
```

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
  -p frust-camera -p frust-native-widgets -p frust-clipboard -p frust-haptics -p frust-iap
cargo check --target aarch64-linux-android -p frust-native-widgets --features demo-components

# --all-targets additionally compiles cfg(test) — the plain checks above never do, so a
# mobile shell's or frust-iap's own target-gated test module otherwise goes uncompiled:
cargo check --all-targets --target aarch64-linux-android -p frust-shell-android -p frust-iap

# iOS compile gate (a type-check, no device/Xcode needed — runs on Linux too; only
# building/running an iOS app needs macOS, see Prerequisites): the whole facade graph
# must compile for the Simulator target (frust-secure-storage also gates the device target).
cargo check --target aarch64-apple-ios-sim \
  -p frust -p frust-shared-preferences -p frust-secure-storage -p frust-camera \
  -p frust-native-widgets -p frust-clipboard -p frust-haptics -p frust-iap
cargo check --target aarch64-apple-ios -p frust-secure-storage
cargo check --target aarch64-apple-ios-sim -p frust-native-widgets --features demo-components

# Same --all-targets rationale as Android above:
cargo check --all-targets --target aarch64-apple-ios-sim  -p frust-shell-ios -p frust-iap
```

The iOS compile gate above is also the only check of the `accesskit_ios` adapter today —
uncompiled on any host in this repo's history. Screen-reader verification
(TalkBack/VoiceOver) and the `cpu-tier` tier's visual behavior on real hardware are
unverified — both need a device/Simulator or physical GPU this headless host cannot provide.
**Running, not just type-checking, `frust-iap`'s iOS tests** needs a booted Simulator as the
test runner — currently the only way this repo executes a target-gated mobile test at all:
`CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER="xcrun simctl spawn booted" cargo test --target
aarch64-apple-ios-sim -p frust-iap --lib`.

**Neither compile gate above touches Kotlin or Swift.** `cargo check --target
aarch64-linux-android`/`aarch64-apple-ios*` only type-checks the Rust `frust-*` graph —
a Kotlin type error in the embedding module or a plugin's Gradle module ships through a
fully green cargo gate undetected. The only gate that actually compiles Kotlin is a real
Gradle build (`frust build apk --debug`, or the ignored `build_e2e`/scaffold tests
above); compiling Swift needs an Xcode build (macOS only).

### Per-unit device gates

The **deep-link** and **safe-area / keyboard / back** manual tests
([SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md)), the **shared-preferences**,
**secure-storage**, **camera**, and **IAP** manual tests
([PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md)), and **template development**
([CLI_DEVELOPMENT.md](CLI_DEVELOPMENT.md)) live in their unit spokes. Every one is a
person-driven device/emulator check with no automated counterpart — none of them ride the
gate chain above.

## Benchmarks

`benchmarks/` is a paired Frust-vs-Flutter measurement suite, out-of-tree from the crate
workspace: `frust_bench/` (standalone Cargo package, gate from its own directory like
`examples/huddle`) and `flutter_bench/` (Flutter SDK, tested against 3.44.2 stable)
implement the same scenarios — eight timed UI scenarios (S1–S8) plus two DB op-latency
scenarios (D1/D2) — driven by `harness/`'s shared scripts. `benchmarks/` is scoped strictly
to this paired-comparison protocol; the terminal-grid and IME-capability probes that used to
ride alongside it as S9/S10 now live in `examples/playground` as exploratory, single-sided
demos instead.

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

Pinned versions in `[workspace.dependencies]` (and the Gradle/SPM equivalents) are deliberate,
not floating. **Pins are LAW**: never bump one independently, and re-run that pin's tripwire
after touching it. The pin rows themselves — pin, rationale, tripwire — live with the unit that
owns them:

| Pins | Owner |
|------|-------|
| `vello`/`wgpu`, `image`, `vello_cpu` | [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md) |
| `reactive_graph`/`any_spawner`/`tokio`, `clean-signals`, `accesskit` + adapters | [CORE_DEVELOPMENT.md](CORE_DEVELOPMENT.md) |
| `ndk-context`, `objc2*` (Foundation/Security/LocalAuthentication/UIKit/QuartzCore/CoreText/CoreFoundation), `androidx.camera`, `openiap-google`/`OpenIAP`, `keyring-core`, `arboard` | [PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md) |
| `notify` | [CLI_DEVELOPMENT.md](CLI_DEVELOPMENT.md) |
| `ratatui`/`crossterm`/`ansi-to-tui`, `toml_edit` | [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md) |

The rules below bind every pin, wherever its row lives:

- `examples/huddle` and `plugins/clean-signals-frust` are each a **standalone package**
  (own `[workspace]` root/`Cargo.lock`, excluded from the root `[workspace]`),
  git+rev-pinning the `clean-signals` core crate to its public repo (see
  [CORE_DEVELOPMENT.md](CORE_DEVELOPMENT.md)) rather than a path dep — every consumer,
  including `templates/app`'s clean-signals scaffold variant, must resolve the identical
  git+rev spec, or Cargo builds two distinct crate identities. Neither manifest can use
  `{ workspace = true }`; gate each from its own directory (`cargo test` + `cargo clippy
  --all-targets -- -D warnings`).
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
