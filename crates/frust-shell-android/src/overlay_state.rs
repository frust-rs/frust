//! Overlay surfaces: the switch, the per-overlay table, the frame ledger.
//!
//! An overlay is a translucent `SurfaceView` Kotlin stacks above a hosted
//! native view; the render thread draws the frame's commands painted after
//! that view onto it (`frust_render::overlay`). The JNI half (the windows,
//! the overlay surfaces, the render-loop wiring) lives in `crate::jni_glue`;
//! the state machines it drives are here, generic over the window and surface
//! types, so their ordering contracts are host-tested. This carries no
//! `jni`/`ndk`/GPU dependency.
//!
//! Ordering contracts:
//!
//! - **A surface is dropped before the window it was built on.**
//!   [`OverlayTable`] keeps this on every path: a withdrawn window drops its
//!   surface first, a replaced window drops the old surface before the old
//!   window, and `drop_surfaces` (the main surface going away) drops every
//!   surface and keeps the windows.
//! - **The ledger pairs cuts to the frame id the render thread draws.** The
//!   differ announces an overlay's range only when it changes and the render
//!   thread may draw a frame older than the newest paint, so every painted
//!   frame publishes a snapshot under its own frame id
//!   ([`OverlayLedger::publish_frame`]) and the render thread looks its
//!   scene's id up ([`OverlayLedger::frame_cuts`]).
//! - **The ledger keeps [`OVERLAY_FRAMES_KEPT`] (16) frames**, above the
//!   platform-view release gate's in-flight bound, so a frame the render
//!   thread can still draw always finds its own cuts.
//! - **The ledger resets on a new native handle**: a differ whose generation
//!   is older than the folded one restarts the ledger, since a new differ
//!   restarts its generations and overlay ids.
//! - **`OverlayTable::prepare` reports refusals; it does not log.** It builds
//!   and resizes surfaces on the render thread and returns the overlays whose
//!   surface was refused (once per window); the caller logs them. Per-frame
//!   engine errors are latched by `note_error`, which answers a count only
//!   while it is within the caller's log limit.

/// The overlay switch: `FRUST_OVERLAYS`, read at runtime or baked at compile
/// time (`frust build --define`). `FRUST_OVERLAY_SPIKE` is still honoured as
/// an alias.
pub(crate) const OVERLAYS_ENV_VAR: &str = "FRUST_OVERLAYS";

/// How (and whether) Kotlin builds overlay surfaces — the switch's value, and
/// the code `nativeOverlayMode` hands the embedding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OverlayMode {
    /// No overlays: the differ is fed without the frame's commands, the JSON
    /// carries no overlay op and the frame path is the single-surface one.
    Off,
    /// Overlay `SurfaceView`s are `setZOrderOnTop(true)` — above the window,
    /// so above every hosted `View`. The default when switched on.
    OnTop,
    /// Overlay `SurfaceView`s are `setZOrderMediaOverlay(true)`
    /// (`FRUST_OVERLAYS=media`) — above the main surface but behind the
    /// window, kept as an A/B arm.
    MediaOverlay,
}

impl OverlayMode {
    /// The mode one switch value selects: blank, `0`, `off` or `false` is
    /// [`Self::Off`], `media` is [`Self::MediaOverlay`], anything else
    /// [`Self::OnTop`] (case-insensitive, surrounding whitespace ignored).
    pub(crate) fn from_value(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty()
            || value == "0"
            || value.eq_ignore_ascii_case("off")
            || value.eq_ignore_ascii_case("false")
        {
            Self::Off
        } else if value.eq_ignore_ascii_case("media") {
            Self::MediaOverlay
        } else {
            Self::OnTop
        }
    }

    /// Resolve the switch from its sources in priority order — the first
    /// one carrying a non-blank value decides (so a runtime `0` can switch
    /// off a baked-in value); none set is [`Self::Off`].
    pub(crate) fn resolve<'a>(sources: impl IntoIterator<Item = Option<&'a str>>) -> Self {
        sources
            .into_iter()
            .flatten()
            .find(|value| !value.trim().is_empty())
            .map_or(Self::Off, Self::from_value)
    }

    /// Whether overlays are switched on at all.
    pub(crate) fn is_on(self) -> bool {
        self != Self::Off
    }

    /// The `nativeOverlayMode` code: `0` off, `1` on top, `2` media overlay
    /// (`FrustViewHost`'s constants mirror these).
    pub(crate) fn code(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::OnTop => 1,
            Self::MediaOverlay => 2,
        }
    }
}

