#!/usr/bin/env bash
# benchmarks/harness/ab_matrix.sh — render-arm A/B device gate, one command:
# drives one cell per render arm across benchmarks/frust_bench's frame
# scenarios AND examples/material3-demo's push/pop nav column, and emits the
# table benchmarks/RESULTS.md needs.
#
# The FRUST_AA_MODE x FRUST_RENDER_SCALE matrix this script was built around
# is GONE: both knobs were vello-classic measurement instruments, and they
# were deleted with vello itself when the engine tier became the only
# renderer. What survives is the `--tier` axis plus the kept-run accounting,
# sanitized capture, raw-copy and device wake/lock machinery below.
#
# For every cell in the matrix, in this order:
#
#   1. Build benchmarks/frust_bench (from its own directory — it is a
#      standalone workspace, see the repo root CLAUDE.md):
#        <frust> build apk --profile --define FRUST_TRACE_RAW=1 <arm args>
#      `<frust>` is the frust CLI binary — `target/debug/frust` at the repo
#      root, built by `cargo build -p frust-cli` there (this script never
#      builds the CLI itself; see `--frust` below).
#   2. `adb install -r` the resulting profile APK — `run.sh` (below) never
#      installs anything itself, so every scenario batch needs a fresh
#      install first.
#   3. For each scenario in `--scenarios`, drive it via
#      `benchmarks/harness/run.sh <scenario> --app frust --device <serial>
#      --runs <n> --duration <d> --skip-device-state --out <dir>` and parse
#      its printed `p50=`/`p95=` line (stats.py's `format_table`, one line).
#      run.sh's own capture loop already sanitizes every persisted
#      `run-NN.log` to ONLY `frust-perf`/`flutter-perf`/`bench-scenario`
#      lines before it touches disk, so this script never sees or copies an
#      unfiltered logcat dump for a scenario.
#   4. Build examples/material3-demo (its own standalone workspace) with the
#      identical arm arguments, install it the same way, then drive ONE
#      push/pop pass — the fine-floor research recipe: `am force-stop`,
#      launch via `monkey -p it.f0x.material3demo -c
#      android.intent.category.LAUNCHER 1`, three `input tap <x> <y>` /
#      `keyevent KEYCODE_BACK` pairs 1.6s apart (`--taps`, below) — then pull
#      the LAST `frust-perf frame` summary line off logcat and read its
#      `n=`/`total_p50_ms=`/`submit_p95_ms=`/`acquire_p95_ms=` fields (see
#      `crates/frust-shell-common/src/perf.rs`'s `FrameStats::emit_log`).
#      The nav capture is sanitized in one pipe, never touching disk
#      unfiltered: `adb logcat -d -v raw | grep -a 'frust-perf' > <log>` —
#      the full-device dump only ever exists in the pipe between the two
#      processes, so there is no intermediate file to delete or leak on a
#      SKIPPED/parse-failure return.
#
# Kept-run accounting (scenario columns S1/S2/S4 ONLY — see the nav-basis
# note below for the two nav columns): `benchmarks/harness/stats.py`'s
# `DEFAULT_DISCARD_FIRST` always discards the first N runs of every scenario
# as warm-up before computing percentiles (PROTOCOL §4; this script never
# hand-duplicates that constant — see `DISCARD_FIRST` below, derived from
# stats.py itself), so `--runs <n>` only ever contributes `n - DISCARD_FIRST`
# runs to the published numbers. This script refuses `--runs` at or below
# `DISCARD_FIRST` (there would be nothing left to keep), defaults to 5 (3
# kept runs at the stock DISCARD_FIRST=2 — a quick pass, still below
# PROTOCOL §4's >=10-run convention), and prints `kept <n> of <m> runs` for
# every cell and in the emitted table's header/footer, so a reader of
# `ab_matrix.md` never has to reverse-engineer stats.py's discard to know
# the effective sample size. Each KEPT run log is also frame-sanity-checked
# (see "Frame sanity" below) before its scenario's percentiles are trusted.
#
# Nav-basis note: the nav columns (`nav total_p50`, `nav submit_p95`) do NOT
# follow the kept-run accounting above — they come from ONE push/pop pass
# per cell (`run_nav`, no repeated runs, nothing discarded as warm-up),
# reporting material3-demo's own in-process rolling percentiles off the
# LAST `frust-perf frame` summary line's `n=<frames>` field (the number of
# frames that summary window itself covers). The per-cell progress line and
# the emitted table both say so, so a reader never mistakes the nav columns
# for another kept-N-of-M sample.
#
# Frame sanity: before a scenario's `p50`/`p95` are trusted, every KEPT run
# log (see above) is checked for `MIN_FRAMES_PER_RUN` (below) or more parsed
# `frust-perf raw`/`flutter-perf raw` frame lines — the same line class
# `stats.py`'s frame-series path parses (see `stats.py`'s
# `FRUST_RAW_PREFIX`/`FLUTTER_RAW_PREFIX`) — and for staying within 3x of
# the kept-run median frame count. A run outside either bound means a
# truncated/stalled/empty capture (device asleep, app crash, wrong
# scenario) slipped past run.sh's own exit-code check, so the whole
# scenario cell degrades to `SKIPPED <reason>` rather than publishing a
# percentile computed over a degenerate sample.
#
# Raw-series output: every sanitized artifact this script produces —
# run.sh's own `run-NN.log` files, a `stats.txt` capturing the run.sh
# stdout tail that carries stats.py's `p50=`/`p95=` table, and the nav
# column's sanitized `logcat-frust-perf.log` — is copied into
# `benchmarks/raw/<device-name>/<group>/<cell>/<scenario>/`
# (nav: `.../<cell>/nav/logcat-frust-perf.log`) — `<group>` is `fine-floor`
# for a run without --tier and `tier` for one with it (the `fine-floor`
# name is historical, kept so the already-committed raw tree is not
# orphaned), `<cell>` is the arm label above; `<device-name>` from
# `--device-name` (default raw root: `<repo>/benchmarks/raw`, overridable
# via `--raw-root`), so RESULTS.md's Fine-floor A/B table can cite the
# exact committed files backing each cell (PROTOCOL §10's "raw series
# committed alongside the computed table" rule).
#
# A `--tier` run's scenario cells ALSO copy each captured run's
# `run-NN.pss_before.txt`/`run-NN.pss_after.txt` (run.sh's own `dumpsys
# meminfo` snapshots, benchmarks/.gitignore's documented `*.pss_*.txt`
# raw-artifact class) alongside `run-NN.log` — best-effort, whichever
# exist, never required (run.sh's own capture already is: a missing
# snapshot is the ordinary "not yet running"/"capture failed" outcome, not
# an error) — the source `gpu_graphics_stats`'s `Graphics (MB)` figure is
# computed from. A fine-floor run (no --tier) copies neither, unchanged
# from before this axis existed.
#
# Each cell's artifacts are first assembled in a staging directory under --out
# (`<out>/stage/<cell>/<sub>`, cleared per copy), every `cp` is
# checked (a failed copy is a hard, loud failure — never a silently-short
# raw series), the sanitization self-check `benchmarks/.gitignore`
# documents as its pre-commit rule runs over the STAGED copy, and only a
# copy that passes is moved into the committed tree: the destination cell
# directory — built from the RESOLVED raw root and asserted to be exactly
# `<raw-root>/<device-name>/<group>/<cell>/<sub>`, nothing else
# — is `rm -rf`'d immediately before the move (a rerun over a stale cell
# never leaves a prior run's files mixed in with the new ones). A rejected
# copy therefore never touches `benchmarks/raw/` at all, and a SKIPPED
# scenario/nav result is never copied (its cell directory, if one exists
# from an earlier rerun, is left alone — the progress line prints
# `raw: not copied (SKIPPED)` instead). The self-check fails loudly —
# never a silent partial copy:
#   - every `*.log` line matches the perf whitelist (`grep -rlvE
#     'frust-perf|flutter-perf|bench-scenario|^[[:space:]]*$'
#     --include='*.log'` must print nothing);
#   - every `stats.txt` line matches the stats.py-table whitelist — the
#     FULL line shapes `format_table`/run.sh's own `-- stats.py` banner
#     produce (STATS_WHITELIST_RE below; `benchmarks/.gitignore` cites the
#     same expression as its pre-commit rule): the banner, `== <label> ==`,
#     `frames: N total, N active, N skipped`, the `p50=..ms  p95=..ms
#     p99=..ms  worst=..ms` row, the `missed_60hz=N (budget ..ms)
#     missed_120hz=N (budget ..ms)` row, or blank;
#   - the expected artifacts are actually present — `run-01.log` through
#     `run-<RUNS>.log` plus `stats.txt` for a scenario cell,
#     `logcat-frust-perf.log` for a nav cell — a missing artifact fails the
#     matrix exactly like an unsanitized one, rather than silently
#     publishing a table row backed by an incomplete raw series.
#
# Device wake/unlock/lock: before cell 1 (skipped in --dry-run, but
# printed by it so a preview shows the full live-run command sequence),
# this script wakes the device if it isn't already `Awake`
# (`dumpsys power`'s `mWakefulness`) and sends the menu key to clear the
# keyguard if one is showing (`dumpsys activity activities`'
# `mKeyguardShowing`) — a scenario/nav capture started against a sleeping
# or locked screen captures nothing usable. It locks the device again on
# exit (normal, error, or Ctrl-C) ONLY if this run itself woke it OR
# dismissed its keyguard (a device the operator had already unlocked on
# purpose — awake, no keyguard showing — is left alone), and never under
# --dry-run.
#
# Emits a Markdown table — column `tier`, one p50/p95 column pair per
# `--scenarios` entry (a
# `--tier` run additionally carries that scenario's `gpu_total p50`/
# `gpu_total p95`/`Graphics (MB)` columns — the v4 raw-format GPU-pass
# percentiles and the post-run `dumpsys meminfo` Graphics-PSS mean,
# `n/a` when no kept frame carries a `gpu_q=1` reading or no kept run's
# snapshot parsed a Graphics row; see `gpu_graphics_stats` below — a
# fine-floor run, no --tier, grows neither, unchanged from before this
# axis existed), `nav total_p50`, `nav submit_p95` — to stdout and to
# `<out>/ab_matrix.md`; each finished cell's row is ALSO appended to
# `<out>/ab_matrix.rows.md` the moment it is measured, so a run that ends
# early (a hard failure in a later cell, Ctrl-C) never loses the cells
# already measured — on such an exit the EXIT trap prints the rows so far
# as a PARTIAL table (also written to `<out>/ab_matrix.partial.md`). The
# table carries a "methodology deviation" note whenever the effective kept
# run count is below 10 or the duration below 30s (see --quick below), and
# its footer ALWAYS restates the encode_us/submit_us arm-remap caveat
# (PROTOCOL §7's v4 note) — every arm here is direct-to-surface now, so a
# comparison against a pre-engine blit-arm capture must read the columns
# immune to the remap: this table's own p50/p95 (total_us, the summed
# column) and, when present, gpu_total.
# Ctrl-C/SIGTERM STOPS the matrix — no further cell is built, installed or
# run — and exits 130/143. Transient build/install/command output
# (including a nav run's `*.monkey.txt` launch capture — kept for
# debugging a SKIPPED cell, which is what its message points at) lives
# under `benchmarks/harness/.runs/` (already gitignored) and is never
# removed by this script; ONLY the sanitized raw-series artifacts
# described above are ever written under `benchmarks/raw/`. The EXIT trap
# performs the device-lock step above.
#
# Usage: ab_matrix.sh --device <serial> [--device-name <slug>]
#                      [--tier engine]
#                      [--scenarios s1,s2,s4] [--runs 12] [--duration 30]
#                      [--quick] [--frust <path>] [--raw-root <dir>]
#                      [--taps 540,472,540,734,540,996] [--out <dir>]
#                      [--dry-run]
#
#   --device <serial>  adb device serial (`adb devices`) — required even
#                      under --dry-run (a dry run still prints the exact
#                      `adb -s <serial> ...` command lines it would issue).
#   --device-name <slug>  lowercase device slug (`[a-z0-9_]+`, e.g.
#                      `pixel5`) used to file this run's raw series under
#                      `benchmarks/raw/<slug>/fine-floor/...` — required
#                      for a live run; may be omitted under --dry-run (a
#                      placeholder is printed instead).
#   --tier <csv>       render arms to sweep. Omitted entirely by default —
#                      the matrix then runs ONE cell, the app's ordinary
#                      default build (which is an engine build: `frust-render`
#                      contains no other renderer), filed under
#                      `<raw-root>/<device>/fine-floor/default/`. Passing it
#                      turns on the tier axis: the run files its raw series
#                      under `<raw-root>/<device>/tier/<cell>/` instead (so a
#                      tier sweep never overwrites a default-arm cell) and the
#                      table grows the per-scenario gpu_total/Graphics
#                      columns. Values (validated up front):
#                        default       the app's ordinary build, no arm-
#                                      specific build args. Cell label
#                                      `default`.
#                        engine        a LABELLED SYNONYM of `default`: the
#                                      frust-owned `frust-engine` strip
#                                      pipeline is what an ordinary build
#                                      already runs, so this arm passes the
#                                      same (empty) build args and differs
#                                      only in its cell label `engine`, kept
#                                      so RESULTS.md's existing `engine` cell
#                                      names stay stable. The feature +
#                                      env-var define this arm used to pass
#                                      named a tier selection that no longer
#                                      exists and would now be rejected by
#                                      cargo.
#                      The vello-classic `classic` arm is GONE with the
#                      renderer it named. The axis is kept, one renderer wide
#                      today, as the seam a second arm plugs into.
#   --scenarios <csv>  benchmarks/frust_bench scenario ids to run per cell,
#                      passed through to run.sh (default: s1,s2,s4). Each
#                      value must be one of s1..s8, d1, d2, validated up
#                      front (same posture as --tier).
#   --runs <n>         runs per scenario, passed to run.sh (default: 12 —
#                      PROTOCOL §4's own >=10-kept-run convention at the
#                      stock DISCARD_FIRST=2, i.e. 10 kept; see --quick
#                      below for the old quick-pass default). Must be >
#                      DISCARD_FIRST (derived at startup from stats.py's
#                      own `DEFAULT_DISCARD_FIRST`, normally 2 — see
#                      below) — stats.py always discards the first
#                      DISCARD_FIRST runs as warm-up (PROTOCOL §4), so
#                      anything at or below it would keep zero runs. An
#                      explicit --runs always wins over --quick's own
#                      default, whichever order the two flags are given in.
#   --duration <secs>  capture window per run, passed to run.sh (default:
#                      30 — PROTOCOL §4's own convention; see --quick).
#                      An explicit --duration always wins over --quick's
#                      own default, the same way --runs does.
#   --quick            use the pre-PROTOCOL-§4 quick pass instead of the
#                      new default: 5 runs x 20s (3 kept at the stock
#                      DISCARD_FIRST=2) — the convention every quick-pass
#                      row already published in RESULTS.md used. Sets
#                      --runs'/--duration's OWN defaults only; an explicit
#                      --runs or --duration on the same command line still
#                      wins (see above). Whenever the EFFECTIVE kept-run
#                      count is below 10 or the effective duration is below
#                      30s — via --quick, or via a caller's own --runs/
#                      --duration below the convention — the emitted
#                      table's header carries a "methodology deviation:
#                      below PROTOCOL §4" note, and the dry-run preview
#                      prints the same disclosure, so a reader is told
#                      up front rather than left to notice the run count.
#   --frust <path>     path to the frust CLI binary (default:
#                      <repo-root>/target/debug/frust).
#   --raw-root <dir>   root of the committed sanitized raw-series tree
#                      (default: <repo-root>/benchmarks/raw). Must be
#                      non-empty and must not resolve to `/` or the home
#                      directory: every cell path — and so the one `rm -rf`
#                      this script performs — is built from its RESOLVED
#                      form (see the raw-series note above). Refused if it
#                      would resolve inside --out, or vice versa — the
#                      transient/unsanitized output directory and the
#                      sanitized raw-series root must never nest inside one
#                      another (see the raw-series note above). Both sides
#                      are resolved with `cd -P ... && pwd -P` (physical
#                      path, symlinks resolved) before the prefix compare,
#                      so a symlinked --out/--raw-root can't slip past the
#                      logical-path guard a plain `pwd` would give.
#   --taps <csv>       six comma-separated integers x1,y1,x2,y2,x3,y3 — the
#                      three tap points for the nav push/pop recipe
#                      (default: 540,472,540,734,540,996 — the fine-floor
#                      research recipe's coordinates, verified to land on
#                      the same list rows on both the Pixel 5a and the
#                      Pixel 5 at 2.75x density).
#   --out <dir>        output directory for the emitted table + transient
#                      build/install/command logs (default: a fresh
#                      mktemp -d under benchmarks/harness/.runs/). Ignored
#                      under --dry-run (nothing is written to disk). See
#                      --raw-root above for where the sanitized per-cell
#                      artifacts land — that is a separate directory, not
#                      this one.
#   --dry-run          print every command this script would run, for every
#                      cell of the matrix — including the raw-series copy
#                      steps and the `kept <n> of <n> runs` line — and exit
#                      0. No build, no adb, no filesystem write, no device
#                      touched.
#
# Never skips a cell silently: a missing required tool (adb, python3, the
# frust CLI binary), an unreachable device, or a build failure is a loud,
# non-zero-exit failure of the whole matrix run. A single scenario or nav
# measurement that can't be parsed out of an otherwise-successful run's
# output instead degrades that one table cell to `SKIPPED <reason>` and the
# matrix continues — never a placeholder number.
#
# Exit status: non-zero on a usage error, a missing required tool, an
# unreachable device, a build/install failure, or a raw-series
# sanitization self-check failure; 0 otherwise (individual `SKIPPED` cells
# do not fail the run).

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." >/dev/null 2>&1 && pwd)"

