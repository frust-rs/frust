#!/usr/bin/env bash
# scripts/size-report.sh — repeatable release-binary size report.
#
# Measures the numbers every phase-7 size-affecting task (see
# workflow/plans/features/frust-phase-7-performance/tasks/11 and 15)
# measures against: the release arm64-v8a `.so` (unstripped/stripped), any
# already-built APK/AAB's per-ABI `.so` + dex sizes, and (best-effort) a
# desktop cargo-bloat top-20 crate breakdown as a host-proxy for the
# device-targeted build.
#
# Usage: scripts/size-report.sh [--app <dir>]
#   --app <dir>   App directory to measure (default: examples/huddle,
#                 relative to the repo root this script lives in).
#
# Exits non-zero only if the release arm64-v8a build itself fails. Missing
# optional tooling (cargo-bloat, an already-built APK) degrades to a printed
# note, never a failure.

set -uo pipefail

# --- Arg parsing -----------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"
APP_REL="examples/huddle"

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
    -h|--help)
      sed -n '2,17p' "$0"
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

echo "== Frust size report =="
echo "App: ${APP_DIR}"
echo "Date: $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
echo

# --- Helpers -----------------------------------------------------------

# Human-readable bytes -> MB with 2 decimals, no external deps beyond awk.
to_mb() {
  awk -v b="$1" 'BEGIN { printf "%.2f MB", b / 1048576 }'
}

file_size_bytes() {
  # macOS/BSD `wc -c` is portable enough for this host; trim leading spaces.
  wc -c <"$1" | tr -d ' '
}

# Locate an `llvm-strip` under the Android NDK, if one can be found; empty
# string (not a failure) if not.
find_llvm_strip() {
  candidate_roots=""
  [ -n "${ANDROID_NDK_HOME:-}" ] && candidate_roots="${ANDROID_NDK_HOME}"
  [ -n "${ANDROID_NDK_ROOT:-}" ] && candidate_roots="${candidate_roots} ${ANDROID_NDK_ROOT}"
  if [ -n "${ANDROID_HOME:-}" ] && [ -d "${ANDROID_HOME}/ndk" ]; then
    # Pick the highest-numbered installed NDK version.
    latest_ndk="$(find "${ANDROID_HOME}/ndk" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort -V | tail -n1)"
    [ -n "${latest_ndk}" ] && candidate_roots="${candidate_roots} ${latest_ndk}"
  fi
  if [ -n "${ANDROID_SDK_ROOT:-}" ] && [ -d "${ANDROID_SDK_ROOT}/ndk" ]; then
    latest_ndk="$(find "${ANDROID_SDK_ROOT}/ndk" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort -V | tail -n1)"
    [ -n "${latest_ndk}" ] && candidate_roots="${candidate_roots} ${latest_ndk}"
  fi
  for root in ${candidate_roots}; do
    # No `-type f`: the NDK ships `llvm-strip` as a symlink to `llvm-objcopy`
    # on this toolchain layout (darwin-x86_64/bin/llvm-strip -> llvm-objcopy).
    found="$(find "${root}/toolchains/llvm/prebuilt" -maxdepth 3 -name 'llvm-strip' 2>/dev/null | head -n1)"
    if [ -n "${found}" ]; then
      printf '%s' "${found}"
      return 0
    fi
  done
  return 1
}

# --- Step 1: release arm64-v8a build ---------------------------------------

echo "-- Building release arm64-v8a .so (cargo ndk -t arm64-v8a build --release) --"
BUILD_LOG="$(mktemp)"
trap 'rm -f "${BUILD_LOG}"' EXIT

if ! (cd "${APP_DIR}" && cargo ndk -t arm64-v8a build --release) >"${BUILD_LOG}" 2>&1; then
  echo "BUILD FAILED:" >&2
  cat "${BUILD_LOG}" >&2
  exit 1
fi
echo "Build OK."
echo

# --- Step 2: unstripped / stripped .so size ---------------------------------

# The lib target's file stem is the package name with '-' replaced by '_'
# (Cargo's own convention); most Frust example apps set no explicit `[lib]
# name`, so this recovers the built file name without a `cargo metadata` call.
PKG_NAME="$(sed -n 's/^name *= *"\(.*\)"/\1/p' "${APP_DIR}/Cargo.toml" | head -n1)"
if [ -z "${PKG_NAME}" ]; then
  echo "error: could not read [package] name from ${APP_DIR}/Cargo.toml" >&2
  exit 1
fi
LIB_STEM="$(printf '%s' "${PKG_NAME}" | tr '-' '_')"
SO_PATH="${APP_DIR}/target/aarch64-linux-android/release/lib${LIB_STEM}.so"

