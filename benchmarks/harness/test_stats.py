#!/usr/bin/env python3
"""benchmarks/harness/test_stats.py — plain-assert, pytest-free tests for
`stats.py`.

Runnable either via `python3 -m unittest test_stats` (or
`test_stats.py -v`) from this directory, or via `stats.py --self-test`
(which loads this module and runs it through the same `unittest` runner —
see `stats.run_self_test`). No third-party test runner required, matching
the harness's "no pip access guaranteed" constraint.
"""

from __future__ import annotations

import unittest
from pathlib import Path

import stats

FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


class ParseRawLineTests(unittest.TestCase):
    def test_parses_frust_raw_line(self):
        rec = stats.parse_raw_line(
            "frust-perf raw n=42 total_us=15234 rebuild_us=3000 layout_us=2000 "
            "paint_us=8000 encode_present_us=2234 skipped=0"
        )
        self.assertIsNotNone(rec)
        self.assertEqual(rec.source, "frust")
        self.assertEqual(rec.n, 42)
        self.assertEqual(rec.total_us, 15234)
        self.assertFalse(rec.skipped)
        self.assertEqual(rec.fields["rebuild_us"], "3000")

    def test_parses_frust_raw_line_with_skipped_flag(self):
        rec = stats.parse_raw_line(
            "frust-perf raw n=7 total_us=0 rebuild_us=0 layout_us=0 paint_us=0 "
            "encode_present_us=0 skipped=1"
        )
        self.assertIsNotNone(rec)
        self.assertTrue(rec.skipped)

    def test_parses_flutter_raw_line(self):
        rec = stats.parse_raw_line(
            "flutter-perf raw n=3 build_us=6000 raster_us=8000 total_us=14000"
        )
        self.assertIsNotNone(rec)
        self.assertEqual(rec.source, "flutter")
        self.assertEqual(rec.n, 3)
        self.assertEqual(rec.total_us, 14000)
        # Flutter's raw line carries no `skipped` field: absence means
        # "not skipped", never a parse failure.
        self.assertFalse(rec.skipped)

    def test_ignores_unrelated_log_lines(self):
        self.assertIsNone(stats.parse_raw_line("some unrelated logcat noise"))
        self.assertIsNone(stats.parse_raw_line(""))

    def test_rejects_raw_line_missing_required_fields(self):
        self.assertIsNone(stats.parse_raw_line("frust-perf raw rebuild_us=10"))
        self.assertIsNone(stats.parse_raw_line("flutter-perf raw n=1"))

    def test_logcat_prefix_before_message_still_parses(self):
        # A real adb logcat -v raw line has no extra prefix by default, but
        # -v threadtime etc. would; parse_raw_line only cares about the
        # `frust-perf raw`/`flutter-perf raw` substring position at the
        # start of the (already-stripped) line, so a harness that greps
        # logcat down to the bare message before parsing works correctly.
        line = "  frust-perf raw n=1 total_us=100 rebuild_us=10 layout_us=10 " "paint_us=10 encode_present_us=70 skipped=0  "
        rec = stats.parse_raw_line(line)
        self.assertIsNotNone(rec)
        self.assertEqual(rec.n, 1)


class ParseMarkerLineTests(unittest.TestCase):
    def test_parses_start_marker(self):
        self.assertEqual(
            stats.parse_marker_line("bench-scenario-start s1"), ("start", "s1")
        )

    def test_parses_end_marker(self):
        self.assertEqual(
            stats.parse_marker_line("bench-scenario-end cold_start"),
            ("end", "cold_start"),
        )

    def test_non_marker_line_returns_none(self):
        self.assertIsNone(stats.parse_marker_line("frust-perf raw n=1 total_us=1"))

    def test_marker_with_no_name_returns_none(self):
        self.assertIsNone(stats.parse_marker_line("bench-scenario-start"))