DEVICE=""
DEVICE_NAME=""
# The tier axis is OFF unless --tier is passed: TIER_AXIS gates the
# `tier` raw-series group, the gpu_total/Graphics columns and their
# pss_*.txt raw-series copy. --quick and the PROTOCOL §4 default below, and
# the emitted table's deviation note and arm-remap footer, are NOT gated by
# TIER_AXIS — they apply to every run (see the --runs/--duration/--quick
# docs above).
TIER_LIST="default"
TIER_AXIS=0
SCENARIOS_LIST="s1,s2,s4"
# RUNS/DURATION are resolved below (after DISCARD_FIRST is derived) from
# either PROTOCOL §4's own convention or --quick's — left unset here so
# that resolution can tell "not given on the command line" apart from "the
# user passed today's default explicitly" (RUNS_EXPLICIT/DURATION_EXPLICIT).
RUNS=""
DURATION=""
RUNS_EXPLICIT=0
DURATION_EXPLICIT=0
QUICK=0
FRUST_BIN="${REPO_ROOT}/target/debug/frust"
RAW_ROOT="${REPO_ROOT}/benchmarks/raw"
TAPS="540,472,540,734,540,996"
OUT_DIR=""
DRY_RUN=0

# Set by wake_and_unlock_device (see Helpers below) only when THIS run
# itself woke the device — gates the cleanup trap's lock-on-exit step (see
# the header's "Device wake/unlock/lock" note). Never set under --dry-run.
WOKE_DEVICE=0
DISMISSED_KEYGUARD=0
# Rows of the emitted table, appended per finished cell by the run loop —
# declared here, not in the loop, so the EXIT trap can print whatever was
# measured before an early exit. MATRIX_DONE flips to 1 once the final
# table has been written; CELL_TOTAL is the matrix size once the lists
# are parsed; RAW_ROOT_ABS is --raw-root resolved (live runs only).
declare -a TABLE_ROWS=()
MATRIX_DONE=0
CELL_TOTAL=0
RAW_ROOT_ABS=""
# Every line a scenario cell's stats.txt may contain, as full-line shapes
# (see the header's raw-series note). benchmarks/.gitignore cites this
# exact expression as its pre-commit rule for stats.txt — keep the two
# identical.
STATS_WHITELIST_RE='^-- stats\.py( --dclass)? \(discarding first [0-9]+ runs?, per protocol convention\) --$|^== [A-Za-z0-9_. ()-]+ ==$|^frames: [0-9]+ total, [0-9]+ active, [0-9]+ skipped$|^p50=[0-9.]+ms[[:space:]]+p95=[0-9.]+ms[[:space:]]+p99=[0-9.]+ms[[:space:]]+worst=[0-9.]+ms$|^missed_60hz=[0-9]+ \(budget [0-9.]+ms\)[[:space:]]+missed_120hz=[0-9]+ \(budget [0-9.]+ms\)$|^[[:space:]]*$'

# DISCARD_FIRST (runs discarded as warm-up before stats.py computes
# percentiles) is derived from stats.py itself a little further down, once
# arg parsing has run — see that derivation for why it's not just a literal
# here. Not a CLI knob either way: changing it independently of stats.py
# would silently disagree with what stats.py itself discards.

FRUST_BENCH_DIR="${REPO_ROOT}/benchmarks/frust_bench"
MATERIAL3_DEMO_DIR="${REPO_ROOT}/examples/material3-demo"
MATERIAL3_DEMO_PKG="it.f0x.material3demo"

usage() {
  # The whole header comment block: line 2 through the last line before
  # `set -uo pipefail`. Derived rather than a hard-coded upper bound, so
  # adding an option to the header above can never silently truncate the
  # usage text mid-option again.
  sed -n "2,$(($(grep -n '^set -uo pipefail' "$0" | head -n1 | cut -d: -f1) - 1))p" "$0"
}

# --- Arg parsing ------------------------------------------------------

