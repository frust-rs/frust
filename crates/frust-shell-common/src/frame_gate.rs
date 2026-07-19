//! [`FrameGate`]: the shared skip-frame decision the mobile shells consult
//! each tick (spec §14 phase 7).
//!
//! # What lives here
//!
//! - [`FrameInputs`] — the OR-list of per-frame "something changed" signals a
//!   shell gathers from its own state (see the struct's field docs, each
//!   naming the shell-side source and its `research/RESEARCH.md` §C entry).
//! - [`FrameGate`] — a plain struct owning the resume-warmup counter and the
//!   kill-switch flag; [`FrameGate::decide`] folds [`FrameInputs`] plus the
//!   warmup into one [`FrameDecision`] (`Run`/`Skip`).
//!
//! # Layering choice
//!
//! Like [`crate::perf`], this is shell-owned by design and lives in
//! `frust-shell-common`, not `frust-core`. The gate takes no platform
//! dependency, no `unsafe`, and — critically — **no `frust-reactive`
//! dependency**: the reactive "did any tracked signal change" answer arrives
//! as the plain [`FrameInputs::signals_dirty`] bool, which the shells fill
//! themselves by calling `frust_reactive::ReactiveRuntime::take_signals_dirty`
//! (task 07) after their per-frame `pump_local` (that crate's documented
//! pump-first ordering contract). This keeps shell-common's compiles-everywhere,
//! reactive-free charter intact (see `docs/ARCHITECTURE.md`'s Layer
//! Dependencies).
//!
//! # The layout-skip seam
//!
//! [`FrameGate`] decides whether a whole frame runs at all. Within a frame it
//! *does* run, whether the layout pass can be skipped is a second, finer gate
//! the shell drives off `RenderRoot::take_change_flags` (exposed through
//! [`crate::AppTree::take_change_flags`]): run `rebuild` → run `layout` iff the
//! drained `ChangeFlags` need layout (or it's the first frame, or the surface
//! resized) → always `paint`. The `set_theme ⇒ LAYOUT|PAINT` contract
//! (`docs/ARCHITECTURE.md`'s Theme delivery) is the correctness anchor there:
//! `Text` bakes its themed glyph color at layout time, so a bare theme swap
//! must force relayout even though no view changed — which
//! `RenderRoot::set_theme` guarantees by marking `LAYOUT` pending. Encode and
//! present are frame-level only: a frame the gate runs always presents.
//!
//! # Wiring is a later task
//!
//! This module ships the decision type + switch only; no shell constructs or
//! feeds a [`FrameGate`] yet (tasks 17/18 wire the Android/iOS shells).

/// The resume-warmup window length: after a [`FrameGate::note_resumed`] the
/// next `WARMUP_FRAMES` [`decide`](FrameGate::decide) calls force a `Run`
/// regardless of [`FrameInputs`].
///
/// Three frames is a deliberately small, fixed cushion: a
/// resume/surface-recreate can leave the first tick's change signals not yet
/// observable (a surface just became ready, the appearance poll hasn't run,
/// the first post-resume event hasn't arrived), and painting a couple of
/// extra frames on resume is far cheaper than showing a stale or blank one.
/// It is not a published platform constant — it is a Frust tuning choice
/// (the "correctness beats savings" default: when in doubt, run).
pub const WARMUP_FRAMES: u8 = 3;

/// The kill-switch environment/compile-time variable: when set to any
/// non-`"0"` value, [`FrameGate::new`] yields a gate that always [`Run`]s,
/// matching pre-gate behavior verbatim.
///
/// [`Run`]: FrameDecision::Run
pub const NO_FRAME_GATE_VAR: &str = "FRUST_NO_FRAME_GATE";

/// The outcome of [`FrameGate::decide`]: whether the shell should run this
/// frame's rebuild/layout/paint/encode/present passes, or skip them entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDecision {
    /// Run the frame: at least one input signalled a change (or the gate is
    /// in its resume warmup, or the gate is disabled).
    Run,
    /// Skip the frame: nothing changed. The continuous Choreographer/
    /// `CADisplayLink` loop keeps re-posting callbacks — only frame
    /// *production* stops (see the module docs).
    Skip,
}

