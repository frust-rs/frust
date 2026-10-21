//! Overlay surfaces' platform-neutral core: the per-overlay layer/surface
//! table, the `FRUST_OVERLAYS` switch, the frame-request latch, the cut
//! assembly and the present-skew window. Every shell that stacks overlay
//! layers above a hosted native view drives these; `S` (the surface) and `L`
//! (the layer or window it is built on) stay generic, so this names no GPU or
//! platform type and the ordering contracts are host-tested.
//!
//! The one invariant [`OverlayTable`] keeps on every path: **a surface is
//! dropped before its layer is handed back.** The render side takes the live
//! surfaces out for the length of a frame and lands them afterwards, so a
//! withdraw or a replacement that meets a surface in flight never blocks: the
//! layer is handed back by the [`OverlayTable::land`] that drops the surface.
//!
//! What the table buys is ordering and leak-freedom, not memory safety:
//! wgpu-hal 30.0.1 retains a `CAMetalLayer` itself (`Surface::new` takes a
//! `Retained<CAMetalLayer>`), and on Android the window dies when
//! `surfaceDestroyed` returns whatever the table does. See
//! `docs/SHELLS_ARCHITECTURE.md` (Native-view hosting).

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

/// The overlay switch's name.
pub const OVERLAYS_ENV_VAR: &str = "FRUST_OVERLAYS";

/// The switch's older name, still honoured after [`OVERLAYS_ENV_VAR`].
pub const OVERLAY_SPIKE_ENV_VAR: &str = "FRUST_OVERLAY_SPIKE";

/// The `FRUST_OVERLAYS` switch's value; [`Self::Off`] is the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlaySwitch {
    Off,
    On,
    /// Overlays on, with Android's media-overlay z-order (an A/B arm); every
    /// other host treats it as plain [`Self::On`].
    Media,
}

impl OverlaySwitch {
    /// One value: blank, `0`, `off` or `false` is off, `media` is
    /// [`Self::Media`], anything else on (case-insensitive, trimmed).
    pub fn from_value(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty()
            || value == "0"
            || value.eq_ignore_ascii_case("off")
            || value.eq_ignore_ascii_case("false")
        {
            Self::Off
        } else if value.eq_ignore_ascii_case("media") {
            Self::Media
        } else {
            Self::On
        }
    }

    /// The first non-blank source decides, so a runtime `0` switches a
    /// baked-in value off; none set is off.
    pub fn resolve<'a>(sources: impl IntoIterator<Item = Option<&'a str>>) -> Self {
        sources
            .into_iter()
            .flatten()
            .find(|value| !value.trim().is_empty())
            .map_or(Self::Off, Self::from_value)
    }

    /// The process's switch: `FRUST_OVERLAYS` at runtime, then as baked by a
    /// compile-time `--define`, then the `FRUST_OVERLAY_SPIKE` alias the same
    /// two ways. Uncached: a shell reads it once and keeps the answer.
    pub fn from_env() -> Self {
        let runtime = std::env::var(OVERLAYS_ENV_VAR).ok();
        let alias = std::env::var(OVERLAY_SPIKE_ENV_VAR).ok();
        Self::resolve([
            runtime.as_deref(),
            option_env!("FRUST_OVERLAYS"),
            alias.as_deref(),
            option_env!("FRUST_OVERLAY_SPIKE"),
        ])
    }

    /// Whether overlays are on at all.
    pub fn is_on(self) -> bool {
        self != Self::Off
    }

    /// Whether the media-overlay arm was asked for.
    pub fn media(self) -> bool {
        self == Self::Media
    }
}

/// A request for one frame, raised when an overlay's layer changes or goes
/// and taken by the frame gate: the surface is built on the render side ahead
/// of a frame, so an idle app would otherwise never draw onto it.
#[derive(Debug, Default)]
pub struct FrameRequest(AtomicBool);

