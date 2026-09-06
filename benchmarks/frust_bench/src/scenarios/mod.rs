//! The scenario driver contract shared by every benchmark scenario.
//!
//! Every scenario is a zero-sized unit struct implementing [`Scenario`],
//! registered once in [`SCENARIOS`]. A scenario is selected at launch via a
//! deep link (`frustbench://<id>`, the on-device path the harness uses — see
//! `docs/SHELLS_DEVELOPMENT.md`'s Deep-link manual test) or the `FRUST_BENCH_SCENARIO`
//! env var (the desktop fallback), and can be switched at runtime from the HUD
//! button row or a warm deep link (see [`crate::BenchApp`]).
//!
//! Two id namespaces share this one registry (`benchmarks/PROTOCOL.md` §9.1):
//! `s1..=s8`, the frame-class scenarios (a per-frame render series), and
//! `d1..=d2`, the DB op-latency class (`d1_db_write`/`d2_db_read` —
//! per-op latency only, no frame series of its own). Both namespaces are
//! selected, switched, and marker-bracketed identically — [`Scenario::id`]
//! is an opaque string to every mechanism below (deep link, env var, HUD
//! button row), so nothing here special-cases the `d`-prefix.
//!
//! The `d*` namespace rides the crate's default-ON `db` cargo feature: it
//! carries the `frust-database` dependency and its bundled SQLite, which
//! only those two scenarios call. A `--no-default-features` build compiles
//! the S1–S8 frame-class app alone (the app-size matrix arm — see the
//! feature's own comment in `Cargo.toml`). Nothing below reads the registry
//! length as a constant, so both shapes drive identically; [`SCENARIOS`] is
//! simply eight entries instead of ten.
//!
//! If a scenario id is not found in the registry (e.g., a `d1` deep link on a
//! `--no-default-features` build, or `FRUST_BENCH_SCENARIO=d1` where the id is
//! not compiled in), the selection falls back to S1 (index 0). A diagnostic is
//! logged once when the unknown id is encountered: after logger installation on
//! the platform (post-init deep links via [`BenchState::consume_deep_link`], or
//! on the first [`crate::BenchApp::build`] call if the id came from the env var).
//! The diagnostic echoes a sanitized version of the id (control characters and
//! length-capped) to avoid log injection.
//!
//! # Marker contract
//!
//! [`Scenario::on_start`]/[`on_end`](Scenario::on_end) bracket a scenario's
//! active window by stamping `bench-scenario-start/end <id>` markers into the
//! same `FRUST_TRACE_RAW` per-frame stream the shell's `FrameStats` emits
//! (`frust_shell_common::perf::mark_scenario_start`/`mark_scenario_end` — see
//! `docs/ARCHITECTURE.md`'s `FrameStats`/`StartupSpans` row), so the harness
//! can slice the raw series per scenario. The default impls stamp
//! the scenario's [`id`](Scenario::id); a scenario that needs finer per-op
//! sub-markers (S3's `s3-create1k` etc.) calls the same `perf::mark_scenario_*`
//! functions directly with its own sub-names.
//!
//! # State model
//!
//! [`BenchState`] is the single app state every scenario builds over. The two
//! animating scenarios (S1, and later S8's burst variant) read `running`/
//! `epoch`/`fps`; scenarios with per-scenario simulation state keep it in
//! their own retained widget (S1's `BubbleChartWidget` owns the physics), so a
//! new scenario needs no `BenchState` field of its own.

use frust::{AnyView, Get, RwSignal, Set, any, deep_links, text};

#[cfg(feature = "db")]
pub mod d1_db_write;
#[cfg(feature = "db")]
pub mod d2_db_read;
pub mod s1_animation;
pub mod s2_list;
pub mod s3_table;
pub mod s4_heavy;
pub mod s5_image;
pub mod s6_text;
pub mod s7_startup;
pub mod s8_prefs;

/// The deep-link scheme registered in the generated Android manifest / iOS
/// Info.plist (`frust create --deeplink-scheme frustbench`); a link's host is
/// the scenario [`id`](Scenario::id), e.g. `frustbench://s1`.
pub const DEEP_LINK_SCHEME: &str = "frustbench";

/// Env var naming the scenario to launch on desktop (the deep-link fallback
/// `frust run`/`cargo run` uses), e.g. `FRUST_BENCH_SCENARIO=s7`.
pub const SCENARIO_ENV: &str = "FRUST_BENCH_SCENARIO";

/// One benchmark scenario: a stable id (deep-link host + marker name), a
/// human-readable title, and a view builder over the shared [`BenchState`].
///
/// `Sync` is required so a scenario can be held in the process-wide
/// [`SCENARIOS`] registry as a `&'static dyn Scenario`.
pub trait Scenario: Sync {
    /// Stable id: the deep-link host (`frustbench://<id>`) and the default
    /// scenario-marker name. One of `s1`..=`s8` (the frame-class table) or,
    /// on a `db`-feature build, `d1`..=`d2` (the DB op-latency class —
    /// `benchmarks/PROTOCOL.md` §9 DB scenarios).
    fn id(&self) -> &'static str;

