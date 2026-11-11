#!/usr/bin/env bash
# 16 KB page-size tripwire for Android shared objects.
#
# Google Play requires 16 KB page-size support for apps targeting Android 15 and later. A shared
# object is 16 KB-compatible when every PT_LOAD segment is aligned to at least 0x4000. This script
# reads the program headers of each .so and fails when any PT_LOAD alignment is below that.
#
# Usage: scripts/ci/page-size-check.sh <file-or-directory>...
#   Files must be .so files; directories are searched recursively for *.so.
#
# The reader is the NDK's llvm-readelf, found through ANDROID_NDK_HOME or ANDROID_NDK_LATEST_HOME
# (any prebuilt host directory). When neither is set, the readelf on PATH is used.
#
# Exit codes: 0 every object passes, 1 at least one object is below 0x4000, 2 usage error
# (bad argument, or no .so found).
set -euo pipefail

readonly MIN_ALIGN=0x4000

usage() {
  echo "usage: $0 <file-or-directory>..." >&2
  echo "  checks every .so given (directories are searched recursively) for PT_LOAD alignment >= 0x4000" >&2
  exit 2
}

[ "$#" -gt 0 ] || usage

find_readelf() {
  local ndk candidate
  for ndk in "${ANDROID_NDK_HOME:-}" "${ANDROID_NDK_LATEST_HOME:-}"; do
    [ -n "$ndk" ] || continue
    for candidate in "$ndk"/toolchains/llvm/prebuilt/*/bin/llvm-readelf; do
      if [ -x "$candidate" ]; then
        printf '%s\n' "$candidate"
        return 0
      fi
    done
  done
  if command -v readelf >/dev/null 2>&1; then
    command -v readelf
    return 0
  fi
  echo "error: no llvm-readelf under ANDROID_NDK_HOME/ANDROID_NDK_LATEST_HOME and no readelf on PATH" >&2
  exit 2
}

objects=()
for arg in "$@"; do
  if [ -d "$arg" ]; then
    while IFS= read -r -d '' found; do
      objects+=("$found")
    done < <(find "$arg" -type f -name '*.so' -print0)
  elif [ -f "$arg" ]; then
    objects+=("$arg")
  else
    echo "error: not a file or directory: $arg" >&2
    usage
  fi
done

if [ "${#objects[@]}" -eq 0 ]; then
  echo "error: no .so files found in the given paths" >&2
  usage
fi

readelf_bin="$(find_readelf)"
status=0

for so in "${objects[@]}"; do
  # Program-header rows look like: "  LOAD  0x... 0x... 0x... 0x... 0x... R  0x4000"; the
  # alignment is the last field.
  if ! headers="$("$readelf_bin" -lW "$so" 2>/dev/null)"; then
    echo "FAIL $so: not a readable ELF object"
    status=1
    continue
  fi
  aligns="$(printf '%s\n' "$headers" | awk '$1 == "LOAD" { print $NF }')"
  if [ -z "$aligns" ]; then
    echo "FAIL $so: no PT_LOAD segments found"
    status=1
    continue
  fi
  min=""
  while IFS= read -r align; do
    value=$((align))
    if [ -z "$min" ] || [ "$value" -lt "$min" ]; then
      min=$value
    fi
  done <<< "$aligns"
  printf -v min_hex '0x%x' "$min"
  if [ "$min" -lt "$((MIN_ALIGN))" ]; then
    echo "FAIL $so: minimum PT_LOAD alignment $min_hex (need >= $MIN_ALIGN)"
    status=1
  else
    echo "OK   $so: minimum PT_LOAD alignment $min_hex"
  fi
done

exit "$status"