while [ $# -gt 0 ]; do
  case "$1" in
    --device)
      [ $# -ge 2 ] || { echo "error: --device requires a value" >&2; exit 2; }
      DEVICE="$2"
      shift 2
      ;;
    --device=*)
      DEVICE="${1#--device=}"
      shift
      ;;
    --device-name)
      [ $# -ge 2 ] || { echo "error: --device-name requires a value" >&2; exit 2; }
      DEVICE_NAME="$2"
      shift 2
      ;;
    --device-name=*)
      DEVICE_NAME="${1#--device-name=}"
      shift
      ;;
    --tier)
      [ $# -ge 2 ] || { echo "error: --tier requires a value" >&2; exit 2; }
      TIER_LIST="$2"
      TIER_AXIS=1
      shift 2
      ;;
    --tier=*)
      TIER_LIST="${1#--tier=}"
      TIER_AXIS=1
      shift
      ;;
    --scenarios)
      [ $# -ge 2 ] || { echo "error: --scenarios requires a value" >&2; exit 2; }
      SCENARIOS_LIST="$2"
      shift 2
      ;;
    --scenarios=*)
      SCENARIOS_LIST="${1#--scenarios=}"
      shift
      ;;
    --runs)
      [ $# -ge 2 ] || { echo "error: --runs requires a value" >&2; exit 2; }
      RUNS="$2"
      RUNS_EXPLICIT=1
      shift 2
      ;;
    --runs=*)
      RUNS="${1#--runs=}"
      RUNS_EXPLICIT=1
      shift
      ;;
    --duration)
      [ $# -ge 2 ] || { echo "error: --duration requires a value" >&2; exit 2; }
      DURATION="$2"
      DURATION_EXPLICIT=1
      shift 2
      ;;
    --duration=*)
      DURATION="${1#--duration=}"
      DURATION_EXPLICIT=1
      shift
      ;;
    --quick)
      QUICK=1
      shift
      ;;
    --frust)
      [ $# -ge 2 ] || { echo "error: --frust requires a path" >&2; exit 2; }
      FRUST_BIN="$2"
      shift 2
      ;;
    --frust=*)
      FRUST_BIN="${1#--frust=}"
      shift
      ;;
    --raw-root)
      [ $# -ge 2 ] && [ -n "$2" ] || { echo "error: --raw-root requires a non-empty directory" >&2; exit 2; }
      RAW_ROOT="$2"
      shift 2
      ;;
    --raw-root=*)
      RAW_ROOT="${1#--raw-root=}"
      [ -n "${RAW_ROOT}" ] || { echo "error: --raw-root requires a non-empty directory" >&2; exit 2; }
      shift
      ;;
    --taps)
      [ $# -ge 2 ] || { echo "error: --taps requires a value" >&2; exit 2; }
      TAPS="$2"
      shift 2
      ;;
    --taps=*)
      TAPS="${1#--taps=}"
      shift
      ;;
    --out)
      [ $# -ge 2 ] || { echo "error: --out requires a directory" >&2; exit 2; }
      OUT_DIR="$2"
      shift 2
      ;;
    --out=*)
      OUT_DIR="${1#--out=}"
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unrecognized argument '$1'" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ -z "${DEVICE}" ]; then
  echo "error: --device <serial> is required (even under --dry-run)" >&2
  usage >&2
  exit 2
fi

if [ -n "${DEVICE_NAME}" ] && ! [[ "${DEVICE_NAME}" =~ ^[a-z0-9_]+$ ]]; then
  echo "error: --device-name must match [a-z0-9_]+ (lowercase letters, digits, underscore), got '${DEVICE_NAME}'" >&2
  exit 2
fi
if [ "${DRY_RUN}" -eq 0 ] && [ -z "${DEVICE_NAME}" ]; then
  echo "error: --device-name <slug> is required for a live run (e.g. pixel5) — it names the benchmarks/raw/<slug>/fine-floor/... directory this run's sanitized raw series is copied into. Pass --dry-run to preview without one." >&2
  exit 2
fi
DEVICE_NAME_DISPLAY="${DEVICE_NAME:-<device-name>}"

# DISCARD_FIRST: derived from stats.py's own DEFAULT_DISCARD_FIRST rather
# than hand-duplicated as a literal here — a future PROTOCOL change to that
# constant would otherwise silently disagree with what this script assumes
# stats.py discards (see the header's "Kept-run accounting" note). A loud,
# non-zero-exit failure (never a silent fallback) if the import doesn't
# yield a non-negative integer — e.g. python3 missing, or stats.py moved.
HARNESS_DIR="${SCRIPT_DIR}"
DISCARD_FIRST="$(cd "${HARNESS_DIR}" >/dev/null 2>&1 && python3 -c 'import stats; print(stats.DEFAULT_DISCARD_FIRST)' 2>/dev/null)"
if ! [[ "${DISCARD_FIRST}" =~ ^[0-9]+$ ]]; then
  echo "error: could not derive DISCARD_FIRST — \`python3 -c 'import stats; print(stats.DEFAULT_DISCARD_FIRST)'\` in ${HARNESS_DIR} did not print a non-negative integer. Is python3 on PATH and is benchmarks/harness/stats.py importable?" >&2
  exit 1
fi
echo "DISCARD_FIRST=${DISCARD_FIRST} (derived from stats.py's DEFAULT_DISCARD_FIRST)"

# --- Resolve --runs/--duration defaults (PROTOCOL §4, or --quick) -------
#
# The canonical default is now PROTOCOL §4's own convention — >=10 kept
# runs (DISCARD_FIRST + 10 total runs, 12 at the stock DISCARD_FIRST=2) x
# 30s — rather than the old quick pass, so the bare command this script's
# own header now advertises as "the canonical A/B" is protocol-compliant
# with no extra flags. --quick restores the old 3-kept x 20s pass (5 runs
# x 20s) for a fast local check. Either default applies ONLY when the
# caller did not pass the corresponding flag explicitly
# (RUNS_EXPLICIT/DURATION_EXPLICIT, set by arg parsing above) — an
# explicit --runs/--duration always wins over --quick's own default,
# whichever order the two flags are given in.
PROTOCOL_RUNS=$((DISCARD_FIRST + 10))
PROTOCOL_DURATION=30
QUICK_RUNS=5
QUICK_DURATION=20
if [ "${RUNS_EXPLICIT}" -eq 0 ]; then
  if [ "${QUICK}" -eq 1 ]; then
    RUNS="${QUICK_RUNS}"
  else
    RUNS="${PROTOCOL_RUNS}"
  fi
fi
if [ "${DURATION_EXPLICIT}" -eq 0 ]; then
  if [ "${QUICK}" -eq 1 ]; then
    DURATION="${QUICK_DURATION}"
  else
    DURATION="${PROTOCOL_DURATION}"
  fi
fi

# Minimum parsed frust-perf/flutter-perf raw frame lines a KEPT scenario run
# must carry (see the header's "Frame sanity" note) — a ~20s capture window
# at even a poor >=5fps still yields ~100 frames, so fewer than this means a
# truncated/stalled/empty capture, not a real (if slow) run. Not a CLI knob:
# a caller who needs a different floor should pass a longer --duration
# instead of loosening this sanity gate.
MIN_FRAMES_PER_RUN=100

if ! [[ "${RUNS}" =~ ^[0-9]+$ ]] || [ "${RUNS}" -le "${DISCARD_FIRST}" ]; then
  echo "error: --runs must be an integer > DISCARD_FIRST (${DISCARD_FIRST}, derived from stats.py's DEFAULT_DISCARD_FIRST) — stats.py always discards the first ${DISCARD_FIRST} runs of every scenario as warm-up (benchmarks/harness/stats.py, PROTOCOL §4), so anything at or below ${DISCARD_FIRST} would keep zero runs; got '${RUNS}'" >&2
  exit 2
fi
KEPT=$((RUNS - DISCARD_FIRST))

if ! [[ "${DURATION}" =~ ^[0-9]+$ ]] || [ "${DURATION}" -lt 1 ]; then
  echo "error: --duration must be a positive integer (seconds), got '${DURATION}'" >&2
  exit 2
fi

# Whether THIS run's runs/duration actually meet PROTOCOL §4's own
# convention (>=10 kept runs x >=30s) — independent of how RUNS/DURATION
# got their values (today's default, --quick, or a caller's own
# --runs/--duration), so a caller who dials below the convention by hand
# gets the identical disclosure --quick does. Read by the emitted table's
# deviation note and the --dry-run preview, below.
if [ "${KEPT}" -ge 10 ] && [ "${DURATION}" -ge 30 ]; then
  PROTOCOL_COMPLIANT=1
else
  PROTOCOL_COMPLIANT=0
fi

# --- Split CSV inputs into arrays --------------------------------------

IFS=',' read -r -a SCENARIO_ARR <<<"${SCENARIOS_LIST}"
IFS=',' read -r -a TIER_ARR <<<"${TIER_LIST}"

if [ "${#TIER_ARR[@]}" -eq 0 ]; then
  echo "error: --tier produced an empty list" >&2
  exit 2
fi
if [ "${#SCENARIO_ARR[@]}" -eq 0 ]; then
  echo "error: --scenarios produced an empty list" >&2
  exit 2
fi

# --tier: each value must be one of the arms the header documents.
# Deliberately NOT case-folded: the value is used verbatim as a cell label
# (and so as a committed raw-series directory name), and two spellings of
# one arm would file two cells for the same build.
for tier in "${TIER_ARR[@]}"; do
  case "${tier}" in
    default|engine) ;;
    *)
      echo "error: --tier value '${tier}' is not one of default, engine" >&2
      exit 2
      ;;
  esac
done

# The raw-series group segment: a tier sweep files under `tier/`, a run
# without --tier keeps writing the pre-existing `fine-floor/` tree (a
# historical name, kept so the committed tree is not orphaned). Only
# ever one of these two literals — assert_raw_cell_dir builds the one
# `rm -rf` this script performs out of it.
if [ "${TIER_AXIS}" -eq 1 ]; then
  RAW_GROUP="tier"
else
  RAW_GROUP="fine-floor"
fi

# --scenarios: each value must be a declared benchmarks/frust_bench
# scenario id — PROTOCOL §8's s1..s8 frame-class ids or §9.1's d1/d2
# DB-class ids.
for scenario in "${SCENARIO_ARR[@]}"; do
  case "${scenario}" in
    s1|s2|s3|s4|s5|s6|s7|s8|d1|d2) ;;
    *)
      echo "error: --scenarios value '${scenario}' is not one of s1..s8, d1, d2" >&2
      exit 2
      ;;
  esac
done

# --taps: exactly six comma-separated non-negative integers -> three (x,y)
# pairs, TAP_X[0..2] / TAP_Y[0..2].
IFS=',' read -r -a TAP_NUMS <<<"${TAPS}"
if [ "${#TAP_NUMS[@]}" -ne 6 ]; then
  echo "error: --taps must be six comma-separated integers x1,y1,x2,y2,x3,y3, got '${TAPS}'" >&2
  exit 2
fi
for n in "${TAP_NUMS[@]}"; do
  if ! [[ "${n}" =~ ^[0-9]+$ ]]; then
    echo "error: --taps value '${n}' is not a non-negative integer" >&2
    exit 2
  fi
done
TAP_X=("${TAP_NUMS[0]}" "${TAP_NUMS[2]}" "${TAP_NUMS[4]}")
TAP_Y=("${TAP_NUMS[1]}" "${TAP_NUMS[3]}" "${TAP_NUMS[5]}")

