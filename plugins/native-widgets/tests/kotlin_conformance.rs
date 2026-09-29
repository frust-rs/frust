//! Source-scan conformance tests for `frust-native-widgets`' Kotlin↔Rust
//! event-constant + bit-packing parity, because **no cargo gate compiles
//! Kotlin at all** (`docs/DEVELOPMENT.md` says so explicitly) — only a real
//! Gradle build does. A Rust-side source scan runs in the standard verify
//! gate, needs no Android toolchain, and fails loudly instead of relying on
//! a reviewer checklist.
//!
//! Precedent for the shape: `crates/frust/tests/surface_mode_conformance.rs`
//! and `frust-drive/tests/print_free_cores.rs` — a plain `std::fs` source
//! scan run as an ordinary `cargo test` (this repo has no lint-plugin
//! tooling, see `docs/CODE_STANDARDS.md`). Like both precedents, this is a
//! substring/line scan, not a parser — correct for the small, hand-written
//! shapes these two files actually take today.
//!
//! **Relocated** from `crates/frust/tests/plugin_kotlin_conformance.rs`,
//! which was originally placed in the published facade crate — coupling the
//! facade's own test suite to one plugin's Kotlin and one example's hand
//! copy, in tension with `docs/ARCHITECTURE.md`'s "the facade never depends
//! on or re-exports a plugin." This half (the `frust-native-widgets`-specific
//! parity/packing checks) now lives beside the code it protects, in this
//! crate's own test tree. The cross-cutting `dev.frust` bare-package scan
//! (the other half) moved to
//! `crates/frust-drive/tests/plugin_package_conformance.rs` instead, since it
//! must see every plugin under `plugins/**`, not just this one.
//!
//! # Kotlin↔Rust event constant + bit-packing parity
//!
//! `plugins/native-widgets/src/events.rs` defines the `EVENT_KIND_*`
//! constants and the `pack_value_changed`/`unpack_value_changed` bit-packing
//! contract; the plugin's `FrustNativeListener.kt` (under
//! `platform/android/src/main/kotlin/dev/frust/nativewidgets/`) duplicates
//! both **independently** (its own `KIND_*` constants, its own
//! `onProgressChanged` packing arithmetic). Both files carry "edit these
//! together" comments; nothing previously failed if they drifted, and drift
//! misroutes events silently (e.g. a slider drag decoding as a click).
//!
//! [`kind_constants_match_between_kotlin_and_rust`] pins every `KIND_*`/
//! `EVENT_KIND_*` pair, whatever the table's length — including `SELECTION`
//! (6), which is appended on BOTH sides even though only the Apple arms emit
//! it (a Rust-only kind would fail the name-set comparison), `DATE` (7),
//! appended after it and emitted by all three arms, and `RESELECTED` (8),
//! appended after that and emitted only by the iOS tab bar; all three are
//! pinned by value in [`the_appended_kinds_keep_their_codes_on_both_sides`].
//! [`value_changed_bit_packing_matches_between_kotlin_and_rust`]
//! pins the packing contract's mask/shift **values** (not their literal
//! text — Kotlin's `0xFFFFFFFFL`/`shl 32` and Rust's `0xFFFF_FFFF`/`<< 32`
//! are never textually identical, so the two sides are compared as parsed
//! numbers instead). [`date_bit_packing_matches_between_kotlin_and_rust`]
//! does the same for `onDateChanged` vs `pack_date` — every mask and every
//! shift, in field order — and additionally pins the Kotlin side's `+ 1`
//! month offset (`DatePicker` reports a 0-based month; the wire carries a
//! 1-based one).
//!
//! # The byte-identity check is gone, and that is the point
//!
//! A third test used to pin this crate's canonical `platform/android/*.kt`
//! byte-for-byte against a hand-copied duplicate in
//! `examples/glyph-catalog`'s own app module — the only defence v1's
//! hand-copy packaging had against the two drifting apart. **There is no
//! copy any more.** The plugin's Kotlin now ships inside its own
//! `com.android.library` module (`plugins/native-widgets/platform/android`,
//! package `dev.frust.nativewidgets`), wired into a consuming app by
//! `Contribution::GradleModule`, so every consumer — the catalog included —
//! compiles the one canonical file. Nothing can drift from it because
//! nothing duplicates it; the check was deleted rather than weakened, and
//! this crate's tests no longer reach into `examples/**` at all.
//!
//! Both parity checks above survive untouched: they guard a genuinely
//! independent duplication (Kotlin arithmetic vs Rust arithmetic) that
//! packaging does not remove.
//!
//! # Package/class-name ↔ JNI-export-symbol parity
//!
//! An earlier rename moved the package to `dev.frust.nativewidgets` and the exports to
//! `Java_dev_frust_nativewidgets_*`. `android/ctx.rs`'s `LISTENER_CLASS`, the
//! Android-cfg'd `VIEW_TYPE` in `api/builders.rs`, and every `Java_*` export
//! name in `android/mod.rs` are matched to the two Kotlin files' `package`/
//! `class` declarations **by hand only** — `android/ctx.rs`'s own doc names
//! the failure mode: a mismatch surfaces only at runtime, as a failed class
//! lookup the first time an interactive control is created, and no cargo gate
//! compiles Kotlin (see this file's opening paragraph). [`factory_class_name_matches_between_kotlin_and_rust`],
//! [`listener_class_name_matches_between_kotlin_and_rust`], and
//! [`java_export_symbols_match_reconstructed_package_and_class_names`]
//! reconstruct the expected class/symbol names from the two Kotlin files'
//! `package`/`class` lines and pin the Rust side against that reconstruction,
//! the same shape as [`kind_constants_match_between_kotlin_and_rust`] above.

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
/// The listener class inside the plugin's own Gradle library module — the one
/// canonical copy every consuming app now compiles (see the module doc's
/// byte-identity section).
const KOTLIN_LISTENER_PATH: &str = "plugins/native-widgets/platform/android/src/main/kotlin/\
                                    dev/frust/nativewidgets/FrustNativeListener.kt";
