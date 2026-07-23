#!/usr/bin/env bash
# scripts/release-lean-check.sh — verify release artifacts are lean (instrumentation compiled out).
#
# Builds release and profile binaries of an app (default: examples/shadertoy),
# asserts the release binary contains ZERO "frust-perf"/"bench-scenario" strings
# (positive control: profile contains them), that the release build is built
# with the SAME `--features lean` the real CLI/device pipeline always passes
# (`BuildMode::cargo_features`'s release arm — the artifact this script
# inspects must match what actually ships), that a first-party info-level
# marker string is folded out of the ceiling in that release+lean build (real
# proof the log ceiling engages, present as a positive control in the
# non-lean profile build), and that warn-level diagnostics are retained
# (not ceiling proof by itself — see the marker check for that). Exits
# non-zero on any assertion failure, and non-zero (a distinct code) if any
# check was SKIPPED rather than executed — a SKIP is never silently reported
# as a PASS.
#
# Usage: scripts/release-lean-check.sh [--app <dir>] [--android] [--help]
#   --app <dir>   App directory to check (default: examples/shadertoy,
#                 relative to the repo root this script lives in).
#   --android     Optional: also check the release arm64-v8a .so (requires NDK).
#   --help        Print this message and exit.
#
# Exits with:
#   0 if all checks executed and passed
#   1 if a build fails
#   2 if a strings check (or other assertion) FAILed
#   3 if every check executed but one or more checks were SKIPPED (missing
#     binary, missing `strings`, or an unavailable --android toolchain) —
#     INCONCLUSIVE, distinct from a hard FAIL: a run entirely made of skips
#     must never report PASS.
#
# Runtime: ~a couple of release builds (3-5 minutes typical).
#
# Documented as a manual gate (no CI).

set -uo pipefail

# --- Arg parsing -----------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"
APP_REL="examples/shadertoy"
CHECK_ANDROID=0

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
    --android)
      CHECK_ANDROID=1
      shift
      ;;
    -h|--help)
      sed -n '2,34p' "$0"
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
  echo "error: no Cargo.toml at ${APP_DIR} — expected an app dir (e.g. examples/shadertoy)" >&2
  exit 2
fi

echo "== Frust release-lean check =="
echo "App: ${APP_DIR}"
echo "Date: $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
echo

# --- Helpers -----------------------------------------------------------

# Global count of SKIPped checks (F3 fix): a run made entirely of skips must
# never report PASS — every SKIP branch below increments this, and the final
# summary reports INCONCLUSIVE (distinct exit code) whenever it's nonzero,
# even if zero checks FAILed.
SKIP_COUNT=0

# Check a binary for the absence of instrumentation strings.
# Returns 0 if the string is NOT found (good for release).
check_strings_absent() {
  local binary="$1"
  local pattern="$2"
  local label="$3"

  if [ ! -f "${binary}" ]; then
    echo "SKIP ${label}: binary not found at ${binary}"
    SKIP_COUNT=$((SKIP_COUNT + 1))
    return 0
  fi

  if ! command -v strings >/dev/null 2>&1; then
    echo "SKIP ${label}: 'strings' command not available"
    SKIP_COUNT=$((SKIP_COUNT + 1))
    return 0
  fi

  local count
  count=$(strings "${binary}" | grep -c "${pattern}" || true)

  if [ "${count}" -eq 0 ]; then
    echo "PASS ${label}: found 0 occurrences of '${pattern}'"
    return 0
  else
    echo "FAIL ${label}: found ${count} occurrences of '${pattern}' (expected 0)"
    return 1
  fi
}

