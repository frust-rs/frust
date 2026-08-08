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
        # Raw format v3 (Phase 11.A): present_us split into
        # acquire_us + submit_us (acquire_us + submit_us == old present_us).
        rec = stats.parse_raw_line(
            "frust-perf raw n=42 total_us=15234 rebuild_us=3000 layout_us=2000 "
            "paint_us=8000 encode_us=1100 acquire_us=900 submit_us=234 skipped=0"
        )
        self.assertIsNotNone(rec)
        self.assertEqual(rec.source, "frust")
        self.assertEqual(rec.n, 42)
        self.assertEqual(rec.total_us, 15234)
        self.assertFalse(rec.skipped)
        self.assertEqual(rec.fields["rebuild_us"], "3000")
        # Both new v3 present sub-span fields are captured in the generic
        # field dict alongside encode_us.
        self.assertEqual(rec.fields["encode_us"], "1100")
        self.assertEqual(rec.fields["acquire_us"], "900")
        self.assertEqual(rec.fields["submit_us"], "234")

    def test_parses_frust_raw_line_with_skipped_flag(self):
        rec = stats.parse_raw_line(
            "frust-perf raw n=7 total_us=0 rebuild_us=0 layout_us=0 paint_us=0 "
            "encode_us=0 acquire_us=0 submit_us=0 skipped=1"
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
        line = "  frust-perf raw n=1 total_us=100 rebuild_us=10 layout_us=10 " "paint_us=10 encode_us=35 acquire_us=24 submit_us=11 skipped=0  "
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
            "frust-perf raw n=1 total_us=100 rebuild_us=10 layout_us=10 paint_us=10 encode_us=35 acquire_us=24 submit_us=11 skipped=0",
            "bench-scenario-start s1",
            "frust-perf raw n=2 total_us=200 rebuild_us=20 layout_us=20 paint_us=20 encode_us=70 acquire_us=47 submit_us=23 skipped=0",
            "frust-perf raw n=3 total_us=300 rebuild_us=30 layout_us=30 paint_us=30 encode_us=105 acquire_us=70 submit_us=35 skipped=0",
            "bench-scenario-end s1",
            "frust-perf raw n=4 total_us=400 rebuild_us=40 layout_us=40 paint_us=40 encode_us=140 acquire_us=94 submit_us=46 skipped=0",
        ]
        frames = stats.slice_scenario(lines, "s1")
        self.assertEqual([f.n for f in frames], [2, 3])

    def test_slices_ignore_other_scenario_names(self):
        lines = [
            "bench-scenario-start s2",
            "frust-perf raw n=1 total_us=100 rebuild_us=10 layout_us=10 paint_us=10 encode_us=35 acquire_us=24 submit_us=11 skipped=0",
            "bench-scenario-end s2",
            "bench-scenario-start s1",
            "frust-perf raw n=2 total_us=200 rebuild_us=20 layout_us=20 paint_us=20 encode_us=70 acquire_us=47 submit_us=23 skipped=0",
            "bench-scenario-end s1",
        ]
        frames = stats.slice_scenario(lines, "s1")
        self.assertEqual([f.n for f in frames], [2])

    def test_none_scenario_includes_every_raw_line(self):
        lines = [
            "frust-perf raw n=1 total_us=100 rebuild_us=10 layout_us=10 paint_us=10 encode_us=35 acquire_us=24 submit_us=11 skipped=0",
            "frust-perf raw n=2 total_us=200 rebuild_us=20 layout_us=20 paint_us=20 encode_us=70 acquire_us=47 submit_us=23 skipped=0",
        ]
        frames = stats.slice_scenario(lines, None)
        self.assertEqual([f.n for f in frames], [1, 2])

    def test_repeated_marker_pairs_aggregate_across_cycles(self):
        # S3's continuous-cycling redesign (benchmarks/PROTOCOL.md's S3 row)
        # stamps the same `s3-create1k` marker name once per cycle rather
        # than once total — this locks in that `slice_scenario` aggregates
        # every occurrence of a repeated start/end pair into one combined
        # series instead of only picking up the first (or breaking).
        lines = [
            "bench-scenario-start s3-create1k",
            "frust-perf raw n=1 total_us=100 rebuild_us=10 layout_us=10 paint_us=10 encode_us=35 acquire_us=24 submit_us=11 skipped=0",
            "bench-scenario-end s3-create1k",
            # An unrelated op's window in between must not leak in.
            "bench-scenario-start s3-create10k",
            "frust-perf raw n=2 total_us=999 rebuild_us=10 layout_us=10 paint_us=10 encode_us=484 acquire_us=324 submit_us=161 skipped=0",
            "bench-scenario-end s3-create10k",
            # Cycle 2's create1k window — same marker name, repeats.
            "bench-scenario-start s3-create1k",
            "frust-perf raw n=3 total_us=200 rebuild_us=20 layout_us=20 paint_us=20 encode_us=70 acquire_us=47 submit_us=23 skipped=0",
            "bench-scenario-end s3-create1k",
            # Cycle 3's create1k window — a third repeat.
            "bench-scenario-start s3-create1k",
            "frust-perf raw n=4 total_us=300 rebuild_us=30 layout_us=30 paint_us=30 encode_us=105 acquire_us=70 submit_us=35 skipped=0",
            "bench-scenario-end s3-create1k",
        ]
        frames = stats.slice_scenario(lines, "s3-create1k")
        self.assertEqual(
            [f.n for f in frames],
            [1, 3, 4],
            "every cycle's create1k window must be included, and the "
            "interleaved create10k window must be excluded",
        )
        s = stats.compute_stats(frames)
        self.assertEqual(s.n_total, 3)
        self.assertEqual(s.p50_us, 200)


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


