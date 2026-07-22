# Frust - Development Guide

The canonical test-tier, golden-image, headless GPU, and Android emulator
runbook is `docs/TESTING.md`. This guide retains the concise build/test commands
and platform prerequisites used during ordinary development.

## Prerequisites

- Rust 1.88+ (workspace `rust-version`), edition 2024.
- A real Metal or Vulkan adapter for the GPU smoke gate (`frust-render`'s
  `--ignored` test). Headless Vulkan needs no display server; see
  `docs/TESTING.md`. Other hosts can build and run the non-GPU suite.
- **Android** (only needed for `frust run`/`build`/`create`'s Android output):
  `rustup target add aarch64-linux-android`; `cargo install cargo-ndk`
  (tested with 4.x); JDK 17+ on `JAVA_HOME` (Android Studio's bundled JBR is
  auto-detected as a fallback on macOS); `ANDROID_HOME`/`ANDROID_SDK_ROOT`
  and `ANDROID_NDK_HOME` set. `frust doctor` checks all of these.
- **iOS** (only needed for `frust run`/`build`/`create`'s iOS output; macOS
  host only): Xcode 26+ with `xcode-select -p` resolving to it; `rustup
  target add aarch64-apple-ios-sim aarch64-apple-ios`. A booted Simulator
  suffices for a debug `frust run`. A signed build (`frust build ios`/`ipa`,
  `frust run --release`, or any physical-device run) needs a codesigning
  identity — `frust` auto-detects `DEVELOPMENT_TEAM` from `security
  find-identity`, or set it via `FRUST_IOS_TEAM`/`[ios] team` in
  `frust.toml`. A physical iPhone run also needs iOS 17+, unlocked/paired/
  trusted with Developer Mode enabled. `frust doctor` checks Rust targets
  on macOS hosts only.
- **clean-signals-rs** cloned as a sibling directory (`../clean-signals-rs`,
  branch `master`) — needed only for the `clean-signals` core crate,
  consumed by `examples/huddle` and the `plugins/clean-signals-frust` glue
  plugin. The root workspace never needs it (see *Version-Pin Policy*);
  both dependents' verify gates are conditional steps in *Test*.
- No Docker, CI config, or `.env` setup exists in this repo yet.

## Build

```bash
cargo build --workspace --locked
```

`--locked` must always pass — it is part of the verify gate below and is how
manifest/lockfile drift is caught (see *Version-Pin Policy*).

**Dev-profile shader-stack overrides.** The root `Cargo.toml`,
`templates/app/Cargo.toml.tmpl`, and `examples/huddle/Cargo.toml` each carry
a `[profile.dev.package.*]` override (`opt-level = 2`) for the shader/render
crates (`vello`, `vello_shaders`, `vello_encoding`, `wgpu`, `wgpu-core`,
`wgpu-hal`, `naga`): debug-profile (`opt-level = 0`) shader
compilation/translation is slow enough on mobile CPUs to trip the iOS
launch watchdog, so only this stack is optimized. The three manifests are
hand-synced (`cargo test -p frust-cli --test profile_sync` is the
tripwire); a `[profile.dev.package."*"]` wildcard (`opt-level = 1`) widens
every other non-workspace-member dependency's debug optimization the same way.

**Release-profile hardening.** `[profile.release]` (root, template, huddle)
sets `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic =
"abort"`, at the default `opt-level = 3` — chosen over `"s"`/`"z"` after a
smaller-opt-level win didn't clear a 5% bar against a render-stack CPU-perf
carve-out (measured via `scripts/size-report.sh`, below; full A/B in
`workflow/plans/features/frust-phase-7-performance/research/BASELINE.md`).

## Run

```bash
# Huddle: the repo's sole example (own [workspace]/Cargo.lock, excluded
# from the root workspace — run/gate from its own directory, not `-p`).
(cd examples/huddle && cargo run)
```

`(cd examples/huddle && cargo run)` is the manual visual gate for
rendering/interaction/theme/navigation/text-input changes (no automated
pixel-diff test yet, so a person must look at the window) — including the
check that a background-thread wake (e.g. a timer-driven completion)
renders content with zero mouse movement, not only on an input-triggered
redraw. `examples/huddle`'s own verify gate (`cargo test` plus clippy, from
its own directory) is the conditional step in *Test* below, gated on the
clean-signals-rs sibling checkout.

