#!/usr/bin/env python3
"""benchmarks/harness/stats.py — the ONE shared statistics script.

Parses Frust's `frust-perf raw ...` lines (see
`crates/frust-shell-common/src/perf.rs`'s `format_raw_frame_line`) AND
Flutter bench's `flutter-perf raw ...` lines (see
`benchmarks/flutter_bench`'s raw-capture callback) into one common
per-frame series, slices that series by `bench-scenario-start/end <name>`
marker lines (identical strings on both sides), discards a caller-chosen
number of leading *runs* (protocol convention: first 2 discarded as
warm-up), and computes identical percentile/missed-budget statistics
from either side's raw series — so a Frust vs Flutter comparison is never
computed by two different code paths.

No third-party dependencies (stdlib only) — this runs on a bench rig with
no guaranteed `pip` access. Runnable in two ways:

    python3 benchmarks/harness/stats.py --self-test
    python3 benchmarks/harness/stats.py [--scenario NAME] [--discard-first N] \\
        [--label LABEL] <logfile> [<logfile> ...]

`run.sh` shells out to this script after collecting each run's raw log.

# Field-format contract

- Frust:   `frust-perf raw n=<n> total_us=<> rebuild_us=<> layout_us=<> \\
            paint_us=<> encode_us=<> acquire_us=<> submit_us=<> skipped=<0|1> \\
            gpu_q=<0|1> [gpu_total_us=<> gpu_prepass_us=<> gpu_main_us=<> \\
            gpu_composite_us=<> gpu_blit_us=<>]`
  (raw format v4, 2026-09-01: real GPU time per pass, appended after
  `skipped`. `gpu_q` says whether the frame carries a GPU reading at all;
  the five `gpu_*_us` fields are present ONLY when it is `1` — a frame with
  no GPU timing writes no zeros, so a series without it never grows a column
  of zeros that reads like a measurement. Every v3 field keeps its name,
  meaning and position, so a v3 log and a v4 log are directly comparable and
  every v3 series under `benchmarks/raw` still parses here unchanged.
  Raw format v3, 2026-07-22: the single `present_us` field was
  split into separate `acquire_us` (blocking swapchain-acquire/vsync wait) +
  `submit_us` (blit + queue-submit + present); `acquire_us + submit_us`
  equals the old v2 `present_us`. 2026-07-21 (v2) had earlier
  split the v1 `encode_present_us` into `encode_us` + `present_us`. Parsing
  is key=value and forward-compatible, so only this contract note changed.)
- Flutter: `flutter-perf raw n=<n> build_us=<> raster_us=<> total_us=<>`
  (no `skipped` field — Flutter's `addTimingsCallback` only ever reports
  frames it actually rendered, so every parsed Flutter frame is treated as
  non-skipped; see `benchmarks/flutter_bench`'s raw-capture callback for
  the emitting side.)

Only `n`/`total_us`/`skipped` are used for the cross-app percentile/budget
table below (the field the two apps' pass breakdowns don't share a
vocabulary for); every parsed key=value pair is still kept per-frame
(`FrameRecord.fields`) for a caller that wants the framework-specific
pass breakdown (e.g. Frust's `rebuild_us`). The v4 GPU fields ride that
same generic dict, with `gpu_spans` reading them back out in a shape that
tells "measured zero" apart from "not measured".

# Graphics memory (`extract_graphics_kb`)

`run.sh` snapshots `dumpsys meminfo <pkg>` before and after every run
(`run-NN.pss_before.txt` / `run-NN.pss_after.txt`). `extract_graphics_kb`
reads the App Summary block's `Graphics:` PSS row out of one of those
snapshots and `graphics_kb_series`/`mean_graphics_mb` average it across a
run set — the arithmetic behind RESULTS.md's `Graphics avg (MB)` column,
which was previously done by hand outside this script.

- `bench-scenario-start <name>` / `bench-scenario-end <name>` — Flutter's
  shape, unchanged, and every Frust series captured before 2026-09-06.
  `bench-scenario-start n=<frame> <name>` / `bench-scenario-end n=<frame>
  <name>` — Frust's shape from 2026-09-06 on (see
  `perf::mark_scenario_start`/`mark_scenario_end`), carrying the 1-indexed
  number of the frame that **carried** the marker through the pipeline —
  the same counter a `frust-perf raw` line's own `n=` uses, stamped by the
  same emitter (`FrameStats::record`) immediately ahead of that frame's raw
  line. Both shapes are `bench-scenario-*` strings either side recognizes
  identically at the prefix; only the tail differs, and `parse_marker_line`
  reports the frame index as `None` when a line carries no `n=` token.

# Marker attribution: index-based vs. position-based (PROTOCOL §7)

A window is **half-open**: `[start_n, end_n)`. `start n=k` names the
window's first frame — the build that raised it also applied the operation
being measured — and `end` is raised in the *next* build (the S3
convention), so `end n=k+1` names the first frame after the window. For the
S3 shape that makes the window exactly frame k, one frame, which is the
point.

When *both* of a window's markers carry a frame index, `slice_scenario`
attributes every raw frame with `start_n <= n < end_n` to that window by
frame identity alone — immune to which physical line comes first in the
log. That immunity is what the indexed shape exists for: on the
render-thread split (`crates/frust-shell-common/src/render_split.rs`) the
markers are emitted by the render thread that records the frame, while the
rest of the log (per-op `frust-perf plugin` lines, app logging) is still
written by the UI thread running ahead of it, so a marker's *line position*
in a captured log is not a reliable proxy for which frames it brackets.
Before that emitter change the marker was logged by the UI thread outright
and position-based slicing gave `s3-create1k` zero frames and shifted every
other S3 op by one (action item `act_000001a070c818837NtGqevW`).

When either of a window's markers carries no index (Flutter's markers,
always; a Frust series captured before this change), `slice_scenario` falls
back to the original **position**-based bracketing: every raw-frame line
between that start marker's line and its matching end marker's line, by log
order — exactly what it always computed, so an old capture reproduces the
identical series it always did (see `test_every_committed_v3_series_still_
parses`-adjacent reproduction: `summarize.py` over `benchmarks/raw/oneplus9`
still matches `RESULTS.md`'s published tables to the digit).

A degenerate indexed window is possible and is not an error: if the render
channel's depth-1 latest-wins slot dropped the operation's own build, both
edges land on the single frame that superseded it, `[k, k)` is empty, and
the operation contributes no frames — the honest reading of "the frame that
op mutated was never drawn".

# Repeated same-name marker pairs (S3 continuous cycling)

A marker name that opens and closes more than once in the same log (S3's
continuous-cycling redesign: `s3-create1k`/`s3-update`/etc. repeat once per
cycle, see `benchmarks/PROTOCOL.md`'s S3 row) produces one window per
occurrence; `slice_scenario` accumulates every occurrence's frames into one
combined series, the same as if the caller had run each op once for a very
long single window — whether each occurrence's window is attributed by
index or by position. Verified by `test_stats.py`'s
`test_repeated_marker_pairs_aggregate_across_cycles` (name-only) and
`test_repeated_indexed_windows_accumulate_across_cycles` (indexed).

# Per-op lines (d1/d2, and S8 backward-compat) — a second parse path

`benchmarks/PROTOCOL.md` §7's "Per-op line format" section formalizes a
second raw-line family, distinct from the per-frame family above: one line
per plugin-boundary/DB call, not per frame. This module's per-op path
(`parse_op_line`/`slice_op_scenario`/`compute_op_stats`/`format_op_table`,
driven by `--dclass` on the CLI) recognizes **both** shapes that line
family can take, so a Frust vs Flutter d-class comparison and S8's own
per-op numbers can both be computed by this one script instead of ad hoc
external math (see PROTOCOL §7's "Methodology-deviations note" — S8's
published per-op numbers predate this and were computed outside the shared
script):

- **Canonical** (PROTOCOL §7, `d1`/`d2` and any future per-op scenario):
  `<app>-perf op scenario=<id> op=<name> n=<u64> us=<u64> err=<0|1> [...]`
  — self-identifying via its inline `scenario=` field.
- **Grandfathered** (S8's shipped shape, unchanged by PROTOCOL §7's
  formalization): `<app>-perf plugin op=<name> type=<tag> n=<i> us=<us>
  err=<0|1>` — no inline `scenario=` field; attributed to a scenario only
  by the bracketing `bench-scenario-start/end` marker pair (S8's own
  `s8-write`/`s8-read` phase markers).

This is entirely additive: `parse_raw_line`/`slice_scenario`/
`compute_stats`/`format_table` (the frame-series path above) are untouched
by this module — a caller that never passes `--dclass` (i.e. every
existing s-class invocation) runs exactly the code it always has. See
`test_stats.py`'s per-op test classes.

# d-class warmup discard (PROTOCOL §9.3/§9.4)

`exclude_dclass_warmup` drops the first (`n=0`) per-run sample of each
warmup-affected op (`DCLASS_WARMUP_EXCLUDE_OPS`) — declared *in addition
to* the standard first-2-runs discard (`discard_first_runs`, reused
unchanged for op records). Because `n` is a 0-indexed counter reset to 0
at the start of each run (PROTOCOL §7), dropping every `n=0` record for a
warmup-affected op removes exactly one sample per remaining run for that
op, with no need to track run boundaries once the per-run lists have
already been concatenated.
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import TypeVar

# The 60Hz/120Hz frame budgets, in whole microseconds — matches
# `frust-shell-common::perf::BUDGET_60HZ`/`BUDGET_120HZ` exactly (both
# truncated `Duration::from_micros` constants), so a "missed budget" count
# means the same thing on both sides of a comparison.
BUDGET_60HZ_US = 16_667
BUDGET_120HZ_US = 8_333

FRUST_RAW_PREFIX = "frust-perf raw"
FLUTTER_RAW_PREFIX = "flutter-perf raw"
MARKER_START_PREFIX = "bench-scenario-start"
MARKER_END_PREFIX = "bench-scenario-end"

# Per-op line prefixes (PROTOCOL §7) — see module docs' "Per-op lines"
# section. Canonical shape uses the `op` token; S8's grandfathered shape
# uses the `plugin` token.
FRUST_OP_PREFIX = "frust-perf op"
FLUTTER_OP_PREFIX = "flutter-perf op"
FRUST_PLUGIN_PREFIX = "frust-perf plugin"
FLUTTER_PLUGIN_PREFIX = "flutter-perf plugin"

# d1/d2's declared warmup-discard convention (PROTOCOL §9.3/§9.4): the
# first (n=0) per-run sample of these ops is excluded from percentile math,
# in addition to the standard first-2-runs discard. `range_scan` (d2) is
# deliberately absent — it is the scenario's only per-run sample of that
# op, so warmup-excluding it would leave zero (see §9.4's small-N caveat).
DCLASS_WARMUP_EXCLUDE_OPS: dict[str, frozenset[str]] = {
    "d1": frozenset({"insert_batch", "insert_single"}),
    "d2": frozenset({"select_point"}),
}

# Runs discarded by default before computing statistics (protocol
# convention — declared as OUR convention, not a framework fact; see
# `benchmarks/PROTOCOL.md`).
DEFAULT_DISCARD_FIRST = 2

# Raw-format v4's per-pass GPU-time fields, in the order the frame line
# writes them (PROTOCOL §7). `gpu_q` is the marker that gates the rest: it is
# always present, and the five below appear only when it is "1".
GPU_QUERY_MARKER = "gpu_q"
GPU_SPAN_FIELDS = (
    "gpu_total_us",
    "gpu_prepass_us",
    "gpu_main_us",
    "gpu_composite_us",
    "gpu_blit_us",
)

# The `dumpsys meminfo` App Summary row `extract_graphics_kb` reads, and the
# column it takes (`Pss(KB)`, the first number on the row — the second is
# `Rss(KB)`). PSS is what RESULTS.md's Graphics figures have always quoted.
GRAPHICS_SUMMARY_ROW = "Graphics:"


@dataclass
class FrameRecord:
    """One parsed raw-per-frame line, source-tagged."""

    source: str  # "frust" | "flutter"
    n: int
    total_us: int
    skipped: bool
    fields: dict[str, str] = field(default_factory=dict)


@dataclass
class ScenarioStats:
    """The identical-shape percentile/missed-budget table computed from
    either app's raw series — see module docs."""

    n_total: int
    n_active: int
    skipped: int
    p50_us: int
    p95_us: int
    p99_us: int
    worst_us: int
    missed_60hz: int
    missed_120hz: int

    def as_dict(self) -> dict[str, int]:
        return {
            "n_total": self.n_total,
            "n_active": self.n_active,
            "skipped": self.skipped,
            "p50_us": self.p50_us,
            "p95_us": self.p95_us,
            "p99_us": self.p99_us,
            "worst_us": self.worst_us,
            "missed_60hz": self.missed_60hz,
            "missed_120hz": self.missed_120hz,
        }


