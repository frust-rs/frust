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
#                    home         bump `hotpatch-sentinel: vN` (app/src/home_page.rs, // SENTINEL-HOME)
#                    card         bump `card-sentinel: vN` (app/src/counter_card.rs, // SENTINEL-CARD)
#                    helper       add a new private fn called from HomePage::build
#                    state-type   swap HomePage's `State` (the named `HomeState`) for an (N+1)-tuple
#                                 of u32: a type-IDENTITY change (RESULTS.md row D, a silent no-op)
#                    state-field  add `extra: u32` to `struct HomeState` (same type identity, new
#                                 layout; RESULTS.md row D2). The patched build reads it into the
#                                 sentinel and logs `frust-hotpatch-spike: state-field vN build ran
#                                 extra=<v>`, so each run records whether the new build ran, the
#                                 value it read, and whether the app PID survived.
#                  A crash, an app exit or a timeout is recorded as a run result, never fatal.
#   STARTUP_TIMEOUT=<s> overrides the first-frame wait (default 900: a cold dx fat build is slow).
#   PRE_RUN_PAUSE=<s>   sleeps <s> seconds after the first frame, before the first edit, so the
#                       counter can be clicked (state-preservation check); default 0.
#   POST_RUN_PAUSE=<s>  sleeps <s> seconds after the last run, before the app is killed, so the
#                       patched window can be inspected; default 0.
#   STATE_FIELD_WRITE=1 with --target state-field, the patched build also WRITES the new field
#                       (`state.extra += 1`) before reading it; default 0 (read only).
#
# Every edited file is restored on exit (trap EXIT, Ctrl-C included) and the runner's process group
# is killed; the summary ends with `git status` of this directory as proof. The manual restore
# command (pristine copies in the log dir) is printed up front, for a SIGKILLed script. Logs live in
# a mktemp dir whose path is printed. A failed wait is recorded as a run result, never a failure.

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
PRE_RUN_PAUSE="${PRE_RUN_PAUSE:-0}"
POST_RUN_PAUSE="${POST_RUN_PAUSE:-0}"
STATE_FIELD_WRITE="${STATE_FIELD_WRITE:-0}"
TARGETS="home|card|helper|state-type|state-field"

# The comment header (line 2 up to the first non-comment line), without the `# ` prefix.
usage() {
  awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "$0"
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
      [ $# -ge 2 ] || { echo "error: --target requires ${TARGETS}" >&2; exit 2; }
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
  home|helper|state-type|state-field) EDIT_FILE="$HOME_RS" ;;
  card) EDIT_FILE="$CARD_RS" ;;
  *) echo "error: --target must be ${TARGETS}, got '$TARGET'" >&2; exit 2 ;;
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
  python3 - "$1" "$2" "$3" "$4" "$STATE_FIELD_WRITE" <<'PY'
import re, sys, time
path, target, run, orig = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
write_field = sys.argv[5] == "1"
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
elif target == "state-type":
    # A new State TYPE: the seam's call_it instance changes its generic arguments (RESULTS.md row D).
    n = run + 1
    replace_marked(src, "// STATE-TYPE",
                   lambda _: f"type State = ({', '.join(['u32'] * n)}); // STATE-TYPE")
    replace_marked(src, "// STATE-INIT", lambda _: f"({', '.join(['0'] * n)}) // STATE-INIT")
    replace_marked(src, "// STATE-READ", lambda _: "let count = state.0; // STATE-READ")
    replace_marked(src, "// STATE-INC",
                   lambda _: 'button("Increment", |state: &mut PageState| state.0 += 1) // STATE-INC')
    # `HomeState` is now unused; keep the patch warning-free.
    src.insert(next(i for i, l in enumerate(src) if l.startswith("pub struct HomeState")),
               "#[allow(dead_code)]")
    replace_marked(src, "// SENTINEL-HOME", bump)
