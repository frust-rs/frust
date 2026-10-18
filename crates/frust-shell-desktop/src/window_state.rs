//! Window-state persistence: the window's size, position and maximized state,
//! saved to the user's data directory and restored on the next launch when the
//! app opts in through `DesktopConfig::persist_window_state`.
//!
//! Everything here is winit-free decision logic plus best-effort file I/O, so
//! the policy is unit-testable without a live event loop: the versioned
//! `key=value` file format, the debounced saver (driven by the caller's clock,
//! no thread of its own), the monitor-validity clamp over plain monitor
//! rectangles, and the initial-geometry precedence. `app_handler` translates
//! winit values in and out. Window state is user state, not a cache, so it
//! lives under `frust_paths::data_dir()`; a failed read is logged at debug
//! level, the first failed write per process at warn (later ones at debug),
//! and neither fails startup.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use kurbo::Size;

use crate::paced_wake::ControlFlowIntent;

/// The on-disk format version. A file carrying any other version is ignored.
const FORMAT_VERSION: u32 = 1;

/// How long the window must stay unchanged before a coalesced save runs.
pub(crate) const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// A persisted window geometry.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WindowState {
    /// The inner size, in logical pixels.
    pub size: Size,
    /// The outer position, in physical pixels, or `None` where the platform
    /// reports none (Wayland).
    pub position: Option<(i32, i32)>,
    /// Whether the window was maximized. `size`/`position` then hold the last
    /// un-maximized geometry, so un-maximizing after a restore lands there.
    pub maximized: bool,
    /// The name of the monitor the window was on, when the platform names it.
    pub monitor: Option<String>,
}

impl WindowState {
    /// Serialize to the versioned `key=value` text format. A monitor name
    /// containing a control character is omitted rather than allowed to break
    /// the line structure.
    pub fn encode(&self) -> String {
        let mut out = format!(
            "version={FORMAT_VERSION}\nwidth={}\nheight={}\nmaximized={}\n",
            self.size.width, self.size.height, self.maximized
        );
        if let Some((x, y)) = self.position {
            out.push_str(&format!("x={x}\ny={y}\n"));
        }
        if let Some(name) = self
            .monitor
            .as_deref()
            .filter(|n| !n.chars().any(char::is_control))
        {
            out.push_str(&format!("monitor={name}\n"));
        }
        out
    }

    /// Parse [`encode`](Self::encode)'s output. `None` for a missing or
    /// unknown version, a malformed line, an invalid value, or a missing or
    /// non-positive size; unknown keys are skipped. Never panics.
    pub fn decode(text: &str) -> Option<Self> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let version = lines.next()?.strip_prefix("version=")?;
        if version.parse::<u32>().ok()? != FORMAT_VERSION {
            return None;
        }
        let (mut width, mut height, mut x, mut y) = (None, None, None, None);
        let (mut maximized, mut monitor) = (false, None);
        for line in lines {
            let (key, value) = line.split_once('=')?;
            match key {
                "width" => width = Some(value.parse::<f64>().ok()?),
                "height" => height = Some(value.parse::<f64>().ok()?),
                "x" => x = Some(value.parse::<i32>().ok()?),
                "y" => y = Some(value.parse::<i32>().ok()?),
                "maximized" => maximized = value.parse::<bool>().ok()?,
                "monitor" => monitor = Some(value.to_string()).filter(|n| !n.is_empty()),
                _ => {}
            }
        }
        let size = Size::new(width?, height?);
        if !valid_size(size) {
            return None;
        }
        let position = match (x, y) {
            (Some(x), Some(y)) => Some((x, y)),
            (None, None) => None,
            _ => return None,
        };
        Some(Self {
            size,
            position,
            maximized,
            monitor,
        })
    }
}

/// A usable window size: both axes finite and strictly positive.
pub(crate) fn valid_size(size: Size) -> bool {
    size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0
}

/// `<data_dir>/frust/window_state_<app_stem>.txt`, or `None` when no data
/// directory resolves (persistence is then silently off).
fn state_path() -> Option<PathBuf> {
    Some(crate::cache::app_file_path(
        &frust_paths::data_dir()?,
        "window_state",
        "txt",
    ))
}

/// Load the persisted window state, or `None` when there is none or it does
/// not parse.
pub(crate) fn load() -> Option<WindowState> {
    load_from(&state_path()?)
}