impl FrameDecision {
    /// Whether this decision is [`FrameDecision::Run`].
    pub fn is_run(self) -> bool {
        matches!(self, FrameDecision::Run)
    }

    /// Whether this decision is [`FrameDecision::Skip`].
    pub fn is_skip(self) -> bool {
        matches!(self, FrameDecision::Skip)
    }
}

/// The per-frame OR-list a shell gathers and hands to [`FrameGate::decide`].
///
/// Every field is a "something that needs this frame to run" signal; the gate
/// runs the frame if **any** is `true`. The list matches
/// `research/RESEARCH.md` §C item-for-item, with two deliberate deltas noted
/// in the field docs:
///
/// - **Semantics adapter needs** (§C's last item) is *not* a field here:
///   semantics-publish gating stays shell-side (Android is already
///   generation-gated via `AppTree::semantics_if_changed`; iOS is handled in
///   task 18), so it never gates whole-frame production.
/// - **[`resumed_recently`](Self::resumed_recently)** is the added input for
///   the resume warmup (see [`WARMUP_FRAMES`]); the gate also drives this same
///   condition internally via [`FrameGate::note_resumed`], so a shell may
///   leave the field `false` and rely on the counter (both force a `Run`).
///
/// `Default` is all-`false` (the "nothing changed" baseline a
/// [`FrameGate::decide`] turns into a [`FrameDecision::Skip`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameInputs {
    /// §C: *tracked-signal dirty*. A tracked signal changed since the last
    /// frame. Source: `frust_reactive::ReactiveRuntime::take_signals_dirty`
    /// (task 07), read by the shell *after* its per-frame `pump_local` per
    /// that crate's pump-first ordering contract.
    pub signals_dirty: bool,
    /// §C: *events dispatched since last frame*. A pointer/scroll/key/IME
    /// event reached `RenderRoot::event` between frames. Source: a shell-side
    /// latch set by the `nativeOnTouch`/IME entry points and cleared each
    /// frame (see task 17).
    pub events_since_last_frame: bool,
    /// §C: *active pointer capture*. A gesture is mid-drag and the captured
    /// widget may animate/track the pointer. Source:
    /// `AppTree`/`RenderRoot::is_pointer_captured`.
    pub pointer_capture_active: bool,
    /// §C: *focus/IME active surface*. Something holds keyboard/IME focus, so
    /// caret/selection chrome may need repainting. Source:
    /// `RenderRoot::is_focus_active` (and/or a published `ime_state`).
    pub focus_or_ime_active: bool,
    /// §C: *last paint's `needs_frame`*. The previous paint advanced an
    /// animation/transition and asked for another frame. Source:
    /// `PaintOutcome::needs_frame`, latched by the shell from the prior
    /// frame's `AppTree::paint` return.
    pub last_needs_frame: bool,
    /// §C: *pending `ChangeFlags`*. A rebuild (or `set_theme`) left layout/
    /// paint dirtiness undrained. Source:
    /// [`AppTree::has_pending_change_flags`](crate::AppTree::has_pending_change_flags)
    /// (a non-draining peek, so a skipped frame preserves the flags).
    pub change_flags_pending: bool,
    /// §C: *theme-override/appearance change*. The app-facing theme override
    /// or the platform light/dark preference changed this tick. Source: the
    /// shell's per-frame `ThemeOverrideWatcher`/appearance poll (see
    /// [`crate::theme_override`]).
    pub theme_or_appearance_changed: bool,
    /// §C: *surface resize/recreation*. The GPU surface was created, resized,
    /// or recreated (rotation/backgrounding). Source: the shell's
    /// `nativeOnSurfaceChanged`/`frust_resize` path.
    pub surface_changed_or_resized: bool,
    /// §C: *accessibility actions*. A platform `accesskit_*` action was
    /// performed this tick, mutating state. Source: the shell's a11y-action
    /// drain feeding `AppTree::perform_accessibility_action`.
    pub a11y_action_performed: bool,
    /// Added input: *resume warmup*. The app resumed / the surface was
    /// (re)created within the last [`WARMUP_FRAMES`] frames. A shell may set
    /// this explicitly, or leave it `false` and let [`FrameGate::note_resumed`]
    /// drive the same condition through the gate's own countdown — both force
    /// a `Run`.
    pub resumed_recently: bool,
}