`examples/huddle` additionally builds and runs on Android and iOS, from
its own directory (its own `frust.toml`, package `it.f0x.huddle`):

```bash
cd examples/huddle
/path/to/frust build apk --debug   # debug APK via Gradle + cargo-ndk
frust run -d <device-id>           # build/install/launch/stream (Android or iOS sim/device)
```

The template `frust create` scaffolds is its own demo (a notes app,
`templates/app/src/lib.rs.tmpl`) with no example counterpart to run
directly in this repo; check scaffold changes via *Template development*
below or the scaffold end-to-end test in *Test*.

In a generated project, `frust run [-d <device>] [--release|--profile]
[--flavor <name>]` builds and launches on a connected Android
device/emulator (preflight → variant-aware `gradlew assemble<Flavor><Mode>`
via cargo-ndk → `adb install`/`launch` → streamed `logcat` until Ctrl-C;
release needs the signing keystore, see Prerequisites), a booted iOS
Simulator (preflight → `xcodebuild build` → `simctl install`/
`launch --console-pty`, streamed until Ctrl-C), or a physical iPhone (iOS
17+: a signed build → `xcrun devicectl device install app`/`process launch
--console --terminate-existing`, streamed — failures hint at
unlocking/pairing/Developer Mode). `run` defaults to debug (`build`
defaults to release); with no device selected it falls back to a streamed
`cargo run` (desktop preview, or the watch loop below with `--watch`) —
first Android/iOS builds take a few minutes.

`frust run --render-tier <gpu|cpu>` forces the desktop preview's render
tier (`FRUST_RENDER_TIER`, settable directly for a manual `cargo run`); an
explicit choice wins among tiers the adapter supports — `gpu` fails fast
on an incapable adapter, `cpu` can always be forced. Desktop-preview-only
today; a device run prints a not-plumbed note and probes its own tier.

**High refresh-rate hints.** A generated app's iOS `CADisplayLink` requests
a 30–120Hz `preferredFrameRateRange`; Android calls `Surface.setFrameRate()`
(API 30+) with the display's max rate — both hints, not guarantees; achieved
rate is device/OEM/thermal-state dependent and unverifiable on the iOS
Simulator or most Android emulators (60Hz-only).

**TUI workbench.** `frust tui` opens the ratatui workbench: `n` scaffolds a
project; the devices panel launches concurrent sessions (`r`/`Enter`);
`d`/`b`/`c` run doctor/build/clean as supervised sessions with their own
log tab; `a` opens Add Plugin; `Ctrl+O`/`p` the project switcher, `i`/chip
the bootstrap wizard, `Ctrl+P`/`:` a fuzzy command palette, `?` help,
`Alt+m` toggles mouse capture. A per-session perf sparkline (`t`) parses
`frust-perf` lines once `FRUST_TRACE` is set; settings persist to
`$XDG_CONFIG_HOME/frust/tui.toml`. `q`/`Ctrl+Q` quits. `cargo test -p
frust-tui` covers engine/render logic; live-terminal gestures, OSC-52
copy, panic-restore, and a fresh-machine bootstrap walk are a **manual
gate** for a person at a real desk, not CI.

## Dev Loop

```bash
frust run --watch
```

