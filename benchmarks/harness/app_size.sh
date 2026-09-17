#!/usr/bin/env bash
# benchmarks/harness/app_size.sh — release app-size report for both benchmark apps.
#
# Measures release-config app sizes for the paired Frust-vs-Flutter benchmark
# suite (see benchmarks/RESULTS.md's "App size (release)" section): the
# Android release APK (universal + arm64-v8a split, with a per-ABI .so/dex
# breakdown for Frust's APK — the scripts/size-report.sh technique) and the
# iOS release .app bundle size, for both benchmarks/frust_bench and
# benchmarks/flutter_bench.
#
# Usage: benchmarks/harness/app_size.sh [--attribute <unstripped.so> [size_attribute.py args...]]
#
# This script never builds anything — it only measures whatever release
# artifacts already exist on disk. A missing artifact degrades to a printed
# "not built" note (with the command that would produce it), never a
# failure; the only non-zero exit is a bad invocation (unexpected argument).
#
# --attribute <unstripped.so>  After the size tables below, additionally
#   shell out to benchmarks/harness/size_attribute.py on the given unstripped
#   .so for a per-crate / per-ELF-section breakdown. Any further arguments
#   are forwarded verbatim to size_attribute.py (e.g. --nm-dir, --compare).
#   This still never builds the .so — see size_attribute.py's own header for
#   the NDK invocation that produces one.

set -uo pipefail

ATTRIBUTE_SO=""
declare -a ATTRIBUTE_EXTRA_ARGS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --attribute)
      [ $# -ge 2 ] || { echo "error: --attribute requires a path argument" >&2; exit 2; }
      ATTRIBUTE_SO="$2"
      shift 2
      ;;
    *)
      if [ -n "${ATTRIBUTE_SO}" ]; then
        ATTRIBUTE_EXTRA_ARGS+=("$1")
        shift
      else
        echo "error: unrecognized argument '$1'" >&2
        exit 2
      fi
      ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." >/dev/null 2>&1 && pwd)"

FRUST_DIR="${REPO_ROOT}/benchmarks/frust_bench"
FLUTTER_DIR="${REPO_ROOT}/benchmarks/flutter_bench"

echo "== Benchmark app size report (release) =="
echo "Date: $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
echo

# --- Helpers -----------------------------------------------------------

to_mb() {
  awk -v b="$1" 'BEGIN { printf "%.2f MB", b / 1048576 }'
}

kb_to_mb() {
  awk -v k="$1" 'BEGIN { printf "%.2f MB", k / 1024 }'
}

file_size_bytes() {
  # macOS/BSD `wc -c` is portable enough for this host; trim leading spaces.
  wc -c <"$1" | tr -d ' '
}

dir_size_kb() {
  du -sk "$1" 2>/dev/null | awk '{ print $1 }'
}

# report_apk <label> <path> — prints a size line, or "not built".
report_apk() {
  local label="$1"
  local path="$2"
  if [ -f "${path}" ]; then
    local bytes
    bytes="$(file_size_bytes "${path}")"
    printf '  %-34s %s (%d bytes)\n' "${label}" "$(to_mb "${bytes}")" "${bytes}"
  else
    printf '  %-34s not built\n' "${label}"
  fi
}

# report_app_bundle <label> <path> — prints a `du -sk` size line for an
# iOS .app bundle, or "not built".
report_app_bundle() {
  local label="$1"
  local path="$2"
  if [ -d "${path}" ]; then
    local kb
    kb="$(dir_size_kb "${path}")"
    printf '  %-34s %s (%s KB)\n' "${label}" "$(kb_to_mb "${kb}")" "${kb}"
  else
    printf '  %-34s not built\n' "${label}"
  fi
}

# report_apk_breakdown <path> — per-ABI .so + dex sizes inside an APK,
# via `unzip -l` (the scripts/size-report.sh technique); a no-op if the
# path doesn't exist.
report_apk_breakdown() {
  local path="$1"
  [ -f "${path}" ] || return 0
  echo "  breakdown ($(basename "${path}")):"
  unzip -l "${path}" 2>/dev/null | awk '
    $NF ~ /^lib\/[^\/]+\/.*\.so$/ {
      split($NF, parts, "/"); abi = parts[2]
      so_sum[abi] += $1
    }
    $NF ~ /^classes.*\.dex$/ { dex_sum += $1 }
    END {
      for (abi in so_sum) printf "    lib/%-14s %.2f MB (%d bytes)\n", abi, so_sum[abi] / 1048576, so_sum[abi]
      if (dex_sum > 0) printf "    %-18s %.2f MB (%d bytes)\n", "dex (total)", dex_sum / 1048576, dex_sum
    }
  '
}