class ParseOpLineTests(unittest.TestCase):
    """Canonical (PROTOCOL §7 `d*`) and grandfathered (S8) per-op line
    shapes — see `stats.py`'s "Per-op lines" module docs."""

    def test_parses_canonical_frust_op_line(self):
        rec = stats.parse_op_line(
            "frust-perf op scenario=d1 op=insert_batch n=3 us=41200 err=0 rows=2000"
        )
        self.assertIsNotNone(rec)
        self.assertEqual(rec.source, "frust")
        self.assertEqual(rec.scenario, "d1")
        self.assertEqual(rec.op, "insert_batch")
        self.assertEqual(rec.n, 3)
        self.assertEqual(rec.us, 41200)
        self.assertFalse(rec.err)
        self.assertEqual(rec.fields["rows"], "2000")

    def test_parses_canonical_flutter_op_line_with_err(self):
        rec = stats.parse_op_line(
            "flutter-perf op scenario=d2 op=select_point n=17 us=88 err=1"
        )
        self.assertIsNotNone(rec)
        self.assertEqual(rec.source, "flutter")
        self.assertEqual(rec.scenario, "d2")
        self.assertEqual(rec.op, "select_point")
        self.assertTrue(rec.err)

    def test_parses_grandfathered_s8_plugin_line_with_no_inline_scenario(self):
        # Real S8-shaped line (frust_bench/src/scenarios/s8_prefs.rs) — pins
        # backward compatibility with the formalized PROTOCOL §7 contract.
        rec = stats.parse_op_line(
            "frust-perf plugin op=write type=bool n=0 us=612 err=0"
        )
        self.assertIsNotNone(rec)
        self.assertEqual(rec.source, "frust")
        self.assertIsNone(rec.scenario, "S8's shape carries no inline scenario= field")
        self.assertEqual(rec.op, "write")
        self.assertEqual(rec.n, 0)
        self.assertEqual(rec.us, 612)
        self.assertFalse(rec.err)
        self.assertEqual(rec.fields["type"], "bool")

    def test_parses_grandfathered_flutter_plugin_line(self):
        rec = stats.parse_op_line(
            "flutter-perf plugin op=read_crossing type=string n=4 us=1500 err=0"
        )
        self.assertIsNotNone(rec)
        self.assertEqual(rec.source, "flutter")
        self.assertEqual(rec.op, "read_crossing")

    def test_rejects_s8_type_total_aggregate_line(self):
        # `errors=` (plural), not `err=` — must not be mistaken for a
        # per-op sample.
        self.assertIsNone(
            stats.parse_op_line(
                "frust-perf plugin op=write type=total n=1000 us=612000 errors=3"
            )
        )

    def test_rejects_s8_errors_tally_marker(self):
        self.assertIsNone(
            stats.parse_op_line(
                "frust-perf plugin s8-errors write_errors=1 read_unexpected_none=0 "
                "read_value_mismatch=0"
            )
        )

    def test_rejects_unrelated_lines(self):
        self.assertIsNone(stats.parse_op_line("some unrelated logcat noise"))
        self.assertIsNone(stats.parse_op_line(""))
        self.assertIsNone(
            stats.parse_op_line("frust-perf raw n=1 total_us=100 skipped=0")
        )

    def test_rejects_op_line_missing_err(self):
        self.assertIsNone(
            stats.parse_op_line("frust-perf op scenario=d1 op=insert_batch n=1 us=100")
        )

    def test_rejects_op_line_with_non_boolean_err(self):
        self.assertIsNone(
            stats.parse_op_line("frust-perf op scenario=d1 op=insert_batch n=1 us=100 err=2")
        )


