#!/usr/bin/env python3
"""Generate plugins/material/src/icons.rs from google/material-design-icons SVGs.

This is a **checked-in, run-manually** codegen tool (NOT a build.rs), the
`frust-material` sibling of the baseline `scripts/gen_icons.py` — it follows
the same generated-file pattern (see that script's own module docstring) but
draws from a different upstream source and coordinate convention, so it is
its own script rather than a shared one. Do not merge this back into the
repo-root `scripts/gen_icons.py` — the two crates' icon sets, upstream
families, and design boxes are independent.

`frust-material`'s icon set is the 88 distinct `M3EIcons.*` glyphs the
`material_3_expressive` reference Flutter library and its example app
actually reference (see `ICON_SET` below — one dart-style snake_case name per
glyph, already including its `_outlined`/`_rounded`/`_sharp` variant suffix
where one is referenced; a bare name with no suffix is the default filled
style). Only the variants actually referenced are generated, not every style
for every glyph.

Standard library only — no third-party dependencies.

## Upstream source

Path data comes from the classic (non-Symbols) `google/material-design-icons`
family, fetched straight from GitHub's raw content host, one glyph at a time:

    https://raw.githubusercontent.com/google/material-design-icons/master/
        src/<category>/<base_name>/materialicons<style>/24px.svg

`<style>` is empty for the default filled export, `outlined` for an
`_outlined`-suffixed name, `round` for `_rounded`, `sharp` for `_sharp`. The
repo layout requires a `<category>` path segment (`action`, `navigation`,
`content`, ...) that the icon's own name does not encode, so this script
carries `BASE_CATEGORY`, a small embedded `base_name -> category` map
resolved once against upstream's own `update/current_versions.json` manifest
(every name in that manifest also carries a `symbols` entry for the separate
Material *Symbols* family — cross-referenced out here since this script wants
the classic family's per-category SVG, not a Symbols glyph).

Material Design Icons is licensed Apache-2.0 (Google) — see
`plugins/material/src/icons.rs`'s module doc and this repo's top-level
license notices; the emitted path data is safe to vendor.

## Coordinate convention

Unlike the Material Symbols family the baseline script normalizes (which
ships a `viewBox="0 -960 960 960"` grid needing an affine rescale — see that
script's "Coordinate normalization" section), the classic Material Icons
24px family this script fetches already ships `viewBox="0 0 24 24"` — the
same `0..design` (y-down) box `IconSource::d` needs directly, so this script
passes every `d` through unscaled. It still parses each SVG's `viewBox` and
hard-fails if a fetched file doesn't match `0 0 <design> <design>` exactly,
rather than silently mis-rendering a differently-scaled asset (classic
Material Icons path data commonly uses cubic-bezier `C`/`S` commands the
baseline script's affine-transform pass does not support at all, so a
"transform and hope" fallback is not an option here).

Each 24px export also ships an invisible `<path d="M0 0h24v24H0z" fill="none"/>`
(or an equivalent `<rect fill="none".../>`) as a click-target keyline — a
`fill="none"` element carries no visible geometry and must be excluded, or
its implicit rectangle would concatenate into the glyph's single fill and
paint a solid 24x24 box over the icon. `extract_path_d` filters any
`<path>`/`<rect>` whose `fill` attribute is (case-insensitively) `none`. A
handful of glyphs (e.g. `horizontal_rule`) draw their actual *visible*
geometry as a plain axis-aligned `<rect>` rather than a `<path>` at all;
`extract_path_d` converts a surviving (visible) one into an equivalent path
`d` string via `_rect_to_d`.

## Usage

    python3 plugins/material/scripts/gen_icons.py \\
        [--out plugins/material/src/icons.rs] \\
        [--cache-dir target/material-icons-cache] \\
        [--design 24.0] [--refresh]
    cargo fmt -p frust-material

`--cache-dir` defaults to `target/material-icons-cache` at the repo root
(already covered by the root `/target` gitignore entry) — downloaded SVGs are
cached there across runs so a regen after the first only needs the network
if `--refresh` is passed or a new glyph is added to `ICON_SET`. Network
access is required at most once per glyph, ever; this script deliberately
does not hand-author path data as a fallback — a fetch failure is a hard
error (see `fetch_svg`).

Determinism: `ICON_SET` is a fixed, sorted literal list (not a directory
listing), so re-running this script against the same cache (or against
upstream, since the fetched files themselves are static) re-emits a
byte-identical file — this is `icons.rs`'s own conformance expectation, the
same one the baseline script documents for its own generated module.
"""