@dataclass
class OpRecord:
    """One parsed per-op line (PROTOCOL §7), source-tagged. `scenario` is
    `None` for S8's grandfathered shape (no inline `scenario=` field) —
    see `slice_op_scenario` for how that case is attributed to a scenario
    anyway."""

    source: str  # "frust" | "flutter"
    scenario: str | None
    op: str
    n: int
    us: int
    err: bool
    fields: dict[str, str] = field(default_factory=dict)


@dataclass
class OpStats:
    """Per-op latency/throughput table (PROTOCOL §9.6) — one instance per
    `op` name within a d-class scenario (or S8 phase)."""

    op: str
    n_total: int
    n_ok: int
    errors: int
    p50_us: int
    p95_us: int
    p99_us: int
    worst_us: int
    ops_per_sec: float

    def as_dict(self) -> dict[str, int | float | str]:
        return {
            "op": self.op,
            "n_total": self.n_total,
            "n_ok": self.n_ok,
            "errors": self.errors,
            "p50_us": self.p50_us,
            "p95_us": self.p95_us,
            "p99_us": self.p99_us,
            "worst_us": self.worst_us,
            "ops_per_sec": self.ops_per_sec,
        }


# ---------------------------------------------------------------------
# Parsing
# ---------------------------------------------------------------------