# --- EXIT / INT / TERM handling (registered before any device/filesystem -
# --- work below, so an interrupted tool-check, wake step, or matrix run ---
# --- still cleans up) -------------------------------------------------------
#
# cleanup — the EXIT trap. When the run ends before the final table was
# written but cells were already measured (a hard failure in a later cell,
# a signal), prints those rows as a PARTIAL table (also written to
# <out>/ab_matrix.partial.md); then, only for a live run that itself woke
# the device or dismissed its keyguard (WOKE_DEVICE / DISMISSED_KEYGUARD,
# set by wake_and_unlock_device below), locks it again if it's still Awake
# (see the header's "Device wake/unlock/lock" note). Best-effort
# throughout — a lost device or an already-gone OUT_DIR must never turn
# cleanup itself into a hard failure. Transient scratch under --out (a nav
# run's *.monkey.txt launch capture included) is deliberately NOT removed:
# it is what a SKIPPED cell's message points the operator at.
cleanup() {
  if [ "${MATRIX_DONE}" -eq 0 ] && [ "${#TABLE_ROWS[@]}" -gt 0 ]; then
    emit_table "**PARTIAL** — this run ended before every cell completed: ${#TABLE_ROWS[@]} of ${CELL_TOTAL} cell(s) measured; the rows below are the completed cells only (each was also appended to ${OUT_DIR}/ab_matrix.rows.md as it finished)." \
      2>/dev/null | tee "${OUT_DIR}/ab_matrix.partial.md" >&2 || true
  fi
  if [ "${DRY_RUN}" -eq 0 ] && [ -n "${DEVICE}" ] \
      && { [ "${WOKE_DEVICE}" -eq 1 ] || [ "${DISMISSED_KEYGUARD}" -eq 1 ]; }; then
    if adb -s "${DEVICE}" shell dumpsys power 2>/dev/null | grep -m1 mWakefulness | grep -q 'Awake'; then
      adb -s "${DEVICE}" shell input keyevent 26 >/dev/null 2>&1 || true
    fi
  fi
}
# on_signal <name> <exit-code> — the INT/TERM trap. A trap whose handler
# merely returns lets bash resume with the NEXT command — for this script
# that would be the next cell, driven against a device cleanup() had just
# locked, ending in exit 0 as if the matrix had completed. So a signal
# stops the run right here: nothing further is built, installed or run,
# the exit status is the conventional 128+signal (130 for SIGINT, 143 for
# SIGTERM), and cleanup() still runs on the way out via the EXIT trap.
on_signal() {
  local name="$1" code="$2"
  echo >&2
  echo "ab_matrix.sh: caught SIG${name} — stopping the matrix (no further cell is built, installed or run); exit ${code}" >&2
  exit "${code}"
}
trap cleanup EXIT
trap 'on_signal INT 130' INT
trap 'on_signal TERM 143' TERM

# --- Tool checks (skipped under --dry-run — see the header) -------------
#
# A dry run never shells out to adb/the frust CLI (DISCARD_FIRST's
# derivation above already needed python3, dry run or not — see there), so
# a preview of the command list works on a box that has neither adb nor a
# built frust CLI yet; a live run needs both, and fails loudly (not a
# SKIPPED cell) if one is missing — see the header's "never skips a cell
# silently" contract.
if [ "${DRY_RUN}" -eq 0 ]; then
  if ! command -v adb >/dev/null 2>&1; then
    echo "error: adb not found on PATH — Android SDK platform-tools required" >&2
    exit 1
  fi
  if ! command -v python3 >/dev/null 2>&1; then
    echo "error: python3 not found on PATH — required by run.sh's stats.py" >&2
    exit 1
  fi
  if [ ! -x "${FRUST_BIN}" ]; then
    echo "error: frust CLI not found or not executable at '${FRUST_BIN}' —" >&2
    echo "       run \`cargo build -p frust-cli\` at the repo root, or pass --frust <path>" >&2
    exit 1
  fi
  if ! adb -s "${DEVICE}" get-state >/dev/null 2>&1; then
    echo "error: device '${DEVICE}' not reachable (adb get-state failed) — check \`adb devices\`" >&2
    exit 1
  fi
fi

# --- Output directory + raw-series root (real runs only — a dry run -----
# --- writes nothing) ------------------------------------------------------

if [ "${DRY_RUN}" -eq 0 ]; then
  if [ -z "${OUT_DIR}" ]; then
    mkdir -p "${SCRIPT_DIR}/.runs"
    OUT_DIR="$(mktemp -d "${SCRIPT_DIR}/.runs/ab_matrix-XXXXXX")"
  else
    mkdir -p "${OUT_DIR}"
  fi
  mkdir -p "${OUT_DIR}/build" "${OUT_DIR}/install" "${OUT_DIR}/nav" "${OUT_DIR}/scenarios" "${OUT_DIR}/stage"
  mkdir -p "${RAW_ROOT}" || { echo "error: mkdir -p ${RAW_ROOT} (--raw-root) failed" >&2; exit 2; }

  # Neither directory may nest inside the other: --out holds transient,
  # unsanitized build/install/command output, and --raw-root holds ONLY
  # sanitized copies that get committed (benchmarks/.gitignore) — nesting
  # either way risks unsanitized content landing under benchmarks/raw.
  # Resolved with `cd -P && pwd -P` (physical path, symlinks followed) —
  # not a plain `pwd` — so a symlinked --out/--raw-root can't present a
  # logical path that dodges the prefix compare below while still landing
  # in the nested location on disk.
  OUT_DIR_ABS="$(cd -P "${OUT_DIR}" && pwd -P)" || { echo "error: cannot resolve --out ${OUT_DIR}" >&2; exit 2; }
  RAW_ROOT_ABS="$(cd -P "${RAW_ROOT}" && pwd -P)" || { echo "error: cannot resolve --raw-root ${RAW_ROOT}" >&2; exit 2; }
  # The resolved raw root is what every cell path — and so the one `rm -rf`
  # this script performs (publish_cell) — is built from; the unresolved
  # --raw-root string is never used for a write again. It must also be a
  # directory of its own: `/` or the home directory would turn a cell
  # directory's `rm -rf` into a deletion inside the operator's own tree.
  home_abs="$(cd -P "${HOME:-/nonexistent}" 2>/dev/null && pwd -P || true)"
  case "${RAW_ROOT_ABS}" in
    /|"${home_abs}")
      echo "error: --raw-root resolves to ${RAW_ROOT_ABS}, which is refused as a raw-series root — it must be a dedicated directory such as <repo>/benchmarks/raw." >&2
      exit 2
      ;;
  esac
  case "${OUT_DIR_ABS}" in
    "${RAW_ROOT_ABS}"|"${RAW_ROOT_ABS}"/*)
      echo "error: --out (${OUT_DIR_ABS}) resolves inside --raw-root (${RAW_ROOT_ABS}) — transient/unsanitized build, install and run.sh output would land under benchmarks/raw, which must hold only sanitized run-NN.log/stats.txt/logcat-frust-perf.log copies. Pass a different --out." >&2
      exit 2
      ;;
  esac
  case "${RAW_ROOT_ABS}" in
    "${OUT_DIR_ABS}"|"${OUT_DIR_ABS}"/*)
      echo "error: --raw-root (${RAW_ROOT_ABS}) resolves inside --out (${OUT_DIR_ABS}) — the sanitized raw-series root must not sit inside the transient output directory. Pass a different --raw-root." >&2
      exit 2
      ;;
  esac

  echo "Output directory: ${OUT_DIR}"
  echo "Raw series root: ${RAW_ROOT_ABS}/${DEVICE_NAME}/${RAW_GROUP}/"
else
  # A dry run resolves nothing and writes nothing: the cell paths it prints
  # use --raw-root as given.
  RAW_ROOT_ABS="${RAW_ROOT}"
fi

# --- Helpers -------------------------------------------------------------

# die <message...> — a hard, loud failure of the whole matrix (see header):
# print to stderr, exit 1. Used by every filesystem step below whose
# silent failure could otherwise leave a short/corrupt raw series.
die() {
  echo "error: $*" >&2
  exit 1
}

# print_cmd <label> <args...> — echo one planned/executed command line,
# identically under --dry-run and a live run (so a --dry-run's output and a
# live run's own progress log read the same way).
print_cmd() {
  local label="$1"
  shift
  echo "  ${label}$*"
}

# cell_label <tier> — the cell's directory name and table identity: the arm
# name itself, which is `default` for a run without --tier.
cell_label() {
  echo "$1"
}

# raw_cell_dir <cell> <sub> — echoes
# <raw-root-abs>/<device-name>/<group>/<cell>/<sub> (sub is a scenario id,
# or "nav"; <group> is `fine-floor` or `tier`, <cell> comes from
# cell_label), built from the RESOLVED raw root. Uses the display
# placeholder for <device-name> when none was given (--dry-run only — a
# live run requires --device-name).
raw_cell_dir() {
  local cell="$1" sub="$2"
  echo "${RAW_ROOT_ABS}/${DEVICE_NAME_DISPLAY}/${RAW_GROUP}/${cell}/${sub}"
}

# stage_cell_dir <cell> <sub> — echoes the staging directory under --out a
# cell is assembled and self-checked in before publish_cell moves it into
# the committed tree: <out>/stage/<cell>/<sub> (`<out>` literally under
# --dry-run, which has no output directory).
stage_cell_dir() {
  local cell="$1" sub="$2"
  echo "${OUT_DIR:-<out>}/stage/${cell}/${sub}"
}

# assert_raw_cell_dir <dir> — dies unless <dir> is exactly one cell
# directory under the RESOLVED raw root —
# <raw-root-abs>/<device-name>/<group>/<cell>/<sub>: four non-empty
# segments below the root, the group segment exactly the `fine-floor`/
# `tier` literal this run resolved, every segment drawn from
# [A-Za-z0-9._-] (no glob metacharacter, no whitespace), no `..` segment
# anywhere, an absolute root.
# Called by publish_cell immediately before its `rm -rf`, the only
# recursive delete this script performs, which must never be pointed
# anywhere else whatever --raw-root, --device-name, --tier or the matrix
# values were.
assert_raw_cell_dir() {
  local dir="$1" rel slashes seg
  [ -n "${RAW_ROOT_ABS}" ] || die "assert_raw_cell_dir: the raw root is not resolved"
  case "${RAW_ROOT_ABS}" in
    /*) ;;
    *) die "assert_raw_cell_dir: raw root '${RAW_ROOT_ABS}' is not absolute" ;;
  esac
  case "${dir}" in
    *'/../'*|*'/..'|'../'*|'..') die "refusing to touch '${dir}': the path contains a '..' segment" ;;
  esac
  case "${RAW_GROUP}" in
    fine-floor|tier) ;;
    *) die "assert_raw_cell_dir: unexpected raw-series group '${RAW_GROUP}'" ;;
  esac
  case "${dir}" in
    "${RAW_ROOT_ABS}"/*/"${RAW_GROUP}"/*/*) ;;
    *) die "refusing to touch '${dir}': not of the form ${RAW_ROOT_ABS}/<device-name>/${RAW_GROUP}/<cell>/<sub>" ;;
  esac
  rel="${dir#"${RAW_ROOT_ABS}"/}"
  case "${rel}" in
    *//*|/*|*/) die "refusing to touch '${dir}': empty path segment in '${rel}'" ;;
  esac
  slashes="${rel//[^\/]/}"
  [ "${#slashes}" -eq 3 ] || die "refusing to touch '${dir}': expected exactly <device-name>/${RAW_GROUP}/<cell>/<sub> below ${RAW_ROOT_ABS}, got '${rel}'"
  # Every segment from a conservative charset: no glob metacharacter, no
  # whitespace, nothing shell-special (parent traversal is already refused
  # by the `..` check above). The cell label is built from
  # --tier and the device name, both validated up front, so
  # this is belt-and-braces on the one path this script deletes.
  local IFS='/'
  for seg in ${rel}; do
    [[ "${seg}" =~ ^[A-Za-z0-9._-]+$ ]] \
      || die "refusing to touch '${dir}': path segment '${seg}' is outside [A-Za-z0-9._-]"
  done
}

