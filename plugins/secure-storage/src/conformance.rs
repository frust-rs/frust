//! Shared conformance suite for every [`Backend`] implementation.
//!
//! `#[cfg(test)]`-only: [`run_conformance_suite`] exercises the full backend
//! contract — string round trips (including empty and unicode), overwrite,
//! missing-key `None`, `remove`/`contains`/`keys`/`clear` semantics, and
//! **store-name isolation** — against any backend factory.
//!
//! The factory (`open: &dyn Fn(&str) -> Box<dyn Backend>`) opens a named
//! store from the same backing storage, so the suite can assert that two
//! stores with different names never see each other's keys. `file`'s test
//! module is this task's caller (against tempdir-isolated
//! [`crate::file::FileStore`]s); later phases' Apple/Android/desktop backend
//! test modules are expected to call the same function (on-target or manual
//! where CI can't run them), so a single assertion set validates every
//! backend.

use crate::Backend;

/// Run the full suite against stores produced by `open`. Each store `open`
/// returns must start out empty (a fresh store) — the suite both asserts on
/// and mutates their state.
pub(crate) fn run_conformance_suite(open: &dyn Fn(&str) -> Box<dyn Backend>) {
    let backend = open("main");
    round_trip_strings(backend.as_ref());
    missing_key_is_none(backend.as_ref());
    overwrite(backend.as_ref());
    empty_and_unicode(backend.as_ref());
    remove_semantics(backend.as_ref());
    contains_semantics(backend.as_ref());
    keys_semantics(backend.as_ref());
    clear_semantics(backend.as_ref());
    store_name_isolation(open);
}

fn round_trip_strings(backend: &dyn Backend) {
    backend.set("token", "abc123").unwrap();
    assert_eq!(backend.get("token").unwrap(), Some("abc123".to_string()));
}

fn missing_key_is_none(backend: &dyn Backend) {
    assert_eq!(backend.get("this_key_was_never_set").unwrap(), None);
}

fn overwrite(backend: &dyn Backend) {
    backend.set("k", "first").unwrap();
    assert_eq!(backend.get("k").unwrap(), Some("first".to_string()));

    backend.set("k", "second").unwrap();
    assert_eq!(backend.get("k").unwrap(), Some("second".to_string()));
}

fn empty_and_unicode(backend: &dyn Backend) {
    backend.set("empty", "").unwrap();
    assert_eq!(backend.get("empty").unwrap(), Some(String::new()));

    let unicode = "héllo 日本語 🎉✨  spaces  ";
    backend.set("unicode", unicode).unwrap();
    assert_eq!(backend.get("unicode").unwrap(), Some(unicode.to_string()));

    // Unicode is also valid in a key.
    backend.set("clé-日本", "v").unwrap();
    assert_eq!(backend.get("clé-日本").unwrap(), Some("v".to_string()));
}

fn remove_semantics(backend: &dyn Backend) {
    backend.set("removable", "x").unwrap();
    assert!(backend.get("removable").unwrap().is_some());

    backend.remove("removable").unwrap();
    assert_eq!(backend.get("removable").unwrap(), None);

    // Removing an absent key is a no-op success, not an error.
    backend.remove("removable").unwrap();
    backend.remove("never_existed_at_all").unwrap();
}

fn contains_semantics(backend: &dyn Backend) {
    assert!(!backend.contains("contains_probe").unwrap());
    backend.set("contains_probe", "1").unwrap();
    assert!(backend.contains("contains_probe").unwrap());
    backend.remove("contains_probe").unwrap();
    assert!(!backend.contains("contains_probe").unwrap());
}

fn keys_semantics(backend: &dyn Backend) {
    backend.clear().unwrap();
    assert!(backend.keys().unwrap().is_empty());

    backend.set("k1", "a").unwrap();
    backend.set("k2", "b").unwrap();
    backend.set("k3", "c").unwrap();

    let mut keys = backend.keys().unwrap();
    keys.sort();
    assert_eq!(
        keys,
        vec!["k1".to_string(), "k2".to_string(), "k3".to_string()]
    );
}

fn clear_semantics(backend: &dyn Backend) {
    backend.set("to_clear_1", "a").unwrap();
    backend.set("to_clear_2", "b").unwrap();
    assert!(!backend.keys().unwrap().is_empty());

    backend.clear().unwrap();
    assert!(backend.keys().unwrap().is_empty());
    assert_eq!(backend.get("to_clear_1").unwrap(), None);
    assert_eq!(backend.get("to_clear_2").unwrap(), None);
}

/// Two stores opened with different names are fully isolated: a write to one
/// is invisible to the other, and clearing one leaves the other intact.
fn store_name_isolation(open: &dyn Fn(&str) -> Box<dyn Backend>) {
    let alpha = open("alpha");
    let beta = open("beta");

    alpha.clear().unwrap();
    beta.clear().unwrap();

    alpha.set("shared_key", "from-alpha").unwrap();
    beta.set("shared_key", "from-beta").unwrap();

    // The same key name in two stores holds two independent values.
    assert_eq!(
        alpha.get("shared_key").unwrap(),
        Some("from-alpha".to_string())
    );
    assert_eq!(
        beta.get("shared_key").unwrap(),
        Some("from-beta".to_string())
    );

    // A key set in one store does not appear in the other.
    alpha.set("alpha_only", "1").unwrap();
    assert_eq!(beta.get("alpha_only").unwrap(), None);
    assert!(!beta.contains("alpha_only").unwrap());

    // Clearing one store leaves the other's entries intact.
    alpha.clear().unwrap();
    assert_eq!(alpha.get("shared_key").unwrap(), None);
    assert_eq!(
        beta.get("shared_key").unwrap(),
        Some("from-beta".to_string())
    );
}
