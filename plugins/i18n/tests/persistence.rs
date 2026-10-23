//! The `persistence` feature: restore -> `set_locale` -> new handle, plus the
//! manifest check that keeps the plugin edge out of the default graph.

use std::fs;
use std::path::PathBuf;

#[test]
fn shared_preferences_is_optional_and_only_the_persistence_feature_pulls_it_in() {
    let manifest = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("reads Cargo.toml");
    let declaration = manifest
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("frust-shared-preferences = "))
        .expect("declared");
    assert!(declaration.contains("optional = true"), "{declaration}");

    let pulling: Vec<&str> = manifest
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && line.contains("\"dep:frust-shared-preferences\""))
        .collect();
    assert_eq!(pulling.len(), 1, "{pulling:?}");
    assert!(pulling[0].starts_with("persistence = "), "{pulling:?}");

    let default = manifest
        .lines()
        .find(|line| line.starts_with("default = "))
        .expect("default feature list");
    assert!(!default.contains("persistence"), "{default}");
}

#[cfg(feature = "persistence")]
mod enabled {
    use std::sync::Mutex;

    use frust_i18n::{I18n, LOCALE_KEY, Locale, LocaleSet, persisted_locale};
    use frust_reactive::Owner;
    use frust_shared_preferences::SharedPreferences;

    /// The store is process-global, so the tests take turns.
    static STORE: Mutex<()> = Mutex::new(());

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    fn set() -> LocaleSet {
        LocaleSet::new(locale("en"))
            .with_locale(locale("en"), [("app.ftl", "hello = Hello")])
            .with_locale(locale("de"), [("app.ftl", "hello = Hallo")])
            .with_isolating(false)
    }

    /// A store in a temp dir where the target uses the JSON-file backend;
    /// the native backends have no redirect, so the key is cleared around
    /// each test instead.
    fn prefs(dir: &std::path::Path) -> SharedPreferences {
        // SAFETY: serialised by `STORE`; no other thread reads the environment.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", dir);
            std::env::set_var("APPDATA", dir);
        }
        let prefs = SharedPreferences::standard().expect("standard store");
        prefs.remove(LOCALE_KEY).expect("clears the key");
        prefs
    }

    #[test]
    fn restores_then_saves_then_restores_again() {
        let _turn = STORE.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile_dir("roundtrip");
        let _owner = {
            let owner = Owner::new();
            owner.set();
            owner
        };
        let prefs = prefs(&dir);

        // Nothing saved: the system list decides.
        let first = I18n::with_persistence(set(), &[locale("en")], prefs.clone()).expect("builds");
        assert_eq!(first.locale().to_string(), "en");
        assert_eq!(persisted_locale(&prefs), None);

        // The explicit choice is saved.
        first.set_locale(locale("de"));
        assert_eq!(persisted_locale(&prefs), Some(locale("de")));

        // A new handle restores it over the system list.
        let second = I18n::with_persistence(set(), &[locale("en")], prefs.clone()).expect("builds");
        assert_eq!(second.locale().to_string(), "de");
        assert_eq!(second.t("hello"), "Hallo");

        // Clearing returns the next launch to the system list.
        second.clear_persisted_locale();
        assert_eq!(persisted_locale(&prefs), None);
        let third = I18n::with_persistence(set(), &[locale("en")], prefs.clone()).expect("builds");
        assert_eq!(third.locale().to_string(), "en");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_retained_region_is_saved_and_an_unparsable_value_is_ignored() {
        let _turn = STORE.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile_dir("region");
        let owner = Owner::new();
        owner.set();
        let prefs = prefs(&dir);

        let i18n =
            I18n::with_persistence(set(), &[locale("de-CH")], prefs.clone()).expect("builds");
        i18n.set_locale(locale("de"));
        assert_eq!(persisted_locale(&prefs), Some(locale("de-CH")));

        prefs.set_string(LOCALE_KEY, "not a locale!").expect("sets");
        assert_eq!(persisted_locale(&prefs), None);
        let again = I18n::with_persistence(set(), &[locale("en")], prefs.clone()).expect("builds");
        assert_eq!(again.locale().to_string(), "en");

        again.clear_persisted_locale();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plain_handle_never_touches_the_store() {
        let _turn = STORE.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile_dir("plain");
        let owner = Owner::new();
        owner.set();
        let prefs = prefs(&dir);

        let plain = I18n::new(set(), &[locale("en")]).expect("builds");
        plain.set_locale(locale("de"));
        plain.clear_persisted_locale();
        assert_eq!(persisted_locale(&prefs), None);
        let _ = fs::remove_dir_all(&dir);
    }

    fn tempfile_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("frust-i18n-persist-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("creates temp dir");
        dir
    }

    use std::fs;
}
