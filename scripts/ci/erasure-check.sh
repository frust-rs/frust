#!/usr/bin/env bash
# Erasure tripwire. Two checks, both must pass.
#
# (T) crates, plugins, examples and benchmarks: no redundant any() at an erasing
# API boundary, no literal Column/Row/Stack/FlexView vec-lists outside the
# builder-equivalence test (crates/frust-widgets/tests/builders.rs), and (rule
# T5) no homogeneous `vec![any(a), any(b), ..]` list argument: a list whose
# elements all share one head needs no erasure, since the list API's `Vec<V>`
# takes the concrete type directly. (Rule T6) vec![..] arguments of sequence
# APIs become tuples when all elements are erasure calls (1-12 of them). (Rule
# T7) helpers ending in one erasure call return impl View instead of AnyView.
# Runs the codemod in --check --strict --t5 --t6 --t7 mode, so a site the tool
# had to skip (shadowed builder, comment in the call head, ...) also fails.
#
# (U) examples and benchmarks only (examples/hotpatch-spike is excluded): every
# any() erasure call left in example and benchmark code carries a
# `// erasure: keep <why>` marker, either on the call's own line, alone on the
# line above it, or on the enclosing fn signature (the `fn` line, its `->`
# line, or alone above the `fn`). A call inside a kept `vec![`, `let` or
# `match` first line is covered too. U is report-only: a leftover fails the
# gate and is never auto-fixed. Framework and plugin internals (crates/,
# plugins/) are deliberately not audited by U; a builder or helper there is
# judged by T alone.
#
# To fix a T failure: run
#   python3 -I scripts/codemod/frust_any_codemod.py --write --t5 --t6 --t7 <paths>
# then `cargo fmt`; for a shadow skip, rename the local binding/fn named
# column/row/stack that shadows the builder, then rerun this script. T5 judges
# heads, not types: when a same-head list has elements of different types
# (e.g. `component(A{..})` vs `component(B{..})`), keep any() and mark the
# `vec![` line with `// erasure: keep <why>`; the alternatives are to bind the
# vec to a local or to use the fluent builder.
#
# To fix a U failure: use the 0.6 idioms instead of erasure: fluent builders
# (column()/row()/stack() .child(..)), tuples for mixed sequences (T6),
# either() for two-arm tails, and impl View / impl ViewSeq helper parameters.
# Only where erasure is genuinely required, mark the site with
# `// erasure: keep <why>`. Do not add a marker merely to silence the gate.
#
# Usage: scripts/ci/erasure-check.sh   (no arguments; runs both checks)
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

rc_t=0
rc_u=0
python3 -I scripts/codemod/frust_any_codemod.py --check --strict --t5 --t6 --t7 \
  crates plugins examples benchmarks \
  --exclude crates/frust-widgets/tests/builders.rs || rc_t=$?
python3 -I scripts/codemod/frust_any_codemod.py --check --unmarked examples benchmarks \
  --exclude examples/hotpatch-spike || rc_u=$?

if [ "$rc_t" -ne 0 ]; then
  echo "erasure-check: rule T check failed (exit $rc_t)" >&2
  exit "$rc_t"
fi
if [ "$rc_u" -ne 0 ]; then
  echo "erasure-check: rule U check failed (exit $rc_u)" >&2
  exit "$rc_u"
fi
echo "erasure-check: T and U passed"
