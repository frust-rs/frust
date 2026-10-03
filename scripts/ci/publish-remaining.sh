#!/usr/bin/env bash
# List the workspace packages whose current version is not on crates.io yet,
# and optionally publish them.
#
# Usage: scripts/ci/publish-remaining.sh [--publish] [--no-verify] [--dry-run]
#   (no flags)    print the unpublished packages, one `name@version` per line
#   --publish     run `cargo publish` for exactly those packages (cargo orders
#                 them by dependency and waits for each to reach the index)
#   --no-verify   with --publish: skip the verification build of each package
#   --dry-run     with --publish: pass --dry-run to cargo (nothing is uploaded)
#
# A package counts as published when the crates.io sparse index lists its
# version. crates.io rate-limits new crates (a burst, then one per ten
# minutes): when cargo stops on that limit this script exits 1 after printing
# the packages still missing and the retry time from cargo's message, so
# re-running it later resumes with the remainder. With nothing left to publish
# it prints nothing and exits 0.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

publish=0
verify_flag=()
dry_run_flag=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --publish) publish=1 ;;
    --no-verify) verify_flag=(--no-verify) ;;
    --dry-run) dry_run_flag=(--dry-run) ;;
    *) echo "error: unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

user_agent="frust-release-check (https://github.com/frust-rs/frust)"

# The sparse index shards names by length: 1/<n>, 2/<n>, 3/<c>/<n>, <ab>/<cd>/<n>.
index_url() {
  local name="$1" lower
  lower="$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]')"
  case "${#lower}" in
    1) printf 'https://index.crates.io/1/%s' "$lower" ;;
    2) printf 'https://index.crates.io/2/%s' "$lower" ;;
    3) printf 'https://index.crates.io/3/%s/%s' "${lower:0:1}" "$lower" ;;
    *) printf 'https://index.crates.io/%s/%s/%s' "${lower:0:2}" "${lower:2:2}" "$lower" ;;
  esac
}

# "name version" per publishable package (`publish` null in cargo metadata).
packages="$(cargo metadata --no-deps --format-version 1 | python3 -c '
import json, sys
for p in json.load(sys.stdin)["packages"]:
    if p.get("publish") is None:
        print(p["name"], p["version"])
' | sort)"

missing=()
while read -r name version; do
  [ -n "$name" ] || continue
  url="$(index_url "$name")"
  body="$(curl -sS -f -A "$user_agent" "$url" 2>/dev/null || true)"
  if [ -z "$body" ] || ! printf '%s\n' "$body" | grep -q "\"vers\":\"$version\""; then
    missing+=("$name@$version")
  fi
done <<EOT
$packages
EOT

if [ "${#missing[@]}" -eq 0 ]; then
  [ "$publish" -eq 1 ] && echo "nothing left to publish" >&2
  exit 0
fi

printf '%s\n' "${missing[@]}"
[ "$publish" -eq 1 ] || exit 0

args=(publish --locked "${verify_flag[@]}" "${dry_run_flag[@]}")
for m in "${missing[@]}"; do
  args+=(-p "${m%@*}")
done

echo "cargo ${args[*]}" >&2
set +e
cargo "${args[@]}" 2>&1 | tee /dev/stderr > /tmp/publish-remaining.$$.log
status="${PIPESTATUS[0]}"
set -e

if [ "$status" -ne 0 ]; then
  if grep -q -i -E 'too many|rate limit|try again' /tmp/publish-remaining.$$.log; then
    echo "stopped on the crates.io rate limit; retry after:" >&2
    grep -o -i -E 'try again after[^.]*' /tmp/publish-remaining.$$.log | head -1 >&2 || true
  else
    echo "cargo publish failed (exit $status); see the output above" >&2
  fi
  echo "still missing:" >&2
  "$0" >&2 || true
  rm -f /tmp/publish-remaining.$$.log
  exit 1
fi
rm -f /tmp/publish-remaining.$$.log
exit 0
