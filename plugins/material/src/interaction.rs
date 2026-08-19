//! The unified M3(E) interaction substrate: hover/focus/pressed/dragged
//! tracking, an M3E-precedence state-layer opacity resolver, an optional
//! press-scale spring spec, and a pluggable haptics hook. Modeled on the
//! reference's `M3ETappable`/`M3EStateLayer`/`M3EHaptics` foundations
//! (`tmp/material_3_expressive/lib/foundations/m3e_tappable.dart`,
//! `m3e_state_layer.dart`, `m3e_haptics.dart`), unified here into one
//! substrate every interactive component in this crate builds on, rather
//! than each hand-rolling its own copy of the same four booleans.
//!
//! # A deliberate porting decision
//!
//! The reference's own `M3EButton` notably does **not** build on
//! `M3ETappable` — it hand-rolls its own state mixin instead (a verified
//! research correction, not an omission in this port). This crate unifies
//! anyway: every interactive component here — present and future — is
//! meant to track its interaction state through [`InteractionState`]
//! rather than re-deriving the same four flags per widget, trading the
//! reference's per-widget flexibility for one shared substrate.
//!
//! # Opacity model: precedence, not max-of-active
//!
//! [`InteractionState::resolve_opacity`] is the new documented default,
//! porting `M3EInteractionState.opacity`'s **priority** resolution
//! exactly: `dragged > pressed > focused > hovered` — the single
//! highest-priority active state contributes the overlay, never a stack of
//! several. [`crate::state_layer::StateLayer`] (this crate's older helper,
//! kept fully back-compatible for its existing consumers —
//! `list_item`/`card`/`chips`/`fab`/`switch`) instead resolves by
//! [`InteractionState::max_active_opacity`], the **maximum** of every
//! active state's opacity.
//!
//! Given this crate's own token table — `hover 8% < focus 10% = pressed
//! 10% < dragged 16%`, i.e. every state later in the precedence order
//! carries opacity `>=` every state earlier in it — the two algorithms
//! always agree numerically today: the highest-priority active state is
//! *always* the maximum active one. They stay two distinct functions
//! rather than collapsing to one because that agreement is a property of
//! today's token values, not a guarantee the resolver shapes make on their
//! own — a future token retune that broke the monotonic ordering would
//! make them diverge, and `StateLayer`'s own tests pin its (max-of-active)
//! behavior specifically, independent of this module.
//!
//! # Haptics: a pluggable, no-op-by-default hook
//!
//! [`MaterialHaptics`] is a process-global, set-once hook. This crate
//! reaches no `frust-haptics` dependency itself — the design-system
//! charter forbids a plugin-tier crate depending on a sibling plugin (see
//! `docs/PLUGINS_ARCHITECTURE.md`'s Layer Dependencies). Firing a
//! [`HapticSignal`] before an app installs a hook is a silent no-op.
//! [`MaterialHaptics::set`]'s doc has the exact install-once timing
//! contract and a worked `frust-haptics` wiring example.
//!
//! # Press-scale: shared spec, no animation runtime
//!
//! [`PressScaleSpec`] carries only the *shape* of an optional press-scale
//! spring — a target scale plus a named [`PressSpringId`] — mirroring the
//! reference's `M3ETappable.pressedScale` + `.spring` pair. This module
//! ships no animation-controller driving loop of its own: actually
//! springing a widget's paint scale toward `target_scale` on press (and
//! back on release) stays each widget's own job, the established pattern
//! `button_group`'s own `PRESS_SPRING` constant plus `press_anim` field
//! set — this task ships the shared state and resolved-values plumbing
//! ([`PressScaleSpec::is_active`], [`PressSpringId::resolve`]), not a
//! runtime.

use std::sync::OnceLock;

use frust::SpringDesc;

pub use frust::authoring::PRESSED_OPACITY;

