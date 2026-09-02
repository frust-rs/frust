#!/usr/bin/env bash
# scripts/testing/engine-lean-check.sh — the frust-engine renderer is what an
# ordinary build contains, with no vello-classic weight left beside it.
#
# `examples/material3-demo` is the target: a standalone workspace (its own
# `Cargo.lock`, run from its own directory — see `docs/DEVELOPMENT.md`'s Test
# section) that deps `frust` with default features. This script builds its
# desktop release binary with NO feature overrides (its ordinary build) and
# asserts `frust-engine`/`frust_engine` marker strings ARE present in it — the
# INVERTED expectation from the pre-swap gate, which asserted their absence —
# and that the vello-classic markers are now ABSENT, since vello and the
# env-var escape hatch that reached it were deleted.
#
# The absence patterns are the CLASSIC-ONLY crate names `vello_shaders` and
# `vello_encoding`, deliberately NOT a bare `vello`: `vello_common` (and
# `glifo`) are `frust-engine`'s own vendored rasterizer core and legitimately
# remain in the binary, so a bare-`vello` absence assertion would be a
# guaranteed false FAIL rather than a leak check.
#
# There is no second, feature-ON arm any more. It existed as a positive
# control while the engine was an opt-in cargo feature — it built
# `frust-render` with that feature explicitly on and asserted the same markers
# in the rlib. The feature is gone (the engine is a plain dependency of
# `frust-render`, and there is no other renderer to build instead), so an
# ON/OFF distinction has nothing to distinguish: the default arm above IS the
# engine build, and its own present-check is what proves the check can see
# what it is looking for.
#
# Reuses `scripts/release-lean-check.sh`'s SKIP-vs-FAIL exit-code shape: a run
# made entirely of skips must never report PASS.
#
# Usage: scripts/testing/engine-lean-check.sh [--help]
#
# Exits with:
#   0 if every check executed (0 skipped) and passed
#   1 if the arguments themselves are invalid (usage error)
#   2 if a built artifact FAILED its marker check — the binary missing the
#     frust-engine markers it should carry, or still carrying a vello-classic
#     one
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
      sed -n '2,45p' "$0"
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

# Asserts each marker in `patterns` (space separated) IS present in a built
# artifact. A build that should carry a marker and shows none means either
# the feature that should have pulled it in did not, or the check itself
# cannot see what it is looking for — either way a FAIL, not a SKIP.
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

# Asserts each marker in `patterns` (space separated) is ABSENT from a built
# artifact — the leak half of the check. Unlike the present-check above, a
# missing artifact or a missing `strings` is still a SKIP: an assertion that
# could not run is inconclusive either way.
check_markers_absent() {
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
    if [ "${count}" -eq 0 ]; then
      echo "PASS ${label}: found 0 occurrences of '${pattern}'"
    else
      echo "FAIL ${label}: found ${count} occurrences of '${pattern}' (expected 0)"
      FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
  done
}

# --- material3-demo's ordinary (no feature overrides) release build ---------

echo "-- Building material3-demo desktop release (default features — the engine build) --"

# Standalone workspace: never let a caller's globally exported
# CARGO_TARGET_DIR (this repo's other cargo invocations require one — see
# docs/DEVELOPMENT.md), or a machine-wide `[build] target-dir` in a user
# `~/.cargo/config.toml`, redirect this build into a shared target dir. It has
# its own Cargo.lock and must keep its own dedicated target dir, the same
# isolation every other standalone workspace in this repo (huddle, playground,
# ...) requires — pinned explicitly rather than trusting "unset means default".
BUILD_LOG="$(mktemp)"
trap 'rm -f "${BUILD_LOG}"' EXIT
APP_TARGET_DIR="${APP_DIR}/target"

if ! (cd "${APP_DIR}" && CARGO_TARGET_DIR="${APP_TARGET_DIR}" cargo build --release) >"${BUILD_LOG}" 2>&1; then
  echo "SKIP build: material3-demo's release build failed on this box (see \
${BUILD_LOG} below) — a build failure never fakes a PASS here, it downgrades to INCONCLUSIVE"
  cat "${BUILD_LOG}"
  SKIP_COUNT=$((SKIP_COUNT + 1))
  APP_BIN=""
else
  echo "Build OK."
  PKG_NAME="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "${APP_DIR}/Cargo.toml" | head -n1)"
  if [ -z "${PKG_NAME}" ]; then
    echo "error: could not read [package] name from ${APP_DIR}/Cargo.toml" >&2
    exit 1
  fi
  BIN_STEM="$(printf '%s' "${PKG_NAME}" | tr '-' '_')"
  APP_BIN="${APP_TARGET_DIR}/release/${BIN_STEM}"
  for ext in "" ".exe"; do
    if [ -f "${APP_BIN}${ext}" ]; then
      APP_BIN="${APP_BIN}${ext}"
      break
    fi
  done
fi
echo

echo "-- marker check (the engine is what a default build contains; vello classic deleted) --"
check_markers_present "${APP_BIN}" "frust-engine markers" "frust_engine"
check_markers_absent "${APP_BIN}" "vello-classic markers" \
  "vello_shaders" "vello_encoding"
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
