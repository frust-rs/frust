# Frust - Development Guide

## Prerequisites

- Rust 1.88+ (workspace `rust-version`), edition 2024.
- macOS with Metal for the GPU smoke gate (`frust-render`'s `--ignored`
  test); other platforms can build and run the non-GPU suite.
- **Android** (only needed for `frust run`/`build`/`create`'s Android output):
  `rustup target add aarch64-linux-android`; `cargo install cargo-ndk`
  (tested with 4.x); JDK 17+ on `JAVA_HOME` (Android Studio's bundled JBR is
  auto-detected as a fallback on macOS); `ANDROID_HOME`/`ANDROID_SDK_ROOT`
  and `ANDROID_NDK_HOME` set. `frust doctor` checks all of these.
- **iOS** (only needed for `frust run`/`build`/`create`'s iOS output;
  macOS host only): Xcode 26+ with `xcode-select -p` resolving to it;
  `rustup target add aarch64-apple-ios-sim aarch64-apple-ios`. A booted
  Simulator is enough for a debug `frust run`. A signed build
  (`frust build ios`/`ipa`, `frust run --release`, or any physical-device
  run) needs a codesigning identity — `frust` auto-detects the
  `DEVELOPMENT_TEAM` from `security find-identity`, or it can be set via
  `FRUST_IOS_TEAM` or `[ios] team` in `frust.toml`. A physical iPhone
  run additionally needs iOS 17+ (driven via `devicectl`), the device
  unlocked/paired/trusted, and Developer Mode enabled (Settings → Privacy &
  Security → Developer Mode). `frust doctor` checks the Rust targets on
  macOS hosts only.
- **clean-signals-rs** cloned as a sibling directory (`../clean-signals-rs`
  next to this checkout), on branch `master` — needed only for the
  `clean-signals` core crate itself, consumed by two in-repo dependents:
  `examples/huddle` (the repo's sole example) and the `plugins/clean-signals-frust`
  glue plugin (the glue code itself is in-repo, not sibling). The root
  workspace never needs it. See *Version-Pin Policy* for why. Both
  dependents' verify gates are conditional steps in *Test* below, not part
  of the unconditional chain.
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
launch watchdog, so only this stack is optimized, keeping the rest of a
debug build fast. The three manifests are hand-synced (`cargo test -p
frust-cli --test profile_sync` is the tripwire); a scaffolded project
inherits the template's overrides. A separate `[profile.dev.package."*"]`
wildcard (`opt-level = 1`, a named override still wins) widens every other
non-workspace-member dependency's debug optimization, hand-synced the same
way.

**Release-profile hardening.** `[profile.release]` (root, template, huddle)
sets `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic =
"abort"`, at the default `opt-level = 3` — chosen over `"s"`/`"z"` because a
smaller-opt-level win didn't clear a 5% bar once a render-stack carve-out
protecting encode-path CPU perf was applied (measured via
`scripts/size-report.sh`, below; see the root `Cargo.toml` comment and
`workflow/plans/features/frust-phase-7-performance/research/BASELINE.md`
for the full A/B).

## Run

```bash
# Huddle: the repo's sole example, a Slack-style showcase app (Component
# tree wiring clean-signals ControllerCores to Frust via the
# clean-signals-frust glue plugin) — exercises the framework surface
# end-to-end. `examples/huddle` is a standalone package (its own
# [workspace] root, own Cargo.lock — see Version-Pin Policy below),
# excluded from the root workspace, so it is run and gated from its own
# directory rather than by `-p` from the repo root.
(cd examples/huddle && cargo run)
```

`(cd examples/huddle && cargo run)` is the manual visual gate for
rendering/interaction/theme/navigation/text-input changes — there is no
automated pixel-diff test yet, so a person must look at the window. It
exercises tab navigation with M3 transitions, a channel/feed/thread flow
with shared-element transitions and swipe/long-press actions,
pull-to-refresh and pagination, multiline text input, themed settings, and
toast/undo feedback — plus the check that a background-thread wake (e.g. a
timer-driven completion) renders content with zero mouse movement, not
only on an input-triggered redraw. `examples/huddle`'s own verify gate
(`cargo test` plus clippy, from its own directory) is the conditional step
in *Test* below, gated on the clean-signals-rs sibling checkout.

`examples/huddle` additionally builds and runs on Android and iOS, from
its own directory (its own `frust.toml`, package `it.f0x.huddle`):

```bash
cd examples/huddle
/path/to/frust build apk --debug   # debug APK via Gradle + cargo-ndk
frust run -d <device-id>           # build, install, launch, stream logcat
                                       # (Android device/emulator or iOS Simulator/device)
```

The template `frust create` scaffolds is its own demo (a notes app,
`templates/app/src/lib.rs.tmpl`) with no example counterpart to run directly
in this repo; check scaffold changes via *Template development* below or
`cargo test -p frust-cli --test create_e2e -- --ignored`.

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
defaults to release — see *Release Builds*); with no device selected it
falls back to a streamed `cargo run` (desktop preview — or the watch loop
below, with `--watch`). First Android/iOS builds take a few minutes
(Gradle download / full simulator dependency compile).

`frust run --render-tier <gpu|cpu>` forces the desktop preview's render
tier (`FRUST_RENDER_TIER`, settable directly for a manual `cargo run`); an
explicit choice wins among tiers the adapter supports — forcing `gpu` on an
incapable adapter fails fast with the probe's diagnosis, `cpu` can always
be forced. Desktop-preview-only today; an Android/iOS device run prints a
not-plumbed note and probes its own tier.

**High refresh-rate hints.** A generated app's iOS `CADisplayLink` requests
a 30–120Hz `preferredFrameRateRange`; Android calls `Surface.setFrameRate()`
(API 30+) with the display's max rate. Both are hints, not guarantees —
achieved rate is device/OEM/thermal-state dependent and unverifiable on the
iOS Simulator (see *Known Issues*) or most Android emulators (60Hz-only).

## Dev Loop

```bash
frust run --watch
```

Desktop only: watches `src/` and `Cargo.toml`, killing and relaunching
(`cargo run`, incremental) on change, debouncing a save-burst into one
relaunch; Ctrl-C exits the loop. A **relaunch loop, not state-preserving
hot reload** — app state resets every rebuild. `--watch` + `-d <device>` is
a hard error (device-side watch isn't implemented).

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

**Android release minification.** A generated app's `release` (and
`profile`) Gradle build type runs R8 (`isMinifyEnabled`/`isShrinkResources
= true`) against `proguard-rules.pro` (ships two keep rules: the Frust
JNI surface and the vendored `accesskit_android` delegate); `frust build
apk --debug` is unaffected. NDK r27+ already links `.so`s with 16KB-aligned
`LOAD` segments by default, so no linker-flag change was needed for Android
15's page-size requirement.

## Test

```bash
# Standard verify gate (run before considering any change done):
cargo build --workspace --locked \
  && cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
```

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
workspaces excluded from the root one (see *Version-Pin Policy*). No sibling
checkout? Do not run these commands — record "huddle/clean-signals-frust
gate not run — no clean-signals-rs sibling checkout" instead, and do not
touch either directory without the sibling in place. There is no CI for
this repo, so this doc is the only enforcement. This gate is separate from
`frust build apk`/`run`'s Android/iOS pipeline gate (see *Run*).

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

# iOS compile gate (no device needed; macOS only): the whole facade graph
# must compile for the iOS Simulator target.
cargo check --target aarch64-apple-ios-sim -p frust
cargo check --target aarch64-apple-ios-sim -p frust-shared-preferences
```

The iOS compile gate above is also the only check of the `accesskit_ios`
adapter today — it has not yet been compiled on any host in this repo's CI
history (Linux-only so far). Screen-reader verification (TalkBack on
Android, VoiceOver on iOS) and the `cpu-tier` render tier's visual behavior
on real hardware are not yet verified — both need a device/Simulator with a
screen reader enabled or a physical GPU, which this repo's headless host
cannot provide.

### Deep-link manual test (Android)

A device/emulator gate for `nativeOnDeepLink` (see
`docs/ARCHITECTURE.md#data-flow`'s Deep-link flow), against a project
scaffolded with `frust create --deeplink-scheme <scheme>
[--deeplink-host <host>]` and installed (`frust run -d <device>`):

```bash
# Cold start: app not running — the link launches it and the deep link
# resolves once the native handle exists (queued until then).
adb shell am force-stop <package>
adb shell am start -a android.intent.action.VIEW \
  -d "<scheme>://<path>" <package>

# Warm: app already foregrounded — android:launchMode="singleTop" routes
# this through MainActivity.onNewIntent (not a fresh onCreate).
adb shell am start -a android.intent.action.VIEW \
  -d "<scheme>://<other-path>" <package>
```

Confirm the app navigates to the linked route both times. The iOS
equivalent (`frust_on_deep_link`, delivered via `SceneDelegate`'s
`scene(_:openURLContexts:)`) has a Simulator-only CLI trigger:

```bash
xcrun simctl openurl booted "<scheme>://<path>"
```

A physical device has no CLI trigger — test by tapping a link to the
registered `CFBundleURLSchemes` scheme (e.g. from Notes) instead.

`frust.toml`'s `[deeplink]` section (written by `--deeplink-scheme`/
`--deeplink-host`) is informational only — editing it does not re-render
the Android manifest intent filter or iOS `Info.plist` it documents; use
`--overwrite` or edit the platform files directly to change the scheme.

