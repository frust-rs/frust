//! Recent-projects persistence for the project switcher: a small
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

use frust_dap::ide_config::{ParentIde, parse_ide_name};
use frust_drive::doctor::{EnvLookup, RealEnv};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use super::dap_settings::{DapSetting, persisted_ide_name};
use super::state::{SIDEBAR_DEFAULT_WIDTH, clamp_sidebar_width};

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

/// Merge the persisted recent list with freshly `detect`ed project roots:
/// every recent entry that still exists on disk, in recency
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

// ── Settings persistence: sidebar width, mouse-capture preference ──────────

/// Persisted workbench preferences (`[settings]` table in `tui.toml`),
/// alongside the `[recent].projects` array above — a fresh launch's
/// "last-active project" is already covered by that list (`AppState::new`
/// opens `projects.first()` when the cwd has no project of its own), so it
/// carries no separate key here. Follow-tail is per-session, in-memory-only
/// state (every session starts following) and is deliberately not persisted
/// here — see `engine::update`'s `RegisterSession`/`ToggleFollow` handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// The sidebar's drag-resized width (columns), from the sidebar splitter.
    pub sidebar_width: u16,
    /// Whether crossterm mouse capture is on.
    pub mouse_capture: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            mouse_capture: true,
        }
    }
}

/// Load the persisted settings against the real process environment. A
/// missing file, an unresolvable config dir, a corrupt document, or a
/// missing/malformed individual key all fall back to that key's
/// [`Settings::default`] value — never a panic, and never a partial load
/// blocking the other keys (mirrors [`load_recent_projects`]'s tolerance).
pub fn load_settings() -> Settings {
    load_settings_with(&RealEnv)
}

fn load_settings_with(env: &dyn EnvLookup) -> Settings {
    let Some(path) = config_path(env) else {
        return Settings::default();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return Settings::default();
    };
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return Settings::default();
    };
    settings_from_doc(&doc)
}

fn settings_from_doc(doc: &DocumentMut) -> Settings {
    let default = Settings::default();
    let table = doc.as_table().get("settings").and_then(Item::as_table);
    let sidebar_width = table
        .and_then(|t| t.get("sidebar_width"))
        .and_then(Item::as_integer)
        .and_then(|n| u16::try_from(n).ok())
        .map(clamp_sidebar_width)
        .unwrap_or(default.sidebar_width);
    let mouse_capture = table
        .and_then(|t| t.get("mouse_capture"))
        .and_then(Item::as_bool)
        .unwrap_or(default.mouse_capture);
    Settings {
        sidebar_width,
        mouse_capture,
    }
}

/// Persist the sidebar's current width — the runner's enactment of a
/// completed `SidebarSplitter` drag (see
/// `engine::update`'s `DragEnd` handling).
pub fn save_sidebar_width(width: u16) {
    save_sidebar_width_with(&RealEnv, width);
}

fn save_sidebar_width_with(env: &dyn EnvLookup, width: u16) {
    save_setting(env, "sidebar_width", Value::from(i64::from(width)));
}

/// Persist the mouse-capture preference — the runner's enactment alongside
/// [`super::update::Effect::SetMouseCapture`]'s terminal-level toggle.
pub fn save_mouse_capture(on: bool) {
    save_mouse_capture_with(&RealEnv, on);
}

fn save_mouse_capture_with(env: &dyn EnvLookup, on: bool) {
    save_setting(env, "mouse_capture", Value::from(on));
}

/// Format-preserving save of exactly one `[settings].<key>` — mirrors
/// [`save_recent_project`]'s "parse, touch one key, write back whole"
/// pattern, so a hand-edited file's comments/unrelated keys always survive
/// (best-effort: an unresolvable config dir or an unwritable filesystem is
/// silently dropped, settings being a convenience rather than load-bearing
/// state).
fn save_setting(env: &dyn EnvLookup, key: &str, value: Value) {
    save_in_table(env, "settings", key, Some(value));
}

