#!/usr/bin/env python3
"""Pin every fvar axis but one to its default, producing a smaller static-axis
variable-font instance with `fontTools.varLib.instancer`.

This is a **checked-in, run-manually** tool (NOT a build.rs): frust-text
(`crates/frust-text/src/editor.rs`) only ever drives `StyleProperty::FontWeight`
and `StyleProperty::FontStyle` on a shaped run — no component anywhere in the
tree requests `opsz`, `wdth`, `GRAD`, `slnt`, or any of the Roboto-Flex-style
parametric axes (`XOPQ`/`YOPQ`/`XTRA`/`YTUC`/`YTLC`/`YTAS`/`YTDE`/`YTFI`). Every
axis but `wght` therefore always renders at its own default position, so an
instance that pins them there and keeps `wght` variable is bit-for-bit
equivalent at every weight the text stack can ever select, while dropping the
`gvar` deltas that vary the other axes.

Requires `fontTools` (this repo's fonts are regenerated with **4.63.0**;
`varLib.instancer`'s output is not guaranteed byte-stable across fontTools
releases, so re-pin the version here if you bump it):

    python3 -m venv /tmp/fonttools-venv
    /tmp/fonttools-venv/bin/pip install fonttools==4.63.0

Usage:

    <venv>/bin/python3 scripts/fonts/instance_variable_font.py \\
        <in.ttf> <out.ttf> --keep wght [--keep <axis> ...]

`--keep` names the axis tag(s) to leave variable (repeatable; at least one
required). Every other axis in the source font's `fvar` table is pinned at
the value the font itself declares as that axis's `defaultValue` — never a
value hard-coded in this script — so the same command instances any variable
font, not just Roboto Flex.

`<in.ttf>` and `<out.ttf>` may be the same path: the instance is built fully
in memory before anything is written, then saved to `<out.ttf>` in one
`TTFont.save` call.

Prints, to stdout: the source and output byte sizes, the remaining (kept)
axes, the instance's glyph count, and the SHA-256 of both files — the exact
provenance line for a `FONTS-LICENSE` "Modification" record.
"""

from __future__ import annotations

import argparse
import hashlib
import sys
import tempfile
from pathlib import Path

from fontTools import version as fonttools_version
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("input", type=Path, help="source variable-font path")
    parser.add_argument("output", type=Path, help="output instance path")
    parser.add_argument(
        "--keep",
        action="append",
        required=True,
        metavar="AXIS",
        help="fvar axis tag to leave variable (repeatable); every other axis "
        "in the source font is pinned at its own defaultValue",
    )
    args = parser.parse_args(argv)

    keep = set(args.keep)
    input_size = args.input.stat().st_size
    input_sha256 = _sha256(args.input)

    font = TTFont(args.input)
    fvar_axes = {axis.axisTag: axis for axis in font["fvar"].axes}

    unknown = keep - fvar_axes.keys()
    if unknown:
        parser.error(
            f"--keep names axis tag(s) not present in {args.input}'s fvar "
            f"table: {sorted(unknown)} (available: {sorted(fvar_axes)})"
        )

    pin_at_default = {
        tag: axis.defaultValue for tag, axis in fvar_axes.items() if tag not in keep
    }

    # instantiateVariableFont mutates in place; instancing to a distinct
    # temp path first keeps <in.ttf> == <out.ttf> safe (no read-after-partial-
    # write on the source file).
    instantiateVariableFont(font, pin_at_default, inplace=True)

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp) / args.output.name
        font.save(tmp_path)
        output_bytes = tmp_path.read_bytes()
        output_sha256 = hashlib.sha256(output_bytes).hexdigest()

    args.output.write_bytes(output_bytes)
    output_size = len(output_bytes)

    remaining_axes = [axis.axisTag for axis in font["fvar"].axes]
    glyph_count = len(font.getGlyphOrder())

    print(f"fontTools version: {fonttools_version}")
    print(f"input:  {args.input} ({input_size} B, SHA-256 {input_sha256})")
    print(f"output: {args.output} ({output_size} B, SHA-256 {output_sha256})")
    print(f"remaining axes: {remaining_axes}")
    print(f"glyph count: {glyph_count}")
    print(f"size reduction: {input_size - output_size} B")

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