# Check a binary for the presence of instrumentation strings (positive control).
# Returns 0 if the string IS found (good for profile/debug).
check_strings_present() {
  local binary="$1"
  local pattern="$2"
  local label="$3"

  if [ ! -f "${binary}" ]; then
    echo "SKIP ${label}: binary not found at ${binary}"
    SKIP_COUNT=$((SKIP_COUNT + 1))
    return 0
  fi

  if ! command -v strings >/dev/null 2>&1; then
    echo "SKIP ${label}: 'strings' command not available"
    SKIP_COUNT=$((SKIP_COUNT + 1))
    return 0
  fi

  local count
  count=$(strings "${binary}" | grep -c "${pattern}" || true)

  if [ "${count}" -gt 0 ]; then
    echo "PASS ${label}: found ${count} occurrences of '${pattern}' (positive control)"
    return 0
  else
    echo "FAIL ${label}: found 0 occurrences of '${pattern}' (expected to find it in profile build)"
    return 1
  fi
}

# --- Desktop release and profile builds -----------------------------------

echo "-- Building desktop release --"
BUILD_LOG="$(mktemp)"
trap 'rm -f "${BUILD_LOG}"' EXIT

# `--features lean` matches `BuildMode::cargo_features()`'s release arm
# (`crates/frust-drive/src/build_info.rs`) — the real CLI/device pipeline
# always passes this on release, so the artifact this script inspects must
# be built the same way or the checks below (in particular the marker
# contrast check) prove nothing about the shipping ceiling (review round-0's
# F1 finding).
if ! (cd "${APP_DIR}" && cargo build --release --features lean) >"${BUILD_LOG}" 2>&1; then
  echo "BUILD FAILED:" >&2
  cat "${BUILD_LOG}" >&2
  exit 1
fi
echo "Release build OK (--features lean)."
echo

echo "-- Building desktop profile --"
if ! (cd "${APP_DIR}" && cargo build --profile profile --features frust/perf-trace) >"${BUILD_LOG}" 2>&1; then
  echo "BUILD FAILED:" >&2
  cat "${BUILD_LOG}" >&2
  exit 1
fi
echo "Profile build OK."
echo

# --- Locate built binaries -------------------------------------------------

# Cargo's convention: package name with '-' replaced by '_' becomes the binary/lib name.
PKG_NAME="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "${APP_DIR}/Cargo.toml" | head -n1)"
if [ -z "${PKG_NAME}" ]; then
  echo "error: could not read [package] name from ${APP_DIR}/Cargo.toml" >&2
  exit 2
fi
BIN_STEM="$(printf '%s' "${PKG_NAME}" | tr '-' '_')"

# For cdylib (Android) or a binary: check both paths.
RELEASE_BIN="${APP_DIR}/target/release/${BIN_STEM}"
PROFILE_BIN="${APP_DIR}/target/profile/${BIN_STEM}"

# On macOS, executables have no extension; on other platforms they might.
# cargo build places binaries in target/(profile_name)/
for ext in "" ".exe"; do
  if [ -f "${RELEASE_BIN}${ext}" ]; then
    RELEASE_BIN="${RELEASE_BIN}${ext}"
    break
  fi
done

for ext in "" ".exe"; do
  if [ -f "${PROFILE_BIN}${ext}" ]; then
    PROFILE_BIN="${PROFILE_BIN}${ext}"
    break
  fi
done

# --- Instrumentation string checks (desktop) ----------------------------

echo "-- Instrumentation string checks (desktop) --"

RELEASE_FAIL=0
PROFILE_FAIL=0

# Release should have ZERO frust-perf strings.
if ! check_strings_absent "${RELEASE_BIN}" "frust-perf" "Release frust-perf strings"; then
  RELEASE_FAIL=1
fi

if ! check_strings_absent "${RELEASE_BIN}" "bench-scenario" "Release bench-scenario strings"; then
  RELEASE_FAIL=1
fi

# Profile should have frust-perf strings (positive control).
if ! check_strings_present "${PROFILE_BIN}" "frust-perf" "Profile frust-perf strings (positive control)"; then
  PROFILE_FAIL=1
fi

echo

# --- Log-ceiling marker contrast check (desktop) --------------------------

echo "-- Log-ceiling marker contrast check (desktop) --"

