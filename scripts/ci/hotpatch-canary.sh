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
#  - the same for an edit to the app's local path dependency outside the
#    workspace (`shared`, captured through RUSTC_WRAPPER): the thin build
#    replays it and the app lib, and the app answers the sum;
#  - a D2-shaped edit (a field added to the type the hot function returns),
#    then the same shape in `shared`'s type, each a thin build answering
#    RestartRequired { LayoutChanged } with nothing linked, sent or applied.
# The driver restores the edited sources itself; the EXIT trap restores them
# again from copies taken before anything ran, whatever stopped the run.
#
# Any unexpected toolchain or linker shape fails the run; nothing is skipped.
# Runs on macOS and Linux (x86_64/aarch64), the targets the builder supports.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

fixture=testing/hotpatch-canary
source_file="$fixture/app/src/lib.rs"
shared_file="$fixture/shared/src/lib.rs"

# L3 reads DWARF type information, so the fixture builds with full debug info
# whatever the caller sets (CI's workflow env sets 0 for every other job).
export CARGO_PROFILE_DEV_DEBUG=2

scratch="$(mktemp -d "${TMPDIR:-/tmp}/hotpatch-canary.XXXXXX")"
cp "$source_file" "$scratch/lib.rs.orig"
cp "$shared_file" "$scratch/shared.rs.orig"
restore() {
  if ! cmp -s "$scratch/lib.rs.orig" "$source_file"; then
    cp "$scratch/lib.rs.orig" "$source_file"
    echo "hotpatch-canary: restored $source_file from the pre-run copy" >&2
  fi
  if ! cmp -s "$scratch/shared.rs.orig" "$shared_file"; then
    cp "$scratch/shared.rs.orig" "$shared_file"
    echo "hotpatch-canary: restored $shared_file from the pre-run copy" >&2
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
expect 'canary: fat build: replayable local non-members ["frust-hotpatch", "hotpatch-canary-shared"]'
expect "canary: fat link: "
expect "canary: app ready: "
expect "canary: L3: pass"
expect "canary: thin build: linked "
expect "canary: app: applied value=2 hits=1 patches=1"
expect 'canary: thin build: replayed ["hotpatch-canary-shared/hotpatch_canary_shared.lib", "hotpatch-canary-app/hotpatch_canary_app.lib"]'
expect "canary: app: applied value=3 "
expect "canary: RestartRequired { LayoutChanged }: "
expect "canary: no apply: "
expect "canary: shared RestartRequired { LayoutChanged }: "
expect "canary: shared no apply: "
expect "canary: PASS"
applied="$(grep -c '^canary: app: applied' "$log" || true)"
[ "$applied" = 2 ] || fail "expected exactly two applied patches, saw $applied"
passed="$(grep -c '^canary: L3: pass' "$log" || true)"
[ "$passed" = 2 ] || fail "expected L3 to pass exactly two edits, saw $passed"
grep -q '^canary: app: applied value=3 hits=[1-9][0-9]* patches=2$' "$log" ||
  fail "the path-dependency patch did not answer value=3 patches=2"
cmp -s "$scratch/lib.rs.orig" "$source_file" || fail "the driver left $source_file edited"
cmp -s "$scratch/shared.rs.orig" "$shared_file" || fail "the driver left $shared_file edited"

echo "=== hot-patch canary passed ==="
