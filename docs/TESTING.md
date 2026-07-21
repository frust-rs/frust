# Frust - Testing Guide

## Purpose

This document is the canonical testing policy and runbook for Frust. It
defines which behavior each test tier owns, which gates are required for a
change, how visual baselines are produced, and how the headless Linux GPU host
is used. Build, run, release, and dependency-pin instructions remain in
`docs/DEVELOPMENT.md`; benchmark fairness and measurement rules remain in
`benchmarks/PROTOCOL.md`.

Frust has three materially different rendering environments. They must not be
collapsed into a single golden-image suite:

1. **CPU/headless:** deterministic framework and raster coverage without a
   display or GPU.
2. **Native GPU/headless:** Frust scene -> Vello -> wgpu -> Vulkan texture ->
   readback, with no X server, window, surface, or swapchain.
3. **Platform end-to-end:** Android/iOS owns the window, compositor, lifecycle,
   input, accessibility, fonts, density, and system UI. Screenshots from this
   tier have their own platform baselines.

The test strategy is defense in depth: inexpensive semantic tests explain most
failures; pixel tests catch visual regressions; platform tests catch integration
behavior that an offscreen texture cannot model; physical devices remain the
authority for hardware-specific behavior.

---

## Test Tiers

| Tier | Environment | Primary ownership | Default cadence |
|---|---|---|---|
| T0 | Pure host process | Math, state machines, parsing, reconciliation, layout invariants | Every change |
| T1 | Headless `RenderRoot`, no rasterizer | View/widget behavior, input, semantics, scene command construction | Every change |
| T2 | `vello_cpu`, no GPU | Deterministic raster goldens and reference output | Every visual change |
| T3 | Native Vulkan/Metal GPU, no display | Real Vello/wgpu encode, shader, texture, and readback behavior | GPU runner; every visual/render change |
| T4 | Desktop window or virtual display | Surface lifecycle, DPI, IME, wake/redraw, presentation | Relevant changes; scheduled smoke |
| T5 | Android emulator | APK/ABI/JNI, `ANativeWindow`, Vulkan/gfxstream, lifecycle, input, screenshots | Relevant changes; nightly full matrix |
| T6 | Physical Android/iOS | Driver/OEM behavior, biometrics, accessibility, refresh rate, thermal/performance | Release candidate and platform changes |
| T7 | Tooling and packaging | CLI, drive cores, TUI, scaffolds, Gradle/Xcode, plugin mutation | Every relevant change |
| T8 | Non-functional | Performance, startup, memory, fuzzing, sanitizers, coverage, dependency policy | Scheduled and release gates |

T0-T1 tests should make visual failures diagnosable. A golden should not be the
only proof that a button fires, a route pops, an IME state reconciles, or a
surface-loss transition is legal.

---

## Current Coverage

The repository already contains substantial non-pixel coverage:

- Unit tests across every framework layer, with the largest suites in
  `frust-widgets`, `frust-drive`, `frust-tui`, and `frust-core`.
- `examples/huddle/tests/`: GPU-free screen, workflow, input, navigation,
  async, and architecture integration tests driven through `RenderRoot`.
- `crates/frust/tests/`: facade-level interaction and text-input integration.
- `crates/frust-tui/tests/snapshots.rs`: terminal rendering snapshots using
  ratatui's `TestBackend` and `insta`.
- `crates/frust-render`: scene-to-Vello conversion, surface lifecycle,
  pipeline-cache, tier-selection, CPU-tier, and adapter-workaround tests.
- `crates/frust-render/tests/gpu_smoke.rs`: one ignored real-GPU offscreen
  render/readback smoke test.
- `frust-cli` and `frust-drive`: command construction, project mutation,
  device selection, preflight, process supervision, and ignored scaffold/build
  end-to-end tests.
- Plugin conformance tests using file/fake backends, with platform secret-store
  and biometric behavior kept as explicit hardware gates.
- `benchmarks/`: deterministic scenario logic, harness parser tests, and the
  S1-S8 cross-framework device protocol.

Known gaps are tracked by the comprehensive-testing feature plan:

- No reusable native headless RGBA rendering API.
- No versioned raster golden corpus for framework widgets or Huddle screens.
- No automated PNG comparison, diff artifact, or controlled baseline-update
  workflow.
- Text goldens currently depend on installed system fonts.
- No automated Android emulator provisioning, launch, state normalization,
  screenshot comparison, or lifecycle matrix.