class SliceScenarioTests(unittest.TestCase):
    def test_slices_only_bracketed_frames(self):
        lines = [
            "noise before",
            "frust-perf raw n=1 total_us=100 rebuild_us=10 layout_us=10 paint_us=10 encode_present_us=70 skipped=0",
            "bench-scenario-start s1",
            "frust-perf raw n=2 total_us=200 rebuild_us=20 layout_us=20 paint_us=20 encode_present_us=140 skipped=0",
            "frust-perf raw n=3 total_us=300 rebuild_us=30 layout_us=30 paint_us=30 encode_present_us=210 skipped=0",
            "bench-scenario-end s1",
            "frust-perf raw n=4 total_us=400 rebuild_us=40 layout_us=40 paint_us=40 encode_present_us=280 skipped=0",
        ]
        frames = stats.slice_scenario(lines, "s1")
        self.assertEqual([f.n for f in frames], [2, 3])

    def test_slices_ignore_other_scenario_names(self):
        lines = [
            "bench-scenario-start s2",
            "frust-perf raw n=1 total_us=100 rebuild_us=10 layout_us=10 paint_us=10 encode_present_us=70 skipped=0",
            "bench-scenario-end s2",
            "bench-scenario-start s1",
            "frust-perf raw n=2 total_us=200 rebuild_us=20 layout_us=20 paint_us=20 encode_present_us=140 skipped=0",
            "bench-scenario-end s1",
        ]
        frames = stats.slice_scenario(lines, "s1")
        self.assertEqual([f.n for f in frames], [2])

    def test_none_scenario_includes_every_raw_line(self):
        lines = [
            "frust-perf raw n=1 total_us=100 rebuild_us=10 layout_us=10 paint_us=10 encode_present_us=70 skipped=0",
            "frust-perf raw n=2 total_us=200 rebuild_us=20 layout_us=20 paint_us=20 encode_present_us=140 skipped=0",
        ]
        frames = stats.slice_scenario(lines, None)
        self.assertEqual([f.n for f in frames], [1, 2])


class DiscardFirstRunsTests(unittest.TestCase):
    def _run(self, *ns: int) -> list[stats.FrameRecord]:
        return [
            stats.FrameRecord(source="frust", n=n, total_us=n * 1000, skipped=False)
            for n in ns
        ]

    def test_discards_leading_runs_and_concatenates_rest(self):
        runs = [self._run(1, 2), self._run(3, 4), self._run(5, 6)]
        combined = stats.discard_first_runs(runs, 2)
        self.assertEqual([f.n for f in combined], [5, 6])

    def test_no_discard_when_not_enough_runs(self):
        runs = [self._run(1), self._run(2)]
        combined = stats.discard_first_runs(runs, 2)
        self.assertEqual(
            [f.n for f in combined],
            [1, 2],
            "with only as many runs as the discard count, nothing is dropped",
        )

    def test_discard_zero_keeps_everything(self):
        runs = [self._run(1), self._run(2)]
        combined = stats.discard_first_runs(runs, 0)
        self.assertEqual([f.n for f in combined], [1, 2])


class ComputeStatsTests(unittest.TestCase):
    def test_known_distribution_matches_rust_side_formula(self):
        # Mirrors frust-shell-common::perf's own
        # percentile_known_distribution_1_to_100ms test: a 1..=100ms sample
        # (n=100) puts p50/p95/p99 at exactly 50/95/99ms under nearest-rank
        # with ceil(p*n/100) — the same formula stats.py implements, so a
        # Python and Rust computation over an identical series never
        # disagree (see module docs).
        frames = [
            stats.FrameRecord(source="frust", n=i, total_us=i * 1000, skipped=False)
            for i in range(1, 101)
        ]
        s = stats.compute_stats(frames)
        self.assertEqual(s.p50_us, 50_000)
        self.assertEqual(s.p95_us, 95_000)
        self.assertEqual(s.p99_us, 99_000)
        self.assertEqual(s.worst_us, 100_000)

    def test_skipped_frames_excluded_from_percentiles_but_counted(self):
        frames = [
            stats.FrameRecord(source="frust", n=1, total_us=16_000, skipped=False),
            stats.FrameRecord(source="frust", n=2, total_us=0, skipped=True),
            stats.FrameRecord(source="frust", n=3, total_us=16_000, skipped=False),
        ]
        s = stats.compute_stats(frames)
        self.assertEqual(s.n_total, 3)
        self.assertEqual(s.n_active, 2)
        self.assertEqual(s.skipped, 1)
        self.assertEqual(s.p50_us, 16_000)

    def test_empty_input_is_all_zero(self):
        s = stats.compute_stats([])
        self.assertEqual(s.as_dict()["n_total"], 0)
        self.assertEqual(s.p50_us, 0)
        self.assertEqual(s.worst_us, 0)

    def test_missed_budget_counts(self):
        frames = [
            stats.FrameRecord(source="frust", n=1, total_us=5_000, skipped=False),  # under both
            stats.FrameRecord(source="frust", n=2, total_us=10_000, skipped=False),  # over 120hz only
            stats.FrameRecord(source="frust", n=3, total_us=20_000, skipped=False),  # over both
        ]
        s = stats.compute_stats(frames)
        self.assertEqual(s.missed_120hz, 2)
        self.assertEqual(s.missed_60hz, 1)


