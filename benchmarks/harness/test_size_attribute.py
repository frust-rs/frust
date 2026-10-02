#!/usr/bin/env python3
"""benchmarks/harness/test_size_attribute.py — plain-`unittest`, fixture-driven
tests for `size_attribute.py`.

No third-party test runner and no NDK/`.so` needed — every parser under test
takes already-captured `llvm-readelf -S -W` / `llvm-nm --print-size
--demangle` text (or synthetic bytes), matching the harness's "no pip access
guaranteed, no build required" constraint (see `test_stats.py`, whose style
this mirrors).

Runnable either via `python3 -m unittest benchmarks.harness.test_size_attribute`
from the repo root, or `python3 benchmarks/harness/test_size_attribute.py [-v]`
directly from any directory.
"""

from __future__ import annotations

import struct
import unittest

try:
    import size_attribute
except ImportError:  # running as `python3 -m unittest benchmarks.harness.test_size_attribute`
    from . import size_attribute


# --- Section-table parsing --------------------------------------------------

# A trimmed `llvm-readelf -S -W` excerpt: the header rows this parser must
# ignore, a NULL entry (blank name — skipped), two allocated PROGBITS
# sections with different flag combinations (`AX` code, `AMS` mergeable
# strings), one allocated read-write section (`WA`), and one non-allocated
# debug section whose Flg column is blank — the case the flags regex must
# not let swallow the numeric Lk column.
SECTION_TABLE_TEXT = """\
There are 5 section headers, starting at offset 0x100:

Section Headers:
  [Nr] Name              Type            Address          Off    Size   ES Flg Lk Inf Al
  [ 0]                   NULL            0000000000000000 000000 000000 00      0   0  0
  [ 1] .text             PROGBITS        0000000000001000 001000 00000a00 00  AX  0   0 16
  [ 2] .rodata           PROGBITS        0000000000002000 002000 00000500 00 AMS  0   0  8
  [ 3] .data.rel.ro      PROGBITS        0000000000003000 003000 00000100 00  WA  0   0  8
  [ 4] .debug_info       PROGBITS        0000000000000000 004000 00000200 00      0   0  1
"""


class ParseSectionLinesTests(unittest.TestCase):
    def test_parses_every_named_section(self):
        # Index 0's row prints no name at all (just `NULL` in the Type
        # column) — a token-based parse has no fixed column widths to lean
        # on, so it reads as a zero-sized, zero-addressed, flagless section
        # literally named "NULL"; harmless since every numeric field for
        # that row genuinely is 0, and it is never a headline section.
        sections = size_attribute.parse_section_lines(SECTION_TABLE_TEXT)
        self.assertEqual(
            [s.name for s in sections],
            ["NULL", ".text", ".rodata", ".data.rel.ro", ".debug_info"],
        )

    def test_addr_offset_size_are_parsed_as_hex(self):
        sections = size_attribute.parse_section_lines(SECTION_TABLE_TEXT)
        text = next(s for s in sections if s.name == ".text")
        self.assertEqual(text.addr, 0x1000)
        self.assertEqual(text.offset, 0x1000)
        self.assertEqual(text.size, 0xA00)

    def test_flag_combinations_are_captured(self):
        sections = size_attribute.parse_section_lines(SECTION_TABLE_TEXT)
        by_name = {s.name: s for s in sections}
        self.assertEqual(by_name[".text"].flags, "AX")
        self.assertEqual(by_name[".rodata"].flags, "AMS")
        self.assertEqual(by_name[".data.rel.ro"].flags, "WA")

    def test_blank_flags_column_does_not_swallow_the_link_field(self):
        # `.debug_info`'s Flg column is empty; a hungry `\S*` capture would
        # eat the "0" Lk field instead of leaving it blank.
        sections = size_attribute.parse_section_lines(SECTION_TABLE_TEXT)
        debug = next(s for s in sections if s.name == ".debug_info")
        self.assertEqual(debug.flags, "")

    def test_is_allocated_reflects_the_a_flag(self):
        sections = size_attribute.parse_section_lines(SECTION_TABLE_TEXT)
        by_name = {s.name: s for s in sections}
        self.assertTrue(by_name[".text"].is_allocated)
        self.assertTrue(by_name[".rodata"].is_allocated)
        self.assertTrue(by_name[".data.rel.ro"].is_allocated)
        self.assertFalse(by_name[".debug_info"].is_allocated)

    def test_header_and_blank_lines_produce_no_sections(self):
        sections = size_attribute.parse_section_lines(
            "There are 5 section headers, starting at offset 0x100:\n\n"
            "Section Headers:\n"
        )
        self.assertEqual(sections, [])