class SliceOpScenarioTests(unittest.TestCase):
    def test_slices_canonical_lines_by_inline_scenario_regardless_of_bracket(self):
        lines = [
            "bench-scenario-start d1",
            "frust-perf op scenario=d1 op=insert_batch n=0 us=100 err=0 rows=2000",
            "frust-perf op scenario=d1 op=insert_single n=0 us=10 err=0",
            "bench-scenario-end d1",
        ]
        ops = stats.slice_op_scenario(lines, "d1")
        self.assertEqual([o.op for o in ops], ["insert_batch", "insert_single"])

    def test_ignores_other_scenarios_inline(self):
        lines = [
            "frust-perf op scenario=d1 op=insert_batch n=0 us=100 err=0",
            "frust-perf op scenario=d2 op=select_point n=0 us=10 err=0",
        ]
        ops = stats.slice_op_scenario(lines, "d1")
        self.assertEqual([o.op for o in ops], ["insert_batch"])

    def test_grandfathered_s8_lines_attributed_via_exact_marker_name(self):
        lines = [
            "bench-scenario-start s8-write",
            "frust-perf plugin op=write type=bool n=0 us=612 err=0",
            "frust-perf plugin op=write type=i64 n=1 us=580 err=0",
            "frust-perf plugin op=write type=total n=2 us=1192 errors=0",
            "bench-scenario-end s8-write",
            "bench-scenario-start s8-read",
            "frust-perf plugin op=read type=bool n=0 us=400 err=0",
            "bench-scenario-end s8-read",
        ]
        write_ops = stats.slice_op_scenario(lines, "s8-write")
        self.assertEqual([o.op for o in write_ops], ["write", "write"])
        read_ops = stats.slice_op_scenario(lines, "s8-read")
        self.assertEqual([o.op for o in read_ops], ["read"])

    def test_grandfathered_lines_attributed_via_prefix_marker_name(self):
        # A phase-marker bracket named "<scenario>-<phase>" (S8's own
        # `s8-write`/`s8-read` convention) is recognized when the caller
        # asks for the bare "s8" scenario id too.
        lines = [
            "bench-scenario-start s8-write",
            "frust-perf plugin op=write type=bool n=0 us=612 err=0",
            "bench-scenario-end s8-write",
        ]
        ops = stats.slice_op_scenario(lines, "s8")
        self.assertEqual([o.op for o in ops], ["write"])

    def test_lines_outside_any_bracket_are_excluded_for_grandfathered_shape(self):
        lines = [
            "frust-perf plugin op=write type=bool n=0 us=612 err=0",
            "bench-scenario-start s8-write",
            "frust-perf plugin op=write type=bool n=1 us=500 err=0",
            "bench-scenario-end s8-write",
        ]
        ops = stats.slice_op_scenario(lines, "s8-write")
        self.assertEqual([o.n for o in ops], [1])


