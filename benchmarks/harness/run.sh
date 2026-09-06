#!/usr/bin/env bash
# benchmarks/harness/run.sh — drive one scenario N times on one app,
# capture raw logs, and report shared statistics (see benchmarks/PROTOCOL.md
# and stats.py).
#
# Usage: run.sh <scenario> --app frust|flutter --device <serial> [--runs N]
#                [--duration <secs>] [--out <dir>] [--pkg <package>]
#                [--scheme <uri-scheme>] [--platform android|ios]
#                [--install <app-path>] [--bundle-id <id>] [--adapter <name>]
#                [--skip-device-state] [--skip-stats]
#
#   <scenario>              scenario id — the frame-class table `s1..s8`
#                           (benchmarks/PROTOCOL.md §8) or the op-latency-only
#                           `d*` DB table `d1`/`d2` (§9); passed through as the
#                           deep-link path and the marker name stats.py slices
#                           by. A `d*` id routes this script's capture-summary
#                           and stats.py invocation onto the d-class (per-op)
#                           path instead of the frame-series path — see the
#                           "d-class scenarios" note below; every other step
#                           (install/device-state/log-pull) is identical.
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
#   --adapter <name>        flutter d-class runs only: which DB adapter this
#                           run's already-installed build was compiled against
#                           — `ffi` (package:sqlite3, the engine-parity
#                           column) or `sqflite` (the ecosystem-typical
#                           column); see PROTOCOL §9.7. The names are the
#                           app's own `DB_ADAPTER` vocabulary, so the value
#                           passed here is the value the build was compiled
#                           with. This script never builds anything — it only
#                           installs/launches whatever `--install` points at
#                           (or whatever is already on-device) — but it does
#                           verify each captured run's reported `adapter=`
#                           against this label and fails the run on a
#                           mismatch, so a mislabeled build can never be
#                           published under the wrong column.
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
# --- d-class scenarios (d1/d2, benchmarks/PROTOCOL.md §9) -----------------
#
# A `d*` scenario id reuses every install/device-state-gate/log-pull step
# unchanged (install, `device_state.sh`, `am start`/`devicectl process
# launch`, logcat/console/container-file capture, PSS snapshots, cleanup) —
# none of that machinery is frame-specific. The only two things that differ:
#
#   - The per-run "captured N raw frame lines" summary line is meaningless
#     for a d-class run (it never emits `*-perf raw` frame lines) — this
#     script instead counts `*-perf op`/`*-perf plugin` per-op lines for a
#     `d*` scenario id.
#   - stats.py is invoked with `--dclass` instead of the default frame-series
#     path, so it parses PROTOCOL §7's per-op line format and prints the
#     d-class RESULTS table (§9.6: per-op p50/p95/p99 latency + ops/s)
#     instead of the frame percentile/missed-budget table.
#
# `d*`'s `--duration` still bounds the capture window the same way as an
# s-class run (PROTOCOL §9.5: a d-class run has no fixed wall-clock target of
# its own — its length is whatever the declared iteration counts take — so
# `--duration` just needs to be long enough to outlast that; it is not a
# per-op-specific concept).
#
# Flutter's two DB adapters (PROTOCOL §9.7 — `ffi`, i.e. package:sqlite3, the
# engine-parity column, vs `sqflite`, the ecosystem-typical column) are a
# **compile-time** dimension on iOS, exactly like the existing
# scenario-per-build split documented above: a physical-iPhone d-class matrix
# pass needs **two separate `flutter build ios --profile
# --dart-define=SCENARIO=<d1|d2> --dart-define=DB_ADAPTER=<ffi|sqflite>`
# builds per d-scenario** (one per adapter), each installed and driven by its
# own `run.sh <scenario> --app flutter --platform ios --install <app-path>
# --adapter <adapter>` invocation — i.e. the caller's existing
# once-per-scenario build/install/run loop just gains an inner once-per-
# adapter iteration for flutter d-class:
#
#   for scenario in d1 d2; do
#     for adapter in ffi sqflite; do
#       flutter build ios --profile --dart-define=SCENARIO=${scenario} \
#         --dart-define=DB_ADAPTER=${adapter}
#       run.sh ${scenario} --app flutter --platform ios \
#         --install <path-to-that-build.app> --adapter ${adapter}
#     done
#   done
#
# `--adapter` and `DB_ADAPTER` therefore speak one vocabulary (`ffi` |
# `sqflite`) end to end: the app rejects any other `DB_ADAPTER` value outright
# (db_adapter.dart's `selectDbAdapter`), and this script rejects any other
# `--adapter` value and then cross-checks the adapter each captured run
# actually reported on its `*-perf info` line. This script never runs the
# build loop itself; `--adapter` labels an already-installed build's output
# directory and stats.py label. Android's two adapters are runtime-selectable
# (no separate build needed) and are out of scope for this documented loop.
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
# Exit status: non-zero on a usage error, an unreachable device, a capture
# step that fails outright, or a captured d-class run whose reported adapter
# disagrees with `--adapter`; a soft fairness-gate warning
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
ADAPTER=""

