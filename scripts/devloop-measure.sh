#!/usr/bin/env bash
# scripts/devloop-measure.sh — repeatable desktop dev-loop timing baseline.
#
# Measures the desktop dev-loop baseline this repo tracks (see
# docs/PERFORMANCE_BASELINES.md's Dev loop section for the recorded numbers): an
# app-crate-only incremental `cargo build` wall time (default toolchain
# config), the same
# incremental measurement under an alternate linker where one is found on
# PATH, and under the `cranelift` codegen backend where the installed
# nightly toolchain supports it. Each variant reports the median of
# several warm runs.
#
# Usage: scripts/devloop-measure.sh [--app <dir>] [--runs <n>]
#   --app <dir>   App directory to measure (default: examples/huddle,
#                 relative to the repo root this script lives in) — a
#                 standalone (non-root-workspace) `frust`-scaffolded-shaped
#                 app, so measuring it never touches the root workspace's
#                 pins/profiles.
#   --runs <n>    Warm incremental-build runs per variant (default: 3).
#
# This is measurement-only: it never edits the root workspace's manifest,
# profiles, or pins (see docs/DEVELOPMENT.md's Version-Pin Policy) and
# always restores the app's touched source file when done. Exits non-zero
# only if the default-config cold/incremental build itself fails; a
# missing alternate linker or an unsupported/uninstalled cranelift backend
# degrades to a printed skip note, never a failure.

set -uo pipefail

# --- Arg parsing -----------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"
APP_REL="examples/huddle"
RUNS=3

while [ $# -gt 0 ]; do
  case "$1" in
    --app)
      [ $# -ge 2 ] || { echo "error: --app requires a directory argument" >&2; exit 2; }
      APP_REL="$2"
      shift 2
      ;;
    --app=*)
      APP_REL="${1#--app=}"
      shift
      ;;
    --runs)
      [ $# -ge 2 ] || { echo "error: --runs requires a number argument" >&2; exit 2; }
      RUNS="$2"
      shift 2
      ;;
    --runs=*)
      RUNS="${1#--runs=}"
      shift
      ;;
    -h|--help)
      sed -n '2,22p' "$0"
      exit 0
      ;;
    *)
      echo "error: unrecognized argument '$1'" >&2
      exit 2
      ;;
  esac
done

