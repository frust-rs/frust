#!/usr/bin/env bash
# Copy the root licence texts into every publishable package directory.
#
# Usage: scripts/sync-licenses.sh [--check] [--only <dir>...]
#   --check        write nothing; exit 1 naming every missing or differing copy
#   --only <dir>…  restrict to the given package directories (repo-relative)
#
# Publishable packages are those `cargo metadata --no-deps` reports with a null
# `publish`. A package whose `license` expression lacks `MIT` receives only
# LICENSE-APACHE (today: frust-material, `Apache-2.0 AND OFL-1.1`).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

check=0
only=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --check) check=1; shift ;;
    --only)
      shift
      while [ "$#" -gt 0 ] && [ "${1#--}" = "$1" ]; do
        only+=("${1%/}"); shift
      done
      if [ "${#only[@]}" -eq 0 ]; then
        echo "error: --only requires at least one package directory" >&2
        exit 2
      fi
      ;;
    *) echo "error: unknown argument: $1" >&2; exit 2 ;;
  esac
done

for f in LICENSE-MIT LICENSE-APACHE; do
  [ -f "$f" ] || { echo "error: $f missing at repository root" >&2; exit 2; }
done

# Lines of "<repo-relative dir>\t<license expression>" for publishable packages.
pkgs="$(cargo metadata --no-deps --format-version 1 | python3 -c '
import json, os, sys
root = os.getcwd()
for p in json.load(sys.stdin)["packages"]:
    if p.get("publish") is None:
        d = os.path.relpath(os.path.dirname(p["manifest_path"]), root)
        print("%s\t%s" % (d, p.get("license") or ""))
' | sort)"

if [ "${#only[@]}" -gt 0 ]; then
  for d in "${only[@]}"; do
    if ! printf '%s\n' "$pkgs" | cut -f1 | grep -Fxq -- "$d"; then
      echo "error: '$d' is not a publishable workspace package directory" >&2
      exit 2
    fi
  done
fi

bad=0
while IFS="$(printf '\t')" read -r dir lic; do
  [ -n "$dir" ] || continue
  if [ "${#only[@]}" -gt 0 ]; then
    keep=0
    for d in "${only[@]}"; do [ "$d" = "$dir" ] && keep=1; done
    [ "$keep" -eq 1 ] || continue
  fi
  files="LICENSE-APACHE"
  case "$lic" in *MIT*) files="LICENSE-MIT LICENSE-APACHE" ;; esac
  for f in $files; do
    if [ "$check" -eq 1 ]; then
      if [ ! -f "$dir/$f" ]; then
        echo "missing: $dir/$f"; bad=1
      elif ! cmp -s "$f" "$dir/$f"; then
        echo "differs: $dir/$f"; bad=1
      fi
    else
      cp "$f" "$dir/$f"
      echo "synced: $dir/$f"
    fi
  done
done <<EOT
$pkgs
EOT

exit "$bad"
