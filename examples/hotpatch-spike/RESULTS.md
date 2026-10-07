# p1-03b results: dx 0.7.10 hot-patch matrix on the two-package spike app (macOS)

Card p1-03b (tsk_000001a115ed4814QoNMau27), plan fplan_000001a10e032a8eKL91PGxC, run 2026-10-07 on
`spike/hotpatch` @ f1ccf689 (p1-01 seam + p1-02b spike app). Rows A-G and I were measured with
`measure.sh` (the README's `--hot-patch --platform desktop --interactive false --verbose`
invocation). Row H is cited, not re-run.

## Verdict: GO for Phase 2

All three GO conditions hold:

1. **Row A patches the app lib package with State preserved.** dx logs
   `replaying crates: ["hotpatch_spike_app"]` on every run. The process is never relaunched: dx
   saw one devtools connection, and the PID was the same before and after. The counter was clicked
   to 2 before the runs and still read `count: 2` after five patches. At that point the window
   showed `hotpatch-sentinel: v5`.
2. **Median save->frame is 30% of the restart median, under the 50% bar.** Row A's median is
   475 ms (cold session) and 480 ms (warm session). Row F's median is 1570 ms, so the ratios are
   30.3% and 30.6%.
3. **Row G has no crash.** Ten consecutive patches landed in one process (median 493 ms). RSS rose
   by about 0.55 MB per patch and stayed flat afterwards.

Four limits come with the GO:

- **Row D:** a State type change is a *silent no-op*. dx reports a successful patch, but the old
  `build` keeps running.
- **Row E:** framework crates are never patched. dx does not even see the edit.
- **Row H:** the `frust create` one-package layout cannot be patched by dx at all (see the
  template implication below).
- **Row F is not `frust run --watch`.** The restart baseline is `measure.sh`'s own kill and
  `cargo run` loop, because `frust run --watch` rejects this two-package workspace (README.md,
  finding 3). That loop skips the CLI's own overhead, so the 30% ratio is conservative.

## Environment

| | |
|---|---|
| Machine | `Mac16,1`, `Apple M4` (`sysctl -n hw.model machdep.cpu.brand_string`) |
| OS | macOS 27.0.1 (26A434), logged-in GUI session; the window really opens |
| Rust | `rustc 1.98.1 (48a229cea 2026-09-01)`, `cargo 1.98.1 (797e8a9bc 2026-08-05)`, dev profile |
| dx | `dioxus 0.7.10 (57d6794)`, installed in a session scratch root (never `~/.cargo/bin`) |
| subsecond / dioxus-devtools | `=0.7.10` (frust-core `hotpatch` feature / runner) |
| Base | `spike/hotpatch` @ f1ccf689; `CARGO_TARGET_DIR` unset; spike `target/` cold at start |
| Window evidence | `screencapture -x -R<window rect>` before the first edit and after the last run |
| Clicks | AppleScript `tell application "System Events" to tell (first process whose unix id is <PID>) to click at {x, y}`. Accessibility was granted, and the click lands on the AccessKit `button Increment`. The first click after launch returns `missing value` and misses; the next ones land. |

## Matrix

Times are milliseconds from the save timestamp (unix ms, stamped by `measure.sh`) to the app's
`frust-hotpatch: applied` / `frust-hotpatch: frame` probe lines. The two lines land in the same or
the next millisecond, so save->applied and save->frame are almost identical in every hot row.
"dx thin build" is dx's own `Build completed in` figure for each patch. Logs were under the
session scratch directory as `hotpatch-spike.<id>/runner.log`. They are not committed, so the
relevant lines are quoted below.

