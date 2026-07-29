//! [`FrameGate`]: the shared skip-frame decision the mobile shells consult
//! each tick.
//!
//! # What lives here
//!
//! - [`FrameInputs`] — the OR-list of per-frame "something changed" signals a
//!   shell gathers from its own state (see the struct's field docs, each
//!   naming the shell-side source).
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
//! # Animation pacing (frame-gate pacing)
//!
//! On top of the whole-frame skip, a frame whose *only* dirtiness source is a
//! paced ([`frust_core::TickClass::CosmeticLoop`]) frame request — a shimmer,
//! idle pulse, or spinner with no user-visible endpoint — is throttled to the
//! active theme's `MotionScheme::cosmetic_loop_rate` rather than reproduced on
//! every vsync. The shell feeds the paced-only fact through
//! [`FrameInputs::last_needs_frame_paced_only`] and the per-frame clock +
//! interval through [`FramePacing`] to [`FrameGate::decide_paced`]; any
//! input/signal/change-flag/transition dirtiness is **never** paced (it runs
//! immediately, per the default-to-run rule). The `FRUST_NO_FRAME_GATE` kill
//! switch disables pacing too (a disabled gate always runs), and
//! [`NO_ANIM_PACING_VAR`] (`FRUST_NO_ANIM_PACING`) disables *only* the pacing
//! while leaving the skip gate active.

use frust_core::anim::FrameTime;

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

/// The animation-pacing kill-switch variable: when set to any non-`"0"` value,
/// [`FrameGate::new`] disables *only* the paced-loop throttling — the
/// whole-frame skip gate stays active, but every paced ([`CosmeticLoop`])
/// request runs on its vsync as before. Narrower than [`NO_FRAME_GATE_VAR`]
/// (which disables the whole gate), it isolates the pacing behavior for A/B
/// diagnosis. Parsed with the same compile-time-`option_env!` + runtime-env
/// family as [`NO_FRAME_GATE_VAR`] / `FRUST_TRACE`.
///
/// [`CosmeticLoop`]: frust_core::TickClass::CosmeticLoop
pub const NO_ANIM_PACING_VAR: &str = "FRUST_NO_ANIM_PACING";

/// The per-frame pacing context a shell hands to [`FrameGate::decide_paced`]:
/// this tick's frame clock and the active theme's paced-loop interval.
///
/// Both are shell-owned: [`now`](Self::now) is the platform frame clock
/// (Choreographer / `CADisplayLink` timestamp — never a wall clock read inside
/// `frust-core`), and [`interval`](Self::interval) is `1 /
/// MotionScheme::cosmetic_loop_rate` resolved from the *active* theme each
/// frame (so an app that retunes the token via `ThemeBuilder` re-paces live).
#[derive(Debug, Clone, Copy)]
pub struct FramePacing {
    /// This tick's shell frame-clock reading. Only *differences* of two
    /// [`FrameTime`]s from the same shell carry meaning (see [`FrameTime`]).
    pub now: FrameTime,
    /// The minimum interval between two paced ([`CosmeticLoop`]) frame
    /// productions, `1 / cosmetic_loop_rate` from the active theme.
    ///
    /// [`CosmeticLoop`]: frust_core::TickClass::CosmeticLoop
    pub interval: std::time::Duration,
}

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
/// runs the frame if **any** is `true`, with two deliberate deltas noted
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
    /// *last paint's `needs_frame_paced_only`*: the prior frame's frame request
    /// aggregated to [`frust_core::TickClass::CosmeticLoop`] alone — a pacable
    /// decorative loop with no concurrent transition. Latched by the shell from
    /// `PaintOutcome::needs_frame_paced_only`. Meaningful only alongside
    /// [`last_needs_frame`](Self::last_needs_frame); with every *other* input
    /// clear ([`is_paced_only_frame`](Self::is_paced_only_frame)) it is the sole
    /// signal [`FrameGate::decide_paced`] throttles to the theme's cadence. Any
    /// concurrent transition/input clears it, so the frame runs immediately.
    pub last_needs_frame_paced_only: bool,
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
            || self.last_needs_frame_paced_only
            || self.change_flags_pending
            || self.theme_or_appearance_changed
            || self.surface_changed_or_resized
            || self.a11y_action_performed
            || self.resumed_recently
    }

    /// Whether the *only* dirtiness this frame is a paced ([`CosmeticLoop`])
    /// frame request — the throttleable case [`FrameGate::decide_paced`] paces.
    ///
    /// True iff [`last_needs_frame`](Self::last_needs_frame) and
    /// [`last_needs_frame_paced_only`](Self::last_needs_frame_paced_only) are
    /// both set and **every other** OR-list input is clear. Any
    /// input/signal/change-flag/transition alongside it makes this `false`, so
    /// the gate runs the frame immediately rather than pacing it (the
    /// default-to-run rule — see `docs/CODE_STANDARDS.md`'s Frame-Gate
    /// conventions).
    ///
    /// [`CosmeticLoop`]: frust_core::TickClass::CosmeticLoop
    pub fn is_paced_only_frame(&self) -> bool {
        self.last_needs_frame
            && self.last_needs_frame_paced_only
            && !self.signals_dirty
            && !self.events_since_last_frame
            && !self.pointer_capture_active
            && !self.focus_or_ime_active
            && !self.change_flags_pending
            && !self.theme_or_appearance_changed
            && !self.surface_changed_or_resized
            && !self.a11y_action_performed
            && !self.resumed_recently
    }
}