    /// Human-readable title, shown in the HUD and each stub's placeholder.
    fn title(&self) -> &'static str;

    /// Build this scenario's view over the shared bench state.
    fn build(&self, state: &mut BenchState) -> AnyView<BenchState>;

    /// Called by the driver when this scenario becomes active — stamps the
    /// opening scenario marker. Override only to add setup work.
    fn on_start(&self) {
        frust_shell_common::perf::mark_scenario_start(self.id());
    }

    /// Called by the driver when this scenario is switched away from — stamps
    /// the closing scenario marker. Override only to add teardown work.
    fn on_end(&self) {
        frust_shell_common::perf::mark_scenario_end(self.id());
    }
}

/// All ten scenarios (the eight `s*` frame-class scenarios plus the two
/// `d*` DB op-latency scenarios), in id order. Index into this from
/// [`BenchState::active`] / [`BenchState::selected`].
///
/// The `db`-off arm below drops the two `d*` entries; the `s1`..=`s8`
/// prefix keeps its order and indices in both, so a deep link, an env var,
/// or a recorded harness index for a frame-class scenario means the same
/// thing either way. Keep the two literals in sync when adding an `s*`
/// scenario — the `s*` prefix is deliberately duplicated rather than
/// spliced, so that the registry stays one flat `static` per configuration.
#[cfg(feature = "db")]
pub static SCENARIOS: [&dyn Scenario; 10] = [
    &s1_animation::S1,
    &s2_list::S2,
    &s3_table::S3,
    &s4_heavy::S4,
    &s5_image::S5,
    &s6_text::S6,
    &s7_startup::S7,
    &s8_prefs::S8,
    &d1_db_write::D1,
    &d2_db_read::D2,
];

/// The eight `s*` frame-class scenarios, in id order — the registry a
/// `--no-default-features` (no `db`) build sees. See the `db`-on arm above.
#[cfg(not(feature = "db"))]
pub static SCENARIOS: [&dyn Scenario; 8] = [
    &s1_animation::S1,
    &s2_list::S2,
    &s3_table::S3,
    &s4_heavy::S4,
    &s5_image::S5,
    &s6_text::S6,
    &s7_startup::S7,
    &s8_prefs::S8,
];

/// The single application state every scenario builds over.
pub struct BenchState {
    /// Index into [`SCENARIOS`] of the scenario currently mounted. Reconciled
    /// against [`selected`](Self::selected) in [`crate::BenchApp::build`],
    /// which fires the [`Scenario::on_end`]/[`on_start`](Scenario::on_start)
    /// markers on a change.
    pub active: usize,
    /// The scenario the app *wants* mounted, written by the HUD buttons and
    /// warm deep links. A signal so a write from an event handler wakes the
    /// tracked rebuild that reconciles it.
    pub selected: RwSignal<usize>,
    /// The most recently consumed deep-link URL, for warm-link de-duplication
    /// (mirrors the facade router glue's last-consumed dedupe).
    pub last_link: Option<String>,
    /// Whether an animating scenario is stepping (S1's Pause/Play).
    pub running: bool,
    /// Bumped to reseed an animating scenario (S1's Reset).
    pub epoch: u64,
    /// Measured painted-frames-per-second, written ~1×/second by an animating
    /// scenario's paint pass and read by the HUD readout.
    pub fps: RwSignal<f64>,
    /// If the `FRUST_BENCH_SCENARIO` env var named an unknown scenario id at
    /// launch, the sanitized id is deferred here to be logged once from
    /// [`crate::BenchApp::build`] after logger installation.
    pub pending_unknown_env_id: Option<String>,
}

impl BenchState {
    /// Construct the initial state with `active`/`selected` resolved from the
    /// launch deep link or [`SCENARIO_ENV`] (defaulting to S1).
    pub fn new() -> Self {
        let (initial, pending_unknown_env_id) = resolve_initial();
        Self {
            active: initial,
            selected: RwSignal::new(initial),
            last_link: None,
            running: true,
            epoch: 0,
            fps: RwSignal::new(0.0),
            pending_unknown_env_id,
        }
    }

    /// Consume a warm deep link (if any new one has arrived), mapping it onto
    /// [`selected`](Self::selected). Tracks the `latest` signal so a new link
    /// wakes the rebuild. Called from [`crate::BenchApp::build`].
    pub fn consume_deep_link(&mut self) {
        let links = deep_links();
        // `.get()` tracks `latest`, so a warm link wakes this rebuild.
        if let Some(link) = links.latest.get()
            && self.last_link.as_deref() != Some(link.url.as_str())
        {
            self.last_link = Some(link.url.clone());
            if let Some(i) = index_from_url(&link.url) {
                self.selected.set(i);
            } else if let Some(id) = scenario_id_from_url(&link.url) {
                // The URL has a valid frustbench:// scheme, but the id is not
                // in the registry. Log a diagnostic (with sanitized id to prevent
                // log injection) and leave selected unchanged.
                let sanitized = sanitize_id(id);
                log::warn!(
                    "frust_bench: unknown scenario id '{}' (not compiled into this build?) \
                     — selection unchanged",
                    sanitized
                );
            }
        }
    }
}