# --- Android release APK -----------------------------------------------

echo "-- Android release APK --"
echo

FRUST_APK_UNIVERSAL="${FRUST_DIR}/android/app/build/outputs/apk/release/app-release.apk"
FRUST_APK_ARM64="${FRUST_DIR}/android/app/build/outputs/apk/release/app-arm64-v8a-release.apk"

echo "Frust (benchmarks/frust_bench):"
report_apk "universal (arm64+armv7+x86_64)" "${FRUST_APK_UNIVERSAL}"
report_apk "arm64-v8a split" "${FRUST_APK_ARM64}"
if [ -f "${FRUST_APK_UNIVERSAL}" ]; then
  report_apk_breakdown "${FRUST_APK_UNIVERSAL}"
elif [ -f "${FRUST_APK_ARM64}" ]; then
  report_apk_breakdown "${FRUST_APK_ARM64}"
else
  echo "  note: run \`(cd benchmarks/frust_bench && frust build apk --release)\` for the universal APK,"
  echo "        or add --split-per-abi --target-platform android-arm64 for the arm64 split"
fi
echo

FLUTTER_APK_UNIVERSAL="${FLUTTER_DIR}/build/app/outputs/flutter-apk/app-release.apk"
FLUTTER_APK_ARM64="${FLUTTER_DIR}/build/app/outputs/flutter-apk/app-arm64-v8a-release.apk"

echo "Flutter (benchmarks/flutter_bench):"
report_apk "universal (arm64+armv7+x86_64)" "${FLUTTER_APK_UNIVERSAL}"
report_apk "arm64-v8a split" "${FLUTTER_APK_ARM64}"
if [ ! -f "${FLUTTER_APK_UNIVERSAL}" ]; then
  echo "  note: run \`(cd benchmarks/flutter_bench && flutter build apk --release)\` for the universal APK"
fi
if [ ! -f "${FLUTTER_APK_ARM64}" ]; then
  echo "  note: run \`(cd benchmarks/flutter_bench && flutter build apk --release --split-per-abi)\` for the arm64 split"
fi
echo

# --- iOS release .app bundle --------------------------------------------

echo "-- iOS release .app bundle (du -sk) --"
echo

FRUST_IOS_RELEASE="${FRUST_DIR}/build/ios/Build/Products/Release-iphoneos/Runner.app"

echo "Frust (benchmarks/frust_bench):"
report_app_bundle "release Runner.app" "${FRUST_IOS_RELEASE}"
if [ ! -d "${FRUST_IOS_RELEASE}" ]; then
  echo "  note: run \`(cd benchmarks/frust_bench && frust build ios --release)\` (needs a codesigning identity — see docs/DEVELOPMENT.md's Prerequisites)"
fi
echo

FLUTTER_IOS_RELEASE="${FLUTTER_DIR}/build/ios/iphoneos/Runner.app"
FLUTTER_IOS_PROFILE="${FLUTTER_DIR}/build/ios/Profile-iphoneos/Runner.app"

echo "Flutter (benchmarks/flutter_bench):"
report_app_bundle "release Runner.app" "${FLUTTER_IOS_RELEASE}"
if [ ! -d "${FLUTTER_IOS_RELEASE}" ]; then
  echo "  note: run \`(cd benchmarks/flutter_bench && flutter build ios --release)\` (needs a codesigning identity — see docs/DEVELOPMENT.md's Prerequisites)"
  report_app_bundle "profile Runner.app (config differs — not release)" "${FLUTTER_IOS_PROFILE}"
fi
echo

# --- Optional per-crate/section attribution -----------------------------

if [ -n "${ATTRIBUTE_SO}" ]; then
  echo "-- Size attribution (${ATTRIBUTE_SO}) --"
  echo
  python3 "${SCRIPT_DIR}/size_attribute.py" "${ATTRIBUTE_SO}" ${ATTRIBUTE_EXTRA_ARGS[@]+"${ATTRIBUTE_EXTRA_ARGS[@]}"}
  echo
fi

echo "== End of report =="