class ComputeOpStatsTests(unittest.TestCase):
    def test_odd_count_percentiles_match_hand_computed_values(self):
        # sorted: [90, 100, 110, 120, 150] -> p50=110 p95=150 p99=150
        recs = [
            stats.OpRecord(source="frust", scenario="d1", op="insert_batch", n=i, us=us, err=False)
            for i, us in enumerate([100, 120, 90, 150, 110])
        ]
        result = stats.compute_op_stats(recs)
        s = result["insert_batch"]
        self.assertEqual(s.n_total, 5)
        self.assertEqual(s.n_ok, 5)
        self.assertEqual(s.errors, 0)
        self.assertEqual(s.p50_us, 110)
        self.assertEqual(s.p95_us, 150)
        self.assertEqual(s.p99_us, 150)
        self.assertEqual(s.worst_us, 150)

    def test_even_count_percentiles_match_hand_computed_values(self):
        # sorted: [50, 60, 70, 80] -> p50=60 p95=80 p99=80
        recs = [
            stats.OpRecord(source="frust", scenario="d1", op="insert_single", n=i, us=us, err=False)
            for i, us in enumerate([50, 70, 60, 80])
        ]
        s = stats.compute_op_stats(recs)["insert_single"]
        self.assertEqual(s.p50_us, 60)
        self.assertEqual(s.p95_us, 80)
        self.assertEqual(s.p99_us, 80)
        self.assertEqual(s.worst_us, 80)

    def test_err_lines_excluded_from_latency_but_counted(self):
        recs = [
            stats.OpRecord(source="frust", scenario="d2", op="select_point", n=0, us=100, err=False),
            stats.OpRecord(source="frust", scenario="d2", op="select_point", n=1, us=200, err=False),
            stats.OpRecord(source="frust", scenario="d2", op="select_point", n=2, us=999_999, err=True),
            stats.OpRecord(source="frust", scenario="d2", op="select_point", n=3, us=300, err=False),
        ]
        s = stats.compute_op_stats(recs)["select_point"]
        self.assertEqual(s.n_total, 4)
        self.assertEqual(s.n_ok, 3)
        self.assertEqual(s.errors, 1)
        # ok values sorted: [100, 200, 300] -> p50=200 p95=300 p99=300
        self.assertEqual(s.p50_us, 200)
        self.assertEqual(s.p95_us, 300)
        self.assertEqual(s.p99_us, 300)
        self.assertEqual(s.worst_us, 300, "the err=1 sample must never surface as the worst")
        self.assertAlmostEqual(s.ops_per_sec, 3 / ((100 + 200 + 300) / 1_000_000))

    def test_mixed_op_interleaving_groups_correctly(self):
        recs = [
            stats.OpRecord(source="frust", scenario="d1", op="insert_batch", n=0, us=100, err=False),
            stats.OpRecord(source="frust", scenario="d1", op="insert_single", n=0, us=10, err=False),
            stats.OpRecord(source="frust", scenario="d1", op="insert_batch", n=1, us=120, err=False),
            stats.OpRecord(source="frust", scenario="d1", op="insert_single", n=1, us=12, err=False),
            stats.OpRecord(source="frust", scenario="d1", op="insert_batch", n=2, us=110, err=False),
        ]
        result = stats.compute_op_stats(recs)
        self.assertEqual(set(result), {"insert_batch", "insert_single"})
        self.assertEqual(result["insert_batch"].n_total, 3)
        self.assertEqual(result["insert_single"].n_total, 2)

    def test_ops_per_sec_round_number(self):
        recs = [
            stats.OpRecord(source="frust", scenario="d1", op="insert_batch", n=0, us=500_000, err=False),
            stats.OpRecord(source="frust", scenario="d1", op="insert_batch", n=1, us=500_000, err=False),
        ]
        s = stats.compute_op_stats(recs)["insert_batch"]
        self.assertEqual(s.ops_per_sec, 2.0)

    def test_op_with_no_ok_samples_reports_all_zero(self):
        recs = [
            stats.OpRecord(source="frust", scenario="d2", op="select_point", n=0, us=100, err=True),
        ]
        s = stats.compute_op_stats(recs)["select_point"]
        self.assertEqual(s.n_total, 1)
        self.assertEqual(s.n_ok, 0)
        self.assertEqual(s.errors, 1)
        self.assertEqual(s.p50_us, 0)
        self.assertEqual(s.worst_us, 0)
        self.assertEqual(s.ops_per_sec, 0.0)

    def test_empty_input_returns_empty_dict(self):
        self.assertEqual(stats.compute_op_stats([]), {})


