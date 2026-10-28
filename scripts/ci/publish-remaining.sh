#!/usr/bin/env bash
# List the workspace packages whose current version is not on crates.io yet,
# and optionally publish them.
#
# Usage: scripts/ci/publish-remaining.sh [--publish] [--no-verify] [--dry-run]
#        scripts/ci/publish-remaining.sh --self-test
#   (no flags)    print the unpublished packages, one `name@version` per line
#                 on stdout; brand-new crates (see below) are listed on stderr
#                 in a labelled block instead
#   --publish     run `cargo publish` for exactly those packages (cargo orders
#                 them by dependency and waits for each to reach the index)
#   --no-verify   with --publish: skip the verification build of each workspace
#                 package (the standalone packages are always verified)
#   --dry-run     with --publish: pass --dry-run to cargo (nothing is uploaded);
#                 the standalone packages are reported as not verifiable
#                 instead of attempted, see below
#   --self-test   offline: swap the index lookup and the package enumeration
#                 for a fixture and assert the three lists and the exit code;
#                 no network, no cargo
#
# A package counts as published when the crates.io sparse index lists its
# version. An index 404 means the crate itself was never published: such a
# NEW crate is kept apart from a missing version of an existing crate, because
# Trusted Publishing can only be configured for a crate that already exists, so
# CI cannot create one. `--publish` therefore publishes only existing crates
# (the workspace batch, then the standalone package) and never a new crate.
#
# Exit codes with --publish:
#   0  everything is on the index (or was just published)
#   1  a plain failure: cargo failed, or the crates.io rate limit stopped it
#      (the message says "rate limit"; crates.io rate-limits new crates, a
#      burst then one per ten minutes, so the script prints the packages still
#      missing and the retry time from cargo's message, and re-running it later
#      resumes with the remainder); also any classification failure (index
#      lookup, package enumeration), which never counts a crate as published
#   3  the existing crates are up but new crates remain: a maintainer must
#      first-publish them from the `vX.Y.Z` checkout (the commands are printed
#      on stderr); re-run afterwards. Distinct from 1 so CI can tell them apart.
# (2 is a usage error.) With nothing left to publish it prints nothing and
# exits 0.
#
# `plugins/clean-signals-frust` is a standalone workspace excluded from the
# root graph (it pins its own `clean-signals`), so `cargo metadata` at the
# root never lists it. It is enumerated separately below and published LAST,
# one `cargo publish --manifest-path` at a time, always with the verification
# build: its `frust-ui` dependency is a path dependency with a version, which
# cargo resolves from the registry, so it can only be packaged once the
# workspace packages are up — which is also why a dry run skips it.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

standalone_manifests=(plugins/clean-signals-frust/Cargo.toml)

publish=0
self_test=0
verify_flag=()
dry_run_flag=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --publish) publish=1 ;;
    --no-verify) verify_flag=(--no-verify) ;;
    --dry-run) dry_run_flag=(--dry-run) ;;
    --self-test) self_test=1 ;;
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

# Prints an error and exits 1 (a plain failure, distinct from 3). `exit` is
# not subject to `set -e` suppression, so this fails closed in any context.
die() {
  echo "error: $*" >&2
  exit 1
}

# "name version" per publishable package (`publish` null in cargo metadata)
# of the manifest given as `$1` (the root workspace when omitted). Fails (via
# pipefail, and on an empty result) rather than printing nothing.
publishable() {
  cargo metadata --no-deps --format-version 1 ${1:+--manifest-path "$1"} | python3 -c '
import json, sys
found = [(p["name"], p["version"]) for p in json.load(sys.stdin)["packages"]
         if p.get("publish") is None]
if not found:
    sys.exit("no publishable packages found")
for name, version in found:
    print(name, version)
' | sort
}

