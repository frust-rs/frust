#!/usr/bin/env bash
# examples/hotpatch-spike/measure.sh — edit-to-frame timing for the hot-patch spike.
#
# Launches the spike app, waits for its first `frust-hotpatch: frame` line, then per run edits one
# source file IN PLACE, stamps the save time (unix ms) and waits for the app's next
# `frust-hotpatch: applied` / `frust-hotpatch: frame` lines stamped later than the save. Prints
# the per-run deltas and their medians. Results are recorded by hand in RESULTS.md.
#
# Usage: examples/hotpatch-spike/measure.sh [--hotpatch | --restart] [--runs <n>] [--target <t>]
#   --hotpatch     (default) `dx serve --hot-patch --platform desktop --interactive false --verbose`
#                  from runner/; DX from $DX, else `dx` on PATH (must be dioxus-cli 0.7.10).
#   --restart      Baseline: `cargo run -p hotpatch-spike`, killed and relaunched by this script
#                  after each edit (`frust run --watch` rejects this two-package workspace: see
#                  README.md). No `applied` line exists here; only the frame delta is recorded.
#   --runs <n>     Edits to measure (default: 5).
#   --target <t>   What each run edits (default: home):
#                    home    bump `hotpatch-sentinel: vN` (app/src/home_page.rs, // SENTINEL-HOME)
#                    card    bump `card-sentinel: vN` (app/src/counter_card.rs, // SENTINEL-CARD)
#                    helper  add a new private fn called from HomePage::build
#                    state   add a field to HomePage's State (expected unsupported: a crash or a
#                            timeout is recorded, not fatal)
#   STARTUP_TIMEOUT=<s> overrides the first-frame wait (default 900: a cold dx fat build is slow).
#
# Every edited file is restored on exit (trap EXIT, Ctrl-C included) and the runner's process group
# is killed; the summary ends with `git status` of this directory as proof. Logs live in a mktemp
# dir whose path is printed. A failed wait is recorded as a run result, never a script failure.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
RUNNER_DIR="${SCRIPT_DIR}/runner"
HOME_RS="${SCRIPT_DIR}/app/src/home_page.rs"
CARD_RS="${SCRIPT_DIR}/app/src/counter_card.rs"

MODE="hotpatch"
RUNS=5
TARGET="home"
EDIT_TIMEOUT=60
STARTUP_TIMEOUT="${STARTUP_TIMEOUT:-900}"

usage() {
  sed -n '2,26p' "$0"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --hotpatch) MODE="hotpatch"; shift ;;
    --restart) MODE="restart"; shift ;;
    --runs)
      [ $# -ge 2 ] || { echo "error: --runs requires a number" >&2; exit 2; }
      RUNS="$2"; shift 2 ;;
    --runs=*) RUNS="${1#--runs=}"; shift ;;
    --target)
      [ $# -ge 2 ] || { echo "error: --target requires home|card|helper|state" >&2; exit 2; }
      TARGET="$2"; shift 2 ;;
    --target=*) TARGET="${1#--target=}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 2 ;;
  esac
done

case "$RUNS" in
  ''|*[!0-9]*|0) echo "error: --runs must be a positive integer, got '$RUNS'" >&2; exit 2 ;;
esac
case "$TARGET" in
  home|helper|state) EDIT_FILE="$HOME_RS" ;;
  card) EDIT_FILE="$CARD_RS" ;;
  *) echo "error: --target must be home|card|helper|state, got '$TARGET'" >&2; exit 2 ;;
esac
command -v python3 >/dev/null 2>&1 || { echo "error: python3 is required" >&2; exit 2; }

if [ "$MODE" = "hotpatch" ]; then
  DX="${DX:-$(command -v dx 2>/dev/null || true)}"
  if [ -z "$DX" ] || [ ! -x "$DX" ]; then
    echo "error: no dx found; set DX=<path> (dioxus-cli 0.7.10, see README.md)" >&2
    exit 2
  fi
  DX_VERSION="$("$DX" --version 2>/dev/null || true)"
  case "$DX_VERSION" in
    *0.7.10*) ;;
    *) echo "warning: '$DX' reports '$DX_VERSION', not 0.7.10 (subsecond is pinned =0.7.10)" >&2 ;;
  esac
fi

TMP_ROOT="${TMPDIR:-/tmp}"
LOG_DIR="$(mktemp -d "${TMP_ROOT%/}/hotpatch-spike.XXXXXX")"
LOG="${LOG_DIR}/runner.log"
: > "$LOG"
cp "$HOME_RS" "${LOG_DIR}/home_page.rs.orig"
cp "$CARD_RS" "${LOG_DIR}/counter_card.rs.orig"
EDIT_ORIG="${LOG_DIR}/$(basename "$EDIT_FILE").orig"