def _parse_kv_tail(rest: str) -> dict[str, str]:
    """Splits a whitespace-separated `key=value` tail into a dict, silently
    skipping any token with no `=` (forward-compatible with a future field
    this script doesn't know about yet)."""
    out: dict[str, str] = {}
    for tok in rest.split():
        if "=" not in tok:
            continue
        key, _, value = tok.partition("=")
        out[key] = value
    return out


def parse_raw_line(line: str) -> FrameRecord | None:
    """Parses one `frust-perf raw ...` or `flutter-perf raw ...` line into a
    [`FrameRecord`], or `None` if `line` isn't a raw-frame line or is missing
    its required `n`/`total_us` fields.

    The prefix is located anywhere in the line (not just at its start) —
    `adb logcat -v raw` (this harness's own capture mode) emits the bare
    message with no metadata prefix, but a log pasted/captured through a
    more verbose `logcat` format (timestamp, pid, tag) still parses
    correctly, and a fixture/manual-capture log can be pasted as-is."""
    stripped = line.strip()
    frust_idx = stripped.find(FRUST_RAW_PREFIX)
    flutter_idx = stripped.find(FLUTTER_RAW_PREFIX)
    if frust_idx != -1:
        source = "frust"
        rest = stripped[frust_idx + len(FRUST_RAW_PREFIX) :]
    elif flutter_idx != -1:
        source = "flutter"
        rest = stripped[flutter_idx + len(FLUTTER_RAW_PREFIX) :]
    else:
        return None

    fields = _parse_kv_tail(rest)
    try:
        n = int(fields["n"])
        total_us = int(fields["total_us"])
    except (KeyError, ValueError):
        return None

    # Frust's raw line always carries `skipped=0|1`; Flutter's carries no
    # such field (see module docs) — absence means "not skipped".
    skipped_raw = fields.get("skipped")
    skipped = skipped_raw == "1" if skipped_raw is not None else False

    return FrameRecord(source=source, n=n, total_us=total_us, skipped=skipped, fields=fields)


