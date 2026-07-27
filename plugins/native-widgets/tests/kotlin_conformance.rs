//! Source-scan conformance tests for `frust-native-widgets`' Kotlin↔Rust
//! event-constant + bit-packing parity, because **no cargo gate compiles
//! Kotlin at all** (`docs/DEVELOPMENT.md` says so explicitly) — only a real
//! Gradle build does. A Rust-side source scan runs in the standard verify
//! gate, needs no Android toolchain, and fails loudly instead of relying on
//! a reviewer checklist (`TASKS.md`'s M4/M5 items).
//!
//! Precedent for the shape: `crates/frust/tests/surface_mode_conformance.rs`
//! and `frust-drive/tests/print_free_cores.rs` — a plain `std::fs` source
//! scan run as an ordinary `cargo test` (this repo has no lint-plugin
//! tooling, see `docs/CODE_STANDARDS.md`). Like both precedents, this is a
//! substring/line scan, not a parser — correct for the small, hand-written
//! shapes these two files actually take today.
//!
//! **Relocated by task c1-04** from `crates/frust/tests/plugin_kotlin_conformance.rs`,
//! which f2-06 originally placed in the published facade crate — coupling the
//! facade's own test suite to one plugin's Kotlin and one example's hand
//! copy, in tension with `docs/ARCHITECTURE.md`'s "the facade never depends
//! on or re-exports a plugin." This half (the `frust-native-widgets`-specific
//! parity/packing/byte-identity checks) now lives beside the code it
//! protects, in this crate's own test tree. The cross-cutting `dev.frust`
//! bare-package scan (f2-06's Part 2) moved to
//! `crates/frust-drive/tests/plugin_package_conformance.rs` instead, since it
//! must see every plugin under `plugins/**`, not just this one.
//!
//! # Kotlin↔Rust event constant + bit-packing parity
//!
//! `plugins/native-widgets/src/events.rs` defines the `EVENT_KIND_*`
//! constants and the `pack_value_changed`/`unpack_value_changed` bit-packing
//! contract; `plugins/native-widgets/platform/android/FrustNativeListener.kt`
//! duplicates both **independently** (its own `KIND_*` constants, its own
//! `onProgressChanged` packing arithmetic). Both files carry "edit these
//! together" comments; nothing previously failed if they drifted, and drift
//! misroutes events silently (e.g. a slider drag decoding as a click).
//!
//! [`kind_constants_match_between_kotlin_and_rust`] pins the five `KIND_*`/
//! `EVENT_KIND_*` pairs. [`value_changed_bit_packing_matches_between_kotlin_and_rust`]
//! pins the packing contract's mask/shift **values** (not their literal
//! text — Kotlin's `0xFFFFFFFFL`/`shl 32` and Rust's `0xFFFF_FFFF`/`<< 32`
//! are never textually identical, so the two sides are compared as parsed
//! numbers instead).
//!
//! [`native_widgets_canonical_kotlin_matches_the_catalogs_hand_copy`] is the
//! packaging half: today's v1 packaging hand-copies `platform/android/*.kt`
//! into `examples/glyph-catalog`'s app module
//! (`docs/CODE_STANDARDS.md`'s documented, time-boxed `dev.frust` exception),
//! so nothing currently re-derives the copy from the plugin's source of
//! truth — a byte-identity check is the cheapest thing that would have
//! caught a future hand-edit landing in one copy and not the other. This one
//! reach outside this crate (into `examples/glyph-catalog`) is inherent to
//! v1's hand-copy packaging shape, not a repeat of f2-06's facade-coupling
//! defect — it is fine to keep here because it's this crate's OWN Kotlin
//! source of truth being checked against its downstream copy, the same
//! direction every other assertion in this file runs; Phase 3's Gradle-module
//! packaging removes the hand-copy (and this check) entirely.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root, resolved from this crate's manifest dir
/// (`plugins/native-widgets`) so the scan is working-directory-independent.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("plugins/native-widgets has a grandparent (the workspace root)")
        .to_path_buf()
}

fn read(rel_path: &str) -> String {
    let path = workspace_root().join(rel_path);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

const RUST_EVENTS_PATH: &str = "plugins/native-widgets/src/events.rs";
const KOTLIN_LISTENER_PATH: &str = "plugins/native-widgets/platform/android/FrustNativeListener.kt";
const KOTLIN_FACTORY_PATH: &str =
    "plugins/native-widgets/platform/android/FrustNativeControlFactory.kt";

/// The catalog's hand-copied duplicates of the plugin's canonical
/// `platform/android/*.kt` (v1 packaging shape — see the module doc).
const CATALOG_LISTENER_PATH: &str =
    "examples/glyph-catalog/android/app/src/main/kotlin/dev/frust/FrustNativeListener.kt";
const CATALOG_FACTORY_PATH: &str =
    "examples/glyph-catalog/android/app/src/main/kotlin/dev/frust/FrustNativeControlFactory.kt";

/// Parse every `const val KIND_<NAME> = <value>` line out of the Kotlin
/// listener's companion object, keyed by `<NAME>` (the `KIND_` prefix
/// stripped so it lines up with the Rust side's `EVENT_KIND_<NAME>` naming).
fn parse_kotlin_kind_constants(contents: &str) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        let Some(after) = trimmed.strip_prefix("const val KIND_") else {
            continue;
        };
        let Some((name, value_str)) = after.split_once('=') else {
            continue;
        };
        let name = name.trim().to_string();
        let value: i64 = value_str.trim().parse().unwrap_or_else(|e| {
            panic!("parsing `const val KIND_{after}` in {KOTLIN_LISTENER_PATH}: {e}")
        });
        out.insert(name, value);
    }
    out
}