Desktop only: watches `src/` and `Cargo.toml`, killing and relaunching
(`cargo run`, incremental) on change, debouncing a save-burst into one
relaunch. Kill/relaunch and Ctrl-C exit both group-kill on Unix, reaching
the compiled preview binary `cargo run` forks too (Windows stays
direct-child-only). A **relaunch loop, not state-preserving hot reload** —
app state resets every rebuild. `--watch` + `-d <device>` is a hard error
(device-side watch isn't implemented).

**Measured baseline** (methodology/hardware:
`workflow/plans/features/frust-phase-9-rust-advantage/research/DEVLOOP_BASELINE.md`):
default-config incremental `cargo build` medians 0.89s; edit-to-first-frame
(`FRUST_TRACE=1`'s `first_frame_presented`) medians ~263ms once built. An
alternate linker (`ld64.lld`, 1.23s) and `cranelift` (2.85s, ~3.2×) both
measured worse, so **no fast-dev template recipe ships** — re-run
`scripts/devloop-measure.sh` if that changes.

## Release Builds

```bash
# Android: signed release APK (needs android/key.properties — see Prerequisites)
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

`--flavor <name>` needs a matching Gradle product flavor / Xcode
scheme+configuration already declared in the generated project.

**Android release minification.** A generated app's `release`/`profile`
Gradle build type runs R8 (`isMinifyEnabled`/`isShrinkResources = true`)
against `proguard-rules.pro` (keeps the Frust JNI surface and the vendored
`accesskit_android` delegate); `--debug` is unaffected. NDK r27+ already
16KB-aligns `.so` `LOAD` segments by default, so Android 15's page-size
requirement needed no linker-flag change.

## Test

```bash
# Standard verify gate (run before considering any change done):
cargo build --workspace --locked \
  && cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
```

`frust-drive`/`frust-tui` ride this gate automatically (root workspace members).

If the clean-signals-rs sibling checkout exists at `../clean-signals-rs`
(branch `master` — see Prerequisites and *Version-Pin Policy*),
additionally run:

```bash
(cd examples/huddle && cargo test) \
  && (cd examples/huddle && cargo clippy --all-targets -- -D warnings) \
  && (cd plugins/clean-signals-frust && cargo test) \
  && (cd plugins/clean-signals-frust && cargo clippy --all-targets -- -D warnings)
```

`examples/huddle` and `plugins/clean-signals-frust` each gate from their own
directory rather than `-p` from the repo root because both are standalone
workspaces excluded from the root one (*Version-Pin Policy*). No sibling
checkout? Do not run these commands — record "huddle/clean-signals-frust
gate not run — no clean-signals-rs sibling checkout" instead, and do not
touch either directory without the sibling in place. This gate is separate
from `frust build apk`/`run`'s pipeline gate (*Run*).

The `cpu-tier` feature (experimental `vello_cpu` render backend, non-default
— see *Version-Pin Policy*) is headless and needs no GPU, but isn't compiled
by the standard chain above since the feature is off by default; run it
directly when touching `frust-render`:

```bash
cargo test -p frust-render --features cpu-tier
```

**Manual/gated tests** (not part of the default `cargo test --workspace`
run — each requires local hardware or is slow, and is marked `#[ignore]`
with a reason):

```bash
# GPU smoke test (frust-render): needs a real Metal/Vulkan device.
cargo test -p frust-render -- --ignored

# Scaffold end-to-end test (frust-cli): compiles a freshly generated
# project's full dependency graph (winit/vello/wgpu) — ~30s cold.
cargo test -p frust-cli --test create_e2e -- --ignored

# iOS scaffold test (frust-cli): scaffolds a project and runs
# `xcodebuild -list` + `plutil -lint` against the generated Xcode project
# (parse-only, no build; needs Xcode on macOS).
cargo test -p frust-cli --test create_ios -- --ignored

# Build pipeline end-to-end test (frust-cli): scaffolds a project,
# generates a throwaway keystore, and runs `build apk --release` through a
# real Gradle build — needs Android SDK/NDK; ~1 minute.
cargo test -p frust-cli --test build_e2e -- --ignored

# Android compile gate (no device needed): the whole facade graph must
# compile for the Android target.
cargo check --target aarch64-linux-android -p frust
cargo check --target aarch64-linux-android -p frust-plugin
cargo check --target aarch64-linux-android -p frust-shared-preferences
cargo check --target aarch64-linux-android -p frust-secure-storage

# iOS compile gate (no device needed; macOS only): the whole facade graph
# must compile for the iOS Simulator target (frust-secure-storage also gates the real device target).
cargo check --target aarch64-apple-ios-sim -p frust
cargo check --target aarch64-apple-ios-sim -p frust-shared-preferences
cargo check --target aarch64-apple-ios-sim -p frust-secure-storage
cargo check --target aarch64-apple-ios -p frust-secure-storage
```

The iOS compile gate above is also the only check of the `accesskit_ios`
adapter today — uncompiled on any host in this repo's history (Linux-only
so far). Screen-reader verification (TalkBack/VoiceOver) and the
`cpu-tier` tier's visual behavior on real hardware are unverified — both
need a device/Simulator with a screen reader or a physical GPU, which this
headless host cannot provide.