# --- Symbol-table parsing ----------------------------------------------------

# Three shapes `llvm-nm --print-size --demangle` prints, in the priority
# order the parser tries them: a full `addr size type name` line, an
# `addr type name` line for a defined `st_size == 0` symbol (size column
# entirely absent, not a zero), and a right-padded `type name` line for an
# undefined symbol (no address at all).
NM_LINES_TEXT = (
    "0000000000001000 0000000000000010 T naga::valid::Validator::validate\n"
    "0000000000001000 0000000000000010 t alias_of_validate\n"
    "0000000000001020 0000000000000020 T sqlite3_exec\n"
    "0000000000001040 t some_local_symbol\n"
    "                  U extern_symbol\n"
    "0000000000001060 0000000000000005 T anon_thing\n"
)


class ParseSymbolLinesTests(unittest.TestCase):
    def test_parses_full_addr_size_type_name_lines(self):
        symbols = size_attribute.parse_symbol_lines(NM_LINES_TEXT)
        first = symbols[0]
        self.assertEqual(first.addr, 0x1000)
        self.assertEqual(first.size, 0x10)
        self.assertEqual(first.sym_type, "T")
        self.assertEqual(first.name, "naga::valid::Validator::validate")
        self.assertTrue(first.has_size)

    def test_addr_only_size_less_line_reports_zero_size_and_has_size_false(self):
        symbols = size_attribute.parse_symbol_lines(NM_LINES_TEXT)
        sizeless_defined = next(s for s in symbols if s.name == "some_local_symbol")
        self.assertEqual(sizeless_defined.addr, 0x1040)
        self.assertEqual(sizeless_defined.size, 0)
        self.assertEqual(sizeless_defined.sym_type, "t")
        self.assertFalse(
            sizeless_defined.has_size,
            "an st_size==0 symbol has no size column at all, not an explicit zero",
        )

    def test_undefined_line_with_no_address_is_parsed_not_dropped(self):
        symbols = size_attribute.parse_symbol_lines(NM_LINES_TEXT)
        undefined = next(s for s in symbols if s.name == "extern_symbol")
        self.assertEqual(undefined.sym_type, "U")
        self.assertFalse(undefined.has_size)
        self.assertEqual(undefined.size, 0)

    def test_every_line_in_the_fixture_is_accounted_for(self):
        symbols = size_attribute.parse_symbol_lines(NM_LINES_TEXT)
        self.assertEqual(len(symbols), 6)

    def test_blank_lines_are_ignored(self):
        symbols = size_attribute.parse_symbol_lines("\n\n" + NM_LINES_TEXT + "\n\n")
        self.assertEqual(len(symbols), 6)


# --- De-duplication ----------------------------------------------------------


