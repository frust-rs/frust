# S9 terminal-grid fixtures

Byte-identical terminal output replayed by **both** bench apps so their
rendering costs can be compared on the same device. Generated once by
`gen_terminal_fixtures.py` and checked in as data — neither app generates
content of its own.

This is the same fairness convention S6 uses for its multilingual corpus
(`benchmarks/frust_bench/src/scenarios/s6_text.rs`'s `CORPUS` is documented as
byte-identical to the Flutter side's `_corpus`). Here the shared artifact is a
file rather than a source constant, because the volume is far too large to
embed as a literal and duplicating a generator in two languages would drift.

## Replay contract

```
<profile>.chunks   binary: repeat [u32 little-endian length][length bytes]
                   no header, no trailer, no padding
<profile>.json     manifest: profile, cols, rows, hz, chunk_count,
                   duration_ms, sha256 (of the .chunks file)
```

An app loads `<profile>.chunks` and emits chunk `i` at
**`t = i * (1000 / hz)` ms** from scenario start. Both apps therefore feed
byte-identical bytes to their emulators at identical times.

Grid is fixed at **80 columns × 45 rows** for every profile, and every profile
runs **30 000 ms**. The grid is fixed rather than derived from the device
screen so the two apps rasterize the same cell count; each app anchors it
top-left in the safe area at a fixed monospace size and clips any overflow.

## Profiles

| profile | hz | chunks | bytes | avg chunk | what it isolates |
|---|---|---|---|---|---|
| `idle` | 0 | 0 | 0 | — | **rest cost.** No output at all — the frame gate should idle at zero frames produced. |
| `typing` | 5 | 150 | 757 | 1 B | **minimum update.** One printable char at the cursor; a single cell changes. |
| `build-log` | 30 | 900 | 73 965 | 78 B | **the realistic workload and primary verdict input.** One scrolling log line per chunk, a few style runs per line. |
| `htop` | 10 | 300 | 5 469 600 | 18 228 B | **style density.** Full-screen redraw with colour changing every 2 cells → ~1800 style runs per frame, at a modest 10 Hz. |
| `firehose` | 120 | 3600 | 1 937 952 | 534 B | **rate.** 8 lines per chunk at 120 Hz = 960 lines/s, scrolling the whole grid ~21×/s, at ~1 style run per line. |

**Why `htop` and `firehose` are separate profiles.** A research finding
(`workflow/plans/features/frust-terminal/research/RESEARCH.md` §3) established
that frust's per-frame GPU cost scales with vello's scene-derived counters —
`n_paths` / `n_draw_objects` / `n_path_tags` — and *not* with surface
resolution: only 2 of ~14 compute stages size their dispatch from
width/height. Style-run count is therefore an independent cost axis from output
rate. If one profile varied both at once, an unfavourable result would be
impossible to attribute to either. `htop` pushes density at low rate;
`firehose` pushes rate at low density.

## Escape-sequence subset

Deliberately conservative, because the two sides run **different emulators** —
Flutter drives kterm 1.5.3, frust drives the `vt100` crate. A divergence in how
they interpret an exotic sequence would silently corrupt the comparison instead
of failing loudly.

**Used:** `ESC[H` (cursor home), `ESC[K` (erase to end of line),
`ESC[<n>m` SGR limited to reset `0`, bold `1`, and the basic `30`–`37` /
`40`–`47` colour range, and `\r\n`.

**Deliberately avoided:** alternate screen, scroll regions, 256-colour and
truecolour SGR, mouse reporting, OSC sequences (including OSC 8 hyperlinks),
and everything in the Kitty protocol family. Scrolling is produced only by
`\r\n` at the last row, which both emulators handle identically.

## Regenerating

```bash
python3 gen_terminal_fixtures.py            # write fixtures into this directory
python3 gen_terminal_fixtures.py --verify   # regenerate to a temp dir and diff
```

Determinism comes from a fixed `SEED` and an explicitly seeded
`random.Random` per profile — never the module-level `random` functions, never
`time`, never `hash()` (which is per-process randomised). Each profile draws
from its own salted stream, so adding or reordering profiles cannot perturb
another profile's bytes. `--verify` is the guard: it regenerates into a temp
directory and asserts byte equality against what is on disk.

### sha256

| profile | sha256 |
|---|---|
| `idle` | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| `typing` | `76ba735ad37c4f8d218d6a1eb7588127677280b3d5c1f36f3b50ad2889f2f2f1` |
| `build-log` | `f6b62258cdbe1b9f4758c22f3eee8dbe07793dab26e0d1f302f1c39ec93776b1` |
| `htop` | `bf9c2d46850f857bdc020f9cfd5e902f4be15ca8a535c10b6cf30e05fd5b3c2c` |
| `firehose` | `d06a7505d5366a982f29efead7148e60740e616e4b116719289ea45bf84ca519` |

(`idle`'s digest is the sha256 of the empty string — the file is zero bytes.)

> **Regenerating invalidates comparability.** Any number already recorded in
> `benchmarks/RESULTS.md` was measured against these exact bytes. Changing the
> seed, a profile's parameters, or the escape subset makes new numbers
> incomparable with the history — the same trap the protocol documents for its
> other scenarios. If a profile genuinely must change, treat it as a new
> profile name rather than a redefinition, and say so in `RESULTS.md`.

Total on disk: ~7.5 MB.