impl FrameRequest {
    /// No request pending (`const`, for a process-global).
    pub const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    /// Ask for one frame; requests before the next take collapse into one.
    pub fn request(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether a frame was requested since the last take; clears it.
    pub fn take(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }
}

#[derive(Debug)]
struct OverlayEntry<L, S> {
    // Fields drop in declaration order: even an implicit drop releases the
    // surface before either layer.
    surface: Option<S>,
    /// The layer the in-flight surface was built on, once withdrawn or
    /// replaced: handed back by the land that drops that surface.
    retiring: Option<L>,
    /// The current layer; `None` once withdrawn while its surface flies.
    layer: Option<L>,
    size: (u32, u32),
    refused: bool,
    in_flight: bool,
    /// The main surface went away while this surface was in flight, so it
    /// lands on a dead device and is dropped rather than kept.
    stale: bool,
    errors: u32,
    unservable_logged: bool,
}

impl<L, S> OverlayEntry<L, S> {
    fn new(layer: L, size: (u32, u32)) -> Self {
        Self {
            surface: None,
            retiring: None,
            layer: Some(layer),
            size,
            refused: false,
            in_flight: false,
            stale: false,
            errors: 0,
            unservable_logged: false,
        }
    }
}

/// What [`OverlayTable::withdraw`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Withdraw<L> {
    /// No surface on the layer was in flight: it was dropped, and the layer
    /// is the caller's to release now.
    Released(L),
    /// The layer's surface is in flight: the layer comes back from the next
    /// [`OverlayTable::land`], after the surface drops. A frame is owed.
    Deferred,
    /// No such overlay (or it was already withdrawn).
    Unknown,
}

/// What [`OverlayTable::land`] did.
#[derive(Debug)]
pub struct Landed<L> {
    /// Layers withdrawn or replaced while their surface flew; each surface
    /// was dropped first, so they are the caller's to release.
    pub released_layers: Vec<(u64, L)>,
    /// Surfaces handed back for an overlay the table no longer tracks.
    pub orphaned: usize,
}

/// [`OverlayTable::take_for_frame`]'s answer: the surfaces taken out
/// (ascending id) and the build refusals that call made.
pub type FrameTake<S, E> = (Vec<(u64, S)>, Vec<(u64, E)>);

/// The overlays by the differ's overlay id: each one's layer (owned by the
/// table between publish and hand-back) and the surface built on it.
///
/// The UI thread publishes and withdraws layers; the render side builds,
/// resizes and takes the surfaces out for a frame ([`Self::take_for_frame`])
/// and lands them ([`Self::land`]), so no lock is held across a drawable
/// acquire and nothing here ever waits.
#[derive(Debug)]
pub struct OverlayTable<L, S> {
    entries: BTreeMap<u64, OverlayEntry<L, S>>,
}

impl<L, S> Default for OverlayTable<L, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<L, S> OverlayTable<L, S> {
    /// An empty table (`const`, for a process-global).
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Overlay `id`'s view delivered `layer` at `size` physical px. Answers
    /// whether a frame is owed: `true` for a new overlay, a new size, a
    /// different layer or a re-attach after a withdraw; `false` for an
    /// unchanged republish (whose redundant `layer` is dropped).
    ///
    /// A replaced layer is dropped after its surface; one whose surface is in
    /// flight is handed back by the next [`Self::land`] instead.
    pub fn publish(&mut self, id: u64, layer: L, size: (u32, u32)) -> bool
    where
        L: PartialEq,
    {
        let Some(entry) = self.entries.get_mut(&id) else {
            self.entries.insert(id, OverlayEntry::new(layer, size));
            return true;
        };
        if entry.layer.as_ref() == Some(&layer) {
            return std::mem::replace(&mut entry.size, size) != size;
        }
        // Surface first. A layer whose surface flies waits for its land; a
        // successor published meanwhile never got a surface and just drops.
        entry.surface = None;
        if entry.in_flight && entry.retiring.is_none() {
            entry.retiring = entry.layer.take();
        }
        entry.layer = Some(layer);
        entry.size = size;
        entry.refused = false;
        entry.errors = 0;
        entry.unservable_logged = false;
        true
    }

    /// Overlay `id`'s view is going away. Never blocks: a surface in flight
    /// defers the hand-back to the next [`Self::land`] ([`Withdraw::Deferred`]),
    /// which the caller treats as a frame owed so that land comes.
    pub fn withdraw(&mut self, id: u64) -> Withdraw<L> {
        let Some(entry) = self.entries.get_mut(&id) else {
            return Withdraw::Unknown;
        };
        let Some(layer) = entry.layer.take() else {
            return Withdraw::Unknown;
        };
        if entry.in_flight {
            if entry.retiring.is_none() {
                entry.retiring = Some(layer);
                return Withdraw::Deferred;
            }
            // A successor published while the old surface flew: never built on.
            return Withdraw::Released(layer);
        }
        self.entries.remove(&id);
        Withdraw::Released(layer)
    }

