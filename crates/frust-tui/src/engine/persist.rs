//! Recent-projects persistence (PLAN.md D6b's project switcher): a small
//! `~/.config/frust/tui.toml` (`$XDG_CONFIG_HOME` respected, per the XDG
//! base-directory spec's documented fallback) tracking the
//! most-recently-opened project roots, newest first.
//!
//! Every write goes through `toml_edit`'s document editing rather than a
//! deserialize/mutate/reserialize round trip, so a user's hand-added
//! comments/keys in the file survive a save — only the `[recent].projects`
//! array is ever touched.
//!
//! Uses `frust_drive::doctor::EnvLookup` (rather than mutating the real,
//! process-global, unsafe-to-mutate environment) so `$HOME`/
//! `$XDG_CONFIG_HOME` resolution is unit-testable without racing other
//! tests over global env state — the same fixture-driven pattern
//! `docs/CODE_STANDARDS.md` documents for `frust-drive`'s validators.

use std::fs;
use std::path::{Path, PathBuf};

use frust_drive::doctor::{EnvLookup, RealEnv};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

/// Cap on the persisted recent-projects list (newest first) — trimmed on
/// every save so the file never grows unbounded on a long-lived machine.
const MAX_RECENT: usize = 20;

/// The config file path: `$XDG_CONFIG_HOME/frust/tui.toml`, falling back to
/// `~/.config/frust/tui.toml`. `None` if neither `XDG_CONFIG_HOME` nor
/// `HOME` resolves — no persistence that run, never a hard error.
fn config_path(env: &dyn EnvLookup) -> Option<PathBuf> {
    config_dir(env).map(|dir| dir.join("frust").join("tui.toml"))
}

fn config_dir(env: &dyn EnvLookup) -> Option<PathBuf> {
    if let Some(xdg) = env.get("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg));
    }
    env.get("HOME")
        .map(|home| PathBuf::from(home).join(".config"))
}

/// Load the persisted recent-projects list (newest first) against the real
/// process environment. A missing file, an unresolvable config dir, or a
/// corrupt document all yield an empty list — never a panic.
pub fn load_recent_projects() -> Vec<PathBuf> {
    load_recent_projects_with(&RealEnv)
}

/// [`load_recent_projects`], with an injected [`EnvLookup`] (tests).
fn load_recent_projects_with(env: &dyn EnvLookup) -> Vec<PathBuf> {
    match config_path(env) {
        Some(path) => recent_from_path(&path),
        None => Vec::new(),
    }
}

fn recent_from_path(path: &Path) -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return Vec::new();
    };
    recent_from_doc(&doc)
}

