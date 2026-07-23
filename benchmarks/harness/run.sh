#!/usr/bin/env bash
# benchmarks/harness/run.sh — drive one scenario N times on one app,
# capture raw logs, and report shared statistics (see benchmarks/PROTOCOL.md
# and stats.py).
#
# Usage: run.sh <scenario> --app frust|flutter --device <serial> [--runs N]
#                [--duration <secs>] [--out <dir>] [--pkg <package>]
#                [--scheme <uri-scheme>] [--platform android|ios]
#                [--install <app-path>] [--bundle-id <id>]
#                [--skip-device-state] [--skip-stats]
#
#   <scenario>              scenario id (e.g. s1..s8 — see PLAN Phase 9.E's
#                           scenario table); passed through as the deep-link
#                           path and the marker name stats.py slices by.
#   --app frust|flutter     which bench app to drive.
#   --device <serial>       adb device serial (`adb devices`) for Android, or
#                           the iOS device UDID for `--platform ios`.
#   --platform android|ios  target platform (default android). See the iOS
#                           automation block below for the physical-iPhone
#                           (iOS 17+ / `devicectl`) capture path — it replaces
#                           the earlier manual-flow stub.
#   --install <app-path>    iOS only: `xcrun devicectl device install app`
#                           the given `.app` bundle once before the run loop
#                           (omit if already installed).
#   --bundle-id <id>        iOS only: bundle id override (default
#                           it.f0x.frustbench / it.f0x.flutterBench).
#
# --- iOS automation (--platform ios) ---------------------------------------
#
# Physical iPhone (iOS 17+), driven via `xcrun devicectl`. Per app the
# scenario-selection + trace-capture mechanism differs (both validated on an
# iPhone SE, 2026-07-20 — see benchmarks/PROTOCOL.md's device matrix):
#
#   frust:   `devicectl device process launch --console --terminate-existing
#            -e '{"FRUST_BENCH_SCENARIO":"<scn>",...}'`. The env dict is
#            delivered to the process, so `resolve_initial()` cold-selects the
#            scenario (no s1→scenario warm-switch flash), and the app's raw
#            `frust-perf`/`bench-scenario` stdout is captured live off
#            `--console`. A single signed profile build serves every scenario.
#   flutter: scenario is baked at build time (`--dart-define=SCENARIO=<scn>`,
#            one profile build per scenario — Dart `String.fromEnvironment` is
#            compile-time). Flutter's `print` routes to os_log, which neither
#            `devicectl --console` nor the (empty on iOS 17) legacy syslog
#            relay surfaces, so the bench app also writes its trace to a file
#            in its `tmp/` container (see flutter_bench/lib/bench/perf.dart)
#            that this script pulls with `devicectl device copy from
#            --domain-type appDataContainer` after each run.
#
# Both captures are grep-filtered to `*-perf`/`bench-scenario` lines only
# before landing in the run log (privacy: device console captures never enter
# git — benchmarks/raw/ is gitignored, and only perf/marker lines are kept).
# iOS has no CLI brightness/battery/thermal control; cooldown is a fixed wait
# and those environmental controls are recorded as uncontrolled in RESULTS.md.
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
PLATFORM="android"
INSTALL_PATH=""
BUNDLE_OVERRIDE=""

usage() {
  sed -n '2,87p' "$0"
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
    --platform)
      [ $# -ge 2 ] || { echo "error: --platform requires a value" >&2; exit 2; }
      PLATFORM="$2"
      shift 2
      ;;
    --platform=*)
      PLATFORM="${1#--platform=}"
      shift
      ;;
    --install)
      [ $# -ge 2 ] || { echo "error: --install requires an app path" >&2; exit 2; }
      INSTALL_PATH="$2"
      shift 2
      ;;
    --install=*)
      INSTALL_PATH="${1#--install=}"
      shift
      ;;
    --bundle-id)
      [ $# -ge 2 ] || { echo "error: --bundle-id requires a value" >&2; exit 2; }
      BUNDLE_OVERRIDE="$2"
      shift 2
      ;;
    --bundle-id=*)
      BUNDLE_OVERRIDE="${1#--bundle-id=}"
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
  echo "error: --device <serial|udid> is required" >&2
  usage >&2
  exit 2
fi

case "${PLATFORM}" in
  android|ios) ;;
  *)
    echo "error: --platform must be 'android' or 'ios', got '${PLATFORM}'" >&2
    exit 2
    ;;
esac

if ! [[ "${RUNS}" =~ ^[0-9]+$ ]] || [ "${RUNS}" -lt 1 ]; then
  echo "error: --runs must be a positive integer, got '${RUNS}'" >&2
  exit 2
fi

if ! [[ "${DURATION}" =~ ^[0-9]+$ ]] || [ "${DURATION}" -lt 1 ]; then
  echo "error: --duration must be a positive integer (seconds), got '${DURATION}'" >&2
  exit 2
fi

# --- iOS path (--platform ios): physical iPhone via devicectl -----------

ios_default_bundle() {
  if [ "${APP}" = "frust" ]; then
    echo "it.f0x.frustbench"
  else
    echo "it.f0x.flutterBench"
  fi
}