### Deep-link manual test (Android)

A device/emulator gate for `nativeOnDeepLink` (`docs/ARCHITECTURE.md`'s Deep-link
flow) against a project scaffolded with `frust create --deeplink-scheme <scheme>
[--deeplink-host <host>]` and installed (`frust run -d <device>`):

```bash
# Cold start (app not running; queues until the native handle exists):
adb shell am force-stop <package>
adb shell am start -a android.intent.action.VIEW -d "<scheme>://<path>" <package>

# Warm (already foregrounded; singleTop routes via onNewIntent):
adb shell am start -a android.intent.action.VIEW -d "<scheme>://<other-path>" <package>
```

Confirm the app navigates to the linked route both times. The iOS
equivalent (`frust_on_deep_link`, via `SceneDelegate`'s
`scene(_:openURLContexts:)`) has a Simulator-only CLI trigger (`xcrun
simctl openurl booted "<scheme>://<path>"`); a physical device has none —
tap a registered `CFBundleURLSchemes` link (e.g. from Notes) instead.

`frust.toml`'s `[deeplink]` section is informational only — it doesn't
re-render the Android manifest intent filter or iOS `Info.plist`; use
`--overwrite` or edit the platform files directly to change the scheme.

### Safe-area / keyboard / back manual test (Android + iOS)

