# Frust - AI Architecture

## Overview

The AI unit is the two on-device model plugins: `frust-models` (`plugins/models`), which stores
model files and downloads them, and `frust-onnx` (`plugins/onnx`), which runs ONNX models over
ONNX Runtime and ships three vision tasks on top. Both sit in the plugin tier beside the `frust`
facade, like every plugin in [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md): an app lists them
in its own `Cargo.toml`, and the facade never names them. `frust-models` knows no model format and
does no inference; `frust-onnx` knows no download protocol and reads its models from a
`frust-models` store. `examples/onnx-vision` is the reference app (see
[ARCHITECTURE.md](ARCHITECTURE.md)'s Examples table).

This spoke also carries the unit's pins, gates and manual tests (there is no `AI_DEVELOPMENT.md`;
see [DOC_POLICY.md](DOC_POLICY.md)). Numbers, tables and full command lines live in the plugin
READMEs, [plugins/models/README.md](../plugins/models/README.md) and
[plugins/onnx/README.md](../plugins/onnx/README.md); this doc links to them rather than copying.

## Module Structure

`plugins/models` (`frust-models`):

| Module | Responsibility |
|--------|-----------------|
| `store`, `id` | `ModelStore`: the file layout under `<data_dir>/frust-models`, one file per `namespace/name/path` `ModelId`, with a `.part` sidecar for a download in progress and a `.frust-meta.json` manifest beside each finished file; list, get, import, remove, partial and total sizes. Identifier rules keep every id inside the root and off the sidecar names |
| `hub` (default `hub` feature) | `HubClient`: a blocking Hugging Face Hub client (search, repository listing with sizes and LFS digests, file URLs, bearer token sent only to the Hub origin) over `ureq` on rustls with the bundled Mozilla roots; it follows redirects itself and checks every hop |
| `download` (`hub`) | `Downloader`: one job (one or more files, in order) on its own `frust-models-dl` thread, reporting `DownloadEvent`s to a sink; resume by `Range`, SHA-256 and size checks before a file moves into place, a size bound, cancel, one job per target file |
| `apple` | Excludes the store and every finished file from iOS and macOS device backups (best effort) |
| `api` (default `frust-api` feature) | `ModelList` and `DownloadTracker`: signals of the stored models and of one download's state and progress, the only code that names `frust` |

`plugins/onnx` (`frust-onnx`, `#![forbid(unsafe_code)]`):

| Module | Responsibility |
|--------|-----------------|
| `session`, `tensor`, `runtime` | `Session` from a file or bytes, its named inputs and outputs, typed tensors (f32, u8, i64) checked against the model before the runtime sees them, `run_blocking`; one process-wide ONNX Runtime environment created on first use, its failure cached; `runtime_info` reports the release and the link kind |
| `accelerator` | `Accelerator` policy and the `Provider` a session really runs on: on Apple targets `Auto` is CoreML only for a model whose inputs are all static in shape (read by a metadata-only probe load) and the CPU otherwise; the CPU elsewhere; a CPU fall back unless `strict_accelerator` |
| `worker` | `Session::run`: one `frust-onnx-run` thread per session, runs queued in order, cancel through ONNX Runtime's terminate flag, exactly one `RunEvent` per run |
| `cache` | `SessionCache`: a bounded LRU of loaded sessions keyed by model path and `SessionOptions`, shared loads, eviction on demand |
| `image` | RGBA views to normalised NCHW tensors and result planes back to display sizes; no image-codec dependency |
| `catalog` (default `catalog` feature) | `VISION_CATALOG`: three vision models with pinned source, size, SHA-256 and licence, each turning into a verified `frust-models` `DownloadRequest` |
| `vision` (default `vision` feature) | `VisionModels`: subject mask, face detection and relative depth, one blocking call each from RGBA pixels, each task on its own cached session read from the store, no fall back when a model is missing |
| `api` (default `frust-api` feature) | `VisionHandle`: the vision tasks on a `frust-onnx-vision` worker thread, results and status as signals, the only code that names `frust` |

## Platform Status

ONNX Runtime is linked statically from a prebuilt archive the `ort` build script fetches, on the
targets that have a prebuilt row; every other target loads the library at run time and the app
ships none, so every session there returns `OnnxError::Runtime`. The per-target table and the
packaging detail are in [plugins/onnx/README.md](../plugins/onnx/README.md) (Status, Android
packaging, iOS packaging).