# Terminate any running instance of the app (best-effort — a not-running app
# is not an error). Xcode 26's devicectl dropped `terminate --pid-of <bundle>`
# (it now requires `--pid <pid>`), so resolve the app's pid by matching the
# running process's executable URL against the bundle's installation URL
# (`device info apps` / `device info processes` JSON output).
ios_terminate() {
  local bundle="$1" apps_json procs_json pid
  apps_json="$(mktemp)"
  procs_json="$(mktemp)"
  xcrun devicectl device info apps --device "${DEVICE}" \
    --bundle-id "${bundle}" --json-output "${apps_json}" >/dev/null 2>&1 || true
  xcrun devicectl device info processes --device "${DEVICE}" \
    --json-output "${procs_json}" >/dev/null 2>&1 || true
  pid="$(python3 - "${apps_json}" "${procs_json}" <<'PY' 2>/dev/null
import json, sys
try:
    apps = json.load(open(sys.argv[1]))["result"]["apps"]
    procs = json.load(open(sys.argv[2]))["result"]["runningProcesses"]
    url = apps[0]["url"]  # file:///private/var/containers/Bundle/Application/<UUID>/X.app/
    for p in procs:
        if p.get("executable", "").startswith(url):
            print(p["processIdentifier"])
            break
except Exception:
    pass
PY
)"
  if [ -n "${pid}" ]; then
    xcrun devicectl device process terminate --device "${DEVICE}" \
      --pid "${pid}" >/dev/null 2>&1 || true
  fi
  rm -f "${apps_json}" "${procs_json}"
}

# frust capture: launch with the scenario+trace env dict delivered to the
# process (cold scenario selection via `resolve_initial()`), attach `--console`
# to capture the app's raw `frust-perf`/`bench-scenario` stdout, hold the
# window open ${DURATION}s, then terminate. Filters to perf/marker lines only.
ios_capture_frust() {
  local bundle="$1" run_log="$2" raw env_json launch_pid
  raw="${run_log%.log}.console.txt"
  env_json="{\"FRUST_BENCH_SCENARIO\":\"${SCENARIO}\",\"FRUST_TRACE\":\"1\",\"FRUST_TRACE_RAW\":\"1\"}"
  xcrun devicectl device process launch --device "${DEVICE}" --console \
    --terminate-existing -e "${env_json}" "${bundle}" >"${raw}" 2>&1 &
  launch_pid=$!
  sleep "${DURATION}"
  ios_terminate "${bundle}"
  # Give the `--console` stream time to drain its buffered tail and exit
  # naturally once the app dies (an immediate kill truncates the capture);
  # fall back to kill only if it hasn't exited after the grace window.
  for _ in $(seq 1 15); do
    kill -0 "${launch_pid}" 2>/dev/null || break
    sleep 1
  done
  kill "${launch_pid}" >/dev/null 2>&1 || true
  wait "${launch_pid}" 2>/dev/null || true
  grep -aE 'frust-perf|flutter-perf|bench-scenario' "${raw}" >"${run_log}" 2>/dev/null || true
  rm -f "${raw}"
}

# flutter capture: the scenario is baked at build time (--dart-define), so the
# installed build already targets ${SCENARIO}; launch it, hold ${DURATION}s,
# terminate, then pull the on-device trace file the bench app wrote to its
# tmp/ container (Flutter `print` does not reach the host on iOS — see the
# header). Filters to perf/marker lines only.
ios_capture_flutter() {
  local bundle="$1" run_log="$2" raw
  raw="${run_log%.log}.container.txt"
  xcrun devicectl device process launch --device "${DEVICE}" \
    --terminate-existing "${bundle}" >/dev/null 2>&1
  sleep "${DURATION}"
  ios_terminate "${bundle}"
  # Let the bench app's ~1s-cadence flush land before pulling the file.
  sleep 2
  rm -f "${raw}"
  xcrun devicectl device copy from --device "${DEVICE}" \
    --domain-type appDataContainer --domain-identifier "${bundle}" \
    --source tmp/flutter_bench_trace.log --destination "${raw}" >/dev/null 2>&1 || true
  if [ -f "${raw}" ]; then
    grep -aE 'flutter-perf|bench-scenario' "${raw}" >"${run_log}" 2>/dev/null || true
    rm -f "${raw}"
  else
    : >"${run_log}"
  fi
}