| Row | What | Patched without relaunch? | State kept? | save->frame median (min-max), n | `replaying crates` | Verdict |
|---|---|---|---|---|---|---|
| A | home sentinel, `app/src/home_page.rs` | yes (1 PID, 1 devtools connection) | yes: count 2 -> 2 | **475** (469-2128), 5 cold session; **480** (471-533), 5 warm session | `["hotpatch_spike_app"]` | PASS |
| B | card sentinel, `app/src/counter_card.rs` | yes | yes: count 2 -> 2 | **479** (469-583), 5 | `["hotpatch_spike_app"]` | PASS |
| C | new private fn called from `HomePage::build` | yes | yes: count 2 -> 2 | **482** (475-1694), 5 | `["hotpatch_spike_app"]` | PASS |
| D | field added to HomePage's State (`u32` -> tuple) | patch "applies", but the old `build` keeps running | count 2 unchanged; the edit never shows | 623 (622-1282), 5, "applied" only | `["hotpatch_spike_app"]` | silent no-op |
| E | `PAD_X` 12 -> 48 in `crates/frust-widgets/src/button.rs` | no: dx logs nothing | n/a | n/a (no event) | none emitted | NOT patched (expected) |
| F | restart: kill + `cargo run -p hotpatch-spike` per edit | relaunch | no: count resets to 0 | **1570** (1436-1731), 5 | n/a | baseline |
| G | 10 consecutive home patches | yes, all 10 | yes: count 2 -> 2 | 493 (490-505), 10 | `["hotpatch_spike_app"]` | no crash; RSS +5.4 MB |
| H | one-package `frust create` shape | no: `replaying crates: []` (cited) | n/a | n/a | `[]` | NOT patched (dx 0.7.10 and git main) |
| I | cold `dx serve` fat build vs cold `cargo build` | n/a | n/a | dx 67.0 s build / 68.7 s to first frame vs cargo 62.4 s | n/a | dx +7% |

### A. Home sentinel (`measure.sh --target home --runs 5`)

There are two sessions.

- **Cold session.** This was the first `dx serve` after `cargo clean`, so it doubles as row I.
  The per-run save->frame values were 2128 / 477 / 469 / 470 / 475 ms (median 475; save->applied
  median 474). The first patch of the session is slow: dx's thin build took 1216 ms that time
  and 430-438 ms afterwards.
- **Warm session.** This used `PRE_RUN_PAUSE=20 POST_RUN_PAUSE=15`. The per-run values were
  533 / 480 / 471 / 476 / 481 ms (median 480; save->applied median 479). The dx thin builds took
  432-444 ms.

Pooled over both sessions, the ten runs have a median of 476.5 ms. dx's own
`Hot-patching: ... took` (from the patch build finishing to the patch being sent) was 204-216 ms
after the first patch.

```
DEBUG Patch rebuild: changed_crates=["hotpatch_spike_app"], modified_crates={"hotpatch_spike", "hotpatch_spike_app"}
DEBUG replaying crates: ["hotpatch_spike_app"]
INFO Hot-patching: app/src/home_page.rs took 211ms
[frust INFO] frust-hotpatch: applied t_unix_ms=1791374975100
[frust INFO] frust-hotpatch: frame t_unix_ms=1791374975100
```

State check: Increment was clicked three times during the pre-run pause. Two clicks landed, so the
before-capture shows `count: 2`, `hotpatch-sentinel: v0`. The after-capture (same PID 26308, still
alive) shows `count: 2`, `hotpatch-sentinel: v5`. RSS was 120.6 MB before the patches and
122.9 MB after.

### B. counter_card (`--target card --runs 5`)

Save->frame per run was 583 / 474 / 479 / 488 / 469 ms (median 479; save->applied median 478).
dx thin builds took 433-451 ms. dx logged `Hot-patching: app/src/counter_card.rs took 204-225ms`
with `replaying crates: ["hotpatch_spike_app"]`. The after-capture (same PID 27414) shows
`count: 2` and `card-sentinel: v5`. So a patch reaches a second `component(..)` child in another
module, and its parent's state survives.

### C. New private helper fn (`--target helper --runs 5`)

Each run replaces the sentinel line with `text(spike_helper_N())` and appends a new
`fn spike_helper_N()`. Save->frame per run was 1694 / 475 / 476 / 490 / 482 ms (median 482). The
dx thin builds took 445-497 ms, so run 1 spent about 1.2 s *before* dx started building (see
Surprises). The after-capture (same PID 28427) shows `hotpatch-helper: v5` and `count: 2`. A new
function, which is a new symbol, patches fine when it is called from a patched `build`.

### D. State field added (`--target state --runs 5`)

Run N changes `type State = u32` to an (N+1)-tuple of `u32`, the init, the read and the increment.
It also bumps the sentinel. What happens:

- dx builds and applies a thin patch every run (`replaying crates: ["hotpatch_spike_app"]`, thin
  builds 580-810 ms, `Hot-patching: app/src/home_page.rs took 214-225ms`).
- The app logs `applied` and `frame` (save->frame 1282 / 624 / 622 / 622 / 623 ms, median 623).
- The process stays alive (PID 29560) through all five runs.
- **But the window never changes.** The after-capture still shows `hotpatch-sentinel: v0` and
  `count: 2`. Not even the sentinel bump in the same edit is visible.

