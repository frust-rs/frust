#!/usr/bin/env bash
# Erasure tripwire. Guards the any-erasure migration: no redundant any() at an
# erasing API boundary, and no literal Column/Row/Stack/FlexView vec-lists
# outside the builder-equivalence test (crates/frust-widgets/tests/builders.rs).
# Runs the codemod in --check --strict mode, so a site the tool had to skip
# (shadowed builder, comment in the call head, ...) also fails the gate.
#
# To fix a failure: run
#   python3 -I scripts/codemod/frust_any_codemod.py --write <paths>
# then `cargo fmt`; for a shadow skip, rename the local binding/fn named
# column/row/stack that shadows the builder, then rerun this script.
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
exec python3 -I scripts/codemod/frust_any_codemod.py --check --strict \
  crates plugins examples benchmarks \
  --exclude crates/frust-widgets/tests/builders.rs