class FormatTableTests(unittest.TestCase):
    def test_identical_shape_for_both_apps(self):
        frust_stats = stats.compute_stats(
            [stats.FrameRecord(source="frust", n=1, total_us=15_000, skipped=False)]
        )
        flutter_stats = stats.compute_stats(
            [stats.FrameRecord(source="flutter", n=1, total_us=15_000, skipped=False)]
        )
        frust_table = stats.format_table(frust_stats, "Frust S1").splitlines()
        flutter_table = stats.format_table(flutter_stats, "Flutter S1").splitlines()
        self.assertEqual(
            len(frust_table),
            len(flutter_table),
            "the two apps' tables must have the identical line shape",
        )
        # Every line but the label line must literally match given
        # identical input stats — proof there is exactly one code path
        # computing both, not two.
        self.assertEqual(frust_table[1:], flutter_table[1:])


class FixtureIntegrationTests(unittest.TestCase):
    """End-to-end: load the real fixture files this task ships, exactly the
    way `run.sh`/a report author would, and check the resulting numbers by
    hand-computed expectation (see the task's completion notes for the
    arithmetic)."""

    def _load(self, app: str) -> list[list[stats.FrameRecord]]:
        run_dir = FIXTURES_DIR / app
        run_files = sorted(run_dir.glob("run*.log"))
        self.assertEqual(len(run_files), 3, f"expected 3 fixture runs for {app}")
        return [stats.load_run_frames(p, "s1") for p in run_files]

    def test_frust_fixtures_produce_expected_stats_after_discard(self):
        runs = self._load("frust")
        combined = stats.discard_first_runs(runs, 2)
        s = stats.compute_stats(combined)
        self.assertEqual(s.n_total, 10)
        self.assertEqual(s.n_active, 9)
        self.assertEqual(s.skipped, 1)
        self.assertEqual(s.p50_us, 15_000)
        self.assertEqual(s.p95_us, 17_200)
        self.assertEqual(s.p99_us, 17_200)
        self.assertEqual(s.worst_us, 17_200)
        self.assertEqual(s.missed_60hz, 1)
        self.assertEqual(s.missed_120hz, 9)

    def test_flutter_fixtures_produce_expected_stats_after_discard(self):
        runs = self._load("flutter")
        combined = stats.discard_first_runs(runs, 2)
        s = stats.compute_stats(combined)
        self.assertEqual(s.n_total, 10)
        self.assertEqual(s.n_active, 10)
        self.assertEqual(s.skipped, 0)
        self.assertEqual(s.p50_us, 14_400)
        self.assertEqual(s.p95_us, 17_500)
        self.assertEqual(s.p99_us, 17_500)
        self.assertEqual(s.worst_us, 17_500)
        self.assertEqual(s.missed_60hz, 1)
        self.assertEqual(s.missed_120hz, 10)

    def test_frust_and_flutter_fixtures_produce_identical_format(self):
        # Acceptance criterion 1: "stats.py produces identical-format
        # percentile tables from the two fixture logs." The two apps'
        # underlying numbers legitimately differ (different fixture data),
        # so this checks *shape* — same stat field names, same line count —
        # not identical values.
        import re

        frust_runs = self._load("frust")
        flutter_runs = self._load("flutter")
        frust_stats = stats.compute_stats(stats.discard_first_runs(frust_runs, 2))
        flutter_stats = stats.compute_stats(stats.discard_first_runs(flutter_runs, 2))
        self.assertEqual(set(frust_stats.as_dict()), set(flutter_stats.as_dict()))

        frust_table = stats.format_table(frust_stats, "frust s1")
        flutter_table = stats.format_table(flutter_stats, "flutter s1")
        self.assertEqual(
            len(frust_table.splitlines()), len(flutter_table.splitlines())
        )

        key_pattern = re.compile(r"([A-Za-z0-9_.]+)=")
        frust_keys = key_pattern.findall(frust_table)
        flutter_keys = key_pattern.findall(flutter_table)
        self.assertEqual(
            frust_keys,
            flutter_keys,
            "both tables must report the identical ordered set of stat fields",
        )


if __name__ == "__main__":
    unittest.main()