class DedupSymbolsTests(unittest.TestCase):
    def test_aliased_addr_size_pair_collapses_to_one_attribution(self):
        symbols = size_attribute.parse_symbol_lines(NM_LINES_TEXT)
        result = size_attribute.dedup_symbols(symbols)
        # naga::valid::Validator::validate and alias_of_validate share
        # (0x1000, 0x10) — one group, one kept name, one region's worth of
        # bytes attributed.
        self.assertEqual(result.alias_groups, 1)
        self.assertEqual(result.alias_suppressed_bytes, 0x10)
        names = [s.name for s in result.symbols]
        self.assertIn("naga::valid::Validator::validate", names)
        self.assertNotIn("alias_of_validate", names, "only the first alias name is kept")

    def test_first_seen_name_is_kept_for_classification(self):
        symbols = [
            size_attribute.Symbol(addr=0x2000, size=8, sym_type="T", name="first_name"),
            size_attribute.Symbol(addr=0x2000, size=8, sym_type="t", name="second_name"),
        ]
        result = size_attribute.dedup_symbols(symbols)
        self.assertEqual([s.name for s in result.symbols], ["first_name"])

    def test_sizeless_symbols_are_tallied_not_attributed(self):
        symbols = size_attribute.parse_symbol_lines(NM_LINES_TEXT)
        result = size_attribute.dedup_symbols(symbols)
        self.assertEqual(result.sizeless_count, 2, "some_local_symbol + extern_symbol")
        self.assertEqual(
            sum(s.size for s in result.symbols),
            0x10 + 0x20 + 0x05,
            "sizeless lines contribute 0 B, never a guessed size",
        )

    def test_deduped_symbol_count_matches_unique_regions(self):
        symbols = size_attribute.parse_symbol_lines(NM_LINES_TEXT)
        result = size_attribute.dedup_symbols(symbols)
        # 4 size>0 lines, one aliased pair -> 3 unique (addr, size) regions.
        self.assertEqual(len(result.symbols), 3)

    def test_no_aliasing_reports_zero_groups_and_zero_suppressed(self):
        symbols = [
            size_attribute.Symbol(addr=0x10, size=4, sym_type="T", name="a"),
            size_attribute.Symbol(addr=0x20, size=4, sym_type="T", name="b"),
        ]
        result = size_attribute.dedup_symbols(symbols)
        self.assertEqual(result.alias_groups, 0)
        self.assertEqual(result.alias_suppressed_bytes, 0)
        self.assertEqual(len(result.symbols), 2)

    def test_three_way_alias_group_counts_as_one_group(self):
        symbols = [
            size_attribute.Symbol(addr=0x30, size=6, sym_type="T", name="x"),
            size_attribute.Symbol(addr=0x30, size=6, sym_type="t", name="y"),
            size_attribute.Symbol(addr=0x30, size=6, sym_type="t", name="z"),
        ]
        result = size_attribute.dedup_symbols(symbols)
        self.assertEqual(result.alias_groups, 1)
        self.assertEqual(result.alias_suppressed_bytes, 6 * 2, "two of the three names suppressed")
        self.assertEqual(len(result.symbols), 1)


# --- Per-section remainder arithmetic ---------------------------------------


class AttributedBytesBySectionTests(unittest.TestCase):
    def test_matches_hand_computed_remainder(self):
        sections = size_attribute.parse_section_lines(SECTION_TABLE_TEXT)
        symbols = size_attribute.parse_symbol_lines(NM_LINES_TEXT)
        deduped = size_attribute.dedup_symbols(symbols).symbols
        totals = size_attribute.attributed_bytes_by_section(sections, deduped)

        text_section = next(s for s in sections if s.name == ".text")
        # Deduped .text symbols: (0x1000, 0x10) + (0x1020, 0x20) + (0x1060, 0x5)
        # = 0x35 = 53 B attributed out of a 0xa00 = 2560 B section.
        attributed = totals[".text"]
        self.assertEqual(attributed, 0x10 + 0x20 + 0x05)
        remainder = text_section.size - attributed
        self.assertEqual(remainder, 0xA00 - 0x35)

    def test_a_section_with_no_symbols_in_range_attributes_nothing(self):
        sections = size_attribute.parse_section_lines(SECTION_TABLE_TEXT)
        symbols = size_attribute.parse_symbol_lines(NM_LINES_TEXT)
        deduped = size_attribute.dedup_symbols(symbols).symbols
        totals = size_attribute.attributed_bytes_by_section(sections, deduped)
        self.assertEqual(totals[".rodata"], 0)
        self.assertEqual(totals[".data.rel.ro"], 0)

    def test_symbol_family_totals_matches_the_sum_attributed_to_its_section(self):
        # Every fixture symbol lives in .text, so the deduped per-family
        # total and the deduped per-section total must agree exactly —
        # the same "attribute each region once" contract viewed two ways.
        sections = size_attribute.parse_section_lines(SECTION_TABLE_TEXT)
        symbols = size_attribute.parse_symbol_lines(NM_LINES_TEXT)
        deduped = size_attribute.dedup_symbols(symbols).symbols
        totals = size_attribute.attributed_bytes_by_section(sections, deduped)
        family_totals = size_attribute.symbol_family_totals(deduped)
        self.assertEqual(sum(family_totals.values()), totals[".text"])


# --- classify_family ---------------------------------------------------------