| Target | `frust-onnx` runtime | What has run |
|--------|----------------------|--------------|
| Linux (glibc, x86_64/aarch64) | static, plain archive | host tests of both crates; the example's Linux desktop leg is not run |
| Apple-silicon macOS | static, `coreml` archive | built and run on an Apple M4; CoreML has run (`onnx-coreml-unverified`) |
| Windows (MSVC) | static, `directml` archive (DirectML not registered) | never built (`onnx-macos-windows-unverified`) |
| Android arm64-v8a | static, `nnapi` archive (NNAPI not registered: CPU only), plus the NDK's `libc++_shared.so` in the APK | the example's Android release leg passed on a phone (`onnx-android-cpu-only`, `onnx-android-binary-size`) |
| Android x86_64, armeabi-v7a | run-time loading, none shipped | compiles only (`onnx-android-arm64-only`) |
| iOS arm64 device and arm64 Simulator | static, `coreml` archive; the app's link adds `-lc++`, CoreML and Foundation; iOS 15.1 or later | CoreML has run on an iPhone (`onnx-ios-arm64-from-15-1`) |
| x86_64 Simulator, Mac Catalyst, Intel macOS | run-time loading, none shipped | Intel macOS not built (`onnx-no-intel-mac-or-x86-ios-sim`) |

`frust-models` is pure Rust on every target; its only platform code is the Apple backup exclusion
and the UI-thread checks (Android Looper, Apple main thread). On Android a release build needs
`android.permission.INTERNET` in the main manifest, which `frust plugin add models` and
`frust plugin add onnx` write (`android-release-internet-from-plugin-only`). Limitation ids above
are entries in [LIMITATIONS.md](LIMITATIONS.md); its `models-*` and `onnx-*` entries list the
remaining gaps (no free-space check, no resume validator, no verification of imported files,
face detection unverified on a real face).

## Layer Dependencies

- **Platform tier with optional facade glue.** Each crate is the single-crate shape
  [PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md) describes: a default-on `frust-api`
  feature gates its one `frust` (published as `frust-ui`, `default-features = false`) edge, so
  `--no-default-features` leaves no framework crate in the graph. `frust-models` otherwise takes
  `frust-paths` (the store root), `frust-plugin` plus `jni` on Android under `hub` (the UI-thread
  check), `objc2`/`objc2-foundation` on Apple, and `ureq`/`rustls`/`webpki-roots`/`sha2` under
  `hub`; with `hub` off it has no HTTP or TLS dependency. `frust-onnx` otherwise takes `ort`
  (target-gated rows, one per prebuilt archive or the run-time loader) and `frust-paths` on Apple
  (the CoreML compiled-model cache).
- **The plugin-to-plugin edge.** `frust-onnx`'s `catalog` feature (default on, implied by
  `vision`) depends on `frust-models` with `default-features = false, features = ["hub"]`: the
  downloader and store only, no `frust-ui`. It is the same kind of edge as `frust-photo-library`
  on `frust-file-dialogs` ([PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)'s Layer
  Dependencies) but gated by a cargo feature rather than a target, since the catalog is used on
  every target; `--no-default-features` removes it. `frust-models` never depends on `frust-onnx`.
- **No third-party type in the public API.** `ort`, `ureq` and the TLS crates stay private:
  sessions, tensors, providers, errors and download events are each crate's own types, so the
  `ort` release candidate can move without an API break.
