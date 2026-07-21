# Benchmark Results

Results record for the paired Frust-vs-Flutter benchmark suite, per
`PROTOCOL.md`. **A device or scenario with no completed runs is omitted, not
filled with placeholder numbers.**

Every table below must be reproducible from `benchmarks/raw/<device>/<scenario>/`
(raw per-run series, committed alongside this file — sanitized by
`harness/run.sh` to perf/marker lines only) via
`./benchmarks/harness/run.sh <scenario> --device <serial>`.

> **Results reset 2026-07-21.** All prior series (OnePlus 9 2026-07-20/21,
> iPhone SE 2026-07-20/21, and the phase-10 attribution re-run) were retired
> in one sweep — see git history for the retired tables. Reasons: (a) the
> **S5 layout-parity v2** fix (frust previously composited a ~66%-width cell
> vs Flutter's actual edge-to-edge render — PROTOCOL §8 S5 notes), (b) the
> build now carries the full phase-10 optimization set (shape cache,
> resampling, warmup, micro-wins) making earlier frust series stale, and
> (c) the device matrix changes to: **Xiaomi 12 (cupid, Snapdragon 8 Gen 1)**,
> **iPhone SE 2 (A13)**, and **OnePlus 9 (Adreno 660)** re-run later.
> Raw logs are now committed (sanitized); the pre-reset raws were never
> committed (old gitignore) and were deleted locally.
