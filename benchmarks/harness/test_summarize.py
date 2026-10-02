#!/usr/bin/env python3
"""benchmarks/harness/test_summarize.py — plain-`unittest` tests for
`summarize.py`'s idle-memory conversion.

`dumpsys meminfo`'s `TOTAL PSS` snapshots are in KiB, never decimal MB — this
locks in `total_pss_mib`'s KiB/1024 conversion (and the "MiB" label the S7
table renders it under) against RESULTS.md's own hand-computed control: a
116,328.5 KiB mean converts to 113.6 MiB (RESULTS.md's "Packed relocations +
ICF" device leg).

Runnable either via `python3 -m unittest benchmarks.harness.test_summarize`
from the repo root, or `python3 benchmarks/harness/test_summarize.py [-v]`
directly from any directory (see `test_size_attribute.py`, whose import-guard
style this mirrors).
"""

from __future__ import annotations

import argparse
import tempfile
import unittest
from pathlib import Path

try:
    import summarize
except ImportError:  # running as `python3 -m unittest benchmarks.harness.test_summarize`
    from . import summarize


# A trimmed real `dumpsys meminfo` App Summary block (see
# test_stats.py's GraphicsSnapshotTests.SNAPSHOT) — only the `TOTAL PSS` row
# matters to `total_pss_mib`, but the surrounding shape is kept so the fixture
# reads like an actual `run-NN.pss_after.txt` capture.
def snapshot_text(total_pss_kib: int) -> str:
    return f"""Applications Memory Usage (in Kilobytes):
Uptime: 35959166 Realtime: 174459924

** MEMINFO in pid 14421 [it.f0x.frustbench] **
                   Pss  Private  Private  SwapPss      Rss     Heap     Heap     Heap
                 Total    Dirty    Clean    Dirty    Total     Size    Alloc     Free
                ------   ------   ------   ------   ------   ------   ------   ------
  Native Heap    79755    79692        4        1    83036    88084    80366     1553

 App Summary
                       Pss(KB)                        Rss(KB)
                        ------                         ------
           Java Heap:     3136                          37396

           TOTAL PSS:   {total_pss_kib}            TOTAL RSS:   275272       TOTAL SWAP PSS:       10
"""


class TotalPssMibTests(unittest.TestCase):
    """`total_pss_mib` — the KiB/1024 conversion the old code skipped
    (it divided by 1000.0 and called the result "MB")."""

    def _write_snapshot(self, tmp: str, name: str, total_pss_kib: int) -> Path:
        p = Path(tmp) / name
        p.write_text(snapshot_text(total_pss_kib))
        return p

    def test_known_snapshot_converts_kib_to_mib_matching_the_results_md_control(self):
        # RESULTS.md's "Packed relocations + ICF" leg quotes this exact
        # control by hand: "mean TOTAL PSS 113.6 MiB (... the 12-run mean is
        # 116,328.5 KiB)". A single 116,328 KiB snapshot is close enough to
        # round to the identical "113.60" 2-decimal figure (116328 / 1024 =
        # 113.6015625), so this reproduces that control rather than
        # approximating it.
        with tempfile.TemporaryDirectory() as tmp:
            self._write_snapshot(tmp, "run-00.pss_after.txt", 116328)
            mean, lo, hi, n = summarize.total_pss_mib(Path(tmp), discard=0)

        self.assertEqual(n, 1)
        self.assertEqual(f"{mean:.2f}", "113.60")
        self.assertEqual(lo, mean)
        self.assertEqual(hi, mean)

    def test_old_1000_divisor_would_mislabel_the_control_as_116_33(self):
        # Documents exactly what the bug produced or under would revert to:
        # 116328 / 1000 = 116.328, which `total_pss_mib`'s KiB/1024 math must
        # NOT reproduce.
        with tempfile.TemporaryDirectory() as tmp:
            self._write_snapshot(tmp, "run-00.pss_after.txt", 116328)
            mean, _, _, _ = summarize.total_pss_mib(Path(tmp), discard=0)
        self.assertNotEqual(f"{mean:.2f}", "116.33")

    def test_discard_first_skips_leading_snapshots(self):
        with tempfile.TemporaryDirectory() as tmp:
            self._write_snapshot(tmp, "run-00.pss_after.txt", 999999)
            self._write_snapshot(tmp, "run-01.pss_after.txt", 999999)
            self._write_snapshot(tmp, "run-02.pss_after.txt", 116328)
            mean, lo, hi, n = summarize.total_pss_mib(Path(tmp), discard=2)

        self.assertEqual(n, 1)
        self.assertEqual(f"{mean:.2f}", "113.60")

    def test_mean_over_several_kept_snapshots(self):
        # (100000 + 120000 + 140000) / 3 KiB -> MiB, hand-computed.
        with tempfile.TemporaryDirectory() as tmp:
            self._write_snapshot(tmp, "run-00.pss_after.txt", 100000)
            self._write_snapshot(tmp, "run-01.pss_after.txt", 120000)
            self._write_snapshot(tmp, "run-02.pss_after.txt", 140000)
            mean, lo, hi, n = summarize.total_pss_mib(Path(tmp), discard=0)

        self.assertEqual(n, 3)
        self.assertAlmostEqual(mean, (100000 + 120000 + 140000) / 3 / 1024)
        self.assertAlmostEqual(lo, 100000 / 1024)
        self.assertAlmostEqual(hi, 140000 / 1024)

    def test_no_snapshots_reports_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            mean, lo, hi, n = summarize.total_pss_mib(Path(tmp), discard=0)
        self.assertIsNone(mean)
        self.assertIsNone(lo)
        self.assertIsNone(hi)
        self.assertEqual(n, 0)


class S7TableRendersMibTests(unittest.TestCase):
    """End-to-end through `summarize.build()`: the S7 idle-memory row must
    render the "MiB" label (never "MB"), and the JSON dump's key is
    `idle_pss_mib`."""

    def _build_args(self, frust_dir: str) -> argparse.Namespace:
        return argparse.Namespace(
            frust=frust_dir,
            flutter=None,
            duration=30,
            s7_duration=60,
            discard_first=0,
            frust_label="Frust (profile)",
            flutter_label="Flutter (profile)",
            flutter_work_sum=False,
            frust_work_row=False,
        )

    def test_idle_memory_row_says_mib_not_mb(self):
        with tempfile.TemporaryDirectory() as tmp:
            s7 = Path(tmp) / "s7"
            s7.mkdir()
            # `have()` requires at least one run-NN.log alongside the
            # pss_after.txt snapshot for the S7 block to be considered
            # present.
            (s7 / "run-00.log").write_text("")
            (s7 / "run-00.pss_after.txt").write_text(snapshot_text(116328))

            md, js = summarize.build(self._build_args(tmp))

        self.assertIn("MiB", md)
        self.assertNotIn(" MB", md, "the old decimal-MB label must not survive the fix")
        self.assertIn("idle_pss_mib", js["scenarios"]["s7"]["frust"])
        self.assertNotIn("idle_pss_mb", js["scenarios"]["s7"]["frust"])
        pss = js["scenarios"]["s7"]["frust"]["idle_pss_mib"]
        self.assertEqual(f"{pss[0]:.2f}", "113.60")


if __name__ == "__main__":
    unittest.main()
