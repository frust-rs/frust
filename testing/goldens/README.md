# Golden Image Corpus

This directory holds Frust's promoted golden-image baselines: one PNG plus a
JSON provenance sidecar per case, grouped by golden class
(`docs/TESTING.md`'s Golden Classes — `cpu/`, `vulkan-nvidia-t400/`,
`android-emulator-api36-host/`, `android-emulator-api36-swiftshader/`, ...).

```
testing/goldens/<class>/<name>.png
testing/goldens/<class>/<name>.json
```

`<name>.json` is the [`GoldenMeta`] record (`crates/frust-testing/src/
meta.rs`): backend/adapter/driver identity, OS, Frust commit, case name,
tolerance, and capture timestamp — the provenance a later mismatch needs to
judge "real regression" vs. "environment drift".

## Storage Policy: plain git, no LFS

This corpus is tracked with plain git — no Git LFS. That is only sound under
an explicit size budget, enforced by `crates/frust-testing/tests/
corpus_budget.rs`:

- **8 MB total** across every file under this directory.
- **64 KB per PNG.**

For scale, upstream vello's comparable `vello_sparse_tests` reference corpus
is ~512 PNGs at ~1.43 MB total — well inside these caps. A case whose
baseline grows past either budget belongs re-encoded (smaller viewport,
fewer distinct baselines, tighter compression) rather than exempted from the
guard; do not raise these constants to make a single oversized case pass.

## Writing a Baseline

`UPDATE_GOLDENS=1` is the only path that writes to this directory
(`crates/frust-testing/src/golden.rs`'s `compare_golden`,
`docs/TESTING.md`'s Baseline Updates). A normal test run never writes here —
on a mismatch it writes expected/actual/triptych/diff-report artifacts under
`target/frust-testing/<class>/<name>/` instead, which is not tracked.

A baseline update is reviewed with its expected/actual/diff artifacts and a
stated reason, same as any other change here — never promoted from a machine
that failed the adapter/font/environment preflight (`docs/TESTING.md`'s
GPU Run Metadata / Baseline Updates sections).
