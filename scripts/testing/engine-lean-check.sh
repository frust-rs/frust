#!/usr/bin/env bash
# scripts/testing/engine-lean-check.sh — Default-tier swap: the
# frust-engine tier is now reached through ordinary default features, not
# opt-in dead weight.
#
# `examples/material3-demo` is the DEFAULT-arm target: a standalone workspace
# (its own `Cargo.lock`, run from its own directory — see
# `docs/DEVELOPMENT.md`'s Test section) that deps `frust` with default
# features, which now reaches `frust-render`'s `default = ["engine-tier"]`
# with no manifest edits anywhere in the chain. This script builds its
# desktop release binary with NO feature overrides (its ordinary default
# build) and asserts `frust-engine`/`frust_engine` marker strings ARE present
# in it — the INVERTED expectation from the pre-swap gate, which asserted
# their absence — and that the vello classic markers are STILL present (the
# one-release `FRUST_RENDER_TIER=gpu` escape hatch this card ships is not
# deleted until p8-04).
#
# The ON arm remains a positive control — the mirror of
# `scripts/release-lean-check.sh`'s `check_strings_present`: it builds
# `frust-render` with the `engine-tier` feature explicitly ON, from the ROOT
# workspace, and asserts the same markers ARE present in the produced rlib. A
# check that can only ever report absence proves nothing about its own
# sensitivity; kept as an independent confirmation of the feature itself even
# though the default arm above now already carries it.
#
# Reuses `scripts/release-lean-check.sh`'s SKIP-vs-FAIL exit-code shape: a run
# made entirely of skips must never report PASS.
#
# Usage: scripts/testing/engine-lean-check.sh [--help]
#
# Exits with:
#   0 if every check executed (0 skipped) and passed
#   1 if the arguments themselves are invalid (usage error)
#   2 if a built artifact FAILED its marker check — the default arm's binary
#     missing the frust-engine or vello-classic markers it should now carry,
#     or the ON arm's rlib carrying none despite the feature being on (a check
#     that cannot see what it is looking for)
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

# --- Default arm: material3-demo's ordinary (no feature overrides) release build ----

echo "-- Building material3-demo desktop release (default features — engine tier now default) --"

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
  echo "SKIP default-arm build: material3-demo's release build failed on this box (see \
${BUILD_LOG} below) — a build failure never fakes a PASS here, it downgrades to INCONCLUSIVE"
  cat "${BUILD_LOG}"
  SKIP_COUNT=$((SKIP_COUNT + 1))
  OFF_BIN=""
else
  echo "default-arm build OK."
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

echo "-- default-arm marker check (engine tier reached via default features; vello classic escape hatch still present) --"
check_markers_present "${OFF_BIN}" "default-arm frust-engine markers" "frust_engine"
check_markers_present "${OFF_BIN}" "default-arm vello-classic markers" "vello"
echo

# --- ON arm: positive control ----------------------------------------------

echo "-- Building frust-render release with the engine tier ON --"

# The ROOT workspace this time, not the standalone example: this arm builds the
# crate that OWNS the feature. Honours a caller-exported CARGO_TARGET_DIR (this
# repo's root builds normally run with one) and falls back to the workspace's
# own `target/` when none is set — never the default arm's dedicated dir,
# which belongs to the standalone example alone.
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
