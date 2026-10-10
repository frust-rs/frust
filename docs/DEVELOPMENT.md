# Frust - Development Guide

This is the shared index — prerequisites, build/run/test gates, benchmarks, instrumentation, the
version-pin *policy*, platform-support floors, known issues; the canonical test-tier, golden-image,
headless GPU and Android emulator runbook is `docs/TESTING.md`. Per-unit device gates, template
work, and the version-pin rows each unit owns live in its spoke (WIDGETS and NATIVE_WIDGETS have
none — everything they need is here):

| Unit | Development spoke | Holds |
|------|-------------------|-------|
| CORE | [CORE_DEVELOPMENT.md](CORE_DEVELOPMENT.md) | `reactive_graph`/`any_spawner`/`tokio`, `clean-signals`, `accesskit` pins |
| RENDER | [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md) | `wgpu`, `image`, `vello_common`/`glifo`, `parley` pins |
| SHELLS | [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md) | deep-link and safe-area/keyboard/back manual tests; `muda`, `windows-sys` pins |
| PLUGINS | [PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md) | shared-preferences, secure-storage, camera, and IAP manual tests; `ndk-context`, `objc2*`, CameraX, OpenIAP, `keyring-core`, `arboard` (shared with the desktop shell) pins |
| CLI | [CLI_DEVELOPMENT.md](CLI_DEVELOPMENT.md) | template development; `notify` pin |
| TUI | [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md) | `ratatui`/`crossterm`/`ansi-to-tui`, `toml_edit` pins |

## Prerequisites

