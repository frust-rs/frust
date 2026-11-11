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
# 32-bit objects (ELF32, e.g. armeabi-v7a) are skipped: the 16 KB rule applies to 64-bit ABIs only.
# An alignment field that is not a hex (0x...) or decimal number fails the file; no part of
# readelf's output is ever evaluated as shell arithmetic.
#
# Exit codes: 0 every checked object passes, 1 at least one object is below 0x4000 or unreadable,
# 2 usage error (bad argument, no .so found, or every object skipped as 32-bit).
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
checked=0

readelf_class() {
  "$readelf_bin" -hW "$1" 2>/dev/null | awk '$1 == "Class:" { print $2; exit }'
}

for so in "${objects[@]}"; do
  # Program-header rows look like: "  LOAD  0x... 0x... 0x... 0x... 0x... R  0x4000"; the
  # alignment is the last field.
  if ! headers="$("$readelf_bin" -lW "$so" 2>/dev/null)"; then
    echo "FAIL $so: not a readable ELF object"
    status=1
    continue
  fi
  # The ELF header block carries "Class:  ELF32" or "Class:  ELF64".
  class="$(readelf_class "$so")"
  if [ "$class" = "ELF32" ]; then
    echo "SKIP $so: 32-bit ABI, not subject to the 16 KB rule"
    continue
  fi
  aligns="$(printf '%s\n' "$headers" | awk '$1 == "LOAD" { print $NF }')"
  if [ -z "$aligns" ]; then
    echo "FAIL $so: no PT_LOAD segments found"
    status=1
    continue
  fi
  min=""
  bad=""
  while IFS= read -r align; do
    if ! [[ "$align" =~ ^0x[0-9a-fA-F]+$ || "$align" =~ ^[0-9]+$ ]]; then
      bad="$align"
      break
    fi
    value=$((align))
    if [ -z "$min" ] || [ "$value" -lt "$min" ]; then
      min=$value
    fi
  done <<< "$aligns"
  if [ -n "$bad" ]; then
    echo "FAIL $so: unrecognised PT_LOAD alignment field '$bad' (expected 0x<hex> or decimal digits)"
    status=1
    continue
  fi
  checked=$((checked + 1))
  printf -v min_hex '0x%x' "$min"
  if [ "$min" -lt "$((MIN_ALIGN))" ]; then
    echo "FAIL $so: minimum PT_LOAD alignment $min_hex (need >= $MIN_ALIGN)"
    status=1
  else
    echo "OK   $so: minimum PT_LOAD alignment $min_hex"
  fi
done

if [ "$checked" -eq 0 ] && [ "$status" -eq 0 ]; then
  echo "error: every object was skipped (32-bit ABI); nothing was checked" >&2
  exit 2
fi

exit "$status"