class ClassifyFamilyTests(unittest.TestCase):
    def test_plain_path(self):
        self.assertEqual(
            size_attribute.classify_family("naga::valid::Validator::validate"), "naga"
        )
        self.assertEqual(
            size_attribute.classify_family("wgpu_core::device::Device::create_buffer"),
            "wgpu_core",
        )

    def test_impl_block_attributes_to_the_concrete_type(self):
        self.assertEqual(
            size_attribute.classify_family(
                "<alloc::vec::Vec<u8> as core::ops::Drop>::drop"
            ),
            "alloc",
        )

    def test_impl_block_with_nested_generics(self):
        self.assertEqual(
            size_attribute.classify_family(
                "<hashbrown::map::HashMap<naga::Handle, naga::Type, "
                "core::hash::BuildHasherDefault<u64>> as core::clone::Clone>::clone"
            ),
            "hashbrown",
        )

    def test_impl_block_on_a_primitive_falls_back_to_the_trait_crate(self):
        self.assertEqual(
            size_attribute.classify_family("<u8 as core::fmt::Debug>::fmt"), "core"
        )

    def test_closure_symbol_attributes_to_the_enclosing_function_crate(self):
        self.assertEqual(
            size_attribute.classify_family(
                "frust_core::widget::Widget::layout::{{closure}}"
            ),
            "frust_core",
        )

    def test_vtable_shim_inside_an_impl_block(self):
        self.assertEqual(
            size_attribute.classify_family(
                "<naga::proc::Layouter as core::default::Default>::default::{{vtable.shim}}"
            ),
            "naga",
        )

    def test_c_prefix_families(self):
        self.assertEqual(size_attribute.classify_family("sqlite3_exec"), "sqlite3")
        self.assertEqual(size_attribute.classify_family("png_read_info"), "png")
        self.assertEqual(size_attribute.classify_family("inflate"), "zlib")

    def test_anon_family_for_unrecognized_names(self):
        self.assertEqual(size_attribute.classify_family("plain_c_style_symbol"), "anon.")
        self.assertEqual(size_attribute.classify_family(""), "anon.")
        self.assertEqual(size_attribute.classify_family("   "), "anon.")


# --- classify_string ----------------------------------------------------------


class ClassifyStringTests(unittest.TestCase):
    def test_wgsl_token_bucket(self):
        self.assertEqual(
            size_attribute.classify_string("@vertex fn vs_main() -> vec4<f32> {"), "wgsl"
        )

    def test_source_path_bucket(self):
        self.assertEqual(
            size_attribute.classify_string("/rustc/abc123/library/core/src/fmt/mod.rs"),
            "source_paths",
        )
        self.assertEqual(
            size_attribute.classify_string("crates/frust-core/src/widget.rs"),
            "source_paths",
        )

    def test_panic_message_bucket(self):
        self.assertEqual(
            size_attribute.classify_string("index out of bounds: the len is 4 but"),
            "panic_messages",
        )

    def test_other_bucket_for_unmatched_strings(self):
        self.assertEqual(
            size_attribute.classify_string("just some regular embedded text here"), "other"
        )

    def test_wgsl_token_takes_priority_over_panic_keywords(self):
        # "invalid " is a panic keyword; a WGSL string containing it must
        # still bucket as wgsl since the WGSL check runs first.
        text = "@fragment fn fs(invalid_input: vec4<f32>) -> vec4<f32> {"
        self.assertEqual(size_attribute.classify_string(text), "wgsl")


# --- sfnt font scanning --------------------------------------------------------


def _build_sfnt(tables: list[tuple[bytes, bytes]]) -> bytes:
    """Lay out a minimal, well-formed sfnt blob: a 12-byte header followed
    by one 16-byte directory entry per table, then the table bytes
    themselves placed back-to-back starting right after the directory."""
    num_tables = len(tables)
    header = struct.pack(">4sHHHH", b"\x00\x01\x00\x00", num_tables, 0, 0, 0)
    dir_start = 12
    data_start = dir_start + 16 * num_tables
    directory = b""
    payload = b""
    offset = data_start
    for tag, data in tables:
        directory += struct.pack(">4sIII", tag, 0, offset, len(data))
        payload += data
        offset += len(data)
    return header + directory + payload