- Rust 1.88+ (workspace `rust-version`), edition 2024.
- A real Metal or Vulkan adapter for the GPU smoke gate (`frust-render`'s `--ignored` test);
  headless Vulkan needs no display server. Other hosts build and run the non-GPU suite.
- **Android** (only for `frust run`/`build`/`create`'s Android output): `rustup target add aarch64-linux-android`; `cargo install
  cargo-ndk`; JDK 17+ on `JAVA_HOME` (Android Studio's bundled JBR auto-detected on macOS); `ANDROID_HOME`/`ANDROID_SDK_ROOT`,
  `ANDROID_NDK_HOME` set — `frust doctor` checks all of these (Rust targets only on macOS hosts).
- **iOS** (only for `frust run`/`build`/`create`'s iOS output; macOS host only): Xcode 26+ resolved by `xcode-select -p`; `rustup target add
  aarch64-apple-ios-sim aarch64-apple-ios`. A booted Simulator suffices for a debug `frust run`; a signed build needs a codesigning identity
  (`frust` auto-detects `DEVELOPMENT_TEAM`, or set `FRUST_IOS_TEAM`/`[ios] team`), and a physical iPhone run needs iOS 17+,
  unlocked/paired/trusted with Developer Mode on.
- **Web** (only needed for browser output): `rustup target add wasm32-unknown-unknown` (no
  RUSTFLAGS/cfg needed on the `wgpu` 30.0.1 pin). Packaging needs host `wasm-bindgen-cli` 0.2.128
  (must equal the `wasm-bindgen` pin) and `wasm-opt` — see `examples/web-gallery/README.md` § Build.
- **clean-signals-rs**: not required to build — `clean-signals` is a crates.io dependency (*Version-Pin Policy*), so a sibling
  checkout (`../clean-signals-rs`) only helps local `[patch]`-override iteration on `clean-signals` itself.
- **`frust-database --features engine-turso`**: needs libclang on the host (pulls
  `bindgen`/`clang-sys`). The default (`engine-sqlite`) build does not — `rusqlite`'s `bundled`
  feature only needs a `cc`-compatible C toolchain.

## Build

```bash
cargo build --workspace --locked
```

`--locked` must always pass — it is part of the verify gate below and is how manifest/lockfile drift
is caught. It is not this workspace's rule alone: eleven workspaces are held to it — this root plus
the ten standalone ones a pin move keeps current — and such a move regenerates all eleven lockfiles
with targeted per-crate updates rather than a blind re-resolve (see *Version-Pin Policy*). **Dev-profile
shader-stack overrides.** The five manifests the `profile_sync` tripwire checks (root `Cargo.toml`,
`crates/frust-drive/templates/app/Cargo.toml.tmpl`, `examples/huddle/Cargo.toml`, `examples/glyph-catalog/Cargo.toml`,
`examples/material3-demo/Cargo.toml`) each carry a `[profile.dev.package.*]` override (`opt-level =
2`) for the shader/render crates: debug-profile (`opt-level = 0`) shader compilation/translation is
slow enough on mobile CPUs to trip the iOS launch watchdog. They stay hand-synced via that tripwire
(`cargo test -p frust-cli --test profile_sync`); a `[profile.dev.package."*"]` wildcard (`opt-level
= 1`) widens every other dependency's debug optimization the same way. Release-profile hardening, Android link flags, the paired Frust-vs-Flutter benchmark suite and the recorded size/build-time baselines live in [PERFORMANCE_BASELINES.md](PERFORMANCE_BASELINES.md).

**No design-system cargo features.** `frust` carries `default = []` (only the tooling opt-ins
`perf-trace`/`devtools`). The built-in design systems (`frust-glyph`/`frust-material`/
`frust-cupertino`) are ordinary sibling plugin crates an app depends on beside `frust`, each
installed via its own `install()` call (see *Run*'s design-system-install gate below). Those crates
do carry one feature of their own: `frust-glyph`, `frust-material` and `frust-shadcn` each have
`bundled-fonts`, default on — an app that sets `default-features = false` on that dependency
compiles in no font bytes and its `install()` registers none, so text falls back to the platform's
system faces through fontique (Roboto on Android). `frust-beui` and `frust-cupertino` are
unaffected. **Widget-authoring test fixtures.** `frust-widgets`' non-default `test-support` feature
compiles in GPU-free container-widget fixtures (`frust_widgets::test_support`) so an out-of-tree
design system can test its own containers too; off by default in a normal app build.

### Build Output Layout

All generated/compiled output for a Frust app lands under one project-relative root, `build/` — never scattered under `android/`, a bare `dist/`, or a project-root `windows/icon.ico` (the pre-migration legacy layout *Migrating an already-scaffolded app to the build/ layout* below replaces):

| Path | Contents |
|---|---|
| `build/rust` | Cargo's target dir (default only — see the `CARGO_TARGET_DIR` note below) |
| `build/android/{app,frust-embedding,<plugin>}` | Each Gradle module's redirected output: `:app`, `:frust-embedding`, one `:frust-<plugin>` per installed plugin |
| `build/android/jniLibs` | Native libs `cargo ndk` stages for Gradle to package |
| `build/android/.gradle` | Project-local Gradle cache (`--project-cache-dir`) |
| `build/ios` | `xcodebuild` derived-data/archive output |
| `build/web` | Browser build output (`wasm-bindgen`/`wasm-opt`) |
| `build/desktop/<macos\|windows\|linux>` | Per-OS bundle; Windows also writes `build/desktop/windows/icon.ico` |

`CARGO_TARGET_DIR` (env) always wins over the scaffold's `.cargo/config.toml` `[build] target-dir = "build/rust"`.
Under an override, `build/rust` stays empty by design — every Frust tool resolves the real directory via `cargo metadata` rather than assuming the default.

## Run

```bash
# Huddle, the sole example: own [workspace]/Cargo.lock, so gate from its dir, not `-p`.
(cd examples/huddle && cargo run)
```

That is the manual visual gate for rendering/interaction/theme/navigation/text-input changes (no
automated pixel-diff test yet, so a person must look at the window) — including the check that a
background-thread wake (e.g. a timer-driven completion) renders content with zero mouse movement,
not only on an input-triggered redraw. **Design-system install gate.** Huddle's appearance settings
expose a four-way `System`/`Material3`/`Cupertino`/`Glyph` toggle; a Glyph dark+light check
(desktop, Android, iOS) plus a reduced-motion pass round out that manual gate — no automated check
exists for either. A shell with no design system installed falls back to `Theme::neutral()`.

**shadcn gallery.** `cargo run -p shadcn-demo` (a root-workspace member, so no standalone gate)
opens the `frust-shadcn` catalog's desktop gallery, the same manual-visual-gate shape as huddle's.
**material3 gallery.** `cd examples/material3-demo && cargo run` (standalone workspace — no `-p`)
opens `frust-material`'s; `frust run -d <device-id>` from there builds/installs/launches it on
Android or iOS.

`examples/huddle` additionally builds and runs on Android and iOS, from its own directory (its own
`frust.toml`, package `it.f0x.huddle`):

```bash
cd examples/huddle
/path/to/frust build apk --debug   # debug APK via Gradle + cargo-ndk
frust run -d <device-id>           # build/install/launch/stream (Android or iOS sim/device)
```

The template `frust create` scaffolds is its own demo (a Material counter: an `App` component hosting a `HomePage` — Scaffold, app bar, FAB — in `crates/frust-drive/templates/app/src/{lib,home_page}.rs.tmpl`)
with no example counterpart to run directly in this repo; check scaffold changes via *Template
development* ([CLI_DEVELOPMENT.md](CLI_DEVELOPMENT.md)) or the scaffold end-to-end test in *Test*.

In a generated project, `frust run [-d <device>] [--release|--profile] [--flavor <name>]` builds and
launches on a connected Android device/emulator (preflight → variant-aware `gradlew
assemble<Flavor><Mode>` via cargo-ndk → `adb install`/`launch` → streamed `logcat`; release needs
the signing keystore, see Prerequisites), a booted iOS Simulator (`xcodebuild build` → `simctl
install`/`launch --console-pty`), or a physical iPhone (iOS 17+: a signed build → `xcrun devicectl
device install app`/`process launch --console --terminate-existing` — failures hint at
unlocking/pairing/Developer Mode). `run` defaults to debug (`build` defaults to release); with no
device selected it falls back to a streamed `cargo run` (desktop preview, or `--watch` below); first
Android/iOS builds take a few minutes.

**`--features <spec>` passthrough** (`run`, `build apk|appbundle|ios|ipa`; repeatable/comma-
splittable; appended after the mode's own feature selection; refused on `build macos|windows|linux`;
the four release lanes additionally refuse a devtools-enabling token) is detailed in
[CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md). **High refresh-rate hints.** A generated app's iOS
`CADisplayLink` requests a 30–120Hz `preferredFrameRateRange`; Android calls
`Surface.setFrameRate()` (API 30+) — both unverifiable on the iOS Simulator or most Android
emulators (60Hz-only).

**TUI workbench.** Bare `frust` in an interactive terminal, or the explicit `frust tui`, opens the
ratatui workbench (scaffold/build/doctor/clean as supervised sessions, a fuzzy command palette, a
bootstrap wizard, Add Plugin — `?` opens the keybinding help overlay). Opening it runs device
discovery, the doctor preflight and the bootstrap report in the invoking directory, and records the
active project (cwd-detected, or a re-stamp of the persisted most-recently-opened one) in
`~/.config/frust/tui.toml`. Both stdin and stdout must be TTYs: a non-interactive bare `frust`
prints help and exits 2; a non-interactive `frust tui` errors cleanly. A per-session perf sparkline
parses `frust-perf` lines once `FRUST_TRACE` is set. `cargo test -p frust-tui` covers engine/render
logic; live-terminal gestures and a fresh-machine bootstrap walk are a **manual gate**, not CI.

## Web

```bash
frust build web [--release]      # wasm32 build -> wasm-bindgen -> optional wasm-opt -> host page
frust run -d web [--no-open]     # the same build, served at http://127.0.0.1:<port> (loopback), then blocks on Ctrl-C
frust create --platforms web     # opt-in web scaffold (crates/frust-drive/templates/app/web.tmpl; absent from the default platform set)
```

`frust doctor` checks the host's wasm32 target, `wasm-bindgen` CLI and `wasm-opt` as non-fatal
validators; its "Web" heading lists only project-dependent preflight rows — neither affects the exit
code. `[web]` `frust.toml` keys and the dev server's containment/security posture:
[CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md). **MANUAL WEB GATE** (browser matrix; no automated pixel
gate exists — a person must look): serve `examples/web-gallery` or a `--platforms web` scaffold per
its README, then run [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md)'s Browser manual gate checklist
(Chrome/Safari/Firefox × WebGPU/WebGL2, plus the IME legs).

## Dev Loop

```bash
frust run --watch
```

A debug `--watch` run — the desktop preview, or `-d <device>` for an Android device or a booted iOS
simulator — watches the workspace graph's path classes (every member and local non-member `src/`,
each package's `Cargo.toml`/`build.rs`, `Cargo.lock`, `frust.toml`, `rust-toolchain*`,
`.cargo/config*`; [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md)) and **hot-patches** each settled
save-burst (100ms debounce) into the running process: `patched in N ms`, app state and PID kept. It
relaunches from a fresh build only on `restart required: <reason>` (a layout change, a framework or
build-input edit, the patch budget — the classes are under `no-hot-reload-restart-is-a-rebuild` in
[LIMITATIONS.md](LIMITATIONS.md)). On the desktop, `--no-hot`, a profile/release build or a
`--features` passthrough keeps the plain kill-and-relaunch loop (`cargo run`, incremental, one
relaunch per burst, state reset; it watches only `src/` + `Cargo.toml`). With `-d <device>` a
non-debug build or `--features` is refused; only `--no-hot` reaches the device relaunch loop.
Kill/relaunch and Ctrl-C exit both reach the whole `cargo run` tree — Unix process-group kill,
Windows `taskkill /T /F` (`frust_drive::process`, shared by every kill path below). Web, a physical
iOS device and other devices are not watched (`--watch` + `-d` refuses them). Measured baselines:
[PERFORMANCE_BASELINES.md](PERFORMANCE_BASELINES.md) § Dev loop.

**TUI equivalents.** `R` restarts the active workbench session the same way — rebuild + relaunch,
never state-preserving. 'Watch: hot patch on save' (`W`, palette, or the run-config checkbox) on a
watched debug session — desktop, an Android device, or a booted iOS simulator — hot-patches each
settled save-burst (100ms debounce, `supervise/watch.rs`, synced with `frust-cli`'s): a
`patched in N ms` toast, and the app relaunches only on `restart required: <reason>`. A session not
launched hot (every device session, or a desktop one toggled with `W` after launch) relaunches
into a hot session on its first watched save; later saves patch. A physical iOS
device's `R` reruns build → install → launch and watch is refused there (mechanism and refusals:
[TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)).

## Release Builds

```bash
# Android: signed release APK (needs resolvable signing material, not just a file's
# existence — the `[signing]` gate in docs/CLI_ARCHITECTURE.md)
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

# Remove build output (cargo clean + build/, see Build Output Layout above)
frust clean
```

`--flavor <name>` needs a matching Gradle flavor or Xcode scheme+configuration already declared in the project.
R8 minification, 16KB alignment, release-lean mode and the debug-signed `frust_bench` size-matrix APK (never distribute it): [PERFORMANCE_BASELINES.md](PERFORMANCE_BASELINES.md) § Release artifacts.

## Test

```bash
# Standard verify gate (run before considering any change done):
cargo build --workspace --locked \
  && cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
```

`frust-drive`/`frust-tui` ride this gate automatically (root workspace members); so do the five
design-system plugin crates (`plugins/{glyph,material,cupertino,shadcn,beui}`); `cargo fmt --check`
is rustfmt's defaults (no `rustfmt.toml`). The root command builds default features only, so the
`bundled-fonts`-off arm (frust-glyph/frust-material/frust-shadcn default it on) is covered by:

```bash
cargo test -p frust-glyph -p frust-material -p frust-shadcn --no-default-features
cargo clippy -p frust-glyph -p frust-material -p frust-shadcn --no-default-features --all-targets -- -D warnings
```

`ScrollView`/`ListView`'s default scroll feel is platform-adaptive
(`frust_widgets::physics::default_physics`): Android → Clamping+Stretch, every other host →
Bouncing+Translate ([WIDGETS_ARCHITECTURE.md](WIDGETS_ARCHITECTURE.md)). A host `cargo test
--workspace` run therefore only exercises the non-Android (Bouncing) arm; the Android arm is
`#[cfg(target_os = "android")]`-gated and compiles only under the Android compile gate below.

Additionally run:

```bash
(cd examples/huddle && cargo test && cargo clippy --all-targets -- -D warnings) \
  && (cd examples/playground && cargo test \
        && cargo clippy --all-targets -- -D warnings && cargo fmt --check) \
  && (cd examples/native-widgets-demo && cargo test \
        && cargo clippy --all-targets -- -D warnings && cargo fmt --check) \
  && (cd examples/design-system-sample && cargo test \
        && cargo clippy --all-targets -- -D warnings && cargo fmt --check) \
  && (cd examples/material3-demo && cargo test \
        && cargo clippy --all-targets -- -D warnings && cargo fmt --check) \
  && (cd plugins/clean-signals-frust && cargo test && cargo clippy --all-targets -- -D warnings)
```

All six gate from their own directory rather than `-p` from the repo root, being standalone
workspaces excluded from the root one (*Version-Pin Policy*) — the same shape `examples/shadertoy`,
`examples/glyph-catalog`, `examples/web-gallery` and `examples/web-spike` gate under, per their own
READMEs (the latter two against the wasm32 target). `huddle` and `clean-signals-frust` take
`clean-signals` from crates.io, so no local sibling checkout is needed. `design-system-sample`
additionally needs `cargo tree -e features -i frust-ui -p sample-app` (from its own directory) to print
**no** `frust-ui feature "..."` line — the out-of-tree-realism check that this workspace resolves
`frust` exactly as a real third-party design-system crate would, with nothing re-enabled by feature
unification. Separate from `frust build apk`/`run`'s pipeline gate (*Run*).

**Non-default features are not compiled by the chain above.** `frust-gpu` and `frust-engine` are
plain (non-feature-gated) dependencies of `frust-render`, so their host-only tests already ride
`cargo test --workspace`; their adapter-pinned real-GPU arms are separate `--ignored` commands — see
[RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)'s GPU Substrate / Golden Oracle Tests sections.
`frust-native-widgets`' `demo-components` is a composite `NativeComponent` demo of real JNI/UIKit/AppKit view construction, shipped inside the plugin rather than in `examples/native-widgets-demo` (whose Composite page merely switches it on) because an app crate cannot implement that trait without raw `jni`/`objc2-ui-kit`/`objc2-app-kit` deps the plugin does not re-export; three builds compile it — `examples/native-widgets-demo`'s standalone gate above (which enables it unconditionally, so it builds and lints on every host), the Android/iOS compile gates below, and the macOS compile gate's darwin check below. Separately, `frust-native-widgets`' own shared runtime/component/mount/demo-lifecycle host tests compile only on a host with **no** platform arm (`#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]`) — so a macOS `cargo test --workspace` run silently skips roughly 50 of them (runtime 26, component 21, `api::mount` 3, demo 3), and Linux/Windows/web are what actually exercise that dispatch/diff/lifecycle contract; see [TESTING.md](TESTING.md).
`frust-database`'s `engine-turso` needs its own gate (`cargo test -p frust-database --features
engine-turso`, libclang required), and `devtools` (the in-app debug service,
[DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)) the same shape: `cargo test -p
frust-shell-common --features devtools` at minimum; `BuildMode::cargo_features()` enables it
alongside `perf-trace` for generated apps' Debug/Profile builds, never Release. `hotpatch`
(opt-in, never default) has two gates, both in CI: `bash scripts/ci/hotpatch-feature-gate.sh`
(`cargo test` + `cargo clippy --all-targets -- -D warnings` for `-p frust-hotpatch`, then the same
pair with `--features hotpatch` by manifest path for `frust-core`, `frust-shell-desktop` and the
facade, e.g. `cargo clippy --manifest-path crates/frust/Cargo.toml --features hotpatch
--all-targets --locked -- -D warnings`) and, on macOS/Linux, `bash scripts/ci/hotpatch-canary.sh`
(the desktop patch builder end to end on the standalone `testing/hotpatch-canary` fixture: fat
build, edit, thin build, L3 layout gate, in-process apply, and a layout-changing edit that must be
refused; the fixture also gates from its own directory like the chain above). Windows-only
hot-patch tests: [TESTING.md](TESTING.md).

```bash
# frust-database mobile compile gates: run separately from the facade-graph gates below
# (this crate's C/bindgen build needs cargo-ndk's CC_*/AR_* passthrough, which a plain
# `cargo check --target` does not supply; the iOS gate needs xcrun, so macOS-only).
cargo ndk -t arm64-v8a check -p frust-database
cargo check -p frust-database --target aarch64-apple-ios
```

**Render golden/oracle tests** (`frust-testing`) ride this gate's CPU arm automatically (root
workspace member); the GPU golden/calibration runs and the material3-demo standalone page-golden
gate are pinned-adapter and standalone-workspace commands respectively — see
[RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md) § Golden / Oracle Tests.

**Manual/gated tests** (outside `cargo test --workspace` — each needs local hardware or is slow, and
is marked `#[ignore]` with a reason):

```bash
# GPU smoke test (frust-render): needs a real Metal/Vulkan device.
cargo test -p frust-render -- --ignored

# Scaffold e2e (frust-cli): compiles a generated project's full graph (winit/wgpu), ~30s cold.
cargo test -p frust-cli --test create_e2e -- --ignored

# iOS scaffold (frust-cli): `xcodebuild -list` + `plutil -lint` on the generated Xcode
# project (parse-only; needs Xcode on macOS).
cargo test -p frust-cli --test create_ios -- --ignored

# Build pipeline e2e (frust-cli): scaffolds a project, generates a throwaway keystore, runs
# `build apk --release` through a real Gradle build — needs Android SDK/NDK; minutes of wall-clock on a cold machine (see the test's doc comment).
cargo test -p frust-cli --test build_e2e -- --ignored

# Macro-diagnostics compile-fail suite (frust-i18n): a generated trybuild project compiled
# against the crate's full default graph in its own target dir (regenerate expectations with
# TRYBUILD=overwrite).
cargo test -p frust-i18n --no-default-features --test macro_diagnostics -- --ignored

# Android compile gate (no device needed): the whole facade graph must compile for Android.
cargo check --target aarch64-linux-android \
  -p frust-ui -p frust-plugin -p frust-shared-preferences -p frust-secure-storage \
  -p frust-camera -p frust-native-widgets -p frust-clipboard -p frust-haptics -p frust-iap -p frust-i18n -p frust-video-player
cargo check --target aarch64-linux-android -p frust-native-widgets --features demo-components

# --all-targets also compiles cfg(test), which the plain checks above never do — without it a
# mobile shell's or frust-iap's own target-gated test module goes uncompiled:
cargo check --all-targets --target aarch64-linux-android -p frust-shell-android -p frust-iap

# iOS compile gate (a type-check, no device/Xcode needed — runs on Linux too; only building
# or running an iOS app needs macOS, see Prerequisites): the whole facade graph must compile
# for the Simulator target (frust-secure-storage also gates the device target).
cargo check --target aarch64-apple-ios-sim \
  -p frust-ui -p frust-shared-preferences -p frust-secure-storage -p frust-camera \
  -p frust-native-widgets -p frust-clipboard -p frust-haptics -p frust-iap -p frust-i18n -p frust-video-player
cargo check --target aarch64-apple-ios -p frust-secure-storage
cargo check --target aarch64-apple-ios-sim -p frust-native-widgets --features demo-components

# Same --all-targets rationale as Android above:
cargo check --all-targets --target aarch64-apple-ios-sim  -p frust-shell-ios -p frust-iap

# wasm compile gate (no browser needed): the facade's whole default graph + web_app! must
# compile for wasm32-unknown-unknown, and so must the web shell crate + gallery registry.
cargo check --target wasm32-unknown-unknown -p frust-ui --tests
cargo check --target wasm32-unknown-unknown -p frust-shell-web -p frust-gallery

# Windows compile gate (cross-check; host-side if mingw-w64 + the rustup target are
# installed, else the containerized recipe below): the shell+facade graph must compile,
# AND the other two per-OS shells must compile inert off-target (the frust-shell-android
# precedent, extended to desktop).
cargo check --target x86_64-pc-windows-gnu \
  -p frust-ui -p frust-shell-windows -p frust-shell-macos -p frust-shell-linux

# macOS compile gate (cross-check; proven from a non-macOS host — objc2/muda are pure
# Rust, no Apple SDK needed). Same inert-off-target coverage as the Windows gate.
cargo check --target aarch64-apple-darwin \
  -p frust-ui -p frust-shell-macos -p frust-shell-windows -p frust-shell-linux
cargo check --target aarch64-apple-darwin -p frust-native-widgets --features demo-components

# Linux: the standard `cargo check --workspace` gate above is native and green on a Linux
# host; on a macOS host run it inside the containerized recipe below instead — a bare Mac
# host fails by construction (`yeslogic-fontconfig-sys` needs a pkg-config sysroot).
```

**Containerized recipe** for the two cross-checks above on a host without mingw-w64 (e.g. devbox):
`docker run --rm -v "$PWD":/src -w /src rust:1-bookworm bash -c 'apt-get update && apt-get install
-y gcc-mingw-w64-x86-64 && rustup target add x86_64-pc-windows-gnu aarch64-apple-darwin && <the two
cargo check commands above>'`.

The iOS compile gate above is also the only check of the `accesskit_ios` adapter today;
screen-reader verification (TalkBack/VoiceOver) needs a device/Simulator and stays unverified on a
headless host. **Running an iOS test, not just type-checking it,** needs a booted Simulator as the
runner: `CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER="xcrun simctl spawn booted" cargo test --target
aarch64-apple-ios-sim -p <crate> [--lib | --test <name>]` — `frust-iap` (`--lib`) and the engine's
Simulator gate (`-p frust-testing --test ios_sim`) use it.

**Neither compile gate above touches Kotlin or Swift.** `cargo check --target
aarch64-linux-android`/`aarch64-apple-ios*` type-checks the Rust `frust-*` graph only, so a Kotlin
error in the embedding or a plugin's Gradle module ships through a green cargo gate undetected. Only
a real Gradle build (`frust build apk --debug`, or the ignored `build_e2e`/scaffold tests above)
compiles Kotlin; compiling Swift needs an Xcode build (macOS only).

### Per-unit device gates

Every person-driven check against an installed app or a real browser lives in its unit spoke (the
table at the top of this guide routes them); none rides the gate chain above.

## Instrumentation

| Variable | Purpose | Default |
|---|---|---|
| `FRUST_TRACE` | Enables `frust-perf` frame/startup logging (`frust-shell-common::perf`); requires a `perf-trace` build (debug/profile compile it in by default) — release compiles the instrumentation out entirely, no code or strings. Runtime env var; `frust run --profile`/`frust build --profile` auto-inject `--define FRUST_TRACE=1` unless already set — opt out with `--define FRUST_TRACE=0`. | off |
| `FRUST_DEVTOOLS` | Runtime kill switch for the in-app debug service (`frust-shell-common::devtools`, [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)): setting it to `0` skips starting the service even in a `devtools`-featured build. Same compile-time-or-runtime shape as `FRUST_TRACE` (the feature gates compilation; the var gates only startup). | unset (service starts if compiled in) |
| `FRUST_NO_FRAME_GATE` | Kill switch for the mobile whole-frame skip gate (`docs/SHELLS_ARCHITECTURE.md`'s `frame_gate` module) — forces every Choreographer/`CADisplayLink` tick to run, restoring pre-gate behavior. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Reach for this first when diagnosing a suspected stuck-UI report. | off (gate active) |
| `FRUST_NO_ANIM_PACING` | Kill switch for animation-loop pacing only (`docs/SHELLS_ARCHITECTURE.md`'s `frame_gate` module) — a paced (`TickClass::CosmeticLoop`) frame request runs on its vsync as before; the whole-frame skip gate (`FRUST_NO_FRAME_GATE` row above) stays active regardless. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Narrower A/B valve than `FRUST_NO_FRAME_GATE` — reach for this when isolating pacing from skip-gate behavior. | off (pacing active) |
| `FRUST_LOG` | Desktop-only stderr log level override (`frust-shell-desktop::logger`) — the sink `perf`'s `log::info!` lines print through; Android/iOS use their platform loggers instead. | `info` |
| `FRUST_WINDOW_SIZE` | Desktop preview-window initial logical size, `<width>x<height>` (`frust-shell-desktop::app_handler`) — strict `^[0-9]+x[0-9]+$`, both ≥ 1; unset/invalid falls back to the default, invalid also logging one `log::warn!` naming the bad value. Compile-time-or-runtime like `FRUST_TRACE`, runtime winning; changing the knob's *value* (not just presence) forces a relink of `frust-shell-desktop` and downstream, so prefer the runtime env on desktop and reserve `--define` for device builds with no runtime env. Effective size/maximized/source logged once. | `800x600` |
| `FRUST_WINDOW_MAXIMIZED` | Desktop preview-window `1`/`true` (case-insensitive) maximizes it at creation, winning visually over `FRUST_WINDOW_SIZE` when both are set. Same compile-time-or-runtime shape as `FRUST_TRACE`. | off |
| `FRUST_TRACE_RAW` | A second dial beside `FRUST_TRACE`, requiring the same `perf-trace` build: with both set, `FrameStats::record` emits one parseable `frust-perf raw ...` line per frame (instead of periodic summaries) — this dial's only remaining job. Setting `FRUST_TRACE_RAW` alone does nothing; `FRUST_TRACE` must also be on. Scenario markers no longer need this dial: `mark_scenario_start`/`mark_scenario_end` (`frust-shell-common::perf`) queue a marker, it rides the scene handoff (`RenderSender::send_scene` → inbox → `drain`, `frust-shell-common::render_split`), and `FrameStats::record` — on the thread that recorded the frame — logs it as `bench-scenario-start/end n=<u64> <name>` next to that frame's own raw line, gated on `FRUST_TRACE` alone; `n` is the recorded frame, not a guess made where the marker was raised. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Raw line format is v4 (adds per-pass GPU-timestamp fields; v3 split `acquire_us`/`submit_us` from v2's single `present_us`); `stats.py` parses key=value so v1–v4 logs stay parseable. In the default render-thread split, a gate-skipped frame never reaches this line — see the `FRUST_NO_RENDER_THREAD` row below for skip-sensitive series. See `benchmarks/PROTOCOL.md` §7 for the raw-format and marker-attribution changelogs. | off |
| `FRUST_PACE_TRACE` | iOS-only render-thread pacing diagnostic (`frust-shell-ios`'s `app::executor::pace_trace`), gated by both the crate's `perf-trace` feature and this dial: emits one `frust-perf ios-pace` line per frame (wake/idle/acquire/submit/loop/p2p microsecond fields), zero-cost when off. Compile-time-or-runtime parsing like `FRUST_TRACE` (any value but `"0"` enables); left off during a measured run so its own line cannot perturb what is being measured. | off |
| `FRUST_NO_RENDER_THREAD` | Kill switch for the render-thread split (`docs/ARCHITECTURE.md`'s frame pipelines) — restores the pre-split single-thread path (rebuild/layout/paint/encode/acquire/present all on the UI/main thread), the fallback if the split needs to be ruled out. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Also the skip-count fix: a `FrameGate` `Skip` sends nothing across the split's UI→render channel, so it is never recorded in `FrameStats`/the raw line — build with this set when a skip-sensitive series (skip counts/rates) needs every skip counted. | off (split active) |
| `FRUST_NO_RESAMPLE` | Kill switch for the mobile pointer-event resampler (`docs/SHELLS_ARCHITECTURE.md`'s `frust-shell-common` kill-switch data flow) — forces raw per-touch delivery with no frame-boundary interpolation/prediction. Same compile-time-or-runtime parsing as `FRUST_TRACE`. | off (resampler active) |

`FRUST_TRACE=1 (cd examples/huddle && cargo run)` prints a `frust-perf startup ...` line, then
periodic `frust-perf frame ...` summaries; on a platform-view page it also emits a rate-limited
`frust-perf platform-view tail depth=...` line from the Android scroll-sync tail. `frust-camera`'s
backends separately `log::debug!` the platform's expected frame-rate range (`FRUST_LOG=debug`). `scripts/size-report.sh` size baselines: [PERFORMANCE_BASELINES.md](PERFORMANCE_BASELINES.md) § Measuring.

## Version-Pin Policy

Pinned versions in `[workspace.dependencies]` (and the Gradle/SPM equivalents) are deliberate, not
floating. **Pins are LAW**: never bump one independently, and re-run that pin's tripwire after
touching it. The pin rows themselves — pin, rationale, tripwire — live with the unit that owns them:

| Pins | Owner |
|------|-------|
| `wgpu`, `image`, `vello_common`/`glifo`, `parley` | [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md) |
| `reactive_graph`/`any_spawner`/`tokio`, `clean-signals`, `accesskit` + adapters, `frust-hotpatch`'s crate-local `libloading`/`memmap2`/`memfd`/`subsecond-types` | [CORE_DEVELOPMENT.md](CORE_DEVELOPMENT.md) |
| `ndk-context`, `objc2*` (Foundation/Security/LocalAuthentication/UIKit/QuartzCore/CoreText/CoreFoundation), `androidx.camera`, `openiap-google`/`OpenIAP`, `keyring-core`, `arboard` (one pin, two consumers — `frust-clipboard` and `frust-shell-desktop`'s clipboard route — so its row carries two tripwires, `cargo check -p frust-clipboard` **and** `cargo check -p frust-shell-desktop`), `fluent-rs`, `icu` (2.2/2.3), `icu_experimental`, `sys-locale` | [PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md) |
| `muda`, `windows-sys` (desktop shells' native menu-bar/Win32 bindings; `objc2-app-kit` rides the objc2 pin family above); web shell tier: `wasm-bindgen` (=0.2.128 exact — must equal the host `wasm-bindgen-cli`), `wasm-bindgen-futures`, `js-sys`/`web-sys`, `web-time`, `console_log`, `console_error_panic_hook`, `wasm-bindgen-test` (=0.3.78 exact — crate-local, bumps in lockstep with `wasm-bindgen`) | [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md) |
| `notify`, `rmcp`, `axum`, `base64`, `tokio-util`, `icns`, the hot-patch builder's `object`/`ar`/`gimli`/`rustc-demangle`/`pdb` (+ `cargo-packager`/`winresource`, external-tool/template-side, not `[workspace.dependencies]`) | [CLI_DEVELOPMENT.md](CLI_DEVELOPMENT.md) |
| `ratatui`/`crossterm`/`ansi-to-tui`, `toml_edit` | [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md) |
| The Rust toolchain itself (`rust-toolchain.toml`: stable 1.98.1 with `rustfmt` + `clippy`) | this section, rule below |

The rules below bind every pin, wherever its row lives:

- **The toolchain is a pin too.** `rust-toolchain.toml` names the exact stable release the gates
  run on; rustup installs it on first use. Bump it in its own change, only after `cargo clippy
  --workspace --all-targets -- -D warnings` and `scripts/ci/hotpatch-canary.sh` (*Test*) pass on
  the candidate — a stable auto-update once turned new lints on under untouched code and failed
  the gate everywhere, and the hot-patch builder relies on rustc/cargo/linker output shapes that
  are stable in practice, not by contract.
- `examples/huddle` and `plugins/clean-signals-frust` are each a **standalone package** (own
  `[workspace]` root/`Cargo.lock`, excluded from the root `[workspace]`), depending on the
  `clean-signals` core crate from crates.io (see
  [CORE_DEVELOPMENT.md](CORE_DEVELOPMENT.md)) rather than a path dep — every consumer, including
  `crates/frust-drive/templates/app`'s clean-signals scaffold variant, must declare the identical version requirement,
  or Cargo builds two distinct crate identities; change all three sites together. Neither manifest can use `{ workspace = true }`;
  gate each from its own directory (*Test*'s standalone chain above).
- **Never run a blind `cargo update`.** After any pinned-dependency manifest change, run
  `cargo generate-lockfile`, then confirm `cargo build --workspace --locked` succeeds **before
  committing**. Removal-only changes — nothing added, no range widened — may use `cargo build`
  instead (prunes orphaned entries); `cargo generate-lockfile` is for adding/widening; review the diff.
  **Carve-out for a duplicate-collapsing bump.** When a pin moves specifically to collapse a
  duplicate (e.g. `parley` 0.11.1 aligning `skrifa`/`read-fonts` onto `glifo`'s copies),
  `cargo generate-lockfile` is the wrong tool — it re-resolves unrelated crates. Run `cargo update
  -p <crate> --precise <version>` from inside each of the eleven workspaces, then a plain
  `cargo metadata` there to pick up missing path-dep edges, and audit every lock diff's
  `name`/`version` lines.
- **wgpu's GPU backends route per target; the `30.0.1` pin stays workspace-owned.** The
  `[workspace.dependencies]` row carries only `std`/`parking_lot`/`wgsl`; `crates/frust-gpu`'s
  `[target.'cfg(...)'.dependencies]` tables add `metal` (apple), `vulkan` (other unix: Android, Linux)
  and `dx12`+`vulkan` (windows). Every wgpu user (`frust-engine`, `frust-render`, `frust-testing`,
  `frust`'s `gpu` feature) depends on `frust-gpu` unconditionally, so it is the single choke point —
  never add a backend back to the row (it re-unions across every target). Moving features between row
  and tables is not a bump, but target-only crates can still enter `Cargo.lock` (the wasm arm added
  seven), so review the lock diff; verify with `cargo tree -e features --target <triple> -p frust-gpu`.
- Use `cargo tree -d` after any manifest change, to catch duplicate/divergent versions of a crate.
  Every workspace resolves with Cargo resolver 3: a fresh resolve prefers versions whose MSRV fits `rust-version`.
- Android deps (`jni`, `ndk`, `ndk-sys`, `android_logger`) are target-gated (`--target
  *-linux-android`) but appear in `Cargo.lock` on all platforms (expected, not drift). `libc`
  (unpinned `0.2`) is the same two-platform shape; android's/ios's render-thread priority
  self-boosts (`docs/CODE_STANDARDS.md`'s sanctioned-unsafe zones). The `jni` row (`"0.22"`)
  cannot drop to 0.21: `android-activity`, winit's Android backend, resolves 0.22.4, and
  `frust-plugin`'s bridge uses 0.22's scoped attachment (`JavaVM::attach_current_thread_for_scope`).
  A second copy, `jni 0.21.1`, rides `accesskit_android` until that crate moves.

## Platform-Support Policy

The minimum supported platform is a **project-wide constant, not a per-module choice**. Every module
must declare the same floor; the numbers are repeated in an in-file comment at each site. **A
mismatch fails in one of two ways, depending on direction** (both verified empirically):

- **App below library → hard build error.** AGP's manifest merger refuses it
  (`uses-sdk:minSdkVersion 24 cannot be smaller than version 26 declared in library
  [:frust-embedding] …`); `:app:processDebugMainManifest` fails and nothing is produced.
- **Library below app → silent behaviour change.** No error; the library just misses APIs it could
  have used, exactly as `IME_FLAG_NO_PERSONALIZED_LEARNING` did (below).

| Platform | Floor | Declared in |
|----------|-------|-------------|
| Android | **`minSdk = 26`** (Android 8.0) · `compileSdk = 36` | 15 Gradle files: `crates/frust-shell-android/platform/android/frust-embedding`, `plugins/{auth-session,camera,iap,native-widgets,secure-storage,video-player}/platform/android`, `crates/frust-drive/templates/app/android.tmpl/app`, and the 7 example/benchmark apps |
| iOS | **15.0** | `crates/frust-shell-ios/platform/ios/FrustEmbedding/Package.swift` (`.iOS(.v15)`) and each app's `IPHONEOS_DEPLOYMENT_TARGET` |
| macOS | **11.0** (`LSMinimumSystemVersion`, `[macos] minimum-system-version` overridable per app) | Three separate literals, none referencing another: `crates/frust-drive/src/manifest.rs`'s private `DEFAULT_MACOS_MINIMUM_SYSTEM_VERSION` (the manifest-parsed default, read back at build time); `crates/frust-drive/src/scaffold/context.rs`'s own `pub` const of the same name (the scaffold-time default that fills `{{ macos_minimum_system_version }}` in `crates/frust-drive/templates/app/macos.tmpl/Info.plist.tmpl`); and a hard-coded `<string>11.0</string>` test literal in `crates/frust-drive/src/scaffold/mod.rs`. This trio needs the same lockstep discipline as the Android table below but none of the three sites carries the in-file lockstep comment yet — treat that as open follow-up work |
| Windows | **10**, de facto — winit itself supports 7+ and only tests 10 regularly | Not an in-repo literal. The floor comes from the Win10-era API `frust-shell-windows` actually drives: winit's `Window::set_theme` (native titlebar light/dark theming) reaches DWM immersive dark-mode support, which requires Windows 10 1809+ |
| Linux | No distro floor; a GPU adapter able to run `frust-engine`'s ordinary (no-compute) render passes | Not declared per-distro anywhere in this repo. `crates/frust-gpu/src/context.rs` requests `Backends::from_env().unwrap_or_default()`, but there is no live GL fallback behind that request: `gles` is deliberately never compiled in for any target (Version-Pin Policy's per-target `wgpu`-backend bullet — Linux routes through the `vulkan`-only arm), so a Vulkan-less host has no backend to fall back to. The WebGL2/GLES3.0 ceiling itself is covered separately, by the engine's downlevel design rehearsing it against a real desktop adapter via `FRUST_ENGINE_DOWNLEVEL` ([RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)) — a materially lower bar than the deleted vello-classic tier — and Linux support remains an untested assumption while the desktop runtime gate is owed (see [LIMITATIONS.md](LIMITATIONS.md) `desktop-shells-runtime-unverified`) |

**Adding a new Android module?** Copy the floor and the lockstep comment. **Adding a new iOS
target?** `Package.swift`'s `platforms:` must stay **at or below** every consumer's
`IPHONEOS_DEPLOYMENT_TARGET` — a package minimum above the app's is a compile error.

### Why Android is 26 and must not go lower

API 24/25 (Android 7.x) sit below the floor because **`EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING`
is API 26+**: it is an `imeOptions` bit, so the constant inlines at compile time and an older IME
simply ignores it — no compile error, no runtime crash, just the keyboard-learning half of FINDINGS
#31's mitigation silently doing nothing. At 26 the flag is unconditionally honoured and
`frust-core/src/event.rs`'s IME contract table holds at the floor.

**Lowering the floor below 26 re-opens that hole silently.** If it is ever lowered, restore the
API-caveat entry in `docs/LIMITATIONS.md` in the same change. Android Lint flags the shape
(**`InlinedApi`**) but **no gate runs Android Lint at all** — wiring `lintDebug` in, and deciding
whether `InlinedApi` is an error here, is open work (*Verifying a floor change* below).

**One** API the floor unlocks is deliberately **not** adopted (`SeekBar.setMin`: native-widgets'
slider keeps its Rust-side range mapping), and two it does not unlock still need their workarounds —
`Font.Builder(ByteBuffer)` (API 29, so the Android typeface path still writes a cache file) and
`BiometricPrompt` (API 28+, so `secure-storage`'s `NotAvailable(UnsupportedApiLevel)` gate stays
reachable on 26/27).

### Migrating an already-scaffolded app

An app scaffolded against an older floor carries `minSdk = 24`. **The bump is mandatory** — a
scaffolded app consumes the embedding as a Gradle *project* dependency, so an app at 24 against the
26 library fails the manifest merger outright. Set `minSdk = 26` in `app/build.gradle.kts`; there is
no other migration step. **Verifying a floor change:** `cargo` sees none of this — from an app dir:

```
./gradlew :app:processDebugMainManifest   # catches an app-below-library mismatch (hard error)
./gradlew compileDebugKotlin lintDebug    # catches API usage above the floor, as InlinedApi/NewApi
```

`lintDebug` reports API-above-floor usage at **warning** severity, so a green exit code does **not**
mean clean — read the SARIF/HTML report under `build/reports/`, or raise the severity, before
concluding anything.

### Migrating an already-scaffolded app to the build/ layout

An app scaffolded before the `build/` root migration still writes Gradle/Xcode/desktop output into its own source tree; `frust build`/`frust run` keep working against that layout (Android falls back to it read-only, with a one-time warning), but migrating gets every artifact under the one root `frust clean` removes wholesale:

1. Add `.cargo/config.toml` with `[build]` / `target-dir = "build/rust"`.
2. `android/settings.gradle.kts`: replace the `:frust-embedding`-only `buildDirectory` redirect with the `when (path)` block redirecting root, `:app`, `:frust-embedding`, and every `:frust-<plugin>` module — copy the block from a fresh `frust create`.
3. `android/app/build.gradle.kts`: point cargo-ndk's `-o` at `../../build/android/jniLibs`, and replace the main source set's `jniLibs` with `sourceSets.getByName("main").jniLibs.setSrcDirs(listOf("../../build/android/jniLibs"))`.
4. `ios/Runner.xcodeproj/project.pbxproj`: replace the `cp "target/$TRIPLE/..."` line with the `TARGET_DIR` resolution (`CARGO_TARGET_DIR` or `cargo metadata --format-version 1 --no-deps`) from a fresh scaffold.
5. `windows/build.rs`: read `build/desktop/windows/icon.ico`, not the project-root `windows/icon.ico`.
6. `.gitignore`: track `/build` (keep `/target`).
7. Run `frust clean` once — it also removes the legacy paths (`android/app/build`, `android/build`, `android/.gradle`, `android/app/src/main/jniLibs`, `dist/`).

Until step 7 runs, `frust build`/`frust run` print the one-time legacy-layout warning, and a stale `android/app/src/main/jniLibs` would otherwise ship packaged alongside the migrated output. See [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md) § "Migrating an already-scaffolded app to edge-to-edge" for the separate Android theme migration.

## Known Issues

### Android emulator GPU on Apple Silicon

An Apple-Silicon Android emulator's default (hardware) GPU path segfaults on `vkQueueSubmit` in the
emulator's gfxstream/MoltenVK Vulkan driver — an emulator/driver limitation, not a Frust bug. Use a
physical device, or boot with `-gpu swiftshader` (software Vulkan; slower but correct).

### iOS Simulator uniform-buffer alignment clamp

The Simulator's GPU (Apple2 Metal feature family) renders correctly under `frust-engine`, which
requires neither `COMPUTE_SHADERS` nor `INDIRECT_EXECUTION` (`ENGINE_REQUIRED_DOWNLEVEL_FLAGS` is
deliberately empty). One residue remains: the Simulator's Metal validation misreports its own
uniform-buffer offset alignment, so a device request there is forced back up to 256 bytes
(`frust-gpu::context`'s `effective_limits`), kept until upstream
[gfx-rs/wgpu#10189](https://github.com/gfx-rs/wgpu/pull/10189) ships (iPhone 13 mini and SE, like
every physical device, are unaffected).

### Android release build may not pick up `--define`

A `--release` Android build threads `--define`s into the `cargo ndk` compile via Gradle's `environment(...)`, but has been observed to drop them (e.g. `FRUST_TRACE=1` missing from the artifact). Workaround: export the same key/value pairs in the build shell first.

### macOS `DYLD_LIBRARY_PATH` shadows ImageIO's codecs, crashing any image decode

A `DYLD_LIBRARY_PATH` naming a directory that also holds Homebrew's `libpng`/`libjpeg`/`libtiff`/`libgif` dylibs (e.g. `/opt/homebrew/lib`) makes dyld replace ImageIO's own private PNG/JPEG/TIFF/GIF/JPEG-2000/Radiance codec dylibs by leaf-name match — case-insensitive on APFS — leaving ImageIO's `__cg_*`-prefixed imports unbound; a bare `cargo run` then SIGBUSes at `0xbad4007` on the first image decode (`-[NSImage initWithData:]`/`CGImageSourceCopyPropertiesAtIndex`), no `NSApplication` required to trigger it, and `DYLD_FALLBACK_LIBRARY_PATH` does not help (only consulted when the real path is missing). `frust-native-widgets`' macOS `Image` control checks for the shadow first and degrades to an empty view plus one warning instead of crashing (`plugins/native-widgets/src/controls/image.rs`'s `shadowed_codec`); nothing else in this repo guards against it. A launcher whose dyld prunes `DYLD_*` (`nohup`, `env`, any SIP-protected binary) never reproduces it, which is why the symptom can look intermittent from an interactive shell that exports the variable globally. Fix: do not export `DYLD_LIBRARY_PATH` in a shell profile; scope it per-invocation, or strip it with `env -u DYLD_LIBRARY_PATH <cmd>`.

### Bitmap-strike color emoji do not render on the engine (COLR emoji do)

`frust-engine` decodes COLRv1 colour glyphs only; PNG/BGRA/Mask bitmap-strike glyphs (pinned
upstream in glifo 0.3.0) go missing rather than landing wrong. **Safe** for huddle's desktop emoji
set (bundled fonts ship COLR); **at-risk**: Android's CBDT-strike system emoji. See
[LIMITATIONS.md](LIMITATIONS.md)'s `engine-bitmap-glyphs-gap` for evidence and removal trigger.

## Website

Site source: [frust-rs/website](https://github.com/frust-rs/website), cloned at `apps/website` (gitignored, never a submodule — see `.gitignore`). It's a Docusaurus site whose gates run in Docker since this toolchain has no Node; `apps/website/CONTRIBUTING.md` is the canonical gate definition — install (`pnpm install --frozen-lockfile`), typecheck (`pnpm typecheck`), lint (`pnpm lint`), and build (`pnpm build`), each via `docker compose run --rm dev`. The production image is compose's `web` service: `docker compose up --build web` (serves `:8080`; image `ghcr.io/frust-rs/website:local`). Content is authored there and cites frust files at a pinned SHA; `docs/` here stays contributor documentation under `DOC_POLICY.md` budgets and is not mirrored. Brand assets: `docs/assets/branding/` (source of truth, copied by the site). API reference tree: `scripts/api-docs.sh` (rustdoc for the root workspace). Widget preview PNGs: `scripts/widget-snapshots.sh` (the `frust-testing` `widget-snapshots` CPU-oracle generator over `examples/gallery`'s case registry — see `docs/TESTING.md`), rsynced into `static/preview/`.