/// One overlay's native half: the window its `SurfaceView` delivered, and
/// the surface the render thread built on it.
#[derive(Debug)]
struct OverlayEntry<W, S> {
    /// Declared before `window` on purpose: fields drop in declaration
    /// order, so even an implicit drop releases the surface before the
    /// window it was built on.
    surface: Option<S>,
    window: Option<W>,
    /// The window's size, physical px.
    size: (u32, u32),
    /// Building a surface on this window was refused (logged once); retried
    /// only for a new window.
    refused: bool,
    /// Frames this overlay's engine refused, for the latched log.
    errors: u32,
}

/// What [`OverlayTable::window_changed`] did with a delivered window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowChange {
    /// A window the overlay did not hold: any surface on the previous one was
    /// dropped first, and the render thread builds a fresh one.
    Adopted,
    /// The window it already holds, at a (possibly) new size; the extra
    /// reference was dropped and the render thread resizes.
    Resized,
}

/// The live overlays by the differ's overlay id: each one's window and the
/// surface built on it.
///
/// Shared by the UI thread, which owns the windows (the overlay
/// `SurfaceView` callbacks deliver and withdraw them), and the render thread,
/// which builds, resizes and draws the surfaces. The one invariant every
/// method keeps: **a surface is dropped before the window it was built on**
/// — a withdrawn window drops its surface first, a replaced window drops the
/// old surface before the old window goes, and [`Self::drop_surfaces`] (the
/// main surface going away) drops every surface and keeps the windows.
#[derive(Debug)]
pub(crate) struct OverlayTable<W, S> {
    entries: std::collections::BTreeMap<u64, OverlayEntry<W, S>>,
}

impl<W, S> Default for OverlayTable<W, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W, S> OverlayTable<W, S> {
    /// An empty table (`const`, for a process-global).
    pub(crate) const fn new() -> Self {
        Self {
            entries: std::collections::BTreeMap::new(),
        }
    }

    /// Overlay `id`'s `SurfaceView` delivered `window` at `size`
    /// (`surfaceCreated`/`surfaceChanged`). `same` answers whether two
    /// windows are the same platform object.
    pub(crate) fn window_changed(
        &mut self,
        id: u64,
        window: W,
        size: (u32, u32),
        same: impl FnOnce(&W, &W) -> bool,
    ) -> WindowChange {
        let entry = self.entries.entry(id).or_insert(OverlayEntry {
            surface: None,
            window: None,
            size,
            refused: false,
            errors: 0,
        });
        entry.size = size;
        if entry
            .window
            .as_ref()
            .is_some_and(|held| same(held, &window))
        {
            drop(window);
            return WindowChange::Resized;
        }
        // Surface first, then the window it was built on.
        entry.surface = None;
        entry.window = Some(window);
        entry.refused = false;
        entry.errors = 0;
        WindowChange::Adopted
    }

    /// Overlay `id`'s `SurfaceView` withdrew its window (`surfaceDestroyed`,
    /// or the overlay was disposed): drop its surface, then release the
    /// window, and forget the overlay. `false` when it held none.
    pub(crate) fn window_destroyed(&mut self, id: u64) -> bool {
        match self.entries.remove(&id) {
            Some(mut entry) => {
                entry.surface = None;
                entry.window = None;
                true
            }
            None => false,
        }
    }

    /// The main surface went away: drop every overlay's surface (they share
    /// its device and must not outlive its teardown) and keep the windows,
    /// so the next [`Self::prepare`] after the main surface returns rebuilds
    /// them. Answers how many surfaces were dropped.
    pub(crate) fn drop_surfaces(&mut self) -> usize {
        let mut dropped = 0;
        for entry in self.entries.values_mut() {
            if entry.surface.take().is_some() {
                dropped += 1;
            }
            entry.refused = false;
        }
        dropped
    }