    /// The main surface is being recreated or destroyed: drop every surface
    /// held here (they share its device), mark the in-flight ones to be
    /// dropped when they land, keep every layer, and clear refusals so the
    /// next frame retries. Answers how many surfaces were dropped now.
    pub fn drop_surfaces(&mut self) -> usize {
        let mut dropped = 0;
        for entry in self.entries.values_mut() {
            if entry.surface.take().is_some() {
                dropped += 1;
            }
            entry.stale |= entry.in_flight;
            entry.refused = false;
        }
        dropped
    }

    /// Ahead of a frame, on the render side: resize each held surface to its
    /// layer's size, `build` one where a layer has none (and was not
    /// refused), then take every surface out, marked in flight. Overlays
    /// already in flight, withdrawn ones included, are skipped. A refused
    /// layer is not retried until it is replaced or [`Self::drop_surfaces`].
    pub fn take_for_frame<E>(
        &mut self,
        mut build: impl FnMut(&L, (u32, u32)) -> Result<S, E>,
        mut resize: impl FnMut(&mut S, (u32, u32)),
    ) -> FrameTake<S, E> {
        let mut flying = Vec::new();
        let mut refusals = Vec::new();
        for (&id, entry) in &mut self.entries {
            if entry.in_flight {
                continue;
            }
            let Some(layer) = entry.layer.as_ref() else {
                continue;
            };
            match entry.surface.as_mut() {
                Some(surface) => resize(surface, entry.size),
                None if !entry.refused => match build(layer, entry.size) {
                    Ok(surface) => entry.surface = Some(surface),
                    Err(error) => {
                        entry.refused = true;
                        refusals.push((id, error));
                    }
                },
                None => {}
            }
            if let Some(surface) = entry.surface.take() {
                entry.in_flight = true;
                flying.push((id, surface));
            }
        }
        (flying, refusals)
    }

    /// Hand back the surfaces [`Self::take_for_frame`] took. A surface whose
    /// layer was withdrawn or replaced meanwhile is dropped, then that layer
    /// is answered for release; a withdrawn overlay is then forgotten.
    pub fn land(&mut self, flying: Vec<(u64, S)>) -> Landed<L> {
        let mut landed = Landed {
            released_layers: Vec::new(),
            orphaned: 0,
        };
        for (id, surface) in flying {
            let Some(entry) = self.entries.get_mut(&id).filter(|entry| entry.in_flight) else {
                drop(surface);
                landed.orphaned += 1;
                continue;
            };
            entry.in_flight = false;
            let stale = std::mem::take(&mut entry.stale);
            match entry.retiring.take() {
                Some(retired) => {
                    drop(surface);
                    landed.released_layers.push((id, retired));
                    if entry.layer.is_none() {
                        self.entries.remove(&id);
                    }
                }
                None if stale => drop(surface),
                None => entry.surface = Some(surface),
            }
        }
        landed
    }

    /// Count one engine refusal on overlay `id`; the running count while it
    /// is at most `log_limit` (the caller logs then), `None` past it.
    pub fn note_error(&mut self, id: u64, log_limit: u32) -> Option<u32> {
        let entry = self.entries.get_mut(&id)?;
        entry.errors = entry.errors.saturating_add(1);
        (entry.errors <= log_limit).then_some(entry.errors)
    }

    /// `true` exactly once per overlay layer: its first cut kept on the main
    /// surface, for a log line that must not repeat every frame.
    pub fn note_unservable(&mut self, id: u64) -> bool {
        self.entries
            .get_mut(&id)
            .is_some_and(|entry| !std::mem::replace(&mut entry.unservable_logged, true))
    }

    /// How many overlays the table tracks, withdrawn-but-in-flight included.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table tracks no overlay.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The overlays holding a surface right now (not in flight), ascending.
    pub fn live(&self) -> impl Iterator<Item = u64> + '_ {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.surface.is_some())
            .map(|(&id, _)| id)
    }
}

