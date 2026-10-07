# p1-03b results: dx 0.7.10 hot-patch matrix on the two-package spike app (macOS)

Card p1-03b (tsk_000001a115ed4814QoNMau27), plan fplan_000001a10e032a8eKL91PGxC, run 2026-10-07 on
`spike/hotpatch` @ f1ccf689 (p1-01 seam + p1-02b spike app). Review round 1 (r1-01,
tsk_000001a116641c6dbbIbPK05) added row D2 and the corrections below on `spike/hotpatch` @ 43fbc864.

**SHA note.** Rows A-I were measured at f1ccf689; the review read 43fbc864. The only commit between
them (27e16359) changes `RESULTS.md` and `measure.sh` (the `PRE_RUN_PAUSE` / `POST_RUN_PAUSE`
additions), so the app, the runner and the frust seam the rows measured are the reviewed ones. Row
D2 and the r1-01 home re-check ran on 43fbc864 plus r1-01's own commit (HomePage's State became the
named `HomeState` struct, see row D2).

**Where each row comes from.** Rows A, B, C, D, F and G are `measure.sh` runs (the README's
`--hot-patch --platform desktop --interactive false --verbose` invocation; F is `--restart`). Row
D2 is `measure.sh --target state-field` plus a scratch harness for the guard-malloc leg (described
there). Row E was manual: a hand edit of a frust source file during a `dx serve` session, read off
dx's log and the window. Row I comes from dx's and cargo's own build-time output on cold
`target/` directories, not from `measure.sh`. Row H is cited, not re-run.

## Verdict: GO for Phase 2

All three GO conditions hold:

1. **Row A patches the app lib package with State preserved.** dx logs
   `replaying crates: ["hotpatch_spike_app"]` on every run. The process is never relaunched: dx
   saw one devtools connection, and the PID was the same before and after. The counter was clicked
   to 2 before the runs and still read `count: 2` after five patches. At that point the window
   showed `hotpatch-sentinel: v5`.
2. **Median save->frame submitted is 30% of the restart median, under the 50% bar.** Row A's median is
   475 ms (cold session) and 480 ms (warm session). Row F's median is 1570 ms, so the ratios are
   30.3% and 30.6%.
3. **Row G has no crash.** Ten consecutive patches landed in one process (median 493 ms). RSS rose
   by about 0.55 MB per patch and stayed flat afterwards.

Row D2 does not touch these three criteria: they are about patching, speed and crashes under
layout-preserving edits, and all three still hold as measured. The GO stands. D2 changes the
*limits*, which are now five:

- **Row D:** a State edit that changes the State *type identity* (here `u32` -> a tuple) is a
  *silent no-op*. dx reports a successful patch, but the old `build` keeps running.