usage() {
  sed -n '2,159p' "$0"
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
    --adapter)
      [ $# -ge 2 ] || { echo "error: --adapter requires a value" >&2; exit 2; }
      ADAPTER="$2"
      shift 2
      ;;
    --adapter=*)
      ADAPTER="${1#--adapter=}"
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

# The adapter vocabulary is the app's own `DB_ADAPTER` vocabulary (`ffi` for
# package:sqlite3, `sqflite` for the platform-channel column — PROTOCOL §9.7),
# so a label here always names the build that produced the numbers.
if [ -n "${ADAPTER}" ]; then
  case "${ADAPTER}" in
    ffi|sqflite) ;;
    *)
      echo "error: --adapter must be 'ffi' or 'sqflite', got '${ADAPTER}'" >&2
      exit 2
      ;;
  esac
fi

# Verify a captured d-class run actually ran the adapter `--adapter` claims.
#
# `--adapter` only labels an already-installed build, and `DB_ADAPTER` is
# baked in at build time — so without this check a stale install silently
# publishes one column's numbers under the other column's heading. The app
# emits its own adapter name once per run on the PROTOCOL §9.7 info line
# (`flutter-perf info scenario=<id> adapter=<name> sqlite_version=<v>`);
# compare that against the requested label and fail the run outright on a
# mismatch (or on a missing line — an unverifiable run is not a usable one).
# Only meaningful for flutter d-class runs with a label given; frust runs emit
# no such line.
assert_captured_adapter() {
  local log="$1" observed
  [ "${APP}" = "flutter" ] || return 0
  [ -n "${ADAPTER}" ] || return 0
  observed="$(grep -aoE 'flutter-perf info scenario=[^ ]+ adapter=[^ ]+' "${log}" 2>/dev/null \
    | head -n 1 | grep -oE 'adapter=[^ ]+' | cut -d= -f2)"
  if [ -z "${observed}" ]; then
    echo "error: ${log} carries no 'flutter-perf info ... adapter=' line — cannot" >&2
    echo "       verify it ran --adapter '${ADAPTER}'; refusing to record this run." >&2
    exit 1
  fi
  if [ "${observed}" != "${ADAPTER}" ]; then
    echo "error: adapter mismatch in ${log}: --adapter '${ADAPTER}' but the build" >&2
    echo "       reported adapter='${observed}' — the installed build was compiled" >&2
    echo "       with a different --dart-define=DB_ADAPTER. Reinstall and re-run." >&2
    exit 1
  fi
}