There was no crash, no garbage, no timeout, and no fall-back to a full rebuild, and dx gave no
warning. The patch is silently not taken for that component.

Likely mechanism, from the symbol tables: subsecond's jump table maps symbols by name. The seam's
call site is the monomorphised `HotFn<(&HomePage, &mut u32), <Component>::build>::call`. The running
binary has `...HotFn...HomePageQmE...5buildE4call`. The last patch dylib has only the
`...HomePageQTmmmmmmE...` instance, for `&mut (u32, u32, u32, u32, u32, u32)`. The old instance has
no counterpart in the patch, so the old call path keeps its original target and the new `build` is
unreachable. This is safer than the UB or crash the plan's risk register predicted, but it is
silent: the developer sees "Hot-patching ... took 220ms" and an unchanged screen. A State change
needs a restart, and dx will not tell you so.

### E. Framework crate edit (frust-widgets)

With `dx serve --hot-patch` running (PID 30853), `const PAD_X: f64 = 12.0;` in
`crates/frust-widgets/src/button.rs` was changed in place to `48.0`, a visibly wider button.

- **dx logged nothing at all for 45 s.** There was no file event, no `Patch rebuild`, no
  `replaying crates` line and no full rebuild. The window was unchanged.
- dx watches only the workspace members. frust is a path dependency outside the spike workspace.
- **Follow-up run.** In a second session the framework edit was left in place, then a home patch
  was triggered. It landed (save->frame 629 ms, `replaying crates: ["hotpatch_spike_app"]`,
  `hotpatch-sentinel: v1` on screen), but the button kept its 12 px padding. The replay links
  against the stale frust-widgets rlib, so the running app silently mixes new app code with old
  framework code.

`button.rs` was restored in place after each session. As expected, framework edits are
restart-only.

### F. Restart baseline (`--restart --target home --runs 5`)

Each edit kills the app and relaunches `cargo run -p hotpatch-spike`, because `frust run --watch`
rejects this workspace (README.md, finding 3). Save->frame per run was
1656 / 1535 / 1570 / 1731 / 1436 ms (median **1570**). cargo's incremental rebuild of the two
spike packages is `Finished ... in 0.59s` each time. The remaining ~1 s is process start, window
creation, the wgpu/engine init and the first frame. The after-capture shows `hotpatch-sentinel: v5`
with `count: 2` lost: a fresh process shows `count: 0`.

The app is tiny and the M4 links fast, so this baseline is about as good as a restart gets. A
real app's incremental rebuild grows with its own crate, while the thin patch pays a similar
per-crate replay.

### G. Robustness (`--target home --runs 10`)

Ten consecutive patches in one process (PID 37777):

- Save->frame per run was 505 / 495 / 492 / 490 / 490 / 492 / 500 / 498 / 496 / 491 ms
  (median 493). There was no crash, timeout or slowdown trend, and dx thin builds stayed at
  460-470 ms.
- The after-capture shows `hotpatch-sentinel: v10` and `count: 2`.
- RSS (`ps -o rss=`, sampled once a second) settled at ~121.3 MB after the clicks. It rose in
  steps during the patches, reached 126.8 MB after run 10 and stayed flat at 126.75 MB through the
  15 s pause. That is +5.4 MB over 10 patches, about 0.55 MB per patch.
- Each patch is a 1.5 MB dylib (`libhotpatch-spike-patch-<ms>.dylib`, 1,534,184 bytes) that is
  loaded and never unloaded.

The memory growth is linear, with no leak beyond the loaded patch images. A long session of
hundreds of patches would grow by hundreds of MB.

### H. One-package `frust create` shape: pre-answered (cited, not re-run)

Two artifacts already answer this row:

- **rsa_000001a115e137983TvtYa1Y** (smoke matrix, runs 3 and 6): with dx 0.7.10, a single
  package with `[lib]` holding the code and a thin `[[bin]]` is not patched. The patch builds
  and applies, but dx logs `replaying crates: []` and the hot symbols are absent from it.
- **rsa_000001a115ecee80V1weef8K** (addendum, run 7): dx from DioxusLabs/dioxus git main f951996
  (`dioxus 0.8.0-alpha.1`), with subsecond and dioxus-devtools pinned to the same rev, gives the
  same `replaying crates: []` line.

The plan's fallback (risk #1, retry on dx main) does not rescue the one-package layout. The
two-package layout in this directory is what rows A-G measured. dx main was not installed for this
card.

### I. Cold builds: first `dx serve` (fat) vs `cargo build`

