#!/usr/bin/env bash
# benchmarks/harness/run.sh — drive one scenario N times on one app,
# capture raw logs, and report shared statistics (see benchmarks/PROTOCOL.md
# and stats.py).
#
# Usage: run.sh <scenario> --app frust|flutter --device <serial> [--runs N]
#                [--duration <secs>] [--out <dir>] [--pkg <package>]
#                [--scheme <uri-scheme>] [--skip-device-state]
#                [--skip-stats]
#
#   <scenario>              scenario id (e.g. s1..s8 — see PLAN Phase 9.E's
#                           scenario table); passed through as the deep-link
#                           path and the marker name stats.py slices by.
#   --app frust|flutter     which bench app to drive.
#   --device <serial>       adb device serial (`adb devices`), or the
#                           literal string `ios` to print the manual iOS
#                           capture flow instead of driving anything (iOS
#                           automation is a declared non-goal this phase —
#                           see PLAN Future Enhancements).
#   --runs N                number of runs to capture (default: 12 — enough
#                           for the protocol's "first 2 discarded, >=10 kept"
#                           convention with zero extra headroom removed).
#   --duration <secs>       capture window per run, in seconds (default: 30,
#                           the protocol's per-scenario run length).
#   --out <dir>             output directory for raw per-run logs and PSS
#                           snapshots (default: a fresh mktemp -d under
#                           benchmarks/harness/.runs/).
#   --pkg <package>         override the bench app's package id (default:
#                           it.f0x.frustbench / it.f0x.flutterbench).
#   --scheme <uri-scheme>   override the deep-link URI scheme (default:
#                           frustbench / flutterbench).
#   --skip-device-state     skip the device_state.sh fairness gate (for a
#                           quick smoke run; never for a recorded result).
#   --skip-stats            capture raw logs only; don't invoke stats.py
#                           afterward (useful when driving both apps
#                           yourself for an interleaved A/B session and
#                           running stats.py once at the end over the
#                           combined file set).
#
# Fairness note (protocol, see benchmarks/PROTOCOL.md): when comparing two
# apps for the same scenario, alternate `run.sh` invocations between them
# (frust run 1, flutter run 1, frust run 2, flutter run 2, ...) rather than
# running all of one app's N runs before starting the other's — this is
# what controls for thermal/battery drift across a benchmarking session.
# `run.sh` itself drives exactly one app per invocation; the interleaving
# is the caller's loop (a matrix-runner wrapper, or by hand) — this keeps a
# single `run.sh` call's failure/retry blast radius to one app's one
# scenario.
#
# Exit status: non-zero on a usage error, an unreachable device, or a
# capture step that fails outright; a soft fairness-gate warning
# (device_state.sh's airplane-mode/charger checks) never stops a run — see
# device_state.sh's own header.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"

SCENARIO=""
APP=""
DEVICE=""
RUNS=12
DURATION=30
OUT_DIR=""
PKG_OVERRIDE=""
SCHEME_OVERRIDE=""
SKIP_DEVICE_STATE=0
SKIP_STATS=0

usage() {
  sed -n '2,53p' "$0"
}

# --- Arg parsing ------------------------------------------------------

if [ $# -eq 0 ]; then
  usage
  exit 2
fi

# First positional (non-flag) argument is the scenario id.
if [[ "$1" != -* ]]; then
  SCENARIO="$1"
  shift
fi

while [ $# -gt 0 ]; do
  case "$1" in
    --app)
      [ $# -ge 2 ] || { echo "error: --app requires a value" >&2; exit 2; }
      APP="$2"
      shift 2
      ;;
    --app=*)
      APP="${1#--app=}"
      shift
      ;;
    --device)
      [ $# -ge 2 ] || { echo "error: --device requires a value" >&2; exit 2; }
      DEVICE="$2"
      shift 2
      ;;
    --device=*)
      DEVICE="${1#--device=}"
      shift
      ;;
    --runs)
      [ $# -ge 2 ] || { echo "error: --runs requires a value" >&2; exit 2; }
      RUNS="$2"
      shift 2
      ;;
    --runs=*)
      RUNS="${1#--runs=}"
      shift
      ;;
    --duration)
      [ $# -ge 2 ] || { echo "error: --duration requires a value" >&2; exit 2; }
      DURATION="$2"
      shift 2
      ;;
    --duration=*)
      DURATION="${1#--duration=}"
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
    --pkg)
      [ $# -ge 2 ] || { echo "error: --pkg requires a value" >&2; exit 2; }
      PKG_OVERRIDE="$2"
      shift 2
      ;;
    --pkg=*)
      PKG_OVERRIDE="${1#--pkg=}"
      shift
      ;;
    --scheme)
      [ $# -ge 2 ] || { echo "error: --scheme requires a value" >&2; exit 2; }
      SCHEME_OVERRIDE="$2"
      shift 2
      ;;
    --scheme=*)
      SCHEME_OVERRIDE="${1#--scheme=}"
      shift
      ;;
    --skip-device-state)
      SKIP_DEVICE_STATE=1
      shift
      ;;
    --skip-stats)
      SKIP_STATS=1
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