/// The factory class beside [`KOTLIN_LISTENER_PATH`], same module.
const KOTLIN_FACTORY_PATH: &str = "plugins/native-widgets/platform/android/src/main/kotlin/\
                                   dev/frust/nativewidgets/FrustNativeControlFactory.kt";
/// `plugins/native-widgets/src/android/ctx.rs`, home of `LISTENER_CLASS`.
const RUST_CTX_PATH: &str = "plugins/native-widgets/src/android/ctx.rs";
/// `plugins/native-widgets/src/api/builders.rs`, home of the Android-cfg'd
/// `VIEW_TYPE` — this file's task doc calls the concept it holds
/// "`FACTORY_CLASS`" for symmetry with `LISTENER_CLASS`, though no Rust
/// identifier of that exact name exists (`crate::apple::factory::
/// FACTORY_CLASS_NAME` is the separate, iOS-side constant).
const RUST_BUILDERS_PATH: &str = "plugins/native-widgets/src/api/builders.rs";
/// `plugins/native-widgets/src/android/mod.rs`, home of the four `Java_*`
/// JNI export symbols.
const RUST_ANDROID_MOD_PATH: &str = "plugins/native-widgets/src/android/mod.rs";

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

#[test]
fn the_appended_kinds_keep_their_codes_on_both_sides() {
    let kotlin_kinds = parse_kotlin_kind_constants(&read(KOTLIN_LISTENER_PATH));
    let rust_kinds = parse_rust_event_kind_constants(&read(RUST_EVENTS_PATH));
    // Append-only: the five shipped kinds keep 1-5, SELECTION took the next
    // free code (6), DATE the one after it (7) and RESELECTED the one after
    // that (8), on both sides.
    for (name, value) in [
        ("CLICK", 1),
        ("TOGGLED", 2),
        ("VALUE_CHANGED", 3),
        ("DRAG_START", 4),
        ("DRAG_END", 5),
        ("SELECTION", 6),
        ("DATE", 7),
        ("RESELECTED", 8),
    ] {
        assert_eq!(
            kotlin_kinds.get(name),
            Some(&value),
            "KIND_{name} in {KOTLIN_LISTENER_PATH} must stay {value} (append-only table)"
        );
        assert_eq!(
            rust_kinds.get(name),
            Some(&value),
            "EVENT_KIND_{name} in {RUST_EVENTS_PATH} must stay {value} (append-only table)"
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

/// Every hex literal in `text`, in order, as numeric values — the
/// many-literal sibling of [`hex_literal_value`] (same `L`-suffix/`_`
/// tolerance), for a packing expression with one mask per field.
fn all_hex_literal_values(text: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(idx) = rest.find("0x") {
        let after = &rest[idx + 2..];
        let raw: String = after
            .chars()
            .take_while(|c| c.is_ascii_hexdigit() || *c == '_')
            .collect();
        let cleaned: String = raw.chars().filter(|c| *c != '_').collect();
        if let Ok(value) = u64::from_str_radix(&cleaned, 16) {
            out.push(value);
        }
        rest = &after[raw.len()..];
    }
    out
}

/// Every integer following an occurrence of `keyword` in `text`, in order —
/// the many-shift sibling of [`int_after`].
fn all_ints_after(text: &str, keyword: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(idx) = rest.find(keyword) {
        let after = rest[idx + keyword.len()..].trim_start();
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(value) = digits.parse() {
            out.push(value);
        }
        rest = &rest[idx + keyword.len()..];
    }
    out
}

#[test]
fn date_bit_packing_matches_between_kotlin_and_rust() {
    let kotlin_contents = read(KOTLIN_LISTENER_PATH);
    let rust_contents = read(RUST_EVENTS_PATH);

    // Kotlin: `onDateChanged`'s body, bounded by its own `nativeOnEvent`
    // call rather than by [`extract_fn_body`]'s top-level `}` (a Kotlin
    // method's closing brace is indented, so that helper would run on to the
    // end of the class and sweep in every later literal).
    let needle = "fun onDateChanged(";
    let start = kotlin_contents
        .find(needle)
        .unwrap_or_else(|| panic!("no `{needle}` found in {KOTLIN_LISTENER_PATH}"));
    let after = &kotlin_contents[start..];
    let end = after
        .find("nativeOnEvent(slotId, KIND_DATE")
        .unwrap_or_else(|| {
            panic!(
                "`onDateChanged` in {KOTLIN_LISTENER_PATH} never calls \
                 `nativeOnEvent(slotId, KIND_DATE, …)`"
            )
        });
    let kotlin_body = &after[..end];
    let kotlin_masks = all_hex_literal_values(kotlin_body);
    let kotlin_shifts = all_ints_after(kotlin_body, "shl");

    // Rust: `pack_date` — the one packing function every Apple arm and the
    // Android decode path share.
    let rust_body = extract_fn_body(&rust_contents, "fn", "pack_date", RUST_EVENTS_PATH);
    let rust_masks = all_hex_literal_values(rust_body);
    let rust_shifts = all_ints_after(rust_body, "<<");

    assert_eq!(
        rust_masks,
        vec![0xFFFF, 0xFF, 0xFF],
        "{RUST_EVENTS_PATH}'s pack_date no longer masks year/month/day as 16/8/8 bits — the \
         scan's expectation and the documented layout must move together"
    );
    assert_eq!(
        rust_shifts,
        vec![16, 8],
        "{RUST_EVENTS_PATH}'s pack_date no longer shifts year/month by 16/8"
    );
    assert_eq!(
        kotlin_masks, rust_masks,
        "{KOTLIN_LISTENER_PATH}'s onDateChanged masks {kotlin_masks:?} but {RUST_EVENTS_PATH}'s \
         pack_date masks {rust_masks:?} — edit both together, or a picked date silently decodes \
         as a different (or no) date"
    );
    assert_eq!(
        kotlin_shifts, rust_shifts,
        "{KOTLIN_LISTENER_PATH}'s onDateChanged shifts {kotlin_shifts:?} but \
         {RUST_EVENTS_PATH}'s pack_date shifts {rust_shifts:?} — edit both together"
    );
    assert!(
        kotlin_body.contains("monthOfYear + 1"),
        "{KOTLIN_LISTENER_PATH}'s onDateChanged must add 1 to DatePicker's 0-based `monthOfYear` \
         before packing — the wire month is 1-based (`pack_date`'s layout table)"
    );
}

/// The dotted package `contents` declares, from its `package <name>` line —
/// Kotlin has no trailing semicolon, unlike Java, so the rest of the trimmed
/// line is the whole name.
fn parse_kotlin_package(contents: &str, source_path: &str) -> String {
    contents
        .lines()
        .find_map(|line| line.trim().strip_prefix("package "))
        .unwrap_or_else(|| panic!("no `package <name>` line found in {source_path}"))
        .trim()
        .to_string()
}

/// The name after the first top-level `class <Name>` declaration in
/// `contents` — good enough for these two files' one-class shape (mirrors
/// this file's "not a full parser" scope note above). A commented-out
/// mention (`// ... class ...`) never matches: its trimmed line starts with
/// `//`, not the bare `class ` keyword.
fn parse_kotlin_class_name(contents: &str, source_path: &str) -> String {
    let line = contents
        .lines()
        .find(|line| line.trim_start().starts_with("class "))
        .unwrap_or_else(|| panic!("no top-level `class <Name>` line found in {source_path}"));
    let after = line
        .trim_start()
        .strip_prefix("class ")
        .expect("line matched by starts_with(\"class \") above");
    let name: String = after
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    assert!(
        !name.is_empty(),
        "found a `class ` line in {source_path} with no identifier following it: {line:?}"
    );
    name
}

/// The quoted string value on the first line of `contents` starting with
/// `needle` (e.g. `needle = "pub(crate) const LISTENER_CLASS: &str = "`,
/// matching a value declared `= "dev.frust...";`) — used for both
/// `LISTENER_CLASS` and the Android-cfg'd `VIEW_TYPE`, whose declarations
/// share this shape.
fn quoted_value_after(contents: &str, needle: &str, source_path: &str) -> String {
    let idx = contents
        .find(needle)
        .unwrap_or_else(|| panic!("no `{needle}` found in {source_path}"));
    let after = &contents[idx + needle.len()..];
    let start = after
        .find('"')
        .unwrap_or_else(|| panic!("no opening `\"` after `{needle}` in {source_path}"));
    let rest = &after[start + 1..];
    let end = rest
        .find('"')
        .unwrap_or_else(|| panic!("no closing `\"` after `{needle}` in {source_path}"));
    rest[..end].to_string()
}

/// `android/ctx.rs`'s `LISTENER_CLASS` value.
fn rust_listener_class(contents: &str) -> String {
    quoted_value_after(
        contents,
        "pub(crate) const LISTENER_CLASS: &str = ",
        RUST_CTX_PATH,
    )
}

/// `api/builders.rs`'s **Android-cfg'd** `VIEW_TYPE` value — the file declares
/// three cfg-gated copies of this constant (Android/iOS/neither); this parses
/// only the one immediately following the Android `#[cfg(...)]` attribute,
/// the half this parity check is about.
fn rust_android_view_type(contents: &str) -> String {
    let cfg_needle = "#[cfg(target_os = \"android\")]";
    let cfg_idx = contents
        .find(cfg_needle)
        .unwrap_or_else(|| panic!("no `{cfg_needle}` found in {RUST_BUILDERS_PATH}"));
    quoted_value_after(
        &contents[cfg_idx..],
        "pub(super) const VIEW_TYPE: &str = ",
        RUST_BUILDERS_PATH,
    )
}

/// Every `Java_*` JNI export symbol name declared in `contents` (`pub extern
/// "system" fn Java_...`), in file order.
fn parse_java_export_names(contents: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in contents.lines() {
        let Some(after) = line
            .trim_start()
            .strip_prefix("pub extern \"system\" fn Java_")
        else {
            continue;
        };
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        out.push(format!("Java_{name}"));
    }
    assert!(
        !out.is_empty(),
        "found no `pub extern \"system\" fn Java_*` export lines in {RUST_ANDROID_MOD_PATH} — \
         the scan's own parser is broken, or the file was restructured"
    );
    out
}

#[test]
fn factory_class_name_matches_between_kotlin_and_rust() {
    let kotlin_contents = read(KOTLIN_FACTORY_PATH);
    let package = parse_kotlin_package(&kotlin_contents, KOTLIN_FACTORY_PATH);
    let class_name = parse_kotlin_class_name(&kotlin_contents, KOTLIN_FACTORY_PATH);
    let reconstructed = format!("{package}.{class_name}");

    let rust_contents = read(RUST_BUILDERS_PATH);
    let rust_view_type = rust_android_view_type(&rust_contents);

    assert_eq!(
        reconstructed, rust_view_type,
        "{KOTLIN_FACTORY_PATH}'s `package {package}` + `class {class_name}` reconstructs to \
         `{reconstructed}`, but {RUST_BUILDERS_PATH}'s Android `VIEW_TYPE` is `{rust_view_type}` \
         — these must be edited together, or the embedding's `FrustViewHost` fails to resolve \
         the platform-view factory and every native control on Android renders nothing"
    );
}

#[test]
fn listener_class_name_matches_between_kotlin_and_rust() {
    let kotlin_contents = read(KOTLIN_LISTENER_PATH);
    let package = parse_kotlin_package(&kotlin_contents, KOTLIN_LISTENER_PATH);
    let class_name = parse_kotlin_class_name(&kotlin_contents, KOTLIN_LISTENER_PATH);
    let reconstructed = format!("{package}.{class_name}");

    let rust_contents = read(RUST_CTX_PATH);
    let rust_listener_class = rust_listener_class(&rust_contents);

    assert_eq!(
        reconstructed, rust_listener_class,
        "{KOTLIN_LISTENER_PATH}'s `package {package}` + `class {class_name}` reconstructs to \
         `{reconstructed}`, but {RUST_CTX_PATH}'s `LISTENER_CLASS` is `{rust_listener_class}` — \
         these must be edited together, or the first interactive control created fails a JNI \
         class lookup at runtime (`android/ctx.rs`'s own doc names this exact failure mode)"
    );
}

#[test]
fn java_export_symbols_match_reconstructed_package_and_class_names() {
    let factory_contents = read(KOTLIN_FACTORY_PATH);
    let package = parse_kotlin_package(&factory_contents, KOTLIN_FACTORY_PATH);
    let factory_class = parse_kotlin_class_name(&factory_contents, KOTLIN_FACTORY_PATH);

    let listener_contents = read(KOTLIN_LISTENER_PATH);
    let listener_package = parse_kotlin_package(&listener_contents, KOTLIN_LISTENER_PATH);
    let listener_class = parse_kotlin_class_name(&listener_contents, KOTLIN_LISTENER_PATH);
    assert_eq!(
        package, listener_package,
        "{KOTLIN_FACTORY_PATH} declares package `{package}` but {KOTLIN_LISTENER_PATH} declares \
         `{listener_package}` — both classes are documented as living in the same package \
         (`android/mod.rs`'s module doc); this scan's own two-Kotlin-files assumption is broken \
         if they ever diverge"
    );

    let mangled_package = package.replace('.', "_");
    let factory_prefix = format!("Java_{mangled_package}_{factory_class}_");
    let listener_prefix = format!("Java_{mangled_package}_{listener_class}_");

    let android_mod_contents = read(RUST_ANDROID_MOD_PATH);
    let exports = parse_java_export_names(&android_mod_contents);

    let mut saw_factory_export = false;
    let mut saw_listener_export = false;
    for export in &exports {
        if export.starts_with(&factory_prefix) {
            saw_factory_export = true;
        } else if export.starts_with(&listener_prefix) {
            saw_listener_export = true;
        } else {
            panic!(
                "{RUST_ANDROID_MOD_PATH} declares export `{export}`, which matches neither the \
                 factory prefix `{factory_prefix}` (reconstructed from {KOTLIN_FACTORY_PATH}'s \
                 package + class) nor the listener prefix `{listener_prefix}` (reconstructed \
                 from {KOTLIN_LISTENER_PATH}'s) — a mismatched export is a silent runtime symbol \
                 lookup failure (`UnsatisfiedLinkError`), since the JVM resolves `native`/\
                 `external` methods by mangled name alone"
            );
        }
    }
    assert!(
        saw_factory_export,
        "found no `{RUST_ANDROID_MOD_PATH}` export starting with the reconstructed factory \
         prefix `{factory_prefix}` — either the scan's own parser is broken, or every factory \
         export drifted from {KOTLIN_FACTORY_PATH}'s package/class declaration"
    );
    assert!(
        saw_listener_export,
        "found no `{RUST_ANDROID_MOD_PATH}` export starting with the reconstructed listener \
         prefix `{listener_prefix}` — either the scan's own parser is broken, or every listener \
         export drifted from {KOTLIN_LISTENER_PATH}'s package/class declaration"
    );
}
