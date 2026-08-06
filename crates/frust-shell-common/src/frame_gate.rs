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
//! after their per-frame `pump_local` (that crate's documented
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
//! every vsync. A paced request may also name its *own*, slower cadence
//! (`PaintCtx::request_frame_paced_at` — a ~500ms caret blink against a 30Hz
//! shimmer cap); the paint pass folds every such request onto a MIN-lattice and
//! the gate resolves the result against the theme cap
//! ([`FramePacing::effective_interval`]). The shell feeds the paced-only fact
//! through [`FrameInputs::last_needs_frame_paced_only`] and the per-frame clock +
//! interval pair through [`FramePacing`] to [`FrameGate::decide_paced`]; any
//! input/signal/change-flag/transition dirtiness is **never** paced (it runs
//! immediately, per the default-to-run rule) — with one deliberate exception,
//! the [`FrameInputs::focus_or_ime_changed`] edge (that field's doc has the
//! reasoning: a blinking caret in a focused field is exactly the loop this gate
//! must be able to throttle). Because that edge *can* ride inside a paced
//! decision, the shells **peek** it rather than drain it: they compare the live
//! generation against their cache every tick but commit the cache only once the
//! gate has decided to `Run`, so an edge landing on a skipped tick is deferred
//! by the pacing — bounded by one cap interval — and never erased. Every other
//! drained-on-gather latch is an [`FrameInputs::is_paced_only_frame`]
//! disqualifier and so can never be true on a tick the gate skips. The
//! `FRUST_NO_FRAME_GATE` kill
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
/// this tick's frame clock, the active theme's paced-loop cap, and whatever
/// interval the previous paint's paced request asked for.
///
/// All three are shell-owned: [`now`](Self::now) is the platform frame clock
/// (Choreographer / `CADisplayLink` timestamp — never a wall clock read inside
/// `frust-core`), [`interval`](Self::interval) is `1 /
/// MotionScheme::cosmetic_loop_rate` resolved from the *active* theme each
/// frame (so an app that retunes the token via `ThemeBuilder` re-paces live),
/// and [`requested_interval`](Self::requested_interval) is the previous paint's
/// latched `PaintOutcome::paced_interval`. [`effective_interval`](Self::effective_interval)
/// folds the last two into the one interval this tick actually paces at.
#[derive(Debug, Clone, Copy)]
pub struct FramePacing {
    /// This tick's shell frame-clock reading. Only *differences* of two
    /// [`FrameTime`]s from the same shell carry meaning (see [`FrameTime`]).
    pub now: FrameTime,
    /// The theme's paced-loop **cap**: `1 / cosmetic_loop_rate` from the active
    /// theme, and so the shortest interval any paced ([`CosmeticLoop`]) frame
    /// may be produced at. `CosmeticLoopRate` clamps its rate up to
    /// `FLOOR_HZ` (10Hz), so this is never longer than 100ms.
    ///
    /// [`CosmeticLoop`]: frust_core::TickClass::CosmeticLoop
    pub interval: std::time::Duration,
    /// The tightest interval the previous paint's paced requests named
    /// (`frust_core::PaintOutcome::paced_interval`, latched by the shell beside
    /// [`FrameInputs::last_needs_frame_paced_only`]), or `None` when that paint
    /// named none.
    ///
    /// `None` and `Some(Duration::ZERO)` both mean "at the theme's own rate" —
    /// a bare `PaintCtx::request_frame_paced` folds `ZERO` into the core-side
    /// MIN-lattice — so both resolve to [`interval`](Self::interval). A longer
    /// value (a ~500ms caret blink against a 30Hz shimmer cap) paces that loop
    /// slower than the theme's own cadence; see
    /// [`effective_interval`](Self::effective_interval).
    pub requested_interval: Option<std::time::Duration>,
}