/// Hover-state overlay opacity (8%). Source: androidx Compose Material3
/// `StateTokens` (`v0_210`), the same table [`crate::state_layer`] cites.
pub const HOVER_OPACITY: f32 = 0.08;
/// Focus-state overlay opacity (10%).
pub const FOCUS_OPACITY: f32 = 0.10;
/// Dragged-state overlay opacity (16%).
pub const DRAGGED_OPACITY: f32 = 0.16;
/// Content (icon/label) opacity for a disabled component (38%). Not yet
/// wired to any live disabled-state widget in this crate — a named token
/// so the value has one source the moment one exists, the same
/// aspirational-but-stable status [`crate::state_layer`] documents for its
/// still-unwired `dragged` state.
pub const DISABLED_CONTENT_OPACITY: f32 = 0.38;
/// Container opacity for a disabled component (12%). Same unwired status as
/// [`DISABLED_CONTENT_OPACITY`].
pub const DISABLED_CONTAINER_OPACITY: f32 = 0.12;

/// Tracks a widget's hover/focus/pressed/dragged interaction state. See the
/// [module docs](self) for the two opacity resolvers this feeds.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct InteractionState {
    pub hovered: bool,
    pub focused: bool,
    pub pressed: bool,
    pub dragged: bool,
}

impl InteractionState {
    /// A fresh state with every interaction flag clear.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the widget's hover state. Returns whether the flag changed —
    /// the frame source for hover *gain* per `docs/CODE_STANDARDS.md`'s
    /// Interaction Semantics hover-claiming seam (`claim_hover` itself
    /// requests no redraw).
    pub fn set_hovered(&mut self, hovered: bool) -> bool {
        let changed = self.hovered != hovered;
        self.hovered = hovered;
        changed
    }

    /// Record the widget's focus state. Returns whether the flag changed.
    pub fn set_focused(&mut self, focused: bool) -> bool {
        let changed = self.focused != focused;
        self.focused = focused;
        changed
    }

    /// Record the widget's pressed state. Returns whether the flag changed.
    pub fn set_pressed(&mut self, pressed: bool) -> bool {
        let changed = self.pressed != pressed;
        self.pressed = pressed;
        changed
    }

    /// Record the widget's dragged state. Returns whether the flag changed.
    pub fn set_dragged(&mut self, dragged: bool) -> bool {
        let changed = self.dragged != dragged;
        self.dragged = dragged;
        changed
    }

    /// Whether any interaction state is currently active.
    pub fn is_active(&self) -> bool {
        self.hovered || self.focused || self.pressed || self.dragged
    }

    /// The M3E-precedence overlay opacity: `dragged > pressed > focused >
    /// hovered` — the single highest-priority active state's opacity, or
    /// `0.0` if none are active. The new documented default; see the
    /// [module docs](self) for how this differs from
    /// [`Self::max_active_opacity`].
    pub fn resolve_opacity(&self) -> f32 {
        if self.dragged {
            DRAGGED_OPACITY
        } else if self.pressed {
            PRESSED_OPACITY
        } else if self.focused {
            FOCUS_OPACITY
        } else if self.hovered {
            HOVER_OPACITY
        } else {
            0.0
        }
    }

    /// The maximum of every active state's opacity —
    /// [`crate::state_layer::StateLayer`]'s original resolver, kept for its
    /// back-compat contract. See the [module docs](self).
    pub fn max_active_opacity(&self) -> f32 {
        let mut opacity: f32 = 0.0;
        if self.hovered {
            opacity = opacity.max(HOVER_OPACITY);
        }
        if self.focused {
            opacity = opacity.max(FOCUS_OPACITY);
        }
        if self.pressed {
            opacity = opacity.max(PRESSED_OPACITY);
        }
        if self.dragged {
            opacity = opacity.max(DRAGGED_OPACITY);
        }
        opacity
    }
}