- **The facade boundary holds.** Neither crate is re-exported by `frust`; tooling reaches them
  through `frust-drive`'s registry entries `models` and `onnx`
  ([CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md)'s plugin-add flow).

```
app ──► frust (facade)
 ├────► frust-onnx ──[catalog]──► frust-models ──► frust-paths, frust-plugin (Android), ureq/rustls
 │          └──► ort ──► ONNX Runtime (static archive, or loaded at run time)
 └────► frust-models
        each crate's `api` module ──[frust-api]──► frust
```

## Data Flow

- **A download.** A component handler calls `DownloadTracker::start_hub` (or `start` with a
  request it already has) and returns: the state is `Running` at once, the repository listing is
  fetched on `frust::spawn_blocking`, and `Downloader::start` runs the job on its own thread. That
  thread writes the `.part` file, hashes while writing, checks size and digest, renames the file
  into the store with its manifest, and feeds `DownloadEvent`s back; the tracker queues them as
  writes the UI thread applies in order, and a linked `ModelList` rescans on the download thread,
  its listing written on the UI thread before `Done`. The download thread writes no signal, never
  reads one, never waits on the UI thread and never calls back into the downloader. A blocking
  Hub call made on the UI thread fails at once with `ModelsError::UiThread` on Android and Apple.
- **An inference.** A handler hands `VisionHandle` an `Arc<[u8]>` of RGBA pixels and returns; the
  status is `Running` before the call returns. The handle's worker thread runs the request
  through `VisionModels`, which loads the task's model from the store on first use through its
  `SessionCache` (one session per task, checked to fit the task), converts the pixels to a
  tensor, runs the session on ONNX Runtime and decodes the output into a `Plane` or `Face`s. The
  worker queues the result signal and `last_run` (provider, run time, whether the run loaded the
  session), then `Idle` once the queue is empty; the UI thread writes them in that order. The
  newest request for a task drops a queued older one, and a superseded result is never published.
  A model missing from the store is `OnnxError::ModelMissing`; nothing is substituted.
- **Threads.** The three handles write signals only on the UI thread. Starts, requests, `refresh`
  and `remove` must be called there and panic elsewhere; a call applies waiting writes before it
  returns, and observers (`on_state`, `on_status`) run on the UI thread right after a write.
- **Memory pressure.** The app keeps its own clone of the `Arc<VisionModels>` and clears its
  session cache on the platform's memory-pressure signal. The sizes behind the session-cache bound
  and the memory defaults (no CPU arena, no memory pattern) are in the onnx README's Sessions and
  Session cache sections.

## Key Types

| Type | Purpose |
|------|---------|
| `ModelStore` / `ModelId` / `LocalModel` / `ModelManifest` | The on-disk store, a model's `namespace/name/path` identity, a stored file with its manifest (source, size, digest, licence, verified flag) |
| `HubClient` / `DownloadRequest` / `Downloader` / `DownloadHandle` / `DownloadEvent` | Hub access, one download job (one or more files), its start, its cancel/wait handle and its event stream |
| `ModelsError` / `HubError` | The store and download error enums, `UiThread` among them |
| `ModelList` / `DownloadTracker` / `DownloadState` / `DownloadProgress` | `frust-models`' reactive handles (`frust-api`) |
| `Session` / `SessionOptions` / `Tensor` / `TensorInfo` | A loaded model, its load options (accelerator, threads, optimisation, memory behaviour), typed tensors and input/output descriptions |
| `Accelerator` / `Provider` / `RuntimeInfo` | The requested execution provider, the one a session really runs on, and the linked ONNX Runtime release and link kind |
| `RunHandle` / `RunEvent` / `SessionCache` | A queued run on a session's worker and its single outcome; the bounded session cache |
| `VisionModels` / `VisionModel` / `Plane` / `Face` / `VisionRun` | The vision tasks over a store, a catalog row, a per-pixel result, a detected face, one published run's provider and time |
| `VisionHandle` / `VisionStatus` | `frust-onnx`'s reactive handle (`frust-api`) and its status signal value |
| `OnnxError` | The crate's one error enum (`Runtime`, `Load`, `InvalidInput`, `Run`, `Cancelled`, `Unsupported`, `ModelMissing`) |

## Native Runtime and Pins

- **Packaging.** The ONNX Runtime library ships inside the app and is never downloaded at run
  time; models are data and are downloaded, verified, through `frust-models`. On Android arm64 the
  runtime is linked into the app's own `.so`, so the plugin has no Gradle module; its registry
  entry contributes `AndroidLibcxxShared`, which makes the scaffold's `cargoNdkBuild` package the
  NDK's `libc++_shared.so` the archive needs (a hot `frust run --watch` does not:
  `onnx-hot-run-libcxx-shared`). On arm64 iOS the runtime is linked statically too; the entry contributes `-lc++` (`IosLibrary`) and the CoreML and Foundation `IosFramework` links, and requires iOS 15.1. The convention every
  plugin shipping a native runtime follows is in
  [PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md)'s Plugin Conventions.