# publish_cell <stage-dir> <cell-dir> — moves a cell that PASSED the
# self-check from its staging directory into the committed tree. The only
# `rm -rf` this script performs happens here, immediately after
# assert_raw_cell_dir has vetted the destination; it clears a stale cell
# from an earlier rerun before the move.
publish_cell() {
  local stage_dir="$1" cell_dir="$2" parent
  assert_raw_cell_dir "${cell_dir}"
  rm -rf "${cell_dir}"
  parent="$(dirname "${cell_dir}")"
  mkdir -p "${parent}" || die "mkdir -p ${parent} failed"
  mv "${stage_dir}" "${cell_dir}" || die "mv ${stage_dir} -> ${cell_dir} failed"
}

# self_check_raw_dir <dir> <kind:scenario|nav> [<runs>] — re-runs the
# sanitization self-check benchmarks/.gitignore documents as its pre-commit
# rule over the raw-series artifacts copy_scenario_raw/copy_nav_raw just
# STAGED into <dir> (under --out — nothing has touched the committed tree
# yet; publish_cell runs only after this returns) — a hard, loud failure
# of the whole matrix on any violation (see header), never a silent
# partial/corrupt copy, and never a rejected file left under
# benchmarks/raw:
#   - every *.log line matches the perf whitelist (frust-perf/
#     flutter-perf/bench-scenario/blank);
#   - kind=scenario: every stats.txt line matches STATS_WHITELIST_RE (the
#     full line shapes format_table/run.sh's own "-- stats.py" banner
#     produce), AND run-01.log..run-<runs>.log plus stats.txt are all
#     present;
#   - kind=nav: logcat-frust-perf.log is present.
# A --tier run's copy_scenario_raw ALSO stages each captured run's
# run-NN.pss_before.txt/run-NN.pss_after.txt (see that function) — NOT
# checked here: their `.txt` name never matches the `*.log` glob above
# (dumpsys meminfo output is not a perf/marker line, so no whitelist for
# it exists in this function), and they are copied best-effort, so their
# presence is never required the way run-NN.log's is.
# benchmarks/.gitignore's own pss content check ("scan *.pss_*.txt for any
# package identifier other than it.f0x.*bench") is a separate pre-commit
# check run by hand, not part of this function.
# Only ever called after a non-SKIPPED copy (see the run loop below) — a
# SKIPPED cell is never staged, so this never runs against one.
self_check_raw_dir() {
  local dir="$1" kind="$2" runs="${3:-0}" offenders i run_file
  [ -d "${dir}" ] || die "expected raw-series directory missing: ${dir}"

  offenders="$(grep -rlvE 'frust-perf|flutter-perf|bench-scenario|^[[:space:]]*$' "${dir}" --include='*.log' 2>/dev/null || true)"
  if [ -n "${offenders}" ]; then
    echo "error: unsanitized content found under ${dir} (file(s) with a line matching none of frust-perf/flutter-perf/bench-scenario/blank):" >&2
    printf '  %s\n' "${offenders}" >&2
    exit 1
  fi

  case "${kind}" in
    scenario)
      [ -f "${dir}/stats.txt" ] || die "expected ${dir}/stats.txt missing"
      offenders="$(grep -vE "${STATS_WHITELIST_RE}" "${dir}/stats.txt" 2>/dev/null || true)"
      if [ -n "${offenders}" ]; then
        echo "error: unexpected content in ${dir}/stats.txt (line(s) matching none of the stats.py-table full-line shapes — banner / == label == / frames: / p50= row / missed_60hz= row / blank):" >&2
        printf '  %s\n' "${offenders}" >&2
        exit 1
      fi
      for ((i = 1; i <= runs; i++)); do
        run_file="${dir}/run-$(printf '%02d' "${i}").log"
        [ -f "${run_file}" ] || die "expected ${run_file} missing"
      done
      ;;
    nav)
      [ -f "${dir}/logcat-frust-perf.log" ] || die "expected ${dir}/logcat-frust-perf.log missing"
      ;;
    *)
      die "self_check_raw_dir: unknown kind '${kind}'"
      ;;
  esac
}

# copy_scenario_raw <scenario> <cell> <scen_out> <runsh_log> —
# assembles run.sh's own sanitized run-NN.log files plus a stats.txt (the
# run.sh stdout tail carrying stats.py's p50=/p95= table) in the cell's
# staging directory (cleared first), runs the raw-series sanitization
# self-check THERE, and only then publishes the cell into
# <raw-root-abs>/<device-name>/<group>/<cell>/<scenario>/ (see
# publish_cell). Only ever called for a non-SKIPPED scenario result (see
# the run loop) — every cp is checked, and a missing expected artifact
# fails the self-check before anything is published.
#
# A --tier run (TIER_AXIS=1) ALSO stages each captured run's
# run-NN.pss_before.txt/run-NN.pss_after.txt (run.sh's own `dumpsys
# meminfo` snapshots, benchmarks/.gitignore's documented `*.pss_*.txt`
# raw-artifact class) — best-effort, whichever exist, never required (see
# self_check_raw_dir's own note): gpu_graphics_stats' Graphics (MB) figure
# is computed from the published run-NN.pss_after.txt files. A fine-floor
# run (no --tier) copies neither, unchanged from before this axis existed.
copy_scenario_raw() {
  local scenario="$1" cell="$2" scen_out="$3" runsh_log="$4" cell_dir stage_dir
  cell_dir="$(raw_cell_dir "${cell}" "${scenario}")"
  stage_dir="$(stage_cell_dir "${cell}" "${scenario}")"
  if [ "${DRY_RUN}" -eq 1 ]; then
    if [ "${TIER_AXIS}" -eq 1 ]; then
      print_cmd "" "rm -rf ${stage_dir} && mkdir -p ${stage_dir} && cp ${scen_out}/run-*.log ${stage_dir}/ && cp ${scen_out}/run-*.pss_before.txt ${scen_out}/run-*.pss_after.txt ${stage_dir}/ (best-effort, whichever exist) && sed -n '/^-- stats.py/,\$p' ${runsh_log} > ${stage_dir}/stats.txt && self-check ${stage_dir} && rm -rf ${cell_dir} && mv ${stage_dir} ${cell_dir}" >&2
    else
      print_cmd "" "rm -rf ${stage_dir} && mkdir -p ${stage_dir} && cp ${scen_out}/run-*.log ${stage_dir}/ && sed -n '/^-- stats.py/,\$p' ${runsh_log} > ${stage_dir}/stats.txt && self-check ${stage_dir} && rm -rf ${cell_dir} && mv ${stage_dir} ${cell_dir}" >&2
    fi
    return 0
  fi
  rm -rf "${stage_dir}"
  mkdir -p "${stage_dir}" || die "mkdir -p ${stage_dir} failed"
  local copied=0 f
  for f in "${scen_out}"/run-*.log; do
    [ -e "${f}" ] || continue
    cp "${f}" "${stage_dir}/" || die "cp ${f} -> ${stage_dir}/ failed"
    copied=1
  done
  if [ "${copied}" -eq 0 ]; then
    echo "note: no sanitized run-NN.log files found under ${scen_out} — nothing staged for ${cell_dir}" >&2
  fi
  if [ "${TIER_AXIS}" -eq 1 ]; then
    for f in "${scen_out}"/run-*.pss_before.txt "${scen_out}"/run-*.pss_after.txt; do
      [ -e "${f}" ] || continue
      cp "${f}" "${stage_dir}/" || die "cp ${f} -> ${stage_dir}/ failed"
    done
  fi
  if [ -f "${runsh_log}" ]; then
    sed -n '/^-- stats.py/,$p' "${runsh_log}" >"${stage_dir}/stats.txt" || die "writing ${stage_dir}/stats.txt failed"
    if [ ! -s "${stage_dir}/stats.txt" ]; then
      rm -f "${stage_dir}/stats.txt"
    fi
  fi
  self_check_raw_dir "${stage_dir}" "scenario" "${RUNS}"
  publish_cell "${stage_dir}" "${cell_dir}"
}

# copy_nav_raw <cell> <nav_log> — stages the already-sanitized nav
# log (see run_nav) as logcat-frust-perf.log in the cell's staging
# directory (cleared first), runs the raw-series sanitization self-check
# THERE, and only then publishes the cell into
# <raw-root-abs>/<device-name>/<group>/<cell>/nav/ (see
# publish_cell). Only ever called for a non-SKIPPED nav result (see the
# run loop) — the cp is checked, and a missing expected artifact fails the
# self-check before anything is published.
copy_nav_raw() {
  local cell="$1" nav_log="$2" cell_dir stage_dir
  cell_dir="$(raw_cell_dir "${cell}" "nav")"
  stage_dir="$(stage_cell_dir "${cell}" "nav")"
  if [ "${DRY_RUN}" -eq 1 ]; then
    print_cmd "" "rm -rf ${stage_dir} && mkdir -p ${stage_dir} && cp ${nav_log} ${stage_dir}/logcat-frust-perf.log && self-check ${stage_dir} && rm -rf ${cell_dir} && mv ${stage_dir} ${cell_dir}" >&2
    return 0
  fi
  rm -rf "${stage_dir}"
  mkdir -p "${stage_dir}" || die "mkdir -p ${stage_dir} failed"
  if [ -f "${nav_log}" ]; then
    cp "${nav_log}" "${stage_dir}/logcat-frust-perf.log" || die "cp ${nav_log} -> ${stage_dir}/logcat-frust-perf.log failed"
  else
    echo "note: nav log ${nav_log} not found — nothing staged for ${cell_dir}" >&2
  fi
  self_check_raw_dir "${stage_dir}" "nav"
  publish_cell "${stage_dir}" "${cell_dir}"
}