# Prints how the index sees `name@version`: `published` (version listed),
# `missing` (crate exists, version absent) or `new` (HTTP 404: the crate was
# never published). Any other failure (network, 5xx) returns non-zero, because
# treating it as "not published" would misreport every package; callers check.
index_status() {
  local name="$1" version="$2" body code
  body="$(curl -sS -A "$user_agent" -w '\n%{http_code}' "$(index_url "$name")" 2>&1)" || {
    echo "error: index lookup for $name failed: ${body##*$'\n'}" >&2; return 1; }
  code="${body##*$'\n'}"
  body="${body%$'\n'*}"
  case "$code" in
    200)
      if printf '%s\n' "$body" | grep -q "\"vers\":\"$version\""; then
        echo published
      else
        echo missing
      fi ;;
    404) echo new ;;
    *) echo "error: index lookup for $name returned HTTP $code" >&2; return 1 ;;
  esac
}

# The three lists, filled by `classify`:
#   missing             name@version of existing crates lacking this version
#   new_crates          name@version of crates the index has never seen
#   missing_standalone  name@version@manifest of existing standalone crates
#                       lacking this version (published last)
missing=()
new_crates=()
missing_standalone=()

# `main` runs on the left of `||`, where bash disables `set -e` inside every
# function it calls, so nothing here relies on errexit: each substitution is
# checked explicitly and every unexpected result dies with exit 1.
classify() {
  local workspace_packages standalone_packages manifest name version status
  workspace_packages="$(publishable)" || die "package enumeration failed for the root workspace"
  [ -n "$workspace_packages" ] || die "package enumeration returned nothing for the root workspace"
  while read -r name version; do
    [ -n "$name" ] || continue
    status="$(index_status "$name" "$version")" || die "index lookup failed for $name@$version"
    case "$status" in
      published) ;;
      missing) missing+=("$name@$version") ;;
      new) new_crates+=("$name@$version") ;;
      *) die "index lookup for $name@$version returned unexpected status [$status]" ;;
    esac
  done <<EOT
$workspace_packages
EOT

  for manifest in "${standalone_manifests[@]}"; do
    standalone_packages="$(publishable "$manifest")" || die "package enumeration failed for $manifest"
    [ -n "$standalone_packages" ] || die "package enumeration returned nothing for $manifest"
    while read -r name version; do
      [ -n "$name" ] || continue
      status="$(index_status "$name" "$version")" || die "index lookup failed for $name@$version"
      case "$status" in
        published) ;;
        missing) missing_standalone+=("$name@$version@$manifest") ;;
        new) new_crates+=("$name@$version") ;;
        *) die "index lookup for $name@$version returned unexpected status [$status]" ;;
      esac
    done <<EOT
$standalone_packages
EOT
  done
}

# The block a maintainer acts on: the labelled list, which listing mode and
# the exit-3 path both print.
print_new_crates() {
  local m
  echo "new crates (need a maintainer's first publish from the tag):" >&2
  for m in "$@"; do
    printf '%s\n' "$m" >&2
  done
}

# The exact commands for the exit-3 path, run from a `vX.Y.Z` checkout.
print_first_publish_steps() {
  local m crate
  echo "" >&2
  echo "A maintainer publishes each new crate once, from a checkout of the release tag" >&2
  echo "(its dependencies are on the index by now). For each crate:" >&2
  for m in "$@"; do
    crate="${m%@*}"
    echo "" >&2
    echo "  # $m" >&2
    echo "  cargo publish -p $crate --locked" >&2
    echo "  # then on crates.io, crate settings > Trusted Publishing: add a GitHub publisher" >&2
    echo "  #   repository frust-rs/frust, workflow release.yml, environment release" >&2
    echo "  cargo owner --add github:frust-rs:owners $crate" >&2
  done
  echo "" >&2
  echo "Then re-run the release workflow for the tag." >&2
}

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

