#!/usr/bin/env bash
# Browser-tier clock tripwire.
#
# `std::time::Instant::now()` and `SystemTime::now()` compile for wasm32-unknown-unknown and then
# panic at runtime ("time not implemented on this platform"). That is how a std clock read in the
# render stack broke every frust.dev live preview (frust-rs/frust#127). The compiler cannot see it,
# so this gate runs clippy's disallowed_types / disallowed_methods on the wasm32 target over every
# root-workspace crate in examples/web-gallery's wasm32 dependency graph (derived at run time, so
# a new crate pulled into the browser tier is covered automatically), plus the gallery app itself.
#
# The lint config lives in scripts/ci/wasm-lint/clippy.toml and is selected with CLIPPY_CONF_DIR
# (absolute path) so it never binds the native `cargo clippy --workspace` gate. Only lib targets
# are linted: tests do not run on wasm and some dev-deps do not build there. The std SystemTime
# *type* is deliberately allowed (std::fs::Metadata::modified() returns it); only `::now` is not.
#
# To fix a failure: use `web_time::Instant` / `web_time::SystemTime` (same API, backed by
# performance.now() in the browser, std on native) and add `web-time` to the crate's manifest.
#
# Scope: first-party lib code as the root workspace resolves features for these crates. Clock
# reads inside registry crates (wgpu, parley_engine, ...) and behind features the root run does
# not enable (perf-trace, devtools) are not linted; scripts/ci/web-smoke.sh, which boots the real
# build, is the runtime backstop for those.
#
# --self-test additionally proves the lint fires on this toolchain for `use std::time::Instant;
# Instant::now()` and for a fully qualified `std::time::Instant::now()` in a scratch crate, and that
# the clean real graph passes.
#
# Exit codes: 0 pass, 1 lint failure, 2 usage/toolchain error.
set -euo pipefail

TARGET=wasm32-unknown-unknown
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CONF_DIR="$ROOT/scripts/ci/wasm-lint"
LINTS=(-D clippy::disallowed_types -D clippy::disallowed_methods)

self_test=0
case "${1:-}" in
  "") ;;
  --self-test) self_test=1 ;;
  *) echo "usage: $0 [--self-test]" >&2; exit 2 ;;
esac

command -v cargo >/dev/null || { echo "error: cargo not found" >&2; exit 2; }
cd "$ROOT"
if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
  echo "error: target $TARGET not installed (rustup target add $TARGET)" >&2
  exit 2
fi

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

# (a) Root-workspace crates in the web-gallery wasm32 graph.
tree_out="$(cd examples/web-gallery && cargo tree --target "$TARGET" -e normal --prefix none -f '{p}')" \
  || { echo "error: cargo tree failed" >&2; exit 2; }
crates=()
while read -r name _ver path; do
  path="${path#(}"; path="${path%)}"
  case "$path" in
    "$ROOT"/examples/web-gallery*) continue ;;
    "$ROOT"/*) ;;
    *) continue ;;
  esac
  # Only root-workspace members can be addressed with -p from the root. A local
  # crate the root cannot resolve (a standalone workspace pulled into the
  # gallery, e.g. one on the root `exclude` list) must not slip through unlinted.
  if ! cargo pkgid -p "$name" >/dev/null 2>&1; then
    echo "error: $name ($path) is in the web-gallery wasm32 graph but is not a root-workspace member; make it one or lint it from its own workspace in this script" >&2
    exit 2
  fi
  crates+=("$name")
done <<<"$tree_out"
mapfile -t crates < <(printf '%s\n' "${crates[@]}" | sort -u)
if [ "${#crates[@]}" -eq 0 ]; then
  echo "error: derived no crates from the web-gallery graph" >&2
  exit 2
fi
echo "wasm-time-check: ${#crates[@]} root-workspace crates in the web-gallery wasm32 graph:"
printf '  %s\n' "${crates[@]}"
echo "  web-gallery (examples/web-gallery, standalone)"

clippy_real() {
  local pargs=()
  for c in "${crates[@]}"; do pargs+=(-p "$c"); done
  CLIPPY_CONF_DIR="$CONF_DIR" cargo clippy --target "$TARGET" --lib "${pargs[@]}" -- "${LINTS[@]}" || return 1
  (cd examples/web-gallery && CLIPPY_CONF_DIR="$CONF_DIR" cargo clippy --target "$TARGET" -- "${LINTS[@]}") || return 1
}

if [ "$self_test" -eq 1 ]; then
  mkdir -p "$SCRATCH/probe/src"
  cat >"$SCRATCH/probe/Cargo.toml" <<'TOML'
[workspace]

[package]
name = "wasm-time-probe"
version = "0.0.0"
edition = "2024"
publish = false
TOML
  probe() { # label, source, expected lint
    printf '%s\n' "$2" >"$SCRATCH/probe/src/lib.rs"
    local out rc=0
    out="$(cd "$SCRATCH/probe" && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$SCRATCH/target}" CLIPPY_CONF_DIR="$CONF_DIR" \
      cargo clippy --target "$TARGET" --lib -- "${LINTS[@]}" 2>&1)" || rc=$?
    if [ "$rc" -ne 0 ] && grep -q "$3" <<<"$out"; then
      echo "self-test OK: $1 rejected ($3)"
    else
      echo "self-test FAIL: $1 not rejected (exit $rc, wanted $3)" >&2
      echo "$out" >&2
      return 1
    fi
  }
  st=0
  probe "imported Instant::now()" 'use std::time::Instant;
pub fn f() { let _ = Instant::now(); }' "disallowed_" || st=1
  probe "fully qualified std::time::Instant::now()" 'pub fn f() { let _ = std::time::Instant::now(); }' "disallowed_methods" || st=1
  probe "SystemTime::now()" 'pub fn f() { let _ = std::time::SystemTime::now(); }' "disallowed_methods" || st=1
  if [ "$st" -ne 0 ]; then echo "wasm-time-check: self-test FAIL"; exit 1; fi
fi

if clippy_real; then
  echo "wasm-time-check: PASS"
else
  echo "wasm-time-check: FAIL (std clock read in the wasm32 graph; use web_time, see header)" >&2
  exit 1
fi