ios_run_matrix() {
  local bundle run_log frames i
  RUN_LOGS=()

  if [ -n "${BUNDLE_OVERRIDE}" ]; then
    bundle="${BUNDLE_OVERRIDE}"
  else
    bundle="$(ios_default_bundle)"
  fi

  if ! command -v xcrun >/dev/null 2>&1; then
    echo "error: xcrun not found — Xcode command-line tools required for --platform ios" >&2
    return 1
  fi

  if ! xcrun devicectl device info details --device "${DEVICE}" >/dev/null 2>&1; then
    echo "error: iOS device '${DEVICE}' not reachable via devicectl — check pairing/trust/Developer Mode" >&2
    return 1
  fi

  if [ -n "${INSTALL_PATH}" ]; then
    echo "Installing ${INSTALL_PATH} on ${DEVICE} ..."
    if ! xcrun devicectl device install app --device "${DEVICE}" "${INSTALL_PATH}" >/dev/null; then
      echo "error: 'devicectl device install app' failed for ${INSTALL_PATH}" >&2
      return 1
    fi
  fi

  if [ -z "${OUT_DIR}" ]; then
    mkdir -p "${SCRIPT_DIR}/.runs"
    OUT_DIR="$(mktemp -d "${SCRIPT_DIR}/.runs/${APP}-${SCENARIO}-XXXXXX")"
  else
    mkdir -p "${OUT_DIR}"
  fi
  echo "Output directory: ${OUT_DIR}"

  if [ "${SKIP_DEVICE_STATE}" -eq 0 ]; then
    echo "note: iOS has no CLI brightness/battery/thermal gate (device_state.sh" >&2
    echo "      is Android-only) — those controls are uncontrolled on this run;" >&2
    echo "      cooldown is a fixed inter-block wait. See RESULTS.md deviations." >&2
  fi

  for i in $(seq 1 "${RUNS}"); do
    echo "== ${APP} / ${SCENARIO} / run ${i} of ${RUNS} (ios) =="
    # Deterministic run-log names (avoids BSD mktemp's no-suffix-after-XXXX
    # limitation — OUT_DIR already scopes uniqueness per invocation).
    run_log="${OUT_DIR}/run-$(printf '%02d' "${i}").log"
    RUN_LOGS+=("${run_log}")
    if [ "${APP}" = "frust" ]; then
      ios_capture_frust "${bundle}" "${run_log}"
    else
      ios_capture_flutter "${bundle}" "${run_log}"
    fi
    frames="$(grep -c -e 'frust-perf raw' -e 'flutter-perf raw' "${run_log}" 2>/dev/null || true)"
    echo "  captured ${run_log} (${frames:-0} raw frame lines)"
  done

  echo
  echo "Captured ${RUNS} runs for ${APP}/${SCENARIO} in ${OUT_DIR}"

  if [ "${SKIP_STATS}" -eq 0 ]; then
    echo
    echo "-- stats.py (discarding first 2 runs, per protocol convention) --"
    python3 "${SCRIPT_DIR}/stats.py" --scenario "${SCENARIO}" \
      --label "${APP} ${SCENARIO}" "${RUN_LOGS[@]}"
  else
    echo "note: --skip-stats given — run stats.py yourself over ${OUT_DIR}/run-*.log"
  fi
}

if [ "${PLATFORM}" = "ios" ]; then
  ios_run_matrix
  exit $?
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

  run_log="${OUT_DIR}/run-$(printf '%02d' "${i}").log"
  if [ -f "${run_log}" ]; then
    echo "error: run log already exists: ${run_log} — collision/stale file detected" >&2
    exit 1
  fi
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
  adb -s "${DEVICE}" logcat -v raw >"${run_log}.unfiltered" 2>/dev/null &
  logcat_pid=$!
  sleep "${DURATION}"
  kill "${logcat_pid}" >/dev/null 2>&1 || true
  wait "${logcat_pid}" 2>/dev/null || true
  # Sanitize before anything persists: full-device logcat can carry other
  # apps'/system lines (a privacy leak if committed). Keep ONLY the
  # perf/marker lines stats.py parses — same whitelist the iOS capture
  # paths already apply — so runN.log is always safe to commit to git.
  grep -aE 'frust-perf|flutter-perf|bench-scenario' "${run_log}.unfiltered" \
    >"${run_log}" 2>/dev/null || : >"${run_log}"
  rm -f "${run_log}.unfiltered"

  adb_shell dumpsys meminfo "${PKG}" >"${pss_after}" 2>/dev/null \
    || echo "note: could not capture post-run PSS — non-fatal" >&2

  adb_shell am force-stop "${PKG}" >/dev/null 2>&1 || true

  frame_count="$(grep -c -e 'frust-perf raw' -e 'flutter-perf raw' "${run_log}" 2>/dev/null || true)"
  echo "  captured ${run_log} (${frame_count:-0} raw frame lines)"
done

echo
echo "Captured ${RUNS} runs for ${APP}/${SCENARIO} in ${OUT_DIR}"

# --- Verify captured runs -----------------------------------------------

unique_run_log_count=$(printf '%s\n' "${RUN_LOGS[@]}" | sort -u | wc -l)
if [ "${unique_run_log_count}" -ne "${RUNS}" ]; then
  echo "error: unique run-log count (${unique_run_log_count}) != expected runs (${RUNS})" >&2
  exit 1
fi

# --- Stats ----------------------------------------------------------------

if [ "${SKIP_STATS}" -eq 0 ]; then
  echo
  echo "-- stats.py (discarding first 2 runs, per protocol convention) --"
  python3 "${SCRIPT_DIR}/stats.py" --scenario "${SCENARIO}" --label "${APP} ${SCENARIO}" "${RUN_LOGS[@]}"
else
  echo "note: --skip-stats given — run stats.py yourself over ${OUT_DIR}/run-*.log"
fi