class ExcludeDclassWarmupTests(unittest.TestCase):
    def test_drops_n_zero_sample_per_run_for_d1_insert_batch_and_insert_single(self):
        # Two runs' worth of insert_batch/insert_single, already concatenated
        # (n resets to 0 at the start of each run, per PROTOCOL §7).
        recs = [
            stats.OpRecord(source="frust", scenario="d1", op="insert_batch", n=0, us=100, err=False),
            stats.OpRecord(source="frust", scenario="d1", op="insert_batch", n=1, us=110, err=False),
            stats.OpRecord(source="frust", scenario="d1", op="insert_single", n=0, us=10, err=False),
            stats.OpRecord(source="frust", scenario="d1", op="insert_single", n=1, us=11, err=False),
            # run 2
            stats.OpRecord(source="frust", scenario="d1", op="insert_batch", n=0, us=105, err=False),
            stats.OpRecord(source="frust", scenario="d1", op="insert_batch", n=1, us=115, err=False),
            stats.OpRecord(source="frust", scenario="d1", op="insert_single", n=0, us=12, err=False),
            stats.OpRecord(source="frust", scenario="d1", op="insert_single", n=1, us=13, err=False),
        ]
        kept = stats.exclude_dclass_warmup(recs, "d1")
        self.assertEqual(
            [(r.op, r.n) for r in kept],
            [
                ("insert_batch", 1),
                ("insert_single", 1),
                ("insert_batch", 1),
                ("insert_single", 1),
            ],
        )

    def test_d2_range_scan_is_not_warmup_excluded(self):
        recs = [
            stats.OpRecord(source="frust", scenario="d2", op="select_point", n=0, us=100, err=False),
            stats.OpRecord(source="frust", scenario="d2", op="range_scan", n=0, us=5000, err=False),
        ]
        kept = stats.exclude_dclass_warmup(recs, "d2")
        self.assertEqual([(r.op, r.n) for r in kept], [("range_scan", 0)])

    def test_scenario_with_no_declared_warmup_ops_is_unchanged(self):
        recs = [
            stats.OpRecord(source="frust", scenario=None, op="write", n=0, us=612, err=False),
        ]
        self.assertEqual(stats.exclude_dclass_warmup(recs, "s8"), recs)


class FormatOpTableTests(unittest.TestCase):
    def test_reports_one_row_per_op_with_expected_fields(self):
        op_stats = stats.compute_op_stats(
            [
                stats.OpRecord(source="frust", scenario="d1", op="insert_batch", n=0, us=100, err=False),
                stats.OpRecord(source="frust", scenario="d1", op="insert_single", n=0, us=10, err=False),
            ]
        )
        table = stats.format_op_table(op_stats, "frust d1")
        self.assertIn("insert_batch", table)
        self.assertIn("insert_single", table)
        self.assertIn("d-class", table)

    def test_empty_stats_reports_no_samples_note(self):
        table = stats.format_op_table({}, "frust d1")
        self.assertIn("no per-op samples", table)


class DclassEndToEndTests(unittest.TestCase):
    """Load-a-log-file-through-slice-through-discard-through-compute, the
    same pipeline `_main_dclass` runs, over a hand-built multi-run fixture
    written to a temp file per run."""

    def test_full_pipeline_over_three_runs_discards_first_two(self):
        import tempfile

        run_lines = [
            # run 1 (discarded) — deliberately wrong-looking numbers so a
            # failure to discard would be obvious.
            [
                "bench-scenario-start d1",
                "frust-perf op scenario=d1 op=insert_batch n=0 us=999999 err=0",
                "bench-scenario-end d1",
            ],
            # run 2 (discarded)
            [
                "bench-scenario-start d1",
                "frust-perf op scenario=d1 op=insert_batch n=0 us=888888 err=0",
                "bench-scenario-end d1",
            ],
            # run 3 (kept) — insert_batch n=0 is warmup-excluded, n=1..3 kept
            [
                "bench-scenario-start d1",
                "frust-perf op scenario=d1 op=insert_batch n=0 us=100 err=0",
                "frust-perf op scenario=d1 op=insert_batch n=1 us=100 err=0",
                "frust-perf op scenario=d1 op=insert_batch n=2 us=200 err=0",
                "frust-perf op scenario=d1 op=insert_batch n=3 us=300 err=1",
                "bench-scenario-end d1",
            ],
        ]
        with tempfile.TemporaryDirectory() as tmp:
            paths = []
            for i, lines in enumerate(run_lines):
                p = Path(tmp) / f"run-{i:02d}.log"
                p.write_text("\n".join(lines) + "\n")
                paths.append(p)

            runs = [stats.load_run_op_records(p, "d1") for p in paths]
            combined = stats.discard_first_runs(runs, 2)
            combined = stats.exclude_dclass_warmup(combined, "d1")
            result = stats.compute_op_stats(combined)

        s = result["insert_batch"]
        # n=0 (warmup) dropped, leaving n=1 (100), n=2 (200), n=3 (300, err).
        self.assertEqual(s.n_total, 3)
        self.assertEqual(s.n_ok, 2)
        self.assertEqual(s.errors, 1)
        self.assertEqual(s.p50_us, 100)
        self.assertEqual(s.p95_us, 200)
        self.assertEqual(s.worst_us, 200)


if __name__ == "__main__":
    unittest.main()