echo "-- Release .so size (arm64-v8a) --"
if [ ! -f "${SO_PATH}" ]; then
  echo "note: expected .so not found at ${SO_PATH} — skipping .so size section"
else
  UNSTRIPPED_BYTES="$(file_size_bytes "${SO_PATH}")"
  printf '%-28s %s\n' "unstripped" "$(to_mb "${UNSTRIPPED_BYTES}") (${UNSTRIPPED_BYTES} bytes)"

  STRIP_TOOL=""
  if STRIP_TOOL="$(find_llvm_strip)"; then
    STRIP_LABEL="llvm-strip (NDK)"
  elif command -v strip >/dev/null 2>&1; then
    STRIP_TOOL="strip"
    STRIP_LABEL="strip (fallback)"
  fi

  if [ -n "${STRIP_TOOL}" ]; then
    STRIP_TMP="$(mktemp)"
    cp "${SO_PATH}" "${STRIP_TMP}"
    if "${STRIP_TOOL}" "${STRIP_TMP}" >/dev/null 2>&1; then
      STRIPPED_BYTES="$(file_size_bytes "${STRIP_TMP}")"
      printf '%-28s %s\n' "stripped (${STRIP_LABEL})" "$(to_mb "${STRIPPED_BYTES}") (${STRIPPED_BYTES} bytes)"
    else
      echo "note: ${STRIP_LABEL} failed to strip a copy of the .so — skipping stripped size"
    fi
    rm -f "${STRIP_TMP}"
  else
    echo "note: no llvm-strip (NDK) or strip found on PATH — skipping stripped size"
  fi
fi
echo

# --- Step 3: APK/AAB report (only if artifacts already exist) --------------

echo "-- APK/AAB report (existing build outputs only; no Gradle build run) --"
OUTPUTS_DIR="${APP_DIR}/android/app/build/outputs"
if [ ! -d "${OUTPUTS_DIR}" ]; then
  echo "note: no ${OUTPUTS_DIR} — run \`frust build apk\`/\`appbundle\` first for this section"
else
  FOUND_ARTIFACT=0

  # shellcheck disable=SC2044 # filenames here are build-tool-generated, no
  # embedded whitespace/newlines to worry about.
  for apk in $(find "${OUTPUTS_DIR}/apk" -name '*.apk' 2>/dev/null); do
    FOUND_ARTIFACT=1
    APK_BYTES="$(file_size_bytes "${apk}")"
    echo "APK: ${apk#"${APP_DIR}"/}"
    printf '  %-26s %s\n' "total" "$(to_mb "${APK_BYTES}") (${APK_BYTES} bytes)"
    LISTING="$(unzip -l "${apk}" 2>/dev/null)"
    printf '%s\n' "${LISTING}" | awk '
      $NF ~ /^lib\/[^\/]+\/.*\.so$/ {
        split($NF, parts, "/"); abi = parts[2]
        so_sum[abi] += $1
      }
      $NF ~ /^classes.*\.dex$/ { dex_sum += $1 }
      END {
        for (abi in so_sum) printf "  lib/%-14s %.2f MB (%d bytes)\n", abi, so_sum[abi] / 1048576, so_sum[abi]
        if (dex_sum > 0) printf "  %-18s %.2f MB (%d bytes)\n", "dex (total)", dex_sum / 1048576, dex_sum
      }
    '
  done

  # shellcheck disable=SC2044
  for aab in $(find "${OUTPUTS_DIR}/bundle" -name '*.aab' 2>/dev/null); do
    FOUND_ARTIFACT=1
    AAB_BYTES="$(file_size_bytes "${aab}")"
    echo "AAB: ${aab#"${APP_DIR}"/}"
    printf '  %-26s %s\n' "total" "$(to_mb "${AAB_BYTES}") (${AAB_BYTES} bytes)"
  done

  if [ "${FOUND_ARTIFACT}" -eq 0 ]; then
    echo "note: ${OUTPUTS_DIR} exists but no .apk/.aab found — run \`frust build apk\`/\`appbundle\` first"
  fi
fi
echo

# --- Step 4: cargo-bloat desktop host-proxy ---------------------------------

echo "-- cargo-bloat top-20 (desktop release build, host-proxy — cargo-bloat cannot target the cdylib .so directly) --"
if command -v cargo-bloat >/dev/null 2>&1; then
  (cd "${APP_DIR}" && cargo bloat --release -n 20) || echo "note: cargo-bloat run failed — skipping (non-fatal)"
else
  echo "note: cargo-bloat not installed — skipping (cargo install cargo-bloat to enable)"
fi

echo
echo "== End of report =="