/// A named reference into `crate::tokens::motion_scheme()`'s six spring
/// presets. A [`PressScaleSpec`] carries this id rather than a raw
/// `SpringDesc` so a widget's event-pass code — which never reads a theme,
/// see `docs/CODE_STANDARDS.md`'s Theming conventions — can still name a
/// resolvable spring by its motion-scheme slot; [`Self::resolve`] reads
/// this crate's own compile-time token table, not a `Theme` recovered from
/// paint/layout context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressSpringId {
    FastSpatial,
    FastEffects,
    DefaultSpatial,
    DefaultEffects,
    SlowSpatial,
    SlowEffects,
}

impl PressSpringId {
    /// Resolve to the concrete [`SpringDesc`] this id names, read from
    /// `crate::tokens::motion_scheme()`.
    pub fn resolve(self) -> SpringDesc {
        let m = crate::tokens::motion_scheme();
        match self {
            Self::FastSpatial => m.fast_spatial.into(),
            Self::FastEffects => m.fast_effects.into(),
            Self::DefaultSpatial => m.default_spatial.into(),
            Self::DefaultEffects => m.default_effects.into(),
            Self::SlowSpatial => m.slow_spatial.into(),
            Self::SlowEffects => m.slow_effects.into(),
        }
    }
}

/// The optional press-scale spring spec a tappable surface may opt into:
/// the target scale to spring toward while pressed, and which named spring
/// drives it. Mirrors the reference's `M3ETappable.pressedScale` + `.spring`
/// pair. See the [module docs](self) for why this is spec data only, with
/// no driving loop of its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressScaleSpec {
    /// The scale factor to spring toward while pressed.
    /// [`Self::DISABLED_SCALE`] disables press-scale entirely.
    pub target_scale: f64,
    /// Which named `crate::tokens::motion_scheme()` spring drives the
    /// scale animation.
    pub spring: PressSpringId,
}

impl PressScaleSpec {
    /// The `target_scale` value that disables press-scale (mirrors the
    /// reference's `pressedScale: 1`).
    pub const DISABLED_SCALE: f64 = 1.0;

    /// Whether this spec actually springs the surface
    /// (`target_scale != `[`Self::DISABLED_SCALE`]).
    pub fn is_active(&self) -> bool {
        self.target_scale != Self::DISABLED_SCALE
    }
}

impl Default for PressScaleSpec {
    /// Disabled (`target_scale == 1.0`), spring id irrelevant while
    /// inactive — [`Self::DISABLED_SCALE`] with [`PressSpringId::FastSpatial`]
    /// as an arbitrary but harmless placeholder.
    fn default() -> Self {
        Self {
            target_scale: Self::DISABLED_SCALE,
            spring: PressSpringId::FastSpatial,
        }
    }
}

/// A haptic feedback intensity signal, mirroring the reference's
/// `M3EHapticFeedback` (`none`/`light`/`medium`/`heavy`) plus a
/// `SliderTick` variant for a discrete stepped control's tick — the
/// reference's separate `M3EHaptics.selection()` entry point, folded into
/// this one closed vocabulary rather than left as a same-numbered alias of
/// `Light`, since a caller may want to route it to a different platform
/// primitive (e.g. iOS's `UISelectionFeedbackGenerator` rather than an
/// impact generator).
///
/// A **closed** enum (not `#[non_exhaustive]`) — this crate owns the whole
/// vocabulary a hook needs to match on exhaustively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HapticSignal {
    /// No haptic feedback — a hook seeing this should treat it as a no-op,
    /// same as firing nothing at all.
    None,
    /// Light tap feedback for subtle interactions.
    Light,
    /// Medium impact for a standard press.
    Medium,
    /// Heavy impact for a significant action.
    Heavy,
    /// A discrete stepped control's tick (e.g. a slider crossing a step, a
    /// segmented picker landing on a new segment).
    SliderTick,
}

/// The set-once hook cell backing [`MaterialHaptics`]. Private: all access
/// goes through [`MaterialHaptics::set`]/[`MaterialHaptics::fire`].
static HAPTIC_HOOK: OnceLock<fn(HapticSignal)> = OnceLock::new();

