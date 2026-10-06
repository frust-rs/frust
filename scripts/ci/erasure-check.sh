#!/usr/bin/env bash
# Erasure tripwire. Guards the any-erasure migration: no redundant any() at an
# erasing API boundary, no literal Column/Row/Stack/FlexView vec-lists outside
# the builder-equivalence test (crates/frust-widgets/tests/builders.rs), and
# (rule T5) no homogeneous `vec![any(a), any(b), ..]` list argument — a list
# whose elements all share one head needs no erasure, since the list API's
# `Vec<V>` takes the concrete type directly. Runs the codemod in
# --check --strict --t5 mode, so a site the tool had to skip (shadowed
# builder, comment in the call head, ...) also fails the gate.
#
# To fix a failure: run
#   python3 -I scripts/codemod/frust_any_codemod.py --write --t5 <paths>
# then `cargo fmt`; for a shadow skip, rename the local binding/fn named
# column/row/stack that shadows the builder, then rerun this script. Keep
# any() only where the elements genuinely differ in type — such a list is not
# homogeneous and the tool does not flag it.
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
exec python3 -I scripts/codemod/frust_any_codemod.py --check --strict --t5 \
  crates plugins examples benchmarks \
  --exclude crates/frust-widgets/tests/builders.rs
