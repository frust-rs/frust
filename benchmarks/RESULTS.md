# Benchmark Results

Template for the paired Frust-vs-Flutter benchmark suite's results, per
`PROTOCOL.md`. This file starts empty (no runs yet) — task `07-run-matrix`
fills in device sections as scenarios are actually run. **A device or
scenario with no completed runs is omitted, not filled with placeholder
numbers.**

Every table below must be reproducible from `benchmarks/raw/<device>/<scenario>/`
(raw per-run series, committed alongside this file) via
`./benchmarks/harness/run.sh <scenario> --device <serial>`.

## Device: OnePlus 9 (LE2115, Adreno 660) — mid-tier Android

**Status:** not yet run.

- Chipset: Snapdragon 888 / Adreno 660
- OS version: _(fill in)_
- Fixed brightness: _(fill in)_
- Refresh-rate mode used: _(fill in — 60Hz or 90/120Hz if supported)_
- Flutter version: 3.44.2 stable
- Frust build: _(commit / release profile confirmation)_
- Thermal-cooldown method: _(fill in — sensor reading or fixed wait)_

### S1 — Animation storm

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | | | | | | |
| Flutter (profile) | | | | | | |
| Flutter (release, in-app cross-check) | | | | | | |

### S2 — Long-list scroll (10k rows)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | | | | | | |
| Flutter (profile) | | | | | | |

### S3 — Table ops (js-framework-benchmark subset)

| Op | Frust (ms) | Flutter (ms) |
|---|---|---|
| create 1k | | |
| update every 10th of 10k | | |
| swap | | |
| clear | | |

### S4 — Heavy-work responsiveness (~50MB JSON parse + concurrent animation)

| Metric | Frust | Flutter |
|---|---|---|
| Total wall time (parse complete) | | |
| Animation p50 during parse (ms) | | |
| Animation p95 during parse (ms) | | |
| Animation p99 during parse (ms) | | |
| Animation missed-budget count during parse | | |

### S5 — Image pipeline (decode-and-display while scrolling)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | | | | | | |
| Flutter (profile) | | | | | | |

### S6 — Text shaping stress (multilingual relayout on width animation)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | | | | | | |
| Flutter (profile) | | | | | | |

### S7 — Cold start + idle

| Metric | Frust | Flutter |
|---|---|---|
| External cold start (`am start -W` TotalTime) | | |
| Framework-reported first-frame span | | |
| Idle CPU, 60s (avg %) | | |
| Idle memory, 60s (PSS, avg MB) | | |

### S8 — Plugin-call overhead (shared preferences)

| Op | Frust (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | | | n/a |
| write i64 | | | n/a |
| write f64 | | | n/a |
| write String | | | n/a |
| write Vec\<String\> | | | n/a |
| read (unique key, forces channel) bool | | | |
| read (unique key, forces channel) i64 | | | |
| read (unique key, forces channel) f64 | | | |
| read (unique key, forces channel) String | | | |
| read (unique key, forces channel) Vec\<String\> | | | |

| Burst-during-animation variant | Frust | Flutter |
|---|---|---|
| Animation p95 during S8 burst (ms) | | |
| Animation missed-budget count during S8 burst | | |

### Methodology deviations (this device)

_(List any control from `PROTOCOL.md` that could not be applied exactly —
e.g. no thermal sensor reading available, no >60Hz mode on this panel, a
scenario that could not complete the full 10-run set. Leave "none recorded"
if every run matched the protocol exactly.)_

---

## Device: iPhone SE — iOS

**Status:** not yet run.

- OS version: _(fill in)_
- Fixed brightness: _(fill in)_
- Refresh-rate mode: 60Hz (iPhone SE has no ProMotion panel — the 8.33ms
  budget column is N/A for every scenario on this device)
- Flutter version: 3.44.2 stable
- Frust build: _(commit / release profile confirmation)_
- Thermal-cooldown method: _(fill in)_

### S1 — Animation storm

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | | | | | N/A (60Hz panel) |
| Flutter (profile) | | | | | N/A (60Hz panel) |

### S2 — Long-list scroll (10k rows)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms |
|---|---|---|---|---|---|---|
| Frust (release) | | | | | N/A (60Hz panel) |
| Flutter (profile) | | | | | N/A (60Hz panel) |

### S3 — Table ops (js-framework-benchmark subset)

| Op | Frust (ms) | Flutter (ms) |
|---|---|---|
| create 1k | | |
| update every 10th of 10k | | |
| swap | | |
| clear | | |

### S4 — Heavy-work responsiveness (~50MB JSON parse + concurrent animation)

| Metric | Frust | Flutter |
|---|---|---|
| Total wall time (parse complete) | | |
| Animation p50 during parse (ms) | | |
| Animation p95 during parse (ms) | | |
| Animation p99 during parse (ms) | | |
| Animation missed-budget count during parse | | |

### S5 — Image pipeline (decode-and-display while scrolling)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms |
|---|---|---|---|---|---|
| Frust (release) | | | | | |
| Flutter (profile) | | | | | |

### S6 — Text shaping stress (multilingual relayout on width animation)

| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms |
|---|---|---|---|---|---|
| Frust (release) | | | | | |
| Flutter (profile) | | | | | |

### S7 — Cold start + idle

| Metric | Frust | Flutter |
|---|---|---|
| External cold start (`xcrun`/Instruments launch time) | | |
| Framework-reported first-frame span | | |
| Idle CPU, 60s (avg %) | | |
| Idle memory, 60s (RSS, avg MB) | | |

### S8 — Plugin-call overhead (shared preferences)

| Op | Frust (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |
|---|---|---|---|
| write bool | | | n/a |
| write i64 | | | n/a |
| write f64 | | | n/a |
| write String | | | n/a |
| write Vec\<String\> | | | n/a |
| read (unique key, forces channel) bool | | | |
| read (unique key, forces channel) i64 | | | |
| read (unique key, forces channel) f64 | | | |
| read (unique key, forces channel) String | | | |
| read (unique key, forces channel) Vec\<String\> | | | |

| Burst-during-animation variant | Frust | Flutter |
|---|---|---|
| Animation p95 during S8 burst (ms) | | |
| Animation missed-budget count during S8 burst | | |

### Methodology deviations (this device)

_(As above.)_

---

## Device: OnePlus 15 (Adreno 840) — headline Android

**Status:** pending device availability — not yet run. This column is the
plan's headline device (proven S1 Impeller-vs-vello win on Adreno 840,
now to be formalized across the full matrix); added here whenever the
device is next at desk. Copy the OnePlus 9 section's table shapes above
when filling this in.

---

## Cross-device narrative

_(Filled in once at least two devices have a full matrix — the honest
summary: which scenarios show a Frust advantage, which don't, on which
device tier, including any scenario where Flutter measures better. Not
written until real data exists.)_
