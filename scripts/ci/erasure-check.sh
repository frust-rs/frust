#!/usr/bin/env bash
# Erasure tripwire. Guards the any-erasure migration: no redundant any() at an
# erasing API boundary, no literal Column/Row/Stack/FlexView vec-lists outside
# the builder-equivalence test (crates/frust-widgets/tests/builders.rs), and
# (rule T5) no homogeneous `vec![any(a), any(b), ..]` list argument — a list
# whose elements all share one head needs no erasure, since the list API's
# `Vec<V>` takes the concrete type directly. (Rule T6) vec![..] arguments of
# sequence APIs become tuples when all elements are erasure calls (1–12 of them).
# (Rule T7) helpers ending in one erasure call return impl View instead of AnyView.
# Runs the codemod in --check --strict --t5 --t6 --t7 mode, so a site the tool
# had to skip (shadowed builder, comment in the call head, ...) also fails the gate.
#
# To fix a failure: run
#   python3 -I scripts/codemod/frust_any_codemod.py --write --t5 --t6 --t7 <paths>
# then `cargo fmt`; for a shadow skip, rename the local binding/fn named
# column/row/stack that shadows the builder, then rerun this script. T5 judges
# heads, not types: when a same-head list has elements of different types
# (e.g. `component(A{..})` vs `component(B{..})`), keep any() and mark the
# `vec![` line with `// erasure: keep <why>`; the alternatives are to bind the
# vec to a local or to use the fluent builder.
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
exec python3 -I scripts/codemod/frust_any_codemod.py --check --strict --t5 --t6 --t7 \
  crates plugins examples benchmarks \
  --exclude crates/frust-widgets/tests/builders.rs