from __future__ import annotations

import argparse
import re
import sys
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]

RAW_BASE = "https://raw.githubusercontent.com/google/material-design-icons/master/src"

# The 88 `M3EIcons.*` names referenced by material_3_expressive's components
# and example app (see this task's own `grep -rho "M3EIcons\\.[a-zA-Z0-9_]*"`
# recipe over `lib/components` + `example/lib`), already dart-style
# snake_case, sorted for determinism. A `_rounded`/`_outlined`/`_sharp`
# suffix (see `split_variant`) picks the upstream style export; a bare name
# is the default filled style.
ICON_SET: list[str] = [
    "account_circle",
    "add",
    "apps",
    "archive",
    "arrow_back",
    "arrow_back_rounded",
    "arrow_drop_down",
    "arrow_drop_down_circle",
    "arrow_drop_up",
    "arrow_right_rounded",
    "autorenew",
    "battery_alert",
    "bookmark",
    "build",
    "calendar_month",
    "calendar_today",
    "calendar_view_week",
    "category",
    "chat_bubble",
    "check",
    "check_box",
    "check_box_outline_blank_rounded",
    "check_box_rounded",
    "check_rounded",
    "chevron_left",
    "chevron_right",
    "close",
    "content_copy",
    "crop_square",
    "dark_mode",
    "delete",
    "edit",
    "edit_outlined",
    "error",
    "expand_more_rounded",
    "favorite",
    "favorite_border",
    "format_align_center",
    "format_align_left",
    "format_align_right",
    "home",
    "horizontal_rule",
    "hourglass_empty",
    "image",
    "inbox",
    "info",
    "keyboard_arrow_down",
    "label",
    "light_mode",
    "linear_scale",
    "list",
    "mail",
    "menu",
    "menu_open",
    "message",
    "mic",
    "more_horiz",
    "more_vert",
    "notifications",
    "palette",
    "radio_button_checked",
    "refresh",
    "report",
    "save",
    "schedule",
    "search",
    "select_all",
    "send",
    "share",
    "smart_button",
    "space_dashboard",
    "star",
    "star_outline",
    "system_update",
    "tab",
    "tag",
    "text_fields",
    "toggle_on",
    "tune",
    "vertical_align_bottom",
    "vertical_split",
    "videocam",
    "view_carousel",
    "view_column",
    "view_sidebar",
    "view_week",
    "volume_up",
    "web_asset",
]