main() {
  local m args
  classify

  if [ "${#missing[@]}" -eq 0 ] && [ "${#missing_standalone[@]}" -eq 0 ] \
    && [ "${#new_crates[@]}" -eq 0 ]; then
    [ "$publish" -eq 1 ] && echo "nothing left to publish" >&2
    return 0
  fi

  # Empty arrays expand with `${arr[@]+"${arr[@]}"}`: bash before 4.4 treats a
  # bare `"${arr[@]}"` on an empty array as an unbound variable under `set -u`.
  for m in ${missing[@]+"${missing[@]}"}; do
    printf '%s\n' "$m"
  done
  for m in ${missing_standalone[@]+"${missing_standalone[@]}"}; do
    printf '%s\n' "${m%@*}"
  done
  if [ "${#new_crates[@]}" -gt 0 ]; then
    print_new_crates "${new_crates[@]}"
  fi
  [ "$publish" -eq 1 ] || return 0

  if [ "${#missing[@]}" -gt 0 ]; then
    args=(publish --locked ${verify_flag[@]+"${verify_flag[@]}"} ${dry_run_flag[@]+"${dry_run_flag[@]}"})
    for m in "${missing[@]}"; do
      args+=(-p "${m%@*}")
    done
    run_publish "${args[@]}"
  fi

  # The standalone packages go last, after every workspace package is up, and
  # always with the verification build: that build is the only check they get
  # before the irreversible upload. A dry run cannot do it at all — their
  # `frust-ui` dependency is resolved from the registry, where this version
  # does not exist until a real run has uploaded the workspace packages.
  for m in ${missing_standalone[@]+"${missing_standalone[@]}"}; do
    if [ "${#dry_run_flag[@]}" -gt 0 ]; then
      echo "${m%@*}: not verifiable in a dry run (its frust-ui dependency is resolved from the registry); a real run publishes it last" >&2
      continue
    fi
    run_publish publish --locked --manifest-path "${m##*@}"
  done

  # New crates are never published here: Trusted Publishing cannot create a
  # crate. Exit 3 tells the workflow a maintainer's first publish is needed.
  if [ "${#new_crates[@]}" -gt 0 ]; then
    print_first_publish_steps "${new_crates[@]}"
    return 3
  fi
  return 0
}

# Offline check: the fixture replaces the package enumeration and the index
# lookup, and `run_publish` only records what it would have published. Nothing
# touches the network or cargo.
self_test_main() {
  local failures=0 tmp out err code
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/publish-remaining-selftest.XXXXXX")"

  publishable() {
    if [ -n "${1:-}" ]; then
      echo "frust-standalone 1.0.0"
    else
      printf '%s\n' "frust-old 1.0.0" "frust-fresh 1.0.0" "frust-bumped 2.0.0" \
        "frust-newer 0.1.0"
    fi
  }
  index_status() {
    case "$1" in
      frust-old) echo published ;;
      frust-bumped) echo missing ;;
      frust-fresh | frust-newer) echo new ;;
      frust-standalone) echo missing ;;
      *) echo "self-test: unexpected lookup $1" >&2; exit 1 ;;
    esac
  }
  run_publish() { echo "$*" >> "$tmp/published"; }
  standalone_manifests=(fixture/Cargo.toml)

  check() { # description expected actual
    if [ "$2" != "$3" ]; then
      echo "FAIL $1: expected [$2] got [$3]" >&2
      failures=$((failures + 1))
    fi
  }

  # Listing mode: three lists, exit 0.
  publish=0
  set +e
  out="$( (main) 2> "$tmp/err")"; code=$?
  set -e
  err="$(cat "$tmp/err")"
  check "listing exit" 0 "$code"
  check "listing stdout" "frust-bumped@2.0.0
frust-standalone@1.0.0" "$out"
  check "listing stderr" "new crates (need a maintainer's first publish from the tag):
frust-fresh@1.0.0
frust-newer@0.1.0" "$err"

  # Publish mode: existing crates only, then exit 3.
  publish=1
  set +e
  out="$( (main) 2> "$tmp/err")"; code=$?
  set -e
  err="$(cat "$tmp/err")"
  check "publish exit" 3 "$code"
  check "published set" "publish --locked -p frust-bumped
