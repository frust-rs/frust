#!/usr/bin/env python3
"""benchmarks/harness/summarize.py — RESULTS.md tables from a raw run tree.

Reads the per-scenario raw series `run.sh` captured for BOTH apps and prints
the per-device S1–S8 tables `benchmarks/RESULTS.md` publishes, computed with
`stats.py` (the one shared statistics script) — never by hand. Also emits a
JSON dump of every number so two passes (e.g. a renderer change) can be
diffed by `compare.py` without re-reading Markdown.

    python3 summarize.py --frust <dir> --flutter <dir> [--json out.json]
        [--duration 30] [--s7-duration 60] [--discard-first 2]
        [--frust-label "Frust (profile)"] [--flutter-label "Flutter (profile)"]

`<dir>` holds `s1/ … s8/`, each with `run-NN.log` (plus, on Android,
`run-NN.pss_after.txt`; on iOS S7, `run-NN.sysmon.txt` from `run.sh --sysmon`;
optionally `coldstart.txt` and `cpuinfo.txt` written by `matrix.sh`). A
missing scenario directory is skipped, not faked (RESULTS.md's own rule: no
placeholder numbers).

The S7 table names its instrument per platform. Android: `am start -W`
TotalTime, `dumpsys cpuinfo` samples, `TOTAL PSS` from `dumpsys meminfo`. iOS:
FrontBoard's bootstrap → scene-content-ready span out of the device syslog
(`coldstart.txt`'s `(ios: …)` header marks it), and Instruments `sysmon`
rows — idle CPU is Δ(time on core)/Δt over each sampled run's recorded
window, idle memory the mean over the sampled runs of each run's last
`phys_footprint` sample (every other kept run carries a recording). The
Android rows and labels are unchanged by the iOS additions.

Conventions (PROTOCOL §4/§5): the first `--discard-first` runs are warm-up and
excluded from every statistic; percentiles are `stats.py`'s nearest-rank over
non-skipped frames; the S8 per-op medians exclude each type's first call
(`n=0`, the warm-up call) exactly as every published pass did.
"""
from __future__ import annotations

import argparse
import json
import re
import statistics
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import stats  # noqa: E402  (benchmarks/harness/stats.py)

FRAME_SCENARIOS = ("s1", "s2", "s4", "s5", "s6")
S3_OPS = (
    ("s3-create1k", "create 1k"),
    ("s3-create10k", "create 10k"),
    ("s3-update", "update every 10th of 10k"),
    ("s3-swap", "swap"),
    ("s3-clear", "clear"),
)
# Cycle-health count for the S3 reconcile loop (PROTOCOL §7): tolerates both
# the name-only marker shape every committed series still carries and the
# 2026-09-06 indexed shape (`bench-scenario-start n=<u64> s3-create1k`) a
# future capture emits, same substring looseness as before.
S3_CREATE1K_START_RE = re.compile(r"bench-scenario-start(?: n=\d+)? s3-create1k")
FRUST_PASS_FIELDS = ("rebuild_us", "layout_us", "paint_us", "encode_us", "acquire_us", "submit_us", "present_us")
S8_ROWS = (
    # (label, frust (op,type), flutter crossing (op,type), flutter cached (op,type))
    ("write bool", ("write", "bool"), ("write", "boolT"), None),
    ("write i64", ("write", "i64"), ("write", "intT"), None),
    ("write f64", ("write", "f64"), ("write", "doubleT"), None),
    ("write String", ("write", "string"), ("write", "stringT"), None),
    ("write Vec\\<String\\>", ("write", "string_list"), ("write", "stringListT"), None),
    ("read (unique key, forces channel) bool", ("read", "bool"), ("read_crossing", "reload"), ("read_cached", "boolT")),
    ("read (unique key, forces channel) i64", ("read", "i64"), ("read_crossing", "reload"), ("read_cached", "intT")),
    ("read (unique key, forces channel) f64", ("read", "f64"), ("read_crossing", "reload"), ("read_cached", "doubleT")),
    ("read (unique key, forces channel) String", ("read", "string"), ("read_crossing", "reload"), ("read_cached", "stringT")),
    ("read (unique key, forces channel) Vec\\<String\\>", ("read", "string_list"), ("read_crossing", "reload"), ("read_cached", "stringListT")),
)