- No committed CI configuration or dedicated self-hosted GPU-runner workflow.
- Accessibility and several platform plugin paths remain manual physical-device
  gates.

---

## Required Host Gate

The standard host gate is defined in `docs/DEVELOPMENT.md` and remains the
minimum before a change is complete:

```bash
cargo build --workspace --locked \
  && cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
```

When `../clean-signals-rs` exists at the required branch, also run the Huddle
and clean-signals-frust gates from their standalone workspace roots:

```bash
(cd examples/huddle && cargo test) \
  && (cd examples/huddle && cargo clippy --all-targets -- -D warnings) \
  && (cd plugins/clean-signals-frust && cargo test) \
  && (cd plugins/clean-signals-frust && cargo clippy --all-targets -- -D warnings)
```

If the sibling checkout is absent, record that these conditional gates were not
run. Do not silently treat the root workspace as coverage for them.

The experimental CPU render tier is outside the default feature set:

```bash
cargo test -p frust-render --features cpu-tier
```

Target compile gates and slow ignored scaffold/build tests remain listed in
`docs/DEVELOPMENT.md`.

---

## Native Headless GPU Rendering

### Why X Is Not Required

Vello renders with compute shaders into a storage texture. A native headless
test creates a wgpu instance and adapter without a compatible surface, renders
to `Rgba8Unorm`, copies the texture into a map-readable buffer, removes row
padding, and optionally encodes a PNG. It never creates a `wgpu::Surface` or
swapchain.

