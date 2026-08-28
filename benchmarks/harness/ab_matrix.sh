#!/usr/bin/env bash
# benchmarks/harness/ab_matrix.sh — Phase-3 fine-floor A/B device gate, one
# command: drives the full FRUST_AA_MODE x FRUST_RENDER_SCALE matrix across
# benchmarks/frust_bench's frame scenarios AND examples/material3-demo's
# push/pop nav column, and emits the table benchmarks/RESULTS.md's "Fine-floor
# A/B" section needs (plan fplan_000001a03fce41148yUkciag, phase
# pph_000001a04171930dcLOuu2jm — the FRUST_AA_MODE and FRUST_RENDER_SCALE
# knob cards).
#
# For every (aa, scale) cell in the matrix, in this order:
#
#   1. Build benchmarks/frust_bench (from its own directory — it is a
#      standalone workspace, see the repo root CLAUDE.md):
#        <frust> build apk --profile --define FRUST_TRACE_RAW=1 \
#          --define FRUST_AA_MODE=<aa> --define FRUST_RENDER_SCALE=<scale>
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
#   4. Build examples/material3-demo (its own standalone workspace) with the
#      identical three defines, install it the same way, then drive ONE
#      push/pop pass — the fine-floor research recipe: `am force-stop`,
#      launch via `monkey -p it.f0x.material3demo -c
#      android.intent.category.LAUNCHER 1`, three `input tap <x> <y>` /
#      `keyevent KEYCODE_BACK` pairs 1.6s apart (`--taps`, below) — then pull
#      the LAST `frust-perf frame` summary line off logcat and read its
#      `total_p50_ms=`/`submit_p95_ms=`/`acquire_p95_ms=` fields (see
#      `crates/frust-shell-common/src/perf.rs`'s `FrameStats::emit_log`).
#
# Emits a Markdown table — columns `aa`, `scale`, one p50/p95 column pair per
# `--scenarios` entry, `nav total_p50`, `nav submit_p95` — to stdout and to
# `<out>/ab_matrix.md`. Raw build/install/logcat output lives under
# `benchmarks/harness/.runs/` (already gitignored) for inspection; nothing
# under `benchmarks/raw/` is written by this script.
#
# Usage: ab_matrix.sh --device <serial> [--aa area,msaa8,msaa16]
#                      [--scale 1.0,0.75,0.5] [--scenarios s1,s2,s4]
#                      [--runs 3] [--duration 20] [--frust <path>]
#                      [--taps 540,472,540,734,540,996] [--out <dir>]
#                      [--dry-run]
#
#   --device <serial>  adb device serial (`adb devices`) — required even
#                      under --dry-run (a dry run still prints the exact
#                      `adb -s <serial> ...` command lines it would issue).
#   --aa <csv>         FRUST_AA_MODE values to sweep (default:
#                      area,msaa8,msaa16 — the three values
#                      `frust-render/src/context.rs`'s `parse_aa_mode`
#                      recognizes; case-insensitive, validated up front).
#   --scale <csv>      FRUST_RENDER_SCALE values to sweep (default:
#                      1.0,0.75,0.5). Under scale<1 the snapshot-layer cache
#                      is disabled BY DESIGN (a scaled intermediate would
#                      need every cached page's quads/scissors/raster
#                      rescaled — see `renderer.rs`'s `FRUST_RENDER_SCALE
#                      refuses on its own` note), so the nav column's
#                      numbers at scale<1 measure the inline (uncached)
#                      path, not the cached one — the emitted table repeats
#                      this in its footer.
#   --scenarios <csv>  benchmarks/frust_bench scenario ids to run per cell,
#                      passed through to run.sh (default: s1,s2,s4).
#   --runs <n>         runs per scenario, passed to run.sh (default: 3 — a
#                      quick pass, below PROTOCOL §4's >=10-run convention;
#                      see the RESULTS.md deviations note it produces).
#   --duration <secs>  capture window per run, passed to run.sh (default:
#                      20 — below PROTOCOL §4's 30s convention; quick pass).
#   --frust <path>     path to the frust CLI binary (default:
#                      <repo-root>/target/debug/frust).
#   --taps <csv>       six comma-separated integers x1,y1,x2,y2,x3,y3 — the
#                      three tap points for the nav push/pop recipe
#                      (default: 540,472,540,734,540,996 — the fine-floor
#                      research recipe's coordinates, verified to land on
#                      the same list rows on both the Pixel 5a and the
#                      Pixel 5 at 2.75x density).
#   --out <dir>        output directory for the emitted table + raw logs
#                      (default: a fresh mktemp -d under
#                      benchmarks/harness/.runs/). Ignored under --dry-run
#                      (nothing is written to disk).
#   --dry-run          print every command this script would run, for every
#                      cell of the matrix, and exit 0 — no build, no adb, no
#                      filesystem write, no device touched.
#
# Never skips a cell silently: a missing required tool (adb, python3, the
# frust CLI binary), an unreachable device, or a build failure is a loud,
# non-zero-exit failure of the whole matrix run. A single scenario or nav
# measurement that can't be parsed out of an otherwise-successful run's
# output instead degrades that one table cell to `SKIPPED <reason>` and the
# matrix continues — never a placeholder number.
#
# Exit status: non-zero on a usage error, a missing required tool, an
# unreachable device, or a build/install failure; 0 otherwise (individual
# `SKIPPED` cells do not fail the run).

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." >/dev/null 2>&1 && pwd)"

