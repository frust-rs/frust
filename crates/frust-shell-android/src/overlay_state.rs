//! Overlay surfaces' Android-only state: the mode Kotlin builds overlay
//! `SurfaceView`s by, the per-frame ledger of overlay command ranges, and the
//! pairing of a frame's cuts with the surfaces taken out for it.
//!
//! An overlay is a translucent `SurfaceView` Kotlin stacks above a hosted
//! native view; the render thread draws the frame's commands painted after
//! that view onto it (`frust_render::overlay`). The overlay table, the
//! `FRUST_OVERLAYS` switch, the frame-request latch, cut assembly and the skew
//! window are `frust_shell_common::overlay`'s, shared with the Apple shells;
//! the JNI half (the windows, the surfaces, the render-loop wiring) lives in
//! `crate::jni_glue`. Nothing here needs `jni`, `ndk` or a GPU, so it is
//! host-tested.
//!
//! Ordering contracts:
//!
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

/// How (and whether) Kotlin builds overlay surfaces: the `FRUST_OVERLAYS`
/// switch's value as the code `nativeOverlayMode` hands the embedding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OverlayMode {
    /// No overlays: the differ is fed without the frame's commands, the JSON
    /// carries no overlay op and the frame path is the single-surface one.
    Off,
    /// Overlay `SurfaceView`s are `setZOrderOnTop(true)`: above the window,
    /// so above every hosted `View`. The default when switched on.
    OnTop,
    /// Overlay `SurfaceView`s are `setZOrderMediaOverlay(true)`
    /// (`FRUST_OVERLAYS=media`): above the main surface but behind the
    /// window, kept as an A/B arm.
    MediaOverlay,
}

impl OverlayMode {
    /// The mode for a switch that is `on`, asking for the `media` arm or not
    /// (the shared switch's `is_on`/`media`). Taken as two flags so the
    /// mapping stays host-testable without the Android-only shared crate.
    pub(crate) fn new(on: bool, media: bool) -> Self {
        match (on, media) {
            (false, _) => Self::Off,
            (true, false) => Self::OnTop,
            (true, true) => Self::MediaOverlay,
        }
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

/// How many frames' cut lists [`OverlayLedger`] keeps for the render thread
/// to pair against — above the platform-view release gate's in-flight bound
/// (`MAX_FRAMES_IN_FLIGHT`, 12), so a frame the render thread can still draw
/// always finds its own cuts.
pub(crate) const OVERLAY_FRAMES_KEPT: usize = 16;

/// One overlay's cut for one frame: its id and the half-open command range of
/// that frame's scene it draws. The same shape as `frust_shell_common`'s
/// `overlay::OverlayCutSpec`, spelled out here because that crate is an
/// Android-only dependency and the ledger is host-tested.
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

/// The surfaces a frame took out of the overlay table, paired with its cuts:
/// one entry per cut, in the cuts' order (the order
/// `SurfaceRenderer::encode_segmented` was given them), each with its range.
/// A cut whose overlay has no surface in `flying` is left out, and no surface
/// is handed out twice.
pub(crate) fn pair_cuts<'a, S>(
    flying: &'a mut [(u64, S)],
    cuts: &[OverlayCutSpec],
) -> Vec<(u64, std::ops::Range<usize>, &'a mut S)> {
    let mut surfaces: std::collections::BTreeMap<u64, &'a mut S> = flying
        .iter_mut()
        .map(|(id, surface)| (*id, surface))
        .collect();
    cuts.iter()
        .filter_map(|(id, range)| {
            surfaces
                .remove(id)
                .map(|surface| (*id, range.clone(), surface))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_mode_maps_the_switch_to_kotlin_codes() {
        assert_eq!(OverlayMode::new(false, false), OverlayMode::Off);
        assert_eq!(OverlayMode::new(false, true), OverlayMode::Off);
        assert_eq!(OverlayMode::new(true, false), OverlayMode::OnTop);
        assert_eq!(OverlayMode::new(true, true), OverlayMode::MediaOverlay);
        assert_eq!(OverlayMode::Off.code(), 0);
        assert_eq!(OverlayMode::OnTop.code(), 1);
        assert_eq!(OverlayMode::MediaOverlay.code(), 2);
        assert!(!OverlayMode::Off.is_on());
        assert!(OverlayMode::OnTop.is_on());
        assert!(OverlayMode::MediaOverlay.is_on());
    }

    #[test]
    fn cuts_pair_with_flying_surfaces_in_cut_order() {
        let mut flying = vec![(1, "a"), (3, "c"), (4, "d")];
        // Overlay 2 was withdrawn before the take: no surface, no entry.
        // A repeated id hands its surface out once.
        let cuts = [(3, 5..9), (2, 9..12), (1, 12..12), (3, 20..21)];
        let paired: Vec<(u64, std::ops::Range<usize>, &str)> = pair_cuts(&mut flying, &cuts)
            .into_iter()
            .map(|(id, range, surface)| (id, range, *surface))
            .collect();
        assert_eq!(paired, vec![(3, 5..9, "c"), (1, 12..12, "a")]);
        assert!(pair_cuts(&mut flying, &[]).is_empty());
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
}