/// One overlay's cut of one frame: its id and the half-open command range of
/// that frame's scene it draws.
pub type OverlayCutSpec = (u64, Range<usize>);

/// The cuts a segmented frame of `scene_len` commands draws: one per live
/// overlay, carrying its range from `spans` clamped to the scene, or an empty
/// range at the scene's end (a transparent clear) when the frame names none
/// or it clamps empty. An overlay in `spans` with no live surface gets no
/// cut, so its commands stay on the main surface. Ascending by range, the
/// order `encode_segmented` takes.
pub fn assemble_overlay_cuts(
    spans: &[OverlayCutSpec],
    live_ids: impl IntoIterator<Item = u64>,
    scene_len: usize,
) -> Vec<OverlayCutSpec> {
    let mut cuts: Vec<OverlayCutSpec> = live_ids
        .into_iter()
        .map(|id| {
            let range = spans
                .iter()
                .find(|(span, _)| *span == id)
                .map(|(_, range)| range.start.min(scene_len)..range.end.min(scene_len))
                .filter(|range| range.start < range.end)
                .unwrap_or(scene_len..scene_len);
            (id, range)
        })
        .collect();
    cuts.sort_by_key(|(id, range)| (range.start, range.end, *id));
    cuts
}

/// Frames folded into one [`SkewReport`].
pub const SKEW_WINDOW_FRAMES: u32 = 120;

/// One window's main-to-last-overlay present skew: frames folded, mean and
/// largest delta in µs, and the overlays presented in its last frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkewReport {
    pub frames: u32,
    pub avg_us: u64,
    pub max_us: u64,
    pub overlays: usize,
}

/// Main-versus-overlay present skew, folded per frame and reported every
/// [`SKEW_WINDOW_FRAMES`] frames.
#[derive(Debug, Default)]
pub struct SkewWindow {
    frames: u32,
    sum_us: u64,
    max_us: u64,
}

impl SkewWindow {
    /// An empty window (`const`, for a process-global).
    pub const fn new() -> Self {
        Self {
            frames: 0,
            sum_us: 0,
            max_us: 0,
        }
    }

    /// Fold one frame whose last of `overlays` overlays presented `delta`
    /// after the main surface; a report closes each full window.
    pub fn record(&mut self, delta: Duration, overlays: usize) -> Option<SkewReport> {
        let delta = u64::try_from(delta.as_micros()).unwrap_or(u64::MAX);
        self.frames += 1;
        self.sum_us = self.sum_us.saturating_add(delta);
        self.max_us = self.max_us.max(delta);
        if self.frames < SKEW_WINDOW_FRAMES {
            return None;
        }
        let report = SkewReport {
            frames: self.frames,
            avg_us: self.sum_us / u64::from(self.frames),
            max_us: self.max_us,
            overlays,
        };
        *self = Self::new();
        Some(report)
    }

    /// [`Self::record`] from wall-clock present times; a clock that stepped
    /// backwards in between counts as zero skew.
    pub fn record_between(
        &mut self,
        main: SystemTime,
        last_overlay: SystemTime,
        overlays: usize,
    ) -> Option<SkewReport> {
        self.record(
            last_overlay.duration_since(main).unwrap_or_default(),
            overlays,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Log = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    /// A layer or surface stand-in that records its own drop; two handles
    /// with one name are the same platform object.
    #[derive(Debug)]
    struct Rec {
        name: String,
        log: Log,
        size: (u32, u32),
    }

    impl Rec {
        fn new(log: &Log, name: &str) -> Self {
            Self {
                name: name.to_string(),
                log: log.clone(),
                size: (0, 0),
            }
        }
    }

    impl PartialEq for Rec {
        fn eq(&self, other: &Self) -> bool {
            self.name == other.name
        }
    }

    impl Drop for Rec {
        fn drop(&mut self) {
            self.log.borrow_mut().push(format!("drop {}", self.name));
        }
    }

    type Table = OverlayTable<Rec, Rec>;

    fn build_on(log: &Log) -> impl FnMut(&Rec, (u32, u32)) -> Result<Rec, String> + '_ {
        move |layer, size| {
            let mut surface = Rec::new(log, &format!("surface@{}", layer.name));
            surface.size = size;
            Ok(surface)
        }
    }

    fn resize(surface: &mut Rec, size: (u32, u32)) {
        surface.size = size;
    }

    fn drops(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.borrow_mut())
    }

