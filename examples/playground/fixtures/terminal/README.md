# Terminal-grid replay fixtures

Fixed terminal output the playground's **Terminal** section
(`../../src/pages/terminal.rs`) replays through a real VT emulator. Generated
once by `gen_terminal_fixtures.py` and checked in as data — the page generates
no content of its own, so the same profile always paints the same screens.

The page embeds each `<profile>.chunks` verbatim with `include_bytes!`, so
there is no per-platform asset loader and a run can never silently replay a
stale or missing file.

## Replay contract

```
<profile>.chunks   binary: repeat [u32 little-endian length][length bytes]
                   no header, no trailer, no padding
<profile>.json     manifest: profile, cols, rows, hz, chunk_count,
                   duration_ms, sha256 (of the .chunks file)
```

The page loads `<profile>.chunks` and feeds chunk `i` at
**`t = i * (1000 / hz)` ms** from replay start.

Grid is fixed at **80 columns × 45 rows** for every profile, and every profile
runs **30 000 ms**. The grid is fixed rather than derived from the device
screen, so the same profile paints the same cell count everywhere; the page
derives its *font size* from the available width instead, and clips anything
outside the grid.

## Profiles

| profile | hz | chunks | bytes | avg chunk | what it isolates |
|---|---|---|---|---|---|
| `idle` | 0 | 0 | 0 | — | **rest cost.** No output at all — the frame gate should idle at zero frames produced. |
| `typing` | 5 | 150 | 757 | 1 B | **minimum update.** One printable char at the cursor; a single cell changes. |
| `build-log` | 30 | 900 | 73 965 | 78 B | **the realistic workload and primary verdict input.** One scrolling log line per chunk, a few style runs per line. |
| `htop` | 10 | 300 | 5 469 600 | 18 228 B | **style density.** Full-screen redraw with colour changing every 2 cells → ~1800 style runs per frame, at a modest 10 Hz. |
| `firehose` | 120 | 3600 | 1 937 952 | 534 B | **rate.** 8 lines per chunk at 120 Hz = 960 lines/s, scrolling the whole grid ~21×/s, at ~1 style run per line. |

**Why `htop` and `firehose` are separate profiles.** frust's per-frame GPU
cost scales with vello's scene-derived counters —
`n_paths` / `n_draw_objects` / `n_path_tags` — and *not* with surface
resolution: only 2 of ~14 compute stages size their dispatch from
width/height. Style-run count is therefore an independent cost axis from output
rate. If one profile varied both at once, an unfavourable result would be
impossible to attribute to either. `htop` pushes density at low rate;
`firehose` pushes rate at low density.

## Escape-sequence subset

Deliberately conservative, so that any emulator interprets every byte the same
way — the page drives the `vt100` crate today, and these bytes were authored to
stay meaningful against a full emulator (kterm) too. A divergence in how two
emulators interpret an exotic sequence would corrupt such a comparison silently
instead of failing loudly.

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

> **Regenerating changes what the page paints.** Changing the seed, a
> profile's parameters, or the escape subset makes every later run
> incomparable with anything observed before it, and shifts the run counts
> `src/pages/terminal.rs`'s own tests pin. If a profile genuinely must change,
> treat it as a new profile name rather than a redefinition.

Total on disk: ~7.5 MB.