/// The per-shell skip-frame gate: a plain struct — no
/// globals — a shell constructs once and drives each frame via
/// [`decide`](Self::decide).
///
/// Owns two pieces of state: whether the gate is enabled at all (the
/// [`NO_FRAME_GATE_VAR`] kill switch, resolved once at construction) and the
/// resume-warmup countdown ([`WARMUP_FRAMES`], seeded by
/// [`note_resumed`](Self::note_resumed)) — the standalone, host-testable
/// decision type the mobile shells wire in.
#[derive(Debug)]
pub struct FrameGate {
    /// When `false`, [`decide`](Self::decide) always returns
    /// [`FrameDecision::Run`] — the kill switch and [`disabled`](Self::disabled)
    /// path, pre-gate behavior verbatim.
    enabled: bool,
    /// When `false`, [`decide_paced`](Self::decide_paced) never throttles a
    /// paced-only frame (it runs on its vsync as before) — the
    /// [`NO_ANIM_PACING_VAR`] kill switch, resolved once at construction. The
    /// whole-frame skip gate stays active regardless.
    anim_pacing: bool,
    /// Frames left in the resume-warmup window; while `> 0`,
    /// [`decide`](Self::decide) forces a `Run` and decrements it.
    warmup_remaining: u8,
    /// The frame clock reading of the last *produced* frame while pacing, used
    /// to measure the paced-loop interval. `None` until the first paced
    /// decision; re-anchored to `now` on every produced frame (see
    /// [`decide_paced`](Self::decide_paced)).
    last_paced_run: Option<FrameTime>,
}

impl FrameGate {
    /// A gate honoring the [`NO_FRAME_GATE_VAR`] kill switch — what every
    /// shell constructs. When the variable is set (compile-time `--define` or
    /// runtime env, any non-`"0"` value), this is equivalent to
    /// [`disabled`](Self::disabled).
    pub fn new() -> Self {
        Self::with_flags(!kill_switch_engaged(), !anim_pacing_kill_switch_engaged())
    }

    /// A gate that always [`Run`](FrameDecision::Run)s regardless of inputs —
    /// the explicit disabled/kill-switch form (and a test seam bypassing the
    /// env read). Mirrors [`FrameGate::new`]'s behavior when
    /// [`NO_FRAME_GATE_VAR`] is set.
    pub fn disabled() -> Self {
        Self::with_flags(false, false)
    }

    /// Construct with an explicit enabled flag, bypassing the env read — the
    /// test/advanced seam (mirrors [`crate::perf::FrameStats::new_enabled`]).
    /// Animation pacing follows `enabled` (a disabled gate never paces because
    /// it never skips); use [`with_flags`](Self::with_flags) to vary the two
    /// independently.
    pub fn with_enabled(enabled: bool) -> Self {
        Self::with_flags(enabled, enabled)
    }