if [ -z "${SCENARIO}" ]; then
  echo "error: <scenario> is required (e.g. s1)" >&2
  usage >&2
  exit 2
fi

case "${APP}" in
  frust|flutter) ;;
  "")
    echo "error: --app frust|flutter is required" >&2
    usage >&2
    exit 2
    ;;
  *)
    echo "error: --app must be 'frust' or 'flutter', got '${APP}'" >&2
    exit 2
    ;;
esac

if [ -z "${DEVICE}" ]; then
  echo "error: --device <serial> (or --device ios) is required" >&2
  usage >&2
  exit 2
fi

if ! [[ "${RUNS}" =~ ^[0-9]+$ ]] || [ "${RUNS}" -lt 1 ]; then
  echo "error: --runs must be a positive integer, got '${RUNS}'" >&2
  exit 2
fi

if ! [[ "${DURATION}" =~ ^[0-9]+$ ]] || [ "${DURATION}" -lt 1 ]; then
  echo "error: --duration must be a positive integer (seconds), got '${DURATION}'" >&2
  exit 2
fi

# --- iOS stub: document the manual capture flow, drive nothing ---------

if [ "${DEVICE}" = "ios" ]; then
  cat <<EOF
iOS automation is a declared non-goal this phase (see PLAN Phase 9.E's
Future Enhancements) — capture manually instead:

1. Install the ${APP} bench app on the device/Simulator (Xcode run, or
   'frust run -d <udid>' for the frust side).
