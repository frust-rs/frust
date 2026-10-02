#!/usr/bin/env bash
# scripts/clean-all.sh — remove every build-output directory in this checkout.
#
# The repo-wide equivalent of `cargo clean`: the root workspace's `target/`,
# every standalone example/benchmark workspace's `target/` and `build/`
# (`build/rust`, `build/ios`, ...), Android Gradle outputs (`build/`,
# `.gradle/`, `.cxx/`, `.externalNativeBuild/`), SwiftPM `.build/`, and
# Flutter's `.dart_tool/`.
#
# Usage: scripts/clean-all.sh [--dry-run]
#   --dry-run   List what would be removed (with sizes) without deleting.
#
# Safety: a candidate directory is only removed if git tracks no file inside
# it, so a source directory that happens to be named `build` is never
# touched. Skipped entirely: `.git/`, `workflow/` (separate nested repo),
# `.claude/` (agent worktrees), `tmp/` (scratch), and `node_modules/`.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"

DRY_RUN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run|-n) DRY_RUN=1; shift ;;
    -h|--help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "error: unknown argument: $1" >&2; exit 2 ;;
  esac
done

cd "${REPO_ROOT}"

# Collect candidates, pruning at each match so nested outputs (e.g.
# `android/app/build` under an already-matched `android/build`) are not
# listed twice.
candidates=()
while IFS= read -r -d '' dir; do
  candidates+=("${dir#./}")
done < <(
  find . \
    \( -path ./.git -o -path ./workflow -o -path ./.claude -o -path ./tmp \
       -o -name node_modules \) -prune \
    -o -type d \( -name target -o -name build -o -name .gradle -o -name .cxx \
       -o -name .externalNativeBuild -o -name .build -o -name .dart_tool \) \
       -prune -print0
)

if [ ${#candidates[@]} -eq 0 ]; then
  echo "Nothing to clean."
  exit 0
fi

removed=0
skipped=0
for dir in "${candidates[@]}"; do
  if [ -n "$(git ls-files -- "${dir}" | head -n 1)" ]; then
    echo "skip   ${dir} (contains tracked files)"
    skipped=$((skipped + 1))
    continue
  fi
  size="$(du -sh "${dir}" 2>/dev/null | cut -f1)"
  if [ "${DRY_RUN}" -eq 1 ]; then
    echo "would  ${dir} (${size})"
  else
    rm -rf "${dir}"
    echo "remove ${dir} (${size})"
  fi
  removed=$((removed + 1))
done

if [ "${DRY_RUN}" -eq 1 ]; then
  echo "Dry run: ${removed} director(ies) would be removed, ${skipped} skipped."
else
  echo "Removed ${removed} director(ies), ${skipped} skipped."
fi