impl Default for BenchState {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the scenario to launch: the `FRUST_BENCH_SCENARIO` env var wins
/// (desktop only), then S1 (index 0). Returns the resolved index and an
/// optional sanitized id that was unknown in the registry (to be logged
/// from [`crate::BenchApp::build`] after logger installation).
///
/// Note: deep-link resolution is not attempted here. On Android/iOS, the
/// cold-start deep link is queued by the OS and delivered post-init through
/// [`BenchState::consume_deep_link`]. On desktop, no deep-link producer exists.
fn resolve_initial() -> (usize, Option<String>) {
    if let Ok(id) = std::env::var(SCENARIO_ENV) {
        let id_trimmed = id.trim();
        if let Some(i) = index_from_id(id_trimmed) {
            return (i, None);
        }
        // Unknown id: defer the diagnostic to be logged from build() after
        // logger installation, and fall back to S1.
        let sanitized = sanitize_id(id_trimmed);
        return (0, Some(sanitized));
    }
    (0, None)
}

/// Extract the scenario id host from a `frustbench://<id>[/...]` URL.
/// Returns `Some(id)` if the URL has the correct scheme, or `None` otherwise.
/// The returned id may or may not be in the registry.
pub fn scenario_id_from_url(url: &str) -> Option<&str> {
    let prefix = format!("{DEEP_LINK_SCHEME}://");
    let rest = url.strip_prefix(&prefix)?;
    // The scenario id is the host, up to the first `/`, `?`, or `#`.
    let id = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .trim_end_matches('/');
    if id.is_empty() { None } else { Some(id) }
}

/// Sanitize a scenario id for safe logging, preventing log injection.
/// Keeps only alphanumeric chars, underscore, and hyphen; caps at 32 chars
/// and marks truncation with `…` if any filtering or truncation occurred.
pub fn sanitize_id(id: &str) -> String {
    let original_char_count = id.chars().count();
    // Count how many safe characters are in the original.
    let safe_char_count = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .count();

    // Extract up to 32 safe characters.
    let result: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(32)
        .collect();

    // Mark with … if we filtered out any chars or if we had to truncate at 32.
    if safe_char_count < original_char_count || safe_char_count > 32 {
        format!("{}…", result)
    } else {
        result
    }
}

/// Map a `frustbench://<id>[/...]` URL to a [`SCENARIOS`] index, if its host is
/// a known scenario id.
pub fn index_from_url(url: &str) -> Option<usize> {
    scenario_id_from_url(url).and_then(index_from_id)
}

/// Map a bare scenario id (`s1`..=`s8`) to its [`SCENARIOS`] index.
pub fn index_from_id(id: &str) -> Option<usize> {
    SCENARIOS.iter().position(|s| s.id() == id)
}

/// A shared labeled placeholder view for a not-yet-implemented scenario
/// stub — a centered title so a desktop run visibly switches to it.
pub fn placeholder(scenario: &dyn Scenario) -> AnyView<BenchState> {
    any(text(format!(
        "{} — {}\n(scenario stub — not yet implemented)",
        scenario.id().to_uppercase(),
        scenario.title(),
    ))
    .size(20.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_id_from_url_extracts_host() {
        // Valid URLs with the correct scheme extract the host.
        assert_eq!(scenario_id_from_url("frustbench://s1"), Some("s1"));
        assert_eq!(scenario_id_from_url("frustbench://d1"), Some("d1"));
        assert_eq!(
            scenario_id_from_url("frustbench://s1/extra/path"),
            Some("s1")
        );
        assert_eq!(scenario_id_from_url("frustbench://s3?x=1"), Some("s3"));
        assert_eq!(scenario_id_from_url("frustbench://s7#fragment"), Some("s7"));
        // Wrong scheme or malformed URLs return None.
        assert_eq!(scenario_id_from_url("other://s1"), None);
        assert_eq!(scenario_id_from_url("frustbench://"), None);
    }

    #[test]
    fn sanitize_id_removes_unsafe_chars() {
        // Normal ids pass through unchanged.
        assert_eq!(sanitize_id("s1"), "s1");
        assert_eq!(sanitize_id("my-scenario_1"), "my-scenario_1");
        // Control characters, spaces, and special chars are filtered out.
        // Since unsafe chars were removed, mark with …
        assert_eq!(sanitize_id("s1 with spaces"), "s1withspaces…");
        assert_eq!(sanitize_id("s1\nwith\nnewlines"), "s1withnewlines…");
        assert_eq!(sanitize_id("s1=value"), "s1value…");
        // Truncation at 32 safe chars is marked with …
        let long_id = "a".repeat(35);
        let sanitized = sanitize_id(&long_id);
        assert_eq!(sanitized, format!("{}…", "a".repeat(32)));
        // Mixed: filter AND hit the 32-char cap.
        let mixed = format!("{}hello world", "a".repeat(30));
        // First 32 safe chars: 30 a's + h + e + l = 33 chars, so take(32) gives us 30 a's + h + e
        let sanitized = sanitize_id(&mixed);
        assert_eq!(sanitized, format!("{}he…", "a".repeat(30)));
    }
}