2. Launch the scenario via its deep link:
     - frust:   frustbench://${SCENARIO}
     - flutter: flutterbench://${SCENARIO}
   (Simulator: 'xcrun simctl openurl booted "<scheme>://${SCENARIO}"';
   physical device: tap a link, or use the app's own scenario-select UI.)
3. Stream the console for the capture window and redirect it to a file:
     - Physical device (iOS 17+): 'xcrun devicectl device console --device
       <udid> > run.log' (Ctrl-C after ${DURATION}s+).
     - Simulator: 'xcrun simctl launch --console-pty booted <bundle-id>
       > run.log' captures stdout/stderr directly from launch.
4. Confirm 'bench-scenario-start ${SCENARIO}' / 'bench-scenario-end
   ${SCENARIO}' markers and 'frust-perf raw'/'flutter-perf raw' lines
   appear in run.log (frust: only under FRUST_TRACE=1 FRUST_TRACE_RAW=1 —
   set via --define at build/run time).
5. Repeat for ${RUNS} runs (protocol: discard the first 2), then run:
     python3 ${SCRIPT_DIR}/stats.py --scenario ${SCENARIO} run1.log run2.log ...
EOF
  exit 0
fi

# --- Package / deep-link scheme -----------------------------------------

if [ -n "${PKG_OVERRIDE}" ]; then
  PKG="${PKG_OVERRIDE}"
elif [ "${APP}" = "frust" ]; then
  PKG="it.f0x.frustbench"
else
  PKG="it.f0x.flutterbench"
fi

if [ -n "${SCHEME_OVERRIDE}" ]; then
  SCHEME="${SCHEME_OVERRIDE}"
elif [ "${APP}" = "frust" ]; then
  SCHEME="frustbench"
else
  SCHEME="flutterbench"
fi

if ! command -v adb >/dev/null 2>&1; then
  echo "error: adb not found on PATH — Android SDK platform-tools required" >&2
  exit 1
fi

if ! adb -s "${DEVICE}" get-state >/dev/null 2>&1; then
  echo "error: device '${DEVICE}' not reachable (adb get-state failed) — check \`adb devices\`" >&2
  exit 1
fi

# --- Output directory (unique per invocation; never a fixed /tmp path) --

if [ -z "${OUT_DIR}" ]; then
  mkdir -p "${SCRIPT_DIR}/.runs"
  OUT_DIR="$(mktemp -d "${SCRIPT_DIR}/.runs/${APP}-${SCENARIO}-XXXXXX")"
else
  mkdir -p "${OUT_DIR}"
fi
echo "Output directory: ${OUT_DIR}"

# --- Device-state fairness gate -----------------------------------------

if [ "${SKIP_DEVICE_STATE}" -eq 0 ]; then
  "${SCRIPT_DIR}/device_state.sh" --device "${DEVICE}"
else
  echo "note: --skip-device-state given — fairness preconditions NOT verified for this run" >&2
fi

# --- Capture loop --------------------------------------------------------

RUN_LOGS=()

adb_shell() {
  adb -s "${DEVICE}" shell "$@"
}

# Restore device power/battery state on any exit (normal, error, or Ctrl-C):
# undo the keep-awake override and clear any battery spoof so the device
# returns to its real charge/timeout behavior after the session. Best-effort —
# a lost device must not turn cleanup into a hard failure.
cleanup() {
  adb -s "${DEVICE}" shell svc power stayon false >/dev/null 2>&1 || true
  adb -s "${DEVICE}" shell dumpsys battery reset >/dev/null 2>&1 || true
}
trap cleanup EXIT

for i in $(seq 1 "${RUNS}"); do
  echo "== ${APP} / ${SCENARIO} / run ${i} of ${RUNS} =="

  run_log="$(mktemp "${OUT_DIR}/run-XXXXXX.log")"
  pss_before="${run_log%.log}.pss_before.txt"
  pss_after="${run_log%.log}.pss_after.txt"
  RUN_LOGS+=("${run_log}")

  adb_shell am force-stop "${PKG}" >/dev/null 2>&1 || true
  adb -s "${DEVICE}" logcat -c

  # Keep the screen on and awake for this run. The screen would otherwise
  # sleep during the inter-run gap (or the per-scenario switch) and lock the
  # device mid-capture — blanking the scenario and starving the raw trace;
  # `stayon true` disables the screen-off timeout and `KEYCODE_WAKEUP` turns
  # a already-off screen back on. Restored to `stayon false` in cleanup().
  adb_shell svc power stayon true >/dev/null 2>&1 || true
  adb_shell input keyevent KEYCODE_WAKEUP >/dev/null 2>&1 || true

  adb_shell dumpsys meminfo "${PKG}" >"${pss_before}" 2>/dev/null \
    || echo "note: could not capture pre-run PSS (app likely not yet running) — non-fatal" >&2

  adb_shell am start -a android.intent.action.VIEW -d "${SCHEME}://${SCENARIO}" "${PKG}" >/dev/null \
    || { echo "error: 'am start' failed for ${PKG} (${SCHEME}://${SCENARIO})" >&2; exit 1; }

  # Capture the scenario window's logcat in the background, then stop it —
  # `logcat -v raw` emits bare messages (no timestamp/pid/tag), matching
  # what a Rust `log::info!`/Dart print call actually wrote, which is what
  # stats.py's raw-line parser expects to find.
  adb -s "${DEVICE}" logcat -v raw >"${run_log}" 2>/dev/null &
  logcat_pid=$!
  sleep "${DURATION}"
  kill "${logcat_pid}" >/dev/null 2>&1 || true
  wait "${logcat_pid}" 2>/dev/null || true

  adb_shell dumpsys meminfo "${PKG}" >"${pss_after}" 2>/dev/null \
    || echo "note: could not capture post-run PSS — non-fatal" >&2

  adb_shell am force-stop "${PKG}" >/dev/null 2>&1 || true

  frame_count="$(grep -c -e 'frust-perf raw' -e 'flutter-perf raw' "${run_log}" 2>/dev/null || true)"
  echo "  captured ${run_log} (${frame_count:-0} raw frame lines)"
done

echo
echo "Captured ${RUNS} runs for ${APP}/${SCENARIO} in ${OUT_DIR}"

# --- Stats ----------------------------------------------------------------

if [ "${SKIP_STATS}" -eq 0 ]; then
  echo
  echo "-- stats.py (discarding first 2 runs, per protocol convention) --"
  python3 "${SCRIPT_DIR}/stats.py" --scenario "${SCENARIO}" --label "${APP} ${SCENARIO}" "${RUN_LOGS[@]}"
else
  echo "note: --skip-stats given — run stats.py yourself over ${OUT_DIR}/run-*.log"
fi
