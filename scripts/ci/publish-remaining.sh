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
#
# `plugins/clean-signals-frust` is a standalone workspace excluded from the
# root graph (it pins its own `clean-signals`), so `cargo metadata` at the
# root never lists it. It is enumerated separately below and published LAST,
# one `cargo publish --manifest-path` at a time: its `frust-ui` dependency is
# a path dependency with a version, which cargo can only resolve from the
# registry once the workspace packages are up.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

standalone_manifests=(plugins/clean-signals-frust/Cargo.toml)

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

# "name version" per publishable package (`publish` null in cargo metadata)
# of the manifest given as `$1` (the root workspace when omitted).
publishable() {
  cargo metadata --no-deps --format-version 1 ${1:+--manifest-path "$1"} | python3 -c '
import json, sys
for p in json.load(sys.stdin)["packages"]:
    if p.get("publish") is None:
        print(p["name"], p["version"])
' | sort
}

# Whether the index already lists `name@version`.
published() {
  local name="$1" version="$2" body
  body="$(curl -sS -f -A "$user_agent" "$(index_url "$name")" 2>/dev/null || true)"
  [ -n "$body" ] && printf '%s\n' "$body" | grep -q "\"vers\":\"$version\""
}

missing=()
while read -r name version; do
  [ -n "$name" ] || continue
  published "$name" "$version" || missing+=("$name@$version")
done <<EOT
$(publishable)
EOT

# `name@version@manifest` per standalone package not on the index yet.
missing_standalone=()
for manifest in "${standalone_manifests[@]}"; do
  while read -r name version; do
    [ -n "$name" ] || continue
    published "$name" "$version" || missing_standalone+=("$name@$version@$manifest")
  done <<EOT
$(publishable "$manifest")
EOT
done

if [ "${#missing[@]}" -eq 0 ] && [ "${#missing_standalone[@]}" -eq 0 ]; then
  [ "$publish" -eq 1 ] && echo "nothing left to publish" >&2
  exit 0
fi

[ "${#missing[@]}" -eq 0 ] || printf '%s\n' "${missing[@]}"
for m in "${missing_standalone[@]}"; do
  printf '%s\n' "${m%@*}"
done
[ "$publish" -eq 1 ] || exit 0

# Runs one `cargo publish` invocation; on failure prints the reason, the
# remainder, and exits 1 so a re-run resumes.
run_publish() {
  local log status
  log="$(mktemp "${TMPDIR:-/tmp}/publish-remaining.XXXXXX")"
  echo "cargo $*" >&2
  set +e
  cargo "$@" 2>&1 | tee /dev/stderr > "$log"
  status="${PIPESTATUS[0]}"
  set -e
  if [ "$status" -ne 0 ]; then
    if grep -q -i -E 'too many|rate limit|try again' "$log"; then
      echo "stopped on the crates.io rate limit; retry after:" >&2
      grep -o -i -E 'try again after[^.]*' "$log" | head -1 >&2 || true
    else
      echo "cargo publish failed (exit $status); see the output above" >&2
    fi
    echo "still missing:" >&2
    "$0" >&2 || true
    rm -f "$log"
    exit 1
  fi
  rm -f "$log"
}

if [ "${#missing[@]}" -gt 0 ]; then
  args=(publish --locked "${verify_flag[@]}" "${dry_run_flag[@]}")
  for m in "${missing[@]}"; do
    args+=(-p "${m%@*}")
  done
  run_publish "${args[@]}"
fi

# The standalone packages go last, after every workspace package is up.
for m in "${missing_standalone[@]}"; do
  run_publish publish --locked "${verify_flag[@]}" "${dry_run_flag[@]}" --manifest-path "${m##*@}"
done
exit 0
