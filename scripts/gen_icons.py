#!/usr/bin/env python3
"""Generate crates/frust-widgets/src/icons/mod.rs from Material Symbols SVGs.

This is a **checked-in, run-manually** codegen tool (NOT a build.rs): a
maintainer points `--src` at a directory of Material Symbols SVG files (e.g. a
`material-design-icons` checkout, or a batch downloaded from
https://fonts.google.com/icons or the upstream raw URLs — see below), and the
script extracts each glyph's `<path d="...">` data, normalizes it into the
requested square design box (see "Coordinate normalization" below), validates
it is non-empty, and re-emits the `icons` module with one
`pub const <NAME>: IconSource = IconSource { d: "...", design: 24.0 };` entry
per glyph plus an `ALL` slice.

Standard library only — no third-party dependencies.

Material Symbols is Apache-2.0 (see
crates/frust-widgets/src/icons/LICENSE-material-symbols); the emitted path
data is safe to vendor.

Usage:

    python3 scripts/gen_icons.py --src <svg-dir> \\
        [--out crates/frust-widgets/src/icons/mod.rs] \\
        [--design 24.0]

`--src` expects one `<stem>.svg` file per `STARTER_SET` entry below, where
`<stem>` is the entry's second tuple element (e.g. `home.svg` for `HOME`).
Fetch each file from upstream, e.g.:

    https://raw.githubusercontent.com/google/material-design-icons/master/\\
        symbols/web/<name>/materialsymbolsoutlined/<name>_24px.svg

renamed to `<stem>.svg` in the `--src` directory (a handful of entries need a
non-default upstream `<name>` — see the STARTER_SET comment below, e.g. the
filled/outline star pair).

By default the icon set is the 41-glyph starter list Huddle needs; pass
`--all-in-dir` to instead emit every `*.svg` found under `--src`.

The `NAME` a file maps to is its stem upper-snake-cased (e.g.
`arrow_back.svg` -> `ARROW_BACK`, `arrow-back.svg` -> `ARROW_BACK`).

## Coordinate normalization

Current-generation Material Symbols exports (the `materialsymbolsoutlined`
family) ship path data authored against a `viewBox="0 -960 960 960"` 960-unit
grid rather than the `<design>×<design>` box directly — a `<path>` in that
family has raw coordinates in `x: 0..960`, `y: -960..0`, scaled down to the
`width`/`height` (typically 24) attributes only by the browser/renderer
honoring the `viewBox`. `IconSource::d` must already be in the `0..design`
(y-down) space `frust-widgets::icon` expects (see its module docs) with no
implicit viewBox scaling, so this script detects a source SVG's `viewBox` and
applies the equivalent affine transform (`normalize_path_d`) directly to each
`d` string's coordinates before emitting it: `new = (old - view_box_origin) *
(design / view_box_size)`. An older-style SVG with no `viewBox` (coordinates
already authored directly against a `<design>`-sized box, as this repo's
previous hand-authored placeholder set and a few still-unmigrated upstream
icons are) passes through unchanged. Only the path commands this repo's
starter set actually uses are supported (`M`/`L`/`H`/`V`/`Q`/`T`/`Z`, upper or
lower case) — a `C`/`S`/`A` (cubic/smooth-cubic/arc) command raises rather
than silently mis-transforming, since those need extra per-command parameter
handling (flags for `A`) this script doesn't implement.
"""

from __future__ import annotations

import argparse
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