# d-class scenario detection (PROTOCOL §9.1's declared `d*` namespace) — a
# `d` followed by one or more digits, e.g. `d1`/`d2`. Everything else
# (`s1..s8`, any future non-`d`-prefixed id) takes the untouched s-class
# (frame-series) path — see the header's "d-class scenarios" note.
IS_DCLASS=0
if [[ "${SCENARIO}" =~ ^d[0-9]+$ ]]; then
  IS_DCLASS=1
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
    OUT_DIR="$(mktemp -d "${SCRIPT_DIR}/.runs/${APP}-${SCENARIO}${ADAPTER:+-${ADAPTER}}-XXXXXX")"
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
    if [ "${IS_DCLASS}" -eq 1 ]; then
      assert_captured_adapter "${run_log}"
      frames="$(grep -c -e 'frust-perf op' -e 'flutter-perf op' -e 'frust-perf plugin op=' -e 'flutter-perf plugin op=' -e 'frust-perf plugin scenario=' -e 'flutter-perf plugin scenario=' "${run_log}" 2>/dev/null || true)"
      echo "  captured ${run_log} (${frames:-0} per-op lines)"
    else
      frames="$(grep -c -e 'frust-perf raw' -e 'flutter-perf raw' "${run_log}" 2>/dev/null || true)"
      echo "  captured ${run_log} (${frames:-0} raw frame lines)"
    fi
  done

  echo
  echo "Captured ${RUNS} runs for ${APP}/${SCENARIO} in ${OUT_DIR}"
  if [ -n "${ADAPTER}" ]; then
    echo "  (flutter DB adapter: ${ADAPTER} — verified against each run's reported adapter=)"
  fi

  if [ "${SKIP_STATS}" -eq 0 ]; then
    echo
    if [ "${IS_DCLASS}" -eq 1 ]; then
      echo "-- stats.py --dclass (discarding first 2 runs, per protocol convention) --"
      python3 "${SCRIPT_DIR}/stats.py" --dclass --scenario "${SCENARIO}" \
        --label "${APP} ${SCENARIO}${ADAPTER:+ (${ADAPTER})}" "${RUN_LOGS[@]}"
    else
      echo "-- stats.py (discarding first 2 runs, per protocol convention) --"
      python3 "${SCRIPT_DIR}/stats.py" --scenario "${SCENARIO}" \
        --label "${APP} ${SCENARIO}" "${RUN_LOGS[@]}"
    fi
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
  OUT_DIR="$(mktemp -d "${SCRIPT_DIR}/.runs/${APP}-${SCENARIO}${ADAPTER:+-${ADAPTER}}-XXXXXX")"
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

  if [ "${IS_DCLASS}" -eq 1 ]; then
    assert_captured_adapter "${run_log}"
    op_count="$(grep -c -e 'frust-perf op' -e 'flutter-perf op' -e 'frust-perf plugin op=' -e 'flutter-perf plugin op=' -e 'frust-perf plugin scenario=' -e 'flutter-perf plugin scenario=' "${run_log}" 2>/dev/null || true)"
    echo "  captured ${run_log} (${op_count:-0} per-op lines)"
  else
    frame_count="$(grep -c -e 'frust-perf raw' -e 'flutter-perf raw' "${run_log}" 2>/dev/null || true)"
    echo "  captured ${run_log} (${frame_count:-0} raw frame lines)"
  fi
done

echo
echo "Captured ${RUNS} runs for ${APP}/${SCENARIO} in ${OUT_DIR}"
if [ -n "${ADAPTER}" ]; then
  echo "  (flutter DB adapter: ${ADAPTER} — labeling/log-dir only, see --adapter)"
fi

# --- Verify captured runs -----------------------------------------------

unique_run_log_count=$(printf '%s\n' "${RUN_LOGS[@]}" | sort -u | wc -l)
if [ "${unique_run_log_count}" -ne "${RUNS}" ]; then
  echo "error: unique run-log count (${unique_run_log_count}) != expected runs (${RUNS})" >&2
  exit 1
fi

# --- Stats ----------------------------------------------------------------

if [ "${SKIP_STATS}" -eq 0 ]; then
  echo
  if [ "${IS_DCLASS}" -eq 1 ]; then
    echo "-- stats.py --dclass (discarding first 2 runs, per protocol convention) --"
    python3 "${SCRIPT_DIR}/stats.py" --dclass --scenario "${SCENARIO}" \
      --label "${APP} ${SCENARIO}${ADAPTER:+ (${ADAPTER})}" "${RUN_LOGS[@]}"
  else
    echo "-- stats.py (discarding first 2 runs, per protocol convention) --"
    python3 "${SCRIPT_DIR}/stats.py" --scenario "${SCENARIO}" --label "${APP} ${SCENARIO}" "${RUN_LOGS[@]}"
  fi
else
  echo "note: --skip-stats given — run stats.py yourself over ${OUT_DIR}/run-*.log"
fi