Both runs started from a clean spike `target/`. dx builds into
`target/aarch64-apple-darwin/desktop-dev/` and `target/dx/`, not cargo's `target/debug/`, so each
cold build compiles everything.

| Build | Wall time |
|---|---|
| `cargo build` (workspace, dev profile), clean `target/` | **62.4 s** (cargo: `Finished ... in 1m 02s`) |
| first `dx serve --hot-patch`, after `cargo clean` | **67.0 s** build (`Build completed in 67027ms`: Compiling 65.7 s, Fat Linking 0.95 s); first frame **68.7 s** after launch |
| row F's first `cargo run` (`target/debug` cold again, since dx never filled it) | 64.1 s to first frame |
| later warm `dx serve` starts (fat relink only) | 1.6-3.2 s build, 2.4-4.1 s to first frame |

The fat build costs about 7% more than a plain cold build. Because the two use separate output
directories, a developer who alternates `dx serve` and `cargo build` pays both cold builds and
keeps two copies on disk: `cargo clean` removed 2.1 GiB after the cargo build alone.

## measure.sh fixes made for this card

Both fixes are additive. Defaults are unchanged and `bash -n measure.sh` passes.

1. **`PRE_RUN_PAUSE=<s>`** (default 0) sleeps after the first frame, before the first edit. Rows
   A-D need the counter clicked before the runs, and the script used to go straight from the
   first frame to edit 1.
2. **`POST_RUN_PAUSE=<s>`** (default 0) sleeps after the last run, before the cleanup trap kills
   the app. Without it, the patched window was gone before it could be captured, and the
   state-preservation question could not be answered from the screen. The first row-A attempt
   lost its after-capture this way.

`--help` prints the extended header (sed range 2-30).

## Surprises and facts that contradict the plan's research

- **A State change is a silent no-op, not UB or a crash** (row D). The plan's risk register
  expected "undefined behaviour or a crash". In practice the whole edited `build` is dropped,
  including unrelated edits in the same patch, while dx and the probe both report success. Any
  "automatic fallback to restart" follow-up must *detect* this: no error is raised to hook. One
  candidate is comparing the patch's `HotFn<..>::call` instances against the binary's.
- **The first patch of a session can be 1-2 s slower.** In the cold session, run 1 took 2128 ms,
  with a 1216 ms thin build. In row C, run 1 took 1694 ms although dx's build took only 497 ms, so
  ~1.2 s passed before dx started. In the warm sessions of rows A and B, run 1 took 533 / 583 ms.
  Steady state is 470-500 ms.
- **dx ignores path-dependency edits entirely** (row E). It does not merely refuse to patch them:
  there is no log line at all. Later app patches then mix new app code with stale framework code.
- **Hot-patch steady state is dominated by dx's fixed costs, not rustc.** A typical thin build of
  ~440 ms breaks down into workspace hotpatch replay ~230 ms (370-590 ms in row D), Compiling
  ~105 ms and Patch: Link ~75 ms. Another ~210 ms goes to `Hot-patching ... took`. The app-side apply-to-frame is ≤1 ms.
- **Memory grows by ~0.55 MB per patch** (row G) and is never returned within a session.
- **The restart loop is faster than the research assumed** for this tiny app (1.57 s, 0.59 s of
  it rebuild). The hot patch still wins by 3.3x, but the absolute gain here is ~1.1 s per edit
  plus kept state.

## Template implication

dx, both 0.7.10 and git main f951996, replays rustc only for workspace packages other than the tip
(the bin package). It never recompiles the lib target of the bin's own package (row H), and it
cannot thin-patch a lib built with the template's `["cdylib", "staticlib", "rlib"]` crate types
(README.md, finding 2). So the `frust create` one-package layout cannot use dx as-is. A
hot-reload-capable frust has two options:

- **Two packages.** The template becomes an app lib package plus a thin runner bin package, as in
  this spike. The patched package must stay `rlib`-only on desktop, so the mobile
  `cdylib`/`staticlib` outputs move to the runner or a separate packaging step.
- **A frust-owned builder.** The builder replays the bin package's own lib target itself and
  handles the multi-crate-type capture that dx 0.7.10 trips over (`.bin` vs `.lib` arg capture,
  missing `-C extra-filename`). The one-package template then stays unchanged.

p2-03's PORT.md should size the second option against the first. Rows D and E add two more
requirements for that builder: detecting signature-changing edits (row D's silent no-op) and
covering frust path dependencies (row E), or explicitly telling the developer to restart.