fn load_from(path: &Path) -> Option<WindowState> {
    let text = fs::read_to_string(path)
        .map_err(|e| {
            log::debug!(
                "frust-shell-desktop: no window state at {}: {e}",
                path.display()
            )
        })
        .ok()?;
    let state = WindowState::decode(&text);
    if state.is_none() {
        log::debug!(
            "frust-shell-desktop: ignoring unreadable window state at {}",
            path.display()
        );
    }
    state
}

/// Whether this run may write the state file: the app opted in and no
/// `FRUST_WINDOW_SIZE`/`FRUST_WINDOW_MAXIMIZED` knob is set. A knob run is a
/// measurement run; it still reads and applies the saved state, but never
/// overwrites it.
pub(crate) fn persistence_enabled(opted_in: bool, knob_set: bool) -> bool {
    opted_in && !knob_set
}

/// Set once the first save failure of the process has been logged at warn.
static SAVE_FAILURE_WARNED: AtomicBool = AtomicBool::new(false);

/// Write `state` atomically (best-effort). The first failed write per process
/// logs at warn (path and error); later failures log at debug.
pub(crate) fn save(state: &WindowState) {
    if let Some(path) = state_path() {
        save_to(&path, state, &SAVE_FAILURE_WARNED);
    }
}

/// `true` exactly once per flag: the failure that should warn.
fn first_failure(warned: &AtomicBool) -> bool {
    !warned.swap(true, Ordering::Relaxed)
}

fn save_to(path: &Path, state: &WindowState, warned: &AtomicBool) {
    match frust_paths::atomic_write(path, state.encode().as_bytes()) {
        Ok(()) => log::debug!(
            "frust-shell-desktop: saved window state to {}",
            path.display()
        ),
        Err(e) => {
            let level = if first_failure(warned) {
                log::Level::Warn
            } else {
                log::Level::Debug
            };
            log::log!(
                level,
                "frust-shell-desktop: failed to save window state to {}: {e}",
                path.display()
            );
        }
    }
}

/// The live window's geometry as the shell reads it at save time.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WindowSnapshot {
    /// Inner size, logical pixels.
    pub size: Size,
    /// Outer position, physical pixels.
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
    pub minimized: bool,
    /// Whether the window is fullscreen. Fullscreen is never persisted.
    pub fullscreen: bool,
    pub monitor: Option<String>,
}

/// The debounced saver: window events mark the geometry dirty, and a save
/// runs once it has been quiet for [`SAVE_DEBOUNCE`] (or at once on close).
/// The caller supplies `now`, polls from its own event loop, and parks the
/// loop until [`deadline`](Self::deadline) so a quiet window still saves.
/// The un-maximized geometry is tracked from creation (and on every
/// un-maximized change via [`note_normal`](Self::note_normal)), so a window
/// maximized before its first save still records its normal size.
#[derive(Debug, Default)]
pub(crate) struct WindowStateSaver {
    due: Option<Instant>,
    written: Option<WindowState>,
    normal: Option<(Size, Option<(i32, i32)>)>,
}

impl WindowStateSaver {
    /// Seed with what was loaded (the file's own contents, so an identical
    /// snapshot skips the write) and the geometry the window is created with,
    /// which is the normal geometry until a change says otherwise.
    pub fn seed(&mut self, loaded: Option<WindowState>, creation: &InitialGeometry) {
        self.written = loaded;
        self.normal = Some((creation.size, creation.position));
    }

    /// Record the geometry of a window that is neither maximized, minimized
    /// nor fullscreen. An invalid size is ignored.
    pub fn note_normal(&mut self, size: Size, position: Option<(i32, i32)>) {
        if valid_size(size) {
            self.normal = Some((size, position));
        }
    }

    /// A resize or move happened at `now`: (re)start the quiet period.
    pub fn note_change(&mut self, now: Instant) {
        self.due = Some(now + SAVE_DEBOUNCE);
    }

    /// When the pending save falls due, if one is pending.
    pub fn deadline(&self) -> Option<Instant> {
        self.due
    }

    /// The state to write now, when the quiet period has elapsed and the
    /// geometry differs from what was last written.
    pub fn poll(
        &mut self,
        now: Instant,
        snapshot: impl FnOnce() -> Option<WindowSnapshot>,
    ) -> Option<WindowState> {
        if self.due.is_none_or(|due| now < due) {
            return None;
        }
        self.flush(snapshot)
    }