# `base_name -> upstream category` for every distinct base name `ICON_SET`
# resolves to (a variant suffix shares its base's category — see
# `split_variant`). Resolved once against upstream's
# `update/current_versions.json` manifest (each `category::name` key), taking
# the one non-`symbols` category every classic-family name carries; kept as a
# small literal map here rather than fetched at generation time so a regen
# needs no extra request beyond the 88 SVGs themselves.
BASE_CATEGORY: dict[str, str] = {
    "account_circle": "action",
    "add": "content",
    "apps": "navigation",
    "archive": "content",
    "arrow_back": "navigation",
    "arrow_drop_down": "navigation",
    "arrow_drop_down_circle": "navigation",
    "arrow_drop_up": "navigation",
    "arrow_right": "navigation",
    "autorenew": "action",
    "battery_alert": "device",
    "bookmark": "action",
    "build": "action",
    "calendar_month": "action",
    "calendar_today": "action",
    "calendar_view_week": "action",
    "category": "maps",
    "chat_bubble": "communication",
    "check": "navigation",
    "check_box": "toggle",
    "check_box_outline_blank": "toggle",
    "chevron_left": "navigation",
    "chevron_right": "navigation",
    "close": "navigation",
    "content_copy": "content",
    "crop_square": "image",
    "dark_mode": "device",
    "delete": "action",
    "edit": "image",
    "error": "alert",
    "expand_more": "navigation",
    "favorite": "action",
    "favorite_border": "action",
    "format_align_center": "editor",
    "format_align_left": "editor",
    "format_align_right": "editor",
    "home": "action",
    "horizontal_rule": "editor",
    "hourglass_empty": "action",
    "image": "image",
    "inbox": "content",
    "info": "action",
    "keyboard_arrow_down": "hardware",
    "label": "action",
    "light_mode": "device",
    "linear_scale": "editor",
    "list": "action",
    "mail": "content",
    "menu": "navigation",
    "menu_open": "navigation",
    "message": "communication",
    "mic": "av",
    "more_horiz": "navigation",
    "more_vert": "navigation",
    "notifications": "social",
    "palette": "image",
    "radio_button_checked": "toggle",
    "refresh": "navigation",
    "report": "content",
    "save": "content",
    "schedule": "action",
    "search": "action",
    "select_all": "content",
    "send": "content",
    "share": "social",
    "smart_button": "action",
    "space_dashboard": "action",
    "star": "toggle",
    "star_outline": "toggle",
    "system_update": "notification",
    "tab": "action",
    "tag": "content",
    "text_fields": "editor",
    "toggle_on": "toggle",
    "tune": "image",
    "vertical_align_bottom": "editor",
    "vertical_split": "action",
    "videocam": "av",
    "view_carousel": "action",
    "view_column": "action",
    "view_sidebar": "action",
    "view_week": "action",
    "volume_up": "av",
    "web_asset": "av",
}

# Suffix -> upstream style-directory fragment (`materialicons<style>`); order
# matters only in that every suffix here is checked before falling back to
# "no suffix, filled" — none of `BASE_CATEGORY`'s keys themselves end in one
# of these suffixes, so there is no stripping ambiguity.
_VARIANT_STYLE_DIR = {
    "_rounded": "round",
    "_outlined": "outlined",
    "_sharp": "sharp",
}


def split_variant(name: str) -> tuple[str, str]:
    """`check_box_rounded` -> `("check_box", "round")`; `home` -> `("home", "")`."""
    for suffix, style in _VARIANT_STYLE_DIR.items():
        if name.endswith(suffix):
            return name[: -len(suffix)], style
    return name, ""


MODULE_HEADER = '''\
//! Vendored Material Design Icons — GENERATED, do not hand-edit.
//!
//! The {count} distinct glyphs `frust_material`'s catalog and the
//! `material_3_expressive` reference library's example app actually use,
//! each an [`IconSource`](frust::IconSource) carrying an upstream
//! `google/material-design-icons` classic-family path as SVG `d` data in a
//! 24×24 design box. Pass one to [`frust::icon`] (or convert it via
//! [`frust::IconData::from`]) to paint it — access is namespaced
//! (`frust_material::icons::CHECK`), not flat re-exported at the crate root.
//!
//! # Provenance & license
//!
//! `google/material-design-icons` is licensed Apache-2.0 (Google); the path
//! data below is vendored verbatim (coordinates already authored against a
//! `viewBox="0 0 24 24"` box needing no rescale — see
//! `plugins/material/scripts/gen_icons.py`'s module docstring), minus each
//! source SVG's invisible `fill="none"` click-target keyline path.
//!
//! # Regenerating
//!
//! ```text
//! python3 plugins/material/scripts/gen_icons.py
//! cargo fmt -p frust-material
//! ```
//!
//! See `plugins/material/scripts/gen_icons.py` for the fetch/normalize
//! pipeline and its own `ICON_SET`/`BASE_CATEGORY` source-of-truth tables;
//! the trailing `cargo fmt` collapses this generator's raw double blank line
//! before the first entry down to the single blank line this file carries,
//! the same reason the baseline `scripts/gen_icons.py` documents.

use frust::IconSource;

/// The design-box side length every entry below is authored against.
const D: f64 = {design};
'''