class FindSfntFontsTests(unittest.TestCase):
    def test_detects_a_well_formed_font_and_reports_its_length(self):
        # 12-byte header + 2*16 directory = 44, + 8 + 20 = 72 — already a
        # multiple of 4, so alignment rounding is a no-op here (covered on
        # its own below) and the buffer ends exactly at the table data.
        font = _build_sfnt([(b"head", b"\x00" * 8), (b"glyf", b"\x01" * 20)])
        self.assertEqual(len(font), 72)
        hits = size_attribute.find_sfnt_fonts(font)
        self.assertEqual(len(hits), 1)
        offset, length = hits[0]
        self.assertEqual(offset, 0)
        self.assertEqual(length, 72)

    def test_length_rounds_up_to_the_next_four_byte_boundary(self):
        # Table data ends at an unaligned 69 B; real embedders pad the
        # blob's tail to the next 4-byte boundary (73 -> 72 is wrong
        # direction — the boundary at or above 69 is 72), so the extra
        # padding bytes must be folded into the reported length rather than
        # left as a separate, unaccounted-for "gap".
        font = _build_sfnt([(b"head", b"\x00" * 8), (b"glyf", b"\x01" * 17)])
        self.assertEqual(len(font), 69)
        padded = font + b"\x00" * 3  # pad the tail to the 72 B boundary
        hits = size_attribute.find_sfnt_fonts(padded)
        self.assertEqual(len(hits), 1)
        offset, length = hits[0]
        self.assertEqual(offset, 0)
        self.assertEqual(length, 72)

    def test_a_magic_byte_sequence_with_an_absurd_table_count_is_not_a_font(self):
        # The sfnt magic with a table count of 0xffff — nowhere near a real
        # font's table directory, so this must be rejected rather than
        # reported as a multi-gigabyte "font".
        junk = b"\x00\x01\x00\x00" + struct.pack(">H", 0xFFFF) + b"\x00" * 100
        hits = size_attribute.find_sfnt_fonts(junk)
        self.assertEqual(hits, [], "a bogus table count must not be reported as a font hit")

    def test_overlapping_detections_keep_only_the_outer_font(self):
        # A font whose own table payload happens to start with another
        # sfnt magic (the table data itself, not a second embedded font):
        # the inner "hit" falls inside the outer blob's already-accepted
        # span and must be dropped.
        inner_magic_as_data = b"\x00\x01\x00\x00" + b"\xaa" * 16
        font = _build_sfnt([(b"data", inner_magic_as_data)])
        hits = size_attribute.find_sfnt_fonts(font)
        self.assertEqual(
            len(hits), 1, "the inner magic byte sequence must be suppressed as an overlap"
        )
        self.assertEqual(hits[0][0], 0)

    def test_no_magic_bytes_present_reports_no_fonts(self):
        self.assertEqual(size_attribute.find_sfnt_fonts(b"plain rodata, no fonts here"), [])

    def test_truncated_magic_near_end_of_buffer_is_not_a_font(self):
        # The magic bytes are present but there isn't room for even the
        # 12-byte sfnt header after them.
        data = b"\x00" * 50 + b"\x00\x01\x00\x00" + b"\x00\x00"
        self.assertEqual(size_attribute.find_sfnt_fonts(data), [])


# --- .rodata accounting (string buckets + font labelling) --------------------


class RodataAccountingTests(unittest.TestCase):
    def test_accounts_named_symbols_strings_and_fonts_together(self):
        font = _build_sfnt([(b"head", b"\x00" * 12)])
        wgsl = b"@vertex fn vs_main() -> vec4<f32> { return x; }\x00"
        panic = b"index out of bounds: the len is 4 but\x00"
        other = b"some other printable padding text!!\x00"
        raw = font + wgsl + panic + other

        section = size_attribute.Section(
            name=".rodata", addr=0x5000, offset=0, size=len(raw), flags="AMS"
        )
        # A named data symbol covering the font blob, so font labelling
        # resolves to a real name rather than "unnamed@...".
        symbols = [
            size_attribute.Symbol(addr=0x5000, size=len(font), sym_type="r", name="FONT_BLOB"),
        ]

        acc = size_attribute.rodata_accounting(section, raw, symbols)

        self.assertEqual(acc.named_symbol_count, 1)
        self.assertEqual(acc.named_symbol_bytes, len(font))

        self.assertEqual(acc.string_counts.get("wgsl", 0), 1)
        self.assertEqual(acc.string_counts.get("panic_messages", 0), 1)
        self.assertEqual(acc.string_counts.get("other", 0), 1)

        self.assertEqual(len(acc.fonts), 1)
        offset, length, label = acc.fonts[0]
        self.assertEqual(offset, 0)
        self.assertEqual(label, "FONT_BLOB")

    def test_unnamed_font_blob_labels_by_address(self):
        font = _build_sfnt([(b"head", b"\x00" * 12)])
        section = size_attribute.Section(
            name=".rodata", addr=0x9000, offset=0, size=len(font), flags="AMS"
        )
        acc = size_attribute.rodata_accounting(section, font, [])
        self.assertEqual(len(acc.fonts), 1)
        _offset, _length, label = acc.fonts[0]
        self.assertTrue(label.startswith("unnamed@0x9000"))


if __name__ == "__main__":
    unittest.main()
