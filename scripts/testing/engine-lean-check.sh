#!/usr/bin/env bash
# scripts/testing/engine-lean-check.sh — G3/G4: the frust-engine tier stays
# feature-gated dead weight until an app actually opts in.
#
# `examples/material3-demo` is the OFF-arm target: a standalone workspace (its
# own `Cargo.lock`, run from its own directory — see `docs/DEVELOPMENT.md`'s
# Test section) that depends on neither `frust-engine` nor a `frust-render`
# engine-tier feature today. This script builds its desktop release binary
# with the engine tier OFF (its ordinary default build — there is nothing to
# opt out of yet) and asserts ZERO `frust-engine`/`frust_engine` marker
# strings in it, then attempts the ON arm as a positive control.
#
# The ON arm is the positive control — the mirror of
# `scripts/release-lean-check.sh`'s `check_strings_present`: it builds
# `frust-render` with the `engine-tier` feature ON, from the ROOT workspace,
# and asserts the same markers ARE present in the produced rlib. A check that
# can only ever report absence proves nothing about its own sensitivity.
#
# THE ON ARM DELIBERATELY DOES NOT TARGET `material3-demo`. Opting an app into
# the tier means turning on `frust-render/engine-tier` through the `frust`
# facade, and the facade forwards no such feature to its shells (unlike
# `perf-trace`/`devtools`/`hybrid-tier` — see `crates/frust/Cargo.toml`), so no
# app in this repo can reach the feature today. `frust-render` itself is the
# nearest reachable ON target: it is the crate that owns the feature and the
# dependency edge the OFF arm is checking for the absence of. Point this arm at
# an app build once the facade forwards the feature.
#
# Reuses `scripts/release-lean-check.sh`'s SKIP-vs-FAIL exit-code shape: a run
# made entirely of skips must never report PASS.
#
# Usage: scripts/testing/engine-lean-check.sh [--help]
#
# Exits with:
#   0 if every check executed (0 skipped) and passed
#   1 if the arguments themselves are invalid (usage error)
#   2 if a built artifact FAILED its marker check — the OFF arm's binary
#     carrying frust-engine markers it never asked for, or the ON arm's rlib
#     carrying none despite the feature being on (a check that cannot see what
#     it is looking for)
#   3 if one or more checks were SKIPPED rather than executed (a build that
#     could not be produced on this box, or no `strings` command) —
#     INCONCLUSIVE, never silently reported as PASS
#
# Documented as a manual gate (no CI), the same shape as
# `scripts/release-lean-check.sh`.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." >/dev/null 2>&1 && pwd)"
APP_DIR="${REPO_ROOT}/examples/material3-demo"

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help)
      sed -n '2,42p' "$0"
      exit 0
      ;;
    *)
      echo "error: unrecognized argument '$1'" >&2
      exit 1
      ;;
  esac
done

if [ ! -f "${APP_DIR}/Cargo.toml" ]; then
  echo "error: no Cargo.toml at ${APP_DIR} — has examples/material3-demo moved?" >&2
  exit 1
fi

echo "== Frust engine-lean check =="
echo "App: ${APP_DIR}"
echo "Date: $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
echo

# --- Helpers -----------------------------------------------------------

# Global count of SKIPped checks: a run made entirely of skips must never
# report PASS — the same F3 shape `scripts/release-lean-check.sh` follows.
SKIP_COUNT=0
FAIL_COUNT=0

# Checks a binary for the absence of every marker in `patterns` (space
# separated). Each individual pattern hit is reported; the binary/`strings`
# absence itself is a single SKIP, not a per-pattern one.
check_markers_absent() {
  local binary="$1"
  local label="$2"
  shift 2
  local patterns=("$@")

  if [ ! -f "${binary}" ]; then
    echo "SKIP ${label}: binary not found at ${binary}"
    SKIP_COUNT=$((SKIP_COUNT + 1))
    return
  fi

  if ! command -v strings >/dev/null 2>&1; then
    echo "SKIP ${label}: 'strings' command not available"
    SKIP_COUNT=$((SKIP_COUNT + 1))
    return
  fi

  local pattern count
  for pattern in "${patterns[@]}"; do
    count=$(strings "${binary}" | grep -c -- "${pattern}" || true)
    if [ "${count}" -eq 0 ]; then
      echo "PASS ${label}: found 0 occurrences of '${pattern}'"
    else
      echo "FAIL ${label}: found ${count} occurrences of '${pattern}' (expected 0)"
      FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
  done
}

# The mirror of `check_markers_absent`: the positive control, asserting each
# marker IS present. A build that turned the feature on and still shows no
# marker means the check above cannot see what it is looking for, which would
# make every OFF-arm PASS meaningless — so this is a FAIL, not a SKIP.
check_markers_present() {
  local artifact="$1"
  local label="$2"
  shift 2
  local patterns=("$@")

  if [ ! -f "${artifact}" ]; then
    echo "SKIP ${label}: artifact not found at ${artifact}"
    SKIP_COUNT=$((SKIP_COUNT + 1))
    return
  fi

  if ! command -v strings >/dev/null 2>&1; then
    echo "SKIP ${label}: 'strings' command not available"
    SKIP_COUNT=$((SKIP_COUNT + 1))
    return
  fi

  local pattern count
  for pattern in "${patterns[@]}"; do
    count=$(strings "${artifact}" | grep -c -- "${pattern}" || true)
    if [ "${count}" -gt 0 ]; then
      echo "PASS ${label}: found ${count} occurrences of '${pattern}'"
    else
      echo "FAIL ${label}: found 0 occurrences of '${pattern}' (expected at least 1)"
      FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
  done
}

