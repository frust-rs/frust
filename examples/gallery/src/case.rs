//! [`Case`]: one entry in the shared registry [`crate::cases`] concatenates —
//! a slug, a pure [`frust_core::View`] constructor, and the design/variant it
//! is meant to be rendered under. See the [crate docs](crate) for the
//! registry-wide contract (pure `View`, no reactive runtime).

use kurbo::Size;

/// Which theme brightness a [`Case`] (or a design-system theme lookup) wants
/// — see [`crate::theme::theme`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Light,
    Dark,
}

/// Which design system a [`Case`] is rendered under. `Base` is the
/// framework's own [`frust_theme::Theme::neutral`] baseline — no design
/// system plugin involved; the rest name a `plugins/*` catalog. See
/// [`crate::theme::theme`] for how each maps onto a concrete theme
/// constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Design {
    Base,
    Material,
    Cupertino,
    Glyph,
    Shadcn,
    Beui,
}

/// One registry entry: a slug, display title, the logical viewport/scale/
/// frame-time to record it at, which design/variant pairing it targets, and
/// the pure constructor that builds its [`frust_core::View`] tree.
///
/// `build` returns a type-erased [`frust_core::AnyView`] over `()` state — a
/// `Case` carries no reactive runtime, no owner and no clock (see the
/// [crate docs](crate) and `crates/frust-testing/src/frame.rs`'s
/// `record_view`, the pure-`View` recorder every consumer of this registry
/// records a `Case` through).
#[derive(Clone, Copy)]
pub struct Case {
    /// The website page's file stem (e.g. `button`, `icon-button`,
    /// `animated-opacity`), prefixed for a design-system variant
    /// (`material/button`, `shadcn/button`, `glyph/button`, `beui/button`,
    /// `cupertino/button`). Unique across the whole registry — see
    /// `tests/registry.rs`.
    pub slug: &'static str,
    /// Human-readable case title, for a gallery UI to display.
    pub title: &'static str,
    /// The logical viewport this case is laid out at. [`Case::DEFAULT_SIZE`]
    /// (360x240) unless a case has a reason to differ.
    pub size: Size,
    /// The device-pixel scale the recorded frame is painted at.
    /// [`Case::DEFAULT_SCALE`] (2.0) unless a case has a reason to differ.
    pub scale: f64,
    /// The fixed frame timestamp, in milliseconds, an animating case is
    /// recorded at. [`Case::DEFAULT_TIME_MS`] (0, i.e. rest) unless a case
    /// deliberately pins a later point in an animation.
    ///
    /// Setting this above zero is also how a case asks the STATIC recorder for
    /// a warm pass — see [`Case::warm_frames`], which derives its count from
    /// this field, and which is what makes a non-zero value take effect at
    /// all.
    pub time_ms: u64,
    /// Which design system (and, transitively via [`crate::theme::theme`],
    /// which theme) this case renders under.
    pub design: Design,
    /// The pure `View` constructor this case records — called fresh on every
    /// rebuild pass, exactly like an app's `app_logic`.
    pub build: fn() -> frust_core::AnyView<()>,
}

impl Case {
    /// The default logical viewport a case is laid out at when it has no
    /// reason to pick a different one: 360x240.
    pub const DEFAULT_SIZE: Size = Size::new(360.0, 240.0);
    /// The default device-pixel scale a case is recorded at: 2.0.
    pub const DEFAULT_SCALE: f64 = 2.0;
    /// The default fixed frame time a case is recorded at: 0ms (rest, no
    /// animation offset).
    pub const DEFAULT_TIME_MS: u64 = 0;

    /// The warm-pass count a case that pins a non-zero [`Case::time_ms`] is
    /// recorded with: one.
    ///
    /// One is what a staged ramp needs and all it needs — a single discarded
    /// paint to seed the animation's clock, so the captured paint is the
    /// second `advance` and sees the whole of `time_ms` as its delta. A count
    /// above one only matters for a motion that INTEGRATES per frame (a
    /// spring), where several small deltas and one large one are not the same
    /// answer; expressing that per case would need a real field here, which
    /// is the follow-up described on [`Case::warm_frames`].
    pub const STAGED_WARM_FRAMES: u8 = 1;

    /// How many discarded warm paints the static recorder runs before
    /// capturing this case's frame.
    ///
    /// # Why a non-zero `time_ms` needs this at all
    ///
    /// A one-paint recorder cannot show a staged animation at anything but
    /// its start. `AnimationController::advance`
    /// (`crates/frust-core/src/anim.rs`) derives its delta from the previous
    /// `advance`, so the first call after a motion starts only seeds the
    /// clock and contributes a zero delta — and a presence driver that
    /// latches its `started` instant on its first paint
    /// (`plugins/beui/src/motion/presence.rs`) does the same. Recording a
    /// case at `time_ms: 400` therefore used to produce *the identical frame*
    /// as recording it at `0`: the poster showed an invisible or unsettled
    /// widget, and the declared timestamp did nothing.
    ///
    /// So the two halves are inseparable, and this derives one from the
    /// other: a case that pins a later point in an animation gets the warm
    /// pass that makes that point reachable, and a case at rest
    /// ([`Case::DEFAULT_TIME_MS`]) gets none. A case that opts in moves its
    /// committed poster, so it must also be named in the opt-in allowlist the
    /// tests below assert against — a poster cannot move silently.
    ///
    /// # How a case opts in
    ///
    /// By setting its own `time_ms`, at its literal in the registry — no
    /// second field, and no edit anywhere else:
    ///
    /// ```text
    /// time_ms: 400,   // was: Case::DEFAULT_TIME_MS
    /// ```
    ///
    /// The number is the point in the animation the poster should show,
    /// usually at or just past where the motion settles.
    ///
    /// # Why the count is derived rather than declared
    ///
    /// A `warm_frames: u8` field on [`Case`] would read better at the literal,
    /// but every literal in the registry spells out all of its fields and none
    /// carries a `..` base, so adding one field is an edit to all 107 of them
    /// across 17 modules. (Functional-record-update syntax IS admissible in
    /// these `const` slices — that was checked, not assumed — but it only
    /// helps literals already written to use it.) Deriving the count keeps the
    /// capability and its opt-in to the recorder and this file; promoting it
    /// to a real field is the follow-up to take when a case genuinely needs a
    /// count of its own.
    ///
    /// # Not the browser gallery's concern
    ///
    /// `examples/web-gallery` runs a live event loop with a real clock, so its
    /// animations advance on their own; warming exists for the STATIC
    /// recorder, which paints each case a fixed number of times and stops.
    #[must_use]
    pub const fn warm_frames(&self) -> u8 {
        if self.time_ms > Self::DEFAULT_TIME_MS {
            Self::STAGED_WARM_FRAMES
        } else {
            0
        }
    }
}

