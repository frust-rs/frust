#!/usr/bin/env bash
# scripts/widget-snapshots.sh — render the website's widget preview PNGs.
#
# Wraps `cargo run -p frust-testing --bin widget-snapshots`, the generator that
# walks the `frust-gallery` case registry (examples/gallery) and writes, per
# case, a light and a dark PNG plus a manifest.json describing them. Rendering
# is GPU-free: the binary's default backend is frust-testing's `vello_cpu`
# oracle, so this runs on a machine with no adapter at all. (The binary's own
# `--gpu` flag renders through a real adapter instead; it is a maintainer's
# eyeball tool, deliberately NOT exposed here — website assets come from the
# deterministic CPU arm.)
#
# The generated tree is build output, not source: it belongs under target/ and
# is rsynced into the website's static/preview/ tree, never committed to this
# repository.
#
# Usage: scripts/widget-snapshots.sh [--out DIR] [--check] [--filter SUBSTR]
#   --out DIR        Output directory (default: target/widget-snapshots),
#                    relative to the repository root unless absolute.
#   --check          Re-render and byte-compare against DIR instead of writing.
#                    Exits non-zero, listing every file that differs or is
#                    missing. This is the determinism gate.
#   --filter SUBSTR  Only render cases whose slug contains SUBSTR.
#
# Toolchain: this script cd's to the repository root before invoking cargo, so
# rust-toolchain.toml's pinned channel (docs/DEVELOPMENT.md's Version-Pin
# Policy) is what rustup resolves — same as scripts/api-docs.sh. Do not add a
# `+channel` override here; it would fork from that pin.
#
# Outputs: prints the output directory and the rsync line that publishes it.
# Exits: 0 on success (rendered, or --check found no differences);
#        1 on failure (a case failed to render, or --check found differences).

set -euo pipefail

# --- Derive repo root from script location ------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"

# --- Arg parsing and config --------------------------------------------------

OUT_DIR="target/widget-snapshots"
CHECK_MODE=0
FILTER=""

while [ $# -gt 0 ]; do
  case "$1" in
    --out)
      if [ $# -lt 2 ]; then
        echo "error: --out requires a directory argument" >&2
        exit 1
      fi
      OUT_DIR="$2"
      shift 2
      ;;
    --check)
      CHECK_MODE=1
      shift
      ;;
    --filter)
      if [ $# -lt 2 ]; then
        echo "error: --filter requires a substring argument" >&2
        exit 1
      fi
      FILTER="$2"
      shift 2
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

# --- Render (or check) -------------------------------------------------------

cd "$REPO_ROOT"

ARGS=(--out "$OUT_DIR")
if [ -n "$FILTER" ]; then
  ARGS+=(--filter "$FILTER")
fi
if [ "$CHECK_MODE" -eq 1 ]; then
  ARGS+=(--check)
  echo "Checking widget snapshots against $OUT_DIR..." >&2
else
  echo "Rendering widget snapshots into $OUT_DIR..." >&2
fi

# --locked: this generator must never be the thing that moves Cargo.lock.
cargo run --locked -p frust-testing --bin widget-snapshots -- "${ARGS[@]}"

# --- Print the publish line and exit -----------------------------------------

if [ "$CHECK_MODE" -eq 1 ]; then
  echo "✓ $OUT_DIR matches a fresh render" >&2
  exit 0
fi

echo "Widget snapshots generated: $OUT_DIR" >&2
echo "Publish them to the website with:" >&2
echo "  rsync -a --delete --exclude .gitkeep $OUT_DIR/ apps/website/static/preview/" >&2

exit 0