    /// Bring every surface in line with its window, on the render thread:
    /// `resize` a live one to its window's size, `build` one where a window
    /// has none (and was not refused). Answers each refusal this call made —
    /// the caller logs those; the overlay is then skipped until a new window
    /// arrives.
    pub(crate) fn prepare<E>(
        &mut self,
        mut build: impl FnMut(&W, (u32, u32)) -> Result<S, E>,
        mut resize: impl FnMut(&mut S, (u32, u32)),
    ) -> Vec<(u64, E)> {
        let mut refusals = Vec::new();
        for (&id, entry) in &mut self.entries {
            let Some(window) = entry.window.as_ref() else {
                continue;
            };
            if let Some(surface) = entry.surface.as_mut() {
                resize(surface, entry.size);
            } else if !entry.refused {
                match build(window, entry.size) {
                    Ok(surface) => entry.surface = Some(surface),
                    Err(error) => {
                        entry.refused = true;
                        refusals.push((id, error));
                    }
                }
            }
        }
        refusals
    }

    /// The ids of the overlays with a live surface, ascending.
    pub(crate) fn live_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.surface.is_some())
            .map(|(&id, _)| id)
    }

    /// Whether any overlay has a live surface.
    pub(crate) fn has_live(&self) -> bool {
        self.live_ids().next().is_some()
    }

    /// The live surfaces for `cuts`, in `cuts`' order, each with its range; a
    /// cut whose overlay has no live surface is left out.
    pub(crate) fn cut_surfaces(
        &mut self,
        cuts: &[(u64, std::ops::Range<usize>)],
    ) -> Vec<(u64, std::ops::Range<usize>, &mut S)> {
        let mut live: std::collections::BTreeMap<u64, &mut S> = self
            .entries
            .iter_mut()
            .filter_map(|(&id, entry)| entry.surface.as_mut().map(|surface| (id, surface)))
            .collect();
        cuts.iter()
            .filter_map(|(id, range)| live.remove(id).map(|surface| (*id, range.clone(), surface)))
            .collect()
    }

    /// Count one engine refusal on overlay `id`; answers the running count
    /// while it is at most `log_limit` (the caller logs then), `None` past it
    /// or for an unknown overlay.
    pub(crate) fn note_error(&mut self, id: u64, log_limit: u32) -> Option<u32> {
        let entry = self.entries.get_mut(&id)?;
        entry.errors = entry.errors.saturating_add(1);
        (entry.errors <= log_limit).then_some(entry.errors)
    }

    /// How many overlays the table tracks (with or without a surface).
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// How many frames' cut lists [`OverlayLedger`] keeps for the render thread
/// to pair against — above the platform-view release gate's in-flight bound
/// (`MAX_FRAMES_IN_FLIGHT`, 12), so a frame the render thread can still draw
/// always finds its own cuts.
pub(crate) const OVERLAY_FRAMES_KEPT: usize = 16;

/// One overlay's cut for one frame: its id and the half-open command range of
/// that frame's scene it draws.
pub(crate) type OverlayCutSpec = (u64, std::ops::Range<usize>);

/// The overlays' command ranges, folded from the differ's `Overlay` /
/// `DisposeOverlay` commands on the UI thread, and published per frame for
/// the render thread.
///
/// The differ announces a range only when it changes, and the render thread
/// may draw a frame older than the UI thread's newest paint (the scene channel
/// is latest-wins, the render side runs behind), so the current ranges alone
/// would pair one frame's ranges with another frame's scene. Every painted
/// frame therefore publishes a snapshot under its own frame id
/// ([`Self::publish_frame`]) and the render thread looks its scene's id up
/// ([`Self::frame_cuts`]).
#[derive(Debug)]
pub(crate) struct OverlayLedger {
    ranges: std::collections::BTreeMap<u64, std::ops::Range<usize>>,
    folded_generation: u64,
    frames: std::collections::VecDeque<(u64, Vec<OverlayCutSpec>)>,
}

impl Default for OverlayLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayLedger {
    /// An empty ledger (`const`, for a process-global).
    pub(crate) const fn new() -> Self {
        Self {
            ranges: std::collections::BTreeMap::new(),
            folded_generation: 0,
            frames: std::collections::VecDeque::new(),
        }
    }

