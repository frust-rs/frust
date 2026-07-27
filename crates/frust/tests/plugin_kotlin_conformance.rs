//! Source-scan conformance tests for two `frust-native-widgets` invariants
//! that no cargo gate can otherwise see, because **no cargo gate compiles
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
//! # Part 1 (M4) — Kotlin↔Rust event constant + bit-packing parity
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
//! packaging half named in the task: today's v1 packaging hand-copies
//! `platform/android/*.kt` into `examples/glyph-catalog`'s app module
//! (`docs/CODE_STANDARDS.md`'s documented, time-boxed `dev.frust` exception
//! below), so nothing currently re-derives the copy from the plugin's source
//! of truth — a byte-identity check is the cheapest thing that would have
//! caught a future hand-edit landing in one copy and not the other.
//!
//! # Part 2 (M5's cheap half) — the `dev.frust` exception must not spread
//!
//! `docs/CODE_STANDARDS.md`'s Plugin Conventions requires a plugin's Android
//! Kotlin to live in its own subpackage inside its own Gradle module
//! (`plugins/secure-storage`'s `dev.frust.securestorage` is the precedent
//! this scan expects everyone else to follow). `frust-native-widgets` has a
//! documented, time-boxed exception putting its two generic classes
//! (`FrustNativeControlFactory`, `FrustNativeListener`) directly in the bare
//! `dev.frust` package, because that package is baked into their JNI export
//! symbol names — moving them is a breaking symbol rename, correctly
//! deferred to Phase 3 packaging and NOT attempted here.
//!
//! [`only_the_two_known_files_use_the_bare_dev_frust_package`] fails if any
//! OTHER `.kt` file under `plugins/**` declares the bare `package dev.frust`
//! (a second plugin copying the exception rather than following the
//! `dev.frust.<plugin>` precedent), and fails just as loudly if either of
//! the two allowlisted files stops declaring it (a stale allowlist entry is
//! as much drift as a new violation). Scoped to `plugins/**` only — the
//! embedding module's own `platform/android/frust-embedding` legitimately
//! ships bare `dev.frust` Kotlin and must never trip this scan.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root, resolved from this crate's manifest dir
/// (`crates/frust`) so the scan is working-directory-independent.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust has a grandparent (the workspace root)")
        .to_path_buf()
}

fn read(rel_path: &str) -> String {
    let path = workspace_root().join(rel_path);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// `path` relative to the workspace root, forward-slashed, for stable
/// failure messages and allowlist keys independent of host path separators.
fn rel(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("dir entry under {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            walk_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Every file under `plugins/`, sorted for a stable failure order.
fn all_plugin_files() -> Vec<PathBuf> {
    let root = workspace_root().join("plugins");
    let mut out = Vec::new();
    walk_files(&root, &mut out);
    out.sort();
    out
}

const RUST_EVENTS_PATH: &str = "plugins/native-widgets/src/events.rs";
const KOTLIN_LISTENER_PATH: &str = "plugins/native-widgets/platform/android/FrustNativeListener.kt";
const KOTLIN_FACTORY_PATH: &str =
    "plugins/native-widgets/platform/android/FrustNativeControlFactory.kt";

/// The catalog's hand-copied duplicates of the plugin's canonical
/// `platform/android/*.kt` (v1 packaging shape — see the module doc's Part
/// 1's last paragraph).
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

/// v1's packaging shape (module doc's Part 1) hand-copies
/// `platform/android/*.kt` into the catalog app's own module rather than
/// building from a shared Gradle module — nothing re-derives the copy from
/// the plugin's source of truth, so a byte-identity check is the cheapest
/// thing that catches the copy drifting from the canonical file.
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

/// Files allowed to declare the bare `package dev.frust` under `plugins/**`
/// — `frust-native-widgets`'s documented, time-boxed exception
/// (`docs/CODE_STANDARDS.md`'s Plugin Conventions), because the package is
/// baked into these two classes' JNI export symbol names. Moving them is a
/// breaking symbol rename, correctly deferred to Phase 3 packaging.
const BARE_DEV_FRUST_ALLOWLIST: &[&str] = &[KOTLIN_FACTORY_PATH, KOTLIN_LISTENER_PATH];

/// True if `contents`' package declaration is the bare `dev.frust` — NOT a
/// subpackage like `dev.frust.camera`/`dev.frust.securestorage`, which are
/// the correct, unexceptional shape every other plugin follows.
fn declares_bare_dev_frust(contents: &str) -> bool {
    contents
        .lines()
        .any(|line| line.trim() == "package dev.frust")
}

#[test]
fn only_the_two_known_files_use_the_bare_dev_frust_package() {
    let mut found_bare: Vec<String> = Vec::new();

    for path in all_plugin_files() {
        if path.extension().is_none_or(|ext| ext != "kt") {
            continue;
        }
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        if declares_bare_dev_frust(&contents) {
            found_bare.push(rel(&path));
        }
    }

    let mut unexpected: Vec<&String> = found_bare
        .iter()
        .filter(|p| !BARE_DEV_FRUST_ALLOWLIST.contains(&p.as_str()))
        .collect();
    unexpected.sort();
    assert!(
        unexpected.is_empty(),
        "found {} `.kt` file(s) under plugins/** declaring the bare `package dev.frust` outside \
         the allowlisted native-widgets exception ({} hit(s)): {:?} — docs/CODE_STANDARDS.md's \
         Plugin Conventions requires a plugin's own subpackage (e.g. `dev.frust.<plugin>`, see \
         plugins/secure-storage's `dev.frust.securestorage`); the bare package is a documented, \
         time-boxed exception for frust-native-widgets's two JNI-symbol-fixed classes ONLY — see \
         the exception clause in docs/CODE_STANDARDS.md and the Phase 3 packaging task that \
         closes it",
        unexpected.len(),
        unexpected.len(),
        unexpected,
    );

    let mut missing: Vec<&str> = BARE_DEV_FRUST_ALLOWLIST
        .iter()
        .filter(|p| !found_bare.contains(&p.to_string()))
        .copied()
        .collect();
    missing.sort_unstable();
    assert!(
        missing.is_empty(),
        "expected these allowlisted files to declare `package dev.frust` but they don't (a stale \
         allowlist entry, or the exception was actually closed and this list needs shrinking): \
         {missing:?}"
    );
}

#[test]
fn scanner_rejects_a_subpackage_as_bare_dev_frust() {
    assert!(!declares_bare_dev_frust("package dev.frust.camera\n"));
    assert!(!declares_bare_dev_frust(
        "package dev.frust.securestorage\n"
    ));
    assert!(declares_bare_dev_frust("package dev.frust\n"));
    assert!(declares_bare_dev_frust(
        "// a comment\npackage dev.frust\n\nimport android.view.View\n"
    ));
}