# Stdlib-only XXE/entity-expansion mitigation, mirrored from the baseline
# `scripts/gen_icons.py`'s own `_reject_unsafe_xml` (see that script's
# comment for the full rationale) — this script is likewise a maintainer-run,
# local, manual codegen tool, not a network-input-trusting service; the guard
# is defense-in-depth against a poisoned/compromised upstream file.
_UNSAFE_XML_RE = re.compile(rb"<!\s*(DOCTYPE|ENTITY)\b", re.IGNORECASE)


def _reject_unsafe_xml(raw: bytes, svg_path: Path) -> None:
    if _UNSAFE_XML_RE.search(raw):
        sys.exit(
            f"error: {svg_path} contains a DOCTYPE/ENTITY declaration — "
            "refusing to parse (see _reject_unsafe_xml's trusted-input note "
            "in this script)"
        )


def upstream_url(base: str, style: str, category: str) -> str:
    style_dir = f"materialicons{style}"
    return f"{RAW_BASE}/{category}/{base}/{style_dir}/24px.svg"


def fetch_svg(url: str, cache_dir: Path, refresh: bool) -> bytes:
    """Return `url`'s bytes, via `cache_dir` unless `refresh` forces a re-fetch.

    A cache hit means a regen after the first needs no network at all; a miss
    (or `--refresh`) fetches once and writes the cache entry. A fetch failure
    is a hard error — this script never falls back to hand-authored data (see
    module docstring).
    """
    cache_dir.mkdir(parents=True, exist_ok=True)
    cache_key = re.sub(r"[^0-9a-zA-Z]+", "_", url).strip("_")
    cache_path = cache_dir / f"{cache_key}.svg"
    if not refresh and cache_path.exists():
        return cache_path.read_bytes()
    req = urllib.request.Request(url, headers={"User-Agent": "frust-material-gen-icons/1"})
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310
            data = resp.read()
    except urllib.error.URLError as e:
        sys.exit(f"error: failed to fetch {url}: {e}")
    cache_path.write_bytes(data)
    return data


def _is_invisible(elem: ET.Element) -> bool:
    fill = (elem.get("fill") or "").strip().lower()
    if fill == "none":
        return True
    style = (elem.get("style") or "").lower()
    return "fill:none" in style.replace(" ", "")


def _rect_to_d(elem: ET.Element, url: str) -> str:
    """Convert an axis-aligned `<rect>` into an equivalent path `d` string.

    A handful of glyphs (e.g. `horizontal_rule`) draw their actual visible
    geometry as a plain `<rect>` rather than a `<path>` — alongside the usual
    invisible `<rect fill="none">` click-target keyline `_is_invisible`
    already filters out. Rounded corners (`rx`/`ry`) would need an arc
    command this script doesn't otherwise implement, so a rounded `<rect>`
    is a hard error rather than a silent square-corner approximation.
    """
    if elem.get("rx") or elem.get("ry"):
        sys.exit(f"error: {url} has a rounded <rect> (rx/ry) — not supported")
    x = float(elem.get("x") or 0)
    y = float(elem.get("y") or 0)
    w = float(elem.get("width") or 0)
    h = float(elem.get("height") or 0)
    return f"M {_fmt_coord(x)} {_fmt_coord(y)} H {_fmt_coord(x + w)} V {_fmt_coord(y + h)} H {_fmt_coord(x)} Z"