This is the same architecture as Vello's
[headless example](https://github.com/linebender/vello/blob/main/examples/headless/src/main.rs).
wgpu provides
[`InstanceDescriptor::new_without_display_handle_from_env`](https://docs.rs/wgpu/latest/wgpu/struct.InstanceDescriptor.html),
and Vulkan compute does not require presentation machinery. A GPU, working
kernel driver, Vulkan loader/ICD, and device-node permissions are required; a
monitor, desktop session, Xorg, and Xvfb are not.

### Preflight

Run these from the same Unix user and service environment that will execute the
tests:

```bash
nvidia-smi
vulkaninfo --summary
test -r /dev/nvidia0
test -r /dev/dri/renderD128 || true
```

`vulkaninfo` printing `DISPLAY environment variable not set; skipping surface
info` is expected. Failure to enumerate the NVIDIA physical device is not.

On the current dual-GPU host, both the NVIDIA T400 and Intel UHD 770 are valid
Vulkan adapters. Golden runs must select one deliberately. The strongest Linux
isolation is to expose only the NVIDIA ICD:

```bash
env -u DISPLAY -u WAYLAND_DISPLAY \
  VK_DRIVER_FILES=/usr/share/vulkan/icd.d/nvidia_icd.json \
  WGPU_BACKEND=vulkan \
  cargo test -p frust-render --test gpu_smoke -- --ignored --nocapture
```

NVIDIA PRIME offload is an alternative when the loader must retain all ICDs:

```bash
__NV_PRIME_RENDER_OFFLOAD=1 \
__VK_LAYER_NV_optimus=NVIDIA_only \
WGPU_BACKEND=vulkan \
cargo test -p frust-render --test gpu_smoke -- --ignored --nocapture
```

NVIDIA documents `__VK_LAYER_NV_optimus=NVIDIA_only` as the way to restrict a
Vulkan application's visible devices to NVIDIA GPUs in a PRIME configuration:
[PRIME Render Offload](https://download.nvidia.com/XFree86/Linux-x86_64/495.44/README/primerenderoffload.html).

`WGPU_ADAPTER_NAME` only affects code that calls wgpu's environment-aware
adapter initializer. The current `gpu_smoke` test calls `request_adapter`
directly, so use ICD/layer isolation until that test adopts the shared Frust
adapter-selection path.

### GPU Run Metadata

Every promoted GPU baseline and failing artifact must record:

- Git commit and dirty-state marker.
- Rust toolchain, target triple, and build profile.
- Vello, wgpu, and image-decoder versions.
- GPU name, PCI ID, driver version, Vulkan API/driver IDs, and ICD path.
- Image dimensions, pixel format, antialiasing mode, and render-tier choice.
- Font bundle identity, locale, theme, scale factor, and animation time.
- Test name, golden schema version, and comparison thresholds.

Do not silently refresh baselines after a driver, Vello, wgpu, shader, font, or
color-space change. Review that migration as an intentional visual change.

---

## Golden Image Policy

### Golden Classes

Store independent baselines for:

- `cpu/`: deterministic `vello_cpu` reference images.
- `vulkan-nvidia-t400/`: real Vello/wgpu output on the pinned T400 runner.
- `android-emulator-api36-host/`: composed Android screenshots using host GPU.
- `android-emulator-api36-swiftshader/`: diagnostic software-GPU screenshots.
- Physical-device families only when a stable, owned device is part of the
  release fleet; never pretend one device represents every Android GPU/OEM.

CPU and GPU output are not required to be byte-identical. Native offscreen and
Android screenshots are never cross-compared: Android adds density, platform
fonts, surface composition, system bars, color management, and potentially a
different Vulkan implementation.

### Deterministic Inputs

A golden case must fix all inputs that can affect pixels:

- Viewport in logical and physical pixels and the scale factor.
- Light/dark theme, design language, accent, locale, text direction, and font
  scale.
- Animation/frame time; use explicit timestamps rather than wall clock.
- App state, random seed, data fixtures, image bytes, and network results.
- Pointer/focus/pressed/disabled/error/loading state.
- Safe-area and keyboard insets.
- Font files and fallback order.
- Base color, texture format, and Vello antialiasing configuration.

Frust currently resolves `SystemUi` through the platform font collection.
Comprehensive text goldens require a test-only bundled-font registration path.
Until that exists, system-font goldens are runner-local and must record the
installed font package/version; they are not portable reference images.

### Comparison

Each comparison reports, at minimum:

- Mismatched-pixel count and percentage.
- Maximum and mean absolute channel error.
- Bounding box of changed pixels.
- Expected, actual, and amplified-diff PNG artifacts.

Use exact comparison for deliberately integer-aligned solid geometry on the
same CPU backend. Use reviewed, fixed tolerances for antialiased edges, text,
gradients, and GPU output. Thresholds belong to the golden class or named test,
not an ad hoc retry path. A test must fail deterministically when its threshold
is exceeded.

### Baseline Updates

Baseline updates must be explicit, for example `UPDATE_GOLDENS=1`; normal test
runs must never write expected files. A baseline change is reviewed with its
expected/actual/diff artifacts and a reason. Store PNGs and compact metadata,
not opaque driver caches. Never promote output from a machine that failed the
adapter/font/environment preflight.

---

## Coverage Catalog

The golden catalog should cover representative states without replacing
semantic tests:

| Area | Required visual cases |
|---|---|
| Primitives | Solid/gradient fill, stroke, path, arc, rounded rect, clip, layer alpha, transform nesting, shadow |
| Text | Latin, RTL/bidi, CJK, combining marks, emoji/color emoji, wrapping, ellipsis, weights/styles, selection/composition |
| Images/icons | PNG/JPEG, fit modes, scaling, clipping, alpha, generated Material icons |
| Layout | Row/column flex, keyed reorder, padding/alignment, stack, constraints, overflow boundaries |
| Inputs | Rest/hover/pressed/focused/disabled/error for button, checkbox, radio, slider, switch, text input |
| Material | Navigation, app bar, cards, chips, dialogs, sheets, FAB/menu, lists, progress, toolbar/button groups |
| Cupertino | Navigation/tab bars, buttons, switch/slider, dialogs/sheets, glass opaque/translucent fallbacks |
| Navigation | Push/pop transitions, hero midpoint/endpoints, edge-swipe states, deep-linked destination |
| Scrolling | Initial/scrolled/overscroll/refresh/pagination states and clipped content |
| Insets/theme | Safe areas, keyboard inset, light/dark, Material/Cupertino, accent variations |
| App screens | Stable Huddle home/feed/thread/search/profile/settings states and loading/empty/error variants |

Animations should use a small set of meaningful fixed timestamps: start,
midpoint, threshold/crossover where relevant, and settled/end. Do not snapshot
every frame.

---

## Android Emulator GPU Lab

### Scope

The emulator tier proves behavior the native headless renderer cannot:

- x86_64 Rust artifact selection and JNI symbol loading.
- Kotlin `SurfaceView` and `ANativeWindow` surface creation.
- Android Vulkan adapter/device creation through gfxstream.
- Surface destroy/recreate, rotation, background/resume, and resize.
- Choreographer-driven frames and background-thread wakeups.
- Touch/key/IME input, insets, back, deep links, and accessibility glue.
- APK build/install/launch and composed-display screenshots.

The Android Emulator officially supports `-no-window` for servers and
`-gpu host` for host graphics acceleration:
[command-line options](https://developer.android.com/studio/run/emulator-commandline),
[graphics acceleration](https://developer.android.com/studio/run/emulator-acceleration).
Current emulator releases use `-gpu swiftshader`; the older
`swiftshader_indirect` spelling is deprecated.

### Install and Create the AVD

Use an x86_64 image on an x86_64 host so KVM can accelerate the guest. Align
the primary AVD with Frust's current compile/target SDK (API 36):

```bash
export ANDROID_HOME="$HOME/Android/Sdk"
export PATH="$ANDROID_HOME/emulator:$ANDROID_HOME/platform-tools:$PATH"
SDKMANAGER="$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager"
AVDMANAGER="$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager"

yes | "$SDKMANAGER" --licenses
"$SDKMANAGER" \
  "emulator" \
  "platform-tools" \
  "system-images;android-36;google_apis;x86_64"

echo no | "$AVDMANAGER" create avd \
  --name frust_api36 \
  --package "system-images;android-36;google_apis;x86_64" \
  --device pixel_7
```

Pin the resolved emulator and system-image package revisions after the first
known-good run. Do not auto-update the rendering fleet immediately before a
baseline run.

### KVM and Permissions

```bash
"$ANDROID_HOME/emulator/emulator" -accel-check
test -r /dev/kvm -a -w /dev/kvm
id
```

The service account normally needs the distribution's `kvm` group and any
`video`/`render` access required by its graphics stack. An emulator inside a
container must be given `/dev/kvm` and the required GPU device nodes; ordinary
container isolation does not pass them automatically.

### Start Without X

```bash
__NV_PRIME_RENDER_OFFLOAD=1 \
__VK_LAYER_NV_optimus=NVIDIA_only \
"$ANDROID_HOME/emulator/emulator" @frust_api36 \
  -no-window \
  -gpu host \
  -accel on \
  -noaudio \
  -no-boot-anim \
  -no-snapshot \
  -port 5554
```

For a clean-room run add `-wipe-data`; do not use it for every local iteration
because it makes boot materially slower. For CI, prefer a known clean AVD data
image or explicitly clear the application between cases.

Wait for a complete boot, not merely an `adb` transport:

```bash
adb -s emulator-5554 wait-for-device
until [ "$(adb -s emulator-5554 shell getprop sys.boot_completed | tr -d '\r')" = 1 ]; do
  sleep 1
done
```

Normalize mutable device state before screenshots:

```bash
adb -s emulator-5554 shell settings put global window_animation_scale 0
adb -s emulator-5554 shell settings put global transition_animation_scale 0
adb -s emulator-5554 shell settings put global animator_duration_scale 0
adb -s emulator-5554 shell settings put system font_scale 1.0
adb -s emulator-5554 shell cmd uimode night no
```

Also fix locale, orientation, navigation mode, display size/density, time zone,
and system-bar policy in the automated harness. Record their observed values in
the run metadata rather than assuming commands were accepted.

### Build, Launch, and Capture

`frust run -d emulator-5554` automatically queries the emulator ABI and builds
`x86_64`. Automation should separate build/install/launch/log capture so it can
wait for a deterministic ready signal:

```bash
cd examples/huddle
frust build apk --debug --target-platform android-x64
adb -s emulator-5554 install -r android/app/build/outputs/apk/debug/app-debug.apk
adb -s emulator-5554 shell am force-stop it.f0x.huddle
adb -s emulator-5554 shell am start -W -n it.f0x.huddle/.MainActivity
adb -s emulator-5554 exec-out screencap -p > actual-android.png
```

Android documents `adb exec-out screencap -p` as the raw PNG capture path:
[ADB screenshots](https://developer.android.com/tools/adb#screencap).

Do not use a fixed sleep as the final ready condition. Prefer an app/test build
signal emitted after the target state has been painted and presented. Until
that exists, require two or more identical consecutive screenshots within a
bounded timeout and retain logs when stabilization fails.

### Hardware Verification

While the app renders:

```bash
nvidia-smi
adb -s emulator-5554 shell getprop ro.hardware.vulkan
adb -s emulator-5554 shell dumpsys SurfaceFlinger
```

Retain the emulator's startup log. It identifies the chosen graphics mode and
is required evidence when a supposedly hardware-backed result is promoted.
Use `-gpu swiftshader` as a diagnostic/reference mode, not as proof that the
T400 path works.

### Android Scenario Matrix

Automate at least:

- Cold launch, warm launch, force-stop/relaunch, background/resume.
- Portrait/landscape rotation and repeated surface recreation.
- Light/dark mode, font scale, fixed density, safe-area/system-bar insets.
- Tap, drag, fling, long-press, hardware Enter, text composition, and keyboard
  show/hide.
- Root and nested back behavior.
- Cold and warm deep links.
- Loading, success, empty, error, modal, navigation, and scroll states.
- TalkBack semantics tree smoke where emulator accessibility automation is
  reliable; retain physical-device TalkBack as the authority.
- Host-GPU and SwiftShader smoke, with the full visual suite on the pinned host
  GPU configuration.

---

## Xorg, Xvfb, and VirtualGL Fallback

Xvfb is a software X server; it does not make NVIDIA rendering available.
VirtualGL redirects OpenGL/GLX/EGL work from a 2D X server to a separate 3D
server. It does not redirect Vulkan calls. Since Frust uses Vulkan inside the
Android guest, VirtualGL is not the primary acceleration mechanism.

Use this escalation order:

1. `-no-window -gpu host`, with no `DISPLAY`.
2. A minimal NVIDIA-backed Xorg with no desktop or window manager, then run the
   emulator with `DISPLAY=:0`.
3. Xvfb plus VirtualGL only when Qt/OpenGL initialization specifically requires
   a 2D X display.
4. `-gpu swiftshader` to distinguish emulator/host-GPU faults from Frust faults.

VirtualGL's headless NVIDIA guide creates a real 3D X server with an empty
display configuration:

```bash
sudo nvidia-xconfig \
  --allow-empty-initial-configuration \
  --use-display-device=None \
  --virtual=1920x1200 \
  --busid PCI:1:0:0
sudo Xorg :0 -noreset &
```

Review the generated X configuration before replacing an existing system
configuration. See the
[VirtualGL headless NVIDIA guide](https://virtualgl.org/Documentation/HeadlessNV).

If a separate 2D server is necessary:

```bash
Xvfb :99 -screen 0 1920x1200x24 -nolisten tcp &

DISPLAY=:99 \
VGL_DISPLAY=:0 \
__NV_PRIME_RENDER_OFFLOAD=1 \
__VK_LAYER_NV_optimus=NVIDIA_only \
vglrun -d :0 \
"$ANDROID_HOME/emulator/emulator" @frust_api36 \
  -no-window -gpu host -accel on -noaudio
```

In that topology, `:99` is the software 2D display and `:0` is the NVIDIA 3D
server. Prove actual GPU use from emulator logs and `nvidia-smi`; the presence
of `vglrun` alone proves nothing about guest Vulkan execution.

---

## Desktop, iOS, and Physical Devices

### Desktop

The native offscreen suite does not exercise presentation. Relevant desktop
changes still require surface create/configure/acquire/present, resize, DPI,
focus, IME, input, background wake, and surface-loss coverage. A virtual
display may automate window screenshots, but it is a separate T4 suite and
must not replace T2/T3 headless rendering.

### iOS

Linux can only run host logic and Android target gates. macOS must run iOS
compile/scaffold gates. The current Vello 0.9/wgpu 29 stack cannot render on
the iOS Simulator because its Metal feature set lacks Vello's required
indirect execution capability; do not classify a black Simulator surface as a
golden result. Physical iOS devices remain required for rendered pixels,
VoiceOver, IME, lifecycle, refresh-rate, and secure-storage/biometric gates.

### Physical Android

At minimum, release testing should cover one modern Qualcomm/Adreno device and
one materially different driver/OEM family when available. Physical devices
own validation of OEM Vulkan drivers, high-refresh behavior, real keyboards,
TalkBack, biometrics/Keystore, thermal behavior, process death, and background
restrictions. Emulator success is necessary but not sufficient.

---

## Tooling, Plugins, and Packaging

Use the owning test layer:

- `frust-drive`: fake `ProcessRunner` unit tests plus narrowly scoped real-tool
  end-to-end tests.
- `frust-cli`: argument/dispatch tests, scaffold content tests, profile-sync
  tripwires, and ignored real Cargo/Gradle/Xcode build gates.
- `frust-tui`: engine/update tests and terminal snapshots at normal, narrow,
  and minimum sizes; retain manual terminal restoration and live mouse/keyboard
  checks.
- Templates: render, parse, and compile generated projects; Android and iOS
  host projects must remain syntactically valid.
- Plugins: run the shared conformance contract against every fake/file backend,
  then platform-specific device gates for Keychain, Keystore, preferences,
  biometrics, cancellation, lockout, and process restart.
- FFI: keep no-unwind guards, null/invalid-handle behavior, UTF-16 conversion,
  and fixed JNI/C symbol names covered by host-testable helpers and target
  compile gates.

Never put secrets, signing keys, biometric data, device identifiers, or raw
user content into golden artifacts or CI logs.

---

## Performance, Coverage, and Robustness

Performance claims use `benchmarks/PROTOCOL.md`; a golden run is not a
benchmark. Do not infer performance from screenshot completion time.

Scheduled robustness work should include:

- `cargo llvm-cov` reporting for root and standalone workspaces. Establish a
  baseline first; prefer changed-code and critical-module targets over a blind
  repository-wide percentage.
- Fuzz targets for parsers/framing, deep links/paths, Android/iOS identifiers,
  scene command sequences, text/IME state conversion, and persisted config.
- Miri for pure Rust crates and test subsets that do not require FFI/GPU/system
  APIs.
- Sanitizer jobs for supported Linux host subsets, especially parsers, scene
  construction, and FFI-support helpers.
- Repeated/stress runs for async cancellation, surface lifecycle, navigation,
  and emulator rotation/background loops.
- Benchmark S1-S8 regression runs only under the protocol's controlled device,
  thermal, warmup, and statistical rules.

A flaky test is quarantined only with an owner, issue, retained artifacts, and
expiry. Retries may gather evidence; they must not turn a first-attempt failure
into an unreported pass.

---

## Gate Selection

| Change area | Required additions beyond the host gate |
|---|---|
| Pure core/theme/scene math | Focused unit/property tests; CPU goldens if pixels change |
| Text shaping/editor | Multiscript/IME tests, bundled-font CPU goldens, GPU text goldens |
| Widget/layout/theme visuals | Semantic interaction tests plus CPU and T400 golden catalog |
| Vello/wgpu/render lifecycle | CPU-tier tests, T400 headless suite, surface lifecycle smoke |
| Desktop shell | Desktop presentation/resize/DPI/IME/manual visual gate |
| Android shell/template/drive | Android target compile, emulator host-GPU matrix, relevant physical-device gate |
| iOS shell/template/drive | macOS target/scaffold gate and relevant physical-iOS gate |
| Navigation/input/accessibility | Headless semantic tests plus emulator/device workflow tests |
| Plugin platform backend | Conformance tests, target compile, real platform service/device gate |
| CLI/scaffold/build | Relevant ignored end-to-end build test |
| TUI | Engine tests, affected snapshots, live-terminal manual gate when terminal behavior changes |
| Performance-sensitive path | Functional gates plus affected S1-S8 protocol scenarios |

---

## Failure Triage

Classify a rendering failure before changing a baseline:

1. **Environment:** wrong adapter, missing device node, KVM unavailable, font or
   locale drift, emulator/system-image/driver update.
2. **Infrastructure:** boot timeout, ADB disconnect, capture corruption, stale
   app data, readiness signal failure.
3. **Backend:** device loss, shader/pipeline failure, gfxstream/driver crash,
   CPU/GPU divergence.
4. **Framework:** scene, layout, text, theme, input state, or lifecycle defect.
5. **Intentional change:** reviewed product/design update requiring explicit
   baseline promotion.

Always retain metadata, logs, expected/actual/diff images, and the exact command
line for non-environment failures. A baseline update is the last step, not the
first debugging action.

---

## Headless Runner Operations

The GPU runner should use a dedicated unprivileged service account with only
the device/group permissions it needs. Serialize T400 golden and emulator jobs
initially: 4 GB of VRAM is sufficient for these suites, but parallel emulator,
Vello, and benchmark jobs create avoidable memory and scheduling noise.

Pin and record:

- OS/kernel and NVIDIA driver packages.
- Vulkan loader and NVIDIA ICD.
- Rust toolchain and Cargo lockfiles.
- Android command-line tools, emulator, platform-tools, NDK, and system image.
- AVD config/data seed and test font/assets.

Use bounded timeouts and guaranteed cleanup for emulator, ADB, Xorg/Xvfb, and
test processes. Expose ADB/emulator control ports only on localhost or a
protected network. Treat promoted goldens and their metadata as versioned test
inputs.