- **Row D2:** a field added to a *named* State struct is NOT a no-op. The patch is taken, and the
  new `build` runs against the old, smaller state value: it reads and writes memory past it (most
  likely the component's `disposed` flag). That is undefined behaviour with no warning. It survived 6/6
  plain runs and crashed in run 1 of both guard-malloc sessions (one confound is open, see D2). A
  State *layout* change must force a restart. Telling the two cases apart needs a layout
  fingerprint, not a symbol check (see Surprises).
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
`frust-hotpatch: applied` / `frust-hotpatch: frame` probe lines. **"frame" means frame submitted:**
the probe is stamped on the UI thread when the finished frame is handed off (desktop shell
`app_handler.rs`, the probe comment says "not the GPU present"). It excludes GPU execution, present
and compositor latency (on a 60 Hz panel, up to one more refresh interval). Every latency below is
save->frame submitted. The applied and frame lines land in the same or the next millisecond, so
save->applied and save->frame submitted are almost identical in every hot row. "dx thin build" is
dx's own `Build completed in` figure for each patch. Logs were under the session scratch directory
as `hotpatch-spike.<id>/runner.log`. They are not committed, so the relevant lines are quoted below.

| Row | What | Patched without relaunch? | State kept? | save->frame submitted median (min-max), n | `replaying crates` | Verdict |
|---|---|---|---|---|---|---|
| A | home sentinel, `app/src/home_page.rs` | yes (1 PID, 1 devtools connection) | yes: count 2 -> 2 | **475** (469-2128), 5 cold session; **480** (471-533), 5 warm session | `["hotpatch_spike_app"]` | PASS |
| B | card sentinel, `app/src/counter_card.rs` | yes | yes: count 2 -> 2 | **479** (469-583), 5 | `["hotpatch_spike_app"]` | PASS |
| C | new private fn called from `HomePage::build` | yes | yes: count 2 -> 2 | **482** (475-1694), 5 | `["hotpatch_spike_app"]` | PASS |
| D | State type identity changed (`type State = u32` -> (N+1)-tuple) | patch "applies", but the old `build` keeps running | count 2 unchanged; the edit never shows | 623 (622-1282), 5, "applied" only | `["hotpatch_spike_app"]` | silent no-op |
| D2 | field `extra: u32` added to the named `struct HomeState` | yes: the NEW `build` runs (its log line appears), same PID | the new code reads/writes past the old 4-byte value | 481 (473-1227), 3 read; 485 (477-1008), 3 write | `["hotpatch_spike_app"]` | patched over the old layout (UB); survived 6/6 plain runs; crashed in run 1 of 2/2 guard-malloc sessions |
| E | `PAD_X` 12 -> 48 in `crates/frust-widgets/src/button.rs` (manual) | no: dx logs nothing | n/a | n/a (no event) | none emitted | NOT patched (expected) |
| F | restart: kill + `cargo run -p hotpatch-spike` per edit | relaunch | no: count resets to 0 | **1570** (1436-1731), 5 | n/a | baseline |
| G | 10 consecutive home patches | yes, all 10 | yes: count 2 -> 2 | 493 (490-505), 10 | `["hotpatch_spike_app"]` | no crash; RSS +5.4 MB |
| H | one-package `frust create` shape | no: `replaying crates: []` (cited) | n/a | n/a | `[]` | NOT patched (dx 0.7.10 and git main) |
| I | cold `dx serve` fat build vs cold `cargo build` (tool output, not `measure.sh`) | n/a | n/a | dx 67.0 s build / 68.7 s to first frame vs cargo 62.4 s | n/a | dx +7% |

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
`Hot-patching: ... took` was 204-216 ms after the first patch. That figure is *not* a separate
phase after the build: dx prints it at the same timestamp as `Build completed`, measuring from the
start of the patch's cargo/rustc step to the jump table being ready, so it is the tail of the thin
build (see the breakdown under Surprises).

**r1-01 re-check after the HomeState change** (`--target home --runs 2`, a fresh cold `dx serve`,
first frame 68.9 s after launch): both patches landed in the same PID (53596) with
`replaying crates: ["hotpatch_spike_app"]`. Save->frame submitted was 548 / 483 ms (median 515),
and the thin builds took 443 / 442 ms. The counter was clicked to 2 before the runs. The
after-capture shows `hotpatch-sentinel: v2` and `count: 2`. So a named State struct changes nothing
for layout-preserving edits.

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

### D. State type identity changed (`--target state --runs 5`, now `--target state-type`)

Run N changes `type State = u32` to an (N+1)-tuple of `u32`, the init, the read and the increment.
It also bumps the sentinel. (The target was called `state` then. r1-01 renamed it `state-type`; it
now swaps the named `HomeState` for the tuple, which is the same kind of change.) What happens:

- dx builds and applies a thin patch every run (`replaying crates: ["hotpatch_spike_app"]`, thin
  builds 580-810 ms, `Hot-patching: app/src/home_page.rs took 214-225ms`).
- The app logs `applied` and `frame` (save->frame 1282 / 624 / 622 / 622 / 623 ms, median 623).
- The process stays alive (PID 29560) through all five runs.
- **But the window never changes.** The after-capture still shows `hotpatch-sentinel: v0` and
  `count: 2`. Not even the sentinel bump in the same edit is visible.

There was no crash, no garbage, no timeout, and no fall-back to a full rebuild, and dx gave no
warning. The patch is silently not taken for that component.

Mechanism. `HotFn::try_call` (subsecond-0.7.10 `src/lib.rs:430`) looks up the address of
`<F as HotFunction<A, M>>::call_it` in the jump table. dx builds that table by matching symbol
*names* between the base binary and the patch. Here `F` is the fn item
`<HomePage as Component>::build`, `A` is `(&HomePage, &mut State)` and `M` is `Fn2Marker`
(impl at `:889-896`). frust-core's `call_build` instantiates it. The running binary has only the
instance for `&mut u32`, and the last patch dylib has only the one for
`&mut (u32, u32, u32, u32, u32, u32)`. They are different symbols, so there is no table entry, the
old call path keeps its original target, and the new `build` is unreachable. This is safer than the
UB or crash the plan's risk register predicted, but it is silent: the developer sees "Hot-patching
... took 220ms" and an unchanged screen. A State type change needs a restart, and dx will not tell
you so.

**This row covers type-identity changes only.** A field added to a named State struct keeps the
type's path, so the symbol matches and the patch IS taken (row D2). The p1-03b wording "a State
change is a silent no-op, not UB" was too broad and is withdrawn.

**Return-type-changing `build` edits are untested** (for example, adding a `.child(..)` changes the
concrete type behind `impl View<State>`). The review asked for them to be recorded as the same
silent no-op. The D2 crash reports suggest otherwise, so they are recorded here as *untested, and
probably not a no-op*. The symbols are v0-mangled (rustc 1.98.1's default; the reports show
`_R...` names). The patched `call_it` symbol in those reports is
`<<HomePage as Component>::build as HotFunction<(&HomePage, &mut HomeState), Fn2Marker>>::call_it`
and carries no return type, because `R` is fixed by the fn-item type `F`. If so, a return-type
change would *match*, and the old caller would read a return value of the new type through the old
type: a D2-class layout hazard, not a no-op. This needs its own row before either claim is relied
on.

### D2. Field added to a named State struct (`--target state-field --runs 3`)

r1-01 made HomePage's State a named struct, `pub struct HomeState { pub count: u32 }` (the only
committed app change). Run N adds `pub extra: u32` (initialised to 4242 in `init`). The patched
`build` reads it into the sentinel (`hotpatch-sentinel: vN extra=<v>`) and logs
`frust-hotpatch-spike: state-field vN build ran extra=<v>`. Only the patched code contains that log
line, so its presence proves the new `build` ran and shows what it read. `init` does not re-run on a
patch (frust-core seam), so the live value is still the old 4-byte `HomeState { count }`, stored
inline in `ComponentWidget::state` (`crates/frust-core/src/component.rs`, read through
`&mut element.state` at `:222`).