def gpu_spans(record: FrameRecord) -> dict[str, int] | None:
    """The v4 per-pass GPU times carried by `record`, or `None` when the
    frame carries no GPU reading at all.

    `None` (the `gpu_q=0`/`gpu_q` absent case) and an all-zero dict are
    deliberately different answers: the first means the renderer measured
    nothing — no engine tier, no `TIMESTAMP_QUERY` device, or the first
    frames of a surface before the ring's first readback landed — while the
    second means it measured a frame that really did no GPU work in those
    passes. Collapsing the two is exactly how a zero column starts reading
    like a result.

    A malformed field (non-integer, or a `gpu_q=1` line missing one of the
    five) drops that key rather than failing the whole frame — the same
    skip-what-you-don't-understand posture `parse_raw_line` takes.
    """
    if record.fields.get(GPU_QUERY_MARKER) != "1":
        return None
    spans: dict[str, int] = {}
    for key in GPU_SPAN_FIELDS:
        raw = record.fields.get(key)
        if raw is None:
            continue
        try:
            spans[key] = int(raw)
        except ValueError:
            continue
    return spans


def parse_marker_line(line: str) -> tuple[str, str, int | None] | None:
    """Parses a `bench-scenario-start/end <name>` (old, name-only shape) or
    `bench-scenario-start/end n=<frame> <name>` (2026-09-06 indexed shape)
    line into `(edge, name, frame)` (`edge` is `"start"`/`"end"`; `frame` is
    the 1-indexed number of the frame that carried the marker, or `None`
    when the line carries no `n=` token — Flutter's markers, always, and
    every Frust series captured before the indexed shape shipped). Returns
    `None` if `line` isn't a marker line, carries no name, or carries a
    malformed `n=` token. Like [`parse_raw_line`], the prefix is located
    anywhere in the line, not just at its start (see that function's
    docs)."""
    stripped = line.strip()
    start_idx = stripped.find(MARKER_START_PREFIX)
    end_idx = stripped.find(MARKER_END_PREFIX)
    if start_idx != -1:
        edge, prefix, idx = "start", MARKER_START_PREFIX, start_idx
    elif end_idx != -1:
        edge, prefix, idx = "end", MARKER_END_PREFIX, end_idx
    else:
        return None
    rest = stripped[idx + len(prefix) :].strip()
    if not rest:
        return None
    frame: int | None = None
    if rest.startswith("n="):
        index_tok, _, name = rest.partition(" ")
        try:
            frame = int(index_tok[len("n=") :])
        except ValueError:
            return None
        name = name.strip()
    else:
        name = rest
    if not name:
        return None
    return edge, name, frame