# `frust-shell-desktop::logger::init_once` emits ONE unconditional (NOT
# perf-trace-gated), OnceLock/Once-guarded `log::info!` marker line
# purpose-built for this check: every other first-party info-level site is
# either perf-trace-gated or mobile-shell-only, so none of them can prove the
# release `lean` feature's `log/release_max_level_warn` ceiling actually
# folds info-level lines out of the binary. This is the REAL ceiling proof
# (review round-0's F1 finding) — present as a positive control in the
# non-lean profile build, absent once the release+lean ceiling engages.
if ! check_strings_present "${PROFILE_BIN}" "frust-shell-desktop: logger initialized" "Profile log-ceiling marker (positive control)"; then
  PROFILE_FAIL=1
fi

if ! check_strings_absent "${RELEASE_BIN}" "frust-shell-desktop: logger initialized" "Release log-ceiling marker (ceiling proof)"; then
  RELEASE_FAIL=1
fi

echo

# --- Warn-level retention check (desktop) ---------------------------------

echo "-- Warn-level retention check (desktop) --"

# Check that warn-level strings like "frust-render:" are still in the release
# binary. This is warn-RETENTION only, NOT ceiling proof by itself (a build
# with no ceiling at all would also pass this) — the marker check above is
# what actually proves the ceiling folds info lines out.
if ! check_strings_present "${RELEASE_BIN}" "frust-render:" "Release warn-level 'frust-render:' retention"; then
  RELEASE_FAIL=1
fi

echo

# --- Android .so checks (optional) ----------------------------------------

if [ "${CHECK_ANDROID}" -eq 1 ]; then
  echo "-- Android arm64-v8a .so checks --"

  # Try to build the .so for Android.
  if ! (cd "${APP_DIR}" && cargo ndk -t arm64-v8a build --release) >"${BUILD_LOG}" 2>&1; then
    echo "SKIP Android .so checks: release .so build failed or NDK not available"
    SKIP_COUNT=$((SKIP_COUNT + 1))
  else
    echo "Android release build OK."

    # Locate the .so.
    SO_PATH="${APP_DIR}/target/aarch64-linux-android/release/lib${BIN_STEM}.so"

    if [ ! -f "${SO_PATH}" ]; then
      echo "SKIP Android .so checks: expected .so not found at ${SO_PATH}"
      SKIP_COUNT=$((SKIP_COUNT + 1))
    else
      # Same string checks for the .so.
      if ! check_strings_absent "${SO_PATH}" "frust-perf" "Android .so frust-perf strings"; then
        RELEASE_FAIL=1
      fi

      if ! check_strings_absent "${SO_PATH}" "bench-scenario" "Android .so bench-scenario strings"; then
        RELEASE_FAIL=1
      fi

      if ! check_strings_present "${SO_PATH}" "frust-render:" "Android .so warn-level 'frust-render:' survival"; then
        RELEASE_FAIL=1
      fi
    fi
  fi
  echo
fi

# --- Summary ---------------------------------------------------------------
#
# Three-way verdict (F3 fix): a SKIP is never silently reported as a PASS.
# Any FAIL wins outright (exit 2); otherwise any SKIP downgrades an
# all-checks-executed PASS to INCONCLUSIVE (exit 3) — only a run where every
# check actually EXECUTED and passed reports PASS (exit 0).

echo "== Release-lean check summary =="

if [ "${RELEASE_FAIL}" -ne 0 ] || [ "${PROFILE_FAIL}" -ne 0 ]; then
  echo "FAIL: One or more checks failed (see above)."
  exit 2
elif [ "${SKIP_COUNT}" -gt 0 ]; then
  echo "INCONCLUSIVE: ${SKIP_COUNT} check(s) skipped (see above) — no assertion failed, but a SKIP is not proof of leanness."
  exit 3
else
  echo "PASS: All checks executed (0 skipped) and passed. Release artifacts are lean."
  exit 0
fi
