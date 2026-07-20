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
            paint_us=<> encode_present_us=<> skipped=<0|1>`
- Flutter: `flutter-perf raw n=<n> build_us=<> raster_us=<> total_us=<>`
  (no `skipped` field — Flutter's `addTimingsCallback` only ever reports
  frames it actually rendered, so every parsed Flutter frame is treated as
  non-skipped; see `benchmarks/flutter_bench`'s raw-capture callback for
  the emitting side.)

Only `n`/`total_us`/`skipped` are used for the cross-app percentile/budget
table below (the field the two apps' pass breakdowns don't share a
vocabulary for); every parsed key=value pair is still kept per-frame
(`FrameRecord.fields`) for a caller that wants the framework-specific
pass breakdown (e.g. Frust's `rebuild_us`).

- `bench-scenario-start <name>` / `bench-scenario-end <name>` — identical
  marker strings on both sides (see `perf::mark_scenario_start`/
  `mark_scenario_end`).
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass, field
from pathlib import Path

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

# Runs discarded by default before computing statistics (protocol
# convention — declared as OUR convention, not a framework fact; see
# `benchmarks/PROTOCOL.md`).
DEFAULT_DISCARD_FIRST = 2


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


def parse_marker_line(line: str) -> tuple[str, str] | None:
    """Parses a `bench-scenario-start/end <name>` line into `(edge, name)`
    (`edge` is `"start"`/`"end"`), or `None` if `line` isn't a marker line or
    carries no name. Like [`parse_raw_line`], the prefix is located anywhere
    in the line, not just at its start (see that function's docs)."""
    stripped = line.strip()
    start_idx = stripped.find(MARKER_START_PREFIX)
    end_idx = stripped.find(MARKER_END_PREFIX)
    if start_idx != -1:
        edge, prefix, idx = "start", MARKER_START_PREFIX, start_idx
    elif end_idx != -1:
        edge, prefix, idx = "end", MARKER_END_PREFIX, end_idx
    else:
        return None
    name = stripped[idx + len(prefix) :].strip()
    if not name:
        return None
    return edge, name


def slice_scenario(lines: list[str], scenario: str | None) -> list[FrameRecord]:
    """Extracts the [`FrameRecord`]s bracketed by `bench-scenario-start
    <scenario>` / `bench-scenario-end <scenario>` marker lines. When
    `scenario` is `None`, every parseable raw-frame line in `lines` is
    included regardless of markers (useful for a single-scenario fixture
    with no marker bracket, or a caller that already sliced upstream)."""
    frames: list[FrameRecord] = []
    active = scenario is None
    for line in lines:
        marker = parse_marker_line(line)
        if marker is not None:
            edge, name = marker
            if scenario is not None and name == scenario:
                active = edge == "start"
            continue
        if active:
            rec = parse_raw_line(line)
            if rec is not None:
                frames.append(rec)
    return frames


def load_run_frames(path: Path, scenario: str | None) -> list[FrameRecord]:
    """Reads one run's log file and slices its scenario frames."""
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    return slice_scenario(lines, scenario)


def discard_first_runs(
    runs: list[list[FrameRecord]], discard_first: int
) -> list[FrameRecord]:
    """Drops the first `discard_first` *runs* (not frames), then
    concatenates the remaining runs' frames into one series. If there
    aren't more runs than `discard_first`, nothing is discarded (a single
    fixture/smoke-test log is still usable directly) rather than erroring."""
    if len(runs) > discard_first:
        kept = runs[discard_first:]
    else:
        kept = runs
    combined: list[FrameRecord] = []
    for run in kept:
        combined.extend(run)
    return combined


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
        "logfiles",
        nargs="*",
        type=Path,
        help="one raw log file per run, in chronological order",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = _build_arg_parser()
    args = parser.parse_args(argv)

    if args.self_test:
        return run_self_test()

    if not args.logfiles:
        parser.error("at least one logfile is required unless --self-test is given")

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
