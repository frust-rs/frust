# ForgeKit - Development Guide

## Prerequisites

- Rust 1.88+ (workspace `rust-version`), edition 2024.
- macOS with Metal for the GPU smoke gate (`forgekit-render`'s `--ignored`
  test); other platforms can build and run the non-GPU suite.
- **Android** (only needed for `forgekit run`/`build`/`create`'s Android output):
  `rustup target add aarch64-linux-android`; `cargo install cargo-ndk`
  (tested with 4.x); JDK 17+ on `JAVA_HOME` (Android Studio's bundled JBR is
  auto-detected as a fallback on macOS); `ANDROID_HOME`/`ANDROID_SDK_ROOT`
  and `ANDROID_NDK_HOME` set. `forgekit doctor` checks all of these.
- **iOS** (only needed for `forgekit run`/`build`/`create`'s iOS output;
  macOS host only): Xcode 26+ with `xcode-select -p` resolving to it;
  `rustup target add aarch64-apple-ios-sim aarch64-apple-ios`. A booted
  Simulator is enough for a debug `forgekit run`. A signed build
  (`forgekit build ios`/`ipa`, `forgekit run --release`, or any physical-device
  run) needs a codesigning identity — `forgekit` auto-detects the
  `DEVELOPMENT_TEAM` from `security find-identity`, or it can be set via
  `FORGEKIT_IOS_TEAM` or `[ios] team` in `forgekit.toml`. A physical iPhone
  run additionally needs iOS 17+ (driven via `devicectl`), the device
  unlocked/paired/trusted, and Developer Mode enabled (Settings → Privacy &
  Security → Developer Mode). `forgekit doctor` checks the Rust targets on
  macOS hosts only.
- **clean-signals-rs** cloned as a sibling directory (`../clean-signals-rs`
  next to this checkout), on branch `develop` — needed only to build/test
  `examples/team-demo`; the root workspace and every other example never
  need it. See *Version-Pin Policy* for why. Its own verify gate is a
  conditional step in *Test* below, not part of the unconditional chain.
- No Docker, CI config, or `.env` setup exists in this repo yet.

## Build

```bash
cargo build --workspace --locked
```

`--locked` must always pass — it is part of the verify gate below and is how
manifest/lockfile drift is caught (see *Version-Pin Policy*).

## Run

```bash
# Desktop preview: opens the example app in a native window.
cargo run -p hello

# Interactive demo: counter with buttons, a checkbox, a slider, and a
# scrolling list — exercises the full event pipeline (see
# docs/ARCHITECTURE.md#data-flow).
cargo run -p counter

# Text-input/IME/image demo: a notes app (TextInput -> submit into a keyed
# list, plus an Image) — exercises focus/keyboard/IME routing and keyed
# reconciliation (see docs/ARCHITECTURE.md#data-flow).
cargo run -p notes

# Async/signals demo: an inbox screen whose Component wires a clean-signals
# ControllerCore (a real async UseCase, with retry) to an RwSignal it renders
# — exercises the reactive substrate end-to-end (signal write -> wake ->
# tracked rebuild, see docs/ARCHITECTURE.md#data-flow's Signal-driven wake).
# `examples/inbox` is a standalone package (its own [workspace] root, own
# Cargo.lock — see Version-Pin Policy below), excluded from the root
# workspace, so it is run and gated from its own directory rather than by
# `-p` from the repo root.
(cd examples/inbox && cargo run)

# Full-app demo: a team roster screen wiring a clean-signals ControllerCore
# (load/retry/rename UseCases) to ForgeKit Components via the
# clean-signals-forgekit glue crate — exercises the same reactive substrate
# as `inbox` but as a complete app (search/filter, per-row rename, error
# banner), with an Android build target below. Also a standalone package
# (own [workspace]/Cargo.lock, excluded from the root workspace — see
# Version-Pin Policy), so it's run and gated from its own directory. The
# first live load intentionally fails once and retries (~200ms backoff)
# before showing rows — expect "Loading team…" for about a second, not a
# hang.
(cd examples/team-demo && cargo run)
```

`cargo run -p hello`/`cargo run -p counter`/`cargo run -p notes`/
`(cd examples/inbox && cargo run)`/`(cd examples/team-demo && cargo run)` are
the manual visual gates for rendering, interaction, text-input, and
async/signals changes respectively — there is no automated pixel-diff test
yet, so a person must look at the window. `notes` is also the demo
`forgekit create` scaffolds (`templates/app/src/lib.rs.tmpl`), so scaffold
changes should be checked against it. `examples/inbox`'s and
`examples/team-demo`'s own verify gates (`cargo test` plus their clippy
lines, run from each example's own directory) are their regression proof:
`inbox`'s is unconditional in the Standard verify gate below; `team-demo`'s
is a separate conditional step gated on the clean-signals-rs sibling
checkout — see Version-Pin Policy (which covers the sibling-checkout
requirement) and *Test* below.

`examples/team-demo` additionally builds and runs on Android, from its own
directory (its own `forgekit.toml`, package `it.f0x.team_demo`):

```bash
cd examples/team-demo
/path/to/forgekit build apk --debug   # debug APK via Gradle + cargo-ndk
forgekit run -d <device-id>           # build, install, launch, stream logcat
```

In a generated project, `forgekit run [-d <device>] [--release|--profile]
[--flavor <name>]` builds and launches on a connected Android
device/emulator (preflight → variant-aware `gradlew assemble<Flavor><Mode>`
(cargo-ndk builds the Rust `.so` for the device's detected ABI) →
`adb install`/`launch` → streamed `logcat` until Ctrl-C; release mode needs
the same signing keystore as `build apk` — see Prerequisites), a booted iOS
Simulator (preflight → `xcodebuild build` at the mode's Xcode configuration
→ `simctl install` → `simctl launch --console-pty`, streamed, until
Ctrl-C), or a physical iPhone (iOS 17+ only: a signed device build →
`xcrun devicectl device install app` → `xcrun devicectl device process
launch --console --terminate-existing`, streamed, until Ctrl-C — failures
hint at unlocking/pairing/Developer Mode). `run` defaults to debug mode
(`forgekit build` defaults to release — see *Release Builds* below). With no
device selected, it falls back to a streamed `cargo run` (desktop preview).
The first Android build downloads Gradle 9.5.x, and the first iOS build
compiles the whole Rust dependency graph for the simulator target — expect
either to take a few minutes.

## Release Builds

```bash
# Android: signed release APK (needs android/key.properties — see Prerequisites)
forgekit build apk --release

# Other Android artifact shapes
forgekit build apk --debug
forgekit build apk --profile
forgekit build apk --release --split-per-abi --target-platform android-arm64,android-x64
forgekit build appbundle --release --build-name 1.2.3 --build-number 7

# iOS: signed device build, unsigned device build, App Store archive
forgekit build ios
forgekit build ios --no-codesign
forgekit build ipa --export-method app-store-connect

# Remove build output (cargo clean + the Android build/.gradle directories)
forgekit clean
```

`--flavor <name>` needs a matching Gradle product flavor / Xcode
scheme+configuration already declared in the generated project.

## Test

```bash
# Standard verify gate (run before considering any change done):
cargo build --workspace --locked \
  && cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check \
  && (cd examples/inbox && cargo test) \
  && (cd examples/inbox && cargo clippy --all-targets -- -D warnings)
```

`examples/inbox` gates from its own directory rather than `-p` from the repo
root because it's a standalone workspace excluded from the root one (see
*Version-Pin Policy*); it needs only GitHub reachability, a bar the
unconditional chain can assume everyone satisfies.

If the clean-signals-rs sibling checkout exists at `../clean-signals-rs`
(branch `develop` — see Prerequisites and *Version-Pin Policy*),
additionally run:

```bash
(cd examples/team-demo && cargo test) \
  && (cd examples/team-demo && cargo clippy --all-targets -- -D warnings)
```

No sibling checkout? Do not run these two commands — record "team-demo gate
not run — no clean-signals-rs sibling checkout" in your completion summary
instead, and do not touch `examples/team-demo` without the sibling in place.
There is no CI for this repo, so this doc is the only enforcement; if CI is
ever introduced, whether it provisions the sibling must be decided
explicitly. Neither example's gate is part of `forgekit build apk`/`run`'s
Android pipeline gate, which is verified separately (see *Run*) rather than
in this chain.

**Manual/gated tests** (not part of the default `cargo test --workspace`
run — each requires local hardware or is slow, and is marked `#[ignore]`
with a reason):

```bash
# GPU smoke test (forgekit-render): needs a real Metal/Vulkan device.
cargo test -p forgekit-render -- --ignored

# Scaffold end-to-end test (forgekit-cli): compiles a freshly generated
# project's full dependency graph (winit/vello/wgpu) — ~30s cold.
cargo test -p forgekit-cli --test create_e2e -- --ignored

# iOS scaffold test (forgekit-cli): scaffolds a project and runs
# `xcodebuild -list` + `plutil -lint` against the generated Xcode project
# (parse-only, no build; needs Xcode on macOS).
cargo test -p forgekit-cli --test create_ios -- --ignored

# Build pipeline end-to-end test (forgekit-cli): scaffolds a project,
# generates a throwaway keystore, and runs `build apk --release` through a
# real Gradle build — needs Android SDK/NDK; ~1 minute.
cargo test -p forgekit-cli --test build_e2e -- --ignored

# Android compile gate (no device needed): the whole facade graph must
# compile for the Android target.
cargo check --target aarch64-linux-android -p forgekit

# iOS compile gate (no device needed; macOS only): the whole facade graph
# must compile for the iOS Simulator target.
cargo check --target aarch64-apple-ios-sim -p forgekit
```

### Template development

`forgekit create` embeds `templates/app/` into the binary at compile time.
To iterate on template files without rebuilding the embedded copy, pass the
hidden, development-only `--template-dir <path>` flag to point at a
filesystem copy of the template tree instead.

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
  default-features = false }` (the `forgekit-reactive` substrate, spec §5.5)
  are pinned to minor, not exact — a pre-1.0 Leptos-ecosystem stack expected
  to churn; `cargo test -p forgekit-reactive` is the tripwire for a breaking
  bump. Never enable `reactive_graph`'s `effects` feature (the frame path is
  a custom subscriber, not `RenderEffect` — see `docs/ARCHITECTURE.md`'s Key
  Types).
- `examples/inbox` is a **standalone package** (its own `[workspace]` root
  and `Cargo.lock`, `exclude`d from the root `[workspace]` in the root
  `Cargo.toml`) specifically so its `git`+`rev`-pinned `clean-signals`
  dependency never enters the root workspace graph/lockfile — the root
  workspace stays registry-only, and the standard verify gate
  (`cargo build --workspace --locked && ...`) never needs GitHub
  reachability. `clean-signals` is never a framework-crate dependency, and
  its rev pin is never floated to a branch/tag. Because it's outside the
  root workspace, `examples/inbox/Cargo.toml` cannot use
  `{ workspace = true }` — every dependency (including the pins shared with
  the root workspace, like `reactive_graph`/`kurbo`/`peniko`) is a literal
  spec kept in sync by hand with the root manifest's
  `[workspace.dependencies]`. Gate it from its own directory:
  `cd examples/inbox && cargo test` (2 async tests) and
  `cd examples/inbox && cargo clippy --all-targets -- -D warnings`.
- `examples/team-demo` is the same standalone-package pattern as `inbox`,
  but with a stricter dependency shape: `clean-signals` AND
  `clean-signals-forgekit` are both path dependencies to a **sibling
  checkout** at `../../../clean-signals-rs` (relative to the example, i.e.
  next to the `forgekit` checkout) on its `develop` branch — neither crate
  is git+rev-pinned yet. Both must resolve `clean-signals` the same way
  (both by path); if one used `git`+`rev` while the other used `path`, Cargo
  would build two distinct `clean-signals` crate identities and the
  controller/glue types (`AsyncState`, `ControllerCore`, `use_controller`,
  `async_view`) would fail to unify. Swap both to `git`+`rev` together, never
  one at a time, once `clean-signals` gains a remote. Gate it from its own
  directory the same way as `inbox`:
  `cd examples/team-demo && cargo test` (headless UI tests + ported unit
  tests) and `cd examples/team-demo && cargo clippy --all-targets -- -D
  warnings`.
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
emulator/driver limitation, not a ForgeKit bug. Use a physical device, or
boot the emulator with `-gpu swiftshader_indirect` (software Vulkan; slower
but correct).

### iOS Simulator cannot render (vello 0.9 / wgpu 29)

The iOS Simulator's GPU only exposes the Apple2 Metal feature family, which
lacks `wgpu::DownlevelFlags::INDIRECT_EXECUTION` — a flag vello 0.9's
renderer unconditionally requires for its working buffers. This is a
wgpu-hal-29/vello-0.9 limitation, not fixable under the workspace's version
pin (see *Version-Pin Policy*). `forgekit-render` detects the missing flag
up front and fails fast with a clear diagnostic instead of letting vello
panic every frame; `forgekit run` on a simulator still builds, installs, and
launches, but the app window stays black and the console logs the adapter
diagnostic. **Physical iOS devices are unaffected** (Apple7+ GPUs expose the
flag; verified rendering on an iPhone 13 mini) — this is a simulator-only
gap, not an iOS-wide one. Use a physical device for a pixel-accurate check
until a future wgpu/vello upgrade closes the gap.
