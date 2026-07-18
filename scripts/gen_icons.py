#!/usr/bin/env python3
"""Generate crates/forgekit-widgets/src/icons/mod.rs from Material Symbols SVGs.

This is a **checked-in, run-manually** codegen tool (NOT a build.rs): a
maintainer points `--src` at a directory of Material Symbols SVG files (e.g. a
`material-design-icons` checkout, or a batch downloaded from
https://fonts.google.com/icons), and the script extracts each glyph's single
`<path d="...">`, validates it is non-empty, and re-emits the `icons` module
with one `pub const <NAME>: IconSource = IconSource { d: "...", design: 24.0 };`
entry per glyph plus an `ALL` slice.

Standard library only — no third-party dependencies.

Material Symbols is Apache-2.0 (see
crates/forgekit-widgets/src/icons/LICENSE-material-symbols); the emitted path
data is safe to vendor.

Usage:

    python3 scripts/gen_icons.py --src <svg-dir> \\
        [--out crates/forgekit-widgets/src/icons/mod.rs] \\
        [--design 24.0]

By default the icon set is the ~40-glyph starter list Huddle needs; pass
`--all-in-dir` to instead emit every `*.svg` found under `--src`.

The `NAME` a file maps to is its stem upper-snake-cased (e.g.
`arrow_back.svg` -> `ARROW_BACK`, `arrow-back.svg` -> `ARROW_BACK`).
"""

from __future__ import annotations

import argparse
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

# The starter set Huddle needs. Each entry is a (CONST_NAME, svg_stem) pair;
# svg_stem is the filename (without .svg) to look for under --src. Material
# Symbols filenames are lower_snake_case, so the stem doubles as the lookup key.
STARTER_SET: list[tuple[str, str]] = [
    ("HOME", "home"),
    ("SEARCH", "search"),
    ("NOTIFICATIONS", "notifications"),
    ("PERSON", "person"),
    ("TAG", "tag"),
    ("LOCK", "lock"),
    ("SEND", "send"),
    ("ADD", "add"),
    ("MOOD", "mood"),
    ("ATTACH_FILE", "attach_file"),
    ("REPLY", "reply"),
    ("FORUM", "forum"),
    ("MORE_VERT", "more_vert"),
    ("MORE_HORIZ", "more_horiz"),
    ("CLOSE", "close"),
    ("CHECK", "check"),
    ("ARROW_BACK", "arrow_back"),
    ("CHEVRON_RIGHT", "chevron_right"),
    ("SETTINGS", "settings"),
    ("EDIT", "edit"),
    ("DELETE", "delete"),
    ("ARCHIVE", "archive"),
    ("VOLUME_OFF", "volume_off"),
    ("PUSH_PIN", "push_pin"),
    ("STAR", "star_outline"),
    ("STAR_FILLED", "star"),
    ("DARK_MODE", "dark_mode"),
    ("LIGHT_MODE", "light_mode"),
    ("PALETTE", "palette"),
    ("FORMAT_SIZE", "format_size"),
    ("LOGOUT", "logout"),
    ("GROUP", "group"),
    ("IMAGE", "image"),
    ("DESCRIPTION", "description"),
    ("LINK", "link"),
    ("REFRESH", "refresh"),
    ("MIC", "mic"),
    ("VIDEOCAM", "videocam"),
    ("CALL", "call"),
    ("SCHEDULE", "schedule"),
    ("DONE_ALL", "done_all"),
]

MODULE_HEADER = '''\
//! Vendored Material Symbols starter set — GENERATED, do not hand-edit.
//!
//! Each entry is a [`IconSource`](crate::IconSource) carrying a Material Symbols
//! glyph as SVG path `d` data in a 24×24 design box. Pass one to
//! [`icon`](crate::icon) (or convert it via `IconData::from(..)`) to paint it.
//!
//! # Provenance & license
//!
//! Material Symbols is licensed Apache-2.0 (see
//! `LICENSE-material-symbols` alongside this module). Google publishes the icons
//! with no NOTICE file and no embedded per-file copyright, and attribution is
//! optional per Google's own guidance — so the path data is safe to vendor.
//!
//! # Regenerating
//!
//! ```text
//! python3 scripts/gen_icons.py --src <material-symbols-svg-dir> \\
//!     --out crates/forgekit-widgets/src/icons/mod.rs
//! ```
//!
//! The script reads each `<name>.svg`, extracts its single `<path d="...">`,
//! validates it parses, and re-emits this file. See `scripts/gen_icons.py`.

use crate::IconSource;

/// The design-box side length every entry below is authored against.
const D: f64 = {design};
'''


