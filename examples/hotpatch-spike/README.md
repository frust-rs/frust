# hotpatch-spike

A throwaway measurement app for the hot-reload spike: can `dx serve --hot-patch` (dioxus-cli
0.7.10; subsecond in Phase 1, Frust's own `crates/frust-hotpatch` runtime since Phase 2) patch a
running Frust desktop app, and how fast is edit-to-frame compared with a
rebuild and relaunch? It is the `frust create` counter with baseline Frust widgets only (no design
system), built against `frust-ui` with the opt-in `hotpatch` feature. With that feature, every
`component(..)` child's `build` runs through the hot-patch seam's jump table (frust-hotpatch), and the desktop shell logs
`frust-hotpatch: applied t_unix_ms=<ms>` when a patch lands and `frust-hotpatch: frame t_unix_ms=<ms>`
after the next frame (also once after the first startup frame).

It is a standalone workspace, listed in the root `Cargo.toml` `exclude`, so the dx/subsecond pins
stay out of the root graph. It builds into its own `target/`.

```
hotpatch-spike/
├── Cargo.toml        workspace only: members app + runner, dev-profile opt-level override
├── app/              lib package `hotpatch-spike-app`: SpikeApp, HomePage, CounterCard, frust::app!
├── runner/           bin package `hotpatch-spike`: dioxus_devtools::connect -> frust_hotpatch::apply_patch (debug only), then __frust_main()
└── measure.sh        edit-to-frame timing (hot patch or restart)
```

## Findings

1. **The app is split into two packages because dx 0.7.10 cannot patch the template's
   one-package shape.** dx's thin rebuild replays rustc only for workspace *packages* other than
   the tip (the bin). It never recompiles the lib target of the bin's own package. With `[lib]` and a
   thin `main.rs` in one package, `dx --verbose` logs `replaying crates: []`: the patch builds and
   applies, but the changed code is not in it. When the lib is its own package, dx logs
   `replaying crates: ["hotpatch_spike_app"]` and the patch lands with the process kept alive (same
   PID). So `app` (lib) and `runner` (bin) split the template's single package. Everything else
   mirrors the template. Save->frame submitted is 0.47-0.5 s in steady state, and the first patch
   of a session takes up to 1.7-2.1 s (RESULTS.md).
2. **The `app` lib is `crate-type = ["rlib"]`, not the template's
   `["cdylib", "staticlib", "rlib"]`.** dx 0.7.10 fails a thin patch of a lib with the template's
   crate types, for two reasons, both verified here:
   - Its rustc wrapper files captured args under the *first* `--crate-type`, and `cdylib` goes
     under `.bin`. The patch then fails with
     `Missing captured rustc args for workspace crate 'hotpatch_spike_app.lib'`.
   - With `rlib` listed first, cargo still builds that crate-type set without the
     `-C extra-filename` hash (`libhotpatch_spike_app.rlib`). The patch then fails with
     `No rlib found for 'hotpatch_spike_app' ... extra-filename=None`.

   A desktop-only spike loses nothing with an rlib. A shipped integration would have to keep the
   mobile crate types out of the patched package.
3. **`frust run --watch` does not accept this workspace.** From `runner/`, it passes
   `--features frust/devtools,frust/perf-trace`, and cargo rejects that because the runner has no
   `frust` dependency: `the package 'hotpatch-spike' does not contain these features`. From `app/`,
   cargo reports `a bin target must be available for cargo run`. It also watches only `<cwd>/src`
   and `<cwd>/Cargo.toml`, so `app/` edits would go unseen from `runner/`. `measure.sh --restart`
   therefore kills and relaunches `cargo run -p hotpatch-spike` itself after each edit.
4. **The sentinels live in child components on purpose.** The desktop root closure that
   `frust::app!` generates calls the *root* component's `build` directly. Only `component(..)`
   children go through the hot seam (frust-core `call_build`), so an edit to `SpikeApp::build`
   would not show.

## Prerequisites

Install dx 0.7.10 into a scratch root, never `~/.cargo/bin`. Its jump-table wire format is the
one `crates/frust-hotpatch` speaks (ported from subsecond 0.7.10), and the runner's
`dioxus-devtools` is pinned `=0.7.10` to match:

```sh
cargo install dioxus-cli --version 0.7.10 --locked --root <scratch-dir>
export DX=<scratch-dir>/bin/dx      # `"$DX" --version` prints `dioxus 0.7.10 (...)`
```

`python3` is needed by `measure.sh`.

From the tip that added `PatchError::AnchorMismatch`, `crates/frust-hotpatch` refuses dx 0.7.10's
jump tables: dx fills both `aslr_reference` and `new_base_address` from `main`, not from
`__frust_hotpatch_anchor`, so the offsets they imply differ from the images' slides. The runner
prints `frust-hotpatch: refused AnchorMismatch` and `measure.sh` records the run as "refused
(anchor mismatch)", which is neither applied nor a silent no-op. Rows D-D3 of `RESULTS.md` were
measured against the runtime at 1c29ba1d, which rebased every key by that wrong constant without
noticing, and they reproduce only against that runtime. A patch builder that anchors on
`__frust_hotpatch_anchor` is needed before the hot rows run again at the tip.

## Running

Use the dev profile only. frust-hotpatch (like subsecond) reads its jump table only under
`cfg!(debug_assertions)`, and
the runner connects to dx's devserver only under `cfg(debug_assertions)`. A release build never
opens the devtools connection or applies a patch. The connection and `apply_patch` (which loads and
runs code the devserver sends) must stay debug-only in any integration built from this spike.

```sh
cd examples/hotpatch-spike
cargo run -p hotpatch-spike                 # plain launch: logs one `frust-hotpatch: frame`
cd runner && "$DX" serve --hot-patch --platform desktop --interactive false --verbose
```

Under `dx serve`, edit `app/src/home_page.rs` or `app/src/counter_card.rs` and save. The flag is
`--hot-patch`, and `--verbose` makes dx print `Patch rebuild: ...` and `replaying crates: [...]`.
The first `dx serve` does a cold fat build of frust and wgpu, which takes minutes.

### measure.sh

```sh
./measure.sh                                # --hotpatch --target home --runs 5
./measure.sh --target card --runs 10        # child component in a second module
./measure.sh --target helper                # adds a new private fn called from HomePage::build
./measure.sh --target state-type            # swaps HomeState for a tuple: State TYPE change (row D)
./measure.sh --target state-field           # adds `extra: u32` to struct HomeState: LAYOUT change (row D2)
STATE_FIELD_WRITE=1 ./measure.sh --target state-field   # ...and the patched build writes it
./measure.sh --restart                      # baseline: rebuild + relaunch per edit
./measure.sh --help
```

Each run rewrites the marked line(s) in place (`// SENTINEL-HOME`, `// SENTINEL-CARD`,
`// STATE-*`). The script uses python3 to write the same file in place, never `sed -i`: `sed -i`
renames a temp file, and dx patches off that temp-file event with stale content. The script stamps
the save time, then waits up to 60 s for the next `applied` and `frame` lines. It prints the
per-run deltas, the dx patch lines and the medians. A timeout or app crash is recorded as a run
result, not a script failure. Hot runs also report whether the app PID survived. `state-field`
runs report whether the patched `build` ran (it logs `frust-hotpatch-spike: state-field vN build
ran extra=<v>`) and the value it read. On exit (Ctrl-C included) the script kills the runner's
process group, restores every edited file and prints `git status` as proof. Every KILL is guarded:
the group only while it exists, and a PID scraped from the log only if it is in the runner's process
group. Logs go to a `mktemp` directory whose path is printed. So does the manual restore command,
for a script killed without its trap.

The probe lines prove a patch landed and a frame followed. They cannot show what the window drew.
Check visually that the text changed and that the counter kept its value.

The two State targets behave differently. A State *type* change (`state-type`, row D) is a silent
no-op: dx says it patched, but the old `build` keeps running. A field added to the named struct
(`state-field`, row D2) is NOT a no-op. The new `build` runs against the old, smaller state value
and reads and writes memory past it, which is undefined behaviour with no warning. Restart after any
State change.

## Results

Measurements are recorded in `RESULTS.md` (p1-03b, plus row D2 and the corrections from review
round r1-01), not here.

## Gates

From this directory:

```sh
cargo build && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
bash -n measure.sh
```
