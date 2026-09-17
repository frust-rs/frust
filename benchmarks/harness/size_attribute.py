#!/usr/bin/env python3
"""size_attribute.py — per-crate / per-section size attribution for an
UNSTRIPPED Frust Android release `.so`.

`benchmarks/harness/app_size.sh` reports whole-artifact sizes only. This
script answers the follow-up question — *which crate, which ELF section,
which .rodata payload* the bytes belong to — from the same unstripped `.so`
that produced them, by shelling out to the Android NDK's `llvm-readelf` and
`llvm-nm` (never building anything itself).

Build the input this script expects with (from `benchmarks/frust_bench`):

    CARGO_PROFILE_RELEASE_STRIP=false \\
      cargo ndk -t arm64-v8a build --release --no-default-features --features lean

(`frust build` has no `--no-default-features` knob, so this NDK invocation
is the direct route to an unstripped lean release `.so`.)

Usage:
    size_attribute.py <unstripped.so> [--nm-dir DIR] [--compare OTHER.so]

`--nm-dir DIR` points at a directory containing `llvm-readelf`/`llvm-nm`
(typically an NDK `toolchains/llvm/prebuilt/<host>/bin`). When omitted, the
tools are located via `ANDROID_NDK_HOME`, then the highest-versioned NDK
under `ANDROID_HOME`/`ANDROID_SDK_ROOT`'s `ndk/` directory, then `PATH`.

`--compare OTHER.so` additionally attributes a second `.so` and prints the
per-family byte delta (this file minus the other), sorted by |delta|. The
delta uses the same deduplicated per-family totals the main report prints
(see "Symbol de-duplication" below), never the raw, alias-inflated sums.

# Symbol de-duplication

`llvm-nm --print-size --demangle` prints one line per symbol *name* at an
address, not one line per byte range: several names can share the exact
same `(address, size)` region (compiler-generated aliases today; far more
once link-time identical-code-folding is enabled). Summing `st_size` over
every name double-counts that region once per alias. This script instead
groups symbols by `(address, size)` and attributes each unique region
exactly once, using the first name encountered in `llvm-nm`'s own output
order for classification. The count of collapsed groups and the bytes that
would otherwise have been double-counted are reported as a summary line
next to the per-family table.

A symbol whose size llvm-nm cannot report (`st_size == 0`, or an undefined
symbol with no address at all) attributes 0 bytes but is still counted, in
a "sizeless symbols" tally, instead of being silently dropped.

# Per-section remainder

For each headline ELF section, the report prints
`section bytes − attributed bytes = unattributed remainder`, using the
deduplicated symbol bytes whose address falls inside that section, so a
short or inflated attribution total is visible instead of implied.

Standard library only — no fontTools, no Pillow, no third-party package.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import struct
import subprocess
import sys
from dataclasses import dataclass, field

# --- ELF sections the app-size plan calls out by name -----------------------

HEADLINE_SECTIONS = [".text", ".rodata", ".eh_frame", ".rela.dyn", ".data.rel.ro"]

# --- C-library symbol families (no Rust mangling, so the demangled name is
# the raw name) — recognised by their conventional prefixes.
C_FAMILY_PREFIXES = [
    ("sqlite3", "sqlite3"),
    ("png_", "png"),
    ("png", "png"),
    ("inflate", "zlib"),
    ("deflate", "zlib"),
    ("crc32", "zlib"),
    ("adler32", "zlib"),
    ("compress2", "zlib"),
    ("uncompress", "zlib"),
    ("zlibVersion", "zlib"),
    ("zcalloc", "zlib"),
    ("zcfree", "zlib"),
    ("gzread", "zlib"),
    ("gzwrite", "zlib"),
    ("gzopen", "zlib"),
    ("tinfl_", "zlib"),
    ("tdefl_", "zlib"),
    ("mz_", "zlib"),
]

ANON_FAMILY = "anon."

# sfnt table-directory magic numbers (OpenType spec).
SFNT_MAGICS = (b"\x00\x01\x00\x00", b"OTTO", b"true", b"ttcf")

# Panic/error phrasing this codebase's `panic!`/`assert!`/std messages use —
# enough to bucket the common cases without a full NLP pass.
PANIC_KEYWORDS = (
    "assertion failed",
    "assertion `",
    "index out of bounds",
    "already borrowed",
    "already mutably borrowed",
    "attempt to",
    "capacity overflow",
    "out of memory",
    "called `Option::",
    "called `Result::",
    "not implemented",
    "unreachable",
    "slice index",
    "byte index",
    "overflow",
    "underflow",
    "panicked at",
    "invalid ",
    "failed to",
    "expected ",
)


def to_mib(num_bytes: int) -> str:
    return f"{num_bytes / 1048576:.2f} MiB"


def fmt_bytes(num_bytes: int) -> str:
    return f"{num_bytes:,} B ({to_mib(num_bytes)})"


# --- Tool discovery -----------------------------------------------------


def _version_key(name: str):
    return [int(p) if p.isdigit() else p for p in re.split(r"[.\-]", name)]


def find_llvm_bin_dir(explicit: str | None) -> str | None:
    """Return a directory containing both llvm-readelf and llvm-nm, or None."""

    def has_tools(d: str) -> bool:
        return os.path.isfile(os.path.join(d, "llvm-readelf")) and os.path.isfile(
            os.path.join(d, "llvm-nm")
        )

    if explicit:
        if has_tools(explicit):
            return explicit
        return None

    candidates: list[str] = []

    ndk_home = os.environ.get("ANDROID_NDK_HOME") or os.environ.get("ANDROID_NDK_ROOT")
    if ndk_home:
        prebuilt = os.path.join(ndk_home, "toolchains", "llvm", "prebuilt")
        if os.path.isdir(prebuilt):
            for sub in sorted(os.listdir(prebuilt)):
                candidates.append(os.path.join(prebuilt, sub, "bin"))

    for env_var in ("ANDROID_HOME", "ANDROID_SDK_ROOT"):
        sdk_root = os.environ.get(env_var)
        if not sdk_root:
            continue
        ndk_root = os.path.join(sdk_root, "ndk")
        if not os.path.isdir(ndk_root):
            continue
        versions = [
            v for v in os.listdir(ndk_root) if os.path.isdir(os.path.join(ndk_root, v))
        ]
        try:
            versions.sort(key=_version_key)
        except TypeError:
            versions.sort()
        for version in reversed(versions):
            prebuilt = os.path.join(ndk_root, version, "toolchains", "llvm", "prebuilt")
            if not os.path.isdir(prebuilt):
                continue
            for sub in sorted(os.listdir(prebuilt)):
                candidates.append(os.path.join(prebuilt, sub, "bin"))

    for candidate in candidates:
        if has_tools(candidate):
            return candidate

    # PATH fallback: only usable if both tools are directly on PATH.
    readelf_on_path = shutil.which("llvm-readelf")
    nm_on_path = shutil.which("llvm-nm")
    if readelf_on_path and nm_on_path:
        return os.path.dirname(readelf_on_path)

    return None


# --- ELF section table (via llvm-readelf) --------------------------------


@dataclass
class Section:
    name: str
    addr: int
    offset: int
    size: int
    flags: str = ""

    @property
    def is_allocated(self) -> bool:
        """SHF_ALLOC — the `A` flag in `readelf -S`'s Flg column: this
        section occupies memory in the loaded image, as opposed to a
        debug/symtab section that exists only in the unstripped file on
        disk."""
        return "A" in self.flags


# `[Nr] Name Type Address Off Size ES Flg Lk Inf Al` (llvm-readelf -S -W).
# The flags column is a run of letters (e.g. "AX", "WAT", "AMS") and is
# blank for a non-allocated section — restricting it to `[A-Za-z]*` (rather
# than `\S*`) stops it from swallowing the numeric Lk column when blank.
SECTION_LINE_RE = re.compile(
    r"^\s*\[\s*\d+\]\s+(?P<name>\S*)\s+(?P<type>\S+)\s+"
    r"(?P<addr>[0-9a-fA-F]+)\s+(?P<off>[0-9a-fA-F]+)\s+(?P<size>[0-9a-fA-F]+)\s+"
    r"(?P<es>[0-9a-fA-F]+)\s+(?P<flags>[A-Za-z]*)\b"
)


def parse_section_lines(text: str) -> list[Section]:
    """Parse `llvm-readelf -S -W` output text into `Section`s. Pure function
    over already-captured text, so it is testable without the NDK."""
    sections: list[Section] = []
    for line in text.splitlines():
        m = SECTION_LINE_RE.match(line)
        if not m or not m.group("name"):
            continue
        sections.append(
            Section(
                name=m.group("name"),
                addr=int(m.group("addr"), 16),
                offset=int(m.group("off"), 16),
                size=int(m.group("size"), 16),
                flags=m.group("flags"),
            )
        )
    return sections


def read_sections(readelf: str, so_path: str) -> list[Section]:
    out = subprocess.run(
        [readelf, "-S", "-W", so_path],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return parse_section_lines(out)


# --- Symbol table (via llvm-nm) -------------------------------------------


@dataclass
class Symbol:
    addr: int
    size: int
    sym_type: str
    name: str
    has_size: bool = True


# The three line shapes `llvm-nm --print-size --demangle` prints:
#   1. `<addr> <size> <type> <name>`   — defined, size known (the common case)
#   2. `<addr> <type> <name>`         — defined, but `st_size == 0`: llvm-nm
#      prints no size column at all rather than an explicit zero.
#   3. `<type> <name>`, right-padded with spaces where the address/size
#      columns would be — undefined (no address, no size), e.g. `U`/`w`.
# Tried in this order so a genuine (addr, size) pair is never mistaken for
# (addr, type) — restricting the type group to letters/`?` (nm's symbol-type
# alphabet, never a digit) keeps a lone hex-letter size from being confused
# with a type code.
NM_TYPE_CHAR = r"[A-Za-z?]"
NM_LINE_FULL_RE = re.compile(
    r"^(?P<addr>[0-9a-fA-F]{1,16})\s+(?P<size>[0-9a-fA-F]{1,16})\s+"
    rf"(?P<type>{NM_TYPE_CHAR})\s+(?P<name>.+)$"
)
NM_LINE_ADDR_ONLY_RE = re.compile(
    rf"^(?P<addr>[0-9a-fA-F]{{1,16}})\s+(?P<type>{NM_TYPE_CHAR})\s+(?P<name>.+)$"
)
NM_LINE_NO_ADDR_RE = re.compile(rf"^\s+(?P<type>{NM_TYPE_CHAR})\s+(?P<name>.+)$")


def parse_symbol_lines(text: str) -> list[Symbol]:
    """Parse `llvm-nm --print-size --demangle` output text into `Symbol`s,
    including size-less lines (`has_size=False`, size reported as 0) instead
    of dropping them. Pure function over already-captured text, so it is
    testable without the NDK."""
    symbols: list[Symbol] = []
    for line in text.splitlines():
        if not line.strip():
            continue
        m = NM_LINE_FULL_RE.match(line)
        if m:
            symbols.append(
                Symbol(
                    addr=int(m.group("addr"), 16),
                    size=int(m.group("size"), 16),
                    sym_type=m.group("type"),
                    name=m.group("name"),
                    has_size=True,
                )
            )
            continue
        m = NM_LINE_ADDR_ONLY_RE.match(line)
        if m:
            symbols.append(
                Symbol(
                    addr=int(m.group("addr"), 16),
                    size=0,
                    sym_type=m.group("type"),
                    name=m.group("name"),
                    has_size=False,
                )
            )
            continue
        m = NM_LINE_NO_ADDR_RE.match(line)
        if m:
            symbols.append(
                Symbol(addr=0, size=0, sym_type=m.group("type"), name=m.group("name"), has_size=False)
            )
            continue
        # Header/noise line (no known shape) — ignored, same as before.
    return symbols


def read_symbols(nm: str, so_path: str) -> list[Symbol]:
    out = subprocess.run(
        [nm, "--print-size", "--demangle", so_path],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return parse_symbol_lines(out)


# --- De-duplication: one attribution per unique (address, size) region ----


@dataclass
class DedupResult:
    symbols: list[Symbol]
    alias_groups: int
    alias_suppressed_bytes: int
    sizeless_count: int


def dedup_symbols(symbols: list[Symbol]) -> DedupResult:
    """Collapse symbols sharing an exact `(address, size)` region into one
    attribution, keeping the first name seen (in `llvm-nm`'s own order) for
    classification. Symbols with `size <= 0` (including every size-less
    line) are excluded from the deduplicated list and instead counted in
    `sizeless_count`."""
    groups: dict[tuple[int, int], list[Symbol]] = {}
    order: list[tuple[int, int]] = []
    sizeless_count = 0
    for sym in symbols:
        if not sym.has_size or sym.size <= 0:
            sizeless_count += 1
            continue
        key = (sym.addr, sym.size)
        if key not in groups:
            groups[key] = []
            order.append(key)
        groups[key].append(sym)

    deduped: list[Symbol] = []
    alias_groups = 0
    alias_suppressed_bytes = 0
    for key in order:
        members = groups[key]
        deduped.append(members[0])
        if len(members) > 1:
            alias_groups += 1
            alias_suppressed_bytes += key[1] * (len(members) - 1)

    return DedupResult(
        symbols=deduped,
        alias_groups=alias_groups,
        alias_suppressed_bytes=alias_suppressed_bytes,
        sizeless_count=sizeless_count,
    )


# --- Crate/family classification -----------------------------------------

PLAIN_PATH_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)::")

_PRIMITIVE_TYPES = {
    "u8", "u16", "u32", "u64", "u128", "usize",
    "i8", "i16", "i32", "i64", "i128", "isize",
    "f32", "f64", "bool", "char", "str",
}

_REF_PTR_PREFIX_RE = re.compile(r"^(&(mut\s+)?|\*(const|mut)\s+|dyn\s+)+")
_LEADING_IDENT_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)")


def _crate_from_type_expr(type_expr: str) -> str | None:
    """Extract a crate name from a Rust type expression's leading segment.

    `Vec<naga::Foo>` -> `Vec`'s own crate is `alloc`, not `naga` — this only
    looks at the *outermost* identifier, matching how `<Type as Trait>::m`
    symbols name the concrete type whose code was generated.
    """
    t = _REF_PTR_PREFIX_RE.sub("", type_expr.strip())
    if not t or t[0] in "([{\"'0123456789":
        return None
    m = _LEADING_IDENT_RE.match(t)
    if not m:
        return None
    ident = m.group(1)
    if ident in _PRIMITIVE_TYPES:
        return None
    return ident


def _split_impl_block(name: str) -> tuple[str, str | None, str] | None:
    """Split a leading `<Type as Trait>::rest` or `<Type>::rest` block.

    Bracket-depth aware, since `Type`/`Trait` are themselves free to contain
    generics (`<HashMap<K, V> as Clone>::clone`). Returns
    `(type_expr, trait_expr_or_None, rest)`, or None if `name` doesn't open
    with a balanced `<...>::` block.
    """
    if not name.startswith("<"):
        return None
    depth = 0
    close = -1
    for i, c in enumerate(name):
        if c == "<":
            depth += 1
        elif c == ">":
            depth -= 1
            if depth == 0:
                close = i
                break
    if close == -1 or not name[close + 1 :].startswith("::"):
        return None
    inner = name[1:close]
    rest = name[close + 3 :]

    depth = 0
    as_idx = -1
    j = 0
    while j < len(inner):
        c = inner[j]
        if c == "<":
            depth += 1
        elif c == ">":
            depth -= 1
        elif depth == 0 and inner[j : j + 4] == " as ":
            as_idx = j
            break
        j += 1

    if as_idx == -1:
        return inner, None, rest
    return inner[:as_idx], inner[as_idx + 4 :], rest


def classify_family(demangled_name: str) -> str:
    name = demangled_name.strip()
    if not name:
        return ANON_FAMILY

    # `<Type as Trait>::method` / `<Type>::method` — attribute to the
    # concrete Type's crate first (the code was generated for that type),
    # falling back to the Trait's crate (e.g. a primitive `impl`).
    impl_block = _split_impl_block(name)
    if impl_block is not None:
        type_expr, trait_expr, _rest = impl_block
        crate = _crate_from_type_expr(type_expr)
        if crate:
            return crate
        if trait_expr:
            crate = _crate_from_type_expr(trait_expr)
            if crate:
                return crate
        return ANON_FAMILY

    m = PLAIN_PATH_RE.match(name)
    if m:
        return m.group(1)

    for prefix, family in C_FAMILY_PREFIXES:
        if name.startswith(prefix):
            return family

    return ANON_FAMILY


def symbol_family_totals(symbols: list[Symbol]) -> dict[str, int]:
    """Sum bytes per crate/family. `symbols` is expected to already be
    deduplicated (see `dedup_symbols`) — this no longer filters `size <= 0`
    itself beyond trusting that contract, so double-counted alias bytes
    never reach this table."""
    totals: dict[str, int] = {}
    for sym in symbols:
        if sym.size <= 0:
            continue
        family = classify_family(sym.name)
        totals[family] = totals.get(family, 0) + sym.size
    return totals


# --- Per-section attribution (uses deduplicated symbols) -------------------


def attributed_bytes_by_section(
    sections: list[Section], deduped_symbols: list[Symbol]
) -> dict[str, int]:
    """For each section, sum the deduplicated symbol bytes whose address
    falls inside it — the figure the per-section remainder line subtracts
    from the section's own reported size."""
    totals = {s.name: 0 for s in sections}
    for sym in deduped_symbols:
        for s in sections:
            if s.addr <= sym.addr < s.addr + s.size:
                totals[s.name] += sym.size
                break
    return totals


# --- .rodata string + font accounting --------------------------------------

# A "printable string" run: bytes in the printable ASCII range, NUL-terminated
# (the C-string convention Rust's rodata literals also follow), length >= 12.
PRINTABLE_RUN_RE = re.compile(rb"[\x20-\x7e]{12,}")

WGSL_TOKENS = ("@vertex", "@fragment", "@compute", "@group(", "@binding(", "var<", "-> vec", "array<")


def classify_string(text: str) -> str:
    if any(tok in text for tok in WGSL_TOKENS):
        return "wgsl"
    if text.endswith(".rs") or "/src/" in text or ".rs:" in text or "registry/src" in text:
        return "source_paths"
    if any(kw in text for kw in PANIC_KEYWORDS):
        return "panic_messages"
    return "other"


@dataclass
class RodataAccounting:
    named_symbol_count: int = 0
    named_symbol_bytes: int = 0
    string_counts: dict[str, int] = field(default_factory=dict)
    string_bytes: dict[str, int] = field(default_factory=dict)
    fonts: list[tuple[int, int, str]] = field(default_factory=list)


def find_sfnt_fonts(data: bytes) -> list[tuple[int, int]]:
    """Return (offset, length) for each sfnt blob detected in `data`."""
    hits: list[tuple[int, int]] = []
    for magic in SFNT_MAGICS:
        start = 0
        while True:
            idx = data.find(magic, start)
            if idx == -1:
                break
            start = idx + 1
            length = _parse_sfnt_at(data, idx)
            if length is not None:
                hits.append((idx, length))
    hits.sort()
    # Drop detections that fall inside an already-accepted blob (a font's own
    # table data can incidentally contain another magic).
    deduped: list[tuple[int, int]] = []
    last_end = -1
    for off, length in hits:
        if off < last_end:
            continue
        deduped.append((off, length))
        last_end = off + length
    return deduped


def _parse_sfnt_at(data: bytes, idx: int) -> int | None:
    n = len(data)
    if idx + 12 > n:
        return None
    try:
        num_tables = struct.unpack_from(">H", data, idx + 4)[0]
    except struct.error:
        return None
    if num_tables == 0 or num_tables > 128:
        return None
    table_dir_end = idx + 12 + 16 * num_tables
    if table_dir_end > n:
        return None
    max_end = 12 + 16 * num_tables
    for t in range(num_tables):
        rec_off = idx + 12 + t * 16
        try:
            tag, _checksum, tbl_offset, tbl_length = struct.unpack_from(">4sIII", data, rec_off)
        except struct.error:
            return None
        if not all(0x20 <= b < 0x7F for b in tag):
            return None
        end = tbl_offset + tbl_length
        if end > max_end:
            max_end = end
    # Table data is 4-byte aligned (OpenType spec); embedders commonly pad
    # the whole blob's tail to the same boundary, so round up to match.
    max_end = (max_end + 3) & ~3
    return min(max_end, n - idx)


def rodata_accounting(
    section: Section, file_bytes: bytes, deduped_symbols: list[Symbol]
) -> RodataAccounting:
    acc = RodataAccounting()
    lo, hi = section.addr, section.addr + section.size
    for sym in deduped_symbols:
        if sym.size <= 0:
            continue
        if lo <= sym.addr < hi:
            acc.named_symbol_count += 1
            acc.named_symbol_bytes += sym.size

    raw = file_bytes[section.offset : section.offset + section.size]
    for m in PRINTABLE_RUN_RE.finditer(raw):
        text = m.group(0).decode("ascii", errors="replace")
        bucket = classify_string(text)
        acc.string_counts[bucket] = acc.string_counts.get(bucket, 0) + 1
        acc.string_bytes[bucket] = acc.string_bytes.get(bucket, 0) + len(text)

    named_by_addr = sorted(
        (s.addr, s.addr + s.size, s.name)
        for s in deduped_symbols
        if lo <= s.addr < hi and s.size > 0
    )

    def label_for(offset_in_section: int) -> str:
        addr = section.addr + offset_in_section
        for start, end, name in named_by_addr:
            if start <= addr < end:
                return name
        return f"unnamed@0x{addr:x}"

    for off, length in find_sfnt_fonts(raw):
        acc.fonts.append((section.offset + off, length, label_for(off)))

    return acc


# --- Per-.so attribution pass ------------------------------------------


@dataclass
class Attribution:
    so_path: str
    file_size: int
    sections: list[Section]
    section_attributed: dict[str, int]
    family_totals: dict[str, int]
    dedup: DedupResult
    rodata: RodataAccounting | None
    allocated_total: int
    non_allocated_total: int


def attribute(readelf: str, nm: str, so_path: str) -> Attribution:
    with open(so_path, "rb") as f:
        file_bytes = f.read()

    sections = read_sections(readelf, so_path)
    raw_symbols = read_symbols(nm, so_path)

    dedup = dedup_symbols(raw_symbols)
    family_totals = symbol_family_totals(dedup.symbols)
    section_attributed = attributed_bytes_by_section(sections, dedup.symbols)

    allocated_total = sum(s.size for s in sections if s.is_allocated)
    non_allocated_total = sum(s.size for s in sections if not s.is_allocated)

    rodata_section = next((s for s in sections if s.name == ".rodata"), None)
    rodata = (
        rodata_accounting(rodata_section, file_bytes, dedup.symbols)
        if rodata_section
        else None
    )

    return Attribution(
        so_path=so_path,
        file_size=len(file_bytes),
        sections=sections,
        section_attributed=section_attributed,
        family_totals=family_totals,
        dedup=dedup,
        rodata=rodata,
        allocated_total=allocated_total,
        non_allocated_total=non_allocated_total,
    )


# --- Reporting ------------------------------------------------------------


def print_report(attribution: Attribution) -> None:
    print(f"== Size attribution: {attribution.so_path} ==")
    print(f"File size: {fmt_bytes(attribution.file_size)}")
    print()

    section_sizes = {s.name: s.size for s in attribution.sections}

    print("-- ELF sections --")
    for name in HEADLINE_SECTIONS:
        size = section_sizes.get(name)
        if size is None:
            print(f"  {name:<16} not present")
            continue
        attributed = attribution.section_attributed.get(name, 0)
        remainder = size - attributed
        print(f"  {name:<16} {fmt_bytes(size)}")
        print(
            f"    {'attributed':<14} {fmt_bytes(attributed)}  "
            f"remainder {remainder:,} B"
        )
    other_total = sum(
        size for name, size in section_sizes.items() if name not in HEADLINE_SECTIONS
    )
    print(f"  {'(other sections)':<16} {fmt_bytes(other_total)}")
    print(f"  {'Allocated total (SHF_ALLOC)':<16} {fmt_bytes(attribution.allocated_total)}")
    print(
        f"  {'Non-allocated total (debug/symtab — unstripped-only)':<16} "
        f"{fmt_bytes(attribution.non_allocated_total)}"
    )
    print()

    print("-- Symbol bytes by crate/family (llvm-nm --print-size --demangle) --")
    attributed_total = sum(attribution.family_totals.values())
    for family, size in sorted(
        attribution.family_totals.items(), key=lambda kv: kv[1], reverse=True
    ):
        print(f"  {family:<24} {fmt_bytes(size)}")
    print(f"  {'(attributed total)':<24} {fmt_bytes(attributed_total)}")
    dedup = attribution.dedup
    print(
        f"  {'(alias groups collapsed)':<24} {dedup.alias_groups:,} groups, "
        f"{fmt_bytes(dedup.alias_suppressed_bytes)} suppressed"
    )
    print(f"  {'(sizeless symbols)':<24} {dedup.sizeless_count:,} symbols, 0 B")
    print()

    if attribution.rodata is not None:
        acc = attribution.rodata
        print("-- .rodata accounting --")
        print(
            f"  Named data symbols: {acc.named_symbol_count} symbols, "
            f"{fmt_bytes(acc.named_symbol_bytes)}"
        )
        print("  Printable strings (>= 12 chars):")
        for bucket in ("wgsl", "source_paths", "panic_messages", "other"):
            count = acc.string_counts.get(bucket, 0)
            size = acc.string_bytes.get(bucket, 0)
            print(f"    {bucket:<16} {count:>6} strings, {fmt_bytes(size)}")
        print()
        if acc.fonts:
            print("  Embedded sfnt fonts detected:")
            for offset, length, label in acc.fonts:
                print(f"    offset=0x{offset:x} length={fmt_bytes(length)} name={label}")
        else:
            print("  Embedded sfnt fonts detected: none")
        print()


def print_compare(base: Attribution, other: Attribution) -> None:
    print(f"-- Per-family delta: {base.so_path} minus {other.so_path} --")
    families = set(base.family_totals) | set(other.family_totals)
    deltas = []
    for family in families:
        a = base.family_totals.get(family, 0)
        b = other.family_totals.get(family, 0)
        deltas.append((family, a - b, a, b))
    deltas.sort(key=lambda t: abs(t[1]), reverse=True)
    for family, delta, a, b in deltas:
        sign = "+" if delta >= 0 else "-"
        print(f"  {family:<24} {sign}{abs(delta):,} B  ({a:,} B vs {b:,} B)")
    print()


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="size_attribute.py",
        description=(
            "Per-crate and per-ELF-section size attribution for an UNSTRIPPED "
            "Frust Android release .so — the follow-up detail behind "
            "benchmarks/harness/app_size.sh's whole-artifact numbers."
        ),
    )
    parser.add_argument("so_path", help="Path to the unstripped .so to attribute.")
    parser.add_argument(
        "--nm-dir",
        default=None,
        help=(
            "Directory containing llvm-readelf/llvm-nm (e.g. an NDK "
            "toolchains/llvm/prebuilt/<host>/bin). Defaults to searching "
            "ANDROID_NDK_HOME, then ANDROID_HOME/ANDROID_SDK_ROOT's ndk/ "
            "directory (highest version), then PATH."
        ),
    )
    parser.add_argument(
        "--compare",
        default=None,
        metavar="OTHER.so",
        help="A second unstripped .so to attribute and diff per-family against.",
    )
    return parser


def main(argv: list[str]) -> int:
    parser = build_arg_parser()
    args = parser.parse_args(argv)

    if not os.path.isfile(args.so_path):
        print(f"error: no such file: {args.so_path}", file=sys.stderr)
        return 2
    if args.compare is not None and not os.path.isfile(args.compare):
        print(f"error: no such file: {args.compare}", file=sys.stderr)
        return 2

    bin_dir = find_llvm_bin_dir(args.nm_dir)
    if bin_dir is None:
        print(
            "error: could not locate llvm-readelf/llvm-nm — pass --nm-dir, or "
            "set ANDROID_NDK_HOME / ANDROID_HOME (with an ndk/ subdirectory)",
            file=sys.stderr,
        )
        return 1
    readelf = os.path.join(bin_dir, "llvm-readelf")
    nm = os.path.join(bin_dir, "llvm-nm")

    base = attribute(readelf, nm, args.so_path)
    print_report(base)

    if args.compare is not None:
        other = attribute(readelf, nm, args.compare)
        print_compare(base, other)

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
