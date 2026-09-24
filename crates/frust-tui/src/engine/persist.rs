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
use frust_drive::host_path;
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use super::dap_settings::{DapSetting, persisted_ide_name};
use super::state::{SIDEBAR_DEFAULT_WIDTH, clamp_sidebar_width};

/// Cap on the persisted recent-projects list (newest first) — trimmed on
/// every save so the file never grows unbounded on a long-lived machine.
const MAX_RECENT: usize = 20;

/// The config file path: `$XDG_CONFIG_HOME/frust/tui.toml`, falling back to
/// `<home>/.config/frust/tui.toml` where `<home>` is [`home_dir`] — `$HOME`,
/// then (on a host with none) `$USERPROFILE`, then `$HOMEDRIVE`+`$HOMEPATH`.
/// That chain is what makes a plain Windows console session (no `$HOME` set)
/// land on the *same* file an `ssh` session into the same machine already
/// writes, since OpenSSH's server sets `HOME=%USERPROFILE%` for the login
/// shell — an existing Windows store carries over rather than forking into a
/// second, console-only file. `None` if nothing in the chain resolves — no
/// persistence that run, never a hard error.
fn config_path(env: &dyn EnvLookup) -> Option<PathBuf> {
    config_dir(env).map(|dir| dir.join("frust").join("tui.toml"))
}

fn config_dir(env: &dyn EnvLookup) -> Option<PathBuf> {
    if let Some(xdg) = env.get("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg));
    }
    home_dir(env).map(|home| home.join(".config"))
}

/// The user's home directory: `$HOME`, then `$USERPROFILE`, then
/// `$HOMEDRIVE`+`$HOMEPATH`'s plain string concatenation (`HOMEDRIVE` is a
/// bare drive like `C:`, `HOMEPATH` a drive-relative path like `\Users\ed`;
/// `Path::join` would discard the drive on that rooted second component, so
/// this is concatenation, not a join) — mirrors
/// `frust_dap::ide_config::vscode`'s `home_dir` exactly, including treating
/// an empty value as unset.
///
/// The `$USERPROFILE`/`$HOMEDRIVE`+`$HOMEPATH` fallbacks are checked
/// unconditionally rather than gated behind `cfg!(windows)`: those variables
/// are practically never set on Linux/macOS, so the extra fallback changes
/// nothing there, and staying unconditional is what makes this function
/// exercisable — including its Windows-only tail — from a unit test on any
/// host via [`EnvLookup`], rather than needing an actual Windows target.
fn home_dir(env: &dyn EnvLookup) -> Option<PathBuf> {
    if let Some(home) = non_empty(env, "HOME") {
        return Some(PathBuf::from(home));
    }
    if let Some(profile) = non_empty(env, "USERPROFILE") {
        return Some(PathBuf::from(profile));
    }
    let drive = non_empty(env, "HOMEDRIVE")?;
    let path = non_empty(env, "HOMEPATH")?;
    Some(PathBuf::from(format!("{drive}{path}")))
}

/// A non-empty environment value, or `None` for unset-or-empty.
fn non_empty(env: &dyn EnvLookup, key: &str) -> Option<String> {
    env.get(key).filter(|value| !value.is_empty())
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

/// Reads `[recent].projects` off `doc`, running every entry through
/// [`host_path::simplify`] (the "paths entering state get simplified once at
/// the boundary" rule) and then collapsing duplicates ([`paths_match`],
/// first occurrence — i.e. most-recently-used — wins). The second step heals
/// a file a pre-[`host_path`] build already wrote with a duplicated entry
/// (e.g. a canonicalized verbatim path alongside the plain one for the same
/// project) on its very next load, without any user action.
fn recent_from_doc(doc: &DocumentMut) -> Vec<PathBuf> {
    let raw: Vec<PathBuf> = doc
        .as_table()
        .get("recent")
        .and_then(Item::as_table)
        .and_then(|t| t.get("projects"))
        .and_then(Item::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| host_path::simplify(Path::new(s)))
                .collect()
        })
        .unwrap_or_default();
    dedupe_first_wins(raw)
}