case "$APP_REL" in
  /*) APP_DIR="$APP_REL" ;;
  *) APP_DIR="${REPO_ROOT}/${APP_REL}" ;;
esac

if [ ! -f "${APP_DIR}/Cargo.toml" ]; then
  echo "error: no Cargo.toml at ${APP_DIR} — expected an app dir (e.g. examples/huddle)" >&2
  exit 2
fi

LIB_RS="${APP_DIR}/src/lib.rs"
if [ ! -f "${LIB_RS}" ]; then
  echo "error: no ${LIB_RS} — devloop-measure needs an app-crate lib.rs to touch" >&2
  exit 2
fi

echo "== Frust dev-loop measurement =="
echo "App: ${APP_DIR}"
echo "Runs per variant: ${RUNS}"
echo "Date: $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
echo "Host: $(uname -s) $(uname -m)"
if command -v sysctl >/dev/null 2>&1; then
  echo "CPU: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown)"
fi
echo "rustc (default): $(rustc --version 2>/dev/null || echo unknown)"
echo

# --- Helpers -----------------------------------------------------------

# Restore the touched lib.rs from its backup, if one is pending. Safe to
# call multiple times (no-op once restored).
LIB_RS_BAK="${LIB_RS}.devloop-measure.bak"
restore_lib() {
  if [ -f "${LIB_RS_BAK}" ]; then
    mv "${LIB_RS_BAK}" "${LIB_RS}"
  fi
}
trap restore_lib EXIT

backup_lib() {
  cp "${LIB_RS}" "${LIB_RS_BAK}"
}

# Median of N floating-point seconds values passed as args.
median() {
  printf '%s\n' "$@" | sort -n | awk '{a[NR]=$1} END {
    n = NR
    if (n % 2 == 1) printf "%.3f", a[(n+1)/2]
    else printf "%.3f", (a[n/2] + a[n/2+1]) / 2
  }'
}

# Run `${RUNS}` incremental builds (each preceded by appending one marker
# comment line to lib.rs, restored at the end), in the app directory,
# under the environment already exported by the caller, invoking the cargo
# command given in "$2" (default "cargo build"; cranelift's caller passes
# "cargo +nightly build"). Prints one "run N: <seconds>" line per run to
# stderr (so it interleaves with cargo's own output for visibility) and
# echoes the median on stdout.
timed_incremental_runs() {
  label="$1"
  cargo_cmd="${2:-cargo build}"
  times=()
  for i in $(seq 1 "${RUNS}"); do
    printf '// devloop-measure: %s incremental touch %s\n' "${label}" "$(date +%s%N)" >> "${LIB_RS}"
    start="$(date +%s.%N)"
    if ! (cd "${APP_DIR}" && eval "${cargo_cmd}") >/tmp/devloop-measure-build.log 2>&1; then
      echo "BUILD FAILED (${label}, run ${i}):" >&2
      cat /tmp/devloop-measure-build.log >&2
      rm -f /tmp/devloop-measure-build.log
      return 1
    fi
    end="$(date +%s.%N)"
    elapsed="$(awk -v s="${start}" -v e="${end}" 'BEGIN { printf "%.3f", e - s }')"
    echo "  run ${i}: ${elapsed}s" >&2
    times+=("${elapsed}")
  done
  rm -f /tmp/devloop-measure-build.log
  median "${times[@]}"
}

# --- Variant 1: default config -----------------------------------------

echo "-- Variant 1: default config (cargo build, incremental) --"
echo "Cold warm-up build (untimed; not the metric)..."
if ! (cd "${APP_DIR}" && cargo build) >/tmp/devloop-measure-cold.log 2>&1; then
  echo "COLD BUILD FAILED:" >&2
  cat /tmp/devloop-measure-cold.log >&2
  rm -f /tmp/devloop-measure-cold.log
  exit 1
fi
rm -f /tmp/devloop-measure-cold.log
echo "Cold build OK."

backup_lib
DEFAULT_MEDIAN="$(timed_incremental_runs "default")"
STATUS=$?
restore_lib
if [ "${STATUS}" -ne 0 ]; then
  echo "error: default-config incremental measurement failed" >&2
  exit 1
fi
echo "Median incremental build (default config): ${DEFAULT_MEDIAN}s"
echo

# --- Variant 2: alternate linker (macOS: ld64.lld; Linux: lld) -----------

echo "-- Variant 2: alternate linker --"
ALT_LINKER=""
if [ "$(uname -s)" = "Darwin" ]; then
  if command -v ld64.lld >/dev/null 2>&1; then
    ALT_LINKER="$(command -v ld64.lld)"
    echo "Found ld64.lld: ${ALT_LINKER} (macOS-relevant alternate; lld itself is Linux-relevant)"
  else
    echo "note: no ld64.lld on PATH (e.g. \`brew install llvm\`/\`lld\`) — skipping alternate-linker variant on macOS"
  fi
else
  if command -v lld >/dev/null 2>&1; then
    ALT_LINKER="$(command -v lld)"
    echo "Found lld: ${ALT_LINKER}"
  else
    echo "note: no lld on PATH — skipping alternate-linker variant"
  fi
fi

if [ -n "${ALT_LINKER}" ]; then
  echo "Warm-up build under alternate linker (untimed; RUSTFLAGS changes invalidate cargo's build cache, so this first build is a full recompile, not the metric)..."
  if ! (cd "${APP_DIR}" && RUSTFLAGS="-C link-arg=-fuse-ld=${ALT_LINKER}" cargo build) >/tmp/devloop-measure-linker-warmup.log 2>&1; then
    echo "note: alternate-linker warm-up build failed — skipping variant"
    cat /tmp/devloop-measure-linker-warmup.log >&2
  else
    rm -f /tmp/devloop-measure-linker-warmup.log
    backup_lib
    export RUSTFLAGS="-C link-arg=-fuse-ld=${ALT_LINKER}"
    LINKER_MEDIAN="$(timed_incremental_runs "alt-linker" "cargo build")"
    STATUS=$?
    unset RUSTFLAGS
    restore_lib
    if [ "${STATUS}" -ne 0 ]; then
      echo "note: alternate-linker incremental measurement failed — see log above"
    else
      echo "Median incremental build (alternate linker): ${LINKER_MEDIAN}s"
    fi
  fi
fi
echo

# --- Variant 3: cranelift codegen backend (nightly only) ----------------

echo "-- Variant 3: cranelift codegen backend --"
CRANELIFT_OK=0
if command -v rustup >/dev/null 2>&1 && rustup toolchain list 2>/dev/null | grep -q '^nightly'; then
  if rustup component list --toolchain nightly --installed 2>/dev/null | grep -q 'rustc-codegen-cranelift'; then
    CRANELIFT_OK=1
  else
    echo "note: rustc-codegen-cranelift is available via \`rustup component add\` but not installed for the nightly toolchain — skipping (install it yourself first if you want this variant measured; this script never installs toolchain components)"
  fi
else
  echo "note: no nightly toolchain installed (\`rustup toolchain install nightly\`) — skipping cranelift variant"
fi

if [ "${CRANELIFT_OK}" -eq 1 ]; then
  echo "rustc (nightly): $(rustc +nightly --version 2>/dev/null || echo unknown)"
  CRANELIFT_TARGET_DIR="$(mktemp -d)"
  echo "Warm-up build under cranelift (untimed; isolated CARGO_TARGET_DIR=${CRANELIFT_TARGET_DIR} so it never disturbs the default-config target dir)..."
  if ! (cd "${APP_DIR}" && CARGO_TARGET_DIR="${CRANELIFT_TARGET_DIR}" RUSTFLAGS="-Zcodegen-backend=cranelift" cargo +nightly build) >/tmp/devloop-measure-cranelift-warmup.log 2>&1; then
    echo "note: cranelift warm-up build failed — skipping variant"
    cat /tmp/devloop-measure-cranelift-warmup.log >&2
  else
    rm -f /tmp/devloop-measure-cranelift-warmup.log
    backup_lib
    export CARGO_TARGET_DIR="${CRANELIFT_TARGET_DIR}"
    export RUSTFLAGS="-Zcodegen-backend=cranelift"
    CRANELIFT_MEDIAN="$(timed_incremental_runs "cranelift" "cargo +nightly build")"
    STATUS=$?
    unset CARGO_TARGET_DIR
    unset RUSTFLAGS
    restore_lib
    if [ "${STATUS}" -ne 0 ]; then
      echo "note: cranelift incremental measurement failed — see log above"
    else
      echo "Median incremental build (cranelift): ${CRANELIFT_MEDIAN}s"
    fi
  fi
  rm -rf "${CRANELIFT_TARGET_DIR}"
fi
echo

echo "== End of report =="
echo "See docs/PERFORMANCE_BASELINES.md's Dev loop section"
echo "for the recorded baseline and methodology this script reproduces."