def apply_work_conventions(frames: list[stats.FrameRecord], flutter_work_sum: bool) -> list[stats.FrameRecord]:
    """iOS convention (RESULTS.md iPhone deviations 1–2): Flutter's `totalSpan`
    goes negative under load on iOS, so its frame total is the `build_us+raster_us`
    work-sum when `flutter_work_sum` is set. Frust frames are untouched here; the
    separate "work" row (rebuild+layout+paint+encode) is added by `frust_work_frames`."""
    if not flutter_work_sum:
        return frames
    out = []
    for f in frames:
        if f.source == "flutter" and "build_us" in f.fields and "raster_us" in f.fields:
            out.append(stats.FrameRecord(f.source, f.n, int(f.fields["build_us"]) + int(f.fields["raster_us"]), f.skipped, f.fields))
        else:
            out.append(f)
    return out


def frust_work_frames(frames: list[stats.FrameRecord]) -> list[stats.FrameRecord]:
    """Frust per-frame CPU WORK = `total_us` minus the blocking swapchain/vsync
    wait: `total - acquire_us` on v3/v4 lines (rebuild+layout+paint+encode+submit —
    `submit_us` is where the direct-to-surface engine's render lands, PROTOCOL §7's
    v4 note, so it must stay in), `total - present_us` on v2 lines (whose single
    `present_us` folds acquire and submit together). The iPhone tables carry this
    row because `total_us` there is pinned to the CADisplayLink cadence."""
    out = []
    for f in frames:
        wait = f.fields.get("acquire_us", f.fields.get("present_us"))
        if wait is not None:
            out.append(stats.FrameRecord(f.source, f.n, max(f.total_us - int(wait), 0), f.skipped, f.fields))
    return out


def run_logs(scn_dir: Path) -> list[Path]:
    return sorted(scn_dir.glob("run-[0-9][0-9].log"))


def kept_frames(scn_dir: Path, marker: str, discard: int) -> tuple[list[stats.FrameRecord], int]:
    logs = run_logs(scn_dir)
    runs = [stats.load_run_frames(p, marker) for p in logs]
    kept = max(len(runs) - discard, 0)
    return stats.discard_first_runs(runs, discard), kept


def ms(us: int | float) -> str:
    return f"{us / 1000:.2f}"


def pct(n: int, total: int) -> str:
    return f"{n:,} ({100.0 * n / total:.2f}%)" if total else "0"


def median_int(values: list[int]) -> int | None:
    return int(statistics.median(values)) if values else None


def frame_row(label: str, st: stats.ScenarioStats, kept_runs: int, duration: int) -> str:
    fps = st.n_active / (kept_runs * duration) if kept_runs and duration else 0.0
    return (
        f"| {label} | {ms(st.p50_us)} | {ms(st.p95_us)} | {ms(st.p99_us)} | {ms(st.worst_us)} | "
        f"{pct(st.missed_60hz, st.n_active)} | {pct(st.missed_120hz, st.n_active)} | "
        f"{st.n_active:,} (~{fps:.1f} fps avg) |"
    )


FRAME_HEADER = (
    "| App | p50 (ms) | p95 (ms) | p99 (ms) | worst (ms) | missed @16.67ms | missed @8.33ms | active frames |\n"
    "|---|---|---|---|---|---|---|---|"
)


def frust_breakdown(frames: list[stats.FrameRecord]) -> dict:
    """p50 per pass over kept non-skipped frames, layout health, GPU spans."""
    active = [f for f in frames if not f.skipped]
    out: dict = {}
    for name in FRUST_PASS_FIELDS:
        vals = sorted(int(f.fields[name]) for f in active if name in f.fields)
        if vals:
            out[name + "_p50"] = stats._nearest_rank_percentile(vals, 50)
    layout_pos = sum(1 for f in active if int(f.fields.get("layout_us", "0")) > 0)
    out["layout_pos_frames"] = layout_pos
    out["active_frames"] = len(active)
    gpu = [stats.gpu_spans(f) for f in active]
    gpu = [g for g in gpu if g]
    if gpu:
        totals = sorted(g["gpu_total_us"] for g in gpu)
        out["gpu_frames"] = len(gpu)
        out["gpu_total_p50"] = stats._nearest_rank_percentile(totals, 50)
        out["gpu_total_p95"] = stats._nearest_rank_percentile(totals, 95)
    return out