fn recent_from_doc(doc: &DocumentMut) -> Vec<PathBuf> {
    doc.as_table()
        .get("recent")
        .and_then(Item::as_table)
        .and_then(|t| t.get("projects"))
        .and_then(Item::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Record `project` as the most-recently-opened (moved to the front,
/// deduped, capped at [`MAX_RECENT`]) against the real process environment.
/// Best-effort: recent-projects is a convenience, not load-bearing state, so
/// any failure (unresolvable config dir, a read-only filesystem, …) is
/// silently dropped rather than surfaced.
pub fn record_recent_project(project: &Path) {
    record_recent_project_with(&RealEnv, project);
}

/// [`record_recent_project`], with an injected [`EnvLookup`] (tests).
fn record_recent_project_with(env: &dyn EnvLookup, project: &Path) {
    if let Some(path) = config_path(env) {
        save_recent_project(&path, project);
    }
}

/// Format-preserving save: parses the existing file (if any) into a
/// [`DocumentMut`], rewrites only the `[recent].projects` array in place,
/// and writes the whole document back — every other key/table and every
/// comment in the source survives untouched.
fn save_recent_project(path: &Path, project: &Path) {
    let mut doc = fs::read_to_string(path)
        .ok()
        .and_then(|text| text.parse::<DocumentMut>().ok())
        .unwrap_or_default();

    let mut recent = recent_from_doc(&doc);
    recent.retain(|p| p != project);
    recent.insert(0, project.to_path_buf());
    recent.truncate(MAX_RECENT);

    let array: Array = recent
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    let root = doc.as_table_mut();
    if !root.contains_table("recent") {
        root.insert("recent", Item::Table(Table::new()));
    }
    if let Some(recent_table) = root.get_mut("recent").and_then(Item::as_table_mut) {
        recent_table.insert("projects", Item::Value(Value::Array(array)));
    }

    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let _ = fs::write(path, doc.to_string());
}

/// Merge the persisted recent list with freshly `detect`ed project roots
/// (PLAN.md D6b): every recent entry that still exists on disk, in recency
/// order, followed by any detected root not already present — deduped
/// throughout. A recent entry that no longer exists (moved/deleted since
/// last use) is silently dropped rather than shown as a dead switcher row.
pub fn merge_recent_and_detected(recent: &[PathBuf], detected: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(recent.len() + detected.len());
    for p in recent {
        if p.is_dir() && !out.contains(p) {
            out.push(p.clone());
        }
    }
    for p in detected {
        if !out.contains(p) {
            out.push(p.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A local [`EnvLookup`] fixture (mirrors `frust_drive::doctor`'s
    /// crate-private `FakeEnv`) so these tests never touch the real, shared
    /// process environment.
    struct FakeEnv(HashMap<String, String>);

    impl FakeEnv {
        fn home(home: &Path) -> Self {
            let mut m = HashMap::new();
            m.insert("HOME".to_string(), home.to_string_lossy().into_owned());
            Self(m)
        }
    }

    impl EnvLookup for FakeEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    /// A fresh, unique temp dir standing in for `$HOME` this test run.
    fn unique_temp_home() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("frust-tui-persist-test-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_yields_empty_recent_list() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        assert!(load_recent_projects_with(&env).is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn corrupt_file_yields_empty_recent_list_not_a_crash() {
        let home = unique_temp_home();
        let config = home.join(".config").join("frust");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("tui.toml"), "not [valid toml").unwrap();
        let env = FakeEnv::home(&home);
        assert!(load_recent_projects_with(&env).is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn record_then_load_round_trips_newest_first() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        record_recent_project_with(&env, Path::new("/tmp/huddle"));
        record_recent_project_with(&env, Path::new("/tmp/bubblebench"));
        let recent = load_recent_projects_with(&env);
        assert_eq!(
            recent,
            vec![
                PathBuf::from("/tmp/bubblebench"),
                PathBuf::from("/tmp/huddle"),
            ],
            "newest recorded project first"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn re_recording_an_existing_entry_moves_it_to_front_without_duplicating() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        record_recent_project_with(&env, Path::new("/tmp/a"));
        record_recent_project_with(&env, Path::new("/tmp/b"));
        record_recent_project_with(&env, Path::new("/tmp/a"));
        let recent = load_recent_projects_with(&env);
        assert_eq!(
            recent,
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn recent_list_is_capped_at_max() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        for i in 0..(MAX_RECENT + 5) {
            record_recent_project_with(&env, &PathBuf::from(format!("/tmp/p{i}")));
        }
        let recent = load_recent_projects_with(&env);
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(
            recent[0],
            PathBuf::from(format!("/tmp/p{}", MAX_RECENT + 4)),
            "newest survives at the front"
        );
        let _ = fs::remove_dir_all(&home);
    }

    /// The format-preservation contract: a hand-edited file's comment and an
    /// unrelated table/key survive a save — only `[recent].projects` is
    /// rewritten.
    #[test]
    fn save_preserves_comments_and_unrelated_keys() {
        let home = unique_temp_home();
        let config_dir = home.join(".config").join("frust");
        fs::create_dir_all(&config_dir).unwrap();
        let path = config_dir.join("tui.toml");
        fs::write(
            &path,
            "# my hand-written note\n[recent]\nprojects = [\"/tmp/old\"]\n\n[other]\nkeep = true\n",
        )
        .unwrap();

        let env = FakeEnv::home(&home);
        record_recent_project_with(&env, Path::new("/tmp/new"));

        let saved = fs::read_to_string(&path).unwrap();
        assert!(
            saved.contains("# my hand-written note"),
            "comment must survive a format-preserving save:\n{saved}"
        );
        assert!(
            saved.contains("keep = true"),
            "an unrelated table/key must survive:\n{saved}"
        );
        let recent = load_recent_projects_with(&env);
        assert_eq!(
            recent,
            vec![PathBuf::from("/tmp/new"), PathBuf::from("/tmp/old")]
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn merge_puts_existing_recent_first_deduped_then_detected() {
        let home = unique_temp_home(); // a real, existing dir stand-in
        let sibling = home.join("child");
        fs::create_dir_all(&sibling).unwrap();
        let recent = vec![sibling.clone(), PathBuf::from("/does/not/exist")];
        let detected = vec![sibling.clone(), PathBuf::from("/tmp/other-detected")];
        let merged = merge_recent_and_detected(&recent, &detected);
        assert_eq!(
            merged,
            vec![sibling.clone(), PathBuf::from("/tmp/other-detected")],
            "existing recent entry first, deduped against detected; a dead \
             recent entry is dropped"
        );
        let _ = fs::remove_dir_all(&home);
    }
}
