#!/usr/bin/env python3
"""benchmarks/harness/sysmon_export.py — one process's rows out of an
Instruments `sysmon-process` table export, as whitelisted text lines.

The iOS S7 idle window has no `dumpsys` equivalent; `run.sh --sysmon` records
`xcrun xctrace record --template 'Activity Monitor' --all-processes` over the
run and exports its `sysmon-process` table:

    xcrun xctrace export --input run.trace \
        --xpath '/trace-toc/run[@number="1"]/data/table[@schema="sysmon-process"]'

That XML lists every process on the device, so it never persists. This script
keeps the bench process's rows only (by pid) and prints them in the one shape
`benchmarks/.gitignore`'s whitelist admits for `run-NN.sysmon.txt`:

    t_s=<seconds since trace start> cpu_pct=<Instruments % CPU, or na>
    cpu_ns=<cumulative time on core> footprint_bytes=<physical footprint>
    rss_bytes=<resident size>

`cpu_ns` is the `cpu-total-user` counter plus `cpu-total-system` when the
sampler reports one (on iOS it is a sentinel: the user column already carries
the whole on-core time). `cpu_pct` is `na` on the first sample, which has no
interval to average over. `summarize.py` derives the idle rows from these:
memory from the last footprint sample of each kept run, CPU from Δcpu_ns/Δt
over the idle window — the counter, not the smoothed percentage, which is
printed for the record.

    python3 sysmon_export.py <sysmon.xml> --pid <pid> [--out <file>]

Exit status 3 when the pid has no rows (the trace missed the process), so the
caller can log the gap instead of staging an empty file.
"""
from __future__ import annotations

import argparse
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

REQUIRED = ("time", "pid", "cpu-percent", "cpu-total-user", "cpu-total-system", "memory-physical-footprint", "memory-resident-size")


class _Resolver:
    """xctrace's export dedups repeated cells: the first occurrence carries an
    `id`, later ones are `<tag ref="id"/>`, and a definition may sit inside
    another cell (the `pid` under a `process`). Walk every cell in document
    order so a ref always finds its definition."""

    def __init__(self) -> None:
        self.by_id: dict[str, ET.Element] = {}

    def resolve(self, el: ET.Element) -> ET.Element:
        ref = el.get("ref")
        if ref is not None:
            return self.by_id[ref]
        el_id = el.get("id")
        if el_id is not None:
            self.by_id[el_id] = el
        for sub in el:
            self.resolve(sub)
        return el


def _value(el: ET.Element) -> str | None:
    """A cell's raw text; `None` for a `<sentinel/>` (no reading this tick)."""
    if el.tag == "sentinel":
        return None
    text = (el.text or "").strip()
    return text or None


def format_line(t_ns: int, cpu_pct: float | None, cpu_ns: int, footprint: int, rss: int) -> str:
    pct = "na" if cpu_pct is None else f"{cpu_pct:.2f}"
    return f"t_s={t_ns / 1e9:.3f} cpu_pct={pct} cpu_ns={cpu_ns} footprint_bytes={footprint} rss_bytes={rss}"


def rows_for_pid(xml_text: str, pid: int) -> list[str]:
    root = ET.fromstring(xml_text)
    schema = root.find(".//schema")
    if schema is None:
        raise ValueError("no <schema> in the export — not a sysmon-process table")
    cols = [c.findtext("mnemonic") for c in schema.findall("col")]
    missing = [c for c in REQUIRED if c not in cols]
    if missing:
        raise ValueError(f"sysmon-process export lacks columns {missing}")
    resolver = _Resolver()
    out: list[str] = []
    for row in root.iter("row"):
        cells = [resolver.resolve(c) for c in row]
        rec = dict(zip(cols, cells))
        row_pid = _value(rec["pid"])
        if row_pid is None or int(row_pid) != pid:
            continue
        t = _value(rec["time"])
        user = _value(rec["cpu-total-user"])
        system = _value(rec["cpu-total-system"])
        footprint = _value(rec["memory-physical-footprint"])
        rss = _value(rec["memory-resident-size"])
        if t is None or user is None or footprint is None or rss is None:
            continue
        pct = _value(rec["cpu-percent"])
        out.append(
            format_line(
                int(t),
                None if pct is None else float(pct),
                int(user) + (int(system) if system is not None else 0),
                int(footprint),
                int(rss),
            )
        )
    return out


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("xml", help="xctrace export of the sysmon-process table")
    ap.add_argument("--pid", type=int, required=True, help="the bench process's pid on the device")
    ap.add_argument("--out", help="write the lines here instead of stdout")
    args = ap.parse_args(argv)
    lines = rows_for_pid(Path(args.xml).read_text(errors="replace"), args.pid)
    if not lines:
        print(f"sysmon_export: no rows for pid {args.pid}", file=sys.stderr)
        return 3
    text = "\n".join(lines) + "\n"
    if args.out:
        Path(args.out).write_text(text)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