/// Collapse `paths` to its first occurrence of each distinct project
/// ([`paths_match`]), preserving order — the load-time duplicate healing
/// [`recent_from_doc`] applies.
fn dedupe_first_wins(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for p in paths {
        if !out.iter().any(|q| paths_match(q, &p)) {
            out.push(p);
        }
    }
    out
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

    let project = host_path::simplify(project);
    let mut recent = recent_from_doc(&doc);
    recent.retain(|p| !paths_match(p, &project));
    recent.insert(0, project);
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

/// Whether `path` is `root` itself or sits nested under it — [`host_path::is_under`]:
/// component-prefix containment after [`host_path::simplify`], case-insensitive
/// on Windows (so a path equal to `root` counts too, and `\\?\`-verbatim or
/// differently-cased Windows paths naming the same place still match), exact
/// component comparison elsewhere. Never touches the filesystem. This is the
/// one comparison [`split_local_and_previous`]'s "is this recent project
/// under the cwd" classification *and* [`AppState::insert_project`]'s
/// local/previous placement go through — the platform-aware seam every
/// project-identity call site shares rather than reimplementing.
///
/// [`AppState::insert_project`]: super::state::AppState::insert_project
pub(crate) fn path_is_within(path: &Path, root: &Path) -> bool {
    host_path::is_under(path, root)
}

/// Whether `a` and `b` name the same project root — [`host_path::same_path`],
/// the same platform-aware rule [`path_is_within`] uses.
fn paths_match(a: &Path, b: &Path) -> bool {
    host_path::same_path(a, b)
}

/// Split the persisted recent list and freshly `detect`ed project roots into
/// one ordered `AppState::projects` vec plus the boundary between its two
/// sections: `[..local_count]` is "local" — every `detected`
/// root, in walk order, followed by any *existing* recent entry that sits
/// under `cwd` but that the bounded walk missed (still local — appended at
/// the end of the local section, in recency order); `[local_count..]` is
/// "previous" — every other existing recent entry not already counted as
/// local, kept in recency order (most-recent first). A recent entry that no
/// longer exists on disk (moved/deleted since last use) is silently dropped
/// rather than shown as a dead switcher row; duplicates are dropped
/// throughout via [`paths_match`].
pub fn split_local_and_previous(
    cwd: &Path,
    recent: &[PathBuf],
    detected: &[PathBuf],
) -> (Vec<PathBuf>, usize) {
    let mut local: Vec<PathBuf> = detected.to_vec();
    let mut previous: Vec<PathBuf> = Vec::new();
    for p in recent {
        if !p.is_dir() || local.iter().any(|l| paths_match(l, p)) {
            continue;
        }
        if path_is_within(p, cwd) {
            local.push(p.clone());
        } else if !previous.iter().any(|q| paths_match(q, p)) {
            previous.push(p.clone());
        }
    }
    let local_count = local.len();
    local.extend(previous);
    (local, local_count)
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
    /// Whether the one-time "an auto-start would open a DAP listener" notice
    /// has already been shown (`false` on a fresh install). Burned exactly
    /// once, by the first auto-start that would actually have fired — see
    /// [`super::DapSettings::intro_port`].
    pub intro_seen: bool,
}

impl Default for DapPrefs {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_start_in_ide: true,
            auto_configure_ide: true,
            port: frust_dap::DEFAULT_DAP_PORT,
            ide_override: None,
            intro_seen: false,
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
        intro_seen: flag("intro_seen", default.intro_seen),
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
        DapSetting::IntroSeen(seen) => {
            save_in_table(env, "dap", "intro_seen", Some(Value::from(seen)));
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

        /// An arbitrary set of environment variables — the `home_dir`
        /// fallback-chain tests, which need `HOME` absent while other
        /// variables are set.
        fn with(pairs: &[(&str, &str)]) -> Self {
            Self(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            )
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

    // ── config_path / home_dir fallback chain ────────────────────────────

    #[test]
    fn xdg_config_home_wins_over_home_and_the_windows_fallbacks() {
        let env = FakeEnv::with(&[
            ("XDG_CONFIG_HOME", "/xdg"),
            ("HOME", "/home/ed"),
            ("USERPROFILE", r"C:\Users\ed"),
        ]);
        assert_eq!(
            config_path(&env),
            Some(PathBuf::from("/xdg").join("frust").join("tui.toml"))
        );
    }

    #[test]
    fn home_wins_over_userprofile_and_homedrive_homepath() {
        let env = FakeEnv::with(&[
            ("HOME", "/home/ed"),
            ("USERPROFILE", r"C:\Users\ed"),
            ("HOMEDRIVE", "C:"),
            ("HOMEPATH", r"\Users\ed"),
        ]);
        assert_eq!(home_dir(&env), Some(PathBuf::from("/home/ed")));
    }

    /// The Windows console gap this card fixes: no `$HOME`, but
    /// `$USERPROFILE` set — the file must land where an `ssh` session into
    /// the same machine already writes it (OpenSSH sets `HOME=%USERPROFILE%`).
    #[test]
    fn home_unset_falls_back_to_userprofile_for_the_config_path() {
        let env = FakeEnv::with(&[("USERPROFILE", r"C:\Users\cpu")]);
        assert_eq!(
            config_path(&env),
            Some(
                PathBuf::from(r"C:\Users\cpu")
                    .join(".config")
                    .join("frust")
                    .join("tui.toml")
            )
        );
    }

    #[test]
    fn home_and_userprofile_unset_falls_back_to_homedrive_and_homepath() {
        let env = FakeEnv::with(&[("HOMEDRIVE", "C:"), ("HOMEPATH", r"\Users\cpu")]);
        assert_eq!(home_dir(&env), Some(PathBuf::from(r"C:\Users\cpu")));
    }

    #[test]
    fn empty_values_are_treated_as_unset_and_fall_through_the_chain() {
        let env = FakeEnv::with(&[
            ("XDG_CONFIG_HOME", ""),
            ("HOME", ""),
            ("USERPROFILE", ""),
            ("HOMEDRIVE", "C:"),
            ("HOMEPATH", r"\Users\cpu"),
        ]);
        assert_eq!(home_dir(&env), Some(PathBuf::from(r"C:\Users\cpu")));
    }

    #[test]
    fn nothing_resolvable_yields_none_as_today() {
        let env = FakeEnv::with(&[]);
        assert_eq!(home_dir(&env), None);
        assert_eq!(config_path(&env), None);
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

    // ── split_local_and_previous (local/previous boundary) ──────────────

    #[test]
    fn a_cwd_project_plus_unrelated_recents_puts_cwd_first_with_local_count_one() {
        let home = unique_temp_home();
        let cwd = home.join("cwd_project");
        fs::create_dir_all(&cwd).unwrap();
        let r1 = home.join("r1");
        let r2 = home.join("r2");
        let r3 = home.join("r3");
        for r in [&r1, &r2, &r3] {
            fs::create_dir_all(r).unwrap();
        }
        let recent = vec![r1.clone(), r2.clone(), r3.clone()];
        let detected = vec![cwd.clone()];
        let (projects, local_count) = split_local_and_previous(&cwd, &recent, &detected);
        assert_eq!(projects, vec![cwd, r1, r2, r3]);
        assert_eq!(local_count, 1);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn no_cwd_project_yields_local_count_zero_with_the_most_recent_first() {
        let home = unique_temp_home();
        let cwd = home.join("empty_cwd");
        fs::create_dir_all(&cwd).unwrap();
        let r1 = home.join("r1");
        let r2 = home.join("r2");
        fs::create_dir_all(&r1).unwrap();
        fs::create_dir_all(&r2).unwrap();
        let recent = vec![r1.clone(), r2.clone()];
        let (projects, local_count) = split_local_and_previous(&cwd, &recent, &[]);
        assert_eq!(local_count, 0);
        assert_eq!(projects, vec![r1.clone(), r2]);
        assert_eq!(
            projects.first(),
            Some(&r1),
            "the most-recently-opened project would become active"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_recent_under_the_cwd_that_the_walk_missed_still_counts_as_local() {
        let home = unique_temp_home();
        let cwd = home.join("cwd");
        // Nested well past the bounded walk's depth — `detected` never finds
        // it, but it's still a real, existing directory under `cwd`.
        let deep = cwd.join("a").join("b").join("c").join("d");
        fs::create_dir_all(&deep).unwrap();
        let outside = home.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let recent = vec![deep.clone(), outside.clone()];
        let (projects, local_count) = split_local_and_previous(&cwd, &recent, &[]);
        assert_eq!(local_count, 1, "the under-cwd recent is local");
        assert_eq!(projects, vec![deep, outside]);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn split_dedupes_a_recent_already_present_as_detected() {
        let home = unique_temp_home();
        let cwd = home.join("cwd");
        fs::create_dir_all(&cwd).unwrap();
        let recent = vec![cwd.clone(), PathBuf::from("/does/not/exist")];
        let detected = vec![cwd.clone()];
        let (projects, local_count) = split_local_and_previous(&cwd, &recent, &detected);
        assert_eq!(
            projects,
            vec![cwd],
            "the recent duplicate of a detected root is dropped, and so is \
             the dead recent entry"
        );
        assert_eq!(local_count, 1);
        let _ = fs::remove_dir_all(&home);
    }

    /// Windows' case-insensitive filesystem: a recent entry naming the same
    /// directory as `cwd` but differently cased must still classify as
    /// local — [`split_local_and_previous`]'s `path_is_within` check goes
    /// through [`host_path::is_under`], which folds case on Windows.
    #[cfg(windows)]
    #[test]
    fn a_recent_matching_the_cwd_only_by_case_still_counts_as_local() {
        let home = unique_temp_home();
        let cwd = home.join("Work");
        fs::create_dir_all(&cwd).unwrap();
        let differently_cased =
            PathBuf::from(cwd.to_string_lossy().replace("Work", "wORK").to_string());
        let (projects, local_count) =
            split_local_and_previous(&cwd, std::slice::from_ref(&differently_cased), &[]);
        assert_eq!(
            local_count, 1,
            "case-insensitively under the cwd is still local"
        );
        assert_eq!(projects, vec![differently_cased]);
        let _ = fs::remove_dir_all(&home);
    }

    // ── Windows project-identity: dedupe on save, heal on load ───────────

    /// A `\\?\`-verbatim path and its plain equivalent name the same
    /// project — the wizard/cwd-detection duplication this card fixes — and
    /// must collapse to one recorded entry.
    #[cfg(windows)]
    #[test]
    fn a_verbatim_and_plain_windows_path_dedupe_on_save() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        record_recent_project_with(&env, Path::new(r"\\?\C:\dev\wintui-scratch"));
        record_recent_project_with(&env, Path::new(r"C:\dev\wintui-scratch"));
        let recent = load_recent_projects_with(&env);
        assert_eq!(
            recent,
            vec![PathBuf::from(r"C:\dev\wintui-scratch")],
            "the verbatim and plain forms are the same project"
        );
        let _ = fs::remove_dir_all(&home);
    }

    /// Windows paths are case-insensitive: `C:\Dev\x` and `c:\dev\x` name the
    /// same project and must not produce two recents rows.
    #[cfg(windows)]
    #[test]
    fn differently_cased_windows_paths_dedupe_on_save() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        record_recent_project_with(&env, Path::new(r"C:\Dev\x"));
        record_recent_project_with(&env, Path::new(r"c:\dev\x"));
        assert_eq!(load_recent_projects_with(&env).len(), 1);
        let _ = fs::remove_dir_all(&home);
    }

    /// The Unix counterpart: case is significant on a case-sensitive
    /// filesystem, so `/a/X` and `/a/x` stay two distinct recents.
    #[cfg(not(windows))]
    #[test]
    fn differently_cased_unix_paths_stay_distinct_on_save() {
        let home = unique_temp_home();
        let env = FakeEnv::home(&home);
        record_recent_project_with(&env, Path::new("/a/X"));
        record_recent_project_with(&env, Path::new("/a/x"));
        assert_eq!(load_recent_projects_with(&env).len(), 2);
        let _ = fs::remove_dir_all(&home);
    }

    /// A `tui.toml` already carrying a duplicate pair from before project
    /// identity went through `host_path` (the wizard's canonicalized
    /// verbatim path alongside cwd-detection's plain one) heals to one entry
    /// on the very next load, with no user action.
    #[cfg(windows)]
    #[test]
    fn loading_an_already_duplicated_windows_file_heals_to_one_entry() {
        let home = unique_temp_home();
        let config_dir = home.join(".config").join("frust");
        fs::create_dir_all(&config_dir).unwrap();
        let array: Array = vec![
            r"C:\dev\wintui-scratch".to_string(),
            r"\\?\C:\dev\wintui-scratch".to_string(),
        ]
        .into_iter()
        .collect();
        let mut recent_table = Table::new();
        recent_table.insert("projects", Item::Value(Value::Array(array)));
        let mut doc = DocumentMut::new();
        doc.insert("recent", Item::Table(recent_table));
        fs::write(config_dir.join("tui.toml"), doc.to_string()).unwrap();

        let env = FakeEnv::home(&home);
        let recent = load_recent_projects_with(&env);
        assert_eq!(
            recent,
            vec![PathBuf::from(r"C:\dev\wintui-scratch")],
            "first occurrence wins, healed on load"
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
        save_dap_setting_with(&env, DapSetting::IntroSeen(true));
        assert_eq!(
            load_dap_prefs_with(&env),
            DapPrefs {
                enabled: true,
                auto_start_in_ide: false,
                auto_configure_ide: false,
                port: 5005,
                ide_override: Some(ParentIde::Zed),
                intro_seen: true,
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
                intro_seen: DapPrefs::default().intro_seen,
            }
        );
        let _ = fs::remove_dir_all(&home);
    }

    /// A `tui.toml` predating the first-run notice has no `intro_seen` key at
    /// all: it must load as "not yet seen", so an existing install gets the
    /// notice on its next in-IDE auto-start rather than skipping it.
    #[test]
    fn a_dap_table_without_intro_seen_loads_as_not_yet_seen() {
        let home = unique_temp_home();
        let config = home.join(".config").join("frust");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("tui.toml"), "[dap]\nport = 5005\n").unwrap();
        let env = FakeEnv::home(&home);
        let prefs = load_dap_prefs_with(&env);
        assert_eq!(prefs.port, 5005);
        assert!(!prefs.intro_seen);

        save_dap_setting_with(&env, DapSetting::IntroSeen(true));
        assert!(load_dap_prefs_with(&env).intro_seen);
        assert_eq!(
            load_dap_prefs_with(&env).port,
            5005,
            "burning the notice must not disturb the other keys"
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