def slice_scenario(lines: list[str], scenario: str | None) -> list[FrameRecord]:
    """Extracts the [`FrameRecord`]s bracketed by `bench-scenario-start
    <scenario>` / `bench-scenario-end <scenario>` marker lines (either the
    old name-only shape or the indexed `n=<frame>` shape — see module docs'
    "Marker attribution" section). When `scenario` is `None`, every
    parseable raw-frame line in `lines` is included regardless of markers
    (useful for a single-scenario fixture with no marker bracket, or a
    caller that already sliced upstream).

    Two independent attribution strategies, chosen per window (one
    start/end occurrence of `scenario`'s markers), not globally for the
    whole log — repeated occurrences of the same scenario name (S3's
    continuous cycling) can each be sliced by whichever strategy that
    occurrence's own marker pair supports:

    - **Indexed** (both the window's start and end marker carry a frame
      index): every raw frame anywhere in `lines` whose `n` falls in the
      half-open `[start_n, end_n)` — start inclusive, end exclusive — is
      included, regardless of that raw line's position relative to the
      marker lines. Half-open because the closing marker is raised in the
      build *after* the window (the S3 convention), so its frame number is
      one past the window's last frame; an empty window is therefore
      possible and meaningful (module docs).
    - **Positional** (either marker carries no index — Flutter's markers,
      always, or a pre-index Frust capture): every raw-frame line between
      that start marker's line and its matching end marker's line, by log
      order, exactly as this function always computed it.

    An unterminated window (a `scenario` start with no matching end before
    EOF) is always positional, extending to the end of `lines` — the same
    trailing-inclusion behavior this function has always had.
    """
    if scenario is None:
        frames: list[FrameRecord] = []
        for line in lines:
            if parse_marker_line(line) is not None:
                continue
            rec = parse_raw_line(line)
            if rec is not None:
                frames.append(rec)
        return frames

    # One pass: collect every raw frame (with its original line position, so
    # a positional window can still be bracketed by position) and every
    # window (start/end marker pair) matching `scenario`.
    all_frames: list[FrameRecord] = []
    frame_positions: list[int] = []
    windows: list[tuple[int, int | None, int, int | None]] = []
    open_start: tuple[int, int | None] | None = None

    for pos, line in enumerate(lines):
        marker = parse_marker_line(line)
        if marker is not None:
            edge, name, frame_n = marker
            if name == scenario:
                if edge == "start":
                    open_start = (pos, frame_n)
                elif open_start is not None:
                    start_pos, start_n = open_start
                    windows.append((start_pos, start_n, pos, frame_n))
                    open_start = None
            continue
        rec = parse_raw_line(line)
        if rec is not None:
            frame_positions.append(pos)
            all_frames.append(rec)

    if open_start is not None:
        # No matching end before EOF: positional, extending to the end of
        # `lines` (there is no end marker to carry an index for this one).
        windows.append((open_start[0], open_start[1], len(lines), None))

    frames: list[FrameRecord] = []
    for start_pos, start_n, end_pos, end_n in windows:
        if start_n is not None and end_n is not None:
            frames.extend(rec for rec in all_frames if start_n <= rec.n < end_n)
        else:
            frames.extend(
                rec
                for rec, line_pos in zip(all_frames, frame_positions)
                if start_pos < line_pos < end_pos
            )
    return frames


def load_run_frames(path: Path, scenario: str | None) -> list[FrameRecord]:
    """Reads one run's log file and slices its scenario frames."""
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    return slice_scenario(lines, scenario)


_T = TypeVar("_T")


def discard_first_runs(runs: list[list[_T]], discard_first: int) -> list[_T]:
    """Drops the first `discard_first` *runs* (not frames), then
    concatenates the remaining runs' frames into one series. If there
    aren't more runs than `discard_first`, nothing is discarded (a single
    fixture/smoke-test log is still usable directly) rather than erroring.

    Generic over the per-run record type — used unchanged for both
    [`FrameRecord`] runs (the s-class path) and [`OpRecord`] runs (the
    d-class path, see `exclude_dclass_warmup`); the logic itself has no
    per-frame semantics, so no s-class behavior changes here."""
    if len(runs) > discard_first:
        kept = runs[discard_first:]
    else:
        kept = runs
    combined: list[_T] = []
    for run in kept:
        combined.extend(run)
    return combined


# ---------------------------------------------------------------------
# Graphics memory, from run.sh's dumpsys snapshots — see module docs
# ---------------------------------------------------------------------


def extract_graphics_kb(text: str) -> int | None:
    """The App Summary `Graphics:` PSS figure (KB) in one `dumpsys meminfo`
    snapshot, or `None` when the snapshot has no such row.

    `run.sh` captures these best-effort (`|| echo "note: could not capture
    ..."`), so a missing or truncated snapshot is an ordinary outcome, not an
    error — a caller averages over whatever it got and says how many samples
    that was.

    The row's shape is `Graphics:   <Pss(KB)>   [<Rss(KB)>]`; the first
    number is taken, which is the PSS column every published Graphics figure
    has quoted. The `Graphics:` label also appears nowhere else in a meminfo
    dump, so no section tracking is needed to find it."""
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith(GRAPHICS_SUMMARY_ROW):
            continue
        for token in stripped[len(GRAPHICS_SUMMARY_ROW) :].split():
            try:
                return int(token)
            except ValueError:
                return None
    return None


def graphics_kb_series(paths: list[Path]) -> list[int]:
    """The `Graphics:` PSS figure from each readable snapshot in `paths`, in
    the given order — snapshots that are missing, unreadable, or carry no
    such row are skipped rather than counted as zero."""
    series: list[int] = []
    for path in paths:
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        value = extract_graphics_kb(text)
        if value is not None:
            series.append(value)
    return series