def _fmt_coord(value: float) -> str:
    s = f"{value:.4f}".rstrip("0").rstrip(".")
    return "0" if s in ("", "-0") else s


def extract_path_d(svg_bytes: bytes, design: float, url: str) -> str:
    """Return the concatenated `d` of every visible `<path>`/`<rect>` in an SVG.

    Hard-fails if the document's `viewBox` isn't exactly `0 0 <design>
    <design>` — this script passes coordinates through unscaled (see module
    docstring's "Coordinate convention"), so a differently-scaled asset would
    otherwise silently mis-render.
    """
    _reject_unsafe_xml(svg_bytes, Path(url))
    root = ET.fromstring(svg_bytes)  # noqa: S314 (guarded above)
    view_box = root.get("viewBox")
    expected = f"0 0 {_fmt_f64(design)} {_fmt_f64(design)}"
    expected_int = f"0 0 {int(design)} {int(design)}"
    if view_box is None or view_box.split() not in (expected.split(), expected_int.split()):
        sys.exit(
            f"error: {url} has viewBox {view_box!r}, expected '0 0 {design} {design}' — "
            "this script only passes through an already-{design}x{design} box "
            "(see module docstring's Coordinate convention)"
        )
    ds: list[str] = []
    for elem in root.iter():
        tag = elem.tag.rsplit("}", 1)[-1]  # strip any XML namespace
        if tag not in ("path", "rect") or _is_invisible(elem):
            continue
        if tag == "rect":
            ds.append(_rect_to_d(elem, url))
            continue
        d = elem.get("d")
        if d:
            ds.append(d.strip())
    joined = " ".join(ds)
    if not joined:
        sys.exit(f"error: {url} has no visible <path d=...> data")
    return joined


def escape_rust(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def resolve_entries(
    cache_dir: Path, design: float, refresh: bool
) -> list[tuple[str, str]]:
    """Map (CONST_NAME -> path d) for every `ICON_SET` entry, in order."""
    entries: list[tuple[str, str]] = []
    seen: set[str] = set()
    for name in ICON_SET:
        const_name = name.upper()
        if const_name in seen:
            sys.exit(f"error: duplicate icon name in ICON_SET: {name!r}")
        seen.add(const_name)
        base, style = split_variant(name)
        category = BASE_CATEGORY.get(base)
        if category is None:
            sys.exit(
                f"error: no BASE_CATEGORY entry for {base!r} (from ICON_SET "
                f"entry {name!r}) — add one (see this script's BASE_CATEGORY "
                "docstring for how it was resolved)"
            )
        url = upstream_url(base, style, category)
        svg_bytes = fetch_svg(url, cache_dir, refresh)
        d = extract_path_d(svg_bytes, design, url)
        entries.append((const_name, d))
    return entries


def render_module(entries: list[tuple[str, str]], design: float) -> str:
    out: list[str] = [MODULE_HEADER.format(design=_fmt_f64(design), count=len(entries))]
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
        "--out",
        type=Path,
        default=REPO_ROOT / "plugins" / "material" / "src" / "icons.rs",
        help="output Rust module path",
    )
    ap.add_argument(
        "--cache-dir",
        type=Path,
        default=REPO_ROOT / "target" / "material-icons-cache",
        help="downloaded-SVG cache directory (gitignored, not committed)",
    )
    ap.add_argument(
        "--design",
        type=float,
        default=24.0,
        help="design-box side length (default 24.0)",
    )
    ap.add_argument(
        "--refresh",
        action="store_true",
        help="ignore any cached SVGs and re-fetch every glyph",
    )
    args = ap.parse_args()

    if len(ICON_SET) != len(set(ICON_SET)):
        sys.exit("error: ICON_SET contains a duplicate name")

    entries = resolve_entries(args.cache_dir, args.design, args.refresh)
    module = render_module(entries, args.design)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(module, encoding="utf-8")
    print(f"wrote {len(entries)} icons to {args.out}")


if __name__ == "__main__":
    main()