# gpu_graphics_stats <scenario> <scen_out> — echoes
# "<gpu_p50_ms> <gpu_p95_ms> <gpu_n> <graphics_mb> <graphics_n>" for one
# scenario's KEPT runs under <scen_out> — the v4 gpu_total_us percentiles
# (stats.py's own gpu_spans, PROTOCOL §7's v4 note) and the Graphics-PSS
# mean (stats.py's own mean_graphics_mb) behind the emitted table's
# "<SCEN> gpu_total p50/p95"/"<SCEN> Graphics (MB)" columns. ONLY ever
# called under --tier (TIER_AXIS=1, see the header's Raw-series output
# note) — a fine-floor run never calls this.
#
# gpu_p50_ms/gpu_p95_ms read "n/a" (gpu_n "0") when no kept frame carries a
# gpu_q=1 reading — the ordinary case for a build with no GPU timer
# (a non-perf-trace build, or an adapter without TIMESTAMP_QUERY; see
# PROTOCOL.md's v4 note), never treated as a failure.
# graphics_mb/graphics_n read "n/a"/"0" the same way when no kept run's
# POST-run dumpsys meminfo snapshot (run-NN.pss_after.txt) parsed a
# Graphics row (run.sh's own capture is best-effort).
#
# Computed via stats.py's OWN functions — load_run_frames/
# discard_first_runs/gpu_spans/_nearest_rank_percentile/mean_graphics_mb,
# imported and reused, exactly the convention RESULTS.md's baseline
# section already established for its by-hand encode+submit p50/
# Graphics (MB) figures — never reimplemented here. Never fails the matrix
# (see header): python3 itself is already a hard startup requirement (Tool
# checks above), so any other error here degrades to the same "n/a" this
# function reports for "nothing parsed" rather than aborting the run — the
# scenario's own p50/p95 already succeeded (run_scenario) by the time this
# is called. Same stdout/stderr split as run_scenario/run_nav: progress
# goes to stderr, ONLY the result line goes to stdout.
gpu_graphics_stats() {
  local scenario="$1" scen_out="$2"
  print_cmd "" "python3 -c '<gpu_total_us + Graphics-PSS percentile computation over ${scen_out}, discard-first ${DISCARD_FIRST}, via stats.py load_run_frames/discard_first_runs/gpu_spans/_nearest_rank_percentile/mean_graphics_mb>'" >&2
  if [ "${DRY_RUN}" -eq 1 ]; then
    echo "n/a n/a 0 n/a 0"
    return 0
  fi
  python3 - "${scenario}" "${scen_out}" "${DISCARD_FIRST}" "${HARNESS_DIR}" <<'PYEOF' 2>/dev/null || echo "n/a n/a 0 n/a 0"
import sys
from pathlib import Path

scenario, scen_out, discard_first, harness_dir = sys.argv[1:5]
discard_first = int(discard_first)
sys.path.insert(0, harness_dir)
import stats

scen_out_path = Path(scen_out)
logs = sorted(scen_out_path.glob("run-*.log"))
runs = [stats.load_run_frames(p, scenario) for p in logs]
frames = stats.discard_first_runs(runs, discard_first)

gpu_vals = []
for f in frames:
    if f.skipped:
        continue
    spans = stats.gpu_spans(f)
    if spans is None:
        continue
    v = spans.get("gpu_total_us")
    if v is not None:
        gpu_vals.append(v)
gpu_vals.sort()
if gpu_vals:
    p50 = stats._nearest_rank_percentile(gpu_vals, 50) / 1000
    p95 = stats._nearest_rank_percentile(gpu_vals, 95) / 1000
    gpu_out = f"{p50:.2f} {p95:.2f} {len(gpu_vals)}"
else:
    gpu_out = "n/a n/a 0"

pss_files = sorted(scen_out_path.glob("run-*.pss_after.txt"))
kept_pss = pss_files[discard_first:] if len(pss_files) > discard_first else pss_files
mean_mb, samples = stats.mean_graphics_mb(kept_pss)
if samples:
    gfx_out = f"{mean_mb:.2f} {samples}"
else:
    gfx_out = "n/a 0"

print(f"{gpu_out} {gfx_out}")
PYEOF
}

# emit_table <status-line> — the Markdown table over whatever TABLE_ROWS
# holds so far, to stdout: called once at the end of a completed matrix
# (empty status line) and, with a PARTIAL status line, by the EXIT trap
# when the run ends early with rows already measured (see cleanup).
emit_table() {
  local status="$1" header sep scenario upper row
  header="| Tier |"
  sep="|---|"
  for scenario in "${SCENARIO_ARR[@]}"; do
    upper="$(printf '%s' "${scenario}" | tr '[:lower:]' '[:upper:]')"
    header="${header} ${upper} p50 (ms) | ${upper} p95 (ms) |"
    sep="${sep}---|---|"
    if [ "${TIER_AXIS}" -eq 1 ]; then
      header="${header} ${upper} gpu_total p50 (ms) | ${upper} gpu_total p95 (ms) | ${upper} Graphics (MB) |"
      sep="${sep}---|---|---|"
    fi
  done
  header="${header} nav total_p50 (ms) | nav submit_p95 (ms) |"
  sep="${sep}---|---|"

  echo
  if [ "${TIER_AXIS}" -eq 1 ]; then
    echo "## Render-tier A/B matrix (--tier ${TIER_LIST}) — device ${DEVICE}"
  else
    echo "## Render A/B matrix (default arm) — device ${DEVICE}"
  fi
  echo
  if [ -n "${status}" ]; then
    echo "${status}"
    echo
  fi
  echo "Kept ${KEPT} of ${RUNS} runs per scenario (the first ${DISCARD_FIRST} are discarded as warm-up per PROTOCOL §4); percentiles below are computed over the kept runs only. The nav columns (nav total_p50, nav submit_p95) are NOT part of this kept-run accounting — see the note below."
  if [ "${PROTOCOL_COMPLIANT}" -eq 0 ]; then
    echo
    if [ "${QUICK}" -eq 1 ] && [ "${RUNS_EXPLICIT}" -eq 0 ] && [ "${DURATION_EXPLICIT}" -eq 0 ]; then
      echo "**Methodology deviation:** --quick was passed — ${KEPT} kept run(s) x ${DURATION}s, below PROTOCOL §4's >=10-kept-run x 30s convention. Restate this (or an equivalent) in RESULTS.md's own 'Methodology deviations' subsection for any row this table's numbers feed, per that file's existing discipline."
    else
      echo "**Methodology deviation:** ${KEPT} kept run(s) x ${DURATION}s — below PROTOCOL §4's >=10-kept-run x 30s convention (--runs/--duration set below it). Restate this (or an equivalent) in RESULTS.md's own 'Methodology deviations' subsection for any row this table's numbers feed, per that file's existing discipline."
    fi
  fi
  echo
  echo "${header}"
  echo "${sep}"
  for row in "${TABLE_ROWS[@]}"; do
    echo "${row}"
  done
  echo
  if [ "${TIER_AXIS}" -eq 1 ]; then
    echo "GPU/Graphics note: <SCEN> gpu_total p50/p95 are computed from each"
    echo "kept run's gpu_total_us field (stats.py's gpu_spans, the v4 raw"
    echo "format) and read n/a when no kept frame carries a gpu_q=1 reading —"
    echo "the ordinary case off a build with no GPU timer (a non-perf-trace"
    echo "build, or an adapter without TIMESTAMP_QUERY; see"
    echo "PROTOCOL.md's v4 note), not a failure. <SCEN> Graphics (MB) averages"
    echo "the Graphics PSS row of each kept run's POST-run dumpsys meminfo"
    echo "snapshot (run.sh's own pss_after capture; pss_before is always"
    echo "empty — the app is force-stopped ahead of every run) and reads n/a"
    echo "the same way when no snapshot parsed."
    echo
  fi
  echo "Nav-basis note: the nav columns come from ONE push/pop pass per cell"
  echo "(no repeated runs, nothing discarded as warm-up) — they report"
  echo "material3-demo's own in-process rolling percentiles off the LAST"
  echo "frust-perf frame summary line's n=<frames> field, not a kept-N-of-M"
  echo "sample like the scenario columns above."
  echo
  echo "Percentiles are computed over the KEPT runs (${KEPT} of ${RUNS}; the first ${DISCARD_FIRST} are discarded as warm-up per PROTOCOL §4) — scenario columns only; see the nav-basis note above for the nav columns."
  if [ "${TIER_AXIS}" -eq 1 ]; then
    echo "Raw series (sanitized run-NN.log/stats.txt/run-NN.pss_before.txt/run-NN.pss_after.txt per scenario, sanitized nav logcat) copied under ${RAW_ROOT}/${DEVICE_NAME}/${RAW_GROUP}/."
  else
    echo "Raw series (sanitized run-NN.log/stats.txt per scenario, sanitized nav logcat) copied under ${RAW_ROOT}/${DEVICE_NAME}/${RAW_GROUP}/."
  fi
  echo
  echo "Encode/submit arm-remap caveat (always stated here — PROTOCOL §7's v4"
  echo "note): encode_us and submit_us are NOT the same instrument on every"
  echo "arm. Every arm this script drives is now direct-to-surface (the engine"
  echo "records into the acquired swapchain view), so the GPU render lands in"
  echo "submit_us and acquire_us precedes it, while encode_us carries only the"
  echo "frame's scene copy. A capture taken before the engine became the only"
  echo "renderer measured a blit arm, where the GPU render landed in encode_us"
  echo "and acquire_us followed it — so do not compare encode_us or submit_us"
  echo "alone against one. Compare on the columns immune to the remap: this"
  echo "table's own p50/p95 (total_us, the summed column — every CPU pass plus"
  echo "acquire/submit) and, when present, the gpu_total columns above"
  echo "(measured on the GPU's own clock, untouched by which CPU call the"
  echo "recording happened inside)."
}

# tier_build_args <tier> — echoes the arm-specific tail of the `frust build
# apk` command line, one argument per line (the caller reads it into an
# array, so a value never re-splits on whitespace):
#
#   default       (nothing — the app's ordinary build)
#   engine        (nothing — same build, kept only for its cell label)
#
# Both arms are empty today. The renderer is no longer selectable: the
# frust-engine strip pipeline is the only one `frust-render` contains, so the
# feature + env-var define the `engine` arm used to pass name a choice that
# does not exist any more (cargo would reject the feature outright). The arm
# survives as a labelled synonym so RESULTS.md's `engine` cells keep their
# names, and this function stays the seam a genuinely different second arm
# would plug its build args into. `--define FRUST_TRACE_RAW=1` is common to
# every arm and stays on the caller's line.
tier_build_args() {
  local tier="$1"
  case "${tier}" in
    default)
      ;;
    engine)
      ;;
    *)
      die "tier_build_args: unknown tier '${tier}'"
      ;;
  esac
}

# tier_build_desc <tier> — the human-readable arm summary the per-build
# progress line carries, matching what tier_build_args passes.
tier_build_desc() {
  case "$1" in
    default) echo "default build (no arm flags)" ;;
    engine) echo "engine arm — same default build, labelled cell" ;;
    *) die "tier_build_desc: unknown tier '$1'" ;;
  esac
}

# build_apk <label> <app-dir> <tier> <log> — runs (or, under --dry-run, only
# prints) `<frust> build apk --profile --define ...` from <app-dir>, with the
# arm's own defines/features from tier_build_args. A build failure is a hard,
# loud failure of the whole matrix — never a SKIPPED cell (see header).
build_apk() {
  local label="$1" app_dir="$2" tier="$3" log="$4"
  local -a tier_args=()
  mapfile -t tier_args < <(tier_build_args "${tier}")
  echo "-- ${label}: build ($(tier_build_desc "${tier}")) --"
  print_cmd "(cd ${app_dir} && " "${FRUST_BIN} build apk --profile --define FRUST_TRACE_RAW=1 ${tier_args[*]})"
  if [ "${DRY_RUN}" -eq 1 ]; then
    return 0
  fi
  if ! ( cd "${app_dir}" && "${FRUST_BIN}" build apk --profile \
      --define "FRUST_TRACE_RAW=1" \
      "${tier_args[@]}" ) >"${log}" 2>&1; then
    echo "error: ${label} build failed (tier=${tier}) — see ${log}" >&2
    exit 1
  fi
}

# install_apk <label> <apk-path> <log> — `adb install -r`; a missing APK
# after a claimed-successful build, or an install failure, is a hard, loud
# failure of the whole matrix (see header).
install_apk() {
  local label="$1" apk="$2" log="$3"
  echo "-- ${label}: install --"
  print_cmd "" "adb -s ${DEVICE} install -r ${apk}"
  if [ "${DRY_RUN}" -eq 1 ]; then
    return 0
  fi
  if [ ! -f "${apk}" ]; then
    echo "error: expected APK not found after build: ${apk}" >&2
    exit 1
  fi
  if ! adb -s "${DEVICE}" install -r "${apk}" >"${log}" 2>&1; then
    echo "error: 'adb install -r' failed for ${apk} — see ${log}" >&2
    exit 1
  fi
}

