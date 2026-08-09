#!/usr/bin/env python3
"""Generate the terminal-grid replay fixtures.

The playground's Terminal section (`src/pages/terminal.rs`) replays these bytes
through a real VT emulator. The content is generated once, here, and checked in
as DATA rather than produced at runtime: a replay whose content differed run to
run could not be compared against itself, and the escape-sequence subset below
is chosen so any emulator interprets every byte the same way (see `README.md`).

Output per profile:

  <profile>.chunks  binary, a flat sequence of length-prefixed chunks:
                        repeat: [u32 little-endian length][length bytes]
                    no header, no trailer, no padding.
  <profile>.json    manifest: profile, cols, rows, hz, chunk_count,
                    duration_ms, sha256 (of the .chunks file).

Replay contract: emit chunk i at t = i * (1000 / hz) ms from replay start.

DETERMINISM is the entire point. Every random draw comes from an explicitly
seeded `random.Random` — never the module-level functions, never time, never
anything environment-dependent. Regenerating on any machine must produce
byte-identical output.

Usage:  python3 gen_terminal_fixtures.py [--out DIR] [--verify]
        --verify regenerates into a temp dir and asserts the bytes match what
        is already on disk.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import struct
import sys
import tempfile

# ── Fixed geometry and timing ────────────────────────────────────────────────

COLS = 80
ROWS = 45
DURATION_MS = 30_000
SEED = 0x5E97E12  # arbitrary but FIXED; changing it changes every fixture byte

# ── Escape-sequence subset ───────────────────────────────────────────────────
#
# Deliberately conservative so that any emulator — the `vt100` Rust crate the
# playground page drives, or a full one it might ever be compared against —
# interprets every byte identically. A divergence in how two emulators handle
# an exotic sequence would silently corrupt such a comparison rather than
# failing loudly.
#
# Used:      ESC[H, ESC[2J, ESC[K, ESC[<n>m (0/1/30-37/40-47), \r\n
# Avoided:   alternate screen, scroll regions, 256-color, truecolor, mouse
#            reporting, OSC, Kitty graphics/keyboard/color-stack.

ESC = "\x1b"
HOME = f"{ESC}[H"
EL = f"{ESC}[K"
SGR_RESET = f"{ESC}[0m"

FG = [30, 31, 32, 33, 34, 35, 36, 37]
BG = [40, 41, 42, 43, 44, 45, 46, 47]

WORDS = [
    "compiling", "linking", "resolving", "fetching", "building", "checking",
    "warning", "note", "expected", "found", "module", "crate", "target",
    "release", "profile", "gradle", "kotlin", "cargo", "vello", "wgpu",
    "surface", "texture", "pipeline", "shader", "glyph", "layout", "paint",
]
LEVELS = [("INFO", 32), ("WARN", 33), ("ERROR", 31), ("DEBUG", 36)]


# ── Chunk writer ─────────────────────────────────────────────────────────────

class ChunkFile:
    """Accumulates length-prefixed chunks."""

    def __init__(self) -> None:
        self.buf = bytearray()
        self.count = 0

    def add(self, s: str) -> None:
        b = s.encode("utf-8")
        self.buf += struct.pack("<I", len(b))
        self.buf += b
        self.count += 1

    def bytes(self) -> bytes:
        return bytes(self.buf)


# ── Profile generators ───────────────────────────────────────────────────────
#
# Rate and style-density are varied INDEPENDENTLY on purpose. Research finding:
# frust's per-frame GPU cost scales with n_draw_objects (i.e. the number of
# style runs), not with surface resolution. So `htop` isolates density at a
# modest rate, and `firehose` isolates rate at a modest density. Varying both
# at once would make an unfavourable result impossible to attribute.

def gen_idle(_rng, _n):
    """No output at all. Measures rest cost — the frame gate should idle."""
    return ChunkFile()


def gen_typing(rng, n):
    """One printable char at the cursor per chunk; occasional newline.

    The smallest possible update: a single cell changes.
    """
    cf = ChunkFile()
    col = 0
    for _ in range(n):
        if col >= COLS - 1 or rng.random() < 0.04:
            cf.add("\r\n")
            col = 0
        else:
            cf.add(chr(rng.randint(0x21, 0x7E)))
            col += 1
    return cf


def gen_build_log(rng, n):
    """One scrolling log line per chunk; a few style runs per line.

    The realistic-workload profile and the primary verdict input.
    """
    cf = ChunkFile()
    for i in range(n):
        label, color = LEVELS[rng.randrange(len(LEVELS))]
        words = " ".join(rng.choice(WORDS) for _ in range(rng.randint(4, 9)))
        line = (
            f"{ESC}[{color}m{label:<5}{SGR_RESET} "
            f"{ESC}[1m[{i:05d}]{SGR_RESET} {words}"
        )
        cf.add(line[: COLS - 1] + EL + "\r\n")
    return cf


def gen_htop(rng, n):
    """Full-screen redraw with dense per-cell colour churn.

    Colour changes every RUN_LEN cells, giving ~(COLS/RUN_LEN) style runs per
    row and ~ROWS*COLS/RUN_LEN per frame — the density axis. RUN_LEN=2 keeps
    the fixture a manageable size while still producing ~1800 style runs per
    screen, far beyond anything the existing widget set paints.
    """
    run_len = 2
    cf = ChunkFile()
    for _ in range(n):
        parts = [HOME]
        for _row in range(ROWS):
            col = 0
            while col < COLS - 1:
                take = min(run_len, COLS - 1 - col)
                parts.append(
                    f"{ESC}[{rng.choice(FG)};{rng.choice(BG)}m"
                    + "".join(chr(rng.randint(0x21, 0x7E)) for _ in range(take))
                )
                col += take
            parts.append(f"{SGR_RESET}\r\n")
        cf.add("".join(parts))
    return cf


def gen_firehose(rng, n):
    """Saturating output at high rate, low style density — a fast `cat`.

    LINES_PER_CHUNK lines per chunk at 120 Hz scrolls the whole grid several
    times per second, saturating any coalescing cap, while keeping style runs
    near one per line so this profile isolates RATE rather than density.
    """
    lines_per_chunk = 8
    cf = ChunkFile()
    for _ in range(n):
        lines = []
        for _l in range(lines_per_chunk):
            words = " ".join(rng.choice(WORDS) for _ in range(rng.randint(6, 11)))
            lines.append(words[: COLS - 1] + EL)
        cf.add("\r\n".join(lines) + "\r\n")
    return cf


PROFILES = [
    ("idle", 0, gen_idle),
    ("typing", 5, gen_typing),
    ("build-log", 30, gen_build_log),
    ("htop", 10, gen_htop),
    ("firehose", 120, gen_firehose),
]


# ── Driver ───────────────────────────────────────────────────────────────────

def stable_salt(s):
    """Stable per-name salt. `hash()` is randomised per process — never use it."""
    return int.from_bytes(hashlib.sha256(s.encode()).digest()[:4], "little")


def generate(out_dir):
    os.makedirs(out_dir, exist_ok=True)
    manifests = []

    for name, hz, fn in PROFILES:
        # One independent, explicitly seeded stream per profile, so adding or
        # reordering profiles never perturbs another profile's bytes.
        rng = random.Random(SEED ^ stable_salt(name))
        count = hz * (DURATION_MS // 1000)
        cf = fn(rng, count)

        raw = cf.bytes()
        assert cf.count == count, f"{name}: {cf.count} chunks, expected {count}"

        with open(os.path.join(out_dir, f"{name}.chunks"), "wb") as f:
            f.write(raw)

        manifest = {
            "profile": name,
            "cols": COLS,
            "rows": ROWS,
            "hz": hz,
            "chunk_count": cf.count,
            "duration_ms": DURATION_MS,
            "sha256": hashlib.sha256(raw).hexdigest(),
        }
        with open(os.path.join(out_dir, f"{name}.json"), "w") as f:
            json.dump(manifest, f, indent=2, sort_keys=True)
            f.write("\n")

        manifest["_bytes"] = len(raw)
        manifests.append(manifest)

    return manifests


def main():
    ap = argparse.ArgumentParser(description="Generate the terminal replay fixtures.")
    ap.add_argument("--out", default=os.path.dirname(os.path.abspath(__file__)))
    ap.add_argument("--verify", action="store_true",
                    help="regenerate into a temp dir and diff against --out")
    args = ap.parse_args()

    if args.verify:
        with tempfile.TemporaryDirectory() as tmp:
            fresh = generate(tmp)
            ok = True
            for m in fresh:
                a = os.path.join(tmp, f"{m['profile']}.chunks")
                b = os.path.join(args.out, f"{m['profile']}.chunks")
                if not os.path.exists(b):
                    print(f"MISSING  {b}")
                    ok = False
                elif open(a, "rb").read() != open(b, "rb").read():
                    print(f"DIFFERS  {m['profile']}.chunks")
                    ok = False
                else:
                    print(f"ok       {m['profile']}.chunks  {m['sha256'][:16]}")
            print("VERIFY:", "byte-identical" if ok else "MISMATCH")
            return 0 if ok else 1

    manifests = generate(args.out)
    print(f"{'profile':<12} {'hz':>4} {'chunks':>7} {'bytes':>10}  sha256")
    for m in manifests:
        print(f"{m['profile']:<12} {m['hz']:>4} {m['chunk_count']:>7} "
              f"{m['_bytes']:>10}  {m['sha256']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