    fn names(flying: &[(u64, Rec)]) -> Vec<(u64, &str)> {
        flying
            .iter()
            .map(|(id, s)| (*id, s.name.as_str()))
            .collect()
    }

    fn take(table: &mut Table, log: &Log) -> Vec<(u64, Rec)> {
        let (flying, refusals) = table.take_for_frame(build_on(log), resize);
        assert!(refusals.is_empty());
        flying
    }

    fn released(landed: Landed<Rec>) -> Vec<(u64, String)> {
        assert_eq!(landed.orphaned, 0);
        landed
            .released_layers
            .into_iter()
            .map(|(id, layer)| (id, layer.name.clone()))
            .collect()
    }

    #[test]
    fn publish_answers_whether_a_frame_is_owed() {
        let log = Log::default();
        let mut table = Table::new();
        assert!(table.publish(1, Rec::new(&log, "a"), (10, 10)), "new");
        assert!(
            !table.publish(1, Rec::new(&log, "a"), (10, 10)),
            "unchanged"
        );
        assert_eq!(drops(&log), ["drop a"], "the redundant handle drops");
        assert!(table.publish(1, Rec::new(&log, "a"), (20, 10)), "resized");
        drops(&log);
        let flying = take(&mut table, &log);
        assert_eq!(flying[0].1.size, (20, 10));
        table.land(flying);

        // A different layer: old surface, then old layer.
        assert!(table.publish(1, Rec::new(&log, "b"), (20, 10)), "replaced");
        assert_eq!(drops(&log), ["drop surface@a", "drop a"]);
        assert_eq!(table.live().count(), 0, "the new layer waits for a build");

        // A re-attach of the same layer after a deferred withdraw.
        let flying = take(&mut table, &log);
        assert_eq!(table.withdraw(1), Withdraw::Deferred);
        assert!(
            table.publish(1, Rec::new(&log, "b"), (20, 10)),
            "re-attached"
        );
        assert!(drops(&log).is_empty());
        assert_eq!(released(table.land(flying)), [(1, "b".to_string())]);
        assert_eq!(drops(&log), ["drop surface@b", "drop b"]);
        assert_eq!(names(&take(&mut table, &log)), [(1, "surface@b")]);
    }

    #[test]
    fn withdraw_of_an_idle_overlay_drops_its_surface_and_hands_the_layer_back() {
        let log = Log::default();
        let mut table = Table::new();
        assert_eq!(table.withdraw(7), Withdraw::Unknown);
        table.publish(1, Rec::new(&log, "a"), (1, 1));
        let flying = take(&mut table, &log);
        table.land(flying);

        let Withdraw::Released(layer) = table.withdraw(1) else {
            panic!("an idle overlay releases at once");
        };
        assert_eq!(drops(&log), ["drop surface@a"]);
        drop(layer);
        assert_eq!(drops(&log), ["drop a"]);
        assert!(table.is_empty());
        assert_eq!(table.withdraw(1), Withdraw::Unknown, "a second withdraw");
    }

