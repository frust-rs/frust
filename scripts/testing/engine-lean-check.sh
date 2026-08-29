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
# THE ON ARM IS CURRENTLY A DESIGN SKIP, NOT A BUG: `frust-render` has no
# engine-tier cargo feature yet (that seam is a later phase of the frust-engine
# plan — see `docs/RENDER_ARCHITECTURE.md`'s Engine-tier boundary), so there is
# no way to opt `material3-demo` into linking `frust-engine` today. Once that
# feature lands, wire this script's ON arm to build with it enabled and assert
# the markers ARE present (the mirror of `scripts/release-lean-check.sh`'s
# `check_strings_present` positive control) — until then it reports SKIP, and
# this file's own header is the reminder to come back and do that.
#
# Reuses `scripts/release-lean-check.sh`'s SKIP-vs-FAIL exit-code shape: a run
# made entirely of skips must never report PASS.
#
# Usage: scripts/testing/engine-lean-check.sh [--help]
#
# Exits with:
#   0 if every check executed (0 skipped) and passed
#   1 if the arguments themselves are invalid (usage error)
#   2 if the OFF arm's binary was built and FAILED the marker-absence check
#     (frust-engine leaked into a build that never asked for it)
#   3 if one or more checks were SKIPPED rather than executed — the ON arm
#     (always, until the engine-tier feature exists) and/or the OFF arm (if
#     even that build could not produce a binary on this box) — INCONCLUSIVE,
#     never silently reported as PASS
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
      sed -n '2,29p' "$0"
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

# --- ON arm: positive control, currently unreachable ------------------------

echo "-- ON-arm marker check (engine tier ON, positive control) --"
echo "SKIP ON-arm: examples/material3-demo depends on no frust-engine feature and \
frust-render exposes no engine-tier cargo feature to opt into yet (docs/RENDER_ARCHITECTURE.md's \
Engine-tier boundary) — wire this arm up once that feature lands, per this script's own header."
SKIP_COUNT=$((SKIP_COUNT + 1))
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