- **Build-time fetch.** The first build on a machine downloads the prebuilt archive for the
  target and caches it under the user cache directory; `cargo --offline`, `ORT_OFFLINE` and
  `ORT_SKIP_DOWNLOAD` skip the cache too, so an offline first build fails to link
  (`onnx-runtime-build-time-fetch`). Both crates are root-workspace members, so the standard
  verify gate makes that fetch.

| Pin | Why | Tripwire |
|---|---|---|
| `ort =2.0.0-rc.15` exact, frust-owned, `default-features = false` (root `Cargo.toml`) | Binds ONNX Runtime 1.31.0; every desktop target's `ort` feature set equals one prebuilt row exactly (no `lax-feature-matching`). A release candidate (`onnx-ort-release-candidate-pin`) that needs Rust 1.92, so `frust-onnx` carries its own `rust-version = "1.92"` above the workspace's 1.89 (`onnx-needs-rust-1-92`) | the onnx gate suite below, run as one change with the bump |
| `ureq ~3.5`, `default-features = false`, `rustls-no-provider` | `frust-models`' blocking Hub client; the plugin builds its own rustls config on ring with `webpki-roots` (no platform verifier, native-tls or gzip). Tilde, because its TLS connector uses `ureq`'s `unversioned` API, which may change in any minor release. `rustls`, `webpki-roots` and `sha2` ride the rows in [PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md)'s Version Pins | `cargo test -p frust-models` plus the ignored live Hub test |

**Bumping `ort`** ([plugins/onnx/README.md](../plugins/onnx/README.md)'s Pin policy is the full
procedure): move the root pin; check that every prebuilt row `plugins/onnx/Cargo.toml` selects
still exists with the same feature set, and the new release against `api-17` and the crate's
`rust-version`; run the onnx gate suite (host tests on each desktop prebuilt row, the ignored
live tests, the Android and iOS compile gates, and one device smoke per platform that ships a
runtime: on Android a `frust plugin add onnx` app built as a release APK whose `lib/arm64-v8a`
passes `scripts/ci/page-size-check.sh`, started on a phone); then update the README's versions
and measurements.

## Gates and Manual Tests

- **Host.** `cargo test -p frust-models` runs against an in-process loopback Hub and touches no
  network; `cargo test -p frust-onnx` needs only the build-time fetch. Both ride the standard
  verify gate. `examples/onnx-vision` gates from its own directory and is in CI's standalone
  matrix ([DEVELOPMENT.md](DEVELOPMENT.md)'s Test section).
- **Mobile compile gates.** Both crates are in the C-building Android and iOS checks of
  [DEVELOPMENT.md](DEVELOPMENT.md)'s Test section: `ring` compiles C, so Android goes through
  `cargo ndk` and iOS needs a macOS host. They type-check the Rust only; no iOS link has run.
- **Ignored network tests** (each downloads, so none runs in the gate):

```bash
cargo test -p frust-models -- --ignored                  # one live Hub download
cargo test -p frust-onnx --test vision -- --ignored      # the three catalog models, each task
cargo test -p frust-onnx --test catalog -- --ignored     # every catalog pin still resolves
(cd examples/onnx-vision && cargo test --test live -- --ignored --nocapture)
```

- **Device checklist** (release build of `examples/onnx-vision`, default `SessionOptions`; the
  full checklist and the recorded results are in its README's Device status):
  1. a fresh install lists the three catalog rows, none installed;
  2. a download shows live progress, cancels, resumes from the kept bytes and ends verified;
  3. a lost connection mid-download gives a clean error, then a resume;
  4. the three tasks on the sample, overlays right, tabs switch during a run;
  5. the three tasks on a picked photo of at least 12 MP;
  6. the median of five warm runs per task and provider, and peak memory;
  7. the app sent to the background during a run;
  8. a relaunch keeps the models; Delete returns the row to not installed.

  Only the Android leg has run: it passed on one arm64 phone with the downloads served from a
  host mirror rather than the internet. The iPhone, macOS and Linux desktop legs have not run,
  and a face check on a real photograph is owed (`onnx-face-detection-unverified`). A
  `frust plugin add onnx` app with no hand edit has been built and started on Android.

## See Also

- [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md) — the plugin tier and its charter
- [PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md) — plugin conventions, the native-runtime rule included
- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, the verify gate, the mobile compile gates
- [LIMITATIONS.md](LIMITATIONS.md) — the `models-*` and `onnx-*` entries