    /// The final save on close/exit: pending or not, the state to write when
    /// it differs from what was last written.
    pub fn flush(
        &mut self,
        snapshot: impl FnOnce() -> Option<WindowSnapshot>,
    ) -> Option<WindowState> {
        self.due = None;
        let state = self.resolve(snapshot()?)?;
        if self.written.as_ref() == Some(&state) {
            return None;
        }
        self.written = Some(state.clone());
        Some(state)
    }

    /// A minimized or fullscreen window records nothing (the previous saved
    /// state stands; platforms report sentinel geometry when minimized); a
    /// maximized one keeps the last un-maximized size/position.
    fn resolve(&mut self, snap: WindowSnapshot) -> Option<WindowState> {
        if snap.minimized || snap.fullscreen || !valid_size(snap.size) {
            return None;
        }
        let (size, position) = if snap.maximized {
            self.normal.unwrap_or((snap.size, snap.position))
        } else {
            self.normal = Some((snap.size, snap.position));
            (snap.size, snap.position)
        };
        Some(WindowState {
            size,
            position,
            maximized: snap.maximized,
            monitor: snap.monitor,
        })
    }
}

/// Fold the saver's deadline into the loop's control flow: a pending save
/// must wake a parked loop, at the earlier of the two deadlines.
pub(crate) fn merge_wake(
    intent: ControlFlowIntent,
    save_due: Option<Instant>,
) -> ControlFlowIntent {
    match (intent, save_due) {
        (intent, None) => intent,
        (ControlFlowIntent::WaitUntil(paced), Some(due)) => {
            ControlFlowIntent::WaitUntil(paced.min(due))
        }
        (ControlFlowIntent::Wait | ControlFlowIntent::Unchanged, Some(due)) => {
            ControlFlowIntent::WaitUntil(due)
        }
    }
}

/// One connected monitor, physical pixels.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MonitorRect {
    pub name: Option<String>,
    pub position: (i32, i32),
    pub size: (u32, u32),
    pub scale: f64,
}

impl MonitorRect {
    fn logical_size(&self) -> Size {
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        Size::new(
            f64::from(self.size.0) / scale,
            f64::from(self.size.1) / scale,
        )
    }

    fn intersects(&self, (x, y): (i32, i32), (w, h): (f64, f64)) -> bool {
        let (mx, my) = (f64::from(self.position.0), f64::from(self.position.1));
        let (mw, mh) = (f64::from(self.size.0), f64::from(self.size.1));
        let (x, y) = (f64::from(x), f64::from(y));
        x < mx + mw && mx < x + w && y < my + mh && my < y + h
    }
}

/// Fit a restored state to the monitors connected now. A saved monitor that
/// is gone, or a rectangle that intersects no monitor, drops the position
/// (the OS places the window) and fits the size to the primary monitor;
/// otherwise the size fits the saved monitor. `max`/`min` clamp last, in
/// that order, so a minimum larger than the monitor still wins.
pub(crate) fn clamp_restored(
    mut state: WindowState,
    monitors: &[MonitorRect],
    primary: Option<&MonitorRect>,
    min: Option<Size>,
    max: Option<Size>,
) -> WindowState {
    let named = state.monitor.as_deref();
    let saved = named.and_then(|n| monitors.iter().find(|m| m.name.as_deref() == Some(n)));
    let primary = primary.or(monitors.first());
    if let Some(position) = state.position {
        let scale = saved.or(primary).map_or(1.0, |m| m.scale);
        let extent = (state.size.width * scale, state.size.height * scale);
        let monitor_gone = named.is_some() && saved.is_none();
        if monitor_gone || !monitors.iter().any(|m| m.intersects(position, extent)) {
            state.position = None;
        }
    }
    let fit = if state.position.is_some() {
        saved.or(primary)
    } else {
        primary
    };
    if let Some(bounds) = fit.map(MonitorRect::logical_size) {
        state.size = Size::new(
            state.size.width.min(bounds.width),
            state.size.height.min(bounds.height),
        );
    }
    if let Some(max) = max.filter(|s| valid_size(*s)) {
        state.size = Size::new(
            state.size.width.min(max.width),
            state.size.height.min(max.height),
        );
    }
    if let Some(min) = min.filter(|s| valid_size(*s)) {
        state.size = Size::new(
            state.size.width.max(min.width),
            state.size.height.max(min.height),
        );
    }
    state
}