DEVICE=""
AA_LIST="area,msaa8,msaa16"
SCALE_LIST="1.0,0.75,0.5"
SCENARIOS_LIST="s1,s2,s4"
RUNS=3
DURATION=20
FRUST_BIN="${REPO_ROOT}/target/debug/frust"
TAPS="540,472,540,734,540,996"
OUT_DIR=""
DRY_RUN=0

FRUST_BENCH_DIR="${REPO_ROOT}/benchmarks/frust_bench"
MATERIAL3_DEMO_DIR="${REPO_ROOT}/examples/material3-demo"
MATERIAL3_DEMO_PKG="it.f0x.material3demo"

usage() {
  sed -n '2,96p' "$0"
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
    --aa)
      [ $# -ge 2 ] || { echo "error: --aa requires a value" >&2; exit 2; }
      AA_LIST="$2"
      shift 2
      ;;
    --aa=*)
      AA_LIST="${1#--aa=}"
      shift
      ;;
    --scale)
      [ $# -ge 2 ] || { echo "error: --scale requires a value" >&2; exit 2; }
      SCALE_LIST="$2"
      shift 2
      ;;
    --scale=*)
      SCALE_LIST="${1#--scale=}"
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
    --frust)
      [ $# -ge 2 ] || { echo "error: --frust requires a path" >&2; exit 2; }
      FRUST_BIN="$2"
      shift 2
      ;;
    --frust=*)
      FRUST_BIN="${1#--frust=}"
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

if ! [[ "${RUNS}" =~ ^[0-9]+$ ]] || [ "${RUNS}" -lt 1 ]; then
  echo "error: --runs must be a positive integer, got '${RUNS}'" >&2
  exit 2
fi

if ! [[ "${DURATION}" =~ ^[0-9]+$ ]] || [ "${DURATION}" -lt 1 ]; then
  echo "error: --duration must be a positive integer (seconds), got '${DURATION}'" >&2
  exit 2
fi

# --- Split CSV inputs into arrays --------------------------------------

IFS=',' read -r -a AA_ARR <<<"${AA_LIST}"
IFS=',' read -r -a SCALE_ARR <<<"${SCALE_LIST}"
IFS=',' read -r -a SCENARIO_ARR <<<"${SCENARIOS_LIST}"

if [ "${#AA_ARR[@]}" -eq 0 ]; then
  echo "error: --aa produced an empty list" >&2
  exit 2
fi
if [ "${#SCALE_ARR[@]}" -eq 0 ]; then
  echo "error: --scale produced an empty list" >&2
  exit 2
fi
if [ "${#SCENARIO_ARR[@]}" -eq 0 ]; then
  echo "error: --scenarios produced an empty list" >&2
  exit 2
fi

for aa in "${AA_ARR[@]}"; do
  case "$(printf '%s' "${aa}" | tr '[:upper:]' '[:lower:]')" in
    area|msaa8|msaa16) ;;
    *)
      echo "error: --aa value '${aa}' is not one of area, msaa8, msaa16" >&2
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

# --- Tool checks (skipped under --dry-run — see the header) -------------
#
# A dry run never shells out to adb/python3/the frust CLI, so a preview of
# the command list works on a box that has none of them installed yet; a
# live run needs all three, and fails loudly (not a SKIPPED cell) if one is
# missing — see the header's "never skips a cell silently" contract.
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

# --- Output directory (real runs only — a dry run writes nothing) -------

if [ "${DRY_RUN}" -eq 0 ]; then
  if [ -z "${OUT_DIR}" ]; then
    mkdir -p "${SCRIPT_DIR}/.runs"
    OUT_DIR="$(mktemp -d "${SCRIPT_DIR}/.runs/ab_matrix-XXXXXX")"
  else
    mkdir -p "${OUT_DIR}"
  fi
  mkdir -p "${OUT_DIR}/build" "${OUT_DIR}/install" "${OUT_DIR}/nav" "${OUT_DIR}/scenarios"
  echo "Output directory: ${OUT_DIR}"