# --- OFF arm: material3-demo's ordinary (engine-tier OFF) release build ----

echo "-- Building material3-demo desktop release (engine tier OFF) --"

# Standalone workspace: never let a caller's globally exported
# CARGO_TARGET_DIR (this repo's other cargo invocations require one — see
# docs/DEVELOPMENT.md), or a machine-wide `[build] target-dir` in a user
# `~/.cargo/config.toml`, redirect this build into a shared target dir. It has
# its own Cargo.lock and must keep its own dedicated target dir, the same
# isolation every other standalone workspace in this repo (huddle, playground,
# ...) requires — pinned explicitly rather than trusting "unset means default".
BUILD_LOG="$(mktemp)"
trap 'rm -f "${BUILD_LOG}"' EXIT
OFF_TARGET_DIR="${APP_DIR}/target"

if ! (cd "${APP_DIR}" && CARGO_TARGET_DIR="${OFF_TARGET_DIR}" cargo build --release) >"${BUILD_LOG}" 2>&1; then
  echo "SKIP OFF-arm build: material3-demo's release build failed on this box (see ${BUILD_LOG} \
below) — a build failure never fakes a PASS here, it downgrades to INCONCLUSIVE"
  cat "${BUILD_LOG}"
  SKIP_COUNT=$((SKIP_COUNT + 1))
  OFF_BIN=""
else
  echo "OFF-arm build OK."
  PKG_NAME="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "${APP_DIR}/Cargo.toml" | head -n1)"
  if [ -z "${PKG_NAME}" ]; then
    echo "error: could not read [package] name from ${APP_DIR}/Cargo.toml" >&2
    exit 1
  fi
  BIN_STEM="$(printf '%s' "${PKG_NAME}" | tr '-' '_')"
  OFF_BIN="${OFF_TARGET_DIR}/release/${BIN_STEM}"
  for ext in "" ".exe"; do
    if [ -f "${OFF_BIN}${ext}" ]; then
      OFF_BIN="${OFF_BIN}${ext}"
      break
    fi
  done
fi
echo

echo "-- OFF-arm marker check (engine tier not requested anywhere in this build) --"
check_markers_absent "${OFF_BIN}" "OFF-arm frust-engine markers" "frust-engine" "frust_engine"
echo

# --- ON arm: positive control ----------------------------------------------

echo "-- Building frust-render release with the engine tier ON --"

# The ROOT workspace this time, not the standalone example: this arm builds the
# crate that OWNS the feature. Honours a caller-exported CARGO_TARGET_DIR (this
# repo's root builds normally run with one) and falls back to the workspace's
# own `target/` when none is set — never the OFF arm's dedicated dir, which
# belongs to the standalone example alone.
ON_TARGET_DIR="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}"
ON_BUILD_LOG="$(mktemp)"
trap 'rm -f "${BUILD_LOG}" "${ON_BUILD_LOG}"' EXIT

if ! (cd "${REPO_ROOT}" && CARGO_TARGET_DIR="${ON_TARGET_DIR}" \
      cargo build --release -p frust-render --features engine-tier) \
      >"${ON_BUILD_LOG}" 2>&1; then
  echo "SKIP ON-arm build: frust-render's engine-tier release build failed on this box (log \
below) — a build failure never fakes a PASS here, it downgrades to INCONCLUSIVE"
  cat "${ON_BUILD_LOG}"
  SKIP_COUNT=$((SKIP_COUNT + 1))
  ON_ARTIFACT=""
else
  echo "ON-arm build OK."
  # Cargo uplifts a library target into the profile dir under its plain name;
  # fall back to the hashed copy under `deps/` if that ever stops holding.
  ON_ARTIFACT="${ON_TARGET_DIR}/release/libfrust_render.rlib"
  if [ ! -f "${ON_ARTIFACT}" ]; then
    ON_ARTIFACT="$(ls -t "${ON_TARGET_DIR}"/release/deps/libfrust_render-*.rlib 2>/dev/null \
      | head -n1)"
  fi
fi
echo

echo "-- ON-arm marker check (engine tier ON, positive control) --"
check_markers_present "${ON_ARTIFACT}" "ON-arm frust-engine markers" "frust_engine"
echo

# --- Summary -----------------------------------------------------------
#
# Same three-way verdict as scripts/release-lean-check.sh: any FAIL wins
# outright (exit 2); otherwise any SKIP downgrades an all-executed PASS to
# INCONCLUSIVE (exit 3); only a run where every check actually ran and passed
# reports PASS (exit 0).

echo "== Engine-lean check summary =="

if [ "${FAIL_COUNT}" -gt 0 ]; then
  echo "FAIL: ${FAIL_COUNT} check(s) failed (see above)."
  exit 2
elif [ "${SKIP_COUNT}" -gt 0 ]; then
  echo "INCONCLUSIVE: ${SKIP_COUNT} check(s) skipped (see above) — no assertion failed, but a \
SKIP is not proof of leanness."
  exit 3
else
  echo "PASS: All checks executed (0 skipped) and passed."
  exit 0
fi