# frust_bench_apk_path / material3_demo_apk_path — the AGP `profile`
# build-type's default universal-APK output path under the app's single
# `build/` root (the scaffold's settings.gradle.kts redirects `:app` to
# `build/android/app`, see docs/DEVELOPMENT.md "Build Output Layout"; the
# `release` twin is .../outputs/apk/release/app-release.apk).
frust_bench_apk_path() {
  echo "${FRUST_BENCH_DIR}/build/android/app/outputs/apk/profile/app-profile.apk"
}
material3_demo_apk_path() {
  echo "${MATERIAL3_DEMO_DIR}/build/android/app/outputs/apk/profile/app-profile.apk"
}

# frame_sanity_check <scen_out> — for the KEPT run-*.log files under
# <scen_out> (files after the first DISCARD_FIRST, in filename order —
# same warm-up convention as stats.py's own discard), counts each kept
# run's `frust-perf raw`/`flutter-perf raw` lines (the line class
# stats.py's frame-series path parses — see stats.py's
# FRUST_RAW_PREFIX/FLUTTER_RAW_PREFIX, and run.sh's own identical
# per-run count) and refuses a degenerate sample: every kept run must
# carry >= MIN_FRAMES_PER_RUN frames AND stay within 3x of the kept-run
# median (catches a truncated/stalled capture that still produced *some*
# frames). Echoes "OK <count>/<count>/..." (kept-run order) on success, or
# "FAIL <reason>" — never fails the matrix directly (see header); the
# caller (run_scenario) turns a FAIL into a SKIPPED cell.
frame_sanity_check() {
  local scen_out="$1"
  local -a files=() counts=() sorted=()
  local f cnt n i median

  for f in "${scen_out}"/run-*.log; do
    [ -e "${f}" ] && files+=("${f}")
  done
  # run-NN.log names already sort lexicographically in capture order.
  mapfile -t files < <(printf '%s\n' "${files[@]}" | sort)
  n="${#files[@]}"
  if [ "${n}" -le "${DISCARD_FIRST}" ]; then
    echo "FAIL only ${n} run log(s) found under ${scen_out} — need more than DISCARD_FIRST=${DISCARD_FIRST} to have a kept sample"
    return 0
  fi

  for ((i = DISCARD_FIRST; i < n; i++)); do
    cnt="$(grep -c -e 'frust-perf raw' -e 'flutter-perf raw' "${files[$i]}" 2>/dev/null || true)"
    counts+=("${cnt:-0}")
  done

  mapfile -t sorted < <(printf '%s\n' "${counts[@]}" | sort -n)
  median="${sorted[$(( (${#sorted[@]} - 1) / 2 ))]}"

  for cnt in "${counts[@]}"; do
    if [ "${cnt}" -lt "${MIN_FRAMES_PER_RUN}" ]; then
      echo "FAIL kept run has ${cnt} frame(s), below MIN_FRAMES_PER_RUN=${MIN_FRAMES_PER_RUN}"
      return 0
    fi
    if [ "${median}" -gt 0 ] && { [ "${cnt}" -gt $((median * 3)) ] || [ $((cnt * 3)) -lt "${median}" ]; }; then
      echo "FAIL kept run has ${cnt} frame(s), outside 3x of kept-run median ${median}"
      return 0
    fi
  done

  echo "OK $(IFS=/; echo "${counts[*]}")"
}

# run_scenario <scenario> <scen_out_dir> <log> — drives one scenario via
# run.sh and echoes "<p50> <p95>" (bare ms numbers) on success, or
# "SKIPPED <reason>" — never fails the matrix (see header). Progress/command
# lines go to stderr; ONLY that final result line goes to stdout, so a
# caller capturing this function via `$(run_scenario ...)` gets exactly the
# result, with the progress still visible live on the terminal. A
# successful p50=/p95= parse is still subject to frame_sanity_check above
# before being trusted — a degenerate kept sample degrades the cell to
# SKIPPED rather than publishing a percentile over it (see header's "Frame
# sanity" note).
run_scenario() {
  local scenario="$1" scen_out="$2" log="$3"
  echo "-- scenario ${scenario}: run.sh --" >&2
  print_cmd "" "${SCRIPT_DIR}/run.sh ${scenario} --app frust --device ${DEVICE} --runs ${RUNS} --duration ${DURATION} --skip-device-state --out ${scen_out}" >&2
  if [ "${DRY_RUN}" -eq 1 ]; then
    echo "SKIPPED dry-run"
    return 0
  fi
  if ! "${SCRIPT_DIR}/run.sh" "${scenario}" --app frust --device "${DEVICE}" \
      --runs "${RUNS}" --duration "${DURATION}" --skip-device-state \
      --out "${scen_out}" >"${log}" 2>&1; then
    echo "SKIPPED run.sh exited non-zero for ${scenario} (see ${log})"
    return 0
  fi
  local line p50 p95
  line="$(grep -m1 'p50=' "${log}" || true)"
  if [ -z "${line}" ]; then
    echo "SKIPPED no p50=/p95= line found in run.sh output for ${scenario} (see ${log})"
    return 0
  fi
  p50="$(printf '%s\n' "${line}" | grep -oE 'p50=[0-9]+\.[0-9]+ms' | sed -E 's/p50=([0-9.]+)ms/\1/')"
  p95="$(printf '%s\n' "${line}" | grep -oE 'p95=[0-9]+\.[0-9]+ms' | sed -E 's/p95=([0-9.]+)ms/\1/')"
  if [ -z "${p50}" ] || [ -z "${p95}" ]; then
    echo "SKIPPED could not parse p50=/p95= out of: ${line}"
    return 0
  fi
  local sanity
  sanity="$(frame_sanity_check "${scen_out}")"
  if [[ "${sanity}" == FAIL* ]]; then
    echo "SKIPPED frame sanity: ${sanity#FAIL }"
    return 0
  fi
  echo "kept runs frames: ${sanity#OK }" >&2
  echo "${p50} ${p95}"
}

# run_nav <log> — drives one push/pop pass against material3-demo and
# echoes "<total_p50_ms> <submit_p95_ms> <acquire_p95_ms> <n>" on success
# (n is the last frust-perf frame summary's own frame-count field — see
# the header's "Nav-basis note"), or "SKIPPED <reason>" — never fails the
# matrix (see header). Same stdout/stderr split as run_scenario above.
#
# The full-device logcat dump never touches disk unfiltered: it is piped
# straight from `adb logcat -d -v raw` into `grep -a 'frust-perf'`, and
# only the already-sanitized result is ever written to ${log} — there is
# no intermediate file to delete or leak on any of this function's
# SKIPPED/parse-failure returns below (see the header's "no unfiltered
# dump" note). This function's other transient file, the monkey-launch
# capture at ${log}.monkey.txt, is deliberately KEPT under --out: it is
# what the SKIPPED message below points the operator at, and --out is
# transient, gitignored scratch (see the header) that no trap removes.
run_nav() {
  local log="$1" i
  echo "-- nav (material3-demo push/pop): drive --" >&2
  print_cmd "" "adb -s ${DEVICE} shell am force-stop ${MATERIAL3_DEMO_PKG}" >&2
  print_cmd "" "adb -s ${DEVICE} logcat -c" >&2
  print_cmd "" "adb -s ${DEVICE} shell monkey -p ${MATERIAL3_DEMO_PKG} -c android.intent.category.LAUNCHER 1" >&2
  print_cmd "" "sleep 1.6" >&2
  for i in 0 1 2; do
    print_cmd "" "adb -s ${DEVICE} shell input tap ${TAP_X[$i]} ${TAP_Y[$i]}" >&2
    print_cmd "" "sleep 1.6" >&2
    print_cmd "" "adb -s ${DEVICE} shell input keyevent KEYCODE_BACK" >&2
    print_cmd "" "sleep 1.6" >&2
  done
  print_cmd "" "adb -s ${DEVICE} logcat -d -v raw 2>/dev/null | grep -a 'frust-perf' > ${log}" >&2
  print_cmd "" "adb -s ${DEVICE} shell am force-stop ${MATERIAL3_DEMO_PKG}" >&2

  if [ "${DRY_RUN}" -eq 1 ]; then
    echo "SKIPPED dry-run"
    return 0
  fi

  adb -s "${DEVICE}" shell am force-stop "${MATERIAL3_DEMO_PKG}" >/dev/null 2>&1 || true
  adb -s "${DEVICE}" logcat -c
  if ! adb -s "${DEVICE}" shell monkey -p "${MATERIAL3_DEMO_PKG}" \
      -c android.intent.category.LAUNCHER 1 >"${log}.monkey.txt" 2>&1; then
    echo "SKIPPED 'monkey' launch failed for ${MATERIAL3_DEMO_PKG} (see ${log}.monkey.txt)"
    return 0
  fi
  sleep 1.6
  for i in 0 1 2; do
    adb -s "${DEVICE}" shell input tap "${TAP_X[$i]}" "${TAP_Y[$i]}" >/dev/null 2>&1
    sleep 1.6
    adb -s "${DEVICE}" shell input keyevent KEYCODE_BACK >/dev/null 2>&1
    sleep 1.6
  done
  # Piped directly — the full-device dump only ever exists between these
  # two processes, never as a standalone file (see the docstring above).
  adb -s "${DEVICE}" logcat -d -v raw 2>/dev/null | grep -a 'frust-perf' >"${log}" 2>/dev/null || true
  adb -s "${DEVICE}" shell am force-stop "${MATERIAL3_DEMO_PKG}" >/dev/null 2>&1 || true

  local line total_p50 submit_p95 acquire_p95 frame_n
  line="$(grep -a 'frust-perf frame' "${log}" 2>/dev/null | tail -n1 || true)"
  if [ -z "${line}" ]; then
    echo "SKIPPED no 'frust-perf frame' summary line found in logcat (see ${log})"
    return 0
  fi
  total_p50="$(printf '%s\n' "${line}" | grep -oE 'total_p50_ms=[0-9]+' | cut -d= -f2)"
  submit_p95="$(printf '%s\n' "${line}" | grep -oE 'submit_p95_ms=[0-9]+' | cut -d= -f2)"
  acquire_p95="$(printf '%s\n' "${line}" | grep -oE 'acquire_p95_ms=[0-9]+' | cut -d= -f2)"
  frame_n="$(printf '%s\n' "${line}" | grep -oE 'n=[0-9]+' | head -n1 | cut -d= -f2)"
  if [ -z "${total_p50}" ] || [ -z "${submit_p95}" ]; then
    echo "SKIPPED could not parse total_p50_ms=/submit_p95_ms= out of: ${line}"
    return 0
  fi
  echo "${total_p50} ${submit_p95} ${acquire_p95:-n/a} ${frame_n:-n/a}"
}