def breakdown_line(bd: dict) -> str:
    parts = []
    for name in FRUST_PASS_FIELDS:
        k = name + "_p50"
        if k in bd:
            parts.append(f"{name.replace('_us', '')} {ms(bd[k])}")
    s = "Frust pass p50 (ms): " + ", ".join(parts) if parts else "Frust pass breakdown: n/a"
    if bd.get("active_frames"):
        s += f"; layout_us>0 on {bd['layout_pos_frames']:,}/{bd['active_frames']:,} frames"
    if "gpu_total_p50" in bd:
        s += f"; GPU (gpu_q=1 on {bd['gpu_frames']:,}) total p50/p95 {ms(bd['gpu_total_p50'])}/{ms(bd['gpu_total_p95'])} ms"
    return s


def total_pss_mib(scn_dir: Path, discard: int) -> tuple[float | None, float | None, float | None, int]:
    """Mean/min/max TOTAL PSS in MiB (KiB/1024 — `dumpsys meminfo`'s unit is
    KiB, never decimal MB) over the kept post-run snapshots, plus the kept
    sample count. Matches the hand-computed control RESULTS.md quotes:
    116,328.5 KiB mean -> 113.6 MiB."""
    snaps = sorted(scn_dir.glob("run-[0-9][0-9].pss_after.txt"))[discard:]
    vals = []
    for p in snaps:
        m = re.search(r"TOTAL PSS:\s+(\d+)", p.read_text(errors="replace"))
        if m:
            vals.append(int(m.group(1)) / 1024.0)
    if not vals:
        return None, None, None, 0
    return statistics.fmean(vals), min(vals), max(vals), len(vals)


def startup_spans(scn_dir: Path) -> dict:
    """Per-launch framework-reported spans over ALL runs (each run is one cold launch)."""
    frust_first, frust_adapter, flutter_first = [], [], []
    for p in run_logs(scn_dir):
        for line in p.read_text(errors="replace").splitlines():
            if "frust-perf startup" in line:
                m = re.search(r"first_frame_presented=(\d+)ms", line)
                if m:
                    frust_first.append(int(m.group(1)))
                m = re.search(r"adapter_ready=(\d+)ms", line)
                if m:
                    frust_adapter.append(int(m.group(1)))
                break
            if "flutter-perf startup" in line:
                m = re.search(r"first_frame_ms=(\d+)", line)
                if m:
                    flutter_first.append(int(m.group(1)))
                break
    out: dict = {}
    if frust_first:
        out["frust_first_frame_ms"] = {"median": median_int(frust_first), "min": min(frust_first), "max": max(frust_first), "n": len(frust_first)}
    if frust_adapter:
        out["frust_adapter_ready_ms"] = {"median": median_int(frust_adapter), "min": min(frust_adapter), "max": max(frust_adapter), "n": len(frust_adapter)}
    if flutter_first:
        out["flutter_first_frame_ms"] = {"median": median_int(flutter_first), "min": min(flutter_first), "max": max(flutter_first), "n": len(flutter_first)}
    return out


COLDSTART_IOS_MARK = "(ios: FrontBoard bootstrap -> scene content ready)"


