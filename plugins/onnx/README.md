# frust-onnx

**ONNX model inference** for frust apps, over [ONNX Runtime](https://onnxruntime.ai) through the
[`ort`](https://crates.io/crates/ort) crate. Load a model from a file or from bytes, read its
named inputs and outputs, and run it on typed tensors (f32, u8, i64), blocking or on a
per-session worker thread with cancel. CoreML accelerates models with static input shapes on Apple
targets; everything else runs on the CPU, and an accelerator that cannot be used falls back to the
CPU.

**Status:** desktop, arm64 Android and arm64 iOS. Linux (glibc, x86_64/aarch64), Apple-silicon
macOS, Windows (MSVC), Android arm64-v8a, iOS devices and the arm64 iOS Simulator link ONNX
Runtime statically from a prebuilt archive fetched at build time. Android also needs the NDK's
`libc++_shared.so` in the APK, which `frust plugin add onnx` arranges (see
[Android packaging](#6-android-packaging)); iOS needs three extra lines in the Xcode link and a
deployment target of 15.1 or later, and `frust plugin add onnx` adds the lines (see
[iOS packaging](#7-ios-packaging)). Intel macOS,
the x86_64 Simulator, Mac Catalyst, 32-bit Android and x86_64 Android load the library at run
time instead and ship none.

Like every frust **platform plugin**, this crate is added to your app's own `Cargo.toml`
alongside `frust`; the `frust` facade does not re-export it. The reactive handles
([In a component](#11-in-a-component)) are the only code that names `frust`.

Building it needs Rust 1.92 or newer (the pinned `ort` requires it), above the workspace's 1.89.

## Add it

```sh
frust plugin add onnx
```

or pick **onnx** in the TUI's Add Plugin dialog. It makes these edits to a project made by
`frust create`:

- `Cargo.toml`: the `frust-onnx` dependency, written in the form the app's own `frust`
  dependency takes (a path into the same checkout, or the same version).
- `android/gradle.properties`: `frust.cargoNdk.linkLibcxxShared=true`, so the scaffold's
  `cargoNdkBuild` task packages the NDK's `libc++_shared.so` (see
  [Android packaging](#6-android-packaging)).
- `android/app/src/main/AndroidManifest.xml`: `<uses-permission android:name="android.permission.INTERNET" />`
  in the main manifest, because the catalog downloads need it and the scaffold declares it only in
  its debug and profile manifests.
- `ios/Runner.xcodeproj/project.pbxproj`: `-lc++`, `-framework CoreML` and `-framework Foundation`
  at the end of every `OTHER_LDFLAGS` list (see [iOS packaging](#7-ios-packaging)).

It adds no Gradle module, Swift package or plist key. Running it twice changes nothing. A
project whose `android/app/build.gradle.kts` predates the property, or whose iOS deployment target
is below 15.1, is refused before anything is written, and the error says what to change. Adding
the dependency by hand (`frust-onnx = "0.6"`) skips the Android and iOS edits; make them yourself
(the property, the permission for a release build that downloads, and the three linker lines) or
the APK will not load, downloads will fail or the iOS link will fail. A project with no `android/`
directory gets no Android edit, and one with no `ios/` directory no iOS edit.

Features, all on by default:

- `catalog`: the verified vision model list, which brings in `frust-models`.
- `vision` (turns on `catalog`): segmentation, face detection and depth.
- `frust-api`: `api::VisionHandle`, the reactive handle. Turn the defaults off
  (`default-features = false`, then `features = ["vision"]` to keep the tasks) for a build with
  no `frust-ui` in its graph.

---

## 1. Desktop quick start

```toml
# app Cargo.toml — [dependencies]
frust-onnx = { path = "<frust>/plugins/onnx" }  # or the crates.io release
```

```rust,ignore
use frust_onnx::{Session, Tensor};

let session = Session::from_file("model.onnx")?;
for input in session.inputs() {
    println!("{}: {} {:?}", input.name, input.dtype, input.shape); // None = dynamic dimension
}

let a = Tensor::from_f32([1, 3], vec![1.0, 2.0, 3.0])?;
let b = Tensor::from_f32([1, 3], vec![10.0, 20.0, 30.0])?;
for (name, tensor) in session.run_blocking(&[("a", a), ("b", b)])? {
    println!("{name}: {:?} {:?}", tensor.shape(), tensor.as_f32());
}
```

- `run_blocking` parks the calling thread until the run finishes: call it from a worker thread,
  never from the UI thread, or use `Session::run` ([Threading](#3-threading)). `Session` is
  `Send + Sync`; runs on one session are serialised, so load one session per thread for parallel
  inference.
- Input names, element types, ranks and static dimensions are checked before the runtime is
  called (`OnnxError::InvalidInput`).
- One ONNX Runtime environment serves the process, created on first use. If it cannot be created,
  every call returns the same `OnnxError::Runtime`; nothing panics.
- `runtime_info()` reports the ONNX Runtime release and whether it is linked statically or loaded
  at run time.

### The build-time download

The first build on a machine downloads the prebuilt ONNX Runtime 1.31.0 archive for the target
(about 11 MB compressed for x86_64 Linux) from `cdn.pyke.io` and caches it, extracted, under
the user cache directory (`~/.cache/ort.pyke.io/dfbin/<target>/<hash>` on Linux,
`~/Library/Caches/ort.pyke.io/...` on macOS, `%LOCALAPPDATA%\ort.pyke.io\...` on Windows). Later
builds reuse the cache without network access. To build without the download, point
`ORT_LIB_PATH` at a local ONNX Runtime build; note that `ORT_SKIP_DOWNLOAD`, `ORT_OFFLINE` and
`cargo --offline` skip the download *and* the cache, which leaves the runtime unlinked.

On the run-time-loading targets the library is the one named by `ORT_DYLIB_PATH`, else the
platform's `onnxruntime` library on the loader path; it must be ONNX Runtime 1.17 or newer.

### The macOS release link

The archive's CoreML objects call `___isPlatformVersionAtLeast`, the `@available` check from
clang's builtins. Cross-crate LTO is what removes it: `lto = "fat"` or `"thin"` (the release
profile `frust create` writes uses `"fat"`) internalizes the standard library's copy, which
satisfies the call in every other build, while `codegen-units = 1`, `strip = "symbols"` and
`panic = "abort"` each link fine on their own. On Apple-silicon macOS the crate's build script
therefore asks `clang --print-file-name=libclang_rt.osx.a` for the toolchain's builtins archive and
adds it to the final link, so an app needs no linker flag; the linker takes the function from it
only when nothing else defines it. If `clang` cannot find the archive, the build prints a
`frust-onnx` warning and an LTO release build fails as above: install the Xcode command-line tools,
or build with `RUSTFLAGS="-C link-arg=-fapple-link-rtlib"`. Every other target, iOS and Intel
macOS included, gets nothing from the script.

---

## 2. Sessions and accelerators

```rust,ignore
use frust_onnx::{Accelerator, Optimization, Provider, Session, SessionOptions};

let options = SessionOptions::default()
    .with_accelerator(Accelerator::Auto) // the default
    .with_intra_threads(2)
    .with_optimization(Optimization::All);
let session = Session::from_file_with("model.onnx", &options)?;
if session.provider() == Provider::Cpu { /* no accelerator on this device */ }
```

`Session::from_file` and `from_bytes` are `from_file_with`/`from_bytes_with` with
`SessionOptions::default()`. `SessionOptions` is `Clone + Eq + Hash`; set its fields or use the
`with_*` methods.

| Option | Default | Effect |
|--------|---------|--------|
| `accelerator` | `Auto` | `Auto`, `Cpu`, `CoreMl(CoreMlUnits::{All, CpuAndGpu, CpuAndNeuralEngine})` or `Xnnpack` |
| `strict_accelerator` | `false` | an explicit accelerator that cannot be used is `OnnxError::Unsupported` instead of a CPU fall back |
| `intra_threads` | `None` = `min(4, available_parallelism)` | threads one run uses, the calling one included (`Some(0)` counts as 1) |
| `optimization` | `All` | graph optimisation: `Basic`, `Extended` or `All` |
| `cpu_arena` | `false` | ONNX Runtime's CPU memory arena |
| `memory_pattern` | `false` | pre-plan one block for a run's intermediate tensors |
| `arena_shrinkage` | `false` | give unused arena blocks back after each run (only with `cpu_arena`) |

**Accelerator policy.** On Apple targets `Auto` uses CoreML (all compute units) only for a model
whose inputs are all fully static in shape; a model with any dynamic input dimension loads on the
CPU, and the load logs why. Set `Accelerator::CoreMl` to force CoreML on such a model. Measured
on an Apple M4: Depth Anything V2 Small (input `[?, 3, ?, ?]`) ran 232 ms at 473 MB on the CPU
against 728 ms at about 5.2 GB on CoreML; U2-NetP (static `[1, 3, 320, 320]`) 90 ms on the CPU
against 75 ms on CoreML. Everywhere else `Auto` is the CPU. On Android that is the CPU because the shipped ONNX Runtime archive has no
XNNPACK provider (see [Android packaging](#6-android-packaging)); NNAPI is compiled into that
archive but this crate never registers it. `Xnnpack` is accepted but compiled into no build yet,
so it always falls back (or is `Unsupported` when strict). When the provider is missing from the
build or from the runtime library, or the runtime refuses it while preparing the model, the load
logs a warning and retries on the CPU; `Session::provider()` reports what actually runs. With
`strict_accelerator`, a model the CPU loads but the accelerator does not is `Unsupported`, while
a model that fails on the CPU too stays `OnnxError::Load`. `Auto` never fails for want of a
provider, strict or not.

| Target | Provider library | `Auto` |
|--------|------------------|--------|
| Apple-silicon macOS | CoreML in the prebuilt `coreml` archive, linked statically | CoreML for static input shapes, else CPU |
| iOS device, arm64 Simulator | CoreML in the prebuilt `coreml` archive, linked statically | CoreML for static input shapes, else CPU |
| Intel macOS, x86_64 Simulator, Mac Catalyst | CoreML if the ONNX Runtime library loaded at run time has it | CoreML for static input shapes if present, else CPU |
| Android arm64 | static `nnapi` archive: CPU (and NNAPI, unused), no XNNPACK | CPU |
| Linux, Windows, others | CPU | CPU |

CoreML needs iOS 13 or newer; nodes it cannot run stay on the CPU. For a model loaded from a file,
CoreML keeps its compiled model under `<cache dir>/frust-onnx/coreml` (`frust_paths::cache_dir()`);
a model from bytes gets no cache. The CoreML code compiles only for Apple targets; it has run on
an Apple M4 Mac, an iPhone and the iOS Simulator (see [iOS packaging](#7-ios-packaging)).

**Memory.** U2-NetP (`u2netp.onnx`, 4.4 MiB) at 1x3x320x320, x86_64 Linux host (i5-12600,
12 threads), release build, ONNX Runtime 1.31.0, one fresh process per row, one warm-up run then
20 timed runs. Peak RSS is `VmHWM`, about 8 MiB before the first session; "after" is `VmRSS` once
the runs end. Rows vary by about 20 MiB between processes.

| Options (`All` unless noted, 4 threads unless noted) | Peak RSS | Resident after | Median per run |
|---------|---------:|---------:|---------:|
| arena on, pattern on (ONNX Runtime's defaults), 6 threads | 511 MiB | 511 MiB | 104 ms |
| arena on, pattern on | 510 MiB | 510 MiB | 114 ms |
| arena on, pattern on, shrinkage | 360 MiB | 110 MiB | 123 ms |
| arena on, pattern off | 317 MiB | 317 MiB | 113 ms |
| arena on, pattern off, shrinkage | 350 MiB | 97 MiB | 121 ms |
| arena off, pattern on | 314 MiB | 64 MiB | 121 ms |
| **arena off, pattern off (the default)** | **328 MiB** | 201 MiB | **120 ms** |
| the default, 1 thread | 301 MiB | 84 MiB | 385 ms |
| the default, 2 threads | 343 MiB | 85 MiB | 205 ms |
| the default, 6 threads | 340 MiB | 199 MiB | 102 ms |
| the default, `Extended` | 235 MiB | 84 MiB | 140 ms |
| the default, `Basic` | 238 MiB | 85 MiB | 144 ms |
| three sessions, arena on, pattern on | 1,460 MiB | 1,460 MiB | 115 ms |
| three sessions, arena on, pattern off, shrinkage | 690 MiB | 432 MiB | 120 ms |
| **three sessions, the default** | **332 MiB** | 85 MiB | 125 ms |

The arena with a memory pattern holds the first run's whole plan, rounded up, for the session's
life, and each session keeps its own: that is the 133 MB to 607 MB jump one U2-NetP session
caused on a OnePlus 9. With both off, the growth over the process falls from about 500 MiB to
about 320 MiB for one session (three sessions: about 1,450 MiB to 325 MiB) for about 5 % more
time per run, and the memory goes back to the allocator between runs. `Extended` cuts the peak
by another 80 MiB at about 15 % more time; it is not the default because the cost depends on the
model. Reproduce a row with the ignored `measure::measure_model` test (`tests/session.rs`), one
process per row:

```sh
FRUST_ONNX_MODEL=/path/to/u2netp.onnx FRUST_ONNX_ARENA=0 FRUST_ONNX_PATTERN=0 \
  cargo test --release -p frust-onnx --test session -- --ignored --exact measure::measure_model --nocapture
```

## 3. Threading

`Session::run` runs on the session's own worker thread, `frust-onnx-run`, and returns a
`RunHandle` at once:

```rust,ignore
use std::sync::Arc;
use frust_onnx::{RunEvent, Session, Tensor};

let session = Arc::new(Session::from_file("model.onnx")?);
let inputs = vec![("input".to_owned(), Tensor::from_f32([1, 3], vec![0.0; 3])?)];
let handle = session.run(inputs, Arc::new(|event| match event {
    RunEvent::Done(outputs) => { /* hand the outputs to the UI thread */ }
    RunEvent::Cancelled => {}
    RunEvent::Error(err) => log::warn!("{err}"),
}));
handle.cancel(); // or handle.wait(), handle.is_finished()
```

- The sink receives **exactly one** `RunEvent`, on the worker thread; a panicking sink is logged
  and the thread carries on. Post the result to the UI thread from the sink.
- One thread per session, started on the first `run`: runs on one session queue in submission
  order, and queued runs keep the session alive. The thread ends once the session is dropped and
  its queue is empty. It holds no thread-local state.
- `cancel()` on a queued run skips it; on a running one it sets ONNX Runtime's terminate flag
  (`RunOptions::terminate` in the pinned `ort`), which stops the graph at the next node boundary.
  Either way the sink receives `RunEvent::Cancelled`, even if the run had already finished but
  its outcome was not yet delivered. A cancel after delivery does nothing.
- `wait()` blocks until the sink has returned; never call it from the sink. Dropping the handle
  neither cancels nor waits.
- If no thread can be started, the sink receives `RunEvent::Error` on the calling thread before
  `run` returns.

## 4. Session cache

```rust,ignore
use frust_onnx::{SessionCache, SessionOptions};

let cache = SessionCache::default(); // at most 3 sessions
let session = cache.get_or_load("model.onnx", &SessionOptions::default())?;
// on the platform's memory-pressure signal:
cache.clear();
```

- Keyed by the canonical model path and the `SessionOptions`; the least recently used session is
  evicted when a new key would exceed `max_sessions`. Eviction and `clear()` drop only the
  cache's reference; a session still held elsewhere lives until its holder lets go.
- Threads asking for the same key at once share one load; a failed load is not cached.
  `evict(path)` drops every session for a path, whatever its options. `load_count()` counts the
  loads the cache has made.
- The default bound is 3 (`SessionCache::DEFAULT_MAX_SESSIONS`): three U2-NetP sessions with the
  default options peak at about 330 MiB on the host (table above), well under 1 GB. With
  ONNX Runtime's own memory defaults the same three reach 1,460 MiB, which is why those defaults
  are off. Larger models need a smaller bound, `SessionCache::new(n)`.

## 5. Pin policy

`ort` is pinned exactly (`=2.0.0-rc.15`, binding ONNX Runtime 1.31.0) in the workspace root
`Cargo.toml`, with default features off. The pin is frust-owned: it moves only together with this
crate's gate suite (`cargo test -p frust-onnx` on each desktop prebuilt row), never on its own.
Each desktop target's `ort` feature set must equal one prebuilt row of `ort`'s distribution table
exactly; `lax-feature-matching` is never turned on.

**Bumping it**, as one change:

1. Move the root `Cargo.toml` pin and read the new `ort-sys` `dist.tsv`: every row this crate's
   `Cargo.toml` selects (plain Linux, `coreml` macOS, `directml` Windows, `nnapi` Android
   arm64, `coreml` iOS device and Simulator) must still exist with the same feature set, or the
   target table changes with it.
2. Check the new ONNX Runtime release against `api-17` (the lowest API level selected) and
   against `rust-version` here, which follows `ort`'s own minimum.
3. Run `cargo test -p frust-onnx` on each desktop row, the ignored live tests
   (`--test vision -- --ignored`, `--test catalog -- --ignored`), and the Android gate: a
   `frust create` app with `frust plugin add onnx` built with
   `frust build apk --release --target-platform android-arm64`, its `lib/arm64-v8a` passing
   `scripts/ci/page-size-check.sh`, installed and started on a phone. Check the archive still
   needs only `libc++_shared.so` beyond the NDK's system libraries (`llvm-readelf -d` on the
   app's `.so`). Then the iOS gate: the same app (its [iOS packaging](#7-ios-packaging) linker
   lines come from `frust plugin add onnx`) built for the Simulator and, signed in Release, for an
   iPhone, started on both; re-read the archive's minimum iOS version
   (`otool -l libonnxruntime.a | grep minos`) and keep the registry's `min_ios` for `onnx` equal
   to it.
4. Update the version numbers and measurements in this README.

## 6. Android packaging

**Route: the prebuilt static archive.** On `aarch64-linux-android` `ort` downloads its prebuilt
ONNX Runtime 1.31.0 archive (feature set `nnapi`, the only Android row; 11.2 MB compressed,
a 132 MB `libonnxruntime.a` once extracted) and links it statically into the app's own `.so`.
The APK carries no `libonnxruntime.so`, so no Gradle module and no Maven dependency are needed.
The alternative, `load-dynamic` over the `onnxruntime-android` AAR from Maven Central, was not
needed and was not tried.

**What the app must add: `libc++_shared.so`.** The archive is built against the NDK's shared
C++ runtime, so the app's `.so` gains `NEEDED libc++_shared.so`. `cargo ndk` stages that library
next to the app's `.so` only when asked (`--link-libcxx-shared`); without it the APK still
builds, but ships an app `.so` whose dependency is missing, which the loader refuses at start-up.

The scaffold's `cargoNdkBuild` task (`android/app/build.gradle.kts`) passes that flag when the
Gradle property `frust.cargoNdk.linkLibcxxShared` is `true`; `android/gradle.properties` ships it
`false`, so an app without this plugin carries no copy. `frust plugin add onnx` sets it `true`.
A project generated before the template read the property needs one line in that task's
command line, right after `add("ndk")` (the add refuses, quoting it, until it is there):

```kotlin
if ((project.findProperty("frust.cargoNdk.linkLibcxxShared") as String?)?.trim().equals("true", ignoreCase = true)) add("--link-libcxx-shared")
```

Setting `CARGO_NDK_LINK_LIBCXX_SHARED=true` in the environment of `frust build` does the same
for one build (`1` is rejected; the value must be `true`). AGP strips the staged copy (9.2 MB in
the NDK, 1.25 MB in the APK). A hot `frust run --watch` session builds the app's `.so`
outside Gradle and does not stage the library, so the app fails to start; use `--no-hot`
(`onnx-hot-run-libcxx-shared` in `docs/LIMITATIONS.md`).

**Floors and ABIs.** minSdk 29: the scaffold's floor, and the archive links at API level 29.
The NDK is the scaffold's r28 (28.2.13676358), which aligns every `.so` to 16 KB pages.

| ABI | What happens |
|-----|--------------|
| arm64-v8a | Static link, as above. A `frust create` app with `frust plugin add onnx` and no hand edit builds, packages `libc++_shared.so` and starts on a OnePlus 9, where `runtime_info()` reports ONNX Runtime 1.31.0 linked `Static`. |
| x86_64 | `ort` has an `nnapi` prebuilt row for it too, but this crate does not use it yet (no build has proven it), so it takes the run-time route: it compiles, and every session returns `OnnxError::Runtime` unless the app ships its own `libonnxruntime.so`. |
| armeabi-v7a | No prebuilt row: the run-time route, as for x86_64. |

**Execution providers in the shipped library.** CPU and NNAPI are compiled in (the archive
defines `onnxruntime::NnapiExecutionProvider`); XNNPACK is not (the archive defines no XNNPACK
symbol). This crate registers neither NNAPI nor (absent) XNNPACK, so Android runs on the CPU and
`Accelerator::Auto` is `Cpu` there.

**Measured** on a fresh `frust create` app (Material counter) built with `frust build apk
--release --target-platform android-arm64`, with and without `frust-onnx` plus a ten-run
U2-NetP smoke (`u2netp.onnx`, 4,574,861 bytes, SHA-256
`309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8`, pushed with `adb push` to
the app's `/sdcard/Android/data/<application id>/files/`, not bundled):

| Measure | Value |
|---------|-------|
| Release APK without `frust-onnx` | 10,793,449 bytes (app `.so` 10,551,048) |
| Release APK with `frust-onnx` | 31,195,333 bytes (+20.4 MB): app `.so` 29,691,288 (+19.1 MB), `libc++_shared.so` 1,253,544 |
| Release APK with `frust-onnx` added by `frust plugin add onnx` (later build, no U2-NetP smoke code) | 31,114,032 bytes: app `.so` 29,609,984, `libc++_shared.so` 1,253,544; the same app without the plugin ships no `libc++_shared.so` |
| 16 KB page size (`scripts/ci/page-size-check.sh` on the APK's `lib/arm64-v8a`) | both `.so` files pass, minimum PT_LOAD alignment 0x4000 |
| Clean `cargo ndk -t arm64-v8a build --release -p frust-onnx` | 9.2 s wall, the archive download included |
| U2-NetP on a OnePlus 9, ONNX Runtime's memory defaults (arena and memory pattern on) | 458 ms median per run; app peak RSS 133 MB before the session, 607 MB after (native heap 439 MB) |
| U2-NetP on a OnePlus 9, this crate's defaults | 519 ms median per warm run (`examples/onnx-vision`, release); app peak RSS 245 MB with a 2048x1536 photo loaded, 409 MB once the U2-NetP session had run |

With this crate's defaults the growth one U2-NetP session causes falls from about 470 MB to about
165 MB, close to the third the host table in [Sessions and accelerators](#2-sessions-and-accelerators)
predicts; the run costs about 13 % more time (519 ms against 458 ms).

## 7. iOS packaging

**Route: the prebuilt static archive**, as on Android. On `aarch64-apple-ios` and
`aarch64-apple-ios-sim` `ort` downloads its prebuilt ONNX Runtime 1.31.0 archive (feature set
`coreml`, the only row for each; about 10 MB compressed, an 88 MB `libonnxruntime.a` once
extracted) and links it statically into the app's Rust staticlib. No Swift package, xcframework,
CocoaPod or bundled framework is needed, and the app ships no ONNX Runtime library. The archive
contains the CoreML execution provider, which `Accelerator::Auto` uses for static-shape models
(see [Sessions and accelerators](#2-sessions-and-accelerators)). The alternative,
a local Swift package over Microsoft's onnxruntime xcframework, was not needed and was not tried.
`x86_64-apple-ios` (the Intel Simulator) and Mac Catalyst have no prebuilt row and load the
library at run time.

**What the app's Xcode link must add.** A Rust staticlib carries `ort-sys`' link requests
(`-lc++`, `-framework Foundation`, `-framework CoreML`) only into Cargo's own link, never into
Xcode's, so the Runner target's `OTHER_LDFLAGS` (every configuration) must name them, after the
scaffold's own entries. `frust plugin add onnx` adds exactly these three to every list
(`Contribution::IosLibrary` for `-lc++`, `Contribution::IosFramework` for the two frameworks).
Add them by hand only for a dependency added by hand:

```text
-lc++
-framework CoreML
-framework Foundation
```

An unmodified scaffold (Debug, Simulator) linked without them, but only by accident: the archive
autolinks CoreML, the embedding's Swift objects autolink Foundation, and `libc++` arrives through
Swift's back-deployment library `libswiftCompatibility56.a`, which Xcode links only while the
deployment target is below iOS 15.4. At 15.4 or later that route is gone: a scaffold at a 17.0
deployment target fails the Simulator link with 233 undefined `std::` references, and adding only
`-lc++` fixes it. So all three are named. `Accelerate` and other frameworks are not needed.

**Minimum iOS version: 15.1.** The archive's objects are built for iOS 15.1 (`minos 15.1`, SDK
18.5, in `otool -l`). A deployment target of 15.0 links, but the linker prints a warning for each
of about 940 objects (`was built for newer 'iOS' version (15.1) than being linked (15.0)`), and
nothing has run on iOS 15. New apps target 15.1, and `frust plugin add onnx` refuses a project
below it before writing anything:

```text
Error: plugin `onnx` needs iOS 15.1 or later; ios/Runner.xcodeproj/project.pbxproj sets IPHONEOS_DEPLOYMENT_TARGET = 15.0. Raise it in every build configuration (and keep it at or above every Swift package's minimum), then re-run.
```

Raise `IPHONEOS_DEPLOYMENT_TARGET` to 15.1 or higher in every configuration and add it again.
CoreML needs iOS 13, so it adds no constraint.

**Simulator.** The `aarch64-apple-ios-sim` row works the same way and needs the same three
lines; the Simulator build needed nothing different from the device build. CoreML registers on
the Simulator too, but runs slower there than the CPU (table below), so a Simulator time says
nothing about a phone.

**Measured** on a fresh `frust create` app (Material counter) with `frust plugin add onnx`, the
three linker lines added by hand for the device build, and a ten-run U2-NetP smoke on a worker
thread (`u2netp.onnx` embedded with `include_bytes!` and loaded with `Session::from_bytes`, input
1x3x320x320). Times
are `run_blocking` per run, ten runs, first included. Memory is the process `phys_footprint`
(`proc_pid_rusage`), what Jetsam counts.

| Measure | Value |
|---------|-------|
| Device | iPhone 13 mini (A15), iOS 27.2 (24B115), signed Release build |
| `runtime_info()` | ONNX Runtime 1.31.0, `Static` |
| CPU (`Accelerator::Cpu`), 4 threads | median 278.8 ms, worst 306.6 ms (first run), best 271.6 ms; 10 of 10 outputs finite |
| CoreML (`SessionOptions::default()`) | load 2.27 s from bytes (CoreML compiles the model); median 94.4 ms, worst 165.6 ms (first run), best 93.3 ms; 10 of 10 finite |
| CoreML, model loaded from a file | first launch 2.30 s load; relaunch 0.46 s from the compiled-model cache; median 94.1 to 97.2 ms |
| Memory, CPU session | 117 MB after the load, peak 275 MB during the first run, 109 MB after the tenth |
| Memory, CoreML session | 281 MB after the load, 480 MB after the first run, 480 MB after the tenth, peak 602 MB; `os_proc_available_memory` 1,817 MB before the first run, 1,618 MB after |
| Release `Runner` binary without `frust-onnx` | 12,907,568 bytes (`.app` zipped: 6,119,301) |
| Release `Runner` binary with `frust-onnx` | 44,604,640 bytes, of which 4,574,861 are the embedded model: +27.1 MB for ONNX Runtime and the crate (`.app` zipped: 18,480,378, about +12 MB with the model) |
| Clean Release `cargo build --target aarch64-apple-ios`, fresh target dir, archive cached | 137 s with `frust-onnx` (staticlib 238,239,000 bytes), 238 s without (124,883,416 bytes), on an Apple-silicon Mac running other builds: machine load, not the crate, made the difference. `frust-onnx` compiles no C or C++ |
| Simulator (iPhone 17, Debug build, Apple-silicon Mac) | CPU median 114.5 ms, worst 227.5 ms; CoreML median 192.6 ms, load 418 ms; 10 of 10 finite each |

U2-NetP under CoreML stays within an iPhone's budget: about 480 MB once warm, against the
2,092 MB `os_proc_available_memory` reported at start, and the footprint did not grow between the
first and the tenth run. CoreML holds about 370 MB more than the CPU session and runs about three
times as fast. Larger models cost more under CoreML: keep one large CoreML session at a time and
clear the `SessionCache` on memory pressure. A OnePlus 9 runs the same model on its CPU at 485 to
519 ms median ([Android packaging](#6-android-packaging)).

## 8. Image helpers

`frust_onnx::image` (no extra dependency, single-threaded) turns pixels into model inputs and
back:

- `RgbaView::new(width, height, &pixels)` checks an RGBA8 buffer (non-zero sides, exactly
  `width * height * 4` bytes).
- `to_nchw_f32(view, out_w, out_h, norm, order)` resizes bilinearly (half-pixel centres, clamped
  borders) and returns a `[1, 3, out_h, out_w]` f32 tensor. `Normalization` computes
  `(byte * scale - mean) / std` per channel: `IMAGENET`, `UNIT` (0..1) and `RAW_255` (no scaling)
  are provided. `ChannelOrder::Rgb` or `Bgr` picks the plane order. Alpha is ignored.
- `resize_plane` resizes a single-channel `f32` plane bilinearly; `normalize_plane` min-max
  scales it to 0..1 in place (a constant plane becomes zeros); `Plane::to_rgba8_gray` makes
  display bytes.

```rust
use frust_onnx::{ChannelOrder, Normalization, RgbaView, to_nchw_f32};

let pixels = vec![255u8; 64 * 48 * 4];
let view = RgbaView::new(64, 48, &pixels)?;
let input = to_nchw_f32(view, 320, 320, Normalization::IMAGENET, ChannelOrder::Rgb)?;
assert_eq!(input.shape(), &[1, 3, 320, 320]);
# Ok::<(), frust_onnx::OnnxError>(())
```

## 9. Vision catalog

The default `catalog` feature adds `frust_onnx::catalog`, a short list of vision models that
download through `frust-models` with a pinned source, byte size and SHA-256 (the feature is the
only thing that brings `frust-models` into the tree; `--no-default-features` leaves it out).
`VISION_CATALOG` holds the rows, `by_key` and `for_task` look them up, and
`VisionModel::download_request()` builds a verified `DownloadRequest`.

| Key | Model | Task | Size | Licence | Source | Input |
|-----|-------|------|------|---------|--------|-------|
| `u2netp` | U2-NetP | Segmentation | 4,574,861 B | Apache-2.0 | rembg release `v0.0.0`, `u2netp.onnx` | 320x320 |
| `yunet` | YuNet 2023mar | Face detection | 232,589 B | MIT | opencv_zoo commit `f12e1279`, `models/face_detection_yunet/` | 640x640 |
| `depth-anything-v2-small` | Depth Anything V2 Small (quantized) | Depth | 27,258,801 B | Apache-2.0 | Hub `onnx-community/depth-anything-v2-small` at `4472b736`, `onnx/model_quantized.onnx` | 518x518 |

The Base and Large depth models are CC-BY-NC and are deliberately not listed. The ignored test
`cargo test -p frust-onnx --test catalog -- --ignored` downloads every row (about 32 MB) and
fails when a pin has drifted. Call `download_request` and start the download from a worker
thread, as `frust-models` describes.

## 10. Vision tasks

The default `vision` feature (which turns on `catalog`) adds `frust_onnx::vision`: one call from
RGBA pixels to a result for each catalog task. `VisionModels::new(store, options)` reads the
models from a `frust_models::ModelStore`, loads each on first use through its own
`SessionCache` (one session per task) and checks the model's inputs and outputs fit the task.

- **Every task function is blocking**: it may load the model, then runs it on the calling
  thread. Call it from a worker thread (in a frust app, inside `frust::spawn_blocking`), never
  from the UI thread.
- **No fallback.** A model that is not in the store is `OnnxError::ModelMissing { key }` (the
  catalog key, or a custom model's store id); nothing is substituted. `missing()` lists the
  catalog rows still to download, and `is_installed(task)` checks one task.
- **Custom models.** `with_model(task, model_id)` uses another store file for a task. It must
  take one f32 `[1, 3, H, W]` image (and, for faces, return YuNet's twelve outputs); a model
  that does not fit is `OnnxError::Unsupported` when it is first loaded. Dynamic input sides take
  the catalog model's size.

```rust,no_run
use frust_models::{Downloader, HubClient, ModelStore};
use frust_onnx::{SessionOptions, VisionModels};

let store = ModelStore::open()?;
let models = VisionModels::new(store.clone(), SessionOptions::default());
let hub = HubClient::new()?;
for model in models.missing() {
    // On a worker thread: downloads block too.
    Downloader::start(&hub, &store, model.download_request(), std::sync::Arc::new(|_| {}))?
        .wait()?;
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

**Subject mask** (U2-NetP): one sample per pixel, 0 (background) to 1 (subject).

```rust,no_run
# use frust_onnx::{RgbaView, SessionOptions, VisionModels};
# let models = VisionModels::new(frust_models::ModelStore::open()?, SessionOptions::default());
# let (width, height, pixels) = (640, 480, vec![0u8; 640 * 480 * 4]);
let mask = models.segment_subject(RgbaView::new(width, height, &pixels)?)?;
assert_eq!((mask.width, mask.height), (640, 480));
# Ok::<(), Box<dyn std::error::Error>>(())
```

**Faces** (YuNet): boxes `[x, y, w, h]` and five landmarks (eyes, nose tip, mouth corners) in
image pixels, highest score first. `None` keeps faces scoring at least 0.6.

```rust,no_run
# use frust_onnx::{RgbaView, SessionOptions, VisionModels};
# let models = VisionModels::new(frust_models::ModelStore::open()?, SessionOptions::default());
# let (width, height, pixels) = (640, 480, vec![0u8; 640 * 480 * 4]);
for face in models.detect_faces(RgbaView::new(width, height, &pixels)?, None)? {
    let [x, y, w, h] = face.rect;
    println!("{:.2} at {x},{y} {w}x{h}, nose at {:?}", face.score, face.landmarks[2]);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

**Depth** (Depth Anything V2 Small): relative depth 0..1 per pixel, near is 1
(`DepthConvention::NearIsOne`); `Plane::inverted()` gives far-is-one. Values compare within one
image only: no metres, no scale shared between images.

```rust,no_run
# use frust_onnx::{RgbaView, SessionOptions, VisionModels};
# let models = VisionModels::new(frust_models::ModelStore::open()?, SessionOptions::default());
# let (width, height, pixels) = (640, 480, vec![0u8; 640 * 480 * 4]);
let near_is_one = models.estimate_depth(RgbaView::new(width, height, &pixels)?)?;
let far_is_one = near_is_one.inverted();
# Ok::<(), Box<dyn std::error::Error>>(())
```

| Task | Model input (stretched, not letterboxed) | Pixels in | Output used |
|------|------------------------------------------|-----------|-------------|
| `segment_subject` | `input.1` f32 [1, 3, 320, 320] | RGB, ImageNet mean/std | first of seven [1, 1, 320, 320] outputs (`1959`), min-max normalised, resized to the image |
| `detect_faces` | `input` f32 [1, 3, 640, 640] | BGR, raw 0..255 | `cls`/`obj`/`bbox`/`kps` at strides 8, 16, 32 ([1, 6400/1600/400, 1/1/4/10]); score `sqrt(cls * obj)`, NMS at IoU 0.3, top 5000 |
| `estimate_depth` | `pixel_values` f32 [?, 3, ?, ?], fed 518x518 | RGB, ImageNet mean/std | `predicted_depth` [1, 518, 518] disparity, min-max normalised, resized to the image |

The decode and suppression are pure functions over output slices (`vision::decode_yunet`,
`vision::nms`, `vision::map_to_image`), usable on any YuNet run.

**Memory.** Every result plane is an f32 per image pixel, and the input image stays alive for
the call: a 12-megapixel photo is 46 MiB of RGBA in and 46 MiB of plane out. One Depth Anything
V2 Small session (518x518, the default `SessionOptions`) on a 4032x3024 photo peaked at
**294 to 305 MiB** resident over three fresh processes (x86_64 Linux host, i5-12600, release
build, two calls), from 56 MiB with the photo loaded; 202 to 213 MiB stayed resident afterwards
with the session warm. Keep at most one large photo and its planes in flight, and call
`models.cache().clear()` on the platform's memory-pressure signal. Reproduce with
`cargo test --release -p frust-onnx --test vision -- --ignored --exact measure_depth_memory --nocapture`
(Linux; set `FRUST_ONNX_MODEL_STORE` to pick where the models are downloaded).

### Measured performance

`examples/onnx-vision` release build on a OnePlus 9 (LE2115, Android 15, Snapdragon 888), ONNX
Runtime 1.31.0 linked statically, `SessionOptions::default()`. `Accelerator::Auto` is the CPU on
Android, so the CPU is the only provider measured there. Times are `VisionHandle::last_run`'s
`duration`: the worker's wall time for one request, pixel conversion and decode included.

| Task (model) | First run, model load included | Median of 5 warm runs, 640x480 sample | Median of 5 warm runs, 2048x1536 photo |
|--------------|-------------------------------:|--------------------------------------:|---------------------------------------:|
| `segment_subject` (U2-NetP) | 595 to 597 ms | 519 ms | 522 ms |
| `detect_faces` (YuNet) | 62 to 69 ms | 58 ms | 57 ms |
| `estimate_depth` (Depth Anything V2 Small, quantized) | 1,139 to 1,157 ms | 930 ms | 947 ms |
| U2-NetP alone, iPhone 13 mini, CPU (4 threads) | 306.6 ms, load not included | 278.8 ms (median of 10, one 320x320 tensor) | not measured |
| U2-NetP alone, iPhone 13 mini, CoreML | 165.6 ms after a 2.27 s load | 94.4 ms (median of 10, one 320x320 tensor) | not measured |

App peak RSS (`VmHWM`) in a fresh process: 171 MB after start, 245 MB with a 12-megapixel photo
picked and downscaled to 2048x1536, 409 MB after the first U2-NetP run, 458 MB with all three
sessions loaded; 499 MB after 30 runs over both images. A run with the app in the background
took about 1.7 times as long (1,649 ms for depth) and completed. The two iPhone rows are the
[iOS packaging](#7-ios-packaging) smoke, not this example: `Session::run_blocking` on a prepared
input tensor (no pixel conversion or decode), signed Release build, iOS 27.2. The tasks have no
other Apple or desktop row yet.

The ignored live tests (`cargo test -p frust-onnx --test vision -- --ignored`) download the three
catalog models (about 32 MB) into `target/` (or `FRUST_ONNX_MODEL_STORE`) and run each task on
synthetic images.

## 11. In a component

`frust_onnx::api::VisionHandle` (default `frust-api` feature) runs the vision tasks off the UI
thread and publishes what they return as signals. `build` reads the signals; a handler hands the
handle an image and returns at once:

```rust,ignore
use std::sync::Arc;
use frust::{Component, Get, View, button, column, text};
use frust_onnx::api::{VisionHandle, VisionStatus};
use frust_onnx::{SessionOptions, VisionModels};

struct State { vision: VisionHandle, photo: (Arc<[u8]>, u32, u32) }

// in `init`:
let models = VisionModels::new(frust_models::ModelStore::open()?, SessionOptions::default());
let vision = VisionHandle::new(models);

// in `build`:
let status = match state.vision.status().get() {
    VisionStatus::Idle => "Ready".to_owned(),
    VisionStatus::Running(task) => format!("{task:?}..."),
    VisionStatus::Failed(message) => message,
};
let mask = state.vision.mask().get(); // Option<Arc<Plane>>
column()
    .child(button("Cut out", |s: &mut State| {
        let (pixels, w, h) = s.photo.clone();
        s.vision.segment(pixels, w, h);
    }))
    .child(text(status))
```

| Request (returns at once) | Result signal | Value |
|---------|---------------|-------|
| `segment(pixels, width, height)` | `mask()` | `Option<Arc<Plane>>`, 0 background to 1 subject |
| `detect_faces(pixels, width, height, min_score)` | `faces()` | `Arc<Vec<Face>>`, highest score first |
| `estimate_depth(pixels, width, height)` | `depth()` | `Option<Arc<Plane>>`, near is 1 |

- `pixels` is RGBA8, `width * height * 4` bytes, shared as an `Arc<[u8]>` so the request copies
  nothing.
- `status()` is `Idle`, `Running(task)` or `Failed(message)`. A request made while the handle is
  idle shows `Running` before the call returns; `Idle` comes once the last queued result is
  published. A failure (a model not downloaded, a buffer of the wrong size) keeps the task's
  previous result and stays until the next request starts. `on_status` sees every status, in
  order, for logging; it runs on the UI thread, right after the status is written.
- **The newest request for a task wins.** Requests run one at a time on the handle's own worker
  thread, `frust-onnx-vision`, in the order made. A newer request for the same task drops a
  queued older one unrun; one already running finishes and its result is dropped, so `mask()`
  never goes back to an older image. Requests for different tasks do not replace each other.
- Every signal is written on the UI thread. The worker never writes one: it hands each result,
  run report and status to the UI thread, which writes them in that order on its next pump of the
  local task queue; a request call writes the waiting ones, `Running` included, before it
  returns. The worker never reads a signal and never waits on the UI thread. It starts on the
  first request, ends when the queue is empty, and holds no thread-local state.
- Make the requests on the UI thread (in `init`, a handler or a `frust::spawn_local` task), with
  the reactive runtime a running app has: on another thread a request panics before it changes
  anything.
- `last_run(task)` is a signal of the task's last published run, `Option<VisionRun>`: the
  provider of the session that ran it, its `duration` on the worker (queue wait excluded) and
  `loaded_model`, set when the run loaded the session, so the time includes the load. It is
  written with the result, before `Idle`; a failed or superseded run leaves it unchanged.
  `VisionModels::loaded_provider(task)` reads the same provider without loading anything.
- `missing_models()` lists the catalog models still to download (it reads the store: call it in
  `init` or a handler, not every frame). Keep your own clone of the `Arc<VisionModels>` passed
  to `VisionHandle::new` to clear its session cache on memory pressure.

`examples/vision_component.rs` is the whole component: a button that segments a generated image
and the mask status.

## 12. Testing

```sh
cargo test -p frust-onnx                    # unit, session, image, catalog, vision and api tests
cargo test -p frust-onnx --examples         # builds examples/vision_component.rs
cargo test -p frust-onnx --test vision -- --ignored    # live: the three catalog models (~32 MB)
cargo test -p frust-onnx --test catalog -- --ignored   # live: every catalog pin still resolves
```

The default tests need the build-time ONNX Runtime download on the first build of a machine and no
network afterwards; the session tests run small models under `tests/fixtures`. `tests/api.rs`
drives `VisionHandle` over a stub runner, so it loads no model: it checks the status order, that
a superseded request's result is dropped, that an error becomes `Failed`, that each task's
`last_run` reports the published run's provider and duration, and that every signal write lands
on the UI thread, also while the UI reads every signal in a tight loop as 2,000 requests
complete. The registry entry
and its Android and iOS edits are tested in `frust-drive` (`cargo test -p frust-drive plugin::`).

## 13. License

The crate is MIT OR Apache-2.0. A binary that ships it ships ONNX Runtime (MIT, Microsoft) and
the third-party components ONNX Runtime contains: see [`NOTICE`](NOTICE).