fi

# --- Helpers -------------------------------------------------------------

# print_cmd <label> <args...> — echo one planned/executed command line,
# identically under --dry-run and a live run (so a --dry-run's output and a
# live run's own progress log read the same way).
print_cmd() {
  local label="$1"
  shift
  echo "  ${label}$*"
}

# build_apk <label> <app-dir> <aa> <scale> <log> — runs (or, under
# --dry-run, only prints) `<frust> build apk --profile --define ...` from
# <app-dir>. A build failure is a hard, loud failure of the whole matrix —
# never a SKIPPED cell (see header).
build_apk() {
  local label="$1" app_dir="$2" aa="$3" scale="$4" log="$5"
  echo "-- ${label}: build (FRUST_AA_MODE=${aa} FRUST_RENDER_SCALE=${scale}) --"
  print_cmd "(cd ${app_dir} && " "${FRUST_BIN} build apk --profile --define FRUST_TRACE_RAW=1 --define FRUST_AA_MODE=${aa} --define FRUST_RENDER_SCALE=${scale})"
  if [ "${DRY_RUN}" -eq 1 ]; then
    return 0
  fi
  if ! ( cd "${app_dir}" && "${FRUST_BIN}" build apk --profile \
      --define "FRUST_TRACE_RAW=1" \
      --define "FRUST_AA_MODE=${aa}" \
      --define "FRUST_RENDER_SCALE=${scale}" ) >"${log}" 2>&1; then
    echo "error: ${label} build failed (aa=${aa} scale=${scale}) — see ${log}" >&2
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
# build-type's default universal-APK output path (mirrors the `release`
# path app_size.sh already reports, e.g.
# .../outputs/apk/release/app-release.apk).
frust_bench_apk_path() {
  echo "${FRUST_BENCH_DIR}/android/app/build/outputs/apk/profile/app-profile.apk"
}
material3_demo_apk_path() {
  echo "${MATERIAL3_DEMO_DIR}/android/app/build/outputs/apk/profile/app-profile.apk"
}

# run_scenario <scenario> <scen_out_dir> <log> — drives one scenario via
# run.sh and echoes "<p50> <p95>" (bare ms numbers) on success, or
# "SKIPPED <reason>" — never fails the matrix (see header). Progress/command
# lines go to stderr; ONLY that final result line goes to stdout, so a
# caller capturing this function via `$(run_scenario ...)` gets exactly the
# result, with the progress still visible live on the terminal.
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
  echo "${p50} ${p95}"
}

# run_nav <log> — drives one push/pop pass against material3-demo and
# echoes "<total_p50_ms> <submit_p95_ms> <acquire_p95_ms>" on success, or
# "SKIPPED <reason>" — never fails the matrix (see header). Same
# stdout/stderr split as run_scenario above.
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
  print_cmd "" "adb -s ${DEVICE} logcat -d -v raw > ${log}" >&2
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
  adb -s "${DEVICE}" logcat -d -v raw >"${log}" 2>/dev/null
  adb -s "${DEVICE}" shell am force-stop "${MATERIAL3_DEMO_PKG}" >/dev/null 2>&1 || true

  local line total_p50 submit_p95 acquire_p95
  line="$(grep -a 'frust-perf frame' "${log}" 2>/dev/null | tail -n1 || true)"
  if [ -z "${line}" ]; then
    echo "SKIPPED no 'frust-perf frame' summary line found in logcat (see ${log})"
    return 0
  fi
  total_p50="$(printf '%s\n' "${line}" | grep -oE 'total_p50_ms=[0-9]+' | cut -d= -f2)"
  submit_p95="$(printf '%s\n' "${line}" | grep -oE 'submit_p95_ms=[0-9]+' | cut -d= -f2)"
  acquire_p95="$(printf '%s\n' "${line}" | grep -oE 'acquire_p95_ms=[0-9]+' | cut -d= -f2)"
  if [ -z "${total_p50}" ] || [ -z "${submit_p95}" ]; then
    echo "SKIPPED could not parse total_p50_ms=/submit_p95_ms= out of: ${line}"
    return 0
  fi
  echo "${total_p50} ${submit_p95} ${acquire_p95:-n/a}"
}

# --- Dry run: print every command for every cell, then exit -------------