    #[test]
    fn withdraw_in_flight_defers_until_the_land_drops_the_surface() {
        let log = Log::default();
        let mut table = Table::new();
        table.publish(1, Rec::new(&log, "a"), (1, 1));
        table.publish(2, Rec::new(&log, "b"), (1, 1));
        let flying = take(&mut table, &log);

        assert_eq!(table.withdraw(1), Withdraw::Deferred);
        assert_eq!(table.withdraw(1), Withdraw::Unknown, "already pending");
        assert!(drops(&log).is_empty(), "nothing drops while it flies");
        assert_eq!(table.len(), 2);
        // A frame while it flies skips it rather than building again.
        assert!(take(&mut table, &log).is_empty());

        let landed = table.land(flying);
        assert_eq!(drops(&log), ["drop surface@a"], "surface before layer");
        assert_eq!(released(landed), [(1, "a".to_string())]);
        assert_eq!(drops(&log), ["drop a"]);
        assert_eq!(table.live().collect::<Vec<_>>(), [2]);
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn replacing_a_layer_in_flight_releases_the_old_one_on_land() {
        let log = Log::default();
        let mut table = Table::new();
        table.publish(1, Rec::new(&log, "a"), (1, 1));
        let flying = take(&mut table, &log);
        assert!(table.publish(1, Rec::new(&log, "b"), (2, 2)));
        // A third layer replaces the never-built second one outright.
        assert!(table.publish(1, Rec::new(&log, "c"), (3, 3)));
        assert_eq!(drops(&log), ["drop b"]);

        assert_eq!(released(table.land(flying)), [(1, "a".to_string())]);
        assert_eq!(drops(&log), ["drop surface@a", "drop a"]);
        let flying = take(&mut table, &log);
        assert_eq!(names(&flying), [(1, "surface@c")]);
        assert_eq!(flying[0].1.size, (3, 3));
    }

    #[test]
    fn withdraw_of_a_successor_releases_it_while_the_old_surface_flies() {
        let log = Log::default();
        let mut table = Table::new();
        table.publish(1, Rec::new(&log, "a"), (1, 1));
        let flying = take(&mut table, &log);
        table.publish(1, Rec::new(&log, "b"), (1, 1));
        let Withdraw::Released(successor) = table.withdraw(1) else {
            panic!("the successor has no surface to wait for");
        };
        assert_eq!(successor.name, "b");
        drop(successor);
        drops(&log);

        assert_eq!(released(table.land(flying)), [(1, "a".to_string())]);
        assert_eq!(drops(&log), ["drop surface@a", "drop a"]);
        assert!(table.is_empty());
    }

    #[test]
    fn drop_surfaces_keeps_layers_and_discards_a_surface_in_flight() {
        let log = Log::default();
        let mut table = Table::new();
        table.publish(1, Rec::new(&log, "a"), (1, 1));
        table.publish(2, Rec::new(&log, "b"), (1, 1));
        let mut flying = take(&mut table, &log);
        let second = flying.pop().expect("two taken");
        table.land(vec![second]);

        // Overlay 1 is in flight, overlay 2 holds its surface.
        assert_eq!(table.drop_surfaces(), 1);
        assert_eq!(drops(&log), ["drop surface@b"]);
        // The in-flight surface was built on the old device: it drops on land.
        assert!(released(table.land(flying)).is_empty());
        assert_eq!(drops(&log), ["drop surface@a"]);
        assert_eq!(table.len(), 2);
        assert_eq!(table.live().count(), 0);

        let flying = take(&mut table, &log);
        assert_eq!(names(&flying), [(1, "surface@a"), (2, "surface@b")]);
        table.land(flying);
        drop(table);
        assert_eq!(
            drops(&log),
            ["drop surface@a", "drop a", "drop surface@b", "drop b"],
            "dropping the table keeps the order"
        );
    }

    #[test]
    fn a_landed_surface_for_an_unknown_overlay_is_orphaned() {
        let log = Log::default();
        let mut table = Table::new();
        let landed = table.land(vec![(9, Rec::new(&log, "stray"))]);
        assert_eq!(landed.orphaned, 1);
        assert!(landed.released_layers.is_empty());
        assert_eq!(drops(&log), ["drop stray"]);
    }

    #[test]
    fn a_refused_build_is_reported_once_until_a_new_layer_or_main_surface() {
        let log = Log::default();
        let mut table = Table::new();
        table.publish(1, Rec::new(&log, "a"), (1, 1));
        let attempts = std::cell::Cell::new(0);
        let mut refuse = |_: &Rec, _: (u32, u32)| -> Result<Rec, &'static str> {
            attempts.set(attempts.get() + 1);
            Err("opaque only")
        };
        let (flying, refusals) = table.take_for_frame(&mut refuse, resize);
        assert!(flying.is_empty());
        assert_eq!(refusals, [(1, "opaque only")]);
        assert!(table.take_for_frame(&mut refuse, resize).1.is_empty());
        assert_eq!(attempts.get(), 1);

        table.drop_surfaces();
        assert_eq!(table.take_for_frame(&mut refuse, resize).1.len(), 1);
        table.publish(1, Rec::new(&log, "b"), (1, 1));
        assert_eq!(table.take_for_frame(&mut refuse, resize).1.len(), 1);
        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn log_latches_fire_once_per_layer() {
        let log = Log::default();
        let mut table = Table::new();
        assert!(!table.note_unservable(1), "unknown overlay");
        assert_eq!(table.note_error(1, 2), None);
        table.publish(1, Rec::new(&log, "a"), (1, 1));
        assert!(table.note_unservable(1));
        assert!(!table.note_unservable(1));
        assert_eq!(table.note_error(1, 2), Some(1));
        assert_eq!(table.note_error(1, 2), Some(2));
        assert_eq!(table.note_error(1, 2), None);
        table.publish(1, Rec::new(&log, "b"), (1, 1));
        assert!(table.note_unservable(1), "a new layer logs again");
        assert_eq!(table.note_error(1, 2), Some(1));
    }

    #[test]
    fn overlay_switch_spellings() {
        use OverlaySwitch::{Media, Off, On};
        let table = [
            ("", Off),
            ("  ", Off),
            ("0", Off),
            ("off", Off),
            ("OFF", Off),
            ("false", Off),
            (" False ", Off),
            ("1", On),
            ("on", On),
            ("yes", On),
            ("top", On),
            ("media", Media),
            (" Media ", Media),
        ];
        for (value, expected) in table {
            assert_eq!(OverlaySwitch::from_value(value), expected, "{value:?}");
        }
        assert!(!Off.is_on() && On.is_on() && Media.is_on());
        assert!(Media.media() && !On.media());
    }

    #[test]
    fn overlay_switch_first_set_source_decides() {
        assert_eq!(
            OverlaySwitch::resolve([None, None, None]),
            OverlaySwitch::Off
        );
        assert_eq!(
            OverlaySwitch::resolve([Some("0"), Some("1"), None]),
            OverlaySwitch::Off
        );
        assert_eq!(
            OverlaySwitch::resolve([None, Some("media"), Some("1")]),
            OverlaySwitch::Media
        );
        assert_eq!(
            OverlaySwitch::resolve([Some(" "), None, Some("1")]),
            OverlaySwitch::On
        );
    }

    #[test]
    fn frame_requests_collapse_into_one_take() {
        static LATCH: FrameRequest = FrameRequest::new();
        assert!(!LATCH.take());
        LATCH.request();
        LATCH.request();
        assert!(LATCH.take());
        assert!(!LATCH.take());
    }

    #[test]
    fn cuts_pair_live_surfaces_with_their_spans() {
        let spans = [(2, 4..8), (1, 12..22), (5, 30..40), (6, 9..9)];
        // 5 has no live surface: no cut. 7 has no span and 6 an empty one:
        // each gets a transparent clear at the scene's end.
        assert_eq!(
            assemble_overlay_cuts(&spans, [1, 2, 6, 7], 25),
            [(2, 4..8), (1, 12..22), (6, 25..25), (7, 25..25)]
        );
        // A span past the scene clamps; one that clamps empty clears.
        assert_eq!(
            assemble_overlay_cuts(&spans, [1, 5], 15),
            [(1, 12..15), (5, 15..15)]
        );
        assert_eq!(
            assemble_overlay_cuts(&[], [3, 1], 6),
            [(1, 6..6), (3, 6..6)]
        );
        assert!(assemble_overlay_cuts(&spans, [], 50).is_empty());
    }

    #[test]
    fn skew_reports_every_window() {
        let mut skew = SkewWindow::default();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        for frame in 0..SKEW_WINDOW_FRAMES - 1 {
            let late = Duration::from_micros(if frame == 3 { 900 } else { 100 });
            assert_eq!(skew.record_between(base, base + late, 2), None);
        }
        // A clock step backwards is zero skew, never a panic.
        let report = skew
            .record_between(base, base - Duration::from_micros(5), 1)
            .expect("a full window reports");
        assert_eq!(report.frames, SKEW_WINDOW_FRAMES);
        assert_eq!(report.max_us, 900);
        assert_eq!(
            report.avg_us,
            (100 * u64::from(SKEW_WINDOW_FRAMES - 2) + 900) / u64::from(SKEW_WINDOW_FRAMES)
        );
        assert_eq!(report.overlays, 1);
        assert_eq!(skew.record(Duration::ZERO, 1), None, "the window restarts");
    }
}