/// Parse every `pub(crate) const EVENT_KIND_<NAME>: i32 = <value>;` line out
/// of the Rust events module, keyed by `<NAME>` (the `EVENT_KIND_` prefix
/// stripped, mirroring [`parse_kotlin_kind_constants`]).
fn parse_rust_event_kind_constants(contents: &str) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        let Some(after) = trimmed.strip_prefix("pub(crate) const EVENT_KIND_") else {
            continue;
        };
        let Some((name, rest)) = after.split_once(':') else {
            continue;
        };
        let Some((_ty, value_str)) = rest.split_once('=') else {
            continue;
        };
        let value_str = value_str.trim().trim_end_matches(';');
        let name = name.trim().to_string();
        let value: i64 = value_str
            .parse()
            .unwrap_or_else(|e| panic!("parsing `EVENT_KIND_{after}` in {RUST_EVENTS_PATH}: {e}"));
        out.insert(name, value);
    }
    out
}

#[test]
fn kind_constants_match_between_kotlin_and_rust() {
    let kotlin_contents = read(KOTLIN_LISTENER_PATH);
    let rust_contents = read(RUST_EVENTS_PATH);

    let kotlin_kinds = parse_kotlin_kind_constants(&kotlin_contents);
    let rust_kinds = parse_rust_event_kind_constants(&rust_contents);

    assert!(
        !kotlin_kinds.is_empty(),
        "found no `const val KIND_*` constants in {KOTLIN_LISTENER_PATH} — the scan's own \
         parser is broken, or the file was restructured; either way this test can no longer see \
         what it's supposed to pin"
    );
    assert!(
        !rust_kinds.is_empty(),
        "found no `EVENT_KIND_*` constants in {RUST_EVENTS_PATH} — the scan's own parser is \
         broken, or the file was restructured"
    );

    let kotlin_names: Vec<&String> = kotlin_kinds.keys().collect();
    let rust_names: Vec<&String> = rust_kinds.keys().collect();
    assert_eq!(
        kotlin_names, rust_names,
        "{KOTLIN_LISTENER_PATH}'s `KIND_*` names and {RUST_EVENTS_PATH}'s `EVENT_KIND_*` names \
         must be the exact same set (a kind added/renamed on one side without the other) — \
         these two tables must be edited together"
    );

    for name in kotlin_names {
        let kotlin_value = kotlin_kinds[name];
        let rust_value = rust_kinds[name];
        assert_eq!(
            kotlin_value, rust_value,
            "KIND_{name} = {kotlin_value} in {KOTLIN_LISTENER_PATH} but EVENT_KIND_{name} = \
             {rust_value} in {RUST_EVENTS_PATH} — these two tables must be edited together, or a \
             control's events silently misroute (e.g. a slider drag decoding as a click)"
        );
    }
}

/// The body of `fn_name` in `contents`, from the line containing `<fn_kind>
/// <fn_name>(` to the next line whose trimmed text is exactly `}` — good
/// enough for this file's flat, single-block helper functions (mirrors
/// `print_free_cores.rs`'s "not a full parser" scope note). `fn_kind` is
/// `"fun"` for Kotlin, `"fn"` for Rust.
fn extract_fn_body<'a>(
    contents: &'a str,
    fn_kind: &str,
    fn_name: &str,
    source_path: &str,
) -> &'a str {
    let needle = format!("{fn_kind} {fn_name}(");
    let start = contents
        .find(&needle)
        .unwrap_or_else(|| panic!("no `{needle}` found in {source_path}"));
    let after = &contents[start..];
    let end = after
        .find("\n}")
        .map(|i| i + 2)
        .unwrap_or_else(|| panic!("no closing `}}` found for `{needle}` in {source_path}"));
    &after[..end]
}

/// Extract the hex-mask literal following the first `0x` in `text` as a
/// numeric value, tolerating a Kotlin `L` type suffix (`0xFFFFFFFFL`) or
/// Rust's `_` digit-group separator (`0xFFFF_FFFF`) — a **contract**
/// comparison, not a text/byte match: the Kotlin/Rust syntaxes for the same
/// mask are never textually identical, so comparing the parsed value is what
/// actually pins the shared intent.
fn hex_literal_value(text: &str, source_path: &str) -> u64 {
    let idx = text
        .find("0x")
        .unwrap_or_else(|| panic!("no `0x...` hex literal found in {source_path}"));
    let rest = &text[idx + 2..];
    let raw: String = rest
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() || *c == '_')
        .collect();
    let cleaned: String = raw.chars().filter(|c| *c != '_').collect();
    u64::from_str_radix(&cleaned, 16)
        .unwrap_or_else(|e| panic!("parsing hex literal `0x{cleaned}` in {source_path}: {e}"))
}