# wake_and_unlock_device — runs once, before cell 1 (Ed's rig rule; see the
# header's "Device wake/unlock/lock" note). Skipped under --dry-run (no
# device touched), but the exact commands are always printed first, so a
# --dry-run preview shows the full command sequence a live run issues,
# including the exit-time lock step (performed by cleanup(), above).
wake_and_unlock_device() {
  echo "-- device: wake/unlock --" >&2
  print_cmd "" "adb -s ${DEVICE} shell dumpsys power | grep -m1 mWakefulness" >&2
  print_cmd "" "adb -s ${DEVICE} shell input keyevent 26   # wake, only if not already Awake" >&2
  print_cmd "" "sleep 1" >&2
  print_cmd "" "adb -s ${DEVICE} shell dumpsys activity activities | grep -m1 mKeyguardShowing" >&2
  print_cmd "" "adb -s ${DEVICE} shell input keyevent 82   # dismiss the keyguard, only if one is showing" >&2
  print_cmd "" "adb -s ${DEVICE} shell input keyevent 26   # lock on exit, only if this run woke the device or dismissed its keyguard (EXIT trap)" >&2
  if [ "${DRY_RUN}" -eq 1 ]; then
    return 0
  fi
  local wakefulness keyguard
  wakefulness="$(adb -s "${DEVICE}" shell dumpsys power 2>/dev/null | grep -m1 mWakefulness || true)"
  if ! printf '%s' "${wakefulness}" | grep -q 'Awake'; then
    adb -s "${DEVICE}" shell input keyevent 26 >/dev/null 2>&1 || true
    sleep 1
    WOKE_DEVICE=1
  fi
  # The keyguard is asked about directly — a device can be Awake AND
  # sitting on its lock screen (the usual state of a rig phone the operator
  # just tapped) — so the exit-time re-lock keys off what this run actually
  # changed, not off wakefulness alone. Verified on the Pixel 5 (Android
  # 14): `mKeyguardShowing=true` asleep and on the lock screen, `=false`
  # once dismissed. An unreadable field counts as "showing": dismissing an
  # absent keyguard is a harmless menu key, while leaving a real one
  # dismissed and unlocked on exit is not.
  keyguard="$(adb -s "${DEVICE}" shell dumpsys activity activities 2>/dev/null | grep -m1 mKeyguardShowing || true)"
  if ! printf '%s' "${keyguard}" | grep -q 'mKeyguardShowing=false'; then
    adb -s "${DEVICE}" shell input keyevent 82 >/dev/null 2>&1 || true
    DISMISSED_KEYGUARD=1
  fi
}

# --- Cell enumeration ----------------------------------------------------
#
# One entry per matrix cell — one per `--tier` arm, in the order given (a
# run without --tier is the single `default` arm).
declare -a CELL_SPECS=()
for tier in "${TIER_ARR[@]}"; do
  CELL_SPECS+=("${tier}")
done
CELL_TOTAL="${#CELL_SPECS[@]}"

# cell_banner <n> <tier> — the per-cell progress heading.
cell_banner() {
  echo "=== cell $1: tier=$2 (kept ${KEPT} of ${RUNS} runs) ==="
}

# table_row_prefix <tier> — the leading identity column of a table row,
# matching emit_table's header.
table_row_prefix() {
  echo "| $1 |"
}

# --- Dry run: print every command for every cell, then exit -------------

if [ "${DRY_RUN}" -eq 1 ]; then
  echo "== ab_matrix.sh --dry-run =="
  echo "device: ${DEVICE} (never queried — --dry-run touches no device)"
  echo "device-name: ${DEVICE_NAME_DISPLAY}  raw-root: ${RAW_ROOT}"
  echo "tier: ${TIER_LIST}  (raw-series group: ${RAW_GROUP}; one cell per arm)"
  echo "scenarios: ${SCENARIOS_LIST}"
  echo "runs: ${RUNS}  duration: ${DURATION}s  frust: ${FRUST_BIN}$( [ "${QUICK}" -eq 1 ] && echo '  (--quick)' )"
  echo "kept ${KEPT} of ${RUNS} runs per scenario (first ${DISCARD_FIRST} discarded as warm-up per PROTOCOL §4)"
  if [ "${PROTOCOL_COMPLIANT}" -eq 0 ]; then
    if [ "${QUICK}" -eq 1 ] && [ "${RUNS_EXPLICIT}" -eq 0 ] && [ "${DURATION_EXPLICIT}" -eq 0 ]; then
      echo "methodology deviation: below PROTOCOL §4's >=10-kept-run x 30s convention (--quick) — restated in the emitted table's own header"
    else
      echo "methodology deviation: below PROTOCOL §4's >=10-kept-run x 30s convention (--runs/--duration below the convention) — restated in the emitted table's own header"
    fi
  fi
  echo "nav basis: ONE push/pop pass per cell (no repeated runs, nothing discarded) — nav total_p50/nav submit_p95 come from the last frust-perf frame summary's own n=<frames> window, not the kept-run accounting above"
  echo "taps: ${TAPS}"
  if [ "${TIER_AXIS}" -eq 1 ]; then
    echo "gpu_total/Graphics columns: computed per scenario from stats.py's gpu_spans/mean_graphics_mb over each cell's kept runs (n/a when no gpu_q=1 reading or no Graphics PSS snapshot parsed)"
  fi
  echo "arm-remap caveat (always in the emitted table's footer): every arm here is direct-to-surface, so the GPU render lands in submit_us and encode_us carries only the scene copy (PROTOCOL §7's v4 note) — against a pre-engine blit-arm capture, compare via the p50/p95 (total_us) / gpu_total columns instead"
  echo
  wake_and_unlock_device
  echo
  cell=0
  for spec in "${CELL_SPECS[@]}"; do
    tier="${spec}"
    cell=$((cell + 1))
    label="$(cell_label "${tier}")"
    cell_banner "${cell}" "${tier}"
    build_apk "frust_bench" "${FRUST_BENCH_DIR}" "${tier}" "/dev/null"
    install_apk "frust_bench" "$(frust_bench_apk_path)" "/dev/null"
    for scenario in "${SCENARIO_ARR[@]}"; do
      run_scenario "${scenario}" "/dev/null" "/dev/null" >/dev/null
      copy_scenario_raw "${scenario}" "${label}" "/dev/null" "/dev/null"
      if [ "${TIER_AXIS}" -eq 1 ]; then
        gpu_graphics_stats "${scenario}" "/dev/null" >/dev/null
      fi
    done
    build_apk "material3-demo" "${MATERIAL3_DEMO_DIR}" "${tier}" "/dev/null"
    install_apk "material3-demo" "$(material3_demo_apk_path)" "/dev/null"
    run_nav "/dev/null" >/dev/null
    copy_nav_raw "${label}" "/dev/null"
    echo
  done
  echo "Dry run complete — ${cell} cell(s), 0 devices touched, nothing written to disk."
  exit 0
fi

# --- Live run: drive the matrix ------------------------------------------

: >"${OUT_DIR}/ab_matrix.rows.md" || die "cannot write ${OUT_DIR}/ab_matrix.rows.md"

wake_and_unlock_device

cell=0
for spec in "${CELL_SPECS[@]}"; do
  tier="${spec}"
  cell=$((cell + 1))
  label="$(cell_label "${tier}")"
  echo
  cell_banner "${cell}" "${tier}"

  build_apk "frust_bench" "${FRUST_BENCH_DIR}" "${tier}" \
    "${OUT_DIR}/build/frust_bench-${label}.log"
  install_apk "frust_bench" "$(frust_bench_apk_path)" \
    "${OUT_DIR}/install/frust_bench-${label}.log"

  row="$(table_row_prefix "${tier}")"
  for scenario in "${SCENARIO_ARR[@]}"; do
    scen_out="${OUT_DIR}/scenarios/${scenario}-${label}"
    runsh_log="${OUT_DIR}/scenarios/${scenario}-${label}.runsh.log"
    result="$(run_scenario "${scenario}" "${scen_out}" "${runsh_log}")"
    if [[ "${result}" == SKIPPED* ]]; then
      echo "${scenario}: ${result}"
      echo "raw: not copied (SKIPPED)"
      row="${row} SKIPPED | SKIPPED |"
      if [ "${TIER_AXIS}" -eq 1 ]; then
        row="${row} n/a | n/a | n/a |"
      fi
    else
      copy_scenario_raw "${scenario}" "${label}" "${scen_out}" "${runsh_log}"
      p50="$(printf '%s' "${result}" | cut -d' ' -f1)"
      p95="$(printf '%s' "${result}" | cut -d' ' -f2)"
      row="${row} ${p50} | ${p95} |"
      if [ "${TIER_AXIS}" -eq 1 ]; then
        gpu_gfx="$(gpu_graphics_stats "${scenario}" "${scen_out}")"
        gpu_p50="$(printf '%s' "${gpu_gfx}" | cut -d' ' -f1)"
        gpu_p95="$(printf '%s' "${gpu_gfx}" | cut -d' ' -f2)"
        gpu_n="$(printf '%s' "${gpu_gfx}" | cut -d' ' -f3)"
        gfx_mb="$(printf '%s' "${gpu_gfx}" | cut -d' ' -f4)"
        gfx_n="$(printf '%s' "${gpu_gfx}" | cut -d' ' -f5)"
        echo "${scenario}: gpu_total p50=${gpu_p50}ms p95=${gpu_p95}ms (n=${gpu_n} readings)  Graphics=${gfx_mb}MB (n=${gfx_n} snapshots)"
        row="${row} ${gpu_p50} | ${gpu_p95} | ${gfx_mb} |"
      fi
    fi
  done

  build_apk "material3-demo" "${MATERIAL3_DEMO_DIR}" "${tier}" \
    "${OUT_DIR}/build/material3demo-${label}.log"
  install_apk "material3-demo" "$(material3_demo_apk_path)" \
    "${OUT_DIR}/install/material3demo-${label}.log"

  nav_log="${OUT_DIR}/nav/${label}.log"
  nav_result="$(run_nav "${nav_log}")"
  if [[ "${nav_result}" == SKIPPED* ]]; then
    echo "nav: ${nav_result}"
    echo "raw: not copied (SKIPPED)"
    row="${row} SKIPPED | SKIPPED |"
  else
    copy_nav_raw "${label}" "${nav_log}"
    nav_total_p50="$(printf '%s' "${nav_result}" | cut -d' ' -f1)"
    nav_submit_p95="$(printf '%s' "${nav_result}" | cut -d' ' -f2)"
    nav_frame_n="$(printf '%s' "${nav_result}" | cut -d' ' -f4)"
    echo "nav: total_p50=${nav_total_p50} submit_p95=${nav_submit_p95} over n=${nav_frame_n} frames"
    row="${row} ${nav_total_p50} | ${nav_submit_p95} |"
  fi

  TABLE_ROWS+=("${row}")
  # Durable the moment it exists: a later hard failure or a signal must
  # not cost the cells already measured (the EXIT trap also prints them).
  printf '%s\n' "${row}" >>"${OUT_DIR}/ab_matrix.rows.md" \
    || echo "warning: could not append the row to ${OUT_DIR}/ab_matrix.rows.md" >&2
done

# --- Emit the Markdown table ----------------------------------------------

emit_table "" | tee "${OUT_DIR}/ab_matrix.md"
MATRIX_DONE=1


echo
echo "Matrix complete — ${cell} cell(s). Table written to ${OUT_DIR}/ab_matrix.md"
echo "Transient build/install/scenario/nav logs are under ${OUT_DIR}/"
echo "Sanitized raw series copied under ${RAW_ROOT}/${DEVICE_NAME}/${RAW_GROUP}/"
