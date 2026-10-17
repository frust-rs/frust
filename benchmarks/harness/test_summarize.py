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


IOS_COLDSTART = """== launch 1 2026-10-17T15:36:34Z (ios: FrontBoard bootstrap -> scene content ready)
Oct 17 22:36:34.445051 SpringBoard(FrontBoard)[35] <Notice>: Bootstrapping app<it.f0x.frustbench> with intent foreground-interactive
Oct 17 22:36:35.001877 SpringBoard(FrontBoard)[35] <Notice>: [(FBSceneManager):sceneID:it.f0x.frustbench-default] scene content state changed: ready
TotalTime: 557
== launch 2 2026-10-17T15:36:46Z (ios: FrontBoard bootstrap -> scene content ready)
Oct 17 22:36:46.100000 SpringBoard(FrontBoard)[35] <Notice>: Bootstrapping app<it.f0x.frustbench> with intent foreground-interactive
Oct 17 22:36:46.400000 SpringBoard(FrontBoard)[35] <Notice>: [(FBSceneManager):sceneID:it.f0x.frustbench-default] scene content state changed: ready
TotalTime: 300
== launch 3 2026-10-17T15:36:58Z (ios: FrontBoard bootstrap -> scene content ready)
Oct 17 22:36:58.100000 SpringBoard(FrontBoard)[35] <Notice>: Bootstrapping app<it.f0x.frustbench> with intent foreground-interactive
Oct 17 22:36:58.420000 SpringBoard(FrontBoard)[35] <Notice>: [(FBSceneManager):sceneID:it.f0x.frustbench-default] scene content state changed: ready
TotalTime: 320
"""

ANDROID_COLDSTART = """== launch 1 2026-10-17T11:03:42Z
Starting: Intent { act=android.intent.action.VIEW dat=frustbench://s7/... pkg=it.f0x.frustbench }
Status: ok
LaunchState: COLD
Activity: it.f0x.frustbench/.MainActivity
TotalTime: 127
WaitTime: 130
Complete
== launch 2 2026-10-17T11:03:50Z
Starting: Intent { act=android.intent.action.VIEW dat=frustbench://s7/... pkg=it.f0x.frustbench }
Status: ok
LaunchState: COLD
Activity: it.f0x.frustbench/.MainActivity
TotalTime: 141
WaitTime: 143
Complete
"""

# Three ~1 s sysmon samples of one run: 100 ms of core time over 2.0 s = 5 %.
SYSMON_RUN = """t_s=41.000 cpu_pct=na cpu_ns=1000000000 footprint_bytes=41943040 rss_bytes=40000000
t_s=42.000 cpu_pct=4.90 cpu_ns=1050000000 footprint_bytes=42991616 rss_bytes=40000000
t_s=43.000 cpu_pct=5.10 cpu_ns=1100000000 footprint_bytes=44040192 rss_bytes=40000000
"""


class S7IosRowsTests(unittest.TestCase):
    """The iOS S7 side files (matrix.sh's FrontBoard coldstart.txt, run.sh's
    run-NN.sysmon.txt) relabel the S7 rows to name their instrument; an
    Android tree keeps its labels and JSON shape untouched."""

    def _args(self, frust_dir: str, discard: int = 0) -> argparse.Namespace:
        return argparse.Namespace(
            frust=frust_dir, flutter=None, duration=30, s7_duration=60, discard_first=discard,
            frust_label="Frust (profile)", flutter_label="Flutter (profile)",
            flutter_work_sum=False, frust_work_row=False,
        )

    def test_frontboard_coldstart_is_parsed_like_am_start_and_relabels_the_row(self):
        with tempfile.TemporaryDirectory() as tmp:
            s7 = Path(tmp) / "s7"
            s7.mkdir()
            (s7 / "run-00.log").write_text("")
            (s7 / "coldstart.txt").write_text(IOS_COLDSTART)
            md, js = summarize.build(self._args(tmp))
        cold = js["scenarios"]["s7"]["frust"]["coldstart"]
        self.assertEqual(cold["source"], "frontboard")
        self.assertEqual(cold["launches"], [557, 300, 320])
        self.assertEqual(cold["kept"], [300, 320])   # first launch discarded, as on Android
        self.assertEqual(cold["median"], 310)
        self.assertIn("| External cold start (FrontBoard bootstrap → scene content ready, median of kept launches) | ~310 ms (300/320; first of 3 discarded) |", md)
        self.assertNotIn("`am start -W`", md)

    def test_android_coldstart_keeps_its_label_and_json_shape(self):
        with tempfile.TemporaryDirectory() as tmp:
            s7 = Path(tmp) / "s7"
            s7.mkdir()
            (s7 / "run-00.log").write_text("")
            (s7 / "coldstart.txt").write_text(ANDROID_COLDSTART)
            md, js = summarize.build(self._args(tmp))
        cold = js["scenarios"]["s7"]["frust"]["coldstart"]
        self.assertNotIn("source", cold)
        self.assertEqual(cold["median"], 141)
        self.assertIn("| External cold start (`am start -W` TotalTime, median of kept launches) | ~141 ms (141; first of 2 discarded) |", md)
        self.assertIn("| Idle CPU during the S7 blocks (`dumpsys cpuinfo`) | n/a |", md)
        self.assertIn("| Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots) | n/a |", md)

    def test_sysmon_files_give_the_instruments_idle_rows(self):
        with tempfile.TemporaryDirectory() as tmp:
            s7 = Path(tmp) / "s7"
            s7.mkdir()
            for i in range(3):
                (s7 / f"run-0{i}.log").write_text("")
            # run-00 is a warm-up without samples; runs 01/02 carry the window.
            (s7 / "run-01.sysmon.txt").write_text(SYSMON_RUN)
            (s7 / "run-02.sysmon.txt").write_text(SYSMON_RUN.replace("44040192", "46137344"))
            md, js = summarize.build(self._args(tmp, discard=1))
        entry = js["scenarios"]["s7"]["frust"]
        mean, lo, hi, n = entry["idle_footprint_mib"]
        self.assertEqual((f"{mean:.1f}", f"{lo:.1f}", f"{hi:.1f}", n), ("43.0", "42.0", "44.0", 2))
        self.assertEqual(entry["idle_cpu"]["source"], "sysmon")
        self.assertEqual(entry["idle_cpu"]["runs"], 2)
        self.assertAlmostEqual(entry["idle_cpu"]["avg_pct"], 5.0, places=6)
        self.assertEqual(entry["idle_cpu"]["samples"], 6)
        self.assertIn("| Idle CPU over each sampled S7 run's recorded window (Instruments sysmon, Δ time on core / Δt) | ~5.0% avg (max 5.0%, 2 sampled runs, 6 samples) |", md)
        self.assertIn("| Idle memory (phys_footprint, mean over the sampled runs' last sample) | ~43.0 MiB (42.0–44.0, 2 sampled runs) |", md)
        self.assertNotIn("TOTAL PSS", md)

    def test_sysmon_discard_skips_the_warmup_runs(self):
        with tempfile.TemporaryDirectory() as tmp:
            s7 = Path(tmp) / "s7"
            s7.mkdir()
            (s7 / "run-00.log").write_text("")
            (s7 / "run-00.sysmon.txt").write_text(SYSMON_RUN)
            mean, lo, hi, n = summarize.sysmon_idle(s7, discard=0)["footprint_mib"]
            self.assertEqual(n, 1)
            self.assertIsNone(summarize.sysmon_idle(s7, discard=1))


if __name__ == "__main__":
    unittest.main()