def mean_graphics_mb(paths: list[Path]) -> tuple[float, int]:
    """`(mean MB, sample count)` over `paths`' Graphics PSS figures —
    RESULTS.md's `Graphics avg (MB)` column, computed here rather than by
    hand. `(0.0, 0)` when nothing parsed.

    MB is KB/1024 (binary), matching what every published Graphics figure
    already divided by, and the count is returned alongside so a cell backed
    by two snapshots is never presented as one backed by ten.

    Which snapshots to hand it is the caller's decision, and RESULTS.md's own
    convention is the *kept* runs' `pss_after` files — the post-run capture
    only, since `run.sh` force-stops the app immediately before each run and
    the pre-run snapshot is therefore empty. Passing a pre-run snapshot is
    harmless (it carries no `Graphics:` row and is skipped), but it is not
    what the published figures average."""
    series = graphics_kb_series(paths)
    if not series:
        return 0.0, 0
    return sum(series) / len(series) / 1024.0, len(series)


# ---------------------------------------------------------------------
# Per-op parsing (d-class + S8 backward-compat) — see module docs
# ---------------------------------------------------------------------


def parse_op_line(line: str) -> OpRecord | None:
    """Parses one per-op raw line into an [`OpRecord`], or `None` if `line`
    isn't a per-op line or is missing a required field. Recognizes both the
    canonical `<app>-perf op scenario=<id> op=<name> n=<u64> us=<u64>
    err=<0|1> [...]` shape (PROTOCOL §7, `d1`/`d2` and any future per-op
    scenario) and S8's grandfathered `<app>-perf plugin op=<name>
    type=<tag> n=<i> us=<us> err=<0|1>` shape (no inline `scenario=` field
    — see `slice_op_scenario` for how that case is attributed to a
    scenario).

    Requiring `op`/`n`/`us`/`err` all be present (like [`parse_raw_line`]'s
    `n`/`total_us` requirement) means S8's own `op=<name> type=total
    n=<writes> us=<total> errors=<n>` aggregate line (`errors`, plural, not
    `err`) and its `s8-errors` tally marker (no `op=`/`n=`/`us=` at all)
    both parse as `None` here, exactly like any other non-per-op line —
    same forward-compatible/skip-unknown posture as the frame parser."""
    stripped = line.strip()

    frust_op_idx = stripped.find(FRUST_OP_PREFIX)
    flutter_op_idx = stripped.find(FLUTTER_OP_PREFIX)
    frust_plugin_idx = stripped.find(FRUST_PLUGIN_PREFIX)
    flutter_plugin_idx = stripped.find(FLUTTER_PLUGIN_PREFIX)

    if frust_op_idx != -1:
        source, rest = "frust", stripped[frust_op_idx + len(FRUST_OP_PREFIX) :]
    elif flutter_op_idx != -1:
        source, rest = "flutter", stripped[flutter_op_idx + len(FLUTTER_OP_PREFIX) :]
    elif frust_plugin_idx != -1:
        source, rest = "frust", stripped[frust_plugin_idx + len(FRUST_PLUGIN_PREFIX) :]
    elif flutter_plugin_idx != -1:
        source, rest = "flutter", stripped[flutter_plugin_idx + len(FLUTTER_PLUGIN_PREFIX) :]
    else:
        return None

    fields = _parse_kv_tail(rest)
    try:
        op = fields["op"]
        n = int(fields["n"])
        us = int(fields["us"])
        err_raw = fields["err"]
    except (KeyError, ValueError):
        return None
    if err_raw not in ("0", "1"):
        return None

    # Canonical shape only — grandfathered S8 lines carry no inline
    # `scenario=` field (see docstring).
    scenario = fields.get("scenario")

    return OpRecord(
        source=source,
        scenario=scenario,
        op=op,
        n=n,
        us=us,
        err=err_raw == "1",
        fields=fields,
    )


def slice_op_scenario(lines: list[str], scenario: str) -> list[OpRecord]:
    """Extracts the [`OpRecord`]s belonging to `scenario` from one run's
    lines. A canonical-shape record (inline `scenario=` field) self-
    identifies regardless of bracket position. A grandfathered S8-shape
    record (no inline `scenario=` field) is instead attributed by the
    bracketing `bench-scenario-start/end` marker — active for a marker name
    equal to `scenario`, or beginning with `scenario + "-"` (a phase-marker
    convention shared by S8's `s8-write`/`s8-read` and d1/d2's
    `d1-insert-batch`/`d2-select-point`/etc. — though the canonical-shape
    records never actually need this branch, since their own inline
    `scenario=` field already identifies them)."""
    ops: list[OpRecord] = []
    active = False
    for line in lines:
        marker = parse_marker_line(line)
        if marker is not None:
            edge, name, _frame = marker
            if name == scenario or name.startswith(scenario + "-"):
                active = edge == "start"
            continue
        rec = parse_op_line(line)
        if rec is None:
            continue
        if rec.scenario is not None:
            if rec.scenario == scenario:
                ops.append(rec)
        elif active:
            ops.append(rec)
    return ops


def load_run_op_records(path: Path, scenario: str) -> list[OpRecord]:
    """Reads one run's log file and slices its per-op records for
    `scenario` — the d-class analog of `load_run_frames`."""
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    return slice_op_scenario(lines, scenario)


