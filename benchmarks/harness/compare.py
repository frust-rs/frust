#!/usr/bin/env python3
"""benchmarks/harness/compare.py — diff two `summarize.py --json` dumps.

    python3 compare.py --old <old.json> --new <new.json> [--old-label L] [--new-label L]
        [--side frust|flutter] [--flag-pct 10]

Prints, per scenario present in BOTH dumps, the old/new p50/p95/p99/worst,
60 Hz miss rate and achieved fps for the chosen side, with the percent
change (positive = slower/worse), then a verdict list: any scenario whose
p50 or p95 moved past `--flag-pct` in either direction. S3 compares the
per-op sub-markers, S7 the framework first-frame span / idle PSS / external
cold start, S8 the per-op medians. Numbers only — it never edits a results
file. Used for the renderer-transition (vello → frust-engine) check.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path


def _fps(d):
    """Achieved fps as text; 'n/a' when the series records none (S3's overall
    series is event-driven on one side and has no fps entry in the JSON)."""
    v = d.get('fps')
    return f"{v:.1f}" if isinstance(v, (int, float)) else 'n/a'

FRAME = ("s1", "s2", "s4", "s5", "s6")


def pct(old: float | None, new: float | None) -> str:
    if not old or new is None:
        return "n/a"
    return f"{100.0 * (new - old) / old:+.1f}%"


def ms(us: int | None) -> str:
    return f"{us / 1000:.2f}" if us is not None else "n/a"


def rate(d: dict) -> float | None:
    return 100.0 * d["missed_60hz"] / d["n_active"] if d.get("n_active") else None


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--old", required=True)
    ap.add_argument("--new", required=True)
    ap.add_argument("--old-label", default="old")
    ap.add_argument("--new-label", default="new")
    ap.add_argument("--side", default="frust", choices=("frust", "flutter"))
    ap.add_argument("--flag-pct", type=float, default=10.0)
    ap.add_argument("--work", action="store_true", help="compare the frust WORK row (iOS) instead of the total row")
    a = ap.parse_args(argv)
    old = json.loads(Path(a.old).read_text())["scenarios"]
    new = json.loads(Path(a.new).read_text())["scenarios"]
    side = a.side
    flags: list[str] = []
    out: list[str] = []
    out.append(f"Side: **{side}** — {a.old_label} → {a.new_label} (Δ positive = slower / worse)\n")
    out.append("| Scenario | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | p99 old→new | worst old→new | miss@16.67 old→new | fps old→new |")
    out.append("|---|---|---|---|---|---|---|---|---|")
    for s in FRAME + ("s3",):
        o = old.get(s, {}).get(side)
        n = new.get(s, {}).get(side)
        if not o or not n:
            continue
        if a.work and side == "frust":
            o = o.get("work", o); n = n.get("work", n)
        d50, d95 = pct(o["p50_us"], n["p50_us"]), pct(o["p95_us"], n["p95_us"])
        ro, rn = rate(o), rate(n)
        label = s.upper() + (" (overall series)" if s == "s3" else "")
        out.append(
            f"| {label} | {ms(o['p50_us'])}→{ms(n['p50_us'])} | {d50} | {ms(o['p95_us'])}→{ms(n['p95_us'])} | {d95} | "
            f"{ms(o['p99_us'])}→{ms(n['p99_us'])} | {ms(o['worst_us'])}→{ms(n['worst_us'])} | "
            f"{ro:.2f}%→{rn:.2f}% | {_fps(o)}→{_fps(n)} |" if ro is not None and rn is not None else
            f"| {label} | {ms(o['p50_us'])}→{ms(n['p50_us'])} | {d50} | {ms(o['p95_us'])}→{ms(n['p95_us'])} | {d95} | {ms(o['p99_us'])}→{ms(n['p99_us'])} | {ms(o['worst_us'])}→{ms(n['worst_us'])} | n/a | n/a |"
        )
        for name, dv in (("p50", d50), ("p95", d95)):
            if dv != "n/a" and abs(float(dv.rstrip("%"))) > a.flag_pct:
                flags.append(f"{label} {name} {dv} ({ms(o[name + '_us'])}→{ms(n[name + '_us'])} ms)")
    # S3 per-op
    o3, n3 = old.get("s3", {}).get("ops", {}), new.get("s3", {}).get("ops", {})
    rows = []
    for op in o3:
        oo, nn = o3[op].get(side), n3.get(op, {}).get(side)
        if oo and nn and oo.get("n_active") and nn.get("n_active"):
            d50 = pct(oo["p50_us"], nn["p50_us"]); d95 = pct(oo["p95_us"], nn["p95_us"])
            rows.append(f"| {op} | {ms(oo['p50_us'])}→{ms(nn['p50_us'])} | {d50} | {ms(oo['p95_us'])}→{ms(nn['p95_us'])} | {d95} | {oo['n_active']}→{nn['n_active']} |")
            for name, dv in (("p50", d50), ("p95", d95)):
                if abs(float(dv.rstrip("%"))) > a.flag_pct:
                    flags.append(f"S3 {op} {name} {dv}")
    if rows:
        out.append("\nS3 per-op (reconcile frames):\n")
        out.append("| Op | p50 old→new (ms) | Δp50 | p95 old→new | Δp95 | n frames |")
        out.append("|---|---|---|---|---|---|")
        out.extend(rows)
    # S7
    o7, n7 = old.get("s7", {}).get(side), new.get("s7", {}).get(side)
    if o7 and n7:
        out.append("\nS7:\n")
        out.append("| Metric | old | new | Δ |")
        out.append("|---|---|---|---|")
        k = f"{side}_first_frame_ms"
        fo, fn = o7.get("startup", {}).get(k), n7.get("startup", {}).get(k)
        if fo and fn:
            out.append(f"| first-frame span median (ms) | {fo['median']} | {fn['median']} | {pct(fo['median'], fn['median'])} |")
            if abs(float(pct(fo['median'], fn['median']).rstrip('%'))) > a.flag_pct:
                flags.append(f"S7 first-frame span {pct(fo['median'], fn['median'])} ({fo['median']}→{fn['median']} ms)")
        po, pn = o7.get("idle_pss_mib"), n7.get("idle_pss_mib")
        if po and pn and po[0] and pn[0]:
            out.append(f"| idle TOTAL PSS mean (MiB) | {po[0]:.1f} | {pn[0]:.1f} | {pct(po[0], pn[0])} |")
            if abs(float(pct(po[0], pn[0]).rstrip('%'))) > a.flag_pct:
                flags.append(f"S7 idle PSS {pct(po[0], pn[0])} ({po[0]:.1f}→{pn[0]:.1f} MiB)")
        co, cn = o7.get("coldstart"), n7.get("coldstart")
        if co and cn:
            out.append(f"| external cold start median (ms) | {co['median']} | {cn['median']} | {pct(co['median'], cn['median'])} |")
    # S8
    o8, n8 = old.get("s8", {}).get(side, {}), new.get("s8", {}).get(side, {})
    rows = []
    for k in sorted(o8):
        if k in n8 and o8[k].get("median") and n8[k].get("median"):
            d = pct(o8[k]["median"], n8[k]["median"])
            rows.append(f"| {k} | {o8[k]['median']} | {n8[k]['median']} | {d} |")
            if abs(float(d.rstrip('%'))) > max(a.flag_pct, 25.0) and k.startswith("write"):
                flags.append(f"S8 {k} {d} ({o8[k]['median']}→{n8[k]['median']} µs)")
    if rows:
        out.append("\nS8 per-op medians (µs/call):\n")
        out.append("| op:type | old | new | Δ |")
        out.append("|---|---|---|---|")
        out.extend(rows)
    out.append("")
    if flags:
        out.append(f"**Moved past ±{a.flag_pct:g}% (inspect):**")
        out.extend(f"- {f}" for f in flags)
    else:
        out.append(f"**No metric moved past ±{a.flag_pct:g}%.**")
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