### Safe-area / keyboard / back manual test (Android + iOS)

A device/emulator gate for the inset and back contracts (see
`docs/ARCHITECTURE.md`'s Inset delivery / Back flow), against an installed
app (`frust run -d <device>`) — no CLI trigger like the deep-link gate
above, so each is a person-driven check:

- **Safe-area:** rotate the device and confirm top/bottom-anchored content
  reflows around the status bar/notch/gesture-nav insets in both
  orientations.
- **Keyboard:** focus a text field near the bottom of the screen and
  confirm surrounding content shifts to stay clear of the on-screen
  keyboard, then dismiss it and confirm the layout returns.
- **Back:** press the hardware/gesture back control on a pushed route and
  confirm it pops one level; at the root route, confirm it falls through
  to the platform's own default (app exit/backgrounding) rather than a
  no-op.
- **Density refresh:** move a running app between displays of different
  density (or a rescaled emulator window) and confirm inset/scale-dependent
  layout rescales rather than sticking to the launch-time density.

### Shared-preferences manual test (desktop + Android + iOS)

A kill-and-relaunch persistence gate for `frust-shared-preferences`
(`plugins/shared-preferences`), against the scaffolded notes app template
(`frust create`'s default `lib.rs.tmpl`, which persists its notes list +
draft through the plugin):

- **Persistence:** add a note (and/or edit the draft), kill the app, relaunch
  it, and confirm the note/draft survived — on the desktop preview (`cargo
  run` from a scaffolded project), an installed Android device, and an
  installed iPhone.
- **Old-scaffold graceful error:** a project scaffolded *before* the plugin
  existed (no `nativeInitPlatform` call on Android) must still boot with
  empty state rather than crash — `SharedPreferences::standard()` surfaces
  a typed `PrefsError::PlatformNotInitialized` in that case, which the
  template's load path catches and falls back to defaults for.
- **macOS storage-location caveat:** the desktop preview is an unbundled
  binary (no `CFBundleIdentifier`), so its `NSUserDefaults` writes land in
  the global defaults domain rather than an app-specific plist — a
  storage-location difference from a bundled app, not a behavioral one (the
  plugin's `frust.`-prefixed keys keep it isolated regardless).

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

## Instrumentation

| Variable | Purpose | Default |
|---|---|---|
| `FRUST_TRACE` | Enables `frust-perf` frame/startup logging (`frust-shell-common::perf`). Runtime env var on any build; `frust run --profile`/`frust build --profile` auto-inject `--define FRUST_TRACE=1` unless already set — opt out of that default with `--define FRUST_TRACE=0`. | off |
| `FRUST_NO_FRAME_GATE` | Kill switch for the mobile whole-frame skip gate (`docs/ARCHITECTURE.md`'s Frame gate) — forces every Choreographer/`CADisplayLink` tick to run, restoring pre-gate behavior. Same compile-time-or-runtime parsing as `FRUST_TRACE`. Reach for this first when diagnosing a suspected stuck-UI report. | off (gate active) |
| `FRUST_LOG` | Desktop-only stderr log level override (`frust-shell-desktop::logger`) — the sink `perf`'s `log::info!` lines print through; Android/iOS use their platform loggers instead. The logger suppresses only known-noisy vello Error/Warn messages below `debug` (see *Known Issues*' vello bitmap-emoji note); unknown vello errors still surface at the default level. Pass `FRUST_LOG=debug` to see all vello log lines when debugging the render stack. | `info` |
| `FRUST_RENDER_TIER` | Forces the desktop preview's render tier (`gpu`/`cpu`) — see *Run* above. | adapter-probed |
| `FRUST_TRACE_RAW` | A second dial beside `FRUST_TRACE`: with both set, `FrameStats` emits one parseable `frust-perf raw ...` line per frame (instead of periodic summaries), plus `bench-scenario-start/end <name>` marker lines for a benchmark harness to slice by. Setting `FRUST_TRACE_RAW` alone does nothing — `FRUST_TRACE` must also be on. Same compile-time-or-runtime parsing as `FRUST_TRACE`. | off |

`FRUST_TRACE=1 (cd examples/huddle && cargo run)` prints one
`frust-perf startup ...` line, then periodic `frust-perf frame ...`
summaries while interacting with the window.

`scripts/size-report.sh [--app <dir>]` (default `examples/huddle`) builds
the arm64-v8a release `.so` via `cargo ndk`, reports its unstripped/stripped
size (auto-discovering the NDK's `llvm-strip`), an APK/AAB per-ABI `.so` +
dex breakdown when Gradle output already exists (no Gradle build
triggered), and a desktop `cargo bloat --release -n 20` breakdown when
`cargo-bloat` is installed — every missing-tool/artifact path degrades to a
printed note rather than failing; only a build failure exits non-zero. See
`workflow/plans/features/frust-phase-7-performance/research/BASELINE.md`
for recorded baselines.

## Version-Pin Policy

The rendering stack's versions in `[workspace.dependencies]` are pinned
deliberately, not floating:

- `vello = "0.9.0"` requires `wgpu ^29.0.3`; the workspace pins
  `wgpu = "29.0.3"` (currently resolving to `29.0.4`) even though `wgpu`
  30.x is the ecosystem-latest release — bumping `wgpu` independently of
  `vello` breaks the build. See `docs/spec.md` §8/§15 for the ecosystem
  status this pin is tracking.
- `image = "=0.25.10"` (the `Image` widget's PNG/JPEG decoder,
  `default-features = false`, `png`/`jpeg` features only) is pinned exact,
  not a caret range: it is the only 0.25.x release whose MSRV is exactly the
  workspace's `rust-version` (1.88), not lower — bumping it needs a fresh
  MSRV check, not just `cargo update`.
- `reactive_graph = "0.2"` / `any_spawner = "0.3"` / `tokio = { version = "1",
  default-features = false }` (the `frust-reactive` substrate, spec §5.5)
  are pinned to minor, not exact — a pre-1.0 Leptos-ecosystem stack expected
  to churn; `cargo test -p frust-reactive` is the tripwire for a breaking
  bump. Never enable `reactive_graph`'s `effects` feature (the frame path is
  a custom subscriber, not `RenderEffect` — see `docs/ARCHITECTURE.md`'s Key
  Types).
- `accesskit = "0.24"` (`frust-core`'s semantics-pass vocabulary, spec §9)
  is pinned to minor; `cargo test -p frust-core semantics` is the tripwire
  for a breaking bump. The three per-shell platform adapters unify on this
  pin: `accesskit_winit = "0.33"` (desktop) and `accesskit_android = "0.7"`
  are pinned to minor; `accesskit_ios = "=0.1.2"` is pinned exact (its 0.1.x
  line is younger/less proven — see *Test* below for its compile-gate
  status).
- `vello_cpu = "=0.0.9"` (the experimental CPU render tier, `frust-render`'s
  non-default `cpu-tier` feature) is pinned exact — pre-1.0 with an unstable
  API, isolated behind the `SceneSink` encode seam so a breaking bump never
  reaches the default GPU path. See *Test* below for its tripwire command.
- `ndk-context = "0.1"` (`frust-plugin`'s Android platform-handle slot,
  written by the Android shell's `nativeInitPlatform`, read by every
  plugin) is pinned to minor; `cargo check --target aarch64-linux-android
  -p frust-plugin` is the tripwire. `objc2 = "0.6"` / `objc2-foundation =
  "0.3"` (the Apple ObjC bridge, `frust-shared-preferences`'s
  `NSUserDefaults` backend) are pinned to minor; `cargo check --target
  aarch64-apple-ios-sim -p frust-shared-preferences` is the tripwire.
- `notify = "8"` (`frust run --watch`'s filesystem-watch dependency,
  `frust-cli`-only like `ctrlc` above) is pinned to minor; `cargo test -p
  frust-cli` is the tripwire for a breaking bump.
- `examples/huddle` and `plugins/clean-signals-frust` are each a
  **standalone package** (own `[workspace]` root and `Cargo.lock`,
  `exclude`d from the root `[workspace]`). Both path-depend on the
  `clean-signals` core crate at the same **sibling checkout**
  (`../../../clean-signals-rs`, branch `master`) — not git+rev-pinned yet,
  so every consumer must resolve it the same way (all by path); mixing
  `path`/`git`+`rev` would build two distinct `clean-signals` identities
  and its controller/glue types would fail to unify, so swap every
  consumer together, never one at a time, once it gains a remote.
  `examples/huddle` is additionally standalone for its project-local
  `target/` dir (generated Android/iOS projects). Neither manifest can use
  `{ workspace = true }` — every dependency is a literal spec kept in sync
  by hand. Gate each from its own directory (`cargo test` + `cargo clippy
  --all-targets -- -D warnings`).
- **Never run a blind `cargo update`.** If a manifest changes any pinned
  dependency, run `cargo generate-lockfile` and then confirm
  `cargo build --workspace --locked` still succeeds before committing.
- Use `cargo tree -d` to check for duplicate/divergent versions of a crate
  across the dependency graph after any manifest change.
- Android deps (`jni`, `ndk`, `ndk-sys`, `android_logger`) are target-gated
  (they only compile for `--target *-linux-android`) but legitimately appear
  in `Cargo.lock` on every platform — that is expected, not drift.

## Known Issues

### Formatting

The codebase is formatted with `rustfmt` using default settings (no
`rustfmt.toml`). `cargo fmt --check` is part of the verify gate above.

### Android emulator GPU on Apple Silicon

An Apple-Silicon Android emulator's default (hardware) GPU path segfaults on
`vkQueueSubmit` inside the emulator's gfxstream/MoltenVK Vulkan driver — an
emulator/driver limitation, not a Frust bug. Use a physical device, or
boot the emulator with `-gpu swiftshader_indirect` (software Vulkan; slower
but correct).

### iOS Simulator cannot render (vello 0.9 / wgpu 29)

The iOS Simulator's GPU only exposes the Apple2 Metal feature family, which
lacks `wgpu::DownlevelFlags::INDIRECT_EXECUTION` — a flag vello 0.9's
renderer unconditionally requires. A wgpu-hal-29/vello-0.9 limitation, not
fixable under the workspace's version pin (see *Version-Pin Policy*);
`frust-render` detects it up front and fails fast with a clear diagnostic
instead of letting vello panic every frame — `frust run` on a simulator
still builds/installs/launches, but the window stays black. **Physical iOS
devices are unaffected** (Apple7+ GPUs expose the flag; verified on an
iPhone 13 mini) — use a physical device for a pixel-accurate check until a
future wgpu/vello upgrade closes the gap.

### vello bitmap color-emoji decode (desktop confirmed safe; Android CBDT at risk)

vello 0.9's bitmap-glyph decode path (`sbix`/COLR bitmap strikes) errors
and skips any glyph whose PNG isn't already `(RGBA, 8-bit)` — pinned,
unfixed upstream ([linebender/vello#1031](https://github.com/linebender/vello/issues/1031),
open as of 2026-07-16; not addressable under the Version-Pin Policy without
vendoring). Confirmed **safe** for huddle's current desktop emoji set: every
reaction-emoji glyph's Apple Color Emoji `sbix` strike is uniformly RGBA8 at
every size. The one **unverified, at-risk** path is Android's CBDT bitmap
strikes (see `docs/ARCHITECTURE.md`'s `frust-text` row) — legacy Noto
Color Emoji CBDT strikes are known in the wild to use palette-indexed PNGs
at smaller sizes, which would trigger this defect; no Android
device/emulator has confirmed either way. If an on-device check finds a
broken glyph, revisit vendoring the one-line `Transformations::EXPAND` fix
before taking a future vello major-version bump. The desktop logger suppresses
known-noisy vello `Error`/`Warn` messages (e.g. "Unsupported `output_color_type`",
"Invalid PNG in font") below `debug` (see *Instrumentation*'s `FRUST_LOG` row),
so a triggered glyph can't spam stderr; other vello errors still surface at the
default level to catch unexpected render issues.