def exclude_dclass_warmup(records: list[OpRecord], scenario: str) -> list[OpRecord]:
    """Drops the first (`n=0`) per-run sample of each op in
    `DCLASS_WARMUP_EXCLUDE_OPS[scenario]` (PROTOCOL §9.3/§9.4's declared
    warmup-discard convention), applied *in addition to* the standard
    first-2-runs discard (`discard_first_runs`) — see module docs. A
    scenario with no declared warmup-excluded ops is returned unchanged."""
    warmup_ops = DCLASS_WARMUP_EXCLUDE_OPS.get(scenario, frozenset())
    if not warmup_ops:
        return records
    return [r for r in records if not (r.op in warmup_ops and r.n == 0)]


# ---------------------------------------------------------------------
# Statistics
# ---------------------------------------------------------------------


def _nearest_rank_percentile(sorted_values: list[int], p: int) -> int:
    """Nearest-rank percentile over an ascending-sorted sample — same
    formula as `frust-shell-common::perf::nearest_rank_percentile`
    (1-indexed rank `ceil(p * n / 100)`, clamped to `[1, n]`), so a Python
    and Rust computation over the same series never disagree."""
    n = len(sorted_values)
    if n == 0:
        return 0
    rank = -(-(p * n) // 100)  # ceil division, integers only
    rank = max(1, min(rank, n))
    return sorted_values[rank - 1]


def compute_stats(frames: list[FrameRecord]) -> ScenarioStats:
    """Computes the identical-format percentile/missed-budget table over
    `frames` — percentiles and the worst value are computed over only the
    *non-skipped* frames (a skipped frame's near-zero cost would otherwise
    pull percentiles down), mirroring `FrameStats::summary`'s semantics.
    An empty (or all-skipped) input reports all-zero fields."""
    skipped_count = sum(1 for f in frames if f.skipped)
    active_totals = sorted(f.total_us for f in frames if not f.skipped)

    return ScenarioStats(
        n_total=len(frames),
        n_active=len(active_totals),
        skipped=skipped_count,
        p50_us=_nearest_rank_percentile(active_totals, 50),
        p95_us=_nearest_rank_percentile(active_totals, 95),
        p99_us=_nearest_rank_percentile(active_totals, 99),
        worst_us=active_totals[-1] if active_totals else 0,
        missed_60hz=sum(1 for t in active_totals if t > BUDGET_60HZ_US),
        missed_120hz=sum(1 for t in active_totals if t > BUDGET_120HZ_US),
    )


def format_table(stats: ScenarioStats, label: str) -> str:
    """Formats one percentile/missed-budget table — the identical shape for
    either app, so a report can print a Frust table and a Flutter table
    back to back with nothing but the label differing."""
    def us_to_ms(us: int) -> str:
        return f"{us / 1000:.2f}ms"

    lines = [
        f"== {label} ==",
        f"frames: {stats.n_total} total, {stats.n_active} active, {stats.skipped} skipped",
        f"p50={us_to_ms(stats.p50_us)}  p95={us_to_ms(stats.p95_us)}  "
        f"p99={us_to_ms(stats.p99_us)}  worst={us_to_ms(stats.worst_us)}",
        f"missed_60hz={stats.missed_60hz} (budget 16.67ms)  "
        f"missed_120hz={stats.missed_120hz} (budget 8.33ms)",
    ]
    return "\n".join(lines)


def compute_op_stats(records: list[OpRecord]) -> dict[str, OpStats]:
    """Computes per-op p50/p95/p99/worst latency (µs) and ops/s (PROTOCOL
    §9.6) over `records`, grouped by `op` name (iteration order:
    alphabetical, for a stable table). An `err=1` sample is excluded from
    the latency percentiles and the ops/s throughput figure — mirroring
    `compute_stats`'s skipped-frame exclusion — but is still counted in
    that op's `n_total`/`errors` tally, so a run with boundary failures is
    never silently reported as clean. An op with zero non-err samples
    reports an all-zero latency/throughput row (same all-zero-on-empty
    posture as `compute_stats`)."""
    by_op: dict[str, list[OpRecord]] = {}
    for r in records:
        by_op.setdefault(r.op, []).append(r)

    result: dict[str, OpStats] = {}
    for op in sorted(by_op):
        recs = by_op[op]
        ok_us = sorted(r.us for r in recs if not r.err)
        errors = sum(1 for r in recs if r.err)
        total_us = sum(ok_us)
        ops_per_sec = (len(ok_us) / (total_us / 1_000_000)) if total_us > 0 else 0.0
        result[op] = OpStats(
            op=op,
            n_total=len(recs),
            n_ok=len(ok_us),
            errors=errors,
            p50_us=_nearest_rank_percentile(ok_us, 50),
            p95_us=_nearest_rank_percentile(ok_us, 95),
            p99_us=_nearest_rank_percentile(ok_us, 99),
            worst_us=ok_us[-1] if ok_us else 0,
            ops_per_sec=ops_per_sec,
        )
    return result


def format_op_table(op_stats: dict[str, OpStats], label: str) -> str:
    """Formats the d-class RESULTS table (PROTOCOL §9.6) — one row per op,
    in `compute_op_stats`'s alphabetical-by-op-name order, so the table is
    stable across runs with the identical op set."""
    lines = [f"== {label} (d-class) =="]
    if not op_stats:
        lines.append("(no per-op samples)")
        return "\n".join(lines)
    lines.append(
        f"{'op':<16}{'n':>8}{'errors':>8}{'p50_us':>10}{'p95_us':>10}"
        f"{'p99_us':>10}{'worst_us':>10}{'ops/s':>12}"
    )
    for op, s in op_stats.items():
        lines.append(
            f"{op:<16}{s.n_total:>8}{s.errors:>8}{s.p50_us:>10}{s.p95_us:>10}"
            f"{s.p99_us:>10}{s.worst_us:>10}{s.ops_per_sec:>12.1f}"
        )
    return "\n".join(lines)


# ---------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------


def _build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Shared percentile/missed-budget stats over Frust and Flutter raw frame logs.",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run this module's built-in fixture-backed assertions and exit "
        "(no pytest/unittest runner required, though `python3 -m unittest "
        "test_stats.py` also works from this directory)",
    )
    parser.add_argument(
        "--scenario",
        default=None,
        help="scenario name to slice each log by its bench-scenario-start/end "
        "markers (omit to treat each whole file as one already-sliced series)",
    )
    parser.add_argument(
        "--discard-first",
        type=int,
        default=DEFAULT_DISCARD_FIRST,
        help=f"number of leading runs (files, in the given order) to discard "
        f"before computing statistics (default: {DEFAULT_DISCARD_FIRST}, the "
        "protocol convention)",
    )
    parser.add_argument(
        "--label",
        default=None,
        help="label for the printed table (default: derived from the input files)",
    )
    parser.add_argument(
        "--dclass",
        action="store_true",
        help="parse per-op lines (PROTOCOL §7) instead of the per-frame series, "
        "and emit the d-class RESULTS table (PROTOCOL §9.6) — for d1/d2 (and any "
        "future op-latency-only scenario); requires --scenario. The s-class "
        "(per-frame) path is entirely unaffected when this flag is omitted.",
    )
    parser.add_argument(
        "--graphics",
        action="store_true",
        help="treat the positional files as `dumpsys meminfo` snapshots "
        "(run.sh's run-NN.pss_after.txt) and print their mean Graphics PSS in "
        "MB — RESULTS.md's `Graphics avg (MB)` column, instead of computed by "
        "hand. Neither the s-class nor the d-class path runs in this mode.",
    )
    parser.add_argument(
        "logfiles",
        nargs="*",
        type=Path,
        help="one raw log file per run, in chronological order (or, with "
        "--graphics, one dumpsys meminfo snapshot per run)",
    )
    return parser