/// [`save_setting`] over any top-level table, with `None` *removing* the key
/// (how an override is cleared back to its default). The whole document is
/// parsed and written back, so every comment, table, and unrelated key in it
/// survives untouched — the one write path both `[settings]` and `[dap]` use.
fn save_in_table(env: &dyn EnvLookup, table: &str, key: &str, value: Option<Value>) {
    let Some(path) = config_path(env) else {
        return;
    };
    let mut doc = fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.parse::<DocumentMut>().ok())
        .unwrap_or_default();

    let root = doc.as_table_mut();
    if !root.contains_table(table) {
        root.insert(table, Item::Table(Table::new()));
    }
    if let Some(target) = root.get_mut(table).and_then(Item::as_table_mut) {
        match value {
            Some(value) => {
                // `insert` replaces the whole entry, decor included — so a
                // comment the user wrote above *this* key would be dropped by
                // a plain overwrite. Carry the existing key's leading decor
                // (that comment, and the blank lines around it) across.
                let decor = target.key(key).map(|k| k.leaf_decor().clone());
                target.insert(key, Item::Value(value));
                if let (Some(decor), Some(mut key)) = (decor, target.key_mut(key)) {
                    *key.leaf_decor_mut() = decor;
                }
            }
            None => {
                target.remove(key);
            }
        }
    }

    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let _ = fs::write(path, doc.to_string());
}

// ── DAP preferences: the `[dap]` table ─────────────────────────────────────

/// The persisted `[dap]` preferences backing the DAP settings dialog
/// ([`super::DapSettings`]) — the embedded debug-adapter server's launch and
/// IDE-configuration behaviour, alongside `[settings]` and `[recent]` in the
/// same `tui.toml`.
///
/// Loaded once at [`AppState::new`](super::AppState::new); every dialog edit
/// writes exactly its own key back through [`save_dap_setting`], so a
/// hand-edited file keeps its comments and its other keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DapPrefs {
    /// Start the server on every launch, IDE or not (`false`: opt-in).
    pub enabled: bool,
    /// Start it automatically when an IDE terminal is detected.
    pub auto_start_in_ide: bool,
    /// Write/refresh the IDE's DAP client config when the server binds.
    pub auto_configure_ide: bool,
    /// The port the server binds.
    pub port: u16,
    /// An explicit IDE override, parsed through
    /// `frust_dap::ide_config::parse_ide_name`. An unrecognised name in the
    /// file is ignored (falls back to detection) rather than failing the load.
    pub ide_override: Option<ParentIde>,
}

impl Default for DapPrefs {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_start_in_ide: true,
            auto_configure_ide: true,
            port: frust_dap::DEFAULT_DAP_PORT,
            ide_override: None,
        }
    }
}

/// Load the persisted `[dap]` preferences against the real process
/// environment — the same tolerance as [`load_settings`]: a missing file, a
/// corrupt document, or a missing/malformed individual key falls back to that
/// key's default rather than failing the load.
pub fn load_dap_prefs() -> DapPrefs {
    load_dap_prefs_with(&RealEnv)
}

fn load_dap_prefs_with(env: &dyn EnvLookup) -> DapPrefs {
    let Some(path) = config_path(env) else {
        return DapPrefs::default();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return DapPrefs::default();
    };
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return DapPrefs::default();
    };
    dap_prefs_from_doc(&doc)
}

fn dap_prefs_from_doc(doc: &DocumentMut) -> DapPrefs {
    let default = DapPrefs::default();
    let table = doc.as_table().get("dap").and_then(Item::as_table);
    let flag = |key: &str, fallback: bool| {
        table
            .and_then(|t| t.get(key))
            .and_then(Item::as_bool)
            .unwrap_or(fallback)
    };
    DapPrefs {
        enabled: flag("enabled", default.enabled),
        auto_start_in_ide: flag("auto_start_in_ide", default.auto_start_in_ide),
        auto_configure_ide: flag("auto_configure_ide", default.auto_configure_ide),
        port: table
            .and_then(|t| t.get("port"))
            .and_then(Item::as_integer)
            .and_then(|n| u16::try_from(n).ok())
            .unwrap_or(default.port),
        ide_override: table
            .and_then(|t| t.get("ide_override"))
            .and_then(Item::as_str)
            .and_then(|name| parse_ide_name(name).ok()),
    }
}

/// Persist one just-changed `[dap]` preference — the runner's enactment of
/// [`super::Effect::SaveDapSetting`], mirroring how `sidebar_width`/
/// `mouse_capture` travel out of the pure engine.
pub fn save_dap_setting(setting: DapSetting) {
    save_dap_setting_with(&RealEnv, setting);
}