elif target == "state-field":
    # Same State type (same def-path, same symbol), new layout (RESULTS.md row D2).
    replace_marked(src, "// STATE-FIELDS",
                   lambda l: l.replace(" // STATE-FIELDS", "") + "\n    pub extra: u32, // STATE-FIELDS")
    replace_marked(src, "// STATE-INIT",
                   lambda _: "HomeState { count: 0, extra: 4242 } // STATE-INIT")
    write = "state.extra = state.extra.wrapping_add(1); " if write_field else ""
    replace_marked(src, "// SENTINEL-HOME", lambda _: (
        f"let sentinel = {{ {write}log::info!(\"frust-hotpatch-spike: state-field v{run} build ran "
        f"extra={{}}\", state.extra); text(format!(\"hotpatch-sentinel: v{run} extra={{}}\", "
        f"state.extra)) }}; // SENTINEL-HOME"))
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

# TERM the runner's process group, wait up to 5 s, then KILL whatever is left. Every KILL is guarded:
# the group only while it still exists (the leader is not reaped until `wait` below, so its group id
# cannot be reused before that), and the scraped app PID only while it is still in the runner's
# process group (`ps -o pgid=`) — a stale or reused PID from the log is never killed.
stop_runner() {
  [ -n "$RUNNER_PID" ] || return 0
  kill -TERM -- "-${RUNNER_PID}" 2>/dev/null || kill -TERM "$RUNNER_PID" 2>/dev/null
  local i=0
  while kill -0 -- "-${RUNNER_PID}" 2>/dev/null && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
  if kill -0 -- "-${RUNNER_PID}" 2>/dev/null; then kill -KILL -- "-${RUNNER_PID}" 2>/dev/null; fi
  if [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null; then
    local pgid
    pgid="$(ps -o pgid= -p "$APP_PID" 2>/dev/null | tr -d ' ')"
    if [ "$pgid" = "$RUNNER_PID" ]; then
      kill -KILL "$APP_PID" 2>/dev/null
    else
      echo "warning: app pid ${APP_PID} is not in the runner's process group" \
        "(pgid ${pgid:-?} != ${RUNNER_PID}); not killing it" >&2
    fi
  fi
  wait "$RUNNER_PID" 2>/dev/null
  RUNNER_PID=""
  APP_PID=""
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

# The `extra=<v>` the patched state-field build of run `$2` logged after log line `$1`, waiting at
# most `$3` seconds. Prints the value, or nothing.
wait_build_line() {
  local from_line="$1" run_n="$2" deadline=$(( $(date +%s) + $3 )) v
  while :; do
    v="$(tail -n +"$((from_line + 1))" "$LOG" \
      | grep -o "frust-hotpatch-spike: state-field v${run_n} build ran extra=[0-9]*" \
      | head -1 | sed 's/.*=//')"
    if [ -n "$v" ] || [ "$(date +%s)" -ge "$deadline" ]; then echo "$v"; return 0; fi
    sleep 0.1
  done
}

# Echo log lines after `$1` that look like an app failure (panic, abort, signal, guard malloc).
trouble_lines() {
  tail -n +"$(($1 + 1))" "$LOG" \
    | grep -iE 'panick|abort|sigsegv|sigbus|sigabrt|sigill|signal [0-9]|guardmalloc|segmentation|bus error|exit (code|status)|exited|crash' \
    | tr -d '\033' | sed -e 's/\[[0-9;]*m//g' -e 's/^ */    log: /' | cut -c1-200 | head -5
}

# Process survival after a hot-patch run: is the app PID found at startup alive, and is it still the
# PID the log names last (a relaunch by dx would log a new `pid: Some(..)`)?
pid_state() {
  local latest
  latest="$(grep -o 'pid: Some([0-9]*)' "$LOG" | tail -1 | grep -o '[0-9][0-9]*')"
  if [ -z "$APP_PID" ]; then
    echo "app pid unknown"
  elif ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "app pid ${APP_PID} GONE"
  elif [ -n "$latest" ] && [ "$latest" != "$APP_PID" ]; then
    echo "app pid ${APP_PID} alive, but the log now names pid ${latest} (relaunch?)"
  else
    echo "app pid ${APP_PID} alive (same)"
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
echo "If this script dies without its EXIT trap, restore the sources in place with:"
echo "  cat '${LOG_DIR}/home_page.rs.orig' > '${HOME_RS}'" \
  "&& cat '${LOG_DIR}/counter_card.rs.orig' > '${CARD_RS}'"

START_MS="$(now_ms)"
start_runner
echo "Waiting up to ${STARTUP_TIMEOUT}s for the first frame (a cold build compiles frust and wgpu)..."
if ! FIRST_FRAME="$(wait_for frame 0 0 "$STARTUP_TIMEOUT")"; then
  echo "error: no first 'frust-hotpatch: frame' line; see ${LOG}" >&2
  exit 1
fi
find_app_pid
echo "First frame after $((FIRST_FRAME - START_MS)) ms (app pid ${APP_PID:-?})."
if [ "$PRE_RUN_PAUSE" != "0" ]; then
  echo "Pausing ${PRE_RUN_PAUSE}s before the first edit (PRE_RUN_PAUSE)..."
  sleep "$PRE_RUN_PAUSE"
fi

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
    start_runner
    frame="$(wait_for frame "$from" "$stamp" "$EDIT_TIMEOUT")" || status="timeout"
    find_app_pid
  else
    applied="$(wait_for applied "$from" "$stamp" "$EDIT_TIMEOUT")" || status="timeout"
    if [ "$status" = "ok" ]; then
      frame="$(wait_for frame "$from" "$stamp" "$EDIT_TIMEOUT")" || status="timeout"
    fi
    extra=""
    if [ "$TARGET" = "state-field" ]; then
      # Did the PATCHED build run? Only it logs this line (the edit adds it); allow 2 s after frame.
      extra="$(wait_build_line "$from" "$run" 2)"
    fi
    if [ -n "$APP_PID" ] && ! kill -0 "$APP_PID" 2>/dev/null; then status="crashed"; fi
    tail -n +"$((from + 1))" "$LOG" \
      | grep -E 'Patch rebuild:|replaying crates:|Hot-patching:|Build failed|Full rebuild' \
      | tr -d '\033' | sed -e 's/\[[0-9;]*m//g' -e 's/^ */    dx: /' | cut -c1-200
    trouble_lines "$from"
  fi
  a_delta="n/a" f_delta="n/a"
  [ -n "$applied" ] && { a_delta=$((applied - stamp)); APPLIED_DELTAS="${APPLIED_DELTAS}${a_delta}"$'\n'; }
  [ -n "$frame" ] && { f_delta=$((frame - stamp)); FRAME_DELTAS="${FRAME_DELTAS}${f_delta}"$'\n'; }
  line="run ${run}: save->applied ${a_delta} ms, save->frame ${f_delta} ms (${status})"
  if [ "$MODE" = "hotpatch" ]; then line="${line}; $(pid_state)"; fi
  if [ "$TARGET" = "state-field" ] && [ "$MODE" = "hotpatch" ]; then
    if [ -n "$extra" ]; then
      line="${line}; patched build RAN, read extra=${extra}"
    else
      line="${line}; patched build NOT seen (old build kept?)"
    fi
  fi
  echo "$line"
  RESULTS="${RESULTS}${line}"$'\n'
  [ "$status" = "crashed" ] && { echo "app exited; stopping runs"; break; }
  run=$((run + 1))
done

if [ "$POST_RUN_PAUSE" != "0" ]; then
  echo "Pausing ${POST_RUN_PAUSE}s after the last run (POST_RUN_PAUSE)..."
  sleep "$POST_RUN_PAUSE"
fi

echo
echo "=== Summary: mode=${MODE} target=${TARGET} ==="
printf '%s' "$RESULTS"
echo "median save->applied: $(printf '%s' "$APPLIED_DELTAS" | grep . | median) ms"
echo "median save->frame:   $(printf '%s' "$FRAME_DELTAS" | grep . | median) ms"