# The starter set Huddle needs. Each entry is a (CONST_NAME, svg_stem) pair;
# svg_stem is the filename (without .svg) to look for under --src. Material
# Symbols filenames are lower_snake_case, so the stem doubles as the lookup key
# for most entries — the upstream icon name matches the stem directly. Two
# exceptions, where the const needs a specific upstream *variant* rather than
# a same-named icon: STAR/STAR_FILLED both come from the single upstream
# `star` icon's two static exports — the default (fill=0, outline) `star.svg`
# and the `fill1` (filled) variant, conventionally saved here as
# `star_fill1.svg` (see this module's docstring for the upstream URL shape;
# append `_fill1` before `_24px.svg` for the filled export).
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
    ("STAR", "star"),
    ("STAR_FILLED", "star_fill1"),
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
//! The glyphs below are regenerated verbatim from a real
//! `google/material-design-icons` `materialsymbolsoutlined` checkout (24px
//! variant), with each `<path d>` renormalized from its source `viewBox` into
//! this crate's `0..24` design-box convention — see `scripts/gen_icons.py`'s
//! module docstring ("Coordinate normalization") for why that step is needed.
//!
//! # Regenerating
//!
//! ```text
//! python3 scripts/gen_icons.py --src <material-symbols-svg-dir> \\
//!     --out crates/frust-widgets/src/icons/mod.rs
//! ```
//!
//! The script reads each `<name>.svg`, extracts and renormalizes its
//! `<path d="...">` data, validates it parses, and re-emits this file. See
//! `scripts/gen_icons.py`.

use crate::IconSource;

