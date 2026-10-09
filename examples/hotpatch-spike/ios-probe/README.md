# ios-probe

The hot-reload spike's iOS simulator load probe, the counterpart of `../android-probe/`. It
answers one question: can a Frust app launched with `xcrun simctl launch` load a code library
that arrives after install, from its own data container, unsigned or only ad-hoc signed? The
results are in `../RESULTS.md` under "Phase 2: iOS simulator load probe".

**Scope: loading only, simulator only.** The probe library exports one C function and nothing
else. No jump table is installed and nothing is relocated against the running app. Building a
patch that is linked against the base executable's addresses is the patch builder's job
(PORT.md), not this probe's. A physical iOS device is out of scope: it will not map an image that
is not signed by a trusted identity.

```
ios-probe/
├── probe.sh     scaffold, build, install, deliver, launch, capture (the whole procedure)
├── app_lib.rs   src/lib.rs of the throwaway probe app
└── patch/       standalone cdylib crate: `frust_probe_value() -> u32` returns 42
```

## Strategies

`probe.sh` delivers four files into `frust_paths::data_dir()`, which on iOS is
`$HOME/Library/Application Support` inside the app's data container. The app's root component
tries a strategy once in `init`, before the first frame, and logs one line at info level (the iOS
shell's stderr logger, carried by `simctl launch --console-pty`):

| Strategy | File | What it does |
|---|---|---|
| `plain-dlopen` | `libfrust_probe_unsigned.dylib` | The dylib after `codesign --remove-signature`, opened with `frust_hotpatch::load_patch_library` (plain `dlopen` through libloading off Android). The signature is removed on purpose: arm64 `ld` leaves a linker ad-hoc signature on everything it links. |
| `linker-signed` | `libfrust_probe_linker.dylib` | The dylib exactly as cargo / `ld` wrote it, with the linker's ad-hoc signature (`flags=0x20002(adhoc,linker-signed)`), opened the same way. This is what an unmodified thin link produces. |
| `adhoc-signed` | `libfrust_probe_adhoc.dylib` | The same dylib after `codesign -f -s -`, opened the same way. |
| `negative-control` | `libfrust_probe_control.dylib` | 11 bytes of text. The loader's own error must reach the log; this proves a failed load is visible at all. |

Each loaded library is resolved for `frust_probe_value` and the function is called. The line is
`frust-probe: strategy=<name> result=<value>` on success and
`frust-probe: strategy=<name> error=<loader error>` on failure. Before each load the app also logs
`frust-probe-attempt: strategy=<name> pid=<pid> path=<path>`, so a process killed during a load
still leaves a trace. No container path is hard-coded in the app.

## Running

Prerequisites: an Apple Silicon Mac with Xcode (`docs/DEVELOPMENT.md`), a booted iOS simulator
(Xcode 27 ships no Simulator.app; `xcrun simctl boot <udid>` is enough) and the
`aarch64-apple-ios-sim` Rust target.

```sh
examples/hotpatch-spike/ios-probe/probe.sh [--udid <udid>] [--keep] [--log-dir <dir>]
```

`--udid` defaults to `$SIMULATOR_UDID`, else the only booted simulator. `probe.sh`:

1. scaffolds `probeapp` into a fresh `mktemp -d` dir with the repo's own CLI
   (`cargo run -p frust-cli -- create ... --frust-path <repo>/crates/frust --platforms ios`),
2. overlays `app_lib.rs` as its `src/lib.rs` and `cargo add`s `frust-hotpatch` and `frust-paths`
   (path dependencies on this checkout) and `libloading@0.8.8`,
3. builds `Debug-iphonesimulator` with the `xcodebuild` invocation `frust run` uses
   (`frust-drive` `ios_run/xcodebuild.rs`) and installs it with `xcrun simctl install`,
4. copies `patch/` into the temp dir, builds it there with
   `cargo build --release --target aarch64-apple-ios-sim` (so no `target/` or `Cargo.lock` lands
   in the checkout) and stages the four files above, recording `codesign -dv` for each and for
   the app executable,
5. resolves the data container with `xcrun simctl get_app_container <udid> <bundle-id> data`
   (after one launch if the container does not exist yet) and copies the files into its
   `Library/Application Support/`,
6. launches the app once per strategy with
   `SIMCTL_CHILD_FRUST_PROBE_STRATEGY=<name> xcrun simctl launch --console-pty <udid> <bundle-id>`,
   so every load runs in a fresh process, waits for the `frust-probe:` line, runs
   `vmmap -w <pid>` on the live process and terminates it,
7. removes the four files and launches once more without a strategy filter: all four must then
   report `patch file missing`,
8. prints the device, the container path shape, the per-strategy lines and any host unified-log
   line naming `libfrust_probe` since the start of the launches.

The udid is replaced with `<udid>` in everything printed or logged. On exit it removes the temp
dir and uninstalls the app; `--keep` keeps both. `--log-dir` also writes `device.txt`,
`container.txt`, `signatures.txt`, `probe.txt`, `syslog.txt`, `console-<launch>.txt`,
`vmmap-<launch>.txt` and `screen-<launch>.png` into an existing directory. The exit status is 0
when a `frust-probe:` line was captured for every strategy, whatever it reports. The probe refuses
to run when an app with the probe's bundle id is already installed.

Nothing the probe builds is committed: the scaffolded app, its build products and the dylib live
only in the temp dir.