impl FrameInputs {
    /// Whether any input signals that this frame must run. The gate ORs the
    /// full field set — the single decision rule the whole type exists to
    /// feed (see [`FrameGate::decide`]).
    pub fn any_set(&self) -> bool {
        self.signals_dirty
            || self.events_since_last_frame
            || self.pointer_capture_active
            || self.focus_or_ime_active
            || self.last_needs_frame
            || self.change_flags_pending
            || self.theme_or_appearance_changed
            || self.surface_changed_or_resized
            || self.a11y_action_performed
            || self.resumed_recently
    }
}

/// The per-shell skip-frame gate (spec §14 phase 7): a plain struct — no
/// globals — a shell constructs once and drives each frame via
/// [`decide`](Self::decide).
///
/// Owns two pieces of state: whether the gate is enabled at all (the
/// [`NO_FRAME_GATE_VAR`] kill switch, resolved once at construction) and the
/// resume-warmup countdown ([`WARMUP_FRAMES`], seeded by
/// [`note_resumed`](Self::note_resumed)). No shell constructs one yet — this
/// is the standalone, host-testable decision type tasks 17/18 wire in.
#[derive(Debug)]
pub struct FrameGate {
    /// When `false`, [`decide`](Self::decide) always returns
    /// [`FrameDecision::Run`] — the kill switch and [`disabled`](Self::disabled)
    /// path, pre-gate behavior verbatim.
    enabled: bool,
    /// Frames left in the resume-warmup window; while `> 0`,
    /// [`decide`](Self::decide) forces a `Run` and decrements it.
    warmup_remaining: u8,
}

impl FrameGate {
    /// A gate honoring the [`NO_FRAME_GATE_VAR`] kill switch — what every
    /// shell constructs. When the variable is set (compile-time `--define` or
    /// runtime env, any non-`"0"` value), this is equivalent to
    /// [`disabled`](Self::disabled).
    pub fn new() -> Self {
        Self::with_enabled(!kill_switch_engaged())
    }

    /// A gate that always [`Run`](FrameDecision::Run)s regardless of inputs —
    /// the explicit disabled/kill-switch form (and a test seam bypassing the
    /// env read). Mirrors [`FrameGate::new`]'s behavior when
    /// [`NO_FRAME_GATE_VAR`] is set.
    pub fn disabled() -> Self {
        Self::with_enabled(false)
    }

    /// Construct with an explicit enabled flag, bypassing the env read — the
    /// test/advanced seam (mirrors [`crate::perf::FrameStats::new_enabled`]).
    pub fn with_enabled(enabled: bool) -> Self {
        Self {
            enabled,
            warmup_remaining: 0,
        }
    }

    /// Whether the gate is active (can ever return [`FrameDecision::Skip`]).
    /// `false` for a [`disabled`](Self::disabled) gate or when the kill switch
    /// is engaged.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Open the resume-warmup window: the next [`WARMUP_FRAMES`]
    /// [`decide`](Self::decide) calls force a [`FrameDecision::Run`].
    ///
    /// A shell calls this on resume and on surface (re)creation, where the
    /// first tick's change signals may not yet be observable (see
    /// [`WARMUP_FRAMES`]).
    pub fn note_resumed(&mut self) {
        self.warmup_remaining = WARMUP_FRAMES;
    }

    /// Frames left in the resume-warmup window (`0` when not warming up).
    /// Exposed for tests/diagnostics.
    pub fn warmup_remaining(&self) -> u8 {
        self.warmup_remaining
    }