/// The design-box side length every entry below is authored against.
const D: f64 = {design};
'''


def upper_snake(stem: str) -> str:
    """`arrow-back` / `arrow_back` -> `ARROW_BACK`."""
    return re.sub(r"[^0-9a-zA-Z]+", "_", stem).strip("_").upper()


# Stdlib-only XXE/entity-expansion mitigation (review finding A15): Python's
# `xml.etree.ElementTree` does not resolve external entities by default, but
# it *does* still expand internally-declared `<!ENTITY>` definitions
# (a "billion laughs"-style amplification vector) and has no built-in
# safelist for a `<!DOCTYPE>` declaration in general. `defusedxml` is the
# usual fix but is a third-party dependency this script deliberately has none
# of (see the module docstring), so instead this rejects any input containing
# a doctype/entity declaration outright before it ever reaches `ET.fromstring`
# — sufficient because every legitimate Material Symbols export is a bare
# `<svg>` document with no DOCTYPE. **Trusted-input assumption**: this script
# is a maintainer-run, local, manual codegen tool, never fed untrusted input
# over a network boundary at run time (the maintainer is the one who already
# chose to fetch the SVGs) — this guard exists as defense-in-depth against a
# poisoned/compromised upstream file, not because `--src` is expected to see
# adversarial input in normal use.
_UNSAFE_XML_RE = re.compile(rb"<!\s*(DOCTYPE|ENTITY)\b", re.IGNORECASE)


def _reject_unsafe_xml(raw: bytes, svg_path: Path) -> None:
    if _UNSAFE_XML_RE.search(raw):
        sys.exit(
            f"error: {svg_path} contains a DOCTYPE/ENTITY declaration — "
            "refusing to parse (see _reject_unsafe_xml's trusted-input note "
            "in this script)"
        )


# Matches one SVG path command letter or one numeric parameter (handles
# concatenated numbers with no separator, e.g. "240-200" or ".5.5", the way
# minified Material Symbols `d` strings are shipped).
_PATH_TOKEN_RE = re.compile(
    r"[A-Za-z]|[-+]?(?:\d+\.\d+|\.\d+|\d+)(?:[eE][-+]?\d+)?"
)

# The path commands this script knows how to renormalize, mapped to how many
# numbers make up one coordinate *pair* repeat unit (M/L/Q/T) vs. one bare
# coordinate repeat unit (H is x-only, V is y-only). Z takes no parameters.
_PAIR_COMMANDS = {"M", "L", "Q", "T"}
_X_ONLY_COMMANDS = {"H"}
_Y_ONLY_COMMANDS = {"V"}
_NO_ARG_COMMANDS = {"Z"}


def _fmt_coord(value: float) -> str:
    s = f"{value:.4f}".rstrip("0").rstrip(".")
    return "0" if s in ("", "-0") else s


def normalize_path_d(
    d: str, view_box: tuple[float, float, float, float] | None, design: float
) -> str:
    """Renormalize `d`'s coordinates into the `0..design` (y-down) box.

    `view_box` is `(min_x, min_y, width, height)` from the source SVG's
    `viewBox` attribute, or `None` if the source had none (coordinates are
    then assumed already authored directly against a `<design>`-sized box —
    an older-style export, or this repo's own previous hand-authored data —
    and passed through unchanged). See this module's docstring for why a
    current-generation Material Symbols export needs this at all.

    Only `M`/`L`/`H`/`V`/`Q`/`T`/`Z` (upper or lower case) are supported —
    the only commands this repo's starter set actually uses; a `C`/`S`/`A`
    raises rather than silently mis-transforming.
    """
    if view_box is None:
        return d
    min_x, min_y, vb_w, vb_h = view_box
    if vb_w <= 0 or vb_h <= 0:
        return d
    # Material Symbols viewBoxes are square (vb_w == vb_h); scale against the
    # width either way since a non-square input would already be a malformed
    # upstream asset.
    scale = design / vb_w
    off_x = -min_x * scale
    off_y = -min_y * scale

    tokens = _PATH_TOKEN_RE.findall(d)
    out: list[str] = []
    cmd: str | None = None
    idx = 0
    n = len(tokens)
    # Per the SVG path grammar, a path's very first moveto is always treated
    # as absolute even when written lowercase ("m") — there is no established
    # current point yet for it to be relative *to*. Only that one leading
    # coordinate pair gets this treatment; every later moveto (including a
    # later `m` opening a new subpath after a `Z`) is relative as normal.
    first_pair_pending = True
    while idx < n:
        tok = tokens[idx]
        if tok.isalpha():
            cmd = tok
            out.append(tok)
            idx += 1
            continue
        if cmd is None:
            sys.exit(f"error: path data starts with a coordinate, not a command: {d!r}")
        upper = cmd.upper()
        is_abs = cmd.isupper()
        if upper in _PAIR_COMMANDS:
            x, y = float(tokens[idx]), float(tokens[idx + 1])
            treat_as_abs = is_abs or (upper == "M" and first_pair_pending)
            if treat_as_abs:
                nx, ny = x * scale + off_x, y * scale + off_y
            else:
                nx, ny = x * scale, y * scale
            out.append(_fmt_coord(nx))
            out.append(_fmt_coord(ny))
            idx += 2
            first_pair_pending = False
        elif upper in _X_ONLY_COMMANDS:
            x = float(tokens[idx])
            out.append(_fmt_coord(x * scale + off_x if is_abs else x * scale))
            idx += 1
        elif upper in _Y_ONLY_COMMANDS:
            y = float(tokens[idx])
            out.append(_fmt_coord(y * scale + off_y if is_abs else y * scale))
            idx += 1
        elif upper in _NO_ARG_COMMANDS:
            # Z takes no numeric args, so we should never see a bare number
            # right after one — the token stream is malformed.
            sys.exit(f"error: unexpected numeric token after '{cmd}' in: {d!r}")
        else:
            sys.exit(
                f"error: unsupported path command '{cmd}' in {d!r} — "
                "normalize_path_d only handles M/L/H/V/Q/T/Z (see its docstring)"
            )
    return " ".join(out)


def _parse_view_box(root: ET.Element) -> tuple[float, float, float, float] | None:
    raw = root.get("viewBox")
    if not raw:
        return None
    parts = raw.replace(",", " ").split()
    if len(parts) != 4:
        sys.exit(f"error: malformed viewBox {raw!r}")
    min_x, min_y, w, h = (float(p) for p in parts)
    return (min_x, min_y, w, h)


def extract_path_d(svg_path: Path, design: float) -> str:
    """Return the concatenated, normalized `d` of every `<path>` in an SVG file.

    Material Symbols glyphs are typically a single `<path>`, but a few compose
    several sub-paths; concatenating their `d` attributes preserves the whole
    glyph as one fill. Each `<path>`'s `d` is renormalized into the `0..design`
    box per its source SVG's `viewBox` (see `normalize_path_d`) before being
    concatenated.
    """
    raw = svg_path.read_bytes()
    _reject_unsafe_xml(raw, svg_path)
    root = ET.fromstring(raw)
    view_box = _parse_view_box(root)
    ds: list[str] = []
    for elem in root.iter():
        tag = elem.tag.rsplit("}", 1)[-1]  # strip any XML namespace
        if tag == "path":
            d = elem.get("d")
            if d:
                ds.append(normalize_path_d(d.strip(), view_box, design))
    return " ".join(ds)


def escape_rust(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def resolve_entries(
    src: Path, all_in_dir: bool, design: float
) -> list[tuple[str, str]]:
    """Map (CONST_NAME -> path d) for the requested set."""
    entries: list[tuple[str, str]] = []
    if all_in_dir:
        svgs = sorted(src.glob("*.svg"))
        if not svgs:
            sys.exit(f"error: no *.svg files found under {src}")
        for svg in svgs:
            name = upper_snake(svg.stem)
            d = extract_path_d(svg, design)
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
        d = extract_path_d(svg, design)
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


def self_test() -> None:
    """Cheap, stdlib-only `assert`-based checks for `normalize_path_d`.

    Covers the affine transform against a synthetic `viewBox="0 -960 960 960"`
    (the current-generation Material Symbols grid this script's whole
    "Coordinate normalization" docstring section exists for) with known
    coordinates and hand-verified expected output, plus the SVG-spec
    "a path's very first moveto is always absolute, even written lowercase"
    exception `normalize_path_d` implements. Run via `--self-test`; exits
    non-zero (via `AssertionError`, uncaught) on any failure, so it's a
    tripwire in the same spirit as this repo's Rust test suite, without a
    third-party test framework dependency.
    """
    vb = (0.0, -960.0, 960.0, 960.0)
    design = 24.0

    # Plain absolute M/L: scale = 24/960 = 0.025; off_x = 0, off_y = 24.
    assert normalize_path_d("M0 -960L960 0", vb, design) == "M 0 0 L 24 24"

    # Implicit-relative-first-m: a lowercase leading `m` is still treated as
    # absolute for its one coordinate pair (no established current point to be
    # relative to yet); a later relative `l` in the same path stays relative
    # (scaled, no translation offset).
    assert normalize_path_d("m480 -480l100 100", vb, design) == "m 12 12 l 2.5 2.5"

    # Absolute H/V (x-only / y-only) plus a bare Z.
    assert normalize_path_d("M100 -860H900V-100Z", vb, design) == "M 2.5 2.5 H 22.5 V 21.5 Z"

    # No viewBox: passed through unchanged (an older-style export already
    # authored directly against the design box).
    assert normalize_path_d("M1 2L3 4", None, design) == "M1 2L3 4"

    print("gen_icons.py self-test: all checks passed")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--src",
        type=Path,
        help="directory of Material Symbols SVG files",
    )
    ap.add_argument(
        "--out",
        type=Path,
        default=Path("crates/frust-widgets/src/icons/mod.rs"),
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
    ap.add_argument(
        "--self-test",
        action="store_true",
        help="run normalize_path_d's built-in assert-based checks and exit "
        "(no --src needed)",
    )
    args = ap.parse_args()

    if args.self_test:
        self_test()
        return

    if args.src is None:
        ap.error("--src is required (unless --self-test)")
    if not args.src.is_dir():
        sys.exit(f"error: --src {args.src} is not a directory")

    entries = resolve_entries(args.src, args.all_in_dir, args.design)
    module = render_module(entries, args.design)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(module, encoding="utf-8")
    print(f"wrote {len(entries)} icons to {args.out}")


if __name__ == "__main__":
    main()