impl FramePacing {
    /// The interval this tick actually paces at: the **longer** of the theme's
    /// cap ([`interval`](Self::interval)) and the previous paint's requested
    /// interval ([`requested_interval`](Self::requested_interval)).
    ///
    /// The two-sided contract behind that `max` (the core-side half lives on
    /// `PaintCtx::request_frame_paced_at`):
    ///
    /// - **The MIN fold already happened in core.** Every paced request in a
    ///   paint pass folds to the tightest interval there, so this sees one
    ///   value: the fastest cadence anything onscreen asked for. A 30Hz shimmer
    ///   beside a 2Hz caret arrives here as `ZERO` (the shimmer's bare request)
    ///   and paces at 30Hz — the caret is simply repainted more often than it
    ///   needs, which is invisible and costs no frame the shimmer wasn't
    ///   already forcing. A slow request can never starve a fast one.
    /// - **The theme rate is a ceiling.** `cosmetic_loop_rate` caps decorative
    ///   motion for battery's sake, so a request *tighter* than the cap is
    ///   clamped up to it rather than honored. Motion that must land every
    ///   vsync is not cosmetic — it belongs to `TickClass::Transition`, which
    ///   is never paced at all.
    pub fn effective_interval(&self) -> std::time::Duration {
        match self.requested_interval {
            Some(requested) => self.interval.max(requested),
            None => self.interval,
        }
    }
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
/// runs the frame if **any** is `true`, with three deliberate deltas noted
/// in the field docs:
///
/// - **[`focus_or_ime_changed`](Self::focus_or_ime_changed)** is an *edge*, not
///   a level: it reports that the focus/IME session moved since the shell last
///   looked, so a steady focus session no longer forces a frame every vsync
///   (and, alone among the wake inputs, it does not disqualify pacing).
///
/// - **Semantics adapter needs** is *not* a field here:
///   semantics-publish gating stays shell-side (both Android and iOS are
///   generation-gated via `AppTree::semantics_if_changed`), so it never gates
///   whole-frame production.
/// - **[`resumed_recently`](Self::resumed_recently)** is the added input for
///   the resume warmup (see [`WARMUP_FRAMES`]); the gate also drives this same
///   condition internally via [`FrameGate::note_resumed`], so a shell may
///   leave the field `false` and rely on the counter (both force a `Run`).
///
/// `Default` is all-`false` (the "nothing changed" baseline a
/// [`FrameGate::decide`] turns into a [`FrameDecision::Skip`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameInputs {
    /// *tracked-signal dirty*. A tracked signal changed since the last
    /// frame. Source: `frust_reactive::ReactiveRuntime::take_signals_dirty`,
    /// read by the shell *after* its per-frame `pump_local` per
    /// that crate's pump-first ordering contract.
    pub signals_dirty: bool,
    /// *events dispatched since last frame*. A pointer/scroll/key/IME
    /// event reached `RenderRoot::event` between frames. Source: a shell-side
    /// latch set by the `nativeOnTouch`/IME entry points and cleared each
    /// frame.
    pub events_since_last_frame: bool,
    /// *active pointer capture*. A gesture is mid-drag and the captured
    /// widget may animate/track the pointer. Source:
    /// `AppTree`/`RenderRoot::is_pointer_captured`.
    pub pointer_capture_active: bool,
    /// *focus/IME session moved*. The root's focus flag or its published IME
    /// surface changed since the shell last **produced a frame** — an **edge**,
    /// not a level. Source:
    /// [`AppTree::focus_ime_generation`](crate::AppTree::focus_ime_generation)
    /// compared against the shell's cached copy, which the shell commits only
    /// once the gate has decided to run this frame (a *peek* at gather time —
    /// see the deferral note at the end of this doc).
    ///
    /// **Why an edge.** This was a *level* input
    /// (`RenderRoot::is_focus_active || ime_state().is_some()`) — the phase-7
    /// conservative default. Because it forces a `Run` through
    /// [`any_set`](Self::any_set) *and* disqualified
    /// [`is_paced_only_frame`](Self::is_paced_only_frame), any screen holding
    /// root focus rendered every single vsync for as long as the focus lasted,
    /// and caret pacing was unreachable while a caret blinked: measured at
    /// 62–120 fps on a static screen whose only live input was focus (Xiaomi
    /// 12). As an edge it still forces one frame per transition (focus gained /
    /// lost, IME surface published / cleared) while a *steady* focus session
    /// leaves the gate free to idle or pace.
    ///
    /// **Why one frame is enough.** Every IME event the platform delivers also
    /// trips [`events_since_last_frame`](Self::events_since_last_frame) (both
    /// shells latch it in their `ime_apply` entry points), and the per-frame
    /// platform IME reconcile (Kotlin `doFrame` / Swift `renderFrame`) polls the
    /// *published Rust state* on its own cadence, independent of whether Rust
    /// produced a frame — all it needs is that state to be current, which the
    /// edge guarantees by forcing the frame after every change.
    ///
    /// Unlike the other wake inputs this one is **not** an
    /// [`is_paced_only_frame`](Self::is_paced_only_frame) disqualifier, so an
    /// edge landing on a tick whose only other dirtiness is a paced loop is
    /// consumed by whichever tick's pacing decision resolves to `Run`: the
    /// repaint then lands with the loop's next paced frame (bounded by one
    /// `cosmetic_loop_rate` interval), and the platform's IME poll is unaffected
    /// either way. This is exactly why the shells peek the generation rather
    /// than draining it at gather time — an edge that a Skip erased would make
    /// the next repaint wait out the loop's *full* effective interval instead of
    /// the bound below. A *per-request*
    /// paced interval ([`FramePacing::requested_interval`]) never widens that
    /// bound — [`FrameGate::decide_paced`] tightens an edge-carrying tick back
    /// to the theme cap on purpose, so a 500ms caret cannot turn a focus
    /// transition into a 500ms lag.
    pub focus_or_ime_changed: bool,
    /// *last paint's `needs_frame`*. The previous paint advanced an
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
    /// *pending `ChangeFlags`*. A rebuild (or `set_theme`) left layout/
    /// paint dirtiness undrained. Source:
    /// [`AppTree::has_pending_change_flags`](crate::AppTree::has_pending_change_flags)
    /// (a non-draining peek, so a skipped frame preserves the flags).
    pub change_flags_pending: bool,
    /// *deferred callbacks owed a flush*. A widget queued a state-bearing
    /// callback during a state-free pass and raised
    /// [`frust_core::mark_pending_result_flush`], which only
    /// `RenderRoot::rebuild` can drain (it holds the `&mut State` the callback
    /// needs). Source: [`frust_core::has_pending_result_flush`], the
    /// non-draining peek — draining stays the rebuild's job on a frame that
    /// actually runs.
    ///
    /// Hardening under the default-to-run rule rather than a reproduced stall:
    /// every mark raised today is *also* covered by another input (a mark from
    /// inside a rebuild is drained by that same rebuild, whose leftover
    /// `pending |= PAINT` reaches [`change_flags_pending`](Self::change_flags_pending)
    /// and whose `deferred_frame` reaches [`last_needs_frame`](Self::last_needs_frame);
    /// `frust-widgets`' paint-time long-press latch pairs its mark with a
    /// `request_frame`). This input closes the general case those two happen to
    /// cover — a mark raised with nothing else dirty must never wait for the
    /// next stray touch.
    pub deferred_callbacks_pending: bool,
    /// *theme-override/appearance change*. The app-facing theme override
    /// or the platform light/dark preference changed this tick. Source: the
    /// shell's per-frame `ThemeOverrideWatcher`/appearance poll (see
    /// [`crate::theme_override`]).
    pub theme_or_appearance_changed: bool,
    /// *surface resize/recreation*. The GPU surface was created, resized,
    /// or recreated (rotation/backgrounding). Source: the shell's
    /// `nativeOnSurfaceChanged`/`frust_resize` path.
    pub surface_changed_or_resized: bool,
    /// *accessibility actions*. A platform `accesskit_*` action was
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
            || self.focus_or_ime_changed
            || self.last_needs_frame
            || self.last_needs_frame_paced_only
            || self.change_flags_pending
            || self.deferred_callbacks_pending
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
    /// One deliberate exception:
    /// [`focus_or_ime_changed`](Self::focus_or_ime_changed) is **not** a
    /// disqualifier. Its level-input predecessor was one, and — being true for
    /// a whole focus session — that is what made caret pacing unreachable: a
    /// blinking caret in a focused field is precisely the paced loop this gate
    /// must be able to throttle (that field's doc has the device measurement).
    /// The edge that replaced it reports one transition, not a session, so an
    /// edge arriving mid-loop is simply carried by the loop's next paced frame
    /// instead of pre-empting it. It is one repaint *per transition*, not
    /// per tick: the shells peek it, so it keeps reporting across every skipped
    /// tick in between and is cleared by the frame that finally runs.
    ///
    /// [`CosmeticLoop`]: frust_core::TickClass::CosmeticLoop
    pub fn is_paced_only_frame(&self) -> bool {
        self.last_needs_frame
            && self.last_needs_frame_paced_only
            && !self.signals_dirty
            && !self.events_since_last_frame
            && !self.pointer_capture_active
            && !self.change_flags_pending
            && !self.deferred_callbacks_pending
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
    /// The paced interval in force at the last produced frame — the cadence
    /// [`last_paced_run`](Self::last_paced_run) was anchored *for*.
    ///
    /// Per-request intervals ([`FramePacing::requested_interval`]) make the
    /// active interval a per-tick value rather than a constant, and
    /// [`anchor_paced`](Self::anchor_paced)'s drift-free `last + interval`
    /// arithmetic is only meaningful while that value holds still. Comparing
    /// against this is how the anchor detects a changed cadence and re-anchors
    /// to `now` instead — "the anchor adopts the interval in force at the last
    /// Run". `None` until the first pacing-aware decision.
    last_paced_interval: Option<std::time::Duration>,
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
            last_paced_interval: None,
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
    /// is throttled to [`FramePacing::effective_interval`] (the theme cap folded
    /// with the previous paint's requested interval): it runs only once
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
    /// **The focus/IME edge tightens the tick back to the theme cap.** The one
    /// wake input that rides *inside* a paced decision
    /// ([`FrameInputs::focus_or_ime_changed`]) has its deferral bounded by the
    /// interval in force, so honoring a long per-request interval on that tick
    /// would stretch a focus/IME transition's repaint out to (say) a caret's
    /// 500ms. Instead a tick carrying the edge paces at
    /// [`FramePacing::interval`] — the theme's own cap — leaving the edge's
    /// worst-case deferral exactly what it was before per-request intervals
    /// existed: one `cosmetic_loop_rate` interval (33ms at the 30Hz default;
    /// ≤100ms at `CosmeticLoopRate::FLOOR_HZ`). The cost is at most one extra
    /// frame per focus/IME *transition* — an edge reporting one transition, not
    /// a per-tick level (see `docs/LIMITATIONS.md`'s
    /// `focus-ime-edge-paced-deferral`).
    ///
    /// That bound only holds end-to-end because the shells **peek** the edge:
    /// the tightening itself resolves most edge-carrying ticks to `Skip` (the
    /// anchor is typically one vsync old, well inside the cap), so a shell that
    /// drained its generation cache at gather time would erase the edge on the
    /// very tick the tightening deferred it, and the repaint would fall back to
    /// the full [`FramePacing::effective_interval`]. The edge must keep being
    /// reported until a tick actually runs — see
    /// [`FrameInputs::focus_or_ime_changed`].
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
            // The interval in force for THIS tick: the theme cap folded with the
            // previous paint's requested interval, tightened back to the bare cap
            // when a focus/IME edge is riding along (see this method's docs — the
            // edge's deferral bound must not widen with a slow per-request
            // interval). The edge arm is literally `p.interval`: since
            // `effective_interval() == max(cap, requested) >= p.interval`,
            // tightening to the cap and `min`-ing with it are the same value —
            // "ignore the request on an edge tick".
            let interval = if inputs.focus_or_ime_changed {
                p.interval
            } else {
                p.effective_interval()
            };
            return match self.last_paced_run {
                // Inside the interval since the last produced frame: throttle.
                // The shell leaves `last_needs_frame` set across a skip (no
                // repaint re-latches it), so the gate keeps waking and never
                // starves the loop.
                Some(last) if p.now.saturating_sub(last) < interval => FrameDecision::Skip,
                _ => {
                    self.anchor_paced(p.now, interval);
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
    /// next interval is measured from the most recent real frame. Records the
    /// interval that was in force too, so the next paced fire compares against a
    /// current cadence rather than a stale one (see
    /// [`last_paced_interval`](Self::last_paced_interval)).
    fn anchor_non_paced(&mut self, pacing: Option<FramePacing>) {
        if let Some(p) = pacing {
            self.last_paced_run = Some(p.now);
            self.last_paced_interval = Some(p.effective_interval());
        }
    }

    /// Anchor the pace clock for a *paced* fire at `now`, for a loop running at
    /// `interval`: advance by exactly one interval to hold a drift-free cadence
    /// on the discrete vsync grid (anchoring to the raw `now`, which lands up to
    /// a tick past the ideal fire time, would drift the effective rate below the
    /// cap).
    ///
    /// Two cases re-anchor to `now` instead:
    ///
    /// - **A long stall** (≥ two intervals — a paused/resumed loop), so the loop
    ///   resumes at cadence rather than firing a catch-up burst.
    /// - **A changed interval.** The drift-free arithmetic assumes a fixed
    ///   interval; with per-request intervals ([`FramePacing::requested_interval`])
    ///   the active one can change between paced frames, and advancing an anchor
    ///   laid down under the *old* cadence by the *new* interval is what produces
    ///   a double-fire on a shortening flip (the old anchor can already be
    ///   several new intervals in the past). Adopting `now` at the fire is both
    ///   the burst-free and the stall-free answer: the flip costs at most one
    ///   fresh interval of wait, never a missed cadence.
    ///
    /// **Overflow ceiling.** `interval` here always traces back to a widget's
    /// [`frust_core::PaintCtx::request_frame_paced_at`], which clamps to
    /// [`frust_core::PaintCtx::MAX_PACED_INTERVAL`] (10s) before it is ever
    /// folded into `PaintOutcome::paced_interval` — so `interval * 2` and
    /// `interval.as_nanos() as u64` below stay far below `Duration`/`u64`
    /// overflow or truncation even at the widest legal input. This function
    /// performs no clamp of its own; it relies entirely on that upstream
    /// bound, the single entry point every paced interval flows through.
    fn anchor_paced(&mut self, now: FrameTime, interval: std::time::Duration) {
        let next = match (self.last_paced_run, self.last_paced_interval) {
            (Some(last), Some(previous))
                if previous == interval && now.saturating_sub(last) < interval * 2 =>
            {
                FrameTime::from_nanos(last.as_nanos().saturating_add(interval.as_nanos() as u64))
            }
            _ => now,
        };
        self.last_paced_run = Some(next);
        self.last_paced_interval = Some(interval);
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
            "focus_or_ime_changed" => i.focus_or_ime_changed = true,
            "last_needs_frame" => i.last_needs_frame = true,
            "last_needs_frame_paced_only" => i.last_needs_frame_paced_only = true,
            "change_flags_pending" => i.change_flags_pending = true,
            "deferred_callbacks_pending" => i.deferred_callbacks_pending = true,
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
        "focus_or_ime_changed",
        "last_needs_frame",
        "last_needs_frame_paced_only",
        "change_flags_pending",
        "deferred_callbacks_pending",
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
        // must force a Run — including the two that changed shape here: the
        // focus/IME EDGE (`focus_or_ime_changed`, which still forces one frame
        // per transition even though it no longer blocks pacing) and the
        // deferred-callback peek (`deferred_callbacks_pending`).
        assert_eq!(
            ALL_INPUTS.len(),
            12,
            "the OR-list must have all eleven wake inputs plus the \
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

    /// A pacing context naming no per-request interval: the theme cap alone —
    /// what every `request_frame_paced` (interval-less) caller produces, and the
    /// shape every pre-`request_frame_paced_at` test in this module drives.
    fn pacing(now: FrameTime, interval: Duration) -> FramePacing {
        FramePacing {
            now,
            interval,
            requested_interval: None,
        }
    }

    /// A pacing context carrying a per-request interval, as a shell latches it
    /// from the previous paint's `PaintOutcome::paced_interval`.
    fn pacing_at(now: FrameTime, interval: Duration, requested: Duration) -> FramePacing {
        FramePacing {
            now,
            interval,
            requested_interval: Some(requested),
        }
    }

    /// A ~2Hz caret blink — the motivating per-request interval, deliberately
    /// far slower than any theme's cosmetic-loop cap.
    fn interval_500ms() -> Duration {
        Duration::from_millis(500)
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
        // ...including the deferred-callback peek, which owes a rebuild.
        let mut with_flush = paced_only();
        with_flush.deferred_callbacks_pending = true;
        assert!(!with_flush.is_paced_only_frame());

        // THE POINT OF THE EDGE: a STEADY focus session (a caret blinking in a
        // focused field — the edge is false because nothing moved since the
        // last tick) alongside a paced-only request now PACES. As the old level
        // input (`focus_or_ime_active`, true for the whole session) this was
        // disqualified, so a focused screen re-rendered every vsync forever and
        // the pacing arm was unreachable — 62–120 fps measured on a Xiaomi 12.
        let steady_focus = FrameInputs {
            focus_or_ime_changed: false,
            ..paced_only()
        };
        assert!(
            steady_focus.is_paced_only_frame(),
            "a steady focus session must not block paced-loop throttling"
        );
        let mut gate = FrameGate::with_flags(true, true);
        let interval = interval_30hz();
        // First paced frame runs (nothing to pace against yet) and anchors...
        assert!(
            gate.decide_paced(steady_focus, pacing(FrameTime::from_nanos(0), interval),)
                .is_run()
        );
        // ...and the very next 120Hz tick, still inside the 30Hz interval, is
        // throttled — the behavior a focused screen could never reach before.
        assert_eq!(
            gate.decide_paced(
                steady_focus,
                pacing(FrameTime::from_nanos(step_nanos(120.0)), interval),
            ),
            FrameDecision::Skip,
            "a paced loop under a steady focus session must throttle to the cap"
        );

        // The transition edge itself is deliberately NOT a disqualifier (see
        // `is_paced_only_frame`'s docs): it is one-shot, so the repaint it asks
        // for lands with the loop's next paced frame rather than immediately.
        let edge_mid_loop = FrameInputs {
            focus_or_ime_changed: true,
            ..paced_only()
        };
        assert!(edge_mid_loop.is_paced_only_frame());
        // With no paced loop in flight it forces a Run like every other input
        // (`each_single_input_forces_run` above covers the isolated case).
        assert!(
            FrameInputs {
                focus_or_ime_changed: true,
                ..FrameInputs::default()
            }
            .any_set()
        );
    }

    // -----------------------------------------------------------------
    // The focus/IME EDGE, driven the way a shell drives it
    // -----------------------------------------------------------------

    /// The shells' cache mechanics, verbatim, in the two halves both mobile
    /// shells actually run: [`peek`](FocusEdgeCache::peek) at gather time (a
    /// non-mutating `generation != last_seen`, the value that feeds
    /// [`FrameInputs::focus_or_ime_changed`]) and
    /// [`commit`](FocusEdgeCache::commit) as the first statement past the
    /// `decide_paced(..).is_skip()` early return, i.e. only on a frame that
    /// actually runs.
    ///
    /// The split is the contract, not an implementation detail: this is the one
    /// OR-list input that is both consumed-on-read and *not* an
    /// [`FrameInputs::is_paced_only_frame`] disqualifier, so committing it at
    /// gather time erases any edge the pacing defers.
    ///
    /// Both mobile shells are target-gated (`#[cfg(target_os = ...)]`) so their
    /// own copies never compile on the host — this stands in for them, with
    /// `frust-core`'s generation tests (`focus_ime_generation_*`) pinning the
    /// other half: that the generation moves exactly once per real focus/IME
    /// transition and never on a same-value write.
    struct FocusEdgeCache {
        last_seen: u64,
    }
    impl FocusEdgeCache {
        fn new(seed: u64) -> Self {
            Self { last_seen: seed }
        }
        /// Gather-time peek: "has the session moved since the last frame this
        /// shell produced?" — never mutates, so a skipped tick keeps reporting
        /// the same pending edge.
        fn peek(&self, generation: u64) -> bool {
            generation != self.last_seen
        }
        /// Run-time commit: the edge has now been consumed by a frame that is
        /// actually being produced.
        fn commit(&mut self, generation: u64) {
            self.last_seen = generation;
        }
    }

    #[test]
    fn focus_edge_fires_once_per_transition_and_goes_quiet() {
        // A scripted focus/IME session as `RenderRoot::focus_ime_generation`
        // reports it: idle, focus gained, steady blinking, IME published,
        // steady typing pause, IME cleared, blur, idle again. Each transition
        // bumps the generation exactly once; the steady stretches repeat it.
        let script: &[(u64, bool, &str)] = &[
            (0, false, "idle before anything is focused"),
            (0, false, "still idle"),
            (1, true, "focus gained"),
            (1, false, "steady focus (caret blinking)"),
            (1, false, "still steady"),
            (2, true, "IME surface published"),
            (2, false, "steady IME session"),
            (3, true, "IME surface cleared"),
            (4, true, "focus lost (blur)"),
            (4, false, "idle again"),
        ];
        let mut cache = FocusEdgeCache::new(0);
        let mut gate = FrameGate::with_enabled(true);
        for (generation, expected_edge, what) in script {
            let inputs = FrameInputs {
                focus_or_ime_changed: cache.peek(*generation),
                ..FrameInputs::default()
            };
            assert_eq!(
                inputs.focus_or_ime_changed, *expected_edge,
                "edge for {what:?}"
            );
            // With every other input clear, the decision follows the edge
            // exactly: one Run per transition, Skip through each steady stretch
            // — where the old level input ran every single tick.
            let expect = if *expected_edge {
                FrameDecision::Run
            } else {
                FrameDecision::Skip
            };
            let decision = gate.decide(inputs);
            assert_eq!(decision, expect, "decision for {what:?}");
            // The shells' commit site: only a produced frame consumes the edge.
            // Here every edge tick Runs (nothing paces it), so the cache
            // advances on exactly the transitions — one repaint each.
            if decision.is_run() {
                cache.commit(*generation);
            }
        }
    }

    #[test]
    fn a_long_steady_focus_session_produces_no_frames() {
        // The regression this task closes, in the shape the device showed it: a
        // static screen holding focus, nothing else dirty, over a long tick
        // stream. The generation never moves, so the gate must produce ZERO
        // frames (it produced one per vsync — 62–120 fps — as a level input).
        let mut cache = FocusEdgeCache::new(7);
        let mut gate = FrameGate::with_enabled(true);
        let mut runs = 0usize;
        for _ in 0..600 {
            let inputs = FrameInputs {
                focus_or_ime_changed: cache.peek(7),
                ..FrameInputs::default()
            };
            if gate.decide(inputs).is_run() {
                cache.commit(7);
                runs += 1;
            }
        }
        assert_eq!(runs, 0, "a steady focus session must produce no frames");
    }

    #[test]
    fn a_focus_edge_persists_across_paced_skips_and_lands_within_one_cap_interval() {
        // The end-to-end contract, driven exactly the way a mobile shell's
        // `frame()` drives it: PEEK the generation into `FrameInputs`, decide,
        // and commit the cache only past the `is_skip()` early return.
        //
        // Shape: a 120Hz tick stream, a caret loop naming its own 500ms cadence
        // (`PaintCtx::request_frame_paced_at`), and a focus/IME transition
        // landing one vsync into that interval. The edge tightening resolves
        // that tick to Skip — the loop's anchor is one tick old, far inside the
        // 33ms cap — which is precisely why the edge must NOT be consumed there:
        // a shell draining its cache at gather time erases it, and the repaint
        // then falls through to the caret's full 500ms interval (the pre-fix
        // behaviour, contrasted at the end of this test).
        let tick = step_nanos(120.0);
        let cap = interval_30hz();
        let at = |i: u64| FrameTime::from_nanos(i * tick);
        let caret = |i: u64| FramePacing {
            now: at(i),
            interval: cap,
            requested_interval: Some(interval_500ms()),
        };
        let inputs = |edge: bool| FrameInputs {
            focus_or_ime_changed: edge,
            ..paced_only()
        };

        let mut gate = FrameGate::with_flags(true, true);
        let mut cache = FocusEdgeCache::new(5);

        // Tick 0: the caret loop's own first paced frame, no edge — it anchors
        // the pace clock at t=0.
        assert!(!cache.peek(5));
        assert!(gate.decide_paced(inputs(false), caret(0)).is_run());
        cache.commit(5);

        // The transition happens: the generation moves 5 -> 6. Every tick from
        // here until the loop's next produced frame PEEKS the same still-pending
        // edge — the persistence a drain-on-gather shell (and the pre-fix
        // `gather` helper this suite used) could not express — and the frame the
        // edge asked for lands on the first tick that runs, which is where it is
        // finally consumed.
        let mut landed_tick = None;
        for i in 1..120u64 {
            assert!(
                cache.peek(6),
                "the edge must still be pending at tick {i} — a Skip consumes nothing"
            );
            if gate.decide_paced(inputs(true), caret(i)).is_run() {
                cache.commit(6);
                landed_tick = Some(i);
                break;
            }
        }
        let landed_tick =
            landed_tick.expect("the deferred edge must be consumed by a tick that runs");
        let landed_ns = landed_tick * tick;
        assert!(
            landed_ns <= cap.as_nanos() as u64 + tick,
            "the edge repaint must land within one cap interval of the loop's \
             last produced frame (plus one tick of 120Hz grid rounding); landed \
             at {landed_ns}ns"
        );
        assert!(
            landed_ns < interval_500ms().as_nanos() as u64,
            "…and nowhere near the caret's own 500ms request"
        );

        // Consumed exactly once: the next tick reports no edge, and the loop
        // goes straight back to its own slow cadence (the tightening is scoped
        // to the edge tick alone).
        assert!(!cache.peek(6), "a produced frame clears the edge");
        assert_eq!(
            gate.decide_paced(inputs(false), caret(landed_tick + 1)),
            FrameDecision::Skip
        );

        // The contrast that makes the shell-side split load-bearing: the same
        // stream with the pre-fix shape — the generation cache committed at
        // GATHER time, whatever the decision — loses the edge on the very first
        // skipped tick, so the repaint waits out the caret's 500ms request.
        let mut drained_gate = FrameGate::with_flags(true, true);
        let mut drained_cache = FocusEdgeCache::new(5);
        let mut first_run_after_bump: Option<u64> = None;
        for i in 0..120u64 {
            let generation = if i >= 1 { 6 } else { 5 };
            let edge = drained_cache.peek(generation);
            drained_cache.commit(generation); // the pre-fix drain: unconditional
            if drained_gate.decide_paced(inputs(edge), caret(i)).is_run() && i >= 1 {
                first_run_after_bump = Some(i * tick);
                break;
            }
        }
        assert!(
            first_run_after_bump.is_some_and(|ns| ns >= interval_500ms().as_nanos() as u64),
            "drain-on-gather erases the deferred edge, so the repaint falls \
             through to the 500ms per-request interval; observed \
             {first_run_after_bump:?}ns"
        );
    }

    #[test]
    fn a_warmup_run_commits_the_focus_edge_too() {
        // The commit-on-Run rule keys off `is_skip()` and nothing else, so it
        // needs no special-casing for the warmup path — which returns `Run`
        // *before* `any_set()`/`is_paced_only_frame` are ever consulted. A
        // pending edge riding a warmup frame is therefore consumed by it, once.
        let cap = interval_30hz();
        let mut gate = FrameGate::with_flags(true, true);
        let mut cache = FocusEdgeCache::new(1);
        gate.note_resumed();

        // A focus/IME transition landing on the first post-resume tick.
        assert!(cache.peek(2));
        let decision = gate.decide_paced(
            FrameInputs {
                focus_or_ime_changed: cache.peek(2),
                ..paced_only()
            },
            pacing(FrameTime::from_nanos(0), cap),
        );
        assert_eq!(
            decision,
            FrameDecision::Run,
            "the resume warmup forces this frame to run"
        );
        assert_eq!(gate.warmup_remaining(), WARMUP_FRAMES - 1);
        // A Run is a Run: the shell commits here exactly as on any other.
        assert!(decision.is_run());
        cache.commit(2);

        // Consumed once — the rest of the warmup window still runs (that is the
        // warmup's job), but it no longer carries an edge.
        assert!(!cache.peek(2), "the warmup frame consumed the edge");
        for i in 1..=u64::from(WARMUP_FRAMES - 1) {
            assert!(!cache.peek(2));
            assert!(
                gate.decide_paced(
                    FrameInputs {
                        focus_or_ime_changed: cache.peek(2),
                        ..paced_only()
                    },
                    pacing(FrameTime::from_nanos(i * step_nanos(120.0)), cap),
                )
                .is_run()
            );
            cache.commit(2);
        }
        assert_eq!(gate.warmup_remaining(), 0);
    }

    // -----------------------------------------------------------------
    // The deferred-callback flush peek
    // -----------------------------------------------------------------

    #[test]
    fn a_marked_flush_alone_forces_a_run() {
        // The stall this input closes: a deferred state-bearing callback is
        // owed a `Housekeeping` broadcast that only `RenderRoot::rebuild` can
        // dispatch, and NOTHING else is dirty. Without this input the gate
        // skips, the rebuild never runs, and the callback waits for whatever
        // touch happens to arrive next.
        let mut gate = FrameGate::with_enabled(true);
        let owed = FrameInputs {
            deferred_callbacks_pending: true,
            ..FrameInputs::default()
        };
        assert_eq!(gate.decide(owed), FrameDecision::Run);
        // And it keeps forcing frames until the drain clears the mark — the
        // shell re-peeks every tick, so the input simply goes false.
        assert_eq!(gate.decide(owed), FrameDecision::Run);
        assert_eq!(
            gate.decide(FrameInputs::default()),
            FrameDecision::Skip,
            "once the rebuild drained the mark the gate idles again"
        );
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
            let pacing = pacing(FrameTime::from_nanos(i * tick), interval);
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
        let run0 = gate.decide_paced(paced_only(), pacing(FrameTime::from_nanos(0), interval));
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
            pacing(FrameTime::from_nanos(step_nanos(120.0)), interval),
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
                pacing(FrameTime::from_nanos(2 * step_nanos(120.0)), interval),
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
            let pacing = pacing(FrameTime::from_nanos(i * tick), interval);
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
            let pacing = pacing(FrameTime::from_nanos(i * tick), interval);
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
            let pacing = pacing(
                FrameTime::from_nanos(i * step_nanos(120.0)),
                interval_30hz(),
            );
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

    // -----------------------------------------------------------------
    // Per-request paced intervals (`PaintCtx::request_frame_paced_at`)
    // -----------------------------------------------------------------

    /// Count the runs a constant paced-only stream produces over `ticks` ticks
    /// of a 120Hz tick stream, at `interval` (theme cap) and `requested`
    /// (per-request, `None` for a bare `request_frame_paced`).
    fn paced_runs(
        gate: &mut FrameGate,
        ticks: u64,
        interval: Duration,
        requested: Option<Duration>,
    ) -> usize {
        let tick = step_nanos(120.0);
        (0..ticks)
            .filter(|i| {
                let now = FrameTime::from_nanos(i * tick);
                let p = FramePacing {
                    now,
                    interval,
                    requested_interval: requested,
                };
                gate.decide_paced(paced_only(), p).is_run()
            })
            .count()
    }

    #[test]
    fn effective_interval_folds_the_request_against_the_theme_cap() {
        let cap = interval_30hz();
        // No request named: the theme's own cadence, verbatim (every caller
        // before `request_frame_paced_at` existed).
        assert_eq!(pacing(FrameTime::ZERO, cap).effective_interval(), cap);
        // `Duration::ZERO` is what a bare `request_frame_paced` folds in — "at
        // the theme's own rate" — so it resolves identically to `None`.
        assert_eq!(
            pacing_at(FrameTime::ZERO, cap, Duration::ZERO).effective_interval(),
            cap
        );
        // A slower request is honored as asked.
        assert_eq!(
            pacing_at(FrameTime::ZERO, cap, interval_500ms()).effective_interval(),
            interval_500ms()
        );
        // A request TIGHTER than the cap is clamped to it: `cosmetic_loop_rate`
        // is a ceiling on decorative motion, not a target (motion that must
        // land every vsync is `TickClass::Transition`, never paced at all).
        assert_eq!(
            pacing_at(FrameTime::ZERO, cap, Duration::from_millis(8)).effective_interval(),
            cap
        );
    }

    #[test]
    fn a_frame_requesting_33ms_and_500ms_paces_at_33ms() {
        // The MIN-lattice, driven through the REAL core-side fold: two paced
        // requests in one paint pass (a 30Hz shimmer and a 2Hz caret) aggregate
        // to the tightest, so the gate paces the frame at 33ms — the caret is
        // simply repainted more often than it needs (no visual harm, by
        // design), and the shimmer is never starved down to 2Hz.
        let mut paint =
            frust_core::PaintCtx::new(kurbo::Point::ZERO, kurbo::Size::new(100.0, 100.0));
        paint.request_frame_paced_at(Duration::from_millis(33));
        paint.request_frame_paced_at(interval_500ms());
        let requested = paint.paced_interval();
        assert_eq!(requested, Some(Duration::from_millis(33)));

        // 1 simulated second at 120Hz against the 30Hz framework-default cap.
        let mut gate = FrameGate::with_flags(true, true);
        let runs = paced_runs(&mut gate, 120, interval_30hz(), requested);
        assert!(
            (28..=32).contains(&runs),
            "a 33ms+500ms frame must pace at 33ms (~30 runs/s), ran {runs}"
        );
        // The contrast that makes the fold load-bearing: the same stream with
        // ONLY the 500ms request paces at 2Hz. Keeping the 33ms requester is
        // what holds the frame at 30Hz.
        let mut slow_gate = FrameGate::with_flags(true, true);
        let slow_runs = paced_runs(&mut slow_gate, 120, interval_30hz(), Some(interval_500ms()));
        assert!(
            (1..=3).contains(&slow_runs),
            "the 500ms request alone should run ~2x/s, ran {slow_runs}"
        );
    }

    #[test]
    fn a_500ms_request_paces_far_slower_than_the_theme_cap() {
        // The motivating case: a caret blink asking for 2Hz against a 30Hz cap
        // must produce ~2 frames per simulated second, not ~30.
        let mut gate = FrameGate::with_flags(true, true);
        let runs = paced_runs(&mut gate, 120, interval_30hz(), Some(interval_500ms()));
        assert!(
            (1..=3).contains(&runs),
            "a 500ms paced request should run ~2x/s, ran {runs}"
        );
        // ...and the same stream with no request named still runs at the cap,
        // so the slowdown is the request's doing and nothing else's.
        let mut cap_gate = FrameGate::with_flags(true, true);
        let cap_runs = paced_runs(&mut cap_gate, 120, interval_30hz(), None);
        assert!(
            (28..=32).contains(&cap_runs),
            "an interval-less paced stream still runs at the cap, ran {cap_runs}"
        );
    }

    #[test]
    fn a_changed_interval_neither_bursts_nor_stalls_the_cadence() {
        // Cadence stability under a VARIABLE interval — the caveat the
        // drift-free `last + interval` anchor arithmetic would otherwise trip
        // on. The active interval flips 33ms <-> 500ms every simulated second
        // across paced frames; at each transition the anchor adopts the interval
        // in force at that Run, so:
        //   * no BURST — two paced runs never land closer than the tighter of
        //     the two intervals (minus one tick of grid rounding), and
        //   * no STALL — they never land further apart than the slower interval
        //     plus one tick.
        let tick = step_nanos(120.0);
        // The realistic flip: a shimmer (the bare theme-cap request, 33ms at the
        // 30Hz default) appearing and disappearing while a 500ms caret blinks —
        // the MIN fold hands the gate 33ms while both run, 500ms once only the
        // caret is left.
        let fast = interval_30hz();
        let slow = interval_500ms();

        // Swept across every flip PHASE, because the pathological case is
        // phase-dependent: it needs the flip to land a *part* of an interval
        // after the last produced frame (the residue an anchor advanced under
        // the old cadence turns into a double-fire), which only some offsets
        // produce. The flip period is deliberately 137 ticks — not a whole
        // number of either interval — so the sweep walks the whole phase space
        // instead of locking onto one alignment.
        const FLIP_PERIOD: u64 = 137;
        for offset in 0..FLIP_PERIOD {
            let mut gate = FrameGate::with_flags(true, true);
            let mut last_run: Option<u64> = None;
            let mut min_gap_ns = u64::MAX;
            let mut max_gap_ns = 0u64;
            // 6 simulated seconds at 120Hz, flipping the request periodically.
            for i in 0..720u64 {
                let now_ns = i * tick;
                let shimmer_onscreen = ((i + offset) / FLIP_PERIOD).is_multiple_of(2);
                let requested = if shimmer_onscreen {
                    Duration::ZERO
                } else {
                    slow
                };
                let p = FramePacing {
                    now: FrameTime::from_nanos(now_ns),
                    interval: fast,
                    requested_interval: Some(requested),
                };
                if gate.decide_paced(paced_only(), p).is_run() {
                    if let Some(prev) = last_run {
                        let gap = now_ns - prev;
                        min_gap_ns = min_gap_ns.min(gap);
                        max_gap_ns = max_gap_ns.max(gap);
                    }
                    last_run = Some(now_ns);
                }
            }

            assert!(
                min_gap_ns + tick >= fast.as_nanos() as u64,
                "no burst (flip offset {offset}): the tightest observed gap \
                 ({min_gap_ns}ns) must not undercut the fast interval by more \
                 than one tick"
            );
            assert!(
                max_gap_ns <= slow.as_nanos() as u64 + tick,
                "no stall (flip offset {offset}): the widest observed gap \
                 ({max_gap_ns}ns) must not exceed the slow interval by more than \
                 one tick"
            );
        }
    }

    #[test]
    fn a_shortening_interval_flip_never_double_fires() {
        // The exact shape the "anchor adopts the interval in force at the last
        // Run" rule exists for, on a real 120Hz grid: a 500ms caret loop whose
        // shimmer comes back (500ms -> 33ms) at a moment that is *part way*
        // through the old cadence. Advancing the old anchor by the NEW interval
        // there leaves it up to a full interval in the past, so the frame after
        // the flip fires on the very next tick — a visible double-fire.
        let cap = interval_30hz();
        let slow = interval_500ms();
        let mut gate = FrameGate::with_flags(true, true);
        let paced_at = |now_ns: u64, requested: Duration| FramePacing {
            now: FrameTime::from_nanos(now_ns),
            interval: cap,
            requested_interval: Some(requested),
        };

        // A shimmer+caret frame (the MIN fold hands the gate the cap) anchors
        // the loop at t=0.
        assert!(
            gate.decide_paced(paced_only(), paced_at(0, Duration::ZERO))
                .is_run()
        );
        // The shimmer ends: only the 500ms caret is left. Two slow frames.
        assert!(
            gate.decide_paced(paced_only(), paced_at(500_000_000, slow))
                .is_run()
        );
        assert!(
            gate.decide_paced(paced_only(), paced_at(900_000_000, slow))
                .is_skip()
        );
        assert!(
            gate.decide_paced(paced_only(), paced_at(1_000_000_000, slow))
                .is_run()
        );

        // The shimmer returns 58ms into the caret's 500ms interval — past the
        // cap, so this tick fires.
        assert!(
            gate.decide_paced(paced_only(), paced_at(1_058_333_333, Duration::ZERO))
                .is_run()
        );
        // The next three 120Hz ticks must all skip: the cadence restarts from
        // the frame just produced, not from the stale 500ms anchor.
        for (n, now_ns) in [1_066_666_666u64, 1_075_000_000, 1_083_333_333]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                gate.decide_paced(paced_only(), paced_at(now_ns, Duration::ZERO)),
                FrameDecision::Skip,
                "tick {n} after a shortening flip must not double-fire"
            );
        }
        // ...and exactly one cap interval later the loop resumes cadence (no
        // stall either).
        assert_eq!(
            gate.decide_paced(paced_only(), paced_at(1_091_666_666, Duration::ZERO)),
            FrameDecision::Run,
            "the fast cadence resumes one interval after the flip frame"
        );
    }

    #[test]
    fn the_anchor_adopts_the_interval_in_force_at_the_last_run() {
        // The rule the test above measures, asserted directly on the two flips.
        let cap = Duration::from_millis(10);
        let fast = Duration::from_millis(33);
        let slow = interval_500ms();
        let mut gate = FrameGate::with_flags(true, true);

        let at = |ms: u64| FrameTime::from_nanos(ms * 1_000_000);
        let paced = |now: FrameTime, requested: Duration| FramePacing {
            now,
            interval: cap,
            requested_interval: Some(requested),
        };

        // t=0: first paced frame anchors at 33ms cadence.
        assert!(gate.decide_paced(paced_only(), paced(at(0), fast)).is_run());
        // t=20ms: inside the 33ms interval — throttled.
        assert!(
            gate.decide_paced(paced_only(), paced(at(20), fast))
                .is_skip()
        );
        // t=40ms: one 33ms interval elapsed — runs, cadence intact.
        assert!(
            gate.decide_paced(paced_only(), paced(at(40), fast))
                .is_run()
        );

        // The loop now flips to 500ms (the shimmer ended; only a caret is left).
        // t=100ms is 60ms past the last run: well inside the NEW interval, so it
        // must throttle rather than fire on the stale 33ms cadence.
        assert!(
            gate.decide_paced(paced_only(), paced(at(100), slow))
                .is_skip()
        );
        // t=545ms — 500ms past the t=40ms(+33) anchor — fires, and re-anchors to
        // `now` because the interval changed.
        assert!(
            gate.decide_paced(paced_only(), paced(at(545), slow))
                .is_run()
        );
        // Next 500ms tick lands one full interval later, not sooner (no burst).
        assert!(
            gate.decide_paced(paced_only(), paced(at(800), slow))
                .is_skip()
        );
        assert!(
            gate.decide_paced(paced_only(), paced(at(1045), slow))
                .is_run()
        );

        // Flip back to 33ms: the very next tick past one fast interval runs (no
        // stall waiting out the old 500ms cadence), then holds the fast cadence.
        assert!(
            gate.decide_paced(paced_only(), paced(at(1060), fast))
                .is_skip()
        );
        assert!(
            gate.decide_paced(paced_only(), paced(at(1080), fast))
                .is_run()
        );
        assert!(
            gate.decide_paced(paced_only(), paced(at(1090), fast))
                .is_skip(),
            "no double-fire immediately after a shortening flip"
        );
        assert!(
            gate.decide_paced(paced_only(), paced(at(1115), fast))
                .is_run()
        );
    }

    #[test]
    fn a_focus_ime_edge_is_bounded_by_the_theme_cap_not_a_long_request() {
        // The decided bound for the one wake input that rides INSIDE a paced
        // decision: a focus/IME edge landing on a paced-only tick waits at most
        // one `cosmetic_loop_rate` interval — never the (much longer) per-request
        // interval a caret named. Without the tightening this edge would sit
        // behind a 500ms caret blink, a user-visible focus lag.
        //
        // This pins the GATE half of that contract and holds unchanged: it feeds
        // the edge to every tick by hand, which is what a shell must actually do
        // for the bound to be real. The shell half — peeking the generation and
        // committing it only on a Run, so a deferred edge survives the Skips in
        // between — is pinned by
        // `a_focus_edge_persists_across_paced_skips_and_lands_within_one_cap_interval`
        // above.
        let cap = interval_30hz();
        let mut gate = FrameGate::with_flags(true, true);
        let at = |ms: u64| FrameTime::from_nanos(ms * 1_000_000);
        let caret = |now: FrameTime| FramePacing {
            now,
            interval: cap,
            requested_interval: Some(interval_500ms()),
        };
        let edge = FrameInputs {
            focus_or_ime_changed: true,
            ..paced_only()
        };

        // A 500ms caret loop, anchored by its first paced frame at t=0.
        assert!(gate.decide_paced(paced_only(), caret(at(0))).is_run());
        // t=20ms: still inside the 33ms cap, so even an edge waits (the bound is
        // ONE cap interval, not zero — this is the documented deferral).
        assert_eq!(
            gate.decide_paced(edge, caret(at(20))),
            FrameDecision::Skip,
            "an edge inside the cap interval is still absorbed by the pacing"
        );
        // t=40ms: one cap interval past the anchor — the edge's frame lands here
        // rather than at t=500ms.
        assert_eq!(
            gate.decide_paced(edge, caret(at(40))),
            FrameDecision::Run,
            "a focus/IME edge must not wait out a long per-request interval"
        );
        // Worst case, stated as the bound: the edge never waits longer than one
        // theme-cap interval from the loop's last produced frame.
        assert!(cap <= Duration::from_millis(100));

        // With no edge, the same loop keeps its own slow cadence — the
        // tightening is scoped to the edge tick alone.
        assert_eq!(
            gate.decide_paced(paced_only(), caret(at(80))),
            FrameDecision::Skip,
            "the edge tick must not permanently re-tighten the caret's cadence"
        );
        assert_eq!(
            gate.decide_paced(paced_only(), caret(at(560))),
            FrameDecision::Run
        );
    }
}
