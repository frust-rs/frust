# android-probe

The hot-reload spike's Android load probe. It answers one question: can an installed Frust app
(SELinux domain `untrusted_app`, the app's linker namespace) load a code library that arrives
after install, and by which path? The results are in `../RESULTS.md` under "Phase 2: Android load
probe".

**Scope: loading only.** The probe library exports one C function and nothing else. No jump table
is installed and nothing is relocated against the running app library. Building a patch that is
linked against the base library's addresses is the patch builder's job (PORT.md), not this probe's.

```
android-probe/
├── probe.sh     scaffold, build, install, deliver, launch, capture (the whole procedure)
├── app_lib.rs   src/lib.rs of the throwaway probe app
└── patch/       standalone cdylib crate: `frust_probe_value() -> u32` returns 42
```

## Strategies

The probe app's root component runs all three in `init`, once, before the first frame, and logs
one line per strategy at info level (logcat tag `frust`):

| Strategy | What it does |
|---|---|
| `memfd` | `frust_hotpatch::load_patch_library` on `<files dir>/libfrust_probe_patch.so`. On Android it copies the bytes into a memfd and opens it with `android_dlopen_ext(ANDROID_DLEXT_USE_LIBRARY_FD)`. |
| `plain-dlopen` | `libloading::Library::new` (plain `dlopen`) on the same files-dir path. |
| `cache-dir` | The file copied into the app's cache dir, then plain `dlopen`. |

Each loaded library is resolved for `frust_probe_value` and the function is called. The line is
`frust-probe: strategy=<name> result=<value>` on success and
`frust-probe: strategy=<name> error=<loader error>` on failure. The files and cache dirs come from
`frust_paths::data_dir()` / `cache_dir()`, which the Android shell installs from
`Context.getFilesDir()` / `getCacheDir()` before the root component is built. No package path is
hard-coded.

## Running

Prerequisites are the usual Android ones (`docs/DEVELOPMENT.md`): `ANDROID_HOME` with an NDK,
`cargo-ndk`, the `aarch64-linux-android` Rust target, a JDK for Gradle, and `adb`. The device must
be unlocked so the launched activity reaches the foreground.

```sh
examples/hotpatch-spike/android-probe/probe.sh --serial <adb-serial> [--keep] [--log-dir <dir>]
```

`probe.sh`:

1. scaffolds `probeapp` into a fresh `mktemp -d` dir with the repo's own CLI
   (`cargo run -p frust-cli -- create ... --frust-path <repo>/crates/frust --platforms android`),
2. overlays `app_lib.rs` as its `src/lib.rs` and `cargo add`s `frust-hotpatch` and `frust-paths`
   (path dependencies on this checkout) and `libloading`,
3. builds the debug APK for arm64-v8a (`frust build apk --debug --target-platform android-arm64`)
   and installs it with `adb install -r` (debug, so `run-as` works),
4. copies `patch/` into the temp dir and builds it there with
   `cargo ndk -t arm64-v8a build --release`, so no `target/` or `Cargo.lock` lands in the checkout,
5. pushes the `.so` to `/data/local/tmp` and copies it into the app's files dir with
   `adb shell run-as <applicationId> cp`,
6. force-stops and launches the app, then prints the device identity, the `frust-probe:` lines from
   `adb logcat -d`, and any `avc: denied` lines logged meanwhile.

On exit it removes the pushed file, the temp dir and the installed app; `--keep` keeps the temp
dir and leaves the app installed so its screen can be inspected. `--log-dir` also writes
`device.txt`, `probe.txt` and the full `logcat.txt` into an existing directory. The exit status is
0 when all three strategy lines were captured, whatever they report.

Nothing the probe builds is committed: the scaffolded app, its APK and the `.so` live only in the
temp dir.