fn save_dap_setting_with(env: &dyn EnvLookup, setting: DapSetting) {
    match setting {
        DapSetting::AutoStartInIde(on) => {
            save_in_table(env, "dap", "auto_start_in_ide", Some(Value::from(on)));
        }
        DapSetting::AutoConfigureIde(on) => {
            save_in_table(env, "dap", "auto_configure_ide", Some(Value::from(on)));
        }
        DapSetting::Port(port) => {
            save_in_table(env, "dap", "port", Some(Value::from(i64::from(port))));
        }
        // An override with no persistable name (never offered by the selector
        // — see `dap_settings::IDE_OVERRIDES`) clears the key rather than
        // writing a name the loader could not read back.
        DapSetting::IdeOverride(ide) => {
            let name = ide.and_then(persisted_ide_name);
            save_in_table(env, "dap", "ide_override", name.map(Value::from));
        }
    }
}

/// Persist `[dap].enabled` — "start the server on every launch". The dialog
/// exposes no control for it (see [`super::DapSettings::enabled`]); this is
/// the write half of the key a user sets by hand, kept beside the others so
/// the table has one owner.
pub fn save_dap_enabled(on: bool) {
    save_dap_enabled_with(&RealEnv, on);
}

fn save_dap_enabled_with(env: &dyn EnvLookup, on: bool) {
    save_in_table(env, "dap", "enabled", Some(Value::from(on)));
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

    // ── Settings persistence ────────────────────────────────────────────

    #[test]
    fn missing_settings_file_yields_defaults() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        assert_eq!(load_settings_with(&env), Settings::default());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn each_setting_round_trips_independently() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        save_sidebar_width_with(&env, 40);
        save_mouse_capture_with(&env, false);
        let loaded = load_settings_with(&env);
        assert_eq!(
            loaded,
            Settings {
                sidebar_width: 40,
                mouse_capture: false,
            }
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_saved_sidebar_width_is_clamped_to_the_drag_resize_bounds_on_load() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        // A value outside the drag-resize bounds (e.g. hand-edited, or a
        // stale value from before the bounds tightened) clamps on load
        // rather than producing an out-of-range sidebar width.
        save_sidebar_width_with(&env, 9999);
        assert_eq!(
            load_settings_with(&env).sidebar_width,
            crate::engine::SIDEBAR_MAX_WIDTH
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn saving_one_setting_leaves_the_others_at_default_and_preserves_recent() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        record_recent_project_with(&env, Path::new("/tmp/huddle"));
        save_mouse_capture_with(&env, false);
        let loaded = load_settings_with(&env);
        assert!(!loaded.mouse_capture);
        assert_eq!(loaded.sidebar_width, Settings::default().sidebar_width);
        assert_eq!(
            load_recent_projects_with(&env),
            vec![PathBuf::from("/tmp/huddle")],
            "the [recent] table must survive a [settings] save"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_corrupt_settings_file_yields_defaults_not_a_crash() {
        let home = unique_temp_home();
        let config = home.join(".config").join("frust");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("tui.toml"), "not [valid toml").unwrap();
        let env = FakeEnv::home(&home);
        assert_eq!(load_settings_with(&env), Settings::default());
        let _ = fs::remove_dir_all(&home);
    }

    // ── DAP preferences (`[dap]`) ───────────────────────────────────────

    #[test]
    fn a_missing_dap_table_yields_defaults() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        assert_eq!(load_dap_prefs_with(&env), DapPrefs::default());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn every_dap_preference_round_trips() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        save_dap_enabled_with(&env, true);
        save_dap_setting_with(&env, DapSetting::AutoStartInIde(false));
        save_dap_setting_with(&env, DapSetting::AutoConfigureIde(false));
        save_dap_setting_with(&env, DapSetting::Port(5005));
        save_dap_setting_with(&env, DapSetting::IdeOverride(Some(ParentIde::Zed)));
        assert_eq!(
            load_dap_prefs_with(&env),
            DapPrefs {
                enabled: true,
                auto_start_in_ide: false,
                auto_configure_ide: false,
                port: 5005,
                ide_override: Some(ParentIde::Zed),
            }
        );
        let _ = fs::remove_dir_all(&home);
    }

    /// Clearing the override removes the key outright rather than writing an
    /// empty string the loader would then have to special-case.
    #[test]
    fn clearing_the_ide_override_removes_the_key() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        save_dap_setting_with(&env, DapSetting::IdeOverride(Some(ParentIde::Neovim)));
        assert_eq!(
            load_dap_prefs_with(&env).ide_override,
            Some(ParentIde::Neovim)
        );
        save_dap_setting_with(&env, DapSetting::IdeOverride(None));
        assert_eq!(load_dap_prefs_with(&env).ide_override, None);
        let saved = fs::read_to_string(home.join(".config/frust/tui.toml")).unwrap();
        assert!(
            !saved.contains("ide_override"),
            "the key must be removed, not blanked:\n{saved}"
        );
        let _ = fs::remove_dir_all(&home);
    }

    /// An IDE whose name `parse_ide_name` cannot read back is never written —
    /// a persisted value that silently forgets itself would be worse than no
    /// value at all.
    #[test]
    fn an_unnameable_ide_override_is_not_persisted() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        save_dap_setting_with(&env, DapSetting::IdeOverride(Some(ParentIde::Cursor)));
        assert_eq!(load_dap_prefs_with(&env).ide_override, None);
        let _ = fs::remove_dir_all(&home);
    }

    /// The format-preservation contract for the second table: a `[dap]` save
    /// touches one key and leaves every comment, `[settings]` key, and
    /// `[recent]` entry in the file untouched.
    #[test]
    fn a_dap_save_preserves_comments_and_the_other_tables() {
        let home = unique_temp_home();
        let config_dir = home.join(".config").join("frust");
        fs::create_dir_all(&config_dir).unwrap();
        let path = config_dir.join("tui.toml");
        fs::write(
            &path,
            "# my hand-written note\n[recent]\nprojects = [\"/tmp/old\"]\n\n\
             [settings]\nsidebar_width = 30\n\n[dap]\n# which port the editor attaches to\nport = 4849\n",
        )
        .unwrap();

        let env = FakeEnv::home(&home);
        save_dap_setting_with(&env, DapSetting::Port(5005));

        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("# my hand-written note"), "{saved}");
        assert!(
            saved.contains("# which port the editor attaches to"),
            "{saved}"
        );
        assert!(saved.contains("sidebar_width = 30"), "{saved}");
        assert_eq!(load_dap_prefs_with(&env).port, 5005);
        assert_eq!(load_settings_with(&env).sidebar_width, 30);
        assert_eq!(
            load_recent_projects_with(&env),
            vec![PathBuf::from("/tmp/old")],
            "the [recent] table must survive a [dap] save"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_malformed_dap_value_falls_back_to_that_keys_default() {
        let home = unique_temp_home();
        let config = home.join(".config").join("frust");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("tui.toml"),
            "[dap]\nport = 99999\nenabled = \"yes\"\nide_override = \"sublime\"\n\
             auto_configure_ide = false\n",
        )
        .unwrap();
        let env = FakeEnv::home(&home);
        assert_eq!(
            load_dap_prefs_with(&env),
            DapPrefs {
                // Out of `u16` range, not a bool, and not a known IDE: each
                // key falls back on its own without blocking the others.
                port: DapPrefs::default().port,
                enabled: DapPrefs::default().enabled,
                ide_override: None,
                auto_configure_ide: false,
                auto_start_in_ide: true,
            }
        );
        let _ = fs::remove_dir_all(&home);
    }

    /// A `tui.toml` written by an older build still carries the now-removed
    /// `follow_tail_default` key (follow-tail is per-session, in-memory-only
    /// state now — see `Settings`'s doc comment). Loading it must not error
    /// and must ignore the stale key, recovering only the settings still
    /// modeled.
    #[test]
    fn a_legacy_settings_file_with_the_removed_follow_tail_key_loads_without_error() {
        let home = unique_temp_home();
        let config = home.join(".config").join("frust");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("tui.toml"),
            "[settings]\nsidebar_width = 30\nmouse_capture = false\nfollow_tail_default = false\n",
        )
        .unwrap();
        let env = FakeEnv::home(&home);
        let loaded = load_settings_with(&env);
        assert_eq!(
            loaded,
            Settings {
                sidebar_width: 30,
                mouse_capture: false,
            },
            "the stale key is ignored, not an error"
        );
        let _ = fs::remove_dir_all(&home);
    }
}