**Read session** (`PRE_RUN_PAUSE=20 POST_RUN_PAUSE=15`, PID 54590, counter clicked to 2 before the
runs, before-capture `count: 2`, `hotpatch-sentinel: v0`):

| Run | save->applied / frame submitted | Patched build ran? | `extra` read | PID |
|---|---|---|---|---|
| 1 | 1226 / 1227 ms (thin build 1124 ms) | yes | 0 | 54590 alive |
| 2 | 481 / 481 ms | yes | 0 | 54590 alive |
| 3 | 473 / 473 ms | yes | 0 | 54590 alive |

**Write session** (`STATE_FIELD_WRITE=1`, so the patched `build` does `state.extra += 1` before
reading it; PID 56161):

| Run | save->applied / frame submitted | Patched build ran? | `extra` read | PID |
|---|---|---|---|---|
| 1 | 1008 / 1008 ms | yes | 1 | 56161 alive |
| 2 | 477 / 478 ms | yes | 2 | 56161 alive |
| 3 | 485 / 485 ms | yes | 3 | 56161 alive |

dx logged `replaying crates: ["hotpatch_spike_app"]` and `Hot-patching: app/src/home_page.rs took
203-279ms` on every run. There was no panic, abort or signal in the log.

What the numbers mean:

- **The patch is taken.** Unlike row D, the seam's `call_it` symbol is the same before and after
  (the State type's path did not change), so the jump table redirects to the new `build`.
- **The new code addresses memory past the old value.** A real `extra` would read 4242 (fresh
  `init`). The read session sees 0, and the write session's value survives from one patch to the
  next (1, 2, 3). So reads and writes land in 4 bytes after the old `HomeState` that are not a
  field of it. `-Zprint-type-sizes` (nightly 1.100, same sources, scratch target dir) gives
  `ComponentWidget<HomePage>`'s layout: 224 bytes, `.state` 4 bytes, then `.disposed` 1 byte and
  3 bytes of end padding. So `extra` aliases the `disposed` flag. The write session stored 1, 2
  and 3 into that `bool`: 1 makes teardown and `Drop` skip disposing the component's owner, and 2
  or 3 is an invalid `bool`. Field order is the compiler's choice and was read from nightly, not
  1.98.1, so treat the exact victim as likely, not proven. That the bytes are *not* the new field
  is proven by the 0 and 1-2-3 values.
- **No visual evidence for this row.** The screen locked about 3 s before read run 1
  (`CGSSessionScreenIsLocked`, lock time 1791377672). The window was then invisible to
  `screencapture` and the accessibility API, and clicks no longer reached it. The before-capture
  exists; the after-captures do not. The log line from the patched `build` is the evidence instead.

**Guard malloc.** dx passes its own environment to the app, so `DYLD_INSERT_LIBRARIES` on
`dx serve` would also put cargo, rustc and the linker under guard malloc. Instead a scratch harness
started `dx serve` normally and waited for its app's first frame. It then sent SIGTERM to that app
(dx logs `exited with error: signal: 15` and keeps serving). It relaunched the same
`target/dx/.../HotpatchSpike.app/Contents/MacOS/hotpatch-spike` directly with the variables dx sets
(`DIOXUS_DEVSERVER_IP=127.0.0.1`, `DIOXUS_DEVSERVER_PORT=8080`, `DIOXUS_BUILD_ID=0`,
`DIOXUS_SESSION_CACHE_DIR=<dx's>`, `RUST_BACKTRACE=1`) plus
`DYLD_INSERT_LIBRARIES=/usr/lib/libgmalloc.dylib`, and applied the same edits. dx registers the
latest connection's PID and ASLR reference, so patches went to the relaunched process. The app
printed `GuardMalloc[hotpatch-spike-<pid>]: Allocations will be placed on 16 byte boundaries.`

| Session | Edit | Result |
|---|---|---|
| guard malloc #1 | state-field (read) | run 1: `applied` logged, then **SIGBUS** (exit 138, `EXC_BAD_ACCESS KERN_PROTECTION_FAILURE at 0x6af062714`) before the patched build's log line. The faulting stack is patched `call_it` -> patched `HomePage::build` (`call_mut` +36) -> a PC in the base binary that symbolicates to `wgpu_core::storage::Storage::remove` +52 (a wild jump). |
| guard malloc #2 | state-field (read) | run 1: **SIGSEGV** (exit 139, `KERN_INVALID_ADDRESS at 0x101d05450`) in the patched `HomePage::build` +240 -> `AtomicUsize::load`. That is most likely the `log::info!` max-level check reading `log`'s static. |
| control: direct launch, no guard malloc | state-field (read) | 3/3 `build ran extra=0`, alive (as under dx): the relaunch harness itself is sound |
| control: guard malloc | home sentinel (layout-neutral) | 3/3 applied + frame, alive: guard malloc alone does not break patching |
| control: guard malloc | `log::info!` added, no field | **could not run**: by then the locked session no longer created windows (the app logged `logger initialized` and nothing more, even without dx) |

Guard malloc did not trap *on* the out-of-bounds field access. The access lies inside the 224-byte
`ComponentWidget` allocation, and guard malloc only guards allocation ends. Both crashes are
instead in code the D2 edit added, with the patched `build` on the stack. The edit adds a field
*and* the crate's first reference to `log` (the instrumentation line). The missing control would
separate those two. Until it runs, the guard-malloc crashes cannot be attributed to the layout
change alone. They do show that a D2 edit is not reliably survivable.

Verdict for D2: a field added to a named State struct is hot-patched *over the old layout*. The new
`build` reads and writes 4 bytes past the live value with no warning from dx, subsecond or frust.
That is memory-unsafe whether or not a given run crashes. Row D2 makes "State layout changed ->
restart" a hard requirement for any Phase 2 builder. Because the symbol matches, a symbol or
instance comparison cannot detect this case.

### E. Framework crate edit (frust-widgets, manual)

Not a `measure.sh` run: the edit was made by hand during a `dx serve` session and judged from dx's
log and the window. With `dx serve --hot-patch` running (PID 30853), `const PAD_X: f64 = 12.0;` in
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

Not a `measure.sh` run: the figures are dx's `Build completed in` line, cargo's `Finished ... in`
line and the first `frust-hotpatch: frame` stamp. Both runs started from a clean spike `target/`. dx builds into
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

`--help` prints the extended header.

r1-01 changed `measure.sh` again:

- Target `state` was renamed `state-type`. A new target `state-field` (row D2) reports, per run,
  whether the patched build ran, the `extra` it read, and whether the app PID survived.
  `STATE_FIELD_WRITE=1` makes the patched build also write the field.
- Every hot run now reports the app PID's survival and any panic/abort/signal lines in the log.
- Kill guards: the final process-group KILL runs only while `kill -0 -- -$RUNNER_PID` says the
  group still exists. `APP_PID` is cleared when the runner stops. The SIGKILL fallback runs only if
  `ps -o pgid=` puts the scraped PID in the runner's process group (dx's app is: pgid = dx's PID).
- The manual restore command, with the pristine copies' paths, is printed up front.
- `--help` prints the whole comment header.

## Surprises and facts that contradict the plan's research

- **State edits split into two cases, and neither raises an error.** A State *type identity*
  change (row D) is a silent no-op: the whole edited `build` is dropped, including unrelated edits
  in the same patch, while dx and the probe both report success. A State *layout* change that
  keeps the type's path (row D2: a field added to a named struct) is the opposite. The patch is
  taken, and the new code reads and writes past the old value. The plan's risk register predicted
  "undefined behaviour or a crash", and that holds for D2. An "automatic fallback to restart"
  follow-up must *detect* the change *before* applying the patch. Comparing `call_it` /
  `HotFn::call` instances or symbol names cannot do it: it catches D and misses D2 by
  construction, because D2's symbols are identical. Detection needs a **layout fingerprint** of each
  component's `State`: size, alignment and the type's structure (field names, offsets and types,
  recursively), recorded at fat-build time and compared against the patch. Any mismatch means
  restart.
- **The first patch of a session can be 1-2 s slower.** In the cold session, run 1 took 2128 ms,
  with a 1216 ms thin build. In row C, run 1 took 1694 ms although dx's build took only 497 ms, so
  ~1.2 s passed before dx started. In row D2, run 1 took 1227 ms (thin build 1124 ms) and 1008 ms.
  In the warm sessions of rows A and B, run 1 took 533 / 583 ms. Steady state is 470-500 ms. The
  honest summary is ~0.47-0.5 s steady state and up to 1.7-2.1 s for the first patch of a session.
- **dx ignores path-dependency edits entirely** (row E). It does not merely refuse to patch them:
  there is no log line at all. Later app patches then mix new app code with stale framework code.
- **Hot-patch steady state is dominated by dx's fixed costs, not rustc.** A steady-state
  save->applied of ~480 ms splits into three sequential phases, read off dx's timestamps (dx's
  relative clock is pinned to unix time by the app's `applied` line, which dx echoes):
  - ~10 ms from the save to dx's `Patch rebuild` line (file event);
  - ~440 ms of dx thin build (`Build completed in`): the workspace hotpatch replay takes ~230 ms
    (370-590 ms in row D), then Compiling ~105 ms, Patch: Link ~75 ms and the jump table;
  - ~20-30 ms from `Build completed` to the app's `applied` line (websocket send, `dlopen`, jump
    table install, event-loop hop).

  The app's applied->frame submitted step is ≤1 ms. For example, row D2 run 2: save 26.44 s,
  `Patch rebuild` 26.45 s, `Build completed in 441ms` 26.89 s, `applied` 26.92 s, so
  10 + 441 + 30 = 481 ms. dx's `Hot-patching ... took ~210ms` is *not* a fourth phase. It is printed
  at the `Build completed` timestamp and measures from the start of the patch's cargo/rustc step to
  the jump table being ready, so it is the last ~210 ms of the 440 ms build (Compiling + Link + jump
  table). The p1-03b text added it on top of the build, which double-counted ~200 ms.
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

p2-03's PORT.md should size the second option against the first. Rows D, D2 and E add
requirements for that builder:

- **State layout gate (row D2, mandatory).** Before applying a patch, compare a layout fingerprint
  of every component `State` reachable from a patched `build` (size, alignment, type structure)
  against the running binary's. Any difference means restart, never apply. A symbol or instance
  comparison is not enough, because a field added to a named struct keeps the symbol.
- **Silent no-ops (row D)** should be surfaced: State type-identity changes, and possibly
  return-type changes (untested; they may instead be D2-like, see row D), should report "restart
  needed" instead of "patched".
- **frust path dependencies (row E)** should be covered, or the builder should tell the developer
  to restart.

The p1-01 seam also must stay debug-only. The spike's runner now connects to the devserver only
under `cfg(debug_assertions)`, the same gate subsecond's jump table uses.
