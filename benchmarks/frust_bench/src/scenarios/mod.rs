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
//! If a scenario id known to the protocol (e.g., a `d1` deep link or
//! `FRUST_BENCH_SCENARIO=d1`) is absent from the compiled registry, the driver
//! logs `frust_bench: unknown scenario id '<id>' (not compiled into this build?)
//! — falling back to s1` and falls back to S1 (index 0). The invariant is: a
//! benchmark run never records numbers under a scenario label the running binary
//! did not actually execute.
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
}

impl BenchState {
    /// Construct the initial state with `active`/`selected` resolved from the
    /// launch deep link or [`SCENARIO_ENV`] (defaulting to S1).
    pub fn new() -> Self {
        let initial = resolve_initial();
        Self {
            active: initial,
            selected: RwSignal::new(initial),
            last_link: None,
            running: true,
            epoch: 0,
            fps: RwSignal::new(0.0),
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
            }
        }
    }
}

impl Default for BenchState {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the scenario to launch: the cold-start deep link wins, then
/// [`SCENARIO_ENV`], then S1 (index 0). If a deep link or env var names
/// an unknown scenario id, logs a diagnostic and falls back to S1.
fn resolve_initial() -> usize {
    if let Some(url) = deep_links().initial {
        if let Some(i) = index_from_url(&url) {
            return i;
        }
        // Extract the id from the URL for logging.
        if let Some(rest) = url.strip_prefix(&format!("{DEEP_LINK_SCHEME}://"))
            && let Some(id) = rest.split(['/', '?', '#']).next()
        {
            log::warn!(
                "frust_bench: unknown scenario id '{}' (not compiled into this build?) \
                 — falling back to s1",
                id
            );
        }
    }
    if let Ok(id) = std::env::var(SCENARIO_ENV) {
        let id_trimmed = id.trim();
        if let Some(i) = index_from_id(id_trimmed) {
            return i;
        }
        log::warn!(
            "frust_bench: unknown scenario id '{}' (not compiled into this build?) \
             — falling back to s1",
            id_trimmed
        );
    }
    0
}

/// Map a `frustbench://<id>[/...]` URL to a [`SCENARIOS`] index, if its host is
/// a known scenario id.
pub fn index_from_url(url: &str) -> Option<usize> {
    let prefix = format!("{DEEP_LINK_SCHEME}://");
    let rest = url.strip_prefix(&prefix)?;
    // The scenario id is the host, up to the first `/`, `?`, or `#`.
    let id = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .trim_end_matches('/');
    index_from_id(id)
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
    fn unknown_scenario_id_falls_back_to_s1() {
        // On any build, an unknown scenario id falls back to S1 (index 0).
        // resolve_initial() logs a diagnostic when this happens.
        let s1_idx = index_from_id("s1").expect("s1 should exist");
        let unknown_idx = index_from_id("unknown_xyz");
        assert_eq!(unknown_idx, None);
        assert_eq!(s1_idx, 0, "s1 should be at index 0");
    }
}