    /// Forget everything — a new differ (a new native handle) starts its
    /// generations and overlay ids over.
    pub(crate) fn reset(&mut self) {
        self.ranges.clear();
        self.frames.clear();
        self.folded_generation = 0;
    }

    /// Start folding the differ's backlog at its current `generation`:
    /// `Some(g)` — fold every command pushed after generation `g` (the
    /// backlog's `commands_up_to(g)` prefix is already folded), then call
    /// [`Self::finish_fold`] — or `None` when nothing new was pushed. A
    /// generation older than the one already folded means a new differ, and
    /// restarts the ledger.
    pub(crate) fn begin_fold(&mut self, generation: u64) -> Option<u64> {
        if generation < self.folded_generation {
            self.reset();
        }
        (generation != self.folded_generation).then_some(self.folded_generation)
    }

    /// Fold one `Overlay` command: overlay `id` now draws `range`.
    pub(crate) fn set(&mut self, id: u64, range: std::ops::Range<usize>) {
        self.ranges.insert(id, range);
    }

    /// Fold one `DisposeOverlay` command.
    pub(crate) fn dispose(&mut self, id: u64) {
        self.ranges.remove(&id);
    }

    /// Close a fold [`Self::begin_fold`] opened at `generation`.
    pub(crate) fn finish_fold(&mut self, generation: u64) {
        self.folded_generation = generation;
    }

    /// The ranges as they stand, ordered by range (then id) — what the
    /// frame painted just now draws.
    pub(crate) fn snapshot(&self) -> Vec<OverlayCutSpec> {
        let mut cuts: Vec<OverlayCutSpec> = self
            .ranges
            .iter()
            .map(|(&id, range)| (id, range.clone()))
            .collect();
        cuts.sort_by_key(|(id, range)| (range.start, range.end, *id));
        cuts
    }

    /// Publish the current ranges as frame `frame_id`'s cuts, keeping the
    /// newest [`OVERLAY_FRAMES_KEPT`] frames. Republishing an id replaces
    /// its cuts.
    pub(crate) fn publish_frame(&mut self, frame_id: u64) {
        let cuts = self.snapshot();
        self.frames.retain(|(id, _)| *id != frame_id);
        self.frames.push_back((frame_id, cuts));
        while self.frames.len() > OVERLAY_FRAMES_KEPT {
            self.frames.pop_front();
        }
    }

    /// Frame `frame_id`'s published cuts, `None` when it published none (or
    /// fell out of the kept window).
    pub(crate) fn frame_cuts(&self, frame_id: u64) -> Option<&[OverlayCutSpec]> {
        self.frames
            .iter()
            .rev()
            .find(|(id, _)| *id == frame_id)
            .map(|(_, cuts)| cuts.as_slice())
    }
}

/// The cuts one segmented frame draws: one per overlay with a live surface
/// (`live`), carrying the range its frame published (`frame`, clamped to the
/// scene's `len` commands) — or an empty range at the scene's end when the
/// frame published none for it (not announced yet, disposed, or the frame's
/// cuts are gone), which draws a transparent clear and nothing else. Ordered
/// by range, so the list is the ascending, disjoint cut list
/// `SurfaceRenderer::encode_segmented` takes. An overlay the frame names but
/// that has no live surface gets no cut: its commands stay on the main
/// surface (under the native view in an opaque surface — the graceful
/// degradation).
pub(crate) fn assemble_overlay_cuts(
    frame: Option<&[OverlayCutSpec]>,
    live: impl IntoIterator<Item = u64>,
    len: usize,
) -> Vec<OverlayCutSpec> {
    let mut cuts: Vec<OverlayCutSpec> = live
        .into_iter()
        .map(|id| {
            let range = frame
                .and_then(|cuts| cuts.iter().find(|(cut, _)| *cut == id))
                .map(|(_, range)| range.start.min(len)..range.end.min(len))
                .filter(|range| range.start < range.end)
                .unwrap_or(len..len);
            (id, range)
        })
        .collect();
    cuts.sort_by_key(|(id, range)| (range.start, range.end, *id));
    cuts
}

/// Rendered overlay frames between two `frust-overlay` skew lines.
pub(crate) const OVERLAY_SKEW_EVERY: u32 = 120;