    /// Construct with explicit `enabled` (whole-frame skip) and `anim_pacing`
    /// (paced-loop throttling) flags, bypassing both env reads — the test seam
    /// for the pacing behavior in isolation.
    pub fn with_flags(enabled: bool, anim_pacing: bool) -> Self {
        Self {
            enabled,
            anim_pacing,
            warmup_remaining: 0,
            last_paced_run: None,
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
    /// otherwise [`FrameDecision::Skip`]. See [`FrameInputs`]'s docs for what
    /// each input signal means.
    ///
    /// Takes `&mut self` because it advances the resume-warmup countdown.
    ///
    /// This is the non-paced entry: a paced-only frame runs on every tick (no
    /// throttling), the conservative pre-pacing behavior. Use
    /// [`decide_paced`](Self::decide_paced) to honor the theme's cosmetic-loop
    /// cadence.
    pub fn decide(&mut self, inputs: FrameInputs) -> FrameDecision {
        self.decide_impl(inputs, None)
    }

    /// Decide whether this frame runs, honoring animation pacing.
    ///
    /// Identical to [`decide`](Self::decide) except that when the *only*
    /// dirtiness is a paced ([`CosmeticLoop`]) frame request
    /// ([`FrameInputs::is_paced_only_frame`]) and pacing is enabled, the frame
    /// is throttled to `pacing.interval`: it runs only once
    /// `pacing.now - last_paced_run >= interval`, otherwise [`Skip`]s. A skip
    /// leaves `last_needs_frame` alive (the shell doesn't repaint, so it never
    /// re-latches), so the gate keeps waking and never starves the loop; the
    /// interval is re-anchored to the clock of every *produced* frame (whatever
    /// its cause), so a transition frame mid-loop resets the cadence.
    ///
    /// Any non-paced input (an event, a signal write, a transition request,
    /// pending change flags, …) makes [`is_paced_only_frame`] `false`, so the
    /// frame runs immediately — pacing never delays real work.
    ///
    /// [`CosmeticLoop`]: frust_core::TickClass::CosmeticLoop
    /// [`Skip`]: FrameDecision::Skip
    /// [`is_paced_only_frame`]: FrameInputs::is_paced_only_frame
    pub fn decide_paced(&mut self, inputs: FrameInputs, pacing: FramePacing) -> FrameDecision {
        self.decide_impl(inputs, Some(pacing))
    }

    /// The shared decision body behind [`decide`](Self::decide) (no pacing) and
    /// [`decide_paced`](Self::decide_paced) (pacing context supplied).
    fn decide_impl(&mut self, inputs: FrameInputs, pacing: Option<FramePacing>) -> FrameDecision {
        if !self.enabled {
            return FrameDecision::Run;
        }
        // Resume warmup: force a Run for the first WARMUP_FRAMES after a
        // note_resumed(), independent of the FrameInputs the shell gathered.
        let warming = self.warmup_remaining > 0;
        if warming {
            self.warmup_remaining -= 1;
        }

        // A paced-only frame (and pacing enabled, and not warming) is the sole
        // throttleable case; every other Run resets the pace cadence to its
        // own clock (see the anchor calls below).
        let paced_case =
            !warming && self.anim_pacing && pacing.is_some() && inputs.is_paced_only_frame();

        if warming {
            self.anchor_non_paced(pacing);
            return FrameDecision::Run;
        }

        if paced_case {
            let p = pacing.expect("paced_case implies pacing.is_some()");
            return match self.last_paced_run {
                // Inside the interval since the last produced frame: throttle.
                // The shell leaves `last_needs_frame` set across a skip (no
                // repaint re-latches it), so the gate keeps waking and never
                // starves the loop.
                Some(last) if p.now.saturating_sub(last) < p.interval => FrameDecision::Skip,
                _ => {
                    self.anchor_paced(p);
                    FrameDecision::Run
                }
            };
        }

        if inputs.any_set() {
            // A non-paced Run (event/signal/transition/change-flag): reset the
            // pace cadence to this real frame so a paced frame never fires
            // immediately after one.
            self.anchor_non_paced(pacing);
            FrameDecision::Run
        } else {
            FrameDecision::Skip
        }
    }

    /// Anchor the pace clock to this frame's exact clock — used for every
    /// *non-paced* produced frame (warmup / event / transition), so the loop's
    /// next interval is measured from the most recent real frame.
    fn anchor_non_paced(&mut self, pacing: Option<FramePacing>) {
        if let Some(p) = pacing {
            self.last_paced_run = Some(p.now);
        }
    }

    /// Anchor the pace clock for a *paced* fire: advance by exactly one interval
    /// to hold a drift-free cadence on the discrete vsync grid (anchoring to the
    /// raw `now`, which lands up to a tick past the ideal fire time, would drift
    /// the effective rate below the cap). After a long stall (≥ two intervals —
    /// a paused/resumed loop) reset to `now` instead, so the loop resumes at
    /// cadence rather than firing a catch-up burst.
    fn anchor_paced(&mut self, p: FramePacing) {
        let next = match self.last_paced_run {
            Some(last) if p.now.saturating_sub(last) < p.interval * 2 => {
                FrameTime::from_nanos(last.as_nanos().saturating_add(p.interval.as_nanos() as u64))
            }
            _ => p.now,
        };
        self.last_paced_run = Some(next);
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

/// Reads the [`NO_ANIM_PACING_VAR`] kill switch from the compile-time define and
/// the process environment, the same `option_env!` + runtime-env shape as
/// [`kill_switch_engaged`]. Public so a non-`FrameGate` consumer (the desktop
/// shell, which paces via a delayed redraw rather than a skip gate) can honor
/// the same switch. Either source set to a non-`"0"` value engages it.
pub fn anim_pacing_kill_switch_engaged() -> bool {
    kill_switch(
        option_env!("FRUST_NO_ANIM_PACING"),
        std::env::var(NO_ANIM_PACING_VAR).ok().as_deref(),
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
            "last_needs_frame_paced_only" => i.last_needs_frame_paced_only = true,
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
        "last_needs_frame_paced_only",
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
            11,
            "the OR-list must have all ten RESEARCH §C-derived inputs plus the \
             paced-only signal"
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

    // -----------------------------------------------------------------
    // decide_paced: animation pacing
    // -----------------------------------------------------------------

    use std::time::Duration;

    /// One vsync step of a `hz`-Hz refresh, in nanoseconds.
    fn step_nanos(hz: f64) -> u64 {
        (1_000_000_000.0 / hz) as u64
    }

    /// The paced-only input: a prior paced (CosmeticLoop) frame request with
    /// every other OR-list signal clear — the sole case pacing throttles.
    fn paced_only() -> FrameInputs {
        FrameInputs {
            last_needs_frame: true,
            last_needs_frame_paced_only: true,
            ..FrameInputs::default()
        }
    }

    /// A 30Hz cosmetic-loop interval, the framework default.
    fn interval_30hz() -> Duration {
        Duration::from_secs_f64(1.0 / 30.0)
    }

    #[test]
    fn is_paced_only_frame_requires_last_needs_frame_and_no_other_input() {
        assert!(paced_only().is_paced_only_frame());
        // paced flag without last_needs_frame is not a paced-only frame.
        let only_flag = FrameInputs {
            last_needs_frame_paced_only: true,
            ..FrameInputs::default()
        };
        assert!(!only_flag.is_paced_only_frame());
        // any concurrent input disqualifies pacing.
        let mut with_event = paced_only();
        with_event.events_since_last_frame = true;
        assert!(!with_event.is_paced_only_frame());
    }

    #[test]
    fn paced_only_stream_runs_at_the_cap_not_every_vsync() {
        // A 120Hz tick stream feeding paced-only inputs, capped at 30Hz, must
        // produce ~30 runs per simulated second (one every ~4 ticks).
        let mut gate = FrameGate::with_flags(true, true);
        let tick = step_nanos(120.0);
        let interval = interval_30hz();

        let mut runs = 0usize;
        // 120 ticks == 1 simulated second.
        for i in 0..120u64 {
            let pacing = FramePacing {
                now: FrameTime::from_nanos(i * tick),
                interval,
            };
            if gate.decide_paced(paced_only(), pacing).is_run() {
                runs += 1;
            }
        }
        // 30Hz cap over 1s ⇒ ~30 runs. Allow ±2 for boundary rounding of the
        // 120→30 tick ratio.
        assert!(
            (28..=32).contains(&runs),
            "paced-only 120Hz stream should run ~30x/s, ran {runs}"
        );
    }

    #[test]
    fn transition_input_mid_interval_runs_immediately() {
        // Pace a loop, then a transition (paced_only == false) arrives inside
        // the interval — it must run immediately, never wait for the cap.
        let mut gate = FrameGate::with_flags(true, true);
        let interval = interval_30hz();

        // First paced frame runs and anchors the pace clock at t=0.
        let run0 = gate.decide_paced(
            paced_only(),
            FramePacing {
                now: FrameTime::from_nanos(0),
                interval,
            },
        );
        assert!(run0.is_run());
        // A tick well inside the 33ms interval, but carrying a transition
        // request (paced-only flag cleared) — runs immediately.
        let mut transition = FrameInputs {
            last_needs_frame: true,
            last_needs_frame_paced_only: false,
            ..FrameInputs::default()
        };
        // sanity: this is NOT a paced-only frame
        assert!(!transition.is_paced_only_frame());
        let decision = gate.decide_paced(
            transition,
            FramePacing {
                now: FrameTime::from_nanos(step_nanos(120.0)),
                interval,
            },
        );
        assert_eq!(
            decision,
            FrameDecision::Run,
            "a transition mid-interval must run immediately, not pace"
        );
        // A plain event mid-interval likewise runs immediately.
        transition = FrameInputs {
            events_since_last_frame: true,
            ..FrameInputs::default()
        };
        assert_eq!(
            gate.decide_paced(
                transition,
                FramePacing {
                    now: FrameTime::from_nanos(2 * step_nanos(120.0)),
                    interval,
                },
            ),
            FrameDecision::Run,
            "an event mid-interval must run immediately"
        );
    }

    #[test]
    fn paced_skip_leaves_the_request_alive_and_never_starves() {
        // A long paced-only stream must keep producing frames at the cap — it
        // never stops running entirely (starvation guard).
        let mut gate = FrameGate::with_flags(true, true);
        let tick = step_nanos(120.0);
        let interval = interval_30hz();

        let mut last_run_tick: Option<u64> = None;
        let mut max_gap = 0u64;
        let mut total_runs = 0usize;
        for i in 0..600u64 {
            // 5 simulated seconds
            let pacing = FramePacing {
                now: FrameTime::from_nanos(i * tick),
                interval,
            };
            if gate.decide_paced(paced_only(), pacing).is_run() {
                total_runs += 1;
                if let Some(prev) = last_run_tick {
                    max_gap = max_gap.max(i - prev);
                }
                last_run_tick = Some(i);
            }
        }
        assert!(
            total_runs > 0,
            "paced stream must keep running (no starvation)"
        );
        // The gap between runs stays bounded near the 4-tick cap ratio — never
        // an unbounded stall.
        assert!(
            max_gap <= 5,
            "paced runs must stay periodic; observed max gap of {max_gap} ticks"
        );
    }

    #[test]
    fn anim_pacing_kill_switch_runs_every_tick() {
        // Gate enabled (skips still work) but pacing disabled: a paced-only
        // stream runs every vsync, pre-pacing behavior.
        let mut gate = FrameGate::with_flags(true, false);
        let tick = step_nanos(120.0);
        let interval = interval_30hz();
        for i in 0..120u64 {
            let pacing = FramePacing {
                now: FrameTime::from_nanos(i * tick),
                interval,
            };
            assert_eq!(
                gate.decide_paced(paced_only(), pacing),
                FrameDecision::Run,
                "with pacing disabled every paced tick must run"
            );
        }
    }

    #[test]
    fn disabled_gate_ignores_pacing() {
        // FRUST_NO_FRAME_GATE (disabled gate) forces Run regardless of pacing.
        let mut gate = FrameGate::disabled();
        for i in 0..10u64 {
            let pacing = FramePacing {
                now: FrameTime::from_nanos(i * step_nanos(120.0)),
                interval: interval_30hz(),
            };
            assert_eq!(gate.decide_paced(paced_only(), pacing), FrameDecision::Run);
        }
    }

    #[test]
    fn non_paced_decide_runs_paced_only_every_tick() {
        // The non-pacing `decide` entry never throttles: a paced-only frame is
        // just another `any_set` Run (conservative pre-pacing behavior).
        let mut gate = FrameGate::with_flags(true, true);
        for _ in 0..10 {
            assert_eq!(gate.decide(paced_only()), FrameDecision::Run);
        }
    }
}
