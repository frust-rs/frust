# onnx-vision

On-device vision in a Frust app, built the way an app author uses `frust-models` and
`frust-onnx`: download a verified model, then run it on a photo without blocking the UI.

Two destinations behind a Material 3 navigation bar:

- **Models**: one row per `frust_onnx::catalog::VISION_CATALOG` model (U2-NetP, YuNet, Depth
  Anything V2 Small) with its task, size, licence and input size, whether it is installed, and
  Download, a live progress line (bytes, percent, speed) from `DownloadTracker`, Cancel, Resume
  (starting again continues from the kept partial file) and Delete. Above the rows: the store's
  disk use (`ModelStore::total_bytes` plus partial downloads) and, on a device with less than
  4 GB of RAM (`DeviceMemory::query`), a note that memory grows with each loaded model session.
  The recorded device run (OnePlus 9, Android 15, release build) peaked at 458 MB in a fresh
  process with all three sessions loaded, and 499 MB after 30 runs over two photos; the figure
  is that app's total peak RSS, not a per-session cost.
- **Vision**: the bundled sample or a photo picked with the system picker, and three actions
  driven by `VisionHandle`: Subject mask (a tinted alpha layer over the photo), Faces (boxes
  and the five landmarks drawn over it) and Depth (a grayscale map, near is white). An action
  stays disabled, with a "download the model first" hint, until its model is in the store. The
  status line shows the shown task's `VisionHandle::last_run`: the execution provider that ran
  the session and the run's time in milliseconds, marked when the run included loading the
  model.

Nothing blocking runs on the UI thread: downloads run on their own threads, and photo
decoding, the 2048 px downscale, disk walks, overlay conversion and inference run on blocking
threads. Photos are downscaled to at most 2048 px on the long side before inference.

## Run it

```sh
cd examples/onnx-vision
cargo run                                   # desktop (Linux, macOS, Windows)
frust run -d <device>                       # an Android phone or the iOS simulator
frust build apk --target-platform android-arm64
```

`ios/FrustEmbedding` is not tracked in the repository: `frust create`, `frust run` and
`frust build ios` generate it (a machine-local symlink to the embedding package). A bare
`xcodebuild` of `ios/`, or opening `ios/Runner.xcodeproj` in Xcode before any of those has run,
fails at package resolution. Run `frust build ios` once first, then open the project.

- **Desktop**: ONNX Runtime is linked statically on Linux (glibc), Apple-silicon macOS and
  Windows. The first build downloads its prebuilt archive (about 11 MB) and caches it. Picked
  photos are decoded natively on macOS (HEIC included) and as PNG or JPEG on Linux and Windows.
- **Android**: arm64-v8a only. ONNX Runtime is linked statically into `libonnxvision.so`, which
  then needs the NDK's `libc++_shared.so` in the APK; `frust plugin add onnx` set
  `frust.cargoNdk.linkLibcxxShared=true` in `android/gradle.properties` for that. A project
  whose `android/app/build.gradle.kts` predates that property needs this line in the
  `cargoNdkBuild` command line, right after `add("ndk")`:

  ```kotlin
  if ((project.findProperty("frust.cargoNdk.linkLibcxxShared") as String?)?.trim().equals("true", ignoreCase = true)) add("--link-libcxx-shared")
  ```

  A release build needs signing values (`android/key.properties` or the `ANDROID_*`
  variables; see `frust.toml`). `android/app/src/main/AndroidManifest.xml` carries the
  `INTERNET` permission: the scaffold declares it only for debug and profile builds, and without
  the line in the main manifest every release download fails (`Operation not permitted`).
  `frust plugin add models` and `frust plugin add onnx` write that line to the main manifest
  for a new project, so a project created that way has it without a hand edit.
- **iOS**: `frust-onnx` links ONNX Runtime statically on an arm64 device and the arm64
  Simulator, and `frust plugin add onnx` adds the link lines and requires iOS 15.1. This example
  has not been walked on an iPhone yet. The x86_64 Simulator loads the runtime at run time and
  ships none, so every task there fails at session creation with `OnnxError::Runtime`, shown as
  a Failed status line.

The models are stored under the app data directory (`<data_dir>/frust-models`); Delete removes
a model, its partial download and its manifest.

## Tests

A standalone workspace (its own `[workspace]` and `Cargo.lock`, excluded from the root one), so
it gates from its own directory:

```sh
cd examples/onnx-vision
cargo test --locked && cargo clippy --all-targets --locked -- -D warnings && cargo fmt --check
```

- `tests/app.rs` boots the app headless (`frust::testing::Tester`, no GPU and no window) and
  checks the Models screen lists three rows, and that with an empty store the three Vision
  actions are disabled with their hints once the sample has decoded.
- `tests/format.rs` pins the progress line for every `DownloadState`, the partial and storage
  lines and the low-memory threshold.
- `tests/live.rs` is ignored by default because it downloads: through the real components it
  downloads U2-NetP from the Models screen, runs Subject mask on the sample from the Vision
  screen, prints both times and checks the mask (the ball is subject, the sky is not):

  ```sh
  cargo test --test live -- --ignored --nocapture
  ```

## The sample image

`assets/sample.png` (640x480, about 42 KB) is generated, not photographed:
`assets/make_sample.py` draws a shaded ball in front of a block on a tiled floor under a sky
with nothing but arithmetic (standard-library Python), and writes the same bytes on every run:

```sh
python3 -I assets/make_sample.py assets/sample.png
```

It contains no person and no third-party material, and is covered by this repository's licence
(MIT OR Apache-2.0). With no face in it, Faces finds none on the sample: pick a photo to try it.

## How it was made

Scaffolded and wired with the `frust` CLI built from this checkout, run inside the new
directory so the relative `--frust-path` means the same thing to the CLI and to Cargo:

```sh
cargo build -p frust-cli                    # from the repository root
mkdir examples/onnx-vision && cd examples/onnx-vision
frust create . --platforms android,ios --frust-path ../.. --org it.f0x --project-name onnxvision \
  --description "On-device vision with frust-models and frust-onnx: download catalog models, then mask, faces and depth"
frust plugin add models
frust plugin add onnx
frust plugin add photo-library --feature decode
frust plugin add device-info
```

The plugin applier wrote the Cargo dependencies, the `:frust-photo-library` Gradle module, the
`PhotosUI` and `ImageIO` Xcode links and the `linkLibcxxShared` property. By hand: the
`[workspace]` header in `Cargo.toml` (as every standalone example has), the plugin paths
shortened from `../../crates/frust/../../plugins/<id>` to `../../plugins/<id>`, the
`frust-ui` `test-support` dev-dependency, this README, the sources and tests, the root
`Cargo.toml` `exclude` entry and the `crates/frust-cli/tests/profile_sync.rs` roster entry
(this manifest keeps the profile blocks `frust create` emitted).

| Piece | Where |
|-------|-------|
| Shell (app bar, navigation bar, both screens kept mounted), `frust::app!` | `src/lib.rs` |
| Shared handles: store, list, trackers, vision handle | `src/services.rs` |
| Models screen | `src/models_screen.rs` |
| Vision screen, overlay canvas | `src/vision_screen.rs` |
| Decoding, the 2048 px cap, overlay images | `src/imaging.rs` |
| Status text | `src/format.rs` |
| Generated Android and iOS projects | `android/`, `ios/` |

## Device status

Release builds, `SessionOptions::default()`. Checklist: (1) fresh install lists three rows, none
installed; (2) U2-NetP download with live progress, cancel, resume from the kept bytes, verified;
(3) the other two, with the connection lost mid-download, a clean error and a resume; (4) the
three tasks on the sample, overlays right, tabs switch during a run; (5) the three tasks on a
picked photo of at least 12 MP; (6) median of five warm runs per task and provider; (7) the app
sent to the background during a run; (8) relaunch keeps the models; (9) Delete returns the row
to not installed and the storage line drops.

| Device | OS | Packaging | 1 | 2 | 3 | 4 | 5 | 6: median warm ms (CPU) | 7 | 8 | 9 | Peak memory | App size |
|--------|----|-----------|---|---|---|---|---|-------------------------|---|---|---|-------------|----------|
| OnePlus 9 (LE2115, arm64) | Android 15 | ONNX Runtime 1.31.0 static in `libonnxvision.so` + `libc++_shared.so` | pass | pass | pass (connection lost by stopping the server) | pass | pass (4032x3024 JPEG, decoded at 2048x1536) | mask 519 / 522, faces 58 / 57, depth 930 / 947 (sample / photo); first runs with load 597, 69, 1,157 | pass (same process, the run completed, 1,649 ms) | pass | pass (32.1 MB to 27.5 MB) | 458 MB fresh with all three sessions; 499 MB after 30 runs | APK 33,366,987 B (app `.so` 31,780,904 B, `libc++_shared.so` 1,253,544 B) |
| iPhone | iOS | static, `coreml` archive | not run | not run | not run | not run | not run | not run | not run | not run | not run | not run | not run |
| Mac (Apple silicon) | macOS | static, `coreml` archive | not run | not run | not run | not run | not run | not run | not run | not run | not run | not run | not run |
| Linux desktop | glibc x86_64 | static, plain archive | not run | not run | not run | not run | not run | not run | not run | not run | not run | not run | not run |

Android notes. `Accelerator::Auto` is the CPU on Android, so one provider is measured. The phone
had no internet during the run, so rows 2 and 3 downloaded the same pinned files (SHA-256
checked by the app) from a computer over USB (`adb reverse` to a loopback HTTP server that
honours `Range`); row 3's lost connection was that server stopping mid-download. Faces found no
face on either image, as expected for images without a person: a check on a real face is owed.
Depth put the sample's ball and the photo's sharp foreground disc near (white) and the sky and
the blurred backdrop far, which matches near-is-one. The iPhone, macOS and Linux desktop rows
are owed (Apple hardware; a Linux display server).

Hot run (Pixel 5, Android 14, `7b299d189`, 2026-11-28): `frust run --watch -d <device>` with
`frust.cargoNdk.linkLibcxxShared=true` installs an APK whose `lib/arm64-v8a` holds only
`libonnxvision.so`. The app dies at launch (`UnsatisfiedLinkError: dlopen failed: library
"libc++_shared.so" not found`) and the CLI reports that it did not report a pid. Use
`--watch --no-hot` or a plain `frust run` (`onnx-hot-run-libcxx-shared` in `docs/LIMITATIONS.md`).
