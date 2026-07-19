//! Shared conformance suite for every [`Backend`] implementation.
//!
//! `#[cfg(test)]`-only: [`run_conformance_suite`] exercises the full
//! backend contract — round trips for all five value types, `remove`/
//! `clear`/`contains`/`keys` semantics, overwrite-with-different-type,
//! missing-key `None`, `f64` exact round trip (including `NaN`), and
//! `Vec<String>` with empty strings/unicode entries — against any fresh,
//! empty [`Backend`].
//!
//! `file::tests::conformance` is this task's caller (against a
//! tempdir-isolated [`crate::file::FileStore`]); task 06's Android/Apple
//! backend test modules are expected to call the same function (on-target
//! or manual where CI can't run them — see the plugin-system plan's Phase 2
//! milestone), so a single assertion set validates every backend.

use crate::{Backend, PrefValue};

/// Run the full suite against `backend`. `backend` must start out empty
/// (a fresh store) — the suite both asserts on and mutates its state.
pub(crate) fn run_conformance_suite(backend: &dyn Backend) {
    round_trip_all_value_types(backend);
    missing_key_is_none(backend);
    overwrite_with_different_type(backend);
    f64_round_trip(backend);
    string_list_empty_and_unicode(backend);
    remove_semantics(backend);
    contains_semantics(backend);
    keys_semantics(backend);
    clear_semantics(backend);
}

fn round_trip_all_value_types(backend: &dyn Backend) {
    backend.set("bool_key", PrefValue::Bool(true)).unwrap();
    assert_eq!(backend.get("bool_key"), Some(PrefValue::Bool(true)));

    backend.set("bool_key", PrefValue::Bool(false)).unwrap();
    assert_eq!(backend.get("bool_key"), Some(PrefValue::Bool(false)));

    backend.set("i64_key", PrefValue::I64(-42)).unwrap();
    assert_eq!(backend.get("i64_key"), Some(PrefValue::I64(-42)));

    backend.set("i64_key", PrefValue::I64(i64::MAX)).unwrap();
    assert_eq!(backend.get("i64_key"), Some(PrefValue::I64(i64::MAX)));

    backend.set("i64_key", PrefValue::I64(i64::MIN)).unwrap();
    assert_eq!(backend.get("i64_key"), Some(PrefValue::I64(i64::MIN)));

    backend.set("f64_key", PrefValue::F64(3.5)).unwrap();
    assert_eq!(backend.get("f64_key"), Some(PrefValue::F64(3.5)));

    backend
        .set("string_key", PrefValue::Str("hello frust".into()))
        .unwrap();
    assert_eq!(
        backend.get("string_key"),
        Some(PrefValue::Str("hello frust".into()))
    );

    backend
        .set(
            "string_list_key",
            PrefValue::StrList(vec!["a".into(), "b".into(), "c".into()]),
        )
        .unwrap();
    assert_eq!(
        backend.get("string_list_key"),
        Some(PrefValue::StrList(vec!["a".into(), "b".into(), "c".into()]))
    );
}

fn missing_key_is_none(backend: &dyn Backend) {
    assert_eq!(backend.get("this_key_was_never_set"), None);
}

fn overwrite_with_different_type(backend: &dyn Backend) {
    backend
        .set("mixed_key", PrefValue::Str("a string".into()))
        .unwrap();
    assert_eq!(
        backend.get("mixed_key"),
        Some(PrefValue::Str("a string".into()))
    );

    // Overwriting with a different type replaces it outright — no
    // coexistence of two types under one key.
    backend.set("mixed_key", PrefValue::I64(7)).unwrap();
    assert_eq!(backend.get("mixed_key"), Some(PrefValue::I64(7)));
}

fn f64_round_trip(backend: &dyn Backend) {
    for value in [0.0, -0.0, 1.0, -1.0, f64::MIN, f64::MAX, f64::EPSILON] {
        backend.set("f64_probe", PrefValue::F64(value)).unwrap();
        let got = backend.get("f64_probe");
        assert_eq!(
            got,
            Some(PrefValue::F64(value)),
            "exact f64 round trip for {value}"
        );
    }

    // NaN policy: exact bit-pattern round trip, but IEEE 754 NaN never
    // equals itself with `==` — assert `is_nan()`, not `PrefValue`
    // equality (see `file`'s module doc).
    backend.set("f64_probe", PrefValue::F64(f64::NAN)).unwrap();
    match backend.get("f64_probe") {
        Some(PrefValue::F64(v)) => assert!(v.is_nan(), "NaN must round-trip as NaN"),
        other => panic!("expected a stored NaN f64, got {other:?}"),
    }
}

fn string_list_empty_and_unicode(backend: &dyn Backend) {
    backend
        .set("list_probe", PrefValue::StrList(vec![]))
        .unwrap();
    assert_eq!(backend.get("list_probe"), Some(PrefValue::StrList(vec![])));

    let unicode = vec![
        String::new(),
        "".into(),
        "héllo".into(),
        "日本語".into(),
        "emoji 🎉✨".into(),
        "  spaces  ".into(),
    ];
    backend
        .set("list_probe", PrefValue::StrList(unicode.clone()))
        .unwrap();
    assert_eq!(backend.get("list_probe"), Some(PrefValue::StrList(unicode)));
}

fn remove_semantics(backend: &dyn Backend) {
    backend.set("removable", PrefValue::Bool(true)).unwrap();
    assert!(backend.get("removable").is_some());

    backend.remove("removable").unwrap();
    assert_eq!(backend.get("removable"), None);

    // Removing an absent key is a no-op success, not an error.
    backend.remove("removable").unwrap();
    backend.remove("never_existed_at_all").unwrap();
}

fn contains_semantics(backend: &dyn Backend) {
    assert!(!backend.contains("contains_probe"));
    backend.set("contains_probe", PrefValue::I64(1)).unwrap();
    assert!(backend.contains("contains_probe"));
    backend.remove("contains_probe").unwrap();
    assert!(!backend.contains("contains_probe"));
}

fn keys_semantics(backend: &dyn Backend) {
    backend.clear().unwrap();
    assert!(backend.keys().is_empty());

    backend.set("k1", PrefValue::Bool(true)).unwrap();
    backend.set("k2", PrefValue::I64(2)).unwrap();
    backend.set("k3", PrefValue::Str("v".into())).unwrap();

    let mut keys = backend.keys();
    keys.sort();
    assert_eq!(
        keys,
        vec!["k1".to_string(), "k2".to_string(), "k3".to_string()]
    );
}

fn clear_semantics(backend: &dyn Backend) {
    backend.set("to_clear_1", PrefValue::Bool(true)).unwrap();
    backend.set("to_clear_2", PrefValue::I64(1)).unwrap();
    assert!(!backend.keys().is_empty());

    backend.clear().unwrap();
    assert!(backend.keys().is_empty());
    assert_eq!(backend.get("to_clear_1"), None);
    assert_eq!(backend.get("to_clear_2"), None);
}