def coldstart(scn_dir: Path) -> dict | None:
    """External cold-start launches: `am start -W` TotalTime lines on Android,
    or matrix.sh's iOS FrontBoard bootstrap → scene-content-ready deltas (same
    `TotalTime: <ms>` line, header marked by COLDSTART_IOS_MARK). `source`
    says which."""
    f = scn_dir / "coldstart.txt"
    if not f.exists():
        return None
    text = f.read_text(errors="replace")
    vals = [int(m.group(1)) for m in re.finditer(r"TotalTime:\s+(\d+)", text)]
    if len(vals) < 2:
        return None
    kept = vals[1:]  # first launch discarded (matches every published pass)
    out = {"launches": vals, "kept": kept, "median": int(statistics.median(kept))}
    if COLDSTART_IOS_MARK in text:
        out["source"] = "frontboard"  # absent on Android: the committed JSON dumps stay byte-identical
    return out


SYSMON_LINE_RE = re.compile(
    r"^t_s=(?P<t>[0-9.]+) cpu_pct=(?P<pct>[0-9.]+|na) cpu_ns=(?P<cpu>\d+) footprint_bytes=(?P<fp>\d+) rss_bytes=(?P<rss>\d+)$"
)


def sysmon_samples(path: Path) -> list[dict]:
    out = []
    for line in path.read_text(errors="replace").splitlines():
        m = SYSMON_LINE_RE.match(line.strip())
        if m:
            out.append({"t": float(m["t"]), "cpu_ns": int(m["cpu"]), "footprint": int(m["fp"]), "rss": int(m["rss"])})
    return out


def sysmon_idle(scn_dir: Path, discard: int) -> dict | None:
    """iOS S7 idle axes from the kept runs' `run-NN.sysmon.txt` (Instruments
    sysmon rows of the bench pid over each run's recorded window). CPU per run
    is Δ(time on core)/Δt between the first and last sample — the counter, not
    Instruments' smoothed percentage; memory per run is the LAST
    `phys_footprint` sample (the run's post-idle snapshot, the counterpart of
    Android's post-run PSS). `None` when no run carries samples."""
    # Warm-ups are the first `discard` RUNS (by run log), not the first
    # sysmon files: run.sh records no sysmon on the warm-up runs, so slicing
    # the files would drop kept runs instead.
    kept = {p.name[: -len(".log")] for p in run_logs(scn_dir)[discard:]}
    files = [f for f in sorted(scn_dir.glob("run-[0-9][0-9].sysmon.txt")) if f.name[: -len(".sysmon.txt")] in kept]
    cpu_pcts, footprints, n_samples = [], [], 0
    for f in files:
        samples = sysmon_samples(f)
        if not samples:
            continue
        n_samples += len(samples)
        footprints.append(samples[-1]["footprint"] / (1024.0 * 1024.0))
        if len(samples) >= 2 and samples[-1]["t"] > samples[0]["t"]:
            dt_ns = (samples[-1]["t"] - samples[0]["t"]) * 1e9
            cpu_pcts.append(100.0 * (samples[-1]["cpu_ns"] - samples[0]["cpu_ns"]) / dt_ns)
    if not footprints:
        return None
    out = {
        "runs": len(footprints),
        "samples": n_samples,
        "footprint_mib": (statistics.fmean(footprints), min(footprints), max(footprints), len(footprints)),
    }
    if cpu_pcts:
        out["cpu"] = {"runs": len(cpu_pcts), "avg_pct": statistics.fmean(cpu_pcts), "max_pct": max(cpu_pcts)}
    return out


def idle_cpu(scn_dir: Path, pkg_hint: str) -> dict | None:
    f = scn_dir / "cpuinfo.txt"
    if not f.exists():
        return None
    vals = []
    for line in f.read_text(errors="replace").splitlines():
        if pkg_hint in line:
            m = re.match(r"\s*([\d.]+)%", line)
            if m:
                vals.append(float(m.group(1)))
    if not vals:
        return {"samples": 0}
    return {"samples": len(vals), "avg_pct": statistics.fmean(vals), "max_pct": max(vals)}


def s8_medians(scn_dir: Path, discard: int) -> tuple[dict[tuple[str, str], dict], int]:
    """Median µs per (op,type) over kept runs, excluding each type's n=0 warm-up call."""
    logs = run_logs(scn_dir)[discard:]
    samples: dict[tuple[str, str], list[int]] = {}
    err_markers = 0
    for p in logs:
        for line in p.read_text(errors="replace").splitlines():
            if "s8-errors" in line:
                err_markers += 1
            rec = stats.parse_op_line(line)
            if rec is None or rec.n == 0:
                continue
            t = rec.fields.get("type", "")
            if t == "total":
                continue
            samples.setdefault((rec.op, t), []).append(rec.us)
    out = {k: {"median": median_int(v), "n": len(v)} for k, v in samples.items()}
    return out, err_markers


