#!/usr/bin/env bash
# Hot-patch canary against the pinned toolchain (rust-toolchain.toml). The
# desktop patch builder depends on rustc/cargo/linker formats that are stable
# in practice, not by contract (examples/hotpatch-spike/PORT.md section 6.4):
# this script is the tripwire a toolchain bump must pass.
#
# On the standalone fixture workspace testing/hotpatch-canary it runs, headless:
#  - fat build: capture every rustc invocation, intercept the link, fat-link
#    the image with the anchor exported, seed the L3 base layout table;
#  - launch the fat image, edit the hot function's return value, thin build
#    (replay the tip lib), L3 gate (must pass), stub + thin link + jump table,
#    apply in-process, and assert the new value through the patched HotFn;
#  - a D2-shaped edit (a field added to the type the hot function returns),
#    thin build, assert RestartRequired { LayoutChanged } and that nothing is
#    linked, sent or applied.
# The driver restores the edited source itself; the EXIT trap restores it
# again from a copy taken before anything ran, whatever stopped the run.
#
# Any unexpected toolchain or linker shape fails the run; nothing is skipped.
# Runs on macOS and Linux (x86_64/aarch64), the targets the builder supports.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

fixture=testing/hotpatch-canary
source_file="$fixture/app/src/lib.rs"

# L3 reads DWARF type information, so the fixture builds with full debug info
# whatever the caller sets (CI's workflow env sets 0 for every other job).
export CARGO_PROFILE_DEV_DEBUG=2

scratch="$(mktemp -d "${TMPDIR:-/tmp}/hotpatch-canary.XXXXXX")"
cp "$source_file" "$scratch/lib.rs.orig"
restore() {
  if ! cmp -s "$scratch/lib.rs.orig" "$source_file"; then
    cp "$scratch/lib.rs.orig" "$source_file"
    echo "hotpatch-canary: restored $source_file from the pre-run copy" >&2
  fi
  rm -rf "$scratch"
}
trap restore EXIT

fail() {
  echo "hotpatch-canary: FAIL: $*" >&2
  exit 1
}

echo "=== toolchain ==="
rustc -vV
cargo -V

echo "=== build the canary driver ==="
driver="$(
  cd "$fixture" &&
    cargo build --locked -p hotpatch-canary-driver --message-format=json-render-diagnostics |
    sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -n 1
)"
[ -n "$driver" ] && [ -x "$driver" ] || fail "cargo reported no driver executable"
echo "driver: $driver"

echo "=== run the canary ==="
log="$scratch/canary.log"
(cd "$fixture" && "$driver" .) 2>&1 | tee "$log"

# The driver fails closed on its own; these re-check its evidence so a driver
# that exits 0 without having run a stage still fails the gate.
expect() {
  grep -Fq -- "$1" "$log" || fail "missing from the canary output: $1"
}
expect "canary: fat build: captured rustc records"
expect "canary: fat link: "
expect "canary: app ready: "
expect "canary: L3: pass"
expect "canary: thin build: linked "
expect "canary: app: applied value=2 hits=1 patches=1"
expect "canary: RestartRequired { LayoutChanged }: "
expect "canary: no apply: "
expect "canary: PASS"
applied="$(grep -c '^canary: app: applied' "$log" || true)"
[ "$applied" = 1 ] || fail "expected exactly one applied patch, saw $applied"
cmp -s "$scratch/lib.rs.orig" "$source_file" || fail "the driver left $source_file edited"

echo "=== hot-patch canary passed ==="