def _main_graphics(args: argparse.Namespace) -> int:
    mean_mb, samples = mean_graphics_mb(args.logfiles)
    label = args.label or ", ".join(str(p) for p in args.logfiles)
    print(f"== {label} (graphics) ==")
    if samples == 0:
        # Every snapshot run.sh takes is best-effort, so "none parsed" is a
        # reportable outcome rather than an error exit.
        print("(no Graphics rows parsed)")
        return 0
    print(f"graphics_mean_mb={mean_mb:.2f}  snapshots={samples}")
    return 0


def _main_dclass(args: argparse.Namespace) -> int:
    if not args.scenario:
        raise SystemExit("error: --dclass requires --scenario")
    runs = [load_run_op_records(path, args.scenario) for path in args.logfiles]
    records = discard_first_runs(runs, args.discard_first)
    records = exclude_dclass_warmup(records, args.scenario)
    op_stats = compute_op_stats(records)
    label = args.label or ", ".join(str(p) for p in args.logfiles)
    print(format_op_table(op_stats, label))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = _build_arg_parser()
    args = parser.parse_args(argv)

    if args.self_test:
        return run_self_test()

    if not args.logfiles:
        parser.error("at least one logfile is required unless --self-test is given")

    if args.graphics:
        if args.dclass:
            raise SystemExit("error: --graphics and --dclass are mutually exclusive")
        return _main_graphics(args)

    if args.dclass:
        return _main_dclass(args)

    runs = [load_run_frames(path, args.scenario) for path in args.logfiles]
    frames = discard_first_runs(runs, args.discard_first)
    stats = compute_stats(frames)
    label = args.label or ", ".join(str(p) for p in args.logfiles)
    print(format_table(stats, label))
    return 0


# ---------------------------------------------------------------------
# Self-test (fixture-backed; no pytest dependency — see module docs)
# ---------------------------------------------------------------------


def run_self_test() -> int:
    """Runs every assertion in `test_stats.py` via `unittest`, so
    `--self-test` and `python3 -m unittest test_stats.py` exercise exactly
    the same checks."""
    import unittest

    this_dir = Path(__file__).resolve().parent
    sys.path.insert(0, str(this_dir))
    import test_stats  # deferred: only needed for --self-test

    loader = unittest.TestLoader()
    suite = loader.loadTestsFromModule(test_stats)
    runner = unittest.TextTestRunner(verbosity=2)
    result = runner.run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