    /// Decide whether this frame runs.
    ///
    /// Returns [`FrameDecision::Run`] when **any** of:
    /// - the gate is disabled (kill switch / [`disabled`](Self::disabled)),
    /// - the resume warmup is active (decrementing it by one), or
    /// - any [`FrameInputs`] field is set ([`FrameInputs::any_set`]) —
    ///
    /// otherwise [`FrameDecision::Skip`]. Every input maps to a
    /// `research/RESEARCH.md` §C OR-list entry (see [`FrameInputs`]'s docs).
    ///
    /// Takes `&mut self` because it advances the resume-warmup countdown.
    pub fn decide(&mut self, inputs: FrameInputs) -> FrameDecision {
        if !self.enabled {
            return FrameDecision::Run;
        }
        // Resume warmup: force a Run for the first WARMUP_FRAMES after a
        // note_resumed(), independent of the FrameInputs the shell gathered.
        let warming = self.warmup_remaining > 0;
        if warming {
            self.warmup_remaining -= 1;
        }
        if warming || inputs.any_set() {
            FrameDecision::Run
        } else {
            FrameDecision::Skip
        }
    }
}

impl Default for FrameGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads the [`NO_FRAME_GATE_VAR`] kill switch from the compile-time define
/// and the process environment, mirroring [`crate::perf::enabled`]'s
/// `option_env!` + runtime-env pattern: either source set to a non-`"0"`
/// value engages the switch.
fn kill_switch_engaged() -> bool {
    kill_switch(
        option_env!("FRUST_NO_FRAME_GATE"),
        std::env::var(NO_FRAME_GATE_VAR).ok().as_deref(),
    )
}