/// The window's creation geometry, after precedence.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InitialGeometry {
    /// Inner size, logical pixels.
    pub size: Size,
    /// Outer position, physical pixels; `None` lets the OS place the window.
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
}

/// Resolve the creation geometry (fullscreen is never part of it), lowest
/// precedence first: the shell
/// `default` → `DesktopConfig::initial_size` → the persisted state (the
/// caller passes it only when persistence is on and it loaded and clamped)
/// → the `FRUST_WINDOW_SIZE`/`FRUST_WINDOW_MAXIMIZED` measurement knobs,
/// each knob overriding only its own field. An invalid config size is
/// ignored.
pub(crate) fn initial_geometry(
    default: Size,
    config_size: Option<Size>,
    persisted: Option<&WindowState>,
    env_size: Option<Size>,
    env_maximized: Option<bool>,
) -> InitialGeometry {
    let mut geometry = InitialGeometry {
        size: config_size.filter(|s| valid_size(*s)).unwrap_or(default),
        position: None,
        maximized: false,
    };
    if let Some(state) = persisted {
        geometry.size = state.size;
        geometry.position = state.position;
        geometry.maximized = state.maximized;
    }
    if let Some(size) = env_size {
        geometry.size = size;
    }
    if let Some(maximized) = env_maximized {
        geometry.maximized = maximized;
    }
    geometry
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(w: f64, h: f64, position: Option<(i32, i32)>, monitor: Option<&str>) -> WindowState {
        WindowState {
            size: Size::new(w, h),
            position,
            maximized: false,
            monitor: monitor.map(str::to_string),
        }
    }

    fn monitor(name: &str, position: (i32, i32), size: (u32, u32), scale: f64) -> MonitorRect {
        MonitorRect {
            name: Some(name.to_string()),
            position,
            size,
            scale,
        }
    }

    fn snapshot(w: f64, h: f64, position: (i32, i32)) -> Option<WindowSnapshot> {
        Some(WindowSnapshot {
            size: Size::new(w, h),
            position: Some(position),
            maximized: false,
            minimized: false,
            fullscreen: false,
            monitor: Some("DP-1".to_string()),
        })
    }

    // --- format ---

    #[test]
    fn the_format_round_trips_every_field() {
        let mut full = state(1280.5, 720.0, Some((-40, 25)), Some("DP-1 = left"));
        full.maximized = true;
        assert_eq!(WindowState::decode(&full.encode()), Some(full));
        let bare = state(800.0, 600.0, None, None);
        assert_eq!(WindowState::decode(&bare.encode()), Some(bare));
    }

    #[test]
    fn a_monitor_name_with_a_control_character_is_dropped_not_written() {
        let named = state(800.0, 600.0, None, Some("bad\nname"));
        let decoded = WindowState::decode(&named.encode()).expect("still decodes");
        assert_eq!(decoded.monitor, None);
        assert_eq!(decoded.size, named.size);
    }

    #[test]
    fn corrupt_or_unknown_version_input_decodes_to_none() {
        for input in [
            "",
            "garbage",
            "width=800\nheight=600\n",
            "version=2\nwidth=800\nheight=600\n",
            "version=x\nwidth=800\nheight=600\n",
            "version=1\nwidth=800\n",
            "version=1\nwidth=0\nheight=600\n",
            "version=1\nwidth=-5\nheight=600\n",
            "version=1\nwidth=NaN\nheight=600\n",
            "version=1\nwidth=inf\nheight=600\n",
            "version=1\nwidth=800\nheight=600\nx=10\n",
            "version=1\nwidth=800\nheight=600\nx=1e3\ny=0\n",
            "version=1\nwidth=800\nheight=600\nmaximized=yes\n",
            "version=1\nwidth=800\nheight=600\nno separator\n",
        ] {
            assert_eq!(WindowState::decode(input), None, "{input:?}");
        }
    }

    #[test]
    fn unknown_keys_are_skipped() {
        let decoded = WindowState::decode("version=1\nwidth=800\nheight=600\nfuture=1\n");
        assert_eq!(decoded, Some(state(800.0, 600.0, None, None)));
    }

    #[test]
    fn the_state_file_lives_in_the_data_dir_under_the_shared_shape() {
        let path = state_path().expect("data dir resolvable on test hosts");
        let base = frust_paths::data_dir().expect("data dir resolvable on test hosts");
        assert_eq!(
            path,
            crate::cache::app_file_path(&base, "window_state", "txt")
        );
    }

    #[test]
    fn save_then_load_round_trips_through_a_scratch_file() {
        let root =
            std::env::temp_dir().join(format!("frust-window-state-test-{}", std::process::id()));
        fs::create_dir(&root).expect("scratch root must not already exist (planted path?)");
        let path = root.join("frust").join("window_state.txt");
        assert_eq!(load_from(&path), None, "absent file");
        let saved = state(1024.0, 768.0, Some((10, 20)), Some("eDP-1"));
        save_to(&path, &saved, &AtomicBool::new(false));
        assert_eq!(load_from(&path), Some(saved));
        fs::write(&path, "version=9\n").unwrap();
        assert_eq!(load_from(&path), None, "unknown version");
        let _ = fs::remove_dir_all(&root);
    }

    fn creation(w: f64, h: f64, position: Option<(i32, i32)>) -> InitialGeometry {
        InitialGeometry {
            size: Size::new(w, h),
            position,
            maximized: false,
        }
    }

    fn maximized_snapshot() -> Option<WindowSnapshot> {
        Some(WindowSnapshot {
            size: Size::new(2560.0, 1400.0),
            position: Some((0, 0)),
            maximized: true,
            ..snapshot(1.0, 1.0, (0, 0))?
        })
    }

    // --- debounce ---

    #[test]
    fn a_burst_of_changes_coalesces_into_one_save_after_the_quiet_period() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut saver = WindowStateSaver::default();
        saver.note_change(ms(0));
        saver.note_change(ms(100));
        saver.note_change(ms(300));
        assert_eq!(saver.deadline(), Some(ms(800)));
        assert_eq!(saver.poll(ms(500), || snapshot(900.0, 700.0, (1, 2))), None);
        assert_eq!(saver.poll(ms(799), || snapshot(900.0, 700.0, (1, 2))), None);
        let saved = saver.poll(ms(800), || snapshot(900.0, 700.0, (1, 2)));
        assert_eq!(saved.map(|s| s.size), Some(Size::new(900.0, 700.0)));
        assert_eq!(saver.deadline(), None, "the save consumed the deadline");
        assert_eq!(saver.poll(ms(2000), || panic!("nothing is pending")), None);
    }

    #[test]
    fn the_final_save_writes_pending_changes_and_skips_an_unchanged_state() {
        let t0 = Instant::now();
        let mut saver = WindowStateSaver::default();
        saver.note_change(t0);
        let first = saver.flush(|| snapshot(900.0, 700.0, (1, 2)));
        assert!(first.is_some(), "a close mid-quiet-period still saves");
        assert_eq!(saver.deadline(), None);
        assert_eq!(
            saver.flush(|| snapshot(900.0, 700.0, (1, 2))),
            None,
            "unchanged"
        );
        assert!(saver.flush(|| snapshot(901.0, 700.0, (1, 2))).is_some());
    }

    #[test]
    fn a_seeded_saver_skips_rewriting_the_loaded_state() {
        let loaded = state(900.0, 700.0, Some((1, 2)), Some("DP-1"));
        let mut saver = WindowStateSaver::default();
        saver.seed(Some(loaded.clone()), &creation(900.0, 700.0, Some((1, 2))));
        assert_eq!(saver.flush(|| snapshot(900.0, 700.0, (1, 2))), None);
    }

    #[test]
    fn a_maximized_window_keeps_the_last_normal_geometry_and_a_minimized_one_saves_nothing() {
        let mut saver = WindowStateSaver::default();
        saver.flush(|| snapshot(900.0, 700.0, (1, 2)));
        let maximized = saver.flush(|| {
            Some(WindowSnapshot {
                size: Size::new(2560.0, 1400.0),
                position: Some((0, 0)),
                maximized: true,
                minimized: false,
                fullscreen: false,
                monitor: Some("DP-1".to_string()),
            })
        });
        let expected = WindowState {
            maximized: true,
            ..state(900.0, 700.0, Some((1, 2)), Some("DP-1"))
        };
        assert_eq!(maximized, Some(expected));
        let minimized = saver.flush(|| {
            Some(WindowSnapshot {
                minimized: true,
                position: Some((-32000, -32000)),
                ..snapshot(160.0, 28.0, (0, 0)).unwrap()
            })
        });
        assert_eq!(minimized, None);
    }

    #[test]
    fn maximizing_before_the_first_save_records_the_creation_geometry() {
        let mut saver = WindowStateSaver::default();
        saver.seed(None, &creation(800.0, 600.0, Some((5, 6))));
        let saved = saver.flush(maximized_snapshot).expect("saves");
        assert_eq!(saved.size, Size::new(800.0, 600.0));
        assert_eq!(saved.position, Some((5, 6)));
        assert!(saved.maximized);
    }

    #[test]
    fn an_unmaximized_resize_is_tracked_before_a_maximize() {
        let mut saver = WindowStateSaver::default();
        saver.seed(None, &creation(800.0, 600.0, None));
        saver.note_normal(Size::new(900.0, 700.0), Some((3, 4)));
        saver.note_normal(Size::new(f64::NAN, 1.0), None);
        let saved = saver.flush(maximized_snapshot).expect("saves");
        assert_eq!(saved.size, Size::new(900.0, 700.0));
        assert_eq!(saved.position, Some((3, 4)));
        assert!(saved.maximized);
    }

    #[test]
    fn a_fullscreen_snapshot_saves_nothing() {
        let mut saver = WindowStateSaver::default();
        saver.seed(None, &creation(800.0, 600.0, None));
        saver.note_change(Instant::now());
        let fullscreen = || {
            Some(WindowSnapshot {
                fullscreen: true,
                ..snapshot(1920.0, 1080.0, (0, 0))?
            })
        };
        assert_eq!(saver.flush(fullscreen), None);
        saver.note_change(Instant::now());
        let later = Instant::now() + SAVE_DEBOUNCE * 2;
        assert_eq!(saver.poll(later, fullscreen), None);
    }

    #[test]
    fn knob_runs_do_not_persist() {
        assert!(persistence_enabled(true, false));
        assert!(!persistence_enabled(true, true));
        assert!(!persistence_enabled(false, false));
        assert!(!persistence_enabled(false, true));
    }

    #[test]
    fn only_the_first_failed_save_warns() {
        let root =
            std::env::temp_dir().join(format!("frust-window-state-warn-{}", std::process::id()));
        fs::create_dir(&root).expect("scratch root must not already exist (planted path?)");
        let blocker = root.join("blocker");
        fs::write(&blocker, "file").unwrap();
        let path = blocker.join("window_state.txt");
        let flag = AtomicBool::new(false);
        assert!(first_failure(&flag));
        assert!(!first_failure(&flag));
        let warned = AtomicBool::new(false);
        let saved = state(800.0, 600.0, None, None);
        save_to(&path, &saved, &warned);
        assert!(warned.load(Ordering::Relaxed), "first failure warns");
        assert!(!path.exists());
        save_to(&path, &saved, &warned);
        assert!(
            warned.load(Ordering::Relaxed),
            "stays set; later ones are debug"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pending_save_folds_into_the_loop_wake() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(1);
        assert_eq!(
            merge_wake(ControlFlowIntent::Wait, None),
            ControlFlowIntent::Wait
        );
        assert_eq!(
            merge_wake(ControlFlowIntent::Wait, Some(t0)),
            ControlFlowIntent::WaitUntil(t0)
        );
        assert_eq!(
            merge_wake(ControlFlowIntent::WaitUntil(later), Some(t0)),
            ControlFlowIntent::WaitUntil(t0)
        );
        assert_eq!(
            merge_wake(ControlFlowIntent::WaitUntil(t0), Some(later)),
            ControlFlowIntent::WaitUntil(t0)
        );
    }

    // --- monitor-validity clamp ---

    fn two_monitors() -> Vec<MonitorRect> {
        vec![
            monitor("DP-1", (0, 0), (1920, 1080), 1.0),
            monitor("DP-2", (1920, 0), (2560, 1440), 2.0),
        ]
    }

    #[test]
    fn a_state_on_a_connected_monitor_is_kept() {
        let monitors = two_monitors();
        let saved = state(1000.0, 600.0, Some((2000, 100)), Some("DP-2"));
        let clamped = clamp_restored(saved.clone(), &monitors, monitors.first(), None, None);
        assert_eq!(clamped, saved);
    }

    #[test]
    fn a_missing_monitor_drops_the_position_and_fits_the_primary() {
        let monitors = two_monitors();
        let saved = state(2400.0, 1300.0, Some((2000, 100)), Some("HDMI-9"));
        let clamped = clamp_restored(saved, &monitors, monitors.first(), None, None);
        assert_eq!(clamped.position, None);
        assert_eq!(clamped.size, Size::new(1920.0, 1080.0));
    }

    #[test]
    fn an_off_screen_rect_drops_the_position() {
        let monitors = two_monitors();
        let saved = state(800.0, 600.0, Some((9000, 9000)), None);
        let clamped = clamp_restored(saved, &monitors, monitors.first(), None, None);
        assert_eq!(clamped.position, None);
        assert_eq!(clamped.size, Size::new(800.0, 600.0));
    }

    #[test]
    fn an_oversize_state_fits_its_monitor_in_logical_pixels() {
        let monitors = two_monitors();
        let saved = state(3000.0, 2000.0, Some((1920, 0)), Some("DP-2"));
        let clamped = clamp_restored(saved, &monitors, monitors.first(), None, None);
        assert_eq!(clamped.position, Some((1920, 0)));
        assert_eq!(clamped.size, Size::new(1280.0, 720.0), "2560x1440 at 2x");
    }

    #[test]
    fn min_and_max_clamp_the_restored_size_with_min_winning() {
        let monitors = two_monitors();
        let min = Some(Size::new(500.0, 400.0));
        let max = Some(Size::new(1200.0, 900.0));
        let small = clamp_restored(state(300.0, 200.0, None, None), &monitors, None, min, max);
        assert_eq!(small.size, Size::new(500.0, 400.0));
        let large = clamp_restored(state(1600.0, 1000.0, None, None), &monitors, None, min, max);
        assert_eq!(large.size, Size::new(1200.0, 900.0));
        let huge_min = Some(Size::new(2500.0, 400.0));
        let forced = clamp_restored(
            state(800.0, 600.0, None, None),
            &monitors,
            None,
            huge_min,
            None,
        );
        assert_eq!(forced.size, Size::new(2500.0, 600.0));
    }

    #[test]
    fn no_monitors_drops_the_position_and_leaves_the_size() {
        let saved = state(800.0, 600.0, Some((10, 10)), Some("DP-1"));
        let clamped = clamp_restored(saved, &[], None, None, None);
        assert_eq!(clamped.position, None);
        assert_eq!(clamped.size, Size::new(800.0, 600.0));
    }

    // --- creation-geometry precedence ---

    #[test]
    fn creation_geometry_follows_default_config_persisted_env_precedence() {
        let default = Size::new(800.0, 600.0);
        let config = Some(Size::new(1024.0, 768.0));
        let mut persisted = state(1100.0, 650.0, Some((5, 6)), None);
        persisted.maximized = true;
        let env = Some(Size::new(2560.0, 1440.0));
        let geometry = |size, maximized, position| InitialGeometry {
            size,
            position,
            maximized,
        };
        let rows = [
            (None, None, None, None, geometry(default, false, None)),
            (
                config,
                None,
                None,
                None,
                geometry(Size::new(1024.0, 768.0), false, None),
            ),
            (
                config,
                Some(&persisted),
                None,
                None,
                geometry(persisted.size, true, Some((5, 6))),
            ),
            (
                config,
                Some(&persisted),
                env,
                None,
                geometry(Size::new(2560.0, 1440.0), true, Some((5, 6))),
            ),
            (
                config,
                Some(&persisted),
                None,
                Some(false),
                geometry(persisted.size, false, Some((5, 6))),
            ),
            (
                None,
                None,
                env,
                Some(true),
                geometry(Size::new(2560.0, 1440.0), true, None),
            ),
            (
                Some(Size::new(f64::NAN, 1.0)),
                None,
                None,
                None,
                geometry(default, false, None),
            ),
        ];
        for (index, (config, persisted, env_size, env_max, expected)) in
            rows.into_iter().enumerate()
        {
            assert_eq!(
                initial_geometry(default, config, persisted, env_size, env_max),
                expected,
                "row {index}"
            );
        }
    }
}