/// One `frust-overlay` skew line's worth of numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OverlaySkewReport {
    /// Frames folded into this report.
    pub(crate) frames: u32,
    /// Mean main-present → last-overlay-present delta, µs.
    pub(crate) avg_us: u64,
    /// Largest delta in the window, µs.
    pub(crate) max_us: u64,
    /// Overlays presented in the window's last frame.
    pub(crate) overlays: usize,
    /// The window's last main present, µs since the Unix epoch.
    pub(crate) main_unix_us: u128,
    /// The window's last overlay present, µs since the Unix epoch.
    pub(crate) overlay_unix_us: u128,
}

/// Main-versus-overlay present skew, accumulated per frame and reported every
/// [`OVERLAY_SKEW_EVERY`] frames — the wall-clock (`SystemTime`) gap between
/// the main surface's present and the last overlay's, read off the log on a
/// device.
#[derive(Debug, Default)]
pub(crate) struct OverlaySkew {
    frames: u32,
    sum_us: u64,
    max_us: u64,
}

impl OverlaySkew {
    /// An empty window (`const`, for a process-global).
    pub(crate) const fn new() -> Self {
        Self {
            frames: 0,
            sum_us: 0,
            max_us: 0,
        }
    }

    /// Fold one frame whose main surface presented at `main` and whose last
    /// overlay (of `overlays`) presented at `last_overlay`; answers a report
    /// every [`OVERLAY_SKEW_EVERY`] frames, then starts a new window. A clock
    /// that stepped backwards in between counts as zero skew.
    pub(crate) fn record(
        &mut self,
        main: std::time::SystemTime,
        last_overlay: std::time::SystemTime,
        overlays: usize,
    ) -> Option<OverlaySkewReport> {
        let delta = last_overlay
            .duration_since(main)
            .unwrap_or_default()
            .as_micros();
        let delta = u64::try_from(delta).unwrap_or(u64::MAX);
        self.frames += 1;
        self.sum_us = self.sum_us.saturating_add(delta);
        self.max_us = self.max_us.max(delta);
        if self.frames < OVERLAY_SKEW_EVERY {
            return None;
        }
        let unix_us = |at: std::time::SystemTime| {
            at.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros()
        };
        let report = OverlaySkewReport {
            frames: self.frames,
            avg_us: self.sum_us / u64::from(self.frames),
            max_us: self.max_us,
            overlays,
            main_unix_us: unix_us(main),
            overlay_unix_us: unix_us(last_overlay),
        };
        *self = Self::new();
        Some(report)
    }
}

/// The overlay host's request for one frame: raised on the UI thread when an
/// overlay window arrives, changes size, is replaced or is withdrawn; consumed
/// by the frame gate on the same thread. The overlay surface is built by the
/// render thread ahead of its next frame, so without a frame an idle app keeps
/// the content on the main surface under the native view (or a hole where the
/// withdrawn overlay's content was cut).
#[derive(Debug, Default)]
pub(crate) struct FrameRequest(std::sync::atomic::AtomicBool);

impl FrameRequest {
    /// A latch with no request pending (`const`, for a process-global).
    pub(crate) const fn new() -> Self {
        Self(std::sync::atomic::AtomicBool::new(false))
    }

    /// Ask for one frame.
    pub(crate) fn request(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }

    /// Whether a frame was requested since the last take; clears the request.
    pub(crate) fn take(&self) -> bool {
        self.0.swap(false, std::sync::atomic::Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_request_latches_once() {
        let latch = FrameRequest::new();
        assert!(!latch.take());
        latch.request();
        assert!(latch.take());
        assert!(!latch.take());
    }

    #[test]
    fn frame_requests_collapse_into_one_take() {
        let latch = FrameRequest::new();
        latch.request();
        latch.request();
        assert!(latch.take());
        assert!(!latch.take());
    }

    #[test]
    fn overlay_mode_reads_the_switch_value() {
        for off in ["", "  ", "0", "off", "OFF", "false"] {
            assert_eq!(OverlayMode::from_value(off), OverlayMode::Off, "{off:?}");
        }
        for on in ["1", "on", "yes", "top"] {
            assert_eq!(OverlayMode::from_value(on), OverlayMode::OnTop, "{on:?}");
        }
        assert_eq!(
            OverlayMode::from_value(" Media "),
            OverlayMode::MediaOverlay
        );
        assert_eq!(OverlayMode::Off.code(), 0);
        assert_eq!(OverlayMode::OnTop.code(), 1);
        assert_eq!(OverlayMode::MediaOverlay.code(), 2);
        assert!(!OverlayMode::Off.is_on());
        assert!(OverlayMode::MediaOverlay.is_on());
    }

    #[test]
    fn overlay_mode_first_set_source_decides() {
        assert_eq!(OverlayMode::resolve([None, None, None]), OverlayMode::Off);
        // The runtime value outranks a baked-in one, including a runtime `0`.
        assert_eq!(
            OverlayMode::resolve([Some("0"), Some("1"), None]),
            OverlayMode::Off
        );
        assert_eq!(
            OverlayMode::resolve([None, Some("media"), Some("1")]),
            OverlayMode::MediaOverlay
        );
        // A blank source is not set; the spike alias answers last.
        assert_eq!(
            OverlayMode::resolve([Some(" "), None, Some("1")]),
            OverlayMode::OnTop
        );
    }

    type Log = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    /// A window or surface stand-in that records its own drop.
    #[derive(Debug)]
    struct Tracked {
        name: String,
        log: Log,
        size: (u32, u32),
    }

    impl Tracked {
        fn new(log: &Log, name: &str) -> Self {
            Self {
                name: name.to_string(),
                log: log.clone(),
                size: (0, 0),
            }
        }
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.log.borrow_mut().push(format!("drop {}", self.name));
        }
    }

    fn same(a: &Tracked, b: &Tracked) -> bool {
        a.name == b.name
    }

    /// Build a surface named after its window.
    fn build_on(log: &Log) -> impl FnMut(&Tracked, (u32, u32)) -> Result<Tracked, String> + '_ {
        move |window, size| {
            let mut surface = Tracked::new(log, &format!("surface@{}", window.name));
            surface.size = size;
            Ok(surface)
        }
    }

    fn resize(surface: &mut Tracked, size: (u32, u32)) {
        surface.size = size;
    }