/// The default hook: does nothing. In effect whenever
/// [`MaterialHaptics::fire`] runs before any [`MaterialHaptics::set`] call
/// has won.
fn noop_hook(_signal: HapticSignal) {}

/// The design system's pluggable, process-global haptics seam. Carries no
/// state of its own — every method is a plain associated function over one
/// process-wide hook cell. See the [module docs](self) for why this crate
/// reaches no `frust-haptics` dependency itself.
pub struct MaterialHaptics;

impl MaterialHaptics {
    /// Install the process-wide haptics hook.
    ///
    /// **Timing contract:** intended to be called once, from an app's own
    /// `install()`-era setup (mirroring how [`crate::install`] seeds the
    /// default theme) — before any widget can call [`Self::fire`]. The
    /// underlying cell is set-once: the *first* successful call wins, and
    /// every later call is rejected outright — a hook swap (last-wins) is
    /// deliberately **not** supported, so there is no way to uninstall or
    /// replace a hook once one has won. Returns `true` iff this call
    /// installed the hook, `false` if a hook was already installed and this
    /// one was rejected.
    ///
    /// # Wiring to `frust-haptics`
    ///
    /// `frust-material` never depends on `frust-haptics` (or any other
    /// plugin) itself — the design-system charter forbids a plugin-tier
    /// crate depending on a sibling plugin (`docs/PLUGINS_ARCHITECTURE.md`'s
    /// Layer Dependencies). An app that wants real haptic feedback depends
    /// on `frust-haptics` itself and wires it here — this example is
    /// `ignore`d (not `no_run`) because `frust-haptics` is intentionally
    /// absent from this crate's own dependency graph, including as a
    /// dev-dependency, so it cannot compile in this doctest:
    ///
    /// ```ignore
    /// // In the app's own crate — frust-haptics is *its* dependency, never
    /// // frust-material's.
    /// fn install_haptics() {
    ///     frust_material::MaterialHaptics::set(|signal| {
    ///         use frust_material::HapticSignal;
    ///         use frust_haptics::{HapticEffect, Haptics};
    ///         let effect = match signal {
    ///             HapticSignal::None => return,
    ///             HapticSignal::Light => HapticEffect::ImpactLight,
    ///             HapticSignal::Medium => HapticEffect::ImpactMedium,
    ///             HapticSignal::Heavy => HapticEffect::ImpactHeavy,
    ///             HapticSignal::SliderTick => HapticEffect::SelectionClick,
    ///         };
    ///         let _ = Haptics::perform(effect);
    ///     });
    /// }
    /// ```
    pub fn set(hook: fn(HapticSignal)) -> bool {
        HAPTIC_HOOK.set(hook).is_ok()
    }