A device/emulator gate for the inset and back contracts (see
`docs/ARCHITECTURE.md`'s Inset delivery / Back flow), against an installed
app (`frust run -d <device>`) — no CLI trigger like the deep-link gate
above, so each is a person-driven check:

- **Safe-area:** rotate the device; confirm top/bottom-anchored content
  reflows around the status bar/notch/gesture-nav insets in both orientations.
- **Keyboard:** focus a bottom text field; confirm content shifts clear of
  the on-screen keyboard, then dismiss it and confirm the layout returns.
- **Back:** press hardware/gesture back on a pushed route; confirm it pops
  one level, and falls through to the platform's own default at the root.
- **Density refresh:** move a running app between displays of different
  density; confirm layout rescales rather than sticking to launch-time density.

### Shared-preferences manual test (desktop + Android + iOS)

A kill-and-relaunch persistence gate for `frust-shared-preferences`
(`plugins/shared-preferences`), against the scaffolded notes app template
(`frust create`'s default `lib.rs.tmpl`, persisting its notes list + draft):

- **Persistence:** add a note (and/or edit the draft), kill the app, relaunch
  it, and confirm it survived — on the desktop preview (`cargo run` from a
  scaffolded project), an installed Android device, and an installed iPhone.
- **Old-scaffold graceful error:** a project scaffolded *before* the plugin
  existed (no `nativeInitPlatform` call on Android) must still boot with
  empty state rather than crash — `SharedPreferences::standard()` surfaces
  a typed `PrefsError::PlatformNotInitialized`, caught by the template's
  load path, which falls back to defaults.
- **macOS storage-location caveat:** the unbundled desktop preview (no
  `CFBundleIdentifier`) writes `NSUserDefaults` to the global defaults
  domain rather than an app-specific plist — a storage-location difference,
  not a behavioral one (the plugin's `frust.`-prefixed keys stay isolated).

### Secure-storage manual test (desktop + Android + iOS)

A device/emulator gate for `frust-secure-storage`, against an app that
depends on the plugin per **only** `plugins/secure-storage/README.md`:

- **Persistence:** store a value, kill/relaunch the app, confirm it reads
  back — desktop, an installed Android device, and an installed iPhone.
- **Biometric round-trip (physical device only):** open a store with
  `AuthPolicy::Required` following *only* the README's steps; confirm Face
  ID/Touch ID (iOS) or `BiometricPrompt` (Android API 28+) gates each call.
- **Old-scaffold graceful error:** a pre-plugin scaffold must surface a
  typed `PlatformNotInitialized`, never crash.
- **Add Plugin dialog:** run the frust TUI's Add Plugin dialog against a
  clean scaffold; confirm the build succeeds with zero hand edits.
- **`--overwrite` caveat:** `frust create --overwrite` re-renders the
  project and drops every addition above; recovery is re-running the Add
  Plugin dialog (idempotent).

### Template development

`frust create` embeds `templates/app/` into the binary at compile time; the
hidden, development-only `--template-dir <path>` flag iterates on template
files without rebuilding the embedded copy. Every scaffold also gets a
default launcher icon set and the platform-specific edge-to-edge/safe-area/
keyboard-inset and back-navigation glue the generated app needs — see
`docs/ARCHITECTURE.md`'s Inset delivery and Back flow.

`--arch clean-signals` scaffolds a clean-architecture variant (controller +
use-case + `async_view` over `clean-signals-frust`) instead of the default
notes-app template — dev-machine-only while `clean-signals` is unpublished,
since the generated project path-depends into this checkout and the
sibling `../clean-signals-rs` checkout (see *Version-Pin Policy*); `--help`
carries this caveat.

## Benchmarks

`benchmarks/` is a paired Frust-vs-Flutter measurement suite, out-of-tree
from the crate workspace: `frust_bench/` (standalone Cargo package, own
workspace/lockfile, gate from its own directory like `examples/huddle`) and
`flutter_bench/` (Flutter SDK, tested against 3.44.2 stable) implement the
same eight scenarios (S1–S8), driven by `harness/`'s shared scripts:

```bash
./benchmarks/harness/run.sh <scenario> --app frust|flutter --device <serial>
```

`benchmarks/PROTOCOL.md` is the published methodology (device matrix, run
counts, statistics, fairness gates, raw-line formats); `benchmarks/
RESULTS.md` is the filled record of actual device runs (never placeholder/
projected numbers), with every methodology deviation labeled per run.

**macOS `mktemp` caveat.** `harness/run.sh`'s `mktemp` only expands
correctly under GNU mktemp — BSD/macOS returns the template unexpanded,
silently colliding runs; alias `mktemp` to `gmktemp` (`brew install coreutils`) until fixed.

## Instrumentation

| Variable | Purpose | Default |
|---|---|---|
| `FRUST_TRACE` | Enables `frust-perf` frame/startup logging (`frust-shell-common::perf`). Runtime env var on any build; `frust run --profile`/`frust build --profile` auto-inject `--define FRUST_TRACE=1` unless already set — opt out of that default with `--define FRUST_TRACE=0`. | off |
| `FRUST_NO_FRAME_GATE` | Kill switch for the mobile whole-frame skip gate (`docs/ARCHITECTURE.md`'s Frame gate) — forces every Choreographer/`CADisplayLink` tick to run, restoring pre-gate behavior. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Reach for this first when diagnosing a suspected stuck-UI report. | off (gate active) |
| `FRUST_LOG` | Desktop-only stderr log level override (`frust-shell-desktop::logger`) — the sink `perf`'s `log::info!` lines print through; Android/iOS use their platform loggers instead. The logger suppresses only known-noisy vello Error/Warn messages below `debug` (see *Known Issues*' vello bitmap-emoji note); unknown vello errors still surface at the default level. Pass `FRUST_LOG=debug` to see all vello log lines when debugging the render stack. | `info` |
| `FRUST_RENDER_TIER` | Forces the desktop preview's render tier (`gpu`/`cpu`) — see *Run* above. | adapter-probed |
| `FRUST_TRACE_RAW` | A second dial beside `FRUST_TRACE`: with both set, `FrameStats` emits one parseable `frust-perf raw ...` line per frame (instead of periodic summaries), plus `bench-scenario-start/end <name>` marker lines for a benchmark harness to slice by. Setting `FRUST_TRACE_RAW` alone does nothing — `FRUST_TRACE` must also be on. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Raw line format is v3: `acquire_us`/`submit_us` are separate fields, replacing v2's single `present_us` (`acquire_us + submit_us` == old `present_us`, so cross-baseline math still works); `stats.py` parses key=value so v1/v2/v3 logs stay parseable. In the default render-thread split, a gate-skipped frame is not sent to the render thread at all, so it never reaches this line — see the `FRUST_NO_RENDER_THREAD` row below for skip-sensitive series. See `benchmarks/PROTOCOL.md`'s raw-format changelog for the full field history. | off |
| `FRUST_NO_RENDER_THREAD` | Kill switch for the render-thread split (`docs/ARCHITECTURE.md`'s frame pipelines) — restores the pre-split single-thread path (rebuild/layout/paint/encode/acquire/present all on the UI/main thread), the fallback if the split needs to be ruled out. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Also the skip-count fix: a `FrameGate` `Skip` sends nothing across the split's UI→render channel, so it is never recorded in `FrameStats`/the raw line — build with this set when a skip-sensitive series (skip counts/rates) needs every skip counted. | off (split active) |
| `FRUST_NO_RESAMPLE` | Kill switch for the mobile pointer-event resampler (`docs/ARCHITECTURE.md`'s `frust-shell-common` row) — forces raw per-touch delivery with no frame-boundary interpolation/prediction. Same compile-time-or-runtime parsing as `FRUST_TRACE`. | off (resampler active) |
| `FRUST_NO_DIRECT_SURFACE` | Kill switch pinning a direct-capable surface onto the blit fallback arm (`docs/ARCHITECTURE.md`'s `frust-render` row) — the A/B valve for comparing the direct-to-surface and blit render paths on the same hardware. Same compile-time-or-runtime parsing as `FRUST_TRACE`. **A/B caveat:** on the direct arm the GPU render moves into `submit_us` (out of `encode_us`) and `acquire_us` now precedes it rather than follows — account for this remap before comparing `submit_us` across arms (`SurfaceRenderer::submit`'s doc comment has the full v3 field mapping). | off (path auto-probed) |
| `FRUST_NO_SHADER_EFFECTS` | Kill switch for the shader-quad pre-pass (`docs/ARCHITECTURE.md`'s `frust-render` row) — disables it entirely, so `Command::ShaderQuad` falls back to the built-in placeholder fill instead of running the pre-pass. Same compile-time-or-runtime parsing as `FRUST_TRACE`. | off (pre-pass active) |

`FRUST_TRACE=1 (cd examples/huddle && cargo run)` prints a `frust-perf
startup ...` line, then periodic `frust-perf frame ...` summaries.

`scripts/size-report.sh [--app <dir>]` (default `examples/huddle`) builds
the arm64-v8a release `.so` via `cargo ndk`, reporting unstripped/stripped
size, an APK/AAB per-ABI `.so`+dex breakdown when Gradle output already
exists, and a desktop `cargo bloat --release -n 20` breakdown when
installed — every missing-tool/artifact path degrades to a printed note.
See `workflow/plans/features/frust-phase-7-performance/research/BASELINE.md`
for recorded baselines.

## Version-Pin Policy

The rendering stack's versions in `[workspace.dependencies]` are pinned
deliberately, not floating. Each row's tripwire must be re-run after
touching that pin:

| Pin | Why | Tripwire |
|---|---|---|
| `vello 0.9.0` / `wgpu 29.0.3` (resolves 29.0.4) | `vello` requires `wgpu ^29.0.3`; bumping `wgpu` independently (30.x is ecosystem-latest) breaks the build — see `docs/spec.md` §8/§15 | `cargo build --workspace --locked` |
| `image =0.25.10` exact (`Image` widget's PNG/JPEG decoder, `png`/`jpeg` only) | Only 0.25.x release whose MSRV equals the workspace `rust-version` (1.88) | fresh MSRV check before bumping, not just `cargo update` |
| `reactive_graph 0.2` / `any_spawner 0.3` / `tokio 1` (no default features), minor | `frust-reactive` substrate (spec §5.5), pre-1.0 Leptos-ecosystem churn expected; never enable `reactive_graph`'s `effects` feature — the frame path is a custom subscriber, not `RenderEffect` (see ARCHITECTURE's Key Types) | `cargo test -p frust-reactive` |
| `accesskit 0.24` minor + adapters (`accesskit_winit 0.33`, `accesskit_android 0.7` minor, `accesskit_ios =0.1.2` exact) | `frust-core`'s semantics-pass vocabulary (spec §9); `accesskit_ios` is younger/less proven, compile-gate only (*Test*) | `cargo test -p frust-core semantics` |
| `vello_cpu =0.0.9` exact | Experimental CPU render tier (`frust-render`'s non-default `cpu-tier` feature), pre-1.0 unstable API, isolated behind the `SceneSink` encode seam so a breaking bump never reaches the default GPU path | tripwire in *Test* |
| `ndk-context 0.1` minor | `frust-plugin`'s Android platform-handle slot (written by `nativeInitPlatform`, read by every plugin) | `cargo check --target aarch64-linux-android -p frust-plugin` |
| `objc2 0.6` / `objc2-foundation 0.3` minor | Apple ObjC bridge (`frust-shared-preferences`'s `NSUserDefaults` backend; `frust-secure-storage`'s apple arm also pulls `objc2-foundation` for `NSString`/`NSError`) | `cargo check --target aarch64-apple-ios-sim -p frust-shared-preferences` |
| `objc2-security 0.3` / `objc2-local-authentication 0.3` minor | `frust-secure-storage`'s Apple Keychain backend + biometric gate (`SecAccessControl`/`LAContext`) | the secure-storage mobile compile gates above |
| `keyring-core 1.0` / `zbus-secret-service-keyring-store 1.0` (`rt-async-io-crypto-rust` feature, keeps the plugin tokio-free) / `windows-native-keyring-store 1.1` minor | `frust-secure-storage`'s desktop Linux/Windows backend | `cargo test -p frust-secure-storage` |
| `notify 8` minor (`frust-cli`-only) | `frust run --watch`'s filesystem watcher (`ctrlc` floats, shared by `frust-drive`/`frust-cli`, unpinned) | `cargo test -p frust-cli` |
| `ratatui 0.30` / `crossterm 0.29` / `ansi-to-tui 8.0.1` minor | `frust-tui`'s render/terminal/log stack, pre-1.0 churn expected | `cargo test -p frust-tui` |
| `toml_edit 0.25` minor | `frust-tui`'s config persistence and `frust-drive::plugin`'s format-preserving Cargo.toml/manifest edits | `cargo test -p frust-tui` && `cargo test -p frust-drive` |

- `examples/huddle` and `plugins/clean-signals-frust` are each a
  **standalone package** (own `[workspace]` root/`Cargo.lock`, excluded from
  the root `[workspace]`; `examples/huddle` additionally standalone for its
  project-local `target/` dir), path-depending on the `clean-signals` core
  crate at the same **sibling checkout** (`../../../clean-signals-rs`,
  branch `master` — not git+rev-pinned, so every consumer must resolve it
  the same way; mixing `path`/`git`+`rev` builds two distinct identities
  whose controller/glue types fail to unify, so swap every consumer
  together once it gains a remote). Neither manifest can use
  `{ workspace = true }` — every dependency is a literal spec kept in sync
  by hand; gate each from its own directory (`cargo test` + `cargo clippy
  --all-targets -- -D warnings`).
- **Never run a blind `cargo update`.** After any pinned-dependency manifest
  change, run `cargo generate-lockfile` then confirm `cargo build
  --workspace --locked` still succeeds before committing.
- Use `cargo tree -d` to check for duplicate/divergent versions of a crate
  across the dependency graph after any manifest change.
- Android deps (`jni`, `ndk`, `ndk-sys`, `android_logger`) are target-gated
  (they only compile for `--target *-linux-android`) but legitimately appear
  in `Cargo.lock` on every platform — expected, not drift. `libc` (unpinned
  `0.2`) is the same shape but two-platform: android's/ios's render-thread
  priority self-boosts (`docs/CODE_STANDARDS.md`'s sanctioned-unsafe zones).

## Known Issues

### Formatting

The codebase is formatted with `rustfmt` using default settings (no
`rustfmt.toml`). `cargo fmt --check` is part of the verify gate above.

### Android emulator GPU on Apple Silicon

An Apple-Silicon Android emulator's default (hardware) GPU path segfaults on
`vkQueueSubmit` in the emulator's gfxstream/MoltenVK Vulkan driver (an
emulator/driver limitation, not a Frust bug). Use a physical device, or boot
with `-gpu swiftshader` (software Vulkan; slower but correct). The older
`swiftshader_indirect` spelling is deprecated by current emulator releases.

### iOS Simulator cannot render (vello 0.9 / wgpu 29)

The iOS Simulator's GPU only exposes the Apple2 Metal feature family,
lacking `wgpu::DownlevelFlags::INDIRECT_EXECUTION`, which vello 0.9's
renderer unconditionally requires — a wgpu-hal-29/vello-0.9 limitation, not
fixable under the version pin. `frust-render` detects it and fails fast
with a clear diagnostic instead of a per-frame panic; `frust run` still
builds/installs/launches, but the window stays black. **Physical iOS
devices are unaffected** (Apple7+ and Apple6/A13 GPUs both render
correctly, verified on an iPhone 13 mini and iPhone SE) — use one for a
pixel-accurate check until a future wgpu/vello upgrade closes the gap.

### Android release build may not pick up `--define`

A `--release` Android build threads `--define`s into the `cargo ndk` compile
via Gradle's `environment(...)` (the mechanism a `--profile`/debug build
uses successfully), but has been observed not to receive them, so a value
like `FRUST_TRACE=1` can be missing from the compiled artifact. Workaround:
export the same key/value pairs in the build shell's environment before
`frust build apk --release`. Fix pending.

### vello bitmap color-emoji decode (desktop confirmed safe; Android CBDT at risk)

vello 0.9's bitmap-glyph decode path (`sbix`/COLR strikes) errors and skips
any glyph whose PNG isn't `(RGBA, 8-bit)` — pinned, unfixed upstream
([linebender/vello#1031](https://github.com/linebender/vello/issues/1031)).
**Safe** for huddle's desktop emoji set (every reaction-emoji's Apple Color
Emoji `sbix` strike is uniformly RGBA8); the **unverified, at-risk** path
is Android's CBDT strikes (`docs/ARCHITECTURE.md`'s `frust-text` row) —
legacy Noto Color Emoji CBDT strikes are known to use palette-indexed PNGs
at smaller sizes, triggering this defect, unconfirmed on any
device/emulator. The desktop logger suppresses this noise below `debug` —
see *Instrumentation*'s `FRUST_LOG` row.
