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
*limits*, which are now seven:

- **Row D:** a State edit that changes the State *type identity* (here `u32` -> a tuple) is a
  *silent no-op*. dx reports a successful patch, but the old `build` keeps running.
- **Row D2:** a field added to a *named* State struct is NOT a no-op. The patch is taken, and the
  new `build` runs against the old, smaller state value: it reads and writes memory past it (most
  likely the component's `disposed` flag). That is undefined behaviour with no warning. It survived 6/6
  plain runs and crashed in run 1 of both guard-malloc sessions (one confound is open, see D2). A
  State *layout* change must force a restart. Telling the two cases apart needs a layout
  fingerprint, not a symbol check (see Surprises).
- **Boundary layout, not just State (follows from D2 by construction; not measured):** the same
  hazard applies to every type whose values cross the old/new code boundary at the seam — the
  component struct itself (props, passed as `&self.component`), its State, and `build`'s
  concrete return type, recursively. A State-only fingerprint is necessary but not sufficient
  (see the Phase 2 requirements).
- **Row D3 (H0-00, measured later): a return-type-changing `build` edit is NOT a no-op.** Wrapping
  the root `column()` in `stack()` is patched, and the app took SIGSEGV on the first rebuild after the
  patch in 3/3 plain dx sessions (details in D3). The same wrap in another `column()`, which keeps the
  type, patched cleanly 3/3.
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

**Measurement baseline (2026-10-08).** Every row below was measured with the `frust-hotpatch`
runtime at 1c29ba1d and dx 0.7.10, whose jump tables are anchored on `main`. That runtime rebased
by the offset the table implied and never compared it with the images' slides, so the anchor
mismatch went unseen. The tip refuses such tables with `PatchError::AnchorMismatch` (the runner
prints `frust-hotpatch: refused AnchorMismatch`), so these rows reproduce only against the runtime
at 1c29ba1d, not at the tip.

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
| D2 | field `extra: u32` added to the named `struct HomeState` | yes: the NEW `build` runs (its log line appears), same PID | the new code reads/writes past the old 4-byte value | 481 (473-1227), 3 read; 485 (478-1008), 3 write | `["hotpatch_spike_app"]` | patched over the old layout (UB); survived 6/6 plain runs; crashed in run 1 of 2/2 guard-malloc sessions (confound open) |
| D3 | `HomePage::build`'s root `column()` wrapped in `stack().child(..)` (`FlexView` -> `StackView` behind `impl View<State>`) | yes: the patch is taken, then the app dies | n/a (app gone) | n/a: SIGSEGV on the first rebuild; save->applied 1723 / 1260 / 1052, n=3 sessions | `["hotpatch_spike_app"]` | crash 3/3 (not a no-op); guard malloc: panic 1/1 (wild call) then SIGSEGV. Control, wrap in `column()` (type kept): 3/3 patched, 583 median |
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
now swaps the named `HomeState` for the tuple, which is the same kind of change. Its outcome was
not re-measured after the HomeState change; it follows from the mechanism below.) What happens:

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

**Return-type-changing `build` edits were untested at this point; row D3 below measures them** (for example, wrapping the root `column()` in another
container changes the concrete type behind `impl View<State>`; `.child(..)`, `.flex(..)` and
`.when(..)` on a `FlexView` return `Self` and do not). The review asked for them to be recorded as the same
silent no-op. The D2 crash reports suggest otherwise, so they are recorded here as *untested, and
probably not a no-op*. The symbols are v0-mangled (rustc 1.98.1's default; the reports show
`_R...` names). The patched `call_it` symbol in those reports is
`<<HomePage as Component>::build as HotFunction<(&HomePage, &mut HomeState), Fn2Marker>>::call_it`
and carries no return type, because `R` is fixed by the fn-item type `F`. If so, a return-type
change would *match*, and the old caller would read a return value of the new type through the old
type: a D2-class layout hazard, not a no-op. D3 below is that row: it crashed.

### D2. Field added to a named State struct (`--target state-field --runs 3`)

r1-01 made HomePage's State a named struct, `pub struct HomeState { pub count: u32 }` (the only
committed app change). Run N adds `pub extra: u32` (initialised to 4242 in `init`). The patched
`build` reads it into the sentinel (`hotpatch-sentinel: vN extra=<v>`) and logs
`frust-hotpatch-spike: state-field vN build ran extra=<v>`. Only the patched code contains that log
line, so its presence proves the new `build` ran and shows what it read. `init` does not re-run on a
patch (frust-core seam), so the live value is still the old 4-byte `HomeState { count }`, stored
inline in `ComponentWidget::state` (`crates/frust-core/src/component.rs`, read through
`&mut element.state` at `:223`).

**Read session** (`PRE_RUN_PAUSE=20 POST_RUN_PAUSE=15`, PID 54590, counter clicked to 2 before the
runs, before-capture `count: 2`, `hotpatch-sentinel: v0`):

| Run | save->applied / frame submitted | Patched build ran? | `extra` read | PID |
|---|---|---|---|---|
| 1 | 1226 / 1227 ms (thin build 1124 ms) | yes | 0 | 54590 alive |
| 2 | 481 / 481 ms | yes | 0 | 54590 alive |
| 3 | 473 / 473 ms | yes | 0 | 54590 alive |