/// Every case that records a warm pass, each named by the change that opted it
/// in. Opting a case in moves a published frust.dev poster, so the list exists
/// to make that deliberate: a case cannot start warming without an edit here,
/// and the edit is where the reason is recorded.
///
/// Public because the guard is asserted from two crates. `frust-testing` owns
/// the recorder and checks the frame inputs it derives; this crate owns the
/// registry and checks the cases themselves. Both must agree on which posters
/// are allowed to have moved, and a second hand-maintained copy of that list
/// is precisely how the first version of this guard went stale.
pub const WARM_PASS_OPT_INS: &[&str] = &[
    // si-10: recorded completely blank. The palette panel and its scrim are
    // both `Presence`-staged, so both captured at progress 0.
    "beui/command-palette",
    // si-10: recorded without its balance figure, which arrives on a ten-cell
    // stagger over a spring settling at roughly 556 ms.
    "beui/wallet-card",
    // si-07: this poster does NOT move — it is listed because the guard is on
    // the warm pass, not on the pixels. The case dropped its
    // `OverlayEntrance::None` pin so the live catalog page animates, and the
    // warm pass is what keeps the recorded frame settled instead of blank
    // (measured: blank without it). `time_ms: 400` is double the entrance's
    // own `MaterialMotion::SHORT_4` (200 ms); the ramp is a `Duration` drive,
    // which snaps to exactly 1.0 the moment its delta reaches its duration, so
    // the poster is byte-identical to the pinned one it replaces.
    "material/dialog",
    // si-07: the same undo on shadcn's side, and the same non-move —
    // `fade-in-0 zoom-in-95` is 200 ms (`FADE_ZOOM_MS`), captured at 400 ms,
    // byte-identical to the pinned poster it replaces. Dropping the pin also
    // let this case go back to the sugared `dialog()` instead of rebuilding
    // that constructor's chrome by hand. Unlike `material/dialog`, this one
    // changes nothing in a browser: `crate::interactive::shadcn` shadows the
    // slug and a live host resolves that twin ahead of `Case::build`.
    "shadcn/dialog",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn case_at(time_ms: u64) -> Case {
        Case {
            slug: "warm-frames-probe",
            title: "Warm frames probe",
            size: Case::DEFAULT_SIZE,
            scale: Case::DEFAULT_SCALE,
            time_ms,
            design: Design::Base,
            build: || frust_core::any(frust_widgets::text("Hello")),
        }
    }

    #[test]
    fn a_case_at_rest_asks_for_no_warm_pass() {
        assert_eq!(case_at(Case::DEFAULT_TIME_MS).warm_frames(), 0);
    }

    #[test]
    fn a_case_pinning_a_later_point_asks_for_one_warm_pass() {
        assert_eq!(case_at(1).warm_frames(), Case::STAGED_WARM_FRAMES);
        assert_eq!(case_at(400).warm_frames(), Case::STAGED_WARM_FRAMES);
    }

    /// The property that protects the 214 committed posters: warming is
    /// opt-in, and every opt-in is named. This replaced a plain emptiness
    /// assertion once the first two cases opted in; asserting containment
    /// rather than equality lets a case be listed here in the same change
    /// that decides against opting it in.
    #[test]
    fn every_case_that_records_a_warm_pass_is_named_in_the_allowlist() {
        let unnamed: Vec<&str> = crate::cases()
            .iter()
            .filter(|case| case.warm_frames() > 0)
            .map(|case| case.slug)
            .filter(|slug| !super::WARM_PASS_OPT_INS.contains(slug))
            .collect();
        assert!(
            unnamed.is_empty(),
            "these cases now record a warm pass, so their posters have moved \
             and must be named in WARM_PASS_OPT_INS by the change that opted \
             them in: {unnamed:?}"
        );
    }

    /// A slug in the allowlist that matches no case is a typo, and a typo
    /// here silently widens the allowlist rather than narrowing it.
    #[test]
    fn every_allowlisted_slug_exists_in_the_registry() {
        let unknown: Vec<&&str> = super::WARM_PASS_OPT_INS
            .iter()
            .filter(|slug| crate::find(slug).is_none())
            .collect();
        assert!(
            unknown.is_empty(),
            "unknown slugs in WARM_PASS_OPT_INS: {unknown:?}"
        );
    }
}