publish --locked --manifest-path fixture/Cargo.toml" "$(cat "$tmp/published")"
  case "$err" in
    *"cargo publish -p frust-fresh --locked"*"cargo owner --add github:frust-rs:owners frust-fresh"*"cargo publish -p frust-newer --locked"*) ;;
    *) echo "FAIL publish stderr lacks the first-publish commands" >&2; failures=$((failures + 1)) ;;
  esac
  case "$err" in
    *"cargo publish -p frust-bumped"* | *"cargo publish -p frust-old"*)
      echo "FAIL publish stderr names an existing crate" >&2; failures=$((failures + 1)) ;;
  esac

  # Nothing new: exit 0. Only published crates: exit 0.
  # shellcheck disable=SC2329  # invoked indirectly by main via classify
  publishable() { echo "frust-old 1.0.0"; }
  : > "$tmp/published"
  set +e
  out="$( (main) 2> "$tmp/err")"; code=$?
  set -e
  check "all-published exit" 0 "$code"
  check "all-published published-set" "" "$(cat "$tmp/published")"

  # Failure fixtures: each must exit 1 (not 0, not 3), publish nothing, and
  # list nothing on stdout. `main` runs as `main || code=$?` like the real
  # call, the context where errexit is off inside functions.
  expect_fail() { # description
    : > "$tmp/published"
    missing=(); new_crates=(); missing_standalone=()
    set -e
    code=0
    out="$( (main) 2> "$tmp/err")" || code=$?
    set -e
    check "$1 exit" 1 "$code"
    check "$1 published-set" "" "$(cat "$tmp/published")"
    check "$1 stdout" "" "$out"
    case "$(cat "$tmp/err")" in
      *"error:"*) ;;
      *) echo "FAIL $1: no error message" >&2; failures=$((failures + 1)) ;;
    esac
  }
  publish=1
  # shellcheck disable=SC2329  # invoked indirectly by main via classify
  publishable() {
    if [ -n "${1:-}" ]; then echo "frust-standalone 1.0.0"
    else printf '%s\n' "frust-old 1.0.0" "frust-bumped 2.0.0"; fi
  }
  # shellcheck disable=SC2329
  index_status() {
    case "$1" in
      frust-old) echo published ;;
      frust-bumped) return 1 ;;
      *) echo missing ;;
    esac
  }
  expect_fail "index lookup fails (non-zero)"
  # shellcheck disable=SC2329
  index_status() { [ "$1" = frust-bumped ] && return 0 || echo published; }
  expect_fail "index lookup yields empty status"
  # shellcheck disable=SC2329
  index_status() { echo bogus; }
  expect_fail "index lookup yields unknown status"
  # shellcheck disable=SC2329
  index_status() { echo published; }
  # shellcheck disable=SC2329
  publishable() { return 1; }
  expect_fail "enumeration fails (workspace)"
  # shellcheck disable=SC2329
  publishable() { :; }
  expect_fail "enumeration empty (workspace)"
  # shellcheck disable=SC2329
  publishable() { if [ -n "${1:-}" ]; then return 1; else echo "frust-old 1.0.0"; fi; }
  expect_fail "enumeration fails (standalone)"
  # shellcheck disable=SC2329
  publishable() { if [ -n "${1:-}" ]; then :; else echo "frust-old 1.0.0"; fi; }
  expect_fail "enumeration empty (standalone)"

  rm -rf "$tmp"
  if [ "$failures" -ne 0 ]; then
    echo "self-test: $failures check(s) failed" >&2
    return 1
  fi
  echo "self-test: ok" >&2
}

if [ "$self_test" -eq 1 ]; then
  self_test_main
  exit $?
fi

# `|| code=$?` keeps `set -e` from ending the script before the exit code of
# `main` (0, or 3 for new crates) is passed on.
code=0
main || code=$?
exit "$code"