    /// Fire `signal` through the installed hook — a silent no-op if none
    /// has been installed yet (the default; see the [module docs](self)).
    pub fn fire(signal: HapticSignal) {
        let hook = HAPTIC_HOOK.get().copied().unwrap_or(noop_hook);
        hook(signal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_resolver_covers_all_sixteen_combinations() {
        for hovered in [false, true] {
            for focused in [false, true] {
                for pressed in [false, true] {
                    for dragged in [false, true] {
                        let state = InteractionState {
                            hovered,
                            focused,
                            pressed,
                            dragged,
                        };
                        let expected = if dragged {
                            DRAGGED_OPACITY
                        } else if pressed {
                            PRESSED_OPACITY
                        } else if focused {
                            FOCUS_OPACITY
                        } else if hovered {
                            HOVER_OPACITY
                        } else {
                            0.0
                        };
                        assert_eq!(
                            state.resolve_opacity(),
                            expected,
                            "hovered={hovered} focused={focused} pressed={pressed} dragged={dragged}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn precedence_and_max_agree_on_this_crates_monotonic_token_table() {
        // Documented in the module doc: given hover < focus == pressed <
        // dragged, precedence-selecting the highest-priority active state
        // always yields the same value as maxing every active state.
        for hovered in [false, true] {
            for focused in [false, true] {
                for pressed in [false, true] {
                    for dragged in [false, true] {
                        let state = InteractionState {
                            hovered,
                            focused,
                            pressed,
                            dragged,
                        };
                        assert_eq!(state.resolve_opacity(), state.max_active_opacity());
                    }
                }
            }
        }
    }

    #[test]
    fn setters_report_whether_the_flag_changed() {
        let mut state = InteractionState::new();
        assert!(state.set_hovered(true), "false -> true is a change");
        assert!(!state.set_hovered(true), "true -> true is not a change");
        assert!(state.set_hovered(false), "true -> false is a change");

        assert!(state.set_focused(true));
        assert!(state.set_pressed(true));
        assert!(state.set_dragged(true));
        assert!(state.is_active());
    }

    #[test]
    fn no_active_state_has_zero_opacity_and_is_inactive() {
        let state = InteractionState::new();
        assert_eq!(state.resolve_opacity(), 0.0);
        assert_eq!(state.max_active_opacity(), 0.0);
        assert!(!state.is_active());
    }

    #[test]
    fn press_spring_id_resolves_to_the_named_motion_scheme_preset() {
        let m = crate::tokens::motion_scheme();
        assert_eq!(PressSpringId::FastSpatial.resolve(), m.fast_spatial.into());
        assert_eq!(PressSpringId::FastEffects.resolve(), m.fast_effects.into());
        assert_eq!(
            PressSpringId::DefaultSpatial.resolve(),
            m.default_spatial.into()
        );
        assert_eq!(
            PressSpringId::DefaultEffects.resolve(),
            m.default_effects.into()
        );
        assert_eq!(PressSpringId::SlowSpatial.resolve(), m.slow_spatial.into());
        assert_eq!(PressSpringId::SlowEffects.resolve(), m.slow_effects.into());
    }

    #[test]
    fn press_scale_spec_is_active_reflects_target_scale() {
        let disabled = PressScaleSpec::default();
        assert!(!disabled.is_active());

        let active = PressScaleSpec {
            target_scale: 0.95,
            spring: PressSpringId::FastSpatial,
        };
        assert!(active.is_active());
    }

    // -- MaterialHaptics ------------------------------------------------
    //
    // `HAPTIC_HOOK` is one process-wide `OnceLock` per the set-once
    // contract `MaterialHaptics::set` documents, so every assertion that
    // depends on install/rejection ordering lives in this single test —
    // splitting it across multiple `#[test]` fns would make the outcome
    // depend on which one the test harness happens to run first (they all
    // share one process).

    static RECORDED: std::sync::Mutex<Vec<HapticSignal>> = std::sync::Mutex::new(Vec::new());

    fn recording_hook(signal: HapticSignal) {
        RECORDED.lock().unwrap().push(signal);
    }

    fn other_recording_hook(signal: HapticSignal) {
        // A second, distinct hook — used only to prove it never wins once
        // the first `set` has already succeeded.
        RECORDED.lock().unwrap().push(signal);
        RECORDED.lock().unwrap().push(HapticSignal::None);
    }

    #[test]
    fn haptics_default_noop_then_install_wins_and_rejects_second_set() {
        // Default: firing before any install must not panic. There is
        // nothing else to assert about a no-op.
        MaterialHaptics::fire(HapticSignal::Medium);

        assert!(
            MaterialHaptics::set(recording_hook),
            "the first set() must install and win"
        );

        MaterialHaptics::fire(HapticSignal::Light);
        assert_eq!(*RECORDED.lock().unwrap(), vec![HapticSignal::Light]);

        assert!(
            !MaterialHaptics::set(other_recording_hook),
            "a second set() after the first must be rejected"
        );

        RECORDED.lock().unwrap().clear();
        MaterialHaptics::fire(HapticSignal::Heavy);
        assert_eq!(
            *RECORDED.lock().unwrap(),
            vec![HapticSignal::Heavy],
            "the rejected set() must not have replaced the installed hook \
             (a plain single push, not other_recording_hook's push-then-None)"
        );
    }
}