if [ "${DRY_RUN}" -eq 1 ]; then
  echo "== ab_matrix.sh --dry-run =="
  echo "device: ${DEVICE} (never queried — --dry-run touches no device)"
  echo "aa: ${AA_LIST}  scale: ${SCALE_LIST}  scenarios: ${SCENARIOS_LIST}"
  echo "runs: ${RUNS}  duration: ${DURATION}s  frust: ${FRUST_BIN}"
  echo "taps: ${TAPS}"
  echo
  cell=0
  for aa in "${AA_ARR[@]}"; do
    for scale in "${SCALE_ARR[@]}"; do
      cell=$((cell + 1))
      echo "=== cell ${cell}: aa=${aa} scale=${scale} ==="
      build_apk "frust_bench" "${FRUST_BENCH_DIR}" "${aa}" "${scale}" "/dev/null"
      install_apk "frust_bench" "$(frust_bench_apk_path)" "/dev/null"
      for scenario in "${SCENARIO_ARR[@]}"; do
        run_scenario "${scenario}" "/dev/null" "/dev/null" >/dev/null
      done
      build_apk "material3-demo" "${MATERIAL3_DEMO_DIR}" "${aa}" "${scale}" "/dev/null"
      install_apk "material3-demo" "$(material3_demo_apk_path)" "/dev/null"
      run_nav "/dev/null" >/dev/null
      echo
    done
  done
  echo "Dry run complete — ${cell} cell(s), 0 devices touched, nothing written to disk."
  exit 0
fi

# --- Live run: drive the matrix ------------------------------------------

declare -a TABLE_ROWS=()

cell=0
for aa in "${AA_ARR[@]}"; do
  for scale in "${SCALE_ARR[@]}"; do
    cell=$((cell + 1))
    echo
    echo "=== cell ${cell}: aa=${aa} scale=${scale} ==="

    build_apk "frust_bench" "${FRUST_BENCH_DIR}" "${aa}" "${scale}" \
      "${OUT_DIR}/build/frust_bench-${aa}-${scale}.log"
    install_apk "frust_bench" "$(frust_bench_apk_path)" \
      "${OUT_DIR}/install/frust_bench-${aa}-${scale}.log"

    row="| ${aa} | ${scale} |"
    for scenario in "${SCENARIO_ARR[@]}"; do
      scen_out="${OUT_DIR}/scenarios/${scenario}-${aa}-${scale}"
      result="$(run_scenario "${scenario}" "${scen_out}" \
        "${OUT_DIR}/scenarios/${scenario}-${aa}-${scale}.runsh.log")"
      if [[ "${result}" == SKIPPED* ]]; then
        echo "${scenario}: ${result}"
        row="${row} SKIPPED | SKIPPED |"
      else
        p50="$(printf '%s' "${result}" | cut -d' ' -f1)"
        p95="$(printf '%s' "${result}" | cut -d' ' -f2)"
        row="${row} ${p50} | ${p95} |"
      fi
    done

    build_apk "material3-demo" "${MATERIAL3_DEMO_DIR}" "${aa}" "${scale}" \
      "${OUT_DIR}/build/material3demo-${aa}-${scale}.log"
    install_apk "material3-demo" "$(material3_demo_apk_path)" \
      "${OUT_DIR}/install/material3demo-${aa}-${scale}.log"

    nav_result="$(run_nav "${OUT_DIR}/nav/${aa}-${scale}.log")"
    if [[ "${nav_result}" == SKIPPED* ]]; then
      echo "nav: ${nav_result}"
      row="${row} SKIPPED | SKIPPED |"
    else
      nav_total_p50="$(printf '%s' "${nav_result}" | cut -d' ' -f1)"
      nav_submit_p95="$(printf '%s' "${nav_result}" | cut -d' ' -f2)"
      row="${row} ${nav_total_p50} | ${nav_submit_p95} |"
    fi

    TABLE_ROWS+=("${row}")
  done
done

# --- Emit the Markdown table ----------------------------------------------

header="| AA mode | Render scale |"
sep="|---|---|"
for scenario in "${SCENARIO_ARR[@]}"; do
  upper="$(printf '%s' "${scenario}" | tr '[:lower:]' '[:upper:]')"
  header="${header} ${upper} p50 (ms) | ${upper} p95 (ms) |"
  sep="${sep}---|---|"
done
header="${header} nav total_p50 (ms) | nav submit_p95 (ms) |"
sep="${sep}---|---|"

{
  echo
  echo "## Fine-floor A/B matrix — device ${DEVICE}"
  echo
  echo "${header}"
  echo "${sep}"
  for row in "${TABLE_ROWS[@]}"; do
    echo "${row}"
  done
  echo
  echo "Note: under FRUST_RENDER_SCALE<1 the snapshot-layer cache is disabled"
  echo "by design, so the nav column's numbers at scale<1 measure the inline"
  echo "(uncached) path, not the cached one."
} | tee "${OUT_DIR}/ab_matrix.md"

echo
echo "Matrix complete — ${cell} cell(s). Table written to ${OUT_DIR}/ab_matrix.md"
echo "Raw build/install/scenario/nav logs are under ${OUT_DIR}/"
