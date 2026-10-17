#!/usr/bin/env python3
"""benchmarks/harness/test_sysmon_export.py — plain-`unittest` tests for
`sysmon_export.py`: the id/ref cell dedup of an `xctrace export` document is
resolved (including a `pid` defined inside an earlier row's `process` cell),
sentinel cells become `na` / a skipped row, and only the requested pid's rows
come out in the whitelisted line shape.

Runnable via `python3 -m unittest benchmarks.harness.test_sysmon_export` from
the repo root, or `python3 benchmarks/harness/test_sysmon_export.py [-v]`.
"""

from __future__ import annotations

import re
import unittest

try:
    import sysmon_export
except ImportError:  # running as `python3 -m unittest benchmarks.harness.test_sysmon_export`
    from . import sysmon_export

# The column order of a real Xcode 27 export, trimmed to the columns the
# exporter reads plus two it must step over (context-switch, proc-status).
SCHEMA = (
    '<schema name="sysmon-process">'
    '<col><mnemonic>time</mnemonic></col>'
    '<col><mnemonic>process</mnemonic></col>'
    '<col><mnemonic>context-switch</mnemonic></col>'
    '<col><mnemonic>cpu-percent</mnemonic></col>'
    '<col><mnemonic>cpu-total-system</mnemonic></col>'
    '<col><mnemonic>cpu-total-user</mnemonic></col>'
    '<col><mnemonic>memory-physical-footprint</mnemonic></col>'
    '<col><mnemonic>memory-resident-size</mnemonic></col>'
    '<col><mnemonic>pid</mnemonic></col>'
    '<col><mnemonic>proc-status</mnemonic></col>'
    "</schema>"
)

# Row 1: the kernel. Row 2: the bench app's first sample — cpu-percent is a
# sentinel (no interval yet), its pid is defined INSIDE the process cell and
# referenced from the pid column. Row 3: a later bench sample that refs the
# earlier process/pid definitions and carries a real cpu-percent. Row 4:
# another process whose cells ref the bench app's values by id — must be
# excluded by pid even though its numbers look identical.
ROWS = (
    "<row>"
    '<start-time id="1" fmt="00:00.084">84607125</start-time>'
    '<process id="2" fmt="kernel (0)"><pid id="3" fmt="0">0</pid></process>'
    '<event-count id="4">15</event-count>'
    "<sentinel/>"
    "<sentinel/>"
    '<duration-on-core id="5">740107559458</duration-on-core>'
    '<size-in-bytes id="6">25653248</size-in-bytes>'
    '<size-in-bytes id="7">103776256</size-in-bytes>'
    '<pid ref="3"/>'
    '<process-status-name id="8">Runnable</process-status-name>'
    "</row>"
    "<row>"
    '<start-time ref="1"/>'
    '<process id="9" fmt="Runner (2191)"><pid id="10" fmt="2191">2191</pid></process>'
    '<event-count id="11">3</event-count>'
    "<sentinel/>"
    "<sentinel/>"
    '<duration-on-core id="12">432230458</duration-on-core>'
    '<size-in-bytes id="13">41239672</size-in-bytes>'
    '<size-in-bytes id="14">47071232</size-in-bytes>'
    '<pid ref="10"/>'
    '<process-status-name ref="8"/>'
    "</row>"
    "<row>"
    '<start-time id="15" fmt="00:09.562">9562767333</start-time>'
    '<process ref="9"/>'
    '<event-count id="16">9</event-count>'
    '<system-cpu-percent id="17" fmt="2.7%">2.719036560</system-cpu-percent>'
    "<sentinel/>"
    '<duration-on-core id="18">681366791</duration-on-core>'
    '<size-in-bytes id="19">44254328</size-in-bytes>'
    '<size-in-bytes id="20">41222144</size-in-bytes>'
    '<pid ref="10"/>'
    '<process-status-name ref="8"/>'
    "</row>"
    "<row>"
    '<start-time ref="15"/>'
    '<process id="21" fmt="other (77)"><pid id="22" fmt="77">77</pid></process>'
    '<event-count ref="16"/>'
    '<system-cpu-percent ref="17"/>'
    '<duration-on-core id="23">1000</duration-on-core>'
    '<duration-on-core ref="18"/>'
    '<size-in-bytes ref="19"/>'
    '<size-in-bytes ref="20"/>'
    '<pid ref="22"/>'
    '<process-status-name ref="8"/>'
    "</row>"
)
DOC = f'<?xml version="1.0"?><trace-query-result><node xpath="x">{SCHEMA}{ROWS}</node></trace-query-result>'
LINE_RE = re.compile(r"^t_s=[0-9]+\.[0-9]{3} cpu_pct=([0-9]+\.[0-9]{2}|na) cpu_ns=[0-9]+ footprint_bytes=[0-9]+ rss_bytes=[0-9]+$")


class RowsForPidTests(unittest.TestCase):
    def test_bench_rows_only_in_whitelisted_shape(self):
        lines = sysmon_export.rows_for_pid(DOC, 2191)
        self.assertEqual(len(lines), 2)
        for line in lines:
            self.assertRegex(line, LINE_RE)
        self.assertEqual(lines[0], "t_s=0.085 cpu_pct=na cpu_ns=432230458 footprint_bytes=41239672 rss_bytes=47071232")
        self.assertEqual(lines[1], "t_s=9.563 cpu_pct=2.72 cpu_ns=681366791 footprint_bytes=44254328 rss_bytes=41222144")

    def test_system_time_is_added_when_the_sampler_reports_it(self):
        # pid 77's row carries a real cpu-total-system (1000 ns) beside the
        # user counter it refs from the bench row.
        lines = sysmon_export.rows_for_pid(DOC, 77)
        self.assertEqual(lines, ["t_s=9.563 cpu_pct=2.72 cpu_ns=681367791 footprint_bytes=44254328 rss_bytes=41222144"])

    def test_unknown_pid_yields_nothing(self):
        self.assertEqual(sysmon_export.rows_for_pid(DOC, 4242), [])

    def test_missing_column_is_an_error_not_a_guess(self):
        doc = DOC.replace("<col><mnemonic>memory-physical-footprint</mnemonic></col>", "<col><mnemonic>memory-other</mnemonic></col>")
        with self.assertRaises(ValueError):
            sysmon_export.rows_for_pid(doc, 2191)


if __name__ == "__main__":
    unittest.main()