def build(args: argparse.Namespace) -> tuple[str, dict]:
    frust = Path(args.frust) if args.frust else None
    flutter = Path(args.flutter) if args.flutter else None
    D = args.discard_first
    md: list[str] = []
    js: dict = {"discard_first": D, "duration": args.duration, "s7_duration": args.s7_duration, "scenarios": {}}

    def have(side: Path | None, s: str) -> bool:
        return bool(side) and (side / s).is_dir() and bool(run_logs(side / s))

    # --- S1/S2/S4/S5/S6 ---
    for s in FRAME_SCENARIOS:
        if not (have(frust, s) or have(flutter, s)):
            continue
        md.append(f"### {s.upper()}\n")
        md.append(FRAME_HEADER)
        entry: dict = {}
        bd_line = None
        for label, side, key in ((args.frust_label, frust, "frust"), (args.flutter_label, flutter, "flutter")):
            if not have(side, s):
                continue
            frames, kept = kept_frames(side / s, s, D)
            frames = apply_work_conventions(frames, args.flutter_work_sum)
            st = stats.compute_stats(frames)
            md.append(frame_row(label + (" †" if (key == "frust" and args.frust_work_row) else ""), st, kept, args.duration))
            entry[key] = {**st.as_dict(), "kept_runs": kept, "fps": st.n_active / (kept * args.duration) if kept else 0.0}
            if key == "frust":
                if args.frust_work_row:
                    wst = stats.compute_stats(frust_work_frames(frames))
                    md.append(frame_row("Frust CPU work (total − acquire wait)", wst, kept, args.duration))
                    entry[key]["work"] = wst.as_dict()
                bd = frust_breakdown(frames)
                entry[key]["breakdown"] = bd
                bd_line = breakdown_line(bd)
        if bd_line:
            md.append("\n" + bd_line)
        md.append("")
        js["scenarios"][s] = entry

    # --- S3 ---
    if have(frust, "s3") or have(flutter, "s3"):
        md.append("### S3\n")
        md.append("Per-op reconcile-frame timing (`s3-*` sub-markers aggregated across all cycles in the kept runs):\n")
        md.append("| Op | Frust p50/p95/p99/worst (ms) | n frames | Flutter p50/p95/p99/worst (ms) | n frames |")
        md.append("|---|---|---|---|---|")
        entry = {"ops": {}}
        for marker, label in S3_OPS:
            cells = []
            entry["ops"][marker] = {}
            for side, key in ((frust, "frust"), (flutter, "flutter")):
                if have(side, "s3"):
                    frames, kept = kept_frames(side / "s3", marker, D)
                    frames = apply_work_conventions(frames, args.flutter_work_sum)
                    st = stats.compute_stats(frames)
                    entry["ops"][marker][key] = st.as_dict()
                    if st.n_active:
                        cells.append(f"{ms(st.p50_us)} / {ms(st.p95_us)} / {ms(st.p99_us)} / {ms(st.worst_us)} | {st.n_active}")
                    else:
                        cells.append("**not captured** | 0")
                else:
                    cells.append("— | —")
            md.append(f"| {label} | {cells[0]} | {cells[1]} |")
        # overall + cycle counts
        md.append("")
        md.append("Overall S3 frame series (whole capture incl. settle gaps — continuous vs event-driven paint, not apples-to-apples):\n")
        md.append(FRAME_HEADER)
        notes = []
        for label, side, key in ((args.frust_label, frust, "frust"), (args.flutter_label, flutter, "flutter")):
            if not have(side, "s3"):
                continue
            frames, kept = kept_frames(side / "s3", "s3", D)
            frames = apply_work_conventions(frames, args.flutter_work_sum)
            st = stats.compute_stats(frames)
            md.append(frame_row(label, st, kept, args.duration))
            entry[key] = {**st.as_dict(), "kept_runs": kept}
            cycles = []
            for p in run_logs(side / "s3")[D:]:
                cycles.append(sum(1 for l in p.read_text(errors="replace").splitlines() if S3_CREATE1K_START_RE.search(l)))
            entry[key]["create1k_cycles_per_kept_run"] = cycles
            if cycles:
                notes.append(f"{label} `s3-create1k` reopens per kept run: {min(cycles)}–{max(cycles)}")
        if notes:
            md.append("\nCycle health — " + "; ".join(notes) + ".")
        md.append("")
        js["scenarios"]["s3"] = entry

    # --- S7 ---
    if have(frust, "s7") or have(flutter, "s7"):
        md.append("### S7\n")
        md.append("| Metric | " + args.frust_label + " | " + args.flutter_label + " |")
        md.append("|---|---|---|")
        entry = {}
        cold = {}
        spans = {}
        pss = {}
        cpu = {}
        sysmon = {}
        for side, key, pkg in ((frust, "frust", "frustbench"), (flutter, "flutter", "flutter_bench")):
            if not have(side, "s7"):
                continue
            spans[key] = startup_spans(side / "s7")
            cold[key] = coldstart(side / "s7")
            pss[key] = total_pss_mib(side / "s7", D)
            cpu[key] = idle_cpu(side / "s7", pkg)
            sysmon[key] = sysmon_idle(side / "s7", D)
            entry[key] = {"startup": spans[key], "coldstart": cold[key], "idle_pss_mib": pss[key], "idle_cpu": cpu[key]}
            if sysmon[key]:
                entry[key]["idle_footprint_mib"] = sysmon[key]["footprint_mib"]
                entry[key]["idle_cpu"] = {"source": "sysmon", "samples": sysmon[key]["samples"], **sysmon[key].get("cpu", {})}
        # The row labels name the instrument: iOS trees (FrontBoard cold start,
        # Instruments sysmon) relabel; an Android tree keeps its labels verbatim.
        ios_cold = any(c and c.get("source") == "frontboard" for c in cold.values())
        ios_idle = any(sysmon.values())

        def cell_cold(k):
            c = cold.get(k)
            return f"~{c['median']} ms ({'/'.join(str(v) for v in c['kept'])}; first of {len(c['launches'])} discarded)" if c else "not captured"

        def cell_span(k):
            sp = spans.get(k, {})
            f = sp.get(f"{k}_first_frame_ms")
            if not f:
                return "not captured"
            name = "`first_frame_presented`" if k == "frust" else "`first_frame_ms`"
            extra = ""
            a = sp.get("frust_adapter_ready_ms")
            if k == "frust" and a:
                extra = f"; `adapter_ready` median {a['median']} ms"
            return f"~{f['median']} ms median ({name}, {f['min']}–{f['max']} across {f['n']} launches{extra})"

        def cell_pss(k):
            if ios_idle:
                sm = sysmon.get(k)
                if not sm:
                    return "n/a"
                fp = sm["footprint_mib"]
                return f"~{fp[0]:.1f} MiB ({fp[1]:.1f}–{fp[2]:.1f}, {fp[3]} sampled runs)"
            p = pss.get(k)
            return f"~{p[0]:.1f} MiB ({p[1]:.1f}–{p[2]:.1f}, {p[3]} kept snapshots)" if p and p[0] is not None else "n/a"

        def cell_cpu(k):
            if ios_idle:
                sm = sysmon.get(k)
                if not sm or "cpu" not in sm:
                    return "n/a"
                c = sm["cpu"]
                return f"~{c['avg_pct']:.1f}% avg (max {c['max_pct']:.1f}%, {c['runs']} sampled runs, {sm['samples']} samples)"
            c = cpu.get(k)
            if not c:
                return "n/a"
            if not c.get("samples"):
                return "0 samples"
            return f"~{c['avg_pct']:.1f}% avg (max {c['max_pct']:.1f}%, {c['samples']} samples)"

        cold_label = "External cold start (FrontBoard bootstrap → scene content ready, median of kept launches)" if ios_cold else "External cold start (`am start -W` TotalTime, median of kept launches)"
        cpu_label = "Idle CPU over each sampled S7 run's recorded window (Instruments sysmon, Δ time on core / Δt)" if ios_idle else "Idle CPU during the S7 blocks (`dumpsys cpuinfo`)"
        mem_label = "Idle memory (phys_footprint, mean over the sampled runs' last sample)" if ios_idle else "Idle memory (TOTAL PSS, mean over kept runs' post-run snapshots)"
        md.append(f"| {cold_label} | {cell_cold('frust')} | {cell_cold('flutter')} |")
        md.append(f"| Framework-reported first-frame span | {cell_span('frust')} | {cell_span('flutter')} |")
        md.append(f"| {cpu_label} | {cell_cpu('frust')} | {cell_cpu('flutter')} |")
        md.append(f"| {mem_label} | {cell_pss('frust')} | {cell_pss('flutter')} |")
        md.append("")
        js["scenarios"]["s7"] = entry

    # --- S8 ---
    if have(frust, "s8") or have(flutter, "s8"):
        md.append("### S8\n")
        md.append("Per-op median latency over the kept runs (µs/call; each type's first call excluded as warm-up).\n")
        md.append(f"| Op | {args.frust_label} (µs/call) | Flutter, channel-crossing (µs/call) | Flutter, cached-read (µs/call) |")
        md.append("|---|---|---|---|")
        entry = {}
        fr, fr_err = s8_medians(frust / "s8", D) if have(frust, "s8") else ({}, 0)
        fl, fl_err = s8_medians(flutter / "s8", D) if have(flutter, "s8") else ({}, 0)
        entry["frust"] = {f"{k[0]}:{k[1]}": v for k, v in fr.items()}
        entry["flutter"] = {f"{k[0]}:{k[1]}": v for k, v in fl.items()}
        entry["s8_error_markers"] = {"frust": fr_err, "flutter": fl_err}
        for label, fk, xk, ck in S8_ROWS:
            a = fr.get(fk)
            b = fl.get(xk)
            c = fl.get(ck) if ck else None
            md.append(
                f"| {label} | {a['median'] if a else 'n/a'} | {str(b['median']) + ('†' if xk[1] == 'reload' else '') if b else 'n/a'} | "
                f"{('~' + str(c['median'])) if c else 'n/a'} |"
            )
        md.append("")
        md.append(f"`s8-errors` marker lines across kept logs: frust {fr_err}, flutter {fl_err}. "
                  "†Flutter's only channel-crossing read is the package's `reload()` whole-store re-read (one figure for every read row).")
        md.append("")
        js["scenarios"]["s8"] = entry

    return "\n".join(md), js


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--frust", help="directory with s1..s8 run dirs for the frust app")
    ap.add_argument("--flutter", help="directory with s1..s8 run dirs for the flutter app")
    ap.add_argument("--duration", type=int, default=30, help="capture seconds per run for fps (default 30)")
    ap.add_argument("--s7-duration", type=int, default=60, help="(unused for fps; recorded in JSON)")
    ap.add_argument("--discard-first", type=int, default=stats.DEFAULT_DISCARD_FIRST)
    ap.add_argument("--frust-label", default="Frust (profile)")
    ap.add_argument("--flutter-label", default="Flutter (profile)")
    ap.add_argument("--flutter-work-sum", action="store_true", help="iOS: use build_us+raster_us as Flutter's frame total (totalSpan is negative under load on iOS)")
    ap.add_argument("--frust-work-row", action="store_true", help="iOS: add a Frust CPU-work row (total minus the acquire/vsync wait) under each frame table; † marks the total row as folding in that wait")
    ap.add_argument("--json", help="write every computed number here")
    args = ap.parse_args(argv)
    if not args.frust and not args.flutter:
        ap.error("at least one of --frust/--flutter is required")
    md, js = build(args)
    print(md)
    if args.json:
        Path(args.json).write_text(json.dumps(js, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