def upper_snake(stem: str) -> str:
    """`arrow-back` / `arrow_back` -> `ARROW_BACK`."""
    return re.sub(r"[^0-9a-zA-Z]+", "_", stem).strip("_").upper()


def extract_path_d(svg_path: Path) -> str:
    """Return the concatenated `d` of every `<path>` in an SVG file.

    Material Symbols glyphs are typically a single `<path>`, but a few compose
    several sub-paths; concatenating their `d` attributes preserves the whole
    glyph as one fill.
    """
    tree = ET.parse(svg_path)
    root = tree.getroot()
    ds: list[str] = []
    for elem in root.iter():
        tag = elem.tag.rsplit("}", 1)[-1]  # strip any XML namespace
        if tag == "path":
            d = elem.get("d")
            if d:
                ds.append(d.strip())
    return " ".join(ds)


def escape_rust(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def resolve_entries(src: Path, all_in_dir: bool) -> list[tuple[str, str]]:
    """Map (CONST_NAME -> path d) for the requested set."""
    entries: list[tuple[str, str]] = []
    if all_in_dir:
        svgs = sorted(src.glob("*.svg"))
        if not svgs:
            sys.exit(f"error: no *.svg files found under {src}")
        for svg in svgs:
            name = upper_snake(svg.stem)
            d = extract_path_d(svg)
            if not d:
                sys.exit(f"error: {svg} has no <path d=...> data")
            entries.append((name, d))
        return entries

    missing: list[str] = []
    for const_name, stem in STARTER_SET:
        svg = src / f"{stem}.svg"
        if not svg.exists():
            missing.append(stem)
            continue
        d = extract_path_d(svg)
        if not d:
            sys.exit(f"error: {svg} has no <path d=...> data")
        entries.append((const_name, d))
    if missing:
        sys.exit(
            "error: missing SVGs under "
            f"{src}: {', '.join(missing)}\n"
            "(the starter set expects Material Symbols lower_snake_case names)"
        )
    return entries


def render_module(entries: list[tuple[str, str]], design: float) -> str:
    out: list[str] = [MODULE_HEADER.format(design=_fmt_f64(design))]
    for name, d in entries:
        out.append("")
        out.append(f"/// `{name.lower()}`")
        out.append(f"pub const {name}: IconSource = IconSource {{")
        out.append(f'    d: "{escape_rust(d)}",')
        out.append("    design: D,")
        out.append("};")
    out.append("")
    out.append(
        "/// Every generated [`IconSource`] in this module, for exhaustive\n"
        "/// iteration (e.g. a parse-validation test, or a picker gallery)."
    )
    out.append("pub const ALL: &[IconSource] = &[")
    for name, _ in entries:
        out.append(f"    {name},")
    out.append("];")
    out.append("")
    return "\n".join(out)


def _fmt_f64(value: float) -> str:
    # Emit `24.0`, not `24`, so the const is unambiguously f64.
    return f"{value:.1f}" if value == int(value) else repr(value)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--src",
        required=True,
        type=Path,
        help="directory of Material Symbols SVG files",
    )
    ap.add_argument(
        "--out",
        type=Path,
        default=Path("crates/forgekit-widgets/src/icons/mod.rs"),
        help="output Rust module path",
    )
    ap.add_argument(
        "--design",
        type=float,
        default=24.0,
        help="design-box side length (default 24.0)",
    )
    ap.add_argument(
        "--all-in-dir",
        action="store_true",
        help="emit every *.svg under --src instead of the fixed starter set",
    )
    args = ap.parse_args()

    if not args.src.is_dir():
        sys.exit(f"error: --src {args.src} is not a directory")

    entries = resolve_entries(args.src, args.all_in_dir)
    module = render_module(entries, args.design)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(module, encoding="utf-8")
    print(f"wrote {len(entries)} icons to {args.out}")


if __name__ == "__main__":
    main()