/// The pure decision [`kill_switch_engaged`] wraps: a non-empty, non-`"0"`
/// value from either the compile-time or runtime source engages the switch.
/// Split out so it is directly unit-testable without touching the process
/// environment (see [`crate::perf`]'s `trace_switch`).
fn kill_switch(compile_time: Option<&str>, runtime: Option<&str>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(compile_time) || is_set_non_zero(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `FrameInputs` with exactly one field set, by name — the driver for
    /// the exhaustive per-input table test below.
    fn only(field: &str) -> FrameInputs {
        let mut i = FrameInputs::default();
        match field {
            "signals_dirty" => i.signals_dirty = true,
            "events_since_last_frame" => i.events_since_last_frame = true,
            "pointer_capture_active" => i.pointer_capture_active = true,
            "focus_or_ime_active" => i.focus_or_ime_active = true,
            "last_needs_frame" => i.last_needs_frame = true,
            "change_flags_pending" => i.change_flags_pending = true,
            "theme_or_appearance_changed" => i.theme_or_appearance_changed = true,
            "surface_changed_or_resized" => i.surface_changed_or_resized = true,
            "a11y_action_performed" => i.a11y_action_performed = true,
            "resumed_recently" => i.resumed_recently = true,
            other => panic!("unknown FrameInputs field {other:?}"),
        }
        i
    }

    /// Every input field, so the table test below is exhaustive by
    /// construction — adding a field without listing it here fails the count
    /// assertion.
    const ALL_INPUTS: &[&str] = &[
        "signals_dirty",
        "events_since_last_frame",
        "pointer_capture_active",
        "focus_or_ime_active",
        "last_needs_frame",
        "change_flags_pending",
        "theme_or_appearance_changed",
        "surface_changed_or_resized",
        "a11y_action_performed",
        "resumed_recently",
    ];

    // -----------------------------------------------------------------
    // kill_switch (pure — the env-reading wrapper's cache-free counterpart)
    // -----------------------------------------------------------------

    #[test]
    fn kill_switch_off_when_neither_set() {
        assert!(!kill_switch(None, None));
    }

    #[test]
    fn kill_switch_on_when_compile_time_set_non_zero() {
        assert!(kill_switch(Some("1"), None));
    }

    #[test]
    fn kill_switch_on_when_runtime_set_non_zero() {
        assert!(kill_switch(None, Some("1")));
    }

    #[test]
    fn kill_switch_off_when_either_is_literal_zero_and_other_unset() {
        assert!(!kill_switch(Some("0"), None));
        assert!(!kill_switch(None, Some("0")));
    }

    #[test]
    fn kill_switch_on_when_either_source_wins() {
        assert!(kill_switch(Some("0"), Some("1")));
        assert!(kill_switch(Some("1"), Some("0")));
    }

    // -----------------------------------------------------------------
    // decide: the OR-list table
    // -----------------------------------------------------------------

    #[test]
    fn each_single_input_forces_run() {
        // Exhaustive: every field, set alone on a fresh (non-warming) gate,
        // must force a Run.
        assert_eq!(
            ALL_INPUTS.len(),
            10,
            "the OR-list must have all ten RESEARCH §C-derived inputs"
        );
        for field in ALL_INPUTS {
            let mut gate = FrameGate::with_enabled(true);
            let decision = gate.decide(only(field));
            assert_eq!(
                decision,
                FrameDecision::Run,
                "input {field:?} alone must force a Run"
            );
        }
    }

    #[test]
    fn all_false_inputs_skip() {
        let mut gate = FrameGate::with_enabled(true);
        assert_eq!(
            gate.decide(FrameInputs::default()),
            FrameDecision::Skip,
            "no input set (and no warmup) must skip"
        );
    }

    #[test]
    fn any_set_matches_decide_for_all_false() {
        let inputs = FrameInputs::default();
        assert!(!inputs.any_set());
    }

    // -----------------------------------------------------------------
    // decide: resume warmup countdown
    // -----------------------------------------------------------------

    #[test]
    fn warmup_forces_run_for_n_frames_then_skips() {
        let mut gate = FrameGate::with_enabled(true);
        gate.note_resumed();
        assert_eq!(gate.warmup_remaining(), WARMUP_FRAMES);

        // All-false inputs: only the warmup keeps these frames running.
        for frame in 0..WARMUP_FRAMES {
            assert_eq!(
                gate.decide(FrameInputs::default()),
                FrameDecision::Run,
                "warmup frame {frame} must run despite no inputs"
            );
        }
        // Warmup exhausted: the next all-false frame skips.
        assert_eq!(gate.warmup_remaining(), 0);
        assert_eq!(
            gate.decide(FrameInputs::default()),
            FrameDecision::Skip,
            "after the warmup window, an all-false frame skips again"
        );
    }

    #[test]
    fn note_resumed_reopens_the_warmup_window() {
        let mut gate = FrameGate::with_enabled(true);
        gate.note_resumed();
        for _ in 0..WARMUP_FRAMES {
            gate.decide(FrameInputs::default());
        }
        assert_eq!(gate.warmup_remaining(), 0);
        // A second resume (e.g. surface recreated after backgrounding) reopens
        // the window.
        gate.note_resumed();
        assert_eq!(
            gate.decide(FrameInputs::default()),
            FrameDecision::Run,
            "a fresh note_resumed reopens the warmup window"
        );
    }

    // -----------------------------------------------------------------
    // Kill switch / disabled
    // -----------------------------------------------------------------

    #[test]
    fn disabled_gate_always_runs() {
        let mut gate = FrameGate::disabled();
        assert!(!gate.is_enabled());
        // All-false inputs, no warmup: a disabled gate still runs.
        assert_eq!(gate.decide(FrameInputs::default()), FrameDecision::Run);
        // And keeps running frame after frame.
        assert_eq!(gate.decide(FrameInputs::default()), FrameDecision::Run);
    }

    #[test]
    fn enabled_gate_can_skip() {
        let mut gate = FrameGate::with_enabled(true);
        assert!(gate.is_enabled());
        assert_eq!(gate.decide(FrameInputs::default()), FrameDecision::Skip);
    }

    #[test]
    fn frame_decision_predicates() {
        assert!(FrameDecision::Run.is_run());
        assert!(!FrameDecision::Run.is_skip());
        assert!(FrameDecision::Skip.is_skip());
        assert!(!FrameDecision::Skip.is_run());
    }
}