/// Extract the integer immediately following the first occurrence of
/// `keyword` in `text` (skipping intervening whitespace), tolerating a
/// trailing non-digit terminator (`)`, newline, etc). Used to pin Kotlin's
/// `shl`/Rust's `<<`/`>>` shift amount as a **value**, since the keyword
/// itself differs by language.
fn int_after(text: &str, keyword: &str, source_path: &str) -> u32 {
    let idx = text
        .find(keyword)
        .unwrap_or_else(|| panic!("no `{keyword}` found in {source_path}"));
    let rest = text[idx + keyword.len()..].trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits
        .parse()
        .unwrap_or_else(|e| panic!("parsing integer after `{keyword}` in {source_path}: {e}"))
}

#[test]
fn value_changed_bit_packing_matches_between_kotlin_and_rust() {
    let kotlin_contents = read(KOTLIN_LISTENER_PATH);
    let rust_contents = read(RUST_EVENTS_PATH);

    // Kotlin: `val detail = (progress.toLong() and 0xFFFFFFFFL) or (if
    // (fromUser) 1L shl 32 else 0L)` — the whole packing lives in
    // `onProgressChanged`'s body.
    let kotlin_body = extract_fn_body(
        &kotlin_contents,
        "fun",
        "onProgressChanged",
        KOTLIN_LISTENER_PATH,
    );
    let kotlin_mask = hex_literal_value(kotlin_body, KOTLIN_LISTENER_PATH);
    let kotlin_shift = int_after(kotlin_body, "shl", KOTLIN_LISTENER_PATH);

    // Rust: `pack_value_changed` masks with the same low-32-bits literal;
    // `unpack_value_changed` masks AND shifts back — pin all three against
    // the Kotlin side so a change to any one without the others fails.
    let rust_pack_body =
        extract_fn_body(&rust_contents, "fn", "pack_value_changed", RUST_EVENTS_PATH);
    let rust_pack_mask = hex_literal_value(rust_pack_body, RUST_EVENTS_PATH);
    let rust_pack_shift = int_after(rust_pack_body, "<<", RUST_EVENTS_PATH);

    let rust_unpack_body = extract_fn_body(
        &rust_contents,
        "fn",
        "unpack_value_changed",
        RUST_EVENTS_PATH,
    );
    let rust_unpack_mask = hex_literal_value(rust_unpack_body, RUST_EVENTS_PATH);
    let rust_unpack_shift = int_after(rust_unpack_body, ">>", RUST_EVENTS_PATH);

    let fail = |what: &str, kotlin: u64, rust: u64| {
        panic!(
            "{what} mismatch: {KOTLIN_LISTENER_PATH}'s onProgressChanged encodes {kotlin} but \
             {RUST_EVENTS_PATH}'s pack/unpack_value_changed encodes {rust} — these two tables \
             must be edited together, or a slider's value/fromUser bits silently decode wrong"
        )
    };

    if kotlin_mask != rust_pack_mask {
        fail("low-32-bits mask (pack)", kotlin_mask, rust_pack_mask);
    }
    if kotlin_mask != rust_unpack_mask {
        fail("low-32-bits mask (unpack)", kotlin_mask, rust_unpack_mask);
    }
    if u64::from(kotlin_shift) != u64::from(rust_pack_shift) {
        fail(
            "bit-32 shift (pack)",
            u64::from(kotlin_shift),
            u64::from(rust_pack_shift),
        );
    }
    if u64::from(kotlin_shift) != u64::from(rust_unpack_shift) {
        fail(
            "bit-32 shift (unpack)",
            u64::from(kotlin_shift),
            u64::from(rust_unpack_shift),
        );
    }
}

/// v1's packaging shape (module doc) hand-copies `platform/android/*.kt`
/// into the catalog app's own module rather than building from a shared
/// Gradle module — nothing re-derives the copy from the plugin's source of
/// truth, so a byte-identity check is the cheapest thing that catches the
/// copy drifting from the canonical file.
#[test]
fn native_widgets_canonical_kotlin_matches_the_catalogs_hand_copy() {
    for (canonical, copy) in [
        (KOTLIN_LISTENER_PATH, CATALOG_LISTENER_PATH),
        (KOTLIN_FACTORY_PATH, CATALOG_FACTORY_PATH),
    ] {
        let canonical_contents = read(canonical);
        let copy_contents = read(copy);
        assert_eq!(
            canonical_contents, copy_contents,
            "{copy} has drifted from its source of truth, {canonical} — v1's packaging \
             hand-copies this file into the catalog app module (Phase 3 packages it as a proper \
             Gradle module instead); re-sync the copy"
        );
    }
}