**Write session** (`STATE_FIELD_WRITE=1`, so the patched `build` does `state.extra = state.extra.wrapping_add(1)` before
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
change alone. Whether a D2 edit is survivable is therefore open; the verdict below rests on the
proven out-of-bounds reads and writes (0, then 1-2-3), not on these crashes.

Verdict for D2: a field added to a named State struct is hot-patched *over the old layout*. The new
`build` reads and writes 4 bytes past the live value with no warning from dx, subsecond or frust.
That is memory-unsafe whether or not a given run crashes. Row D2 makes "State layout changed ->
restart" a hard requirement for any Phase 2 builder. Because the symbol matches, a symbol or
instance comparison cannot detect this case.

### D3. Return type of `build` changed (`measure.sh --target return-type`)

Card H0-00 (tsk_000001a1181b5c6eQeQDWKjI), run 2026-10-08 on `task/hb-h0-00` cut from main
1c29ba1d, dx 0.7.10 (57d6794), macOS, desktop, `--hot-patch --platform desktop --interactive false
--verbose`. The edit wraps the root of `HomePage::build` and bumps the sentinel (`hotpatch-sentinel:
vN`):

```
column()                               stack().child(column()
    .child(..)                  ->         .child(..)
    .cross_axis(CrossAxisAlignment::Center)    .cross_axis(CrossAxisAlignment::Center))
```

with `stack` added to the `use frust::{..}` list. `column()` returns `FlexView<State>` and
`stack()` returns `StackView<State>`, so the concrete type behind `impl View<State>` changes
`FlexView` -> `StackView`. Wrapping in a second `column()` does NOT change it: `FlexView`
children are type-erased, so `column().child(column()..)` is still a `FlexView`. That is why
`measure.sh` defaults to `RETURN_TYPE_WRAP=stack` and offers `RETURN_TYPE_WRAP=column` as the
control. The edit adds no new crate reference and no log line (D2's confound), so "did the patched
build run" cannot come from a log line. `AFTER_RUN_HOOK` captures the window instead.

**Stack wrap, three independent `dx serve` sessions** (`PRE_RUN_PAUSE` 40 / 20 / 20 s,
`POST_RUN_PAUSE=15`, `--runs 3`; `measure.sh` stops at the first crash, so each session has one run):

| Session | save->applied | dx thin build | dx log | App |
|---|---|---|---|---|
| 1 (cold, counter clicked to 2 first) | 1723 ms | 1672 ms | `replaying crates: ["hotpatch_spike_app"]`, `Hot-patching: app/src/home_page.rs took 586ms` | `Application [macos] exited with error: signal: 11 (SIGSEGV)` about 20 ms after the patch line; PID gone |
| 2 | 1260 ms | 693 ms | same, `took 326ms` | `frust-hotpatch: applied` then `exited with error: signal: 11 (SIGSEGV)` in the same millisecond |
| 3 | 1052 ms | 778 ms | same, `took 345ms` | `frust-hotpatch: applied` then `exited with error: signal: 11 (SIGSEGV)` in the same millisecond |

The before-captures show `hotpatch-sentinel: v0` (session 1: `count: 2`). No after-capture exists: the
window was gone, so the `AFTER_RUN_HOOK` screencapture could not find the process. So `v1` was
never seen and the patched `build`'s own output was never observed, in any session.

**Crash reports** (macOS `.ips`, sessions 1-3; sessions 1 and 2 were symbolicated and read, session
3's log shows the identical `signal: 11`): `EXC_BAD_ACCESS`, `SIGSEGV`, `KERN_INVALID_ADDRESS at
0x0000000000000020`, the same on both reports read. The faulting frame is
`<AnyView<HomeState> as View<HomeState>>::rebuild` +104, reached from
`FlexView::rebuild` -> `rebuild_children` -> `rebuild_children_positional` -> `rebuild_child_tracked`,
called from `ComponentView<HomePage>::rebuild` (the retained tree). Every frame in the report is in
the base image and the frames carry no patched-code symbol, so the report does not say whether the
new `build` ran to completion. A null-ish pointer plus offset 0x20 means a field read through a
pointer that is not what the old layout expects. The crash is on the first frame after the patch
in all three sessions: it is deterministic.

**Control, wrap in `column()`** (`RETURN_TYPE_WRAP=column`, `PRE_RUN_PAUSE=20`, `--runs 3`, one
session, PID 59432): 3/3 patched with no crash, the same PID before and after:

| Run | save->applied / frame submitted | Screen |
|---|---|---|
| 1 | 705 / 705 ms | `hotpatch-sentinel: v1` |
| 2 | 568 / 568 ms | `hotpatch-sentinel: v2` |
| 3 | 582 / 583 ms | `hotpatch-sentinel: v3` |

The patch pipeline, the sentinel capture and the harness are therefore sound, and the crash follows
the type change, not the wrap itself. (The control's counter was not clicked, so it shows `count: 0`
before and after. It makes no state-preservation claim.)

**Guard malloc** (same harness as D2, one `dx serve`, app SIGTERMed and relaunched under
`DYLD_INSERT_LIBRARIES=/usr/lib/libgmalloc.dylib`, `GuardMalloc[hotpatch-spike-<pid>]` banner
confirmed, stack wrap applied once after a `frust-hotpatch: frame`). The app logged
`frust-hotpatch: applied`, then:

```
thread 'main' panicked at pxfm-0.1.30/src/logs/log1p_dyadic.rs:98:21:
index out of bounds: the len is 188 but the index is 2147483647
```

The backtrace runs `pxfm::logs::log1p_dyadic::log1p_accurate` <- the patched
`<HomePage as Component>::build` (`call_mut`) <- `HotFunction::call_it`. `HomePage::build` has no
reason to reach a `log1p` routine, so this is a call through a wrong target, evidence of the new
code running against data it does not understand. The panic then ended in SIGSEGV
(`KERN_INVALID_ADDRESS at 0x0000000000000008`) in winit's `EventHandler::handle_event`, which
looks like unwinding out of an ObjC run-loop callout. I read that second crash as fallout of the
panic, not as an independent finding. This is one run. Guard malloc, as in D2, guards allocation
ends, so it is not what caught the corruption; the point of the leg is that the patched `build` does
run and misbehaves before the crash here, which the plain sessions could not show.

**Verdict for D3: crash.** An edit that changes `build`'s concrete return type is neither a no-op
(row D) nor silently survivable. dx matches the seam symbol
`<<HomePage as Component>::build as HotFunction<..>>::call_it`, which names the component and its
State but not `R`; that symbol is unchanged, so the patch is taken, and the old caller reads the
value, and the retained old `R` tree, through the wrong layout. It crashed on the first rebuild
in 3/3 plain sessions and in the guard-malloc session, against 3/3 clean patches when the type is kept.
What this does not show: which of the two crossings (the return slot or the retained tree `prev`)
faulted first, since the report has no patched-code frames; and any run where the type changed
and the app survived (none seen in 4 attempts).

**Does PORT.md 2.c's prediction hold?** Yes, as far as H0-00 can test it. 2.c predicts a
return-type change is a D2-class layout hazard (patch taken, not a no-op), that L1 removes only the
return slot, and that a retained-tree read through the new layout stays a hazard until L3 refuses
the patch. Observed: patch taken, crash on the first rebuild, in a retained-tree walk
(`AnyView::rebuild` under `FlexView::rebuild`), type kept -> clean. That is not a behaviour 2.c
fails to predict, so the "stop at phase H0" condition is not triggered. Two limits: dx here has no
L1 or L3, so this measures the hazard L3 must catch, not that L3 catches it (that is H1-05's
fixture); and the faulting frames being in the base image does not separate return slot from
retained tree.

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
  construction, because D2's symbols are identical. Detection needs a **layout fingerprint** of every type
  whose values cross the patch boundary at the seam — the component type `C` (props),
  `C::State` and `build`'s concrete return type: size, alignment and the type's structure
  (field names, offsets and types, recursively), recorded at fat-build time and compared
  against the patch. Any mismatch means restart. A State-only fingerprint would miss a new prop
  field or a changed return type, whose symbols are equally unchanged.
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

- **Boundary layout gate (row D2, mandatory).** Before applying a patch, compare a layout
  fingerprint of every type whose values cross the old/new boundary at a patched `build` — the
  component type `C` (props), `C::State` and the concrete `build` return type, recursively
  (size, alignment, type structure) — against the running binary's. Any difference means
  restart, never apply. A symbol or instance comparison is not enough: a field added to a named
  struct, a new prop field and a changed return type all keep the symbol.
- **Silent no-ops (row D)** should be surfaced: a State type-identity change should report
  "restart needed" instead of "patched". Return-type changes are not a no-op case; they belong
  under the boundary layout gate above (untested, see row D).
- **frust path dependencies (row E)** should be covered, or the builder should tell the developer
  to restart.

The p1-01 seam also must stay debug-only. The spike's runner now connects to the devserver only
under `cfg(debug_assertions)`, the same gate subsecond's jump table uses.

## Phase 2: frust-hotpatch runtime

Card p2-01b (tsk_000001a1165e435aotlRYShH), run 2026-10-07 on `task/hp-p2-01` (base
`spike/hotpatch` @ 219e099d plus this card's changes), same machine, toolchain and dx 0.7.10 as the
Environment block above. The in-app runtime is now `crates/frust-hotpatch`, a native-only port of
subsecond 0.7.10. frust-core's `call_build` / `set_patch_listener` call `frust_hotpatch::HotFn` /
`register_handler`, and the runner uses `dioxus_devtools::connect(callback)`: the callback keeps
`connect_subsecond`'s filter (a `HotReload` message with a jump table and `for_pid` equal to this
process), converts dx's `subsecond_types::JumpTable` into `frust_hotpatch::JumpTable` by a
`serde_json` round-trip and calls `frust_hotpatch::apply_patch`. `dioxus-devtools` still links
subsecond (it reports `aslr_reference` in the websocket URL), but nothing calls subsecond's
`apply_patch` any more. The `frust-hotpatch: applied` probe line comes from the listener
registered with `frust_hotpatch::register_handler`, so every `applied` line below proves the patch
went through frust-hotpatch.

All sessions used `PRE_RUN_PAUSE=20 POST_RUN_PAUSE=15 ./measure.sh --target <home|card> --runs 5`
with `DX` exported, except the two control rows (no pauses, no clicks). Increment was clicked three
times during the pre-run pause (AppleScript click at the button centre, window at `500,141`, size
`800x632`); all three landed this time.

| Row | Session | Patched without relaunch? | State kept? | save->frame submitted median (per run), n | dx thin build | `replaying crates` |
|---|---|---|---|---|---|---|
| A | home, frust-hotpatch, clicks + captures | yes, PID 20117 alive (same) for all 5 | yes: count 3 -> 3, `hotpatch-sentinel: v0` -> `v5` | **575** (960 / 569 / 566 / 579 / 575), 5 | 533-560 ms | `["hotpatch_spike_app"]` |
| B | card, frust-hotpatch, clicks + captures | yes, PID 21385 alive (same) for all 5 | yes: count 3 -> 3, `card-sentinel: v0` -> `v5` | **577** (580 / 566 / 575 / 577 / 580), 5 | 532-544 ms | `["hotpatch_spike_app"]` |
| A' | home, frust-hotpatch, screen locked (no clicks possible) | yes, PID 98843 same | not checked | 576 (1147 / 569 / 576 / 570 / 578), 5 | 528-1066 ms | `["hotpatch_spike_app"]` |
| B' | card, frust-hotpatch, no clicks | yes, PID 17881 same | not checked | 583 (994 / 568 / 583 / 593 / 582), 5 | 528-550 ms | `["hotpatch_spike_app"]` |
| A-ctl | home, **control**: unmodified base 219e099d (subsecond runtime), exported to a scratch dir, cold `target/` | yes, PID 26887 same | not checked | 493 (495 / 480 / 493 / 493 / 499), 5 | 451-458 ms | `["hotpatch_spike_app"]` |
| A-exp | home, frust-hotpatch, experiment: field-by-field table conversion instead of serde (reverted) | yes, PID 27994 same | not checked | 546 (552 / 551 / 542 / 538 / 546), 5 | 506-521 ms | `["hotpatch_spike_app"]` |

No crash in any session, and every run logged both probe lines. Quoted from row B:

```
DEBUG Patch rebuild: changed_crates=["hotpatch_spike_app"], modified_crates={"hotpatch_spike", "hotpatch_spike_app"}
DEBUG replaying crates: ["hotpatch_spike_app"]
INFO Hot-patching: app/src/counter_card.rs took 310ms
[frust INFO] frust-hotpatch: applied t_unix_ms=1791382075220
```

**Comparison with the subsecond runtime.** Rows A/B are about 100 ms slower than the Phase 1 medians
(475 / 479 ms) and about 80 ms slower than a same-day control on the unmodified base (A-ctl, 493 ms).
The runtime is not where the time goes. From dx's `Build completed` to the app's `applied` line
takes 20-35 ms in every session, frust-hotpatch and control alike, and `applied` and `frame` land
in the same millisecond. The whole difference is inside dx's thin build, in its `Compiling` step
(the tip crate, i.e. the runner bin, which dx recompiles on every patch): 104-106 ms in the control,
181-192 ms with the new runner. `Workspace hotpatch replay` (241 ms) and `Patch: Link` (79-80 ms)
are unchanged. The serde round-trip in the runner accounts for about 15-20 ms of it (A-exp:
`Compiling` 160-174 ms). The rest is most likely `dioxus_devtools::connect(callback)` itself: it is
generic over the callback, so the tip crate now monomorphises the websocket loop and the
`DevserverMsg` deserializer that `connect_subsecond` kept pre-compiled inside dioxus-devtools. This
is a property of where the connection code lives, not of frust-hotpatch: moving the devserver
connection and the table conversion out of the tip crate (into a non-tip lib or the frust shell)
should return the thin build to the control's figure. Even as measured, 575 ms is 37% of row F's
1570 ms restart median, under the 50% bar.

The first patch of each session is slower (960-1147 ms), as in Phase 1 (2128 ms cold): dx's first
thin build of a session takes longer.

**Screen-lock note.** The first frust-hotpatch session (A') ran while the screen was locked. dx's
fat build finished in 71 s, but the window was not created until 378 s, so its first-frame time is
meaningless. Patching itself worked locked, at the same latency.

**Plain launch.** `cargo run -p hotpatch-spike` (no dx) logs exactly one `frust-hotpatch: frame`
line, as before.

## Phase 2: Android load probe

Card p2-02b. Can an installed Frust app load a code library that arrives after install, and by
which path? `android-probe/probe.sh` scaffolds a throwaway `frust create` app (in a mktemp dir,
never committed), overlays `android-probe/app_lib.rs`, installs the debug APK, delivers
`libfrust_probe_patch.so` (built from `android-probe/patch/`, exports
`frust_probe_value() -> u32 { 42 }`) into the app's files dir with `run-as cp`, launches the app
and reads logcat. The root component's `init` tries three strategies once, before the first frame.
See `android-probe/README.md`.

**This probe tests loading only.** The library is self-contained: no jump table is installed and
nothing in it is relocated against the running app library. Building a patch that links against
the base library's addresses and making those addresses resolve is the patch builder's job
(PORT.md), and nothing here says that part works on Android.

**Run.** `probe.sh --serial <adb-serial> --keep --log-dir <scratch>`, base `spike/hotpatch` @
593d54d1 plus this card's files, 2026-10-07.

| | |
|---|---|
| Device | Pixel 5 (`redfin`), USB serial <adb-serial> |
| Android | 14 (SDK 34) |
| `ro.build.fingerprint` | `google/redfin/redfin:14/UP1A.231105.001.B2/11260668:user/release-keys` |
| App | `dev.frust.probe.probeapp`, debug APK, arm64-v8a, `targetSdk 36`, process domain `u:r:untrusted_app:s0:c18,c257,c512,c768` (`ps -Z`) |
| Build | `frust build apk --debug --target-platform android-arm64` (Gradle 9.5.1, JDK 21.0.8 from Android Studio's JBR as `JAVA_HOME`: Gradle accepted 21), patch via cargo-ndk 4.1.2 `-t arm64-v8a build --release`, rustc 1.98.1 |
| Delivery | `adb push` to `/data/local/tmp`, then `adb shell run-as <app> cp` into `files/`; the file there is `u:object_r:app_data_file`, owned by the app uid |

| Strategy | Load path | Outcome |
|---|---|---|
| `memfd` | `frust_hotpatch::load_patch_library(<files>/libfrust_probe_patch.so)`: bytes copied into a memfd, `android_dlopen_ext(ANDROID_DLEXT_USE_LIBRARY_FD)` | **loaded, returned 42** |
| `plain-dlopen` | `libloading::Library::new` (`dlopen`) on the same files-dir path | **loaded, returned 42** |
| `cache-dir` | file copied to `<cache>/libfrust_probe_patch.so`, then `dlopen` | **loaded, returned 42** |

Verbatim logcat (`adb logcat -d`, filtered on `frust-probe`; the app logs under the `frust` tag):

```
10-07 21:50:51.066 14417 14417 I frust   : probeapp: frust-probe: strategy=memfd result=42
10-07 21:50:51.066 14417 14417 I frust   : probeapp: frust-probe: strategy=plain-dlopen result=42
10-07 21:50:51.066 14417 14417 I frust   : probeapp: frust-probe: strategy=cache-dir result=42
```

No `avc: denied` line was logged during the run. The three loads are three distinct images:
`/proc/14417/maps` shows an `r-xp` text segment for each of `/memfd:frust-hotpatch (deleted)`,
`/data/data/dev.frust.probe.probeapp/files/libfrust_probe_patch.so` and
`/data/data/dev.frust.probe.probeapp/cache/libfrust_probe_patch.so`. The bionic linker did not hand
back one already-loaded library for the later calls.

**Controls** (same install, relaunched by hand with `am force-stop` + `am start`):

1. *Mode bits.* `adb push` leaves the file `-rwxrwxrwx` and `run-as cp` keeps that. A file the app
   wrote itself would be `0600`, so the files copy was set to `chmod 600` and the cache copy
   deleted. All three strategies still returned 42 (`10-07 21:51:18.797 ... strategy=memfd
   result=42`, same for `plain-dlopen` and `cache-dir`). The exec bit is not needed.
2. *Negative control: can the probe see a failed load at all?* Yes. The patch file was replaced
   with 11 bytes of text, and each loader's own error reached the log:

   ```
   10-07 21:51:26.462 14891 14891 I frust   : probeapp: frust-probe: strategy=memfd error=Failed to load library on Android: android_dlopen_ext failed: dlopen failed: "/memfd:frust-hotpatch (deleted)" is too small to be an ELF executable: only found 11 bytes
   10-07 21:51:26.462 14891 14891 I frust   : probeapp: frust-probe: strategy=plain-dlopen error=dlopen failed: "/data/data/dev.frust.probe.probeapp/files/libfrust_probe_patch.so" is too small to be an ELF executable: only found 11 bytes
   10-07 21:51:26.462 14891 14891 I frust   : probeapp: frust-probe: strategy=cache-dir error=dlopen failed: "/data/data/dev.frust.probe.probeapp/cache/libfrust_probe_patch.so" is too small to be an ELF executable: only found 11 bytes
   ```

   With the file removed, all three logged `error=patch file missing: /data/user/0/dev.frust.probe.probeapp/files/libfrust_probe_patch.so`.

**Interpretation.** On this device, a frust app process in `untrusted_app` loads app-delivered
code by all three paths. Neither SELinux (`app_data_file` may be mapped executable) nor the app's
linker namespace (its permitted paths cover the app's data dir) blocks a plain `dlopen` from
`files/` or `cache/`. So the memfd detour is not *required* on Android 14 for a file inside the
app's own data dir. A frust builder should still use frust-hotpatch's default:

- **Load path: `frust_hotpatch::load_patch_library` (memfd + `android_dlopen_ext`).** It works
  here, `apply_patch` already uses it, and it does not depend on where the bytes sit or on that
  location's SELinux label or namespace permission. Plain `dlopen` from the app's data dir is a
  verified fallback.
- **Delivery: get the bytes into the app's own data dir** (`files/` or `cache/`). This probe
  verified `adb push` + `run-as <applicationId> cp`. That needs a debuggable build, which hot
  patching is anyway (debug-only, see the frust-hotpatch README). Having the app fetch the bytes
  from the devserver and write them itself is the other option, and control 1 shows such a `0600`
  file loads.

**Not covered.** Loading straight from `/data/local/tmp` (where `adb push` lands) was not tried:
the probe always copies into the app's dirs first. Only a debuggable APK on one device and Android
version was tested, and no release or non-debuggable build. 32-bit (`armeabi-v7a`) and x86_64
were not tested. Symbol interposition and relocation against the base library are not covered
(see the scope note above).

## Phase 2 conclusion

Card p2-03c (tsk_000001a1169831d4W8yKhj8T). Phase 2 shows that frust can own both halves of hot
patching. The ported runtime works: frust-hotpatch drives rows A and B at a 575 / 577 ms median,
which is 37% of the 1570 ms restart. An Android app also loads code delivered after install, by
memfd, by plain `dlopen` from `files/` and from `cache/`, though only with a self-contained library,
not a relocated patch. [PORT.md](PORT.md) sizes the frust-owned builder that would replace dx. dx's
native builder core is 3,153 lines at v0.7.10, 2,023 of them code. The frust builder is estimated at
4,610 new code lines plus 2,690 test lines for desktop (stage 1), and about 9,500 lines (6,110 code,
3,390 tests) once Android, Windows and the iOS simulator are added. PORT.md section 6 is the
authoritative sizing; these figures are copied from it as of its revision r2-04 and follow it if it
changes. PORT.md gives each Phase 1 requirement a cited cause and a design, and ends with 36
adoptable cards. The key design is the boundary-layout gate, covering the component type, its State
and `build`'s return type, recursively: `build` erases its result inside the hot function, the host
compares DWARF layouts against every accepted image before it sends a patch, and the app keeps a
creator-image State-size witness as a backstop. PORT.md recommends STAGED delivery, desktop first.
Milestone 1 is `frust run --watch` hot-patching an unmodified one-package `frust create` app on
macOS, with Linux covered by a CI canary, and rows D, D2, D3, D4, D5 and E must each answer
"restart required". Android follows once a device gate proves patch relocation against the base
cdylib. Row D3, a `build` return-type edit, was measured afterwards by card H0-00 and crashed
the app (see D3).

## Milestone 1: `frust run --watch` on a stock `frust create` app (H1-11)

Card H1-11 (tsk_000001a1181b5c72XYnKkX5g), run 2026-10-08 on `task/hb-h1-11`, cut from
`feature/hotpatch-h1` @ ad69b4d6 (every other H1 card merged: builder, session, CLI hot mode, TUI
hot path, canary). Same machine, OS and toolchain as the Environment block above (`Mac16,1`, Apple
M4, macOS 27.0.1, rustc/cargo 1.98.1), logged-in GUI session, screen unlocked throughout. dx is not
used anywhere in this section.

### Verdict: FAIL

The pass bar (PORT.md §7) has three parts, and two fail:

1. **Median save->frame at most 50% of row F: FAIL.** Row A's median is **2191 ms**. Row F, the
   restart through `frust run --watch --no-hot`, has a median of **3671 ms** over 10 runs in two
   sessions (3288 and 3706 ms per session). 2191 ms is 59.7% of the pooled F (66.6% and 59.1% of
   the per-session medians). The bar is 1836 ms; no hot row's median comes under it (A0, A, B, C,
   G: 2191-2309 ms).
2. **Zero `patched` lines for D/D2/D3/D4/D5/E: FAIL, on D3.** D, D2, D4's second edit, D5 and E
   answer `restart required` on every run (16 runs in total, 2 of them under guard malloc). Row D3,
   H0-00's stack wrap, prints `patched` 3/3, plus 2/2 under guard malloc. The app neither crashes
   nor shows a heap error, and its state survives (row D3 below), but the bar says zero.
3. **Row A against the 493 ms estimate: recorded.** 2191 ms is 1698 ms (4.4x) over the estimate.
   The ~290 ms margin PORT.md §7 allowed is exceeded almost six times over. Where the time goes
   is measured below (*Latency breakdown*). The single largest new cost is moving the patch to the
   app (~800 ms), which no Phase 1/2 row paid.

What holds: a stock one-package `frust create` app hot-patches with no template change. Child,
second-child and new-function edits land in the same PID with State kept (rows A0, A, B, C, H). Ten
patches run in one process (G). The State and type layout hazards (D2, D4's second patch, D5)
and the State identity change (D, see its reason variant) are refused before anything is sent, E
is refused by path class, and the TUI Watch leg behaves like the CLI. D3 is the exception above.

Per PORT.md §7 a FAIL stops the plan at milestone 1. Neither failure is softened here.

### Rig and method

- **frust**: `cargo build --release -p frust-cli` in the worktree (1 m 49 s), i.e. the optimised
  `frust` binary a developer installs. Every row used that binary: CLI, builder (`frust` as
  `RUSTC_WORKSPACE_WRAPPER` and linker) and TUI. The app is a debug build (hot sessions are
  debug-only).
- **App**: `frust create hotapp --frust-path <worktree>` into a scratch directory outside the
  repository (never committed): one package, the template's `crate-type = ["cdylib", "staticlib",
  "rlib"]`, Material 3, generated `main.rs` and `frust::app!` line. Row A0 ran on it unmodified.
  Then `measure.sh --prepare-app` rewrote `src/home_page.rs` and `src/lib.rs` and added
  `src/counter_card.rs` (header of `measure.sh`). It kept the template's scaffold, app bar, FAB
  and label, and added a named `HomeState { count }`, a `CounterCard` child, the edit markers, and
  a root `build` that hosts `HomePage` in one arm of `either(home_page::SHOW_HOME, ..)` (row D5's
  shape). It changed no package, crate type, `main.rs` or `app!` line. Row E ran on a second app
  whose `--frust-path` is a `git archive` copy of ad69b4d6 in the scratch directory, so the
  framework edit never touched the worktree.
- **Runs**: `measure.sh --frust-run --app <app> --target <t> --runs <n>` runs `frust run --watch`
  from the app directory and logs every output line as `<arrival unix ms> <line>`. It edits in
  place, stamps the save, and reads the CLI's verbatim `patched in N ms (k components rebuilt)` /
  `restart required: <reason>` line and the app's unchanged `frust-hotpatch: applied
  t_unix_ms=` / `frust-hotpatch: frame t_unix_ms=` probes (level info). The app PID comes from the
  executable path (`ps`), RSS from `ps -o rss=` (reported in MiB, i.e. KiB / 1024). **Times are
  save->frame submitted** (the probe's definition, see Matrix above). Hazard edits are cumulative,
  so each run changes the layout against the base the previous run's restart relaunched from.
- **Row F**: `measure.sh --frust-restart`, which is `frust run --watch --no-hot --features
  frust/hotpatch` (the CLI's own kill + `cargo run`). The feature only turns on the first-frame
  probe. Timed to the relaunched process's first frame.
- **State evidence**: Increment (the FAB) was clicked three times through System Events after the
  first frame. The first click returns `missing value` and misses, as before, so the count is 2.
  Then `screencapture` of the window (made frontmost) after every run. Captures and logs stayed in
  the scratch directory and are quoted here, not committed.
- **Guard malloc**: `FRUST_RUN_ARGS="--define DYLD_INSERT_LIBRARIES=/usr/lib/libgmalloc.dylib"`.
  `frust run`'s `--define` pairs reach the spawned fat image's environment (`desktop_exe_plan`) and
  the fat build's, so the session drives a guard-malloc app itself. A fat exe launched by hand
  cannot be driven: the session only attaches to the child it spawned, through that child's
  discovery line (`read_discovery` in `hotpatch::session`), and has no attach-to-pid entry. The
  cost of the `--define` route is that the CLI's build and link children run under guard malloc
  too (`GuardMalloc[frust-<pid>]` banners from the linker step).
- The machine was otherwise idle during every timed session. No build ran in parallel; the spike
  gate ran afterwards.

### Matrix

| Row | Edit | Outcome lines (n) | PID / State | save->frame median (min-max), n | Verdict |
|---|---|---|---|---|---|
| A0 | template label `...this many times:` -> `... vN` on the **unmodified** generated app | `patched in 1812-2207 ms (1 components rebuilt)` (5/5) | 17422 kept; count 2 -> 2, label `v5` | **2309** (2128-2828), 5 | patched |
| A | home sentinel (prepared app) | `patched in 1852-2229 ms (1 components rebuilt)` (5/5) | 22039 kept; count 2 -> 2, `hotpatch-sentinel: v5` | **2191** (2172-2623), 5 | patched; **bar FAIL** |
| B | card sentinel, second child `CounterCard` | `patched in 1840-2002 ms (1 components rebuilt)` (5/5) | 24304 kept; count 2 -> 2, `card-sentinel: v5` | **2210** (2159-2440), 5 | patched |
| C | new private fn called from `HomePage::build` | `patched in 1874-2036 ms (1 components rebuilt)` (5/5) | 26153 kept; count 2 -> 2, `hotpatch-helper: v5` | **2219** (2195-2392), 5 | patched |
| D | `HomeState` -> (N+1)-tuple (type identity) | `restart required: ... ComponentWidget<hotapp::home_page::HomePage> changed layout (248 bytes, members moved); ... restarting to keep memory safe` (3/3) | new PID each run | outcome line at save+1236-1402; relaunched first frame 6997 (5567-8456), 3 | restart, but **`LayoutChanged`, not `StateTypeChanged`** |
| D2 | `HomeState` gains `extra1..extraN` (nested component, root through the seam) | `restart required: hotapp::home_page::HomeState changed layout (4 → 8 bytes); restarting to keep memory safe`, then 8 → 12, 12 → 16 (3/3) | new PID each run; relaunched app logs `extra1=4242` | line at save+1232-1316; relaunch 5384 (5294-6815), 3 | restart (`LayoutChanged`) |
| D3 | H0-00's edit: root `scaffold(..)` wrapped in N `stack()`s | `patched in 1958-2203 ms (1 components rebuilt)` (3/3; 2/2 under guard malloc) | 39066 kept, `hotpatch-sentinel: v3`; guard malloc: count 2 kept, FAB still counts to 4 | 2307 (2278-2702), 3 | **patched: bar FAIL** (no crash, no heap error) |
| D4 | pair: patch 1 adds `struct Badge<k> { n: u32 }`, patch 2 adds `m` | patch 1: `patched in 2168 / 1894 ms`; patch 2: `restart required: hotapp::home_page::Badge1 changed layout (4 → 8 bytes); restarting to keep memory safe` (and `Badge2`) | patch 1 keeps the PID, patch 2 relaunches | patch 2: line at save+1018-1019; relaunch 5471 / 5987, 2 pairs | patch 2 refused against the accepted set (`LayoutChanged`) |
| D5 | `HomeState` gains `extraN` AND `SHOW_HOME` flips the root `either(..)` arm | `restart required: hotapp::home_page::HomeState changed layout (4 → 8 bytes); restarting to keep memory safe`, 8 → 12, 12 → 16 (3/3; 2/2 under guard malloc) | new PID each run; relaunched app shows the other arm | line at save+1231-1339; relaunch 7038 (6061-7517), 3 | restart (`LayoutChanged`); guard malloc: no heap error |
| E | append a line to `frust-widgets/src/button.rs` of the app's frust path dependency | `` restart required: `<frust copy>/crates/frust-widgets/src/button.rs` belongs to the path dependency `frust-widgets`, which a patch cannot replay `` (3/3) | new PID each run | line at save+320-419; relaunch 7742 (7494-9331), 3 | restart (`PathDependencyChanged`) |
| F | restart: `frust run --watch --no-hot`, kill + `cargo run` | n/a | new PID each run; count 2 -> 0 | **3671** pooled (2584-4504), 10: session 1 3288, session 2 3706 | baseline |
| G | 10 consecutive home patches | `patched in 1865-2141 ms` (10/10) | 28144 kept; count 2 -> 2, `v10` | 2276 (2187-2750), 10 | no crash; RSS +21.9 MiB; counters below |
| H | one-package template app patches | A0 (unmodified app) and A-C (same package shape) | | | **yes** |
| I | cold fat session vs cold `cargo build`, one `target/` | | | fat: first frame 74.3 s (2nd cold app: 67.4 s); `cargo build`: 63.0 s | fat +18%; one target dir |

Zero `patched` lines appeared for D, D2, D4's second edit, D5 and E. D3 printed three, plus two
under guard malloc.

### A, A0, B, C, H: patched with State kept

The CLI line and the probe lines of row A run 5, as logged (arrival stamp first):

```
1791452489677 patched in 1852 ms (1 components rebuilt)
1791452489677 [frust INFO] frust-hotpatch: applied t_unix_ms=1791452489673
1791452489677 [frust INFO] frust-hotpatch: frame t_unix_ms=1791452489675
```

The applied and frame probes land in the same or the next millisecond in every patched run, so
save->applied and save->frame are interchangeable here. The CLI's `patched` line follows `frame` by a
few milliseconds (`PatchOutcome` is answered after the next frame). The app's own lines reach the
CLI's stdout only when `on_change` returns, so their arrival stamps bunch; only the `t_unix_ms`
values are used.

- **A0 (row H's strongest case).** This was the unmodified `frust create` output, in the same
  session as row I's cold start (first frame 74.3 s after launch). Per run: 2828 / 2427 / 2141 /
  2309 / 2128 ms. Before-capture: count 2 and the stock label; after run 5: count 2, label `v5`,
  PID 17422 throughout.
- **A.** 2623 / 2542 / 2179 / 2191 / 2172 ms; PID 22039; before `count: 2`, `hotpatch-sentinel:
  v0`; after `count: 2`, `v5`, `card-sentinel: v0`.
- **B.** 2440 / 2210 / 2159 / 2323 / 2183 ms; PID 24304; after `count: 2`, `card-sentinel: v5`,
  `hotpatch-sentinel: v0`. A patch reaches a second `component(..)` child in another module.
- **C.** 2392 / 2350 / 2201 / 2219 / 2195 ms; PID 26153; after `hotpatch-helper: v5`, `count: 2`.
- **H.** Answered by A0 and A-C: the frust-owned builder patches the template's one-package
  `[lib]` with all three crate types. Rows H of Phase 1 (dx, `replaying crates: []`) no longer
  apply. Patch images are 3,017,952 bytes for a sentinel edit (3,018,096 with a new fn), twice the
  spike's 1.5 MB.
- **First patch of a session.** It costs 2.4-2.8 s against 2.2-2.3 s steady (run 1 of A0 / A / B /
  C / G: 2828 / 2623 / 2440 / 2392 / 2750 ms). The difference sits before the build starts:
  save->`on_change` is 358-839 ms in those runs, 312-324 ms in steady state. The 1-2 s first-patch
  penalty of the dx rows is gone.

### Latency breakdown (why 2.2 s, not 493 ms)

Two sources split a steady-state patched run:

- **Host files.** The session dir (`<target>/frust-hotpatch/session-hotapp/`) keeps `stub-N.o` and
  `patch-N.dylib`. Their mtimes give "stub written" and "patch linked".
- **App files.** A 2 ms poller on the app's cache dir (`~/Library/Caches/frust-hotpatch/`) gives
  the moment the app finished writing the uploaded patch (file mtime) and the moment it removed
  the file after loading it.

Measured over 4 runs in a diagnostic session (median 2260 ms, consistent with A):

| Phase | ms (steady state) |
|---|---|
| save -> `on_change` starts (300 ms debounce + file event) | 312-324 |
| `on_change` -> `stub-N.o` written (thin compile of the tip lib with its three crate types, L3 DWARF read + diff, seam check, symbol work) | 640-900 |
| `stub-N.o` -> `patch-N.dylib` linked | 75-81 |
| patch linked -> app's copy fully written (`patch_chunk` upload of 3,017,952 bytes as base64) | **801-805** |
| app's copy written -> removed = `applied` probe (write, `dlopen`, table install; same ms as `applied`) | 335-367 |
| `applied` -> `frame` | 0-2 |

A standalone `dlopen` of fresh copies of a patch image (ad-hoc, linker-signed) took 111-329 ms,
so most of the last phase is the first-map signature check, not frust code. Against PORT.md §7's
assumptions, the estimate had no ~800 ms transport and no ~340 ms load. It also assumed the tip
compile would be "used up" by the gates, but compile + gates here are 640-900 ms. The debounce
(300 ms, `WATCH_DEBOUNCE`) alone is 61% of the 493 ms estimate.

### D: State type identity: refused, but as `LayoutChanged`

All three runs (tuple of 2, 3 and 4 `u32`) answered, verbatim:

```
restart required: frust_core::component::ComponentWidget<hotapp::home_page::HomePage> changed layout (248 bytes, members moved); frust_core::component::{impl#2}::teardown::{closure_env#0}<(), hotapp::home_page::HomePage> changed layout (32 bytes, members moved); frust_widgets::either::EitherWidget<frust_core::component::ComponentWidget<hotapp::home_page::HomePage>, frust_widgets::text::TextWidget> changed layout (544 bytes, members moved); frust_widgets::either::EitherWidget<frust_core::component::ComponentWidget<hotapp::home_page::HomePage>, frust_widgets::text::TextWidget>::Left changed layout (544 bytes, members moved); restarting to keep memory safe
```

PIDs 33888 -> 34600 -> 35245 -> 35809. Acceptance criterion 2 expects `StateTypeChanged` here.
`AcceptedSets::check` runs the L3 diff before the seam identity check, and a State identity change
also changes the member type of `ComponentWidget<HomePage>` (`state: Option<Box<HomeState>>` ->
`Option<Box<(u32, u32)>>`). The size stays the same, but the hashed member type does not, so L3
refuses first. On a DWARF target this edit can never reach `StateTypeChanged`. The edit is safe
(never patched), but the reason a developer reads is a layout message about framework types, not
"State type changed".

### D2, D4, D5: refused by L3 against the accepted set

- **D2** (`HomeState` in `HomePage`, a nested component, with the root routed through the seam):
  4 → 8, 8 → 12, 12 → 16 bytes; PIDs 36501 -> 37037 -> 37574 -> 38128. The relaunched app reads
  the new field correctly: `frust-hotpatch-spike: state-field v1 build ran extra1=4242`. (Its
  `build` runs and logs this on every frame, about 41,000 lines in a few seconds, so the log line
  should not be copied into real code.)
- **D4**: run 1 adds `Badge1 { n }` -> `patched in 2168 ms`, PID 40432 kept, new type merged into
  the accepted set. Run 2 adds `m` -> `restart required: hotapp::home_page::Badge1 changed layout
  (4 → 8 bytes); restarting to keep memory safe`. The second pair (`Badge2`, PID 41202) repeats
  it exactly. The base table never held `Badge1`, so this refusal comes only from the accepted
  set.
- **D5**: the same edit as D2 plus the `either` arm swap in one save. 3/3 `restart required:
  hotapp::home_page::HomeState changed layout (...)`. After run 1 the relaunched window shows
  `HomePage is in the other arm (SHOW_HOME = false)`. Nothing was sent to the old process, so the
  in-app swap never ran over a mismatched state.
- **D5 under guard malloc** (2 runs): `restart required: hotapp::home_page::HomeState changed
  layout (4 → 8 bytes)` and `(8 → 12 bytes)`. Three app processes printed
  `GuardMalloc[hotapp-<pid>]: Allocations will be placed on 16 byte boundaries.` (PIDs 46200,
  47618, 48945). No `GuardMalloc` error, signal, panic or abort appears in the log. The refusal
  sends nothing, so no non-creator drop can happen. The in-app leak path is H0-02's test.
  Guard-malloc timings (outcome line at save+7.1-7.8 s, RSS ~960 MiB) are not latency data.

### D3: H0-00's edit is patched, and survives

Each run wraps the root of `HomePage::build` in one more `stack()` (`stack().child(scaffold(..))`,
then two, then three). This is a new concrete return type every time, the H0-00 edit on the
template's scaffold root.

- **Plain** (PID 39066): `patched in 2203 / 1987 / 1958 ms (1 components rebuilt)`, save->frame
  2702 / 2307 / 2278 ms. Same PID, `hotpatch-sentinel: v3` on screen, no crash.
- **Guard malloc** (PID 50548, banner confirmed): `patched in 8139 / 3486 ms`. The count was 2
  before the patches and 2 after. Two clicks after the second patch took it to 4, so the patched
  tree is live and interactive. No heap error, signal or panic.

**Against H0-00's prediction note.** PORT.md 2.c predicted the following. L1 removes the return
slot. The retained-tree crossing stays a hazard. L3 refuses the edit (H1-05 fixture
`d3_the_observed_stack_wrap_is_refused`). Observed instead: L3 reports nothing. In frust-core,
`ComponentWidget<C>` holds `prev: AnyView<C::State>` and `child: ChildPod`, both erased
(`crates/frust-core/src/component.rs:133-162`). A new `R` therefore reaches the DWARF table only as
new types (`StackView<..>`), and a type absent from the accepted set passes. The H1-05 fixture
refuses the edit only because its mock `ComponentWidget` keeps `prev: C::View` by value
(`crates/frust-drive/tests/fixtures/hotpatch/core/src/lib.rs:125-131`), a layout the real crate no
longer has. The edit is survivable because `AnyView`'s rebuild compares `TypeId`s and, on a
mismatch, tears the old subtree down through the old view and builds a fresh one
(`crates/frust-core/src/view.rs:253-273`). H0-00's crash came from the return slot and the
by-value tree that dx-era frust had. The row does not match "refused by L3"; it matches "L1 makes
a type-changing `R` an ordinary rebuild". That is not a crash, but under the bar's letter it is a
`patched` line on a hazard row. A same-named type that changes layout inside `R` (a field added to
an app view struct) remains L3's job and is what D2/D4 exercise.

### E: framework edit

Three appended-comment edits to `crates/frust-widgets/src/button.rs` of the scratch frust copy.
Each answered `` restart required: `<frust copy>/crates/frust-widgets/src/button.rs` belongs to
the path dependency `frust-widgets`, which a patch cannot replay `` within 320-419 ms. The relaunch
re-fat-builds the edited crate and its dependents: first frame at save+7.5-9.3 s, PIDs 56457 ->
57119 -> 57886 -> 58559. The file was restored byte-identical afterwards (`cmp`).

### F: restart baseline through `frust run --watch`

`--no-hot` relaunch loop, home sentinel edits:

- Session 1: 3707 / 2774 / 2584 / 3288 / 3720 ms (median 3288). Before PID 19259 with `count:
  2`, after PID 20957 with `count: 0` and `v5`.
- Session 2: 4054 / 3706 / 3452 / 4504 / 3636 ms (median 3706). PIDs 68724 -> 69569 -> 69890
  -> 70227 -> 70539 -> 70919.

cargo's `Finished` per relaunch is 1.4-3.5 s. The template's three crate types make every restart
re-link a cdylib and a staticlib as well as the bin, which is why F here is 2.1-2.3x the spike's
1570 ms. Pooled F is **3671 ms** (n=10).

For comparison, the hot mode's own restart path (`restart required` -> kill -> fat rebuild ->
relaunch) costs 5.3-9.3 s to the relaunched first frame (D, D2, D4, D5, budget; pooled median
6438 ms, n=12). That is roughly twice the `--no-hot` restart a developer would otherwise get.

### G: ten patches, RSS and the budget counters

PID 28144, count 2 kept, `hotpatch-sentinel: v10` after run 10. Per-run save->frame: 2750 / 2187 /
2351 / 2194 / 2242 / 2368 / 2208 / 2461 / 2310 / 2200 ms. No crash and no trend.

RSS (MiB, sampled each second and after each run):

| Point | RSS |
|---|---|
| after the clicks, before patch 1 (settled) | 121.9 |
| after patch 1 | 135.6 |
| after patches 2-10 | 136.7, 137.5, 138.5, 139.3, 140.2, 141.1, 142.0, 142.9, 143.8 |
| 15 s after patch 10 | 143.7 (flat) |

The first patch costs +13.7 MiB, every later one about +0.9 MiB (spike: +0.55 MB). That is +21.9
MiB over 10 patches.

**Budget counters.** The CLI prints no counters on a `patched` outcome; `patched in N ms (k
components rebuilt)` is all it says. To read the app's own counters, a session ran with
`frust.toml` `[hotpatch] patches = 3`. Three patches landed (`patched in 1898 / 1922 / 2032 ms`,
PID 31777). The fourth save answered, verbatim:

```
restart required: the patch budget is spent (4 patches / 9053856 bytes loaded would pass 3 patches or 100663296 bytes)
```

So after three patches the app reported `patches_applied = 3` and `patch_bytes_loaded = 9053856`,
exactly 3 × 3,017,952. After G's ten patches the same counters are 10 / 30,179,520 (ten 3,017,952-
byte images in the session dir). At this app's patch size the default budget's **byte** limit (96
MiB = 100,663,296) binds at the 34th patch, before the 64-patch limit.

### I: cold builds and one `target/`

- **Cold `cargo build`** of the generated app (no features), `build/rust` empty: 63.0 s wall
  (cargo: `Finished ... in 1m 02s`), 3.1 GiB.
- **`cargo clean`**, then a cold **`frust run --watch`** (row A0's session): first frame **74.3 s**
  after launch, +18% over the plain cold build (dx was +7% in Phase 1). The figure includes the
  fat link, the base L3 table and the launch: the app's logger came up 0.6 s before its first
  frame. The copy-backed app of row E, also cold: 67.4 s.
- **One target dir.** After the fat session, `build/rust` holds `debug/` (cargo's tree, 2.7 GiB)
  and `frust-hotpatch/` (574 MiB). The latter is `fat/<scope>/` (`libdeps-*.a` fat archive
  532,244,608 B, fat image 53,925,960 B, `link-args.json`) plus `session-hotapp/`
  (`layout-base.json`, `layouts-accepted.json`, `patch-N.dylib`, `stub-N.o`). There is no second
  dependency tree. `cargo build --features frust/perf-trace --features frust/devtools --features
  frust/hotpatch` straight after the fat session compiled only `hotapp` (8.1 s): the fat build's
  dependency artifacts are cargo's own. The app also writes each uploaded patch to
  `~/Library/Caches/frust-hotpatch/` and deletes it after loading.

### TUI Watch leg

Bare `frust` in a tmux session (`tmux new-session -d -s h1-11`), project `hotapp` (prepared
app), Run modal (`r`) -> `[x] desktop`, mode `debug`, Watch unchecked -> Enter. The session
started as a plain `cargo run` (PID 59667) and the counter was clicked to 2.

| Step | Toast / status (verbatim) | App | save->frame |
|---|---|---|---|
| `W` | watch indicator `⟳ watch` in the log pane's status line; tab title `▶ desktop ⟳` | 59667 | |
| save home `v1` | no toast: the watcher started on a cold session, so the save restarted it into a hot one | 59667 -> 60613 (fat image), count reset | relaunch first frame at save+6351 |
| save home `v2`, `v3`, `v4` | `✓ patched in 1932 ms`, `✓ patched in 1919 ms`, `✓ patched in 1904 ms` | 60613 kept; count 2 -> 2, `v4` | 2341 / 2351 / 2226 |
| save D2 (`HomeState` + `extra1`) | `restart required: hotapp::home_page::HomeState changed layout (4 → 8 bytes); restarting to keep memory safe` (on screen by save+1746) | 60613 -> 62805, `v1 extra1=4242` | |
| `R` (count clicked to 2 first) | `! desktop: stopped`, then a fresh session; `⟳ watch` kept | 62805 -> 63663 (fat image); count 2 -> 0 | |
| save sentinel after `R` | `✓ patched in 1940 ms` | 63663 kept | toast by save+2421 |

So `W` hot-patches and its `restart required` falls through to the relaunch, as in the CLI. `R`
is a full restart that resets State, re-fat-builds and keeps Watch on.

### Known limits seen (recorded, not fixed)

- `W` on a session launched without Watch does not hot-patch the first save: the session is not
  hot, so that save is a plain relaunch into a hot session.
- The run modal's checkbox still reads `Watch src/ and restart on change (desktop only)`.
- Killing the TUI's tmux session (SIGHUP) left the hot app (PID 63663) running; it was killed by
  hand.
- The other known H1 limits did not show up in these runs: a restart-only session's double
  launch, a Ctrl-C during the first fat build, a dropped unreferenced function (row C's new
  function is referenced), and `--features` skipping hot mode (row F uses `--no-hot` anyway).

### measure.sh changes for this card

`bash -n` and `shellcheck -S warning` are clean, and the spike gate (`cargo build && cargo clippy
--workspace --all-targets -- -D warnings && cargo fmt --check` from `examples/hotpatch-spike`)
passes. The dx and `--restart` modes are unchanged in behaviour. Additions:

- `--frust-run` / `--frust-restart` with `--app <dir>`, `FRUST`, `FRUST_RUN_ARGS`,
  `RESTART_TIMEOUT`, `APP_TARGET_DIR`. Output lines are timestamped by a small Python wrapper that
  leads the runner's process group and turns SIGTERM into the SIGINT the CLI's Ctrl-C handler
  answers. App processes are found (and, at exit, killed if they outlived the CLI) by their
  executable path under the app's target dir only.
- `--prepare-app <dir>` (the overlay above). It refuses an app inside this repository or one
  already prepared.
- New targets `stock-label`, `badge`, `either-swap` and `framework`. In frust modes, `state-type`,
  `state-field` and `return-type` are cumulative. `framework` refuses a `FRAMEWORK_FILE` inside
  this repository.
- Per-run outcome line with its save offset, old -> new PID, RSS, and a summary that splits
  patched from restart runs. Restoration is checked with `cmp` against the pristine copies (every
  session printed `same:`).

## Milestone 1 re-run (R3-04)

Card R3-04 (tsk_000001a11b196bf8lLrJux6H), run 2026-10-08 on `task/hb-r3-04`, cut from
`feature/hotpatch-h1` @ 195fc509. That tip carries the three fixes the H1-11 FAIL led to:

- R3-01: on loopback, the patch is handed off by file instead of base64 `patch_chunk`s.
- R3-02: the watch debounce drops from 300 to 100 ms in the CLI and the TUI.
- R3-03: the seam identity check runs before the layout diff, and PORT.md §7's D3 criterion is
  amended.

The machine, OS, toolchain and logged-in GUI session are the same as H1-11's. The method and
instrument are H1-11's (*Rig and method* above), unchanged except where noted below.

### Verdict: PASS

Each part of the amended bar (PORT.md §7, card R3-04):

1. **Row A median at most 50% of row F's pooled median: PASS.** Row A's median is **1397 ms**
   (n=5). Row F, the restart through `frust run --watch --no-hot`, has a pooled median of **5218
   ms** (n=10, two sessions: 4629 and 5807 ms). 1397 ms is **26.8%** of it (30.2% and 24.1% of the
   two session medians). F ran slower than in H1-11 (*F* below). Against H1-11's own F of 3671 ms,
   A is 38.1%, so the part passes even if this re-run's F is set aside.
2. **Zero `patched` for D and D2 (measured) and for D4, D5 and E (cited); D answers
   `StateTypeChanged`: PASS.** D gives `restart required` 3/3 with the `StateTypeChanged` text,
   `hotapp::home_page::HomePage's State changed type: ... ; the patch could not reach the
   component`. D2 gives `restart required` 2/2 with `LayoutChanged`. Neither prints a `patched`
   line. D4, D5 and E are cited below, each with the reason no changed code decides it.
3. **D3 is `patched` and survives: PASS.** Plain runs: `patched` 3/3, the PID is kept, count 2 is
   kept, and two presses after the last patch take it to 4. Under guard malloc: `patched` 2/2, the
   PID is kept, count 2 is kept and then pressed to 4, the `GuardMalloc[hotapp-<pid>]` banner is
   present, and there is no heap error, signal or panic.
4. **Row A against the 493 ms estimate and against H1-11's 2191 ms: recorded.**
   - Against the estimate: 1397 ms is 904 ms (2.8x) over 493 ms.
   - Against H1-11: it is 794 ms (36%) under 2191 ms.
   - Steady state: runs 2-5 have a median of 1362 ms, and G (n=10) has 1310 ms, against H1-11's
     2276 ms.

   The breakdown below accounts for the difference. The transport phase fell from 801-805 ms to
   27-36 ms. Save to build start fell from 312-324 ms to 144-148 ms. The rest is unchanged: the
   thin compile with its gates, the link and the `dlopen`.

Read against PORT.md's older figure: the 785 ms in §7's estimate paragraph is 50% of the *spike's*
1570 ms restart, not a row F. A does not pass against it (1397 > 785). The bar the card states is
row F, and the verdict above uses row F.

### Rig and method (differences from H1-11)

- **frust**: `cargo build --release -p frust-cli` in the worktree (1 m 24 s), `frust 0.6.0` from
  195fc509. Every row used this release binary: CLI, builder and TUI. The app is a debug build.
- **App**: a fresh `frust create hotapp --frust-path <worktree>` in a scratch directory outside the
  repository, then `measure.sh --prepare-app`. The cold fat session (a warm-up, not a row) reached
  its first frame 85.3 s after launch. Warm sessions reached it in 3.8-11.6 s.
- **State evidence**: the FAB is pressed through System Events by AXPress on the `Increment`
  button (`click button "Increment" of group 1 of window 1`), not by screen point. The app's window
  moved between Spaces and displays on activation, so point clicks missed in both F sessions; the
  AX press reaches the same control. As before, the first press after launch fails
  (`Invalid index`) and the count is 2 after three presses. The window was made frontmost before
  every `screencapture`.
- **Load**: no build ran in parallel with a timed session; the spike gate ran afterwards. The
  machine was not fully idle, though: a browser (Zen) was active, its GPU helper at ~25% CPU, with
  load averages of 2.9-4.3 during the sessions. One capture (D3 under guard malloc, after the
  presses) is partly covered by its window.
- **New per-run lines** from `measure.sh` (changes below):
  - `host:`, the session dir's `stub-N.o` / `patch-N.dylib` mtimes as save offsets plus the
    patch's size and mode;
  - `hand-off:`, the app's debug line, when logged.

### Matrix

| Row | Edit | Outcome lines (n) | PID / State | save->frame median (min-max), n | Verdict |
|---|---|---|---|---|---|
| A | home sentinel (prepared app) | `patched in 1152-1393 ms (1 components rebuilt)` (5/5) | 24883 kept; count 2 -> 2, `hotpatch-sentinel: v5` | **1397** (1265-1551), 5; run 1 1551 | patched; **bar PASS** |
| B | card sentinel, second child `CounterCard` | `patched in 1146-1290 ms (1 components rebuilt)` (3/3) | 26523 kept; count 2 -> 2, `card-sentinel: v3` | **1406** (1259-1559), 3 | patched |
| F | restart: `frust run --watch --no-hot`, kill + `cargo run` | n/a | new PID each run; count 0 -> 0 (clicks missed, see F) | **5218** pooled (2532-8125), 10: session 1 4629, session 2 5807 | baseline |
| D | `HomeState` -> (N+1)-tuple (type identity) | `restart required: hotapp::home_page::HomePage's State changed type: ... the patch could not reach the component` (3/3) | new PID each run | line at save+1061-1759; relaunched first frame 5631 (5337-10524), 3 | restart, **`StateTypeChanged`** |
| D2 | `HomeState` gains `extra1..extraN` | `restart required: hotapp::home_page::HomeState changed layout (4 → 8 bytes); restarting to keep memory safe`, then 8 → 12 (2/2) | new PID each run; relaunched app logs `extra1=4242` | line at save+1170-1372; relaunch 9745 (8045-11446), 2 | restart (`LayoutChanged`) |
| D3 | H0-00's edit: root `scaffold(..)` wrapped in N `stack()`s | `patched in 1170-1425 ms (1 components rebuilt)` (3/3); guard malloc `patched in 9103 / 2554 ms` (2/2) | 38600 kept, count 2 -> 2 -> 4, `v3`; guard malloc: 41267 kept, count 2 -> 2 -> 4 | 1454 (1281-1603), 3 | **patched and survives: bar PASS** |
| G | 10 consecutive home patches | `patched in 1144-1313 ms` (10/10) | 43301 kept; count 2 -> 2, `v10` | 1310 (1260-1424), 10 | no crash; RSS +15.0 MiB |
| TUI | Watch leg (below) | `✓ patched in 1122 / 1119 ms` steady; D2 `restart required` toast | kept across patches; D2 and `R` relaunch | 1238 / 1228 steady | as the CLI |
| D4, D5, E | cited (below) | | | | not re-run |

Zero `patched` lines appeared for D and D2. D3 printed three, plus two under guard malloc, which
is the amended criterion.

### A, B: patched with State kept

The CLI line and the probe lines of row A run 4, as logged (arrival stamp first):

```
1791460280261 patched in 1152 ms (1 components rebuilt)
1791460280262 [frust INFO] frust-hotpatch: applied t_unix_ms=1791460280256
1791460280262 [frust INFO] frust-hotpatch: frame t_unix_ms=1791460280258
```

- **A.** 1551 / 1328 / 1504 / 1265 / 1397 ms; PID 24883. Before: `count: 2`, `hotpatch-sentinel:
  v0`. After run 5: `count: 2`, `v5`, `card-sentinel: v0`. RSS 169.2 -> 172.6 MiB.
  - **First patch of the session.** Run 1 is 1551 ms, against a steady median (runs 2-5) of
    1362 ms. B's run 1 is 1559 ms against 1259-1406. G's run 1 (1324 ms) shows no penalty. The
    diagnostic session's run 1 (1915 ms) puts the extra before the build again: save to build
    start is 498 ms there, against 144-148 ms steady.
- **B.** 1559 / 1406 / 1259 ms; PID 26523. After: `count: 2`, `card-sentinel: v3`,
  `hotpatch-sentinel: v0`.
- **Patch image**: 3,060,304 bytes for a sentinel edit (H1-11: 3,017,952) and 3,106,096 bytes for
  D3. Every `patch-N.dylib` in the session dir is `-rw-------` (mode 600), as R3-01 requires.

### Latency breakdown (why 1.4 s, not 493 ms, and not 2.2 s)

The breakdown comes from a diagnostic session of 4 runs, `--target home` (1915 / 1348 / 1264 /
1394 ms; steady median 1348). It uses three instruments:

- **Host files.** `measure.sh`'s `host:` line gives the mtimes of the session dir's `stub-N.o`
  ("stub written") and `patch-N.dylib` ("patch linked").
- **App files.** H1-11's 2 ms poller on the app's cache dir (`~/Library/Caches/frust-hotpatch/`)
  gives the mtime of the app's own copy ("copy written") and the moment the app removes it.
- **Build start (new).** A ~10 ms `ps` poller records when the thin `rustc --crate-name hotapp`
  for the tip lib first appears. Each `ps` call takes 20-30 ms, so this start is late by up to
  ~30 ms. H1-11 did not record how it timed `on_change`, so treat the comparison in this row as
  approximate.

| Phase | H1-11 (ms, steady) | R3-04 (ms, runs 2-4; run 1) |
|---|---|---|
| save -> thin `rustc` of the tip lib starts (debounce + file event) | 312-324 | **144-148**; 498 |
| build start -> `stub-N.o` written (thin compile with three crate types, L3 DWARF read + diff, seam check, symbol work) | 640-900 | 684-819; 940 |
| `stub-N.o` -> `patch-N.dylib` linked | 75-81 | 81-97; 118 |
| patch linked -> app's copy written (H1-11: `patch_chunk` upload as base64; now the hand-off: open, five checks, SHA-256, own `0600` copy) | **801-805** | **27-36**; 34 |
| app's copy written -> removed = `applied` (`dlopen`, table install) | 335-367 | 316-332; 324 |
| `applied` -> `frame` | 0-2 | 1; 1 |

**Transport.** The phase from patch linked to the app's copy fell from ~800 ms to ~30 ms. What is
left is the app reading the 3 MB file the host wrote, checking it and writing its own copy. Its
time to `applied` (316-332 ms) is the same load cost as before.

**Evidence that `apply_patch` carried `file`**: the app's own log, plus the timing.

- A separate session ran with `FRUST_RUN_ARGS="--define FRUST_LOG=debug"` (2 runs, not latency
  data: the debug log is ~155,000 lines). Each patch logged, verbatim:

  ```
  [frust DEBUG] frust-devtools: patch file handed off (3060304 bytes, checks passed)
  [frust DEBUG] frust-devtools: applying patch 1 (3060304 bytes, 3 expected seams)
  ```

  `measure.sh` reported `hand-off: file` for both runs. That line comes from
  `ShellBackend::patch_file`, which runs only for an `apply_patch` that names a file.
- The 27-36 ms phase above cannot hold a 3 MB base64 upload (H1-11: ~800 ms).
- The absence of `patch_chunk` in the log is *not* evidence: the app does not log chunk receipt
  at any level, and the CLI prints no RPC trace.

**Against the estimate.** The ~900 ms between A (1397 ms) and the 493 ms estimate are mostly two
things the estimate treated as zero:

- the build-and-gate phase, 684-819 ms. PORT.md §7 assumed the tip compile would be roughly
  "used up" by the new gates.
- the load, 316-332 ms. H1-11's standalone `dlopen` of a fresh ad-hoc-signed copy costs 111-329
  ms, so most of it is the first-map signature check.

Debounce plus file event (~145 ms) and the link (~85 ms) make up most of the rest.

**Against H1-11.** In steady state the fixes remove ~770 ms of transport and ~170 ms of debounce,
~940 ms together. G's median fell by 966 ms (2276 -> 1310) and A's by 794 ms (2191 -> 1397; A's
median includes a slower run 1). That matches the breakdown.

### D: State type identity answers `StateTypeChanged`

All three runs (tuple of 2, 3 and 4 `u32`) answered with R3-03's reason. Run 1, verbatim:

```
restart required: hotapp::home_page::HomePage's State changed type: (&hotapp::home_page::HomePage, &mut hotapp::home_page::HomeState, frust_core::hotpatch::SeamWitness) → (&hotapp::home_page::HomePage, &mut (u32, u32), frust_core::hotpatch::SeamWitness); the patch could not reach the component
```

Runs 2 and 3 print `(u32, u32) → (u32, u32, u32)` and `(u32, u32, u32) → (u32, u32, u32, u32)`.

- PIDs: 33478 -> 34338 -> 34910 -> 35461.
- Count: 2 before run 1 and 0 after it (relaunched), with `hotpatch-sentinel: v1`.
- The outcome line arrives at save+1061-1759 ms. Nothing is sent to the old process.

### D2: refused by L3

`restart required: hotapp::home_page::HomeState changed layout (4 → 8 bytes); restarting to keep
memory safe`, then `(8 → 12 bytes)`. PIDs 36191 -> 37107 -> 37765. The relaunched app reads the new
field: `frust-hotpatch-spike: state-field v1 build ran extra1=4242`.

### D3: patched and survives (amended criterion)

- **Plain** (PID 38600): `patched in 1425 / 1170 / 1340 ms (1 components rebuilt)`, save->frame
  1603 / 1281 / 1454 ms.
  - Before the patches: `count: 2`, `hotpatch-sentinel: v0`. After patch 3: `count: 2`, `v3`.
  - Two presses after the last patch: `count: 4`. The patched tree is live and interactive.
- **Guard malloc** (PID 41267, banner `GuardMalloc[hotapp-41267]: Allocations will be placed on
  16 byte boundaries.`): `patched in 9103 / 2554 ms`.
  - Count: 2 before, 2 after patch 2 (`v2`), then 4 after two presses.
  - No `GuardMalloc` error, signal, panic or abort appears in the log.
  - The CLI's build and link children printed their own banner (`GuardMalloc[frust-<pid>]`), as in
    H1-11.
  - Timings under guard malloc (first frame 22.2 s, RSS 783-847 MiB) are not latency data.

### F: restart baseline

`--no-hot` relaunch loop, home sentinel edits:

- **Session 1**: 4629 / 8125 / 2532 / 4197 / 5885 ms (median 4629). PIDs 27312 -> 27882 -> 28263 ->
  28864 -> 29120 -> 29489.
- **Session 2**: 5807 / 7300 / 4251 / 2699 / 8040 ms (median 5807). PIDs 30083 -> 30510 -> 30914 ->
  31467 -> 31836 -> 32109.

cargo's `Finished` per relaunch is 1.57-7.17 s (H1-11: 1.4-3.5 s), and it dominates the spread.
F also gains R3-02's shorter debounce (300 -> 100 ms in the relaunch loop). The slower F is a
rig condition (see *Load* above), not a code change.

**State.** The State reset is shown by the new PID, not by a capture. In both F sessions the
point clicks missed (the window moved between Spaces), so `count: 0` stands before and after.
After run 5 the window shows `hotpatch-sentinel: v5` in a new PID.

### G: ten patches, RSS and the budget counters

PID 43301, count 2 kept, `hotpatch-sentinel: v10` after run 10. Per-run save->frame: 1324 / 1415 /
1274 / 1424 / 1297 / 1406 / 1280 / 1260 / 1355 / 1297 ms. No crash and no trend. Each of the ten
patches went through the hand-off.

RSS (MiB):

| Point | RSS |
|---|---|
| after the presses, before patch 1 (settled) | 155.3 |
| after patch 1 | 161.5 |
| after patches 2-10 | 163.0, 163.8, 164.7, 165.5, 166.6, 167.4, 168.2, 169.3, 170.3 |
| 15 s after patch 10 | 170.1 (flat) |

The first patch costs +6.2 MiB (H1-11: +13.7), every later one about +0.9 MiB, +15.0 MiB over 10
patches. The app now keeps no base64 decode buffers, since there are no chunks, which plausibly
accounts for the smaller first step. That is not separately measured.

**Budget counters.** The CLI still prints no counters on a `patched` outcome, and R3-01 adds none.
The session dir holds ten 3,060,304-byte images, so the app's counters are 10 / 30,603,040 bytes
(H1-11 showed `patch_bytes_loaded` equal to the images' sum). At this size the default 96 MiB byte
budget (100,663,296) lets 32 patches through and refuses the 33rd.

### TUI Watch leg

Bare `frust` in a tmux session, project `hotapp` (prepared app). Run modal (`r`) -> `[x] desktop`,
mode `debug`, Watch unchecked -> Enter, as in H1-11, so the first-save behaviour is compared like
for like. The session started as a plain `cargo run` (PID 50525) and the counter was pressed to 2.

| Step | Toast / status (verbatim) | App | save->frame |
|---|---|---|---|
| `W` | `⟳ watch` in the log pane's status line | 50525 | |
| save home `v1` | no toast: the save restarted the cold session into a hot one | 50525 -> 50980 (fat image), count reset | relaunch first frame at save+5896 |
| save home `v2`, `v3`, `v4` | `✓ patched in 2386 ms`, `✓ patched in 1122 ms`, `✓ patched in 1119 ms` | 50980 kept; count 2 -> 2, `v4` | 2906 / 1238 / 1228 |
| save D2 (`HomeState` + `extra1`) | `restart required: hotapp::home_page::HomeState changed layout (4 → 8 bytes); restarting to keep memory safe` (on screen by save+1668) | 50980 -> 51958, `v5 extra1=4242`, count 0 | relaunch first frame at save+8237 |
| `R` (count pressed to 2 first) | `⟳ watch` kept (the stop toast was not captured) | 51958 -> 52287 (fat image); count 2 -> 0 | |
| save sentinel after `R` | `✓ patched in 1146 ms` (toast by save+1414) | 52287 kept | 1315 |

The first-save relaunch after `W` (H1-11 known limit) is **unchanged**. `v2` is then the hot
session's first patch (2906 ms). Steady TUI patches (1228-1315 ms) match the CLI rows.

### Cited rows: D4, D5, E (not re-run)

None of the three fixes changes the code that decides these rows:

- R3-01 changes only what happens *after* the gates pass: how a patch reaches the app.
- R3-02 changes only the debounce before `on_change`.
- R3-03 reorders two checks in `AcceptedSets::check` that both refuse. Reordering them can change
  which reason a refusal names, but never turns a refusal into `patched`. Its `layout.rs` changes
  are tests only.

- **D4** (patch 2 grows `Badge<k>` after patch 1 added it). The layout diff and the accepted-set
  merge are unchanged, and no State identity changes, so the seam check passes and L3 refuses
  patch 2 with `LayoutChanged` exactly as in H1-11.
- **D5** (`HomeState` gains a field and the `either(..)` arm flips). `HomeState` keeps its identity
  as `HomePage`'s State, so the new first check passes and the unchanged layout diff refuses it
  (`LayoutChanged`) before anything is sent.
- **E** (an edit in the frust path dependency). It is refused by path class
  (`PathDependencyChanged`) before any compile or gate runs, and no fix touched that
  classification.

### Known limits seen (recorded, not fixed)

- `W` on a session launched without Watch still makes the first save a plain relaunch into a hot
  session (TUI leg).
- The run modal's checkbox still reads `Watch src/ and restart on change (desktop only)`.
- Killing the TUI's tmux session (SIGHUP) still left the hot app (PID 52287) running; it was
  killed by hand.
- Not seen in these runs: a restart-only session's double launch, a Ctrl-C during the first fat
  build, and `--features` skipping hot mode (row F uses `--no-hot`).
- Rig, not frust: the app's window can move between Spaces and displays when activated, so a
  point click can land on another app. AXPress on the `Increment` button avoids that.

### measure.sh changes for this card

`bash -n` and `shellcheck -S warning` are clean. The spike gate (`cargo build && cargo clippy
--workspace --all-targets -- -D warnings && cargo fmt --check` from `examples/hotpatch-spike`)
passes. Every existing mode behaves as before; the additions only print more:

- `--row <label>` names the row on the header and summary lines.
- `--frust-run` prints two new lines per patched run:
  - `host:`, the save offsets of the session dir's newest `stub-N.o` and `patch-N.<ext>`, with the
    patch's size and mode;
  - `hand-off:`, whether the app logged `frust-devtools: patch file handed off (<len> bytes,
    checks passed)`. The line is debug level, so it appears only with
    `--define FRUST_LOG=debug`; otherwise the run says it was not logged.
- The `--frust-run` summary adds the patched median over runs 2..n, so a session's first patch is
  reported on its own.

## Stage 2: Pixel 5 gate (H2-04)

Card H2-04 (tsk_000001a1181b5c74iWmqcYvY), run 2026-10-09 on `task/hb-h2-04`, cut from
`feature/hotpatch-h2` @ 2a592437: H1 merged, plus H2-01 (Android fat build outside Gradle), H2-02
(the Android shell's `hotpatch` feature) and H2-03 (`frust run -d <android> --watch`). Every row
runs `frust run -d <adb-serial> --watch` on a physical Pixel 5. The arm64 emulator is not used.

### Verdict: FAIL

1. **Median save->frame at most 50% of the in-session Android restart median: FAIL.** The restart
   median is **15338 ms** (row D2's five `restart required` reruns: full build -> install ->
   launch -> first frame). The bar is therefore 7669 ms. The patched rows land at:
   - save->applied: A **11966 ms** (n=5), B **12056 ms** (n=5), G **11923 ms** (n=10). That is
     78.0%, 78.6% and 77.7% of the restart median. The patched frame follows on the next
     Choreographer tick (*Rig and method*).
   - save->CLI `patched` line: A 16987 ms, B 17077 ms, G 16937 ms. That is 110.8% of the restart
     median.

   Against the pooled restart median of D2 and E (14439 ms, n=9), A is 82.9%. No reading of the
   numbers passes. Two costs no desktop row had dominate (*Latency breakdown*): ~10.4 s to upload
   the 13.1 MB patch in chunks, and a 5 s answer delay on every apply.
2. **Zero `patched` lines for D2 and E: PASS.** D2 answers `restart required` 5/5 with
   `LayoutChanged`, E 4/4 with the path-dependency reason. No `patched` line appears for either.
3. **PID unchanged across A, B and G: PASS.** A: 15386 throughout, B: 16573, G: 18461 (10/10). In
   each, count 3 survives every patch.
4. **Relocation against the base cdylib: proven.** A patched `HomePage::build` calls the unchanged
   base `frust_widgets::text::text::<&str>` through a stub thunk. The thunk holds the address of the
   symbol in the fat image plus the slide that `__frust_hotpatch_anchor` gives. All 778 thunks of
   the stub land inside the base library's `r-xp` mapping (*Relocation evidence*).
5. **Device re-locked at the end: done.** `svc power stayon false`, then `input keyevent 26`. After
   that, `isKeyguardShowing=true` and `mWakefulness=Dozing`: the always-on display, not `Asleep`.
   The test app was uninstalled; it was not installed before the gate.

What holds: the first Android patch linked against a running base cdylib relocates and runs, in
one process, ten times in a row. State survives every patch. The layout and path-class refusals
behave as on the desktop. What fails is the bar: on this rig, a patch takes ~78% of the restart
(applied) or ~111% (CLI line), not 50%.

### Environment

| | |
|---|---|
| Device | Pixel 5 (`redfin`), USB serial <adb-serial> |
| Android | 14 (SDK 34) |
| `ro.build.fingerprint` | `google/redfin/redfin:14/UP1A.231105.001.B2/11260668:user/release-keys` |
| App | `dev.frust.gate.hotapp`, debug, arm64-v8a, `untrusted_app` |
| Host | `Mac16,1`, Apple M4, macOS 27.0.1, rustc/cargo 1.98.1, NDK 28.2.13676358, cargo-ndk 4.1.2, Gradle 9.5.1, JDK 21.0.8 (Android Studio JBR) |
| Clock | device - host = 1158-1163 ms, measured before every run (min-RTT `adb shell echo $EPOCHREALTIME`, RTT 25-31 ms, so the error is at most ~15 ms) |

### Rig and method

- **frust**: `cargo build --release -p frust-cli` in the worktree (1 m 39 s), `frust 0.6.0`. That
  binary ran every row: CLI, builder (`RUSTC_WORKSPACE_WRAPPER`, linker proxy) and thin links. The
  app is a debug arm64-v8a build, which `frust run -d <adb-serial> --watch` makes itself (fat build
  outside Gradle, `assembleDebug -x cargoNdkBuild`, install, launch, `adb forward`).
- **App**: `frust create hotapp --frust-path <worktree>/crates/frust --org dev.frust.gate
  --platforms android` into `<scratch>`, then `measure.sh --prepare-app` (H1-11's overlay:
  `HomeState { count }`, `CounterCard`, the markers, the root `either(..)`). The cold first session
  built in 113.4 s and installed in 3.7 s. Warm session starts reached their first frame 11.1-19.1 s
  after launch.
- **Runs**: `measure.sh --frust-run --app <scratch>/hotapp --android --target <t> --runs <n> --row
  <r>`, serial from `ANDROID_SERIAL`, `FRUST=<worktree>/target/release/frust`, `PRE_RUN_HOOK` = three
  `input tap`s on the FAB (count 0 -> 3), `SCREENCAP_DIR` in `<scratch>`. Each row is its own
  `frust run` session.
- **Instruments.** The Android tree has no frame probe: `frust-hotpatch: applied/frame
  t_unix_ms=` is logged only by `frust-shell-desktop`. The Android shell also never calls
  `devtools::frame_submitted` (`frust-shell-common/src/devtools.rs`: "Only the desktop shell calls
  `frame_submitted` today"). So every `apply_patch` parks for its frame, waits out
  `UI_HOP_DEADLINE` (5 s), logs `frust-devtools: no frame followed the patch within 5s; answering
  anyway`, and only then answers. That line appeared exactly once per patch (A 5, B 5, G 10, in each
  patched PID). The script reports:
  - **save->applied**: the backstop line's device time minus 5000 ms, moved onto the host clock.
    That is the moment the patch was loaded and its table installed (the attempt runs before the
    park). The patch listener sets the frame latch at install, so the patched rebuild runs on the
    next Choreographer tick. This is the closest Android equivalent of the desktop's save->frame.
    It understates the frame by up to one vsync and is not a direct probe.
  - **save->CLI line**: the arrival of `patched in N ms` on the host. One clock, an upper bound,
    and 5 s late by construction.
  - **restart save->frame**: the relaunched activity's `ActivityTaskManager: Displayed
    dev.frust.gate.hotapp/.MainActivity` line, device time moved onto the host clock. In the first
    session it came 2 ms after `frust-render tier=engine`, the renderer's creation for the first
    frame.
- **Device evidence per run**: PID (`pidof`), RSS (`VmRSS` of `/proc/<pid>/status`),
  `/proc/<pid>/maps` through `run-as`, the app's own `hotpatch_info` over a private `adb forward`
  (counters and `anchor_runtime`), the run's `stub-N.o`, and an `adb exec-out screencap -p`. The
  device log was captured with `adb logcat -v epoch` from the session start, bounded by the device
  clock and never cleared, then grepped for `avc: denied` and read from the crash buffer.
- **Load**: no build ran in parallel with a timed session. The host was not idle: a browser
  process (Zen `plugin-container`) at ~90% CPU, load averages 3.0-4.5.
- **Row G's budget**: the first G attempt hit the default budget at the 8th patch (G below). The
  timed G ran with `[hotpatch] bytes = 268435456` in the scratch app's `frust.toml`, which was
  restored afterwards.

### Matrix

| Row | Edit | Outcome lines (n) | PID / State | save->applied median (min-max), n | save->CLI line median | Verdict |
|---|---|---|---|---|---|---|
| A | home sentinel | `patched in 16453-19577 ms (1 components rebuilt)` (5/5) | 15386 kept; count 3 -> 3, `hotpatch-sentinel: v5` | **11966** (11621-15004), 5 | 16987 | patched; **bar FAIL** |
| B | card sentinel, `CounterCard` in its own module | `patched in 16448-16738 ms (1 components rebuilt)` (5/5) | 16573 kept; count 3 -> 3, `card-sentinel: v5` | **12056** (11665-12091), 5 | 17077 | patched; **bar FAIL** |
| D2 | `HomeState` gains `extra1..extraN` | `restart required: hotapp::home_page::HomeState changed layout (4 → 8 bytes); restarting to keep memory safe`, then 8 → 12 ... 20 → 24 (5/5) | new PID each run; relaunch shows `v5 extra1=4242`, count 0 | relaunch first frame **15338** (13432-16781), 5 | outcome line at save+1488-2415 | restart (`LayoutChanged`); **the restart baseline** |
| E | `PAD_X` 12 -> 48 -> 12 -> 48 -> 12 in `crates/frust-widgets/src/button.rs` | `` restart required: `<worktree>/crates/frust-widgets/src/button.rs` belongs to the path dependency `frust-widgets`, which a patch cannot replay `` (4/4) | new PID each run | relaunch first frame 13614 (12997-16163), 4 | outcome line at save+116-850 | restart (`PathDependencyChanged`) |
| G | 10 home patches, byte budget 256 MiB | `patched in 16447-18012 ms (1 components rebuilt)` (10/10) | 18461 kept; count 3 -> 3, `v10` | **11923** (11735-13245), 10 | 16937 | no crash; RSS +53.5 MiB; counters below |
| G (default budget) | same, default budget | 7 × `patched`, then `restart required: the patch budget is spent (8 patches / 105195008 bytes loaded would pass 64 patches or 100663296 bytes)` | 17160 kept for 7 patches | 11603-12353 for patches 1-7 | | budget binds at the 8th patch |

Zero `patched` lines appeared for D2 and E.

### A, B: patched with State kept

Row A run 1 as logged (CLI stream, arrival stamp first; the app's logcat line is device time):

```
1791496525030 patched in 19577 ms (1 components rebuilt)
```

The app's own line for the same patch, in the device log (`-v epoch`):

```
1791496526.171 15386 15593 W frust   : frust_shell_common::devtools: frust-devtools: no frame followed the patch within 5s; answering anyway
```

On the host clock (offset 1160 ms) the backstop line is at 1791496525011, so the patch was applied
at 1791496520011 (save+15004 ms). The CLI line arrived 19 ms after the backstop line.

- **A.** save->applied 15004 / 11966 / 11634 / 12368 / 11621 ms; CLI line 20023 / 16987 / 16651 /
  17386 / 16637 ms. PID 15386 throughout. Before: `count: 3`, `hotpatch-sentinel: v0`. After run
  5: `count: 3`, `v5`, `card-sentinel: v0`. RSS 161.4 -> 194.6 MiB.
- **B.** save->applied 11830 / 12056 / 11665 / 12060 / 12091 ms; CLI line 16849 / 17077 / 16682 /
  17080 / 17109 ms. PID 16573. After: `count: 3`, `card-sentinel: v5`, `hotpatch-sentinel: v0`.
  RSS 153.2 -> 183.0 MiB.
- **First patch.** A's run 1 is 15004 ms against 11621-12368 later. Its stub was written at
  save+4465 ms against 1106-1882 ms later. B's and G's first runs show no comparable penalty
  (11830, 13245).
- **Patch image**: 13,149,376 bytes for a sentinel edit, 4.3x the 3,060,304 bytes of the macOS
  rows. Every `patch-N.so` in the session dir (`session-hotapp-aarch64-linux-android/`) is mode
  600.
- **Maps.** After patch k the process maps four `/memfd:frust-hotpatch (deleted)` segments per
  patch, one of them `r-xp`. Row A after patch 1:

  ```
  7174079000-71741d8000 r--p 00000000 00:01 5857246                        /memfd:frust-hotpatch (deleted)
  71741db000-7174294000 r-xp 0015e000 00:01 5857246                        /memfd:frust-hotpatch (deleted)
  7174297000-71742a3000 r--p 00216000 00:01 5857246                        /memfd:frust-hotpatch (deleted)
  71742a6000-71742a7000 rw-p 00221000 00:01 5857246                        /memfd:frust-hotpatch (deleted)
  ```

  After A's run 5: 20 mappings, 5 `r-xp`. After G's run 10: 40 and 10.

### Relocation evidence

Row A, PID 15386, patch 1 (the first patch of this stage linked against a running base cdylib):

- **seam_hits**: the CLI's `patched in 19577 ms (1 components rebuilt)`. The count is the
  outcome's `seam_hits` (`Outcome::Patched { components: outcome.seam_hits }`), so the patched
  seam was dispatched once. A zero would have been `restart required` (`NoSeamHit`).
- **The slide.** The app reports `anchor_runtime=0x71869f62f8` (`hotpatch_info`). In the fat image
  (`<target>/frust-hotpatch/fat/<scope>/libhotapp.so`, unstripped), `__frust_hotpatch_anchor` is
  at `0xba22f8`. The slide is therefore `0x7185e54000`, which is exactly where the library's first
  mapping starts. The base library is mapped straight out of the APK:

  ```
  7185e54000-7186994000 r--p 00204000 fd:2d 25358                          /data/app/.../base.apk
  7186997000-71878d8000 r-xp 00d43000 fd:2d 25358                          /data/app/.../base.apk
  ```

- **The call.** In `patch-1.so`, `<hotapp::home_page::HomePage as Component>::build` calls the
  stub's thunk for the unchanged base helper:

  ```
  16a95c:   bl  0x207fb8 <frust_widgets::text::text::<&str>>
  207fb8:   ldr x16, 0x207fc0
  207fbc:   br  x16
  ```

  The literal at `0x207fc0` (from `stub-1.o`, offset `0x168`) is `0x7187258c30`. That equals the
  fat image's `frust_widgets::text::text::<&str>` at `0x1404c30` plus the slide, and lies inside
  the base `r-xp` mapping above. The patched frame shows `hotpatch-sentinel: v1`, the text that
  call built.
- **Every thunk.** `stub-1.o` has 778 text thunks (`ldr x16, #8; br x16; .quad <addr>`). Checked
  against the fat image's symbol table and the run's maps: 778/778 equal the fat link address plus
  the slide, and 778/778 lie inside the base library's `r-xp` mapping. A second process (a sanity
  run after the matrix, PID 22337, a different install) has slide `0x7185085000` and also 778/778.
  The stub holds 13 further defined symbols (data/absolute). They were not checked one by one.
- **SELinux.** Over every session of the gate, 0 `avc: denied` lines had `scontext` `untrusted_app`.
  The ones logged came from `system_server` and `hal_sensors_default`. The crash buffer stayed
  empty in every session.

So all three Android-only links of PORT.md §7 hold on this device: the anchor rebase in a
JNI-loaded cdylib, undefined symbols resolved through thunks to slid base addresses, and the memfd
load.

### Latency breakdown (why ~12 s to applied and ~17 s to the CLI line)

Host-side mtimes from the `host:` line, plus the backstop-derived apply moment. Steady-state runs
(A 2-5, B 1-5, G attempt 1's 1-7):

| Phase | ms |
|---|---|
| save -> `stub-N.o` written (debounce, thin compile for aarch64, L3 DWARF diff, seam check, stub) | 1106-1882 |
| `stub-N.o` -> `patch-N.so` linked (NDK clang) | 56-64 |
| patch linked -> applied (13,149,376 bytes as base64 `patch_chunk`s over `adb forward`, then the memfd write, `android_dlopen_ext`, table install) | **10358-10499** |
| applied -> the app answers (`UI_HOP_DEADLINE` backstop) | **5016-5021** |

The transport and the load were not separated. The app writes, loads and deletes its copy inside
the app's cache dir, and polling that through `run-as` would perturb the run. At ~10.4 s for
13.1 MB the phase runs at ~1.26 MB/s, and it costs more than the restart's whole build step.
Without the 5 s backstop, the CLI line would still sit ~5 s past `applied` on every patch.

### D2: refused by L3; the restart baseline

Run 1, verbatim: `restart required: hotapp::home_page::HomeState changed layout (4 → 8 bytes);
restarting to keep memory safe`. Runs 2-5 print 8 → 12, 12 → 16, 16 → 20, 20 → 24.

- PIDs 19300 -> 19492 -> 19709 -> 19872 -> 20013 -> 20161.
- Each relaunched app reads the new field: the device log holds `frust-hotpatch-spike: state-field
  vN build ran extra1=4242` for v1..v5. The screen after run 5 shows `hotpatch-sentinel: v5
  extra1=4242`, `count: 0`.
- save->first frame per run: 14439 / 13432 / 16495 / 15338 / 16781 ms, median **15338**.
- Per relaunch: outcome line at save+1488-2415 ms, `Build finished in` 7.9-11.6 s (fat build +
  Gradle), `Installed in` 2.9-3.1 s, then launch -> `Displayed` in ~0.5 s.

This is the hot session's own restart, which a hazard edit costs. A `--no-hot` relaunch row was not
measured: `measure.sh --frust-restart` passes `--features`, which `--watch -d` refuses, and that row
is not needed for the bar.

### E: framework edit

Four edits to the worktree's `crates/frust-widgets/src/button.rs`: `const PAD_X: f64 = 12.0;` ->
`48.0` -> `12.0` -> `48.0` -> `12.0`. They were made by a scratch driver around its own `frust run
-d <adb-serial> --watch` session, so `measure.sh` was not involved. Each answered, verbatim apart
from the path:

```
restart required: `<worktree>/crates/frust-widgets/src/button.rs` belongs to the path dependency `frust-widgets`, which a patch cannot replay
```

- Outcome lines arrived at save+116 / 269 / 850 / 271 ms.
- The relaunch re-fat-built the crate and its dependents: `Build finished in` 9.0-11.6 s.
- First frames at save+12997 / 14202 / 16163 / 13026 ms. PIDs 20334 -> 20470 -> 20598 -> 20731 ->
  20849.
- The file ended byte-identical to its original, and `git status` of the worktree was clean
  afterwards. That restore was itself a content-identical write. It drew a fifth `restart
  required` with the same text: the path class refuses on the event, without comparing content.

### G: ten patches, RSS and the budget counters

**Default budget (first attempt).** PID 17160 took 7 patches (save->applied 11603-12353 ms). The
8th save answered, at save+1305 ms:

```
restart required: the patch budget is spent (8 patches / 105195008 bytes loaded would pass 64 patches or 100663296 bytes)
```

At 13,149,376 bytes per sentinel patch, the default 96 MiB byte budget admits 7 patches on this
app (desktop: 32). The session then relaunched as a restart should. Its next start failed for a
rig reason (*Known limits*), so this attempt is not latency data past patch 7.

**Timed G** (byte budget 256 MiB): PID 18461, count 3 kept, `hotpatch-sentinel: v10` on screen
after run 10. save->applied 13245 / 12320 / 11748 / 11917 / 11752 / 12149 / 12029 / 11862 / 11929 /
11735 ms (median 11923; runs 2-10: 11917). CLI line median 16937. No crash, no trend.

| After | `patches_applied` | `patch_bytes_loaded` | memfd mappings (`r-xp`) | RSS (MiB) |
|---|---|---|---|---|
| presses, before patch 1 | 0 | 0 | 0 (0) | 152.1 |
| patch 1 | 1 | 13149376 | 4 (1) | 167.1 |
| patch 2 | 2 | 26298752 | 8 (2) | 168.2 |
| patch 3 | 3 | 39448128 | 12 (3) | 172.8 |
| patch 4 | 4 | 52597504 | 16 (4) | 177.6 |
| patch 5 | 5 | 65746880 | 20 (5) | 182.2 |
| patch 6 | 6 | 78896256 | 24 (6) | 186.9 |
| patch 7 | 7 | 92045632 | 28 (7) | 191.3 |
| patch 8 | 8 | 105195008 | 32 (8) | 196.0 |
| patch 9 | 9 | 118344384 | 36 (9) | 200.9 |
| patch 10 | 10 | 131493760 | 40 (10) | 205.6 |

The counters are the app's own `hotpatch_info` answers, read after each outcome.
`patch_bytes_loaded` is exactly k × 13,149,376. RSS grows +15.0 MiB at the first patch and
+1.1-4.9 MiB at each later one, +53.5 MiB over 10. On the desktop (R3-04) each later patch cost
~0.9 MiB.

### Known limits seen (recorded, not fixed)

- **No frame answer on Android.** The Android shell does not call `devtools::frame_submitted`, so
  every apply is answered by the 5 s `UI_HOP_DEADLINE` backstop with a warning. This adds 5 s to
  every `patched` line, and the outcome's seam hits come from whatever ran in those 5 s, not
  strictly from "the frame after the apply".
- **No frame probe on Android.** `frust-hotpatch: applied/frame` exist only in
  `frust-shell-desktop`, so save->frame cannot be measured directly on a device.
- **Transport.** Chunked base64 upload of a 13.1 MB patch takes ~10.4 s (~1.26 MB/s) over `adb
  forward`, the largest single cost. (`adb install` of the whole APK takes ~3 s.)
- **Patch size and budget.** Patch images are 13.1 MB (4.3x macOS). The default byte budget
  refuses the 8th patch.
- **A dead hot app goes unnoticed (act_000001a11d6d27b655fJfemp).** `am force-stop` of the watched
  app (a clean session, PID 21721): `pidof` was empty at once, and the CLI printed nothing in the
  next 30 s. The `adb logcat --pid` stream does not end when the process dies. The next save (a
  sentinel edit) answered at save+14151 ms with:

  ```
  restart required: the devtools connection failed: timed out waiting for a `hotpatch_info` response after 10s
  ```

  The full pipeline then ran and relaunched PID 22130 (first frame at save+32479 ms). So a dead app
  is noticed only by the next save, which pays a 10 s timeout before the restart.
- **Rig incidents, not frust.**
  - After G's default-budget relaunch the device vanished from `adb devices`, though it was still
    on USB and not rebooted (uptime 11 days). The new app's discovery line never arrived (`restart
    required: hot-patch builder unsupported: the app announced no devtools endpoint within 60s;
    hot reload unavailable, relaunching on change instead`), and later pipeline runs failed with
    `adb install failed: ... not found`. Restarting the host adb server brought the device back.
    Separately, `measure.sh` leaked its logcat stream at exit; that is fixed (below).
  - Google Play Protect held one `adb install` with "Send app for a security check?" until the
    dialog was dismissed by hand with "Don't send" (`Installed in 628.5s`, the dead-app session's
    first start). Another install took 21.0 s. Every other install took 2.9-3.7 s. No timed row
    includes either.
- Not exercised: the TUI (not required by this card) and a Profile build (refused for `--watch -d`
  by design).

### measure.sh changes for this card

`bash -n` and `shellcheck -S warning` are clean. The desktop and dx modes behave as before. New:

- `--serial <adb-serial>` / `-d <adb-serial>` / `--android` (serial from `ANDROID_SERIAL`) on
  `--frust-run` runs `frust run --watch -d <serial>`. The serial is never written outside the log
  dir. The script refuses an asleep device and refuses `--frust-restart` with a serial.
  `ANDROID_PACKAGE` overrides the `applicationId` read from `android/app/build.gradle.kts`.
- Android instruments: `applied` from the 5 s backstop line (device time - 5000 ms) and
  `Displayed` for first frames. Device times are moved onto the host clock by an offset measured
  before every run (`clock:` line). Summary lines give save->applied and save->CLI line for patched
  runs.
- Device log: `adb logcat -v epoch` from the session start into `logcat.txt`. At exit it reports
  `avc: denied` lines (split by `untrusted_app`) and the crash buffer. That read is skipped when
  the device is gone, so the exit no longer hangs.
- Per run: PID (`pidof`), RSS (`VmRSS`), `maps:` (memfd mappings, saved as `maps-run-N.txt`),
  `info:` (the app's `hotpatch_info` over a private `adb forward`; `HOTPATCH_INFO=0` skips it), a
  copy of `stub-N.o`, and `SCREENCAP_DIR` captures (`before.png`, `run-N.png`; outside the
  repository only). An app still running after the CLI exits is `am force-stop`ped.
- `PRE_RUN_HOOK=<cmd>` (desktop and Android) runs after the first frame with the app PID.
- Fixes found on the way:
  - `grep -q` behind a pipe under `pipefail` failed the discovery wait at random (SIGPIPE).
  - The device log stream outlived the script, because `$!` named a function's subshell rather
    than `adb`.