    fn drops(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.borrow_mut())
    }

    #[test]
    fn overlay_table_attach_builds_and_resizes_on_the_render_thread() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        assert_eq!(
            table.window_changed(1, Tracked::new(&log, "w1"), (100, 200), same),
            WindowChange::Adopted
        );
        // No surface until the render thread prepares.
        assert!(!table.has_live());
        assert!(table.prepare(build_on(&log), resize).is_empty());
        assert_eq!(table.live_ids().collect::<Vec<_>>(), vec![1]);

        // The same window again is a resize: the extra reference goes, the
        // surface stays and picks the size up at the next prepare.
        assert_eq!(
            table.window_changed(1, Tracked::new(&log, "w1"), (300, 400), same),
            WindowChange::Resized
        );
        assert_eq!(drops(&log), vec!["drop w1"]);
        table.prepare(build_on(&log), resize);
        let cut = table.cut_surfaces(&[(1, 0..4)]);
        assert_eq!(cut.len(), 1);
        assert_eq!(cut[0].2.size, (300, 400));
        assert_eq!(cut[0].2.name, "surface@w1");
    }

    #[test]
    fn overlay_table_drops_the_surface_before_its_window() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        table.window_changed(1, Tracked::new(&log, "w1"), (1, 1), same);
        table.prepare(build_on(&log), resize);

        // A new window replaces the old one: old surface, then old window.
        assert_eq!(
            table.window_changed(1, Tracked::new(&log, "w2"), (1, 1), same),
            WindowChange::Adopted
        );
        assert_eq!(drops(&log), vec!["drop surface@w1", "drop w1"]);
        assert!(
            !table.has_live(),
            "the new window waits for a fresh surface"
        );
        table.prepare(build_on(&log), resize);

        // Withdrawn: surface, then window, and the overlay is forgotten.
        assert!(table.window_destroyed(1));
        assert_eq!(drops(&log), vec!["drop surface@w2", "drop w2"]);
        assert_eq!(table.len(), 0);
        assert!(!table.window_destroyed(1), "a second withdraw is a no-op");

        // Dropping the whole table keeps the order too.
        table.window_changed(2, Tracked::new(&log, "w3"), (1, 1), same);
        table.prepare(build_on(&log), resize);
        drop(table);
        assert_eq!(drops(&log), vec!["drop surface@w3", "drop w3"]);
    }

    #[test]
    fn overlay_table_follows_the_main_surface_through_a_recreate() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        table.window_changed(1, Tracked::new(&log, "w1"), (1, 1), same);
        table.window_changed(2, Tracked::new(&log, "w2"), (1, 1), same);
        table.prepare(build_on(&log), resize);

        // The main surface goes away: every surface goes, every window stays.
        assert_eq!(table.drop_surfaces(), 2);
        assert_eq!(drops(&log), vec!["drop surface@w1", "drop surface@w2"]);
        assert!(!table.has_live());
        assert_eq!(table.len(), 2);

        // It comes back: the next prepare rebuilds on the kept windows.
        table.prepare(build_on(&log), resize);
        assert_eq!(table.live_ids().collect::<Vec<_>>(), vec![1, 2]);
        assert!(drops(&log).is_empty());
    }

    #[test]
    fn overlay_table_refuses_once_per_window() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        table.window_changed(1, Tracked::new(&log, "w1"), (1, 1), same);
        let attempts = std::cell::Cell::new(0);
        let mut refuse = |_: &Tracked, _: (u32, u32)| -> Result<Tracked, &'static str> {
            attempts.set(attempts.get() + 1);
            Err("no translucent mode")
        };
        assert_eq!(
            table.prepare(&mut refuse, resize),
            vec![(1, "no translucent mode")]
        );
        // Not retried for the same window...
        assert!(table.prepare(&mut refuse, resize).is_empty());
        assert_eq!(attempts.get(), 1);
        // ...but a new window gets a fresh attempt.
        table.window_changed(1, Tracked::new(&log, "w2"), (1, 1), same);
        assert_eq!(table.prepare(&mut refuse, resize).len(), 1);
        assert_eq!(attempts.get(), 2);
        assert!(!table.has_live());
    }

    #[test]
    fn overlay_table_cuts_follow_the_cut_order_and_skip_dead_overlays() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        for (id, name) in [(1, "a"), (2, "b"), (3, "c")] {
            table.window_changed(id, Tracked::new(&log, name), (1, 1), same);
        }
        table.prepare(build_on(&log), resize);
        table.window_destroyed(2);
        let cuts = table.cut_surfaces(&[(3, 5..9), (2, 9..12), (1, 12..12)]);
        let order: Vec<(u64, std::ops::Range<usize>, String)> = cuts
            .into_iter()
            .map(|(id, range, surface)| (id, range, surface.name.clone()))
            .collect();
        assert_eq!(
            order,
            vec![
                (3, 5..9, "surface@c".to_string()),
                (1, 12..12, "surface@a".to_string())
            ]
        );
    }

    #[test]
    fn overlay_table_latches_error_logging() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        table.window_changed(1, Tracked::new(&log, "w1"), (1, 1), same);
        assert_eq!(table.note_error(1, 2), Some(1));
        assert_eq!(table.note_error(1, 2), Some(2));
        assert_eq!(table.note_error(1, 2), None);
        assert_eq!(table.note_error(9, 2), None);
        // A new window resets the count.
        table.window_changed(1, Tracked::new(&log, "w2"), (1, 1), same);
        assert_eq!(table.note_error(1, 2), Some(1));
    }

    #[test]
    fn overlay_ledger_folds_publishes_and_pairs_frames() {
        let mut ledger = OverlayLedger::new();
        // Generation 1: overlay 1 appears.
        assert_eq!(ledger.begin_fold(1), Some(0));
        ledger.set(1, 10..20);
        ledger.finish_fold(1);
        ledger.publish_frame(1);
        // Nothing new: no fold.
        assert_eq!(ledger.begin_fold(1), None);
        // Generation 2: overlay 2 appears below it, overlay 1's range moves.
        assert_eq!(ledger.begin_fold(2), Some(1));
        ledger.set(2, 4..8);
        ledger.set(1, 12..22);
        ledger.finish_fold(2);
        ledger.publish_frame(2);
        // Generation 3: overlay 1 is disposed.
        ledger.begin_fold(3);
        ledger.dispose(1);
        ledger.finish_fold(3);
        ledger.publish_frame(3);

        // Each frame keeps the ranges it was painted with, sorted by range.
        assert_eq!(ledger.frame_cuts(1), Some(&[(1, 10..20)][..]));
        assert_eq!(ledger.frame_cuts(2), Some(&[(2, 4..8), (1, 12..22)][..]));
        assert_eq!(ledger.frame_cuts(3), Some(&[(2, 4..8)][..]));
        assert_eq!(ledger.frame_cuts(4), None);
    }

    #[test]
    fn overlay_ledger_keeps_a_bounded_window_of_frames() {
        let mut ledger = OverlayLedger::new();
        ledger.begin_fold(1);
        ledger.set(1, 0..1);
        ledger.finish_fold(1);
        for frame in 1..=(OVERLAY_FRAMES_KEPT as u64 + 4) {
            ledger.publish_frame(frame);
        }
        assert_eq!(ledger.frame_cuts(4), None);
        assert!(ledger.frame_cuts(5).is_some());
        assert!(ledger.frame_cuts(OVERLAY_FRAMES_KEPT as u64 + 4).is_some());
        // Republishing an id replaces, never duplicates.
        ledger.begin_fold(2);
        ledger.set(1, 3..4);
        ledger.finish_fold(2);
        ledger.publish_frame(20);
        assert_eq!(ledger.frame_cuts(20), Some(&[(1, 3..4)][..]));
    }

    #[test]
    fn overlay_ledger_restarts_for_a_new_differ() {
        let mut ledger = OverlayLedger::new();
        ledger.begin_fold(5);
        ledger.set(3, 1..2);
        ledger.finish_fold(5);
        ledger.publish_frame(9);
        // A new differ's generations start over: the old ranges go with it.
        assert_eq!(ledger.begin_fold(2), Some(0));
        assert!(ledger.snapshot().is_empty());
        assert_eq!(ledger.frame_cuts(9), None);
        ledger.reset();
        assert_eq!(ledger.begin_fold(0), None);
    }

    #[test]
    fn overlay_cuts_pair_live_surfaces_with_their_frame() {
        let frame = [(2, 4..8), (1, 12..22), (5, 30..40)];
        // Overlay 5 has no live surface: no cut, so the main surface keeps
        // drawing its range. Overlay 7 is live but the frame names no range:
        // a transparent clear at the end.
        let cuts = assemble_overlay_cuts(Some(&frame), [1, 2, 7], 25);
        assert_eq!(cuts, vec![(2, 4..8), (1, 12..22), (7, 25..25)]);
        // A range past the scene clamps; one that clamps empty clears.
        let cuts = assemble_overlay_cuts(Some(&frame), [1, 5], 15);
        assert_eq!(cuts, vec![(1, 12..15), (5, 15..15)]);
        // A frame with no published cuts: every live overlay clears.
        assert_eq!(
            assemble_overlay_cuts(None, [3, 1], 6),
            vec![(1, 6..6), (3, 6..6)]
        );
        assert!(assemble_overlay_cuts(Some(&frame), [], 50).is_empty());
    }

    #[test]
    fn overlay_skew_reports_every_window() {
        use std::time::{Duration, SystemTime};
        let mut skew = OverlaySkew::default();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        for frame in 0..OVERLAY_SKEW_EVERY - 1 {
            let late = Duration::from_micros(if frame == 3 { 900 } else { 100 });
            assert_eq!(skew.record(base, base + late, 2), None);
        }
        // A clock step backwards is zero skew, never a panic.
        let report = skew
            .record(base, base - Duration::from_micros(5), 1)
            .expect("a full window reports");
        assert_eq!(report.frames, OVERLAY_SKEW_EVERY);
        assert_eq!(report.max_us, 900);
        assert_eq!(
            report.avg_us,
            (100 * u64::from(OVERLAY_SKEW_EVERY - 2) + 900) / u64::from(OVERLAY_SKEW_EVERY)
        );
        assert_eq!(report.overlays, 1);
        assert_eq!(report.main_unix_us, 1_000_000_000);
        // The window restarts.
        assert_eq!(skew.record(base, base, 1), None);
    }
}