RUNNER_PID=""
APP_PID=""

now_ms() {
  python3 -c 'import time;print(int(time.time()*1000))'
}

# Rewrite `$1` in place with the content of `$2` (same inode, no rename: dx watches for in-place
# modification and a temp-file rename makes it patch off stale content).
write_in_place() {
  python3 - "$1" "$2" <<'PY'
import sys
dst, src = sys.argv[1], sys.argv[2]
new = open(src).read()
with open(dst, "r+") as f:
    if f.read() != new:
        f.seek(0)
        f.write(new)
        f.truncate()
PY
}

# Apply run `$3`'s edit for target `$2` to `$1` (in place, derived from the pristine copy `$4`),
# then print the save time in unix ms.
apply_edit() {
  python3 - "$1" "$2" "$3" "$4" <<'PY'
import re, sys, time
path, target, run, orig = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
src = open(orig).read().split("\n")

def replace_marked(lines, marker, fn):
    hits = [i for i, l in enumerate(lines) if l.rstrip().endswith(marker)]
    if len(hits) != 1:
        sys.exit(f"measure.sh: expected one line marked {marker}, found {len(hits)}")
    i = hits[0]
    indent = lines[i][: len(lines[i]) - len(lines[i].lstrip())]
    lines[i] = indent + fn(lines[i].strip())

bump = lambda l: re.sub(r"v\d+\"", f'v{run}"', l, count=1)
if target == "home":
    replace_marked(src, "// SENTINEL-HOME", bump)
elif target == "card":
    replace_marked(src, "// SENTINEL-CARD", bump)
elif target == "helper":
    replace_marked(src, "// SENTINEL-HOME",
                   lambda _: f"let sentinel = text(spike_helper_{run}()); // SENTINEL-HOME")
    src[-1:] = ["",
                f"/// Added by measure.sh --target helper (run {run}).",
                f"fn spike_helper_{run}() -> String {{",
                f'    "hotpatch-helper: v{run}".to_string()',
                "}", ""]
elif target == "state":
    n = run + 1
    replace_marked(src, "// STATE-TYPE",
                   lambda _: f"type State = ({', '.join(['u32'] * n)}); // STATE-TYPE")
    replace_marked(src, "// STATE-INIT", lambda _: f"({', '.join(['0'] * n)}) // STATE-INIT")
    replace_marked(src, "// STATE-READ", lambda _: "let count = state.0; // STATE-READ")
    replace_marked(src, "// STATE-INC",
                   lambda _: 'button("Increment", |state: &mut HomeState| state.0 += 1) // STATE-INC')
    replace_marked(src, "// SENTINEL-HOME", bump)
new = "\n".join(src)
with open(path, "r+") as f:
    f.seek(0)
    f.write(new)
    f.truncate()
print(int(time.time() * 1000))
PY
}

# Start the runner in its own process group (so one kill reaches dx/cargo and the app they spawn).
start_runner() {
  set -m
  if [ "$MODE" = "hotpatch" ]; then
    (cd "$RUNNER_DIR" && exec "$DX" serve --hot-patch --platform desktop --interactive false \
      --verbose) >> "$LOG" 2>&1 < /dev/null &
  else
    (cd "$RUNNER_DIR" && exec cargo run -p hotpatch-spike) >> "$LOG" 2>&1 < /dev/null &
  fi
  RUNNER_PID=$!
  set +m
}

stop_runner() {
  [ -n "$RUNNER_PID" ] || return 0
  kill -TERM -- "-${RUNNER_PID}" 2>/dev/null || kill -TERM "$RUNNER_PID" 2>/dev/null
  local i=0
  while kill -0 "$RUNNER_PID" 2>/dev/null && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
  kill -KILL -- "-${RUNNER_PID}" 2>/dev/null || true
  if [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null; then kill -KILL "$APP_PID" 2>/dev/null; fi
  wait "$RUNNER_PID" 2>/dev/null
  RUNNER_PID=""
}

cleanup() {
  stop_runner
  write_in_place "$HOME_RS" "${LOG_DIR}/home_page.rs.orig"
  write_in_place "$CARD_RS" "${LOG_DIR}/counter_card.rs.orig"
  echo
  echo "Restored edited sources. git status (expect nothing under app/):"
  git -C "$SCRIPT_DIR" status --short -- . | grep -v '^?? target/' || echo "  (clean)"
  echo "Logs: ${LOG_DIR}"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# First `frust-hotpatch: <kind>` line after log line `$2` whose t_unix_ms exceeds `$3`, waiting at
# most `$4` seconds. Prints the t_unix_ms, or nothing on timeout / app exit.
wait_for() {
  local kind="$1" from_line="$2" after_ms="$3" timeout_s="$4"
  local deadline=$(( $(date +%s) + timeout_s )) t
  while [ "$(date +%s)" -lt "$deadline" ]; do
    t="$(tail -n +"$((from_line + 1))" "$LOG" \
      | grep -o "frust-hotpatch: ${kind} t_unix_ms=[0-9]*" | sed 's/.*=//' \
      | awk -v s="$after_ms" '$1 > s { print; exit }')"
    if [ -n "$t" ]; then echo "$t"; return 0; fi
    if [ -n "$APP_PID" ] && ! kill -0 "$APP_PID" 2>/dev/null; then return 1; fi
    if ! kill -0 "$RUNNER_PID" 2>/dev/null; then return 1; fi
    sleep 0.1
  done
  return 1
}

log_lines() {
  wc -l < "$LOG" | tr -d ' '
}

# The app's PID, once its first frame is logged: dx logs it on the devtools connection; in restart
# mode `cargo run` exec()s the binary on Unix, so it is the runner PID itself.
find_app_pid() {
  if [ "$MODE" = "hotpatch" ]; then
    APP_PID="$(grep -o 'pid: Some([0-9]*)' "$LOG" | tail -1 | grep -o '[0-9][0-9]*')"
  else
    APP_PID="$RUNNER_PID"
  fi
}

median() {
  sort -n | awk '{ v[NR] = $1 } END {
    if (NR == 0) { print "n/a"; exit }
    if (NR % 2) print v[(NR + 1) / 2]; else print int((v[NR / 2] + v[NR / 2 + 1]) / 2) }'
}

echo "hotpatch-spike measure: mode=${MODE} target=${TARGET} runs=${RUNS}"
[ "$MODE" = "hotpatch" ] && echo "dx: ${DX} (${DX_VERSION})"
echo "Logs: ${LOG_DIR}"

START_MS="$(now_ms)"
start_runner
echo "Waiting up to ${STARTUP_TIMEOUT}s for the first frame (a cold build compiles frust and wgpu)..."
if ! FIRST_FRAME="$(wait_for frame 0 0 "$STARTUP_TIMEOUT")"; then
  echo "error: no first 'frust-hotpatch: frame' line; see ${LOG}" >&2
  exit 1
fi
find_app_pid
echo "First frame after $((FIRST_FRAME - START_MS)) ms (app pid ${APP_PID:-?})."

APPLIED_DELTAS=""
FRAME_DELTAS=""
RESULTS=""
run=1
while [ "$run" -le "$RUNS" ]; do
  sleep 1
  from="$(log_lines)"
  if ! stamp="$(apply_edit "$EDIT_FILE" "$TARGET" "$run" "$EDIT_ORIG")"; then
    RESULTS="${RESULTS}run ${run}: edit failed"$'\n'
    break
  fi
  applied="" frame="" status="ok"
  if [ "$MODE" = "restart" ]; then
    stop_runner
    APP_PID=""
    start_runner
    frame="$(wait_for frame "$from" "$stamp" "$EDIT_TIMEOUT")" || status="timeout"
    find_app_pid
  else
    applied="$(wait_for applied "$from" "$stamp" "$EDIT_TIMEOUT")" || status="timeout"
    if [ "$status" = "ok" ]; then
      frame="$(wait_for frame "$from" "$stamp" "$EDIT_TIMEOUT")" || status="timeout"
    fi
    if [ -n "$APP_PID" ] && ! kill -0 "$APP_PID" 2>/dev/null; then status="crashed"; fi
    tail -n +"$((from + 1))" "$LOG" \
      | grep -E 'Patch rebuild:|replaying crates:|Hot-patching:|Build failed|Full rebuild' \
      | tr -d '\033' | sed -e 's/\[[0-9;]*m//g' -e 's/^ */    dx: /' | cut -c1-200
  fi
  a_delta="n/a" f_delta="n/a"
  [ -n "$applied" ] && { a_delta=$((applied - stamp)); APPLIED_DELTAS="${APPLIED_DELTAS}${a_delta}"$'\n'; }
  [ -n "$frame" ] && { f_delta=$((frame - stamp)); FRAME_DELTAS="${FRAME_DELTAS}${f_delta}"$'\n'; }
  line="run ${run}: save->applied ${a_delta} ms, save->frame ${f_delta} ms (${status})"
  echo "$line"
  RESULTS="${RESULTS}${line}"$'\n'
  [ "$status" = "crashed" ] && { echo "app exited; stopping runs"; break; }
  run=$((run + 1))
done

echo
echo "=== Summary: mode=${MODE} target=${TARGET} ==="
printf '%s' "$RESULTS"
echo "median save->applied: $(printf '%s' "$APPLIED_DELTAS" | grep . | median) ms"
echo "median save->frame:   $(printf '%s' "$FRAME_DELTAS" | grep . | median) ms"
