//! Theme + appearance: the default-theme precedence ladder (app override >
//! design-system-seeded default > the built-in `Theme::neutral()` fallback),
//! the reduced-motion floor, and the three arms that drive them — the
//! construction seed, `frust_set_appearance`/`frust_set_reduce_motion`, and the
//! per-frame `set_app_theme`/`clear_app_theme` poll.
//!
//! This module owns the whole ladder on purpose: the source-scan conformance
//! test `crates/frust/tests/theme_ladder_conformance.rs` pins every helper here
//! byte-identical to the desktop shell's copy (the only one covered by host-run
//! unit tests) and counts each arm's call site, so the helpers and their callers
//! must live in one file.

use frust_reactive::{ReactiveRuntime, provide_context};
use frust_shell_common::{default_theme, effective_brightness_for_platform_change};
use frust_theme::{Brightness, Theme};

use super::IosAppHandle;

/// The base [`Theme`] this shell seeds itself from: the design-system-supplied
/// default (`frust_shell_common::set_default_theme`, read back through
/// [`default_theme`]) when a plugin seeded one, else the shell's own built-in
/// fallback.
///
/// A seeded default supplies only the *starting point*: unlike an app-forced
/// override (`set_app_theme`) it does not pin brightness — every call site
/// still derives `brightness` from the platform's own preference
/// (`frust_set_appearance` → [`IosAppHandle::platform_brightness`]) against
/// this base, so a design-system default keeps following system dark mode.
///
/// The fallback is deliberately the design-language-free
/// [`Theme::neutral`] — system fonts, no bundled font bytes: a shell names no
/// design system of its own, so an app that installs none gets the neutral
/// floor rather than someone's brand. A design system supplies both halves
/// itself (its base theme through `set_default_theme`, its font bytes through
/// `frust_shell_common::font_registry::register_app_fonts`).
///
/// Takes the slot's value as an argument rather than reading the process-global
/// itself, so the fallback ladder is unit-testable without touching a
/// process-wide slot that has no reset; every call site passes
/// [`default_theme()`](default_theme).
fn base_theme(seeded: Option<Theme>) -> Theme {
    seeded.unwrap_or_else(Theme::neutral)
}

/// The construction-time seed: [`base_theme`] against the live process-global
/// default slot — this shell's single seed site, called from
/// [`IosAppHandle::new`](super::IosAppHandle::new).
///
/// Extracted so the seed composition (`base_theme(default_theme())`, the one
/// thing no ladder unit test can observe — they supply `base_theme`'s argument
/// themselves) stays in this module beside the helpers it composes, where
/// `theme_ladder_conformance.rs`'s scan pins it.
pub(super) fn seed_theme() -> Theme {
    base_theme(default_theme())
}

/// The theme a cleared app-theme override reverts to: the seeded base
/// ([`base_theme`] — a design system's `set_default_theme`, else the built-in
/// fallback) at the platform's *current* brightness
/// ([`IosAppHandle::platform_brightness`]), never the cleared override's own
/// pinned one.
///
/// Extracted so the `clear_app_theme` arm in
/// [`IosAppHandle::poll_theme_override`] and its unit tests run one
/// implementation: a test that recomputed this in its own
/// body would stay green if the arm regressed to an unconditional
/// `Theme::neutral()` — the exact regression this ladder exists to
/// prevent. `crates/frust/tests/theme_ladder_conformance.rs` pins this body
/// identical to the desktop shell's twin, whose unit tests DO run in a host
/// `cargo test --workspace` (this module's cannot — see its own docs).
fn reverted_theme(seeded: Option<Theme>, platform: Brightness) -> Theme {
    let mut theme = base_theme(seeded);
    theme.brightness = platform;
    theme
}

/// The active-theme decision for one `ThemeOverrideWatcher::poll` result —
/// the precedence ladder's top two rungs as one pure function, shared by the
/// per-frame poll arm in [`IosAppHandle::poll_theme_override`] and its unit
/// tests.
///
/// Returns `None` when the poll reported no change (the shell leaves its theme
/// alone), else the new active theme paired with whether an app-forced override
/// is now pinning it ([`IosAppHandle::theme_override_active`]).
///
/// `seeded`/`platform` are suppliers rather than values because only the
/// cleared-override arm needs them: reading the process-global default slot
/// ([`default_theme`] — a `Mutex` lock plus a whole-`Theme` clone) would
/// otherwise become per-frame cost on every `CADisplayLink` tick, for a poll
/// that reports "nothing changed" on all but a handful of frames.
fn theme_after_override_poll(
    polled: Option<Option<Theme>>,
    seeded: impl FnOnce() -> Option<Theme>,
    platform: impl FnOnce() -> Brightness,
) -> Option<(Theme, bool)> {
    match polled {
        // `set_app_theme`: the forced theme wins wholesale — neither the seeded
        // default nor the platform's brightness is even consulted (the
        // override-wins rule).
        Some(Some(theme)) => Some((theme, true)),
        // `clear_app_theme`: back to the seeded base at the platform's own
        // current brightness.
        Some(None) => Some((reverted_theme(seeded(), platform()), false)),
        None => None,
    }
}

/// Re-derive `theme`'s brightness from a platform appearance report, honouring
/// the override-wins rule ([`effective_brightness_for_platform_change`]): an
/// app-forced override pins brightness, a design-system-seeded default does not
/// — `theme` still IS that base, so flipping it in place re-derives light/dark
/// against the design system's own tokens.
///
/// Extracted for the same reason as [`reverted_theme`]: the
/// `frust_set_appearance` arm ([`IosAppHandle::set_appearance`]) and the
/// seed-ladder tests share one implementation.
fn follow_platform_brightness(theme: &mut Theme, override_active: bool, platform: Brightness) {
    theme.brightness =
        effective_brightness_for_platform_change(override_active, theme.brightness, platform);
}

/// The reduced-motion **floor** rule: the OS's accessibility preference
/// (`UIAccessibility.isReduceMotionEnabled`, reported through
/// `frust_set_reduce_motion`) is OR'd over the active theme's own authored
/// `motion.reduce_motion` token — never assigned over it.
///
/// Why a floor rather than the brightness rule's override-wins ladder
/// ([`follow_platform_brightness`]): reduced motion is an accessibility
/// *guarantee*, not a style preference, so a live OS toggle must reach a theme
/// an app forced with `set_app_theme` (a catalog's design-language switcher,
/// say) instead of being pinned out of it. Symmetrically, an app that
/// deliberately authored `reduce_motion: true` keeps it while the OS setting is
/// off — neither side can un-reduce what the other asked for, which is the one
/// direction it is never safe to get wrong.
///
/// Deliberately NOT one of the ladder helpers pinned byte-identical across all
/// three shells by `crates/frust/tests/theme_ladder_conformance.rs`: desktop
/// has no reduced-motion source at all (see `MotionScheme::reduce_motion`), so
/// only the two mobile shells carry this.
fn effective_reduce_motion(authored: bool, os: bool) -> bool {
    authored || os
}

impl IosAppHandle {
    /// `frust_set_appearance`: flip the theme's brightness and re-push it to
    /// both delivery paths (mirrors the desktop shell's `apply_theme`).
    ///
    /// Runs the `provide_context` re-provide under the process-wide root
    /// [`ReactiveRuntime`]'s owner (fetched fresh here, since — unlike
    /// [`Self::new`] — this call arrives on its own C-ABI entry, not nested
    /// inside `create_handle`'s `with_owner` wrap). No explicit redraw is
    /// scheduled — the continuous `CADisplayLink` loop already repaints every
    /// tick.
    ///
    /// Override-wins rule: while an app-forced theme override is
    /// active, this platform-appearance report must not flip brightness (see
    /// `effective_brightness_for_platform_change`). A seeded *default* is
    /// deliberately not pinned that way: [`Self::theme`] still IS that base
    /// ([`base_theme`] seeded it and nothing replaced it), so flipping its
    /// brightness in place re-derives light/dark from the design system's own
    /// theme.
    pub(crate) fn set_appearance(&mut self, dark: bool) {
        let platform = match crate::ffi_support::appearance_from_dark(dark) {
            crate::ffi_support::Appearance::Dark => Brightness::Dark,
            crate::ffi_support::Appearance::Light => Brightness::Light,
        };
        self.platform_brightness = platform;
        follow_platform_brightness(&mut self.theme, self.theme_override_active, platform);
        self.push_theme();
        // Frame-gate latch: an OS appearance flip must force the next ready
        // frame to Run (see the `appearance_dirty` field doc).
        self.appearance_dirty = true;
    }

    /// `frust_set_reduce_motion`: apply the platform's reduced-motion
    /// accessibility preference to the active theme's `MotionScheme` and
    /// re-push it to both delivery paths — the reduced-motion twin of
    /// [`Self::set_appearance`], travelling the identical transport (a C-ABI
    /// entry → a `Theme` edit → [`Self::push_theme`]) over a different sensor.
    ///
    /// `reduce` is Swift's `UIAccessibility.isReduceMotionEnabled` read; it is
    /// not a `UITraitCollection` trait, so Swift sources it from
    /// `UIAccessibility.reduceMotionStatusDidChangeNotification` (plus a
    /// re-read on foreground) rather than `traitCollectionDidChange`.
    ///
    /// The OS value is a floor over the theme's own authored token, not a
    /// replacement ([`effective_reduce_motion`]) — including while an app-forced
    /// override is active, unlike the brightness path's override-wins rule
    /// (that helper's doc has the reasoning). No explicit redraw is scheduled:
    /// the continuous `CADisplayLink` loop already repaints every tick, and
    /// `appearance_dirty` keeps the frame gate from skipping the tick that
    /// carries the change (and survives a toggle made while backgrounded — the
    /// latch is only taken past the pause/ready gate).
    ///
    /// Unchanged-value calls return early, unlike [`Self::set_appearance`]:
    /// Swift re-pushes this on every foreground (and the notification can fire
    /// more than once per real change), where a config-change-driven appearance
    /// push is rare — without the guard every resume would pay a `push_theme`'s
    /// forced relayout for nothing. Sound because `os_reduce_motion` is the
    /// only writer of the token outside a whole-`Theme` install, and that
    /// install re-applies the floor itself.
    pub(crate) fn set_reduce_motion(&mut self, reduce: bool) {
        if self.os_reduce_motion == reduce {
            return;
        }
        self.os_reduce_motion = reduce;
        self.theme.motion.reduce_motion =
            effective_reduce_motion(self.authored_reduce_motion, reduce);
        self.push_theme();
        self.appearance_dirty = true;
    }

    /// The per-frame app-facing theme-override poll
    /// (`frust::set_app_theme`/`clear_app_theme`), run from [`Self::frame`]
    /// before the pause/ready gate — theme delivery needs no renderer, so it
    /// stays in sync even while backgrounded/not-ready. Returns whether the
    /// active theme changed, which the caller folds into the frame gate's
    /// `theme_or_appearance_changed` input.
    ///
    /// Reverting an override lands on the base this shell seeded itself
    /// from — the design-system default when one was supplied, else the
    /// built-in fallback — with brightness re-derived from the platform's
    /// last reported preference rather than inherited from the cleared
    /// override; that whole ladder lives in [`theme_after_override_poll`] so
    /// this arm and its unit tests share one implementation. The seeded
    /// supplier stays lazy: an unchanged poll never reads the
    /// process-global slot.
    pub(super) fn poll_theme_override(&mut self) -> bool {
        let platform_brightness = self.platform_brightness;
        if let Some((mut theme, override_active)) =
            theme_after_override_poll(self.theme_override.poll(), default_theme, || {
                platform_brightness
            })
        {
            // A whole-`Theme` swap re-bases the reduced-motion floor: the
            // incoming theme carries its own authored token, so record that and
            // re-apply the OS report over it. Without this, a `set_app_theme`/
            // `clear_app_theme` would silently un-reduce motion while the
            // platform setting is still on (`effective_reduce_motion`).
            self.authored_reduce_motion = theme.motion.reduce_motion;
            theme.motion.reduce_motion =
                effective_reduce_motion(self.authored_reduce_motion, self.os_reduce_motion);
            self.theme = theme;
            self.theme_override_active = override_active;
            self.push_theme();
            return true;
        }
        false
    }

    /// Push the current [`Self::theme`] to both delivery paths — boxed
    /// type-erased into the render root (`AppTree::set_theme`) and
    /// re-`provide_context`ed under the process-wide root
    /// [`ReactiveRuntime`]'s owner for app-side `use_context` reads.
    ///
    /// This IS the shared theme-delivery body: [`Self::set_appearance`]
    /// (after it resolves the new brightness), [`Self::set_reduce_motion`],
    /// [`Self::poll_theme_override`] and [`Self::frame`]'s late-font drain all
    /// call it, so a forced override and a live appearance change go through
    /// one code path.
    pub(super) fn push_theme(&mut self) {
        self.app.set_theme(Box::new(self.theme.clone()));
        let theme = self.theme.clone();
        match ReactiveRuntime::get() {
            Some(rt) => rt.with_owner(|| provide_context(theme)),
            None => provide_context(theme),
        }
    }
}

/// Unit tests for the default-theme seed ladder.
///
/// This module is inside the `#[cfg(target_os = "ios")]` `app` module, so it
/// only compiles/runs for an iOS target — the ladder references `frust_theme`'s
/// [`Theme`] and `frust_shell_common`, both iOS-gated dependencies of this crate
/// by deliberate design (see `Cargo.toml`), so it cannot be a host test the way
/// [`crate::ffi_support`]'s pure helpers are — worse, the documented iOS
/// compile gate (`cargo check --target aarch64-apple-ios-sim -p frust-ui`) never
/// builds test cfg either, so nothing below is even type-checked without an
/// explicit `--all-targets`.
///
/// The theme ladder is therefore guarded off-device by two things that DO run
/// on every `cargo test --workspace`: the desktop shell's twin of each ladder
/// test (`frust-shell-desktop`'s `app_handler::tests`), and
/// `crates/frust/tests/theme_ladder_conformance.rs`, whose source scan pins
/// this shell's arms to the extracted helpers and pins those helpers' bodies
/// identical to the desktop copies the twin tests exercise.
#[cfg(test)]
mod tests {
    use super::{
        base_theme, effective_reduce_motion, follow_platform_brightness, theme_after_override_poll,
    };
    use frust_theme::{Brightness, DesignLanguage, Theme};

    // Every assertion below drives the SAME ladder helpers the production seed
    // ([`super::seed_theme`], called by `IosAppHandle::new`), appearance
    // (`frust_set_appearance`) and override-poll
    // ([`IosAppHandle::poll_theme_override`]) arms call — a test that
    // recomputed the ladder in its own body would stay green if one of those
    // arms regressed to an unconditional `Theme::neutral()`. What no assertion
    // here can see is how an arm *composes* those helpers (a seed site passing
    // `None` instead of `default_theme()` drives the same `base_theme`).
    //
    // These do not run in a host `cargo test --workspace` (see this module's
    // own docs), so they are not this ladder's only guard:
    // `crates/frust/tests/theme_ladder_conformance.rs` pins each arm's call
    // site — including the seed site's `base_theme(default_theme())` — AND pins
    // these helpers' bodies identical to the desktop shell's, whose twin of
    // every test below does run on the host.

    /// A stand-in for the base theme a design-system plugin seeds through
    /// `set_default_theme` — the desktop twin's helper, verbatim.
    ///
    /// Deliberately **not** `Theme::neutral()`: the ladder's whole point is
    /// that a seeded base displaces the shell's built-in floor, so every
    /// assertion below that spells `assert_ne!(.., Theme::neutral())` — or
    /// reads a seeded value back — needs a theme the floor cannot be confused
    /// with. The visible edit is the design-language tag, the one field a
    /// third-party design system is expected to claim; every token stays the
    /// neutral baseline's, so no catalog is named here.
    fn seeded_design_system_theme() -> Theme {
        Theme::builder(Theme::neutral())
            .design_language(DesignLanguage::Custom("test-design-system"))
            .build()
    }

    #[test]
    fn an_unseeded_shell_starts_on_the_builtin_fallback_at_the_platform_brightness() {
        // Behavior 1. Nothing seeded: the seed site lands on the shell's own
        // built-in, design-language-free floor...
        let mut theme = base_theme(None);
        assert_eq!(theme, Theme::neutral());

        // ...and Swift's follow-up `frust_set_appearance` drives brightness, so
        // the fallback's own starting brightness never leaks onto a
        // dark-preference device.
        follow_platform_brightness(&mut theme, false, Brightness::Light);
        assert_eq!(theme, Theme::neutral().with_brightness(Brightness::Light));
        follow_platform_brightness(&mut theme, false, Brightness::Dark);
        assert_eq!(theme, Theme::neutral().with_brightness(Brightness::Dark));
    }

    #[test]
    fn a_seeded_default_is_the_base_and_still_follows_platform_brightness() {
        // Behavior 2. A design system's `set_default_theme` supplies the base...
        let seeded = seeded_design_system_theme();
        let mut theme = base_theme(Some(seeded.clone()));
        assert_eq!(theme, seeded);
        // ...in place of the built-in floor, not layered over it.
        assert_ne!(theme, Theme::neutral());

        // ...and unlike an app-forced override it does NOT pin brightness: the
        // `set_appearance` arm keeps flipping the seeded base in place.
        follow_platform_brightness(&mut theme, false, Brightness::Dark);
        assert_eq!(theme, seeded.clone().with_brightness(Brightness::Dark));
        follow_platform_brightness(&mut theme, false, Brightness::Light);
        assert_eq!(theme, seeded.with_brightness(Brightness::Light));
    }

    #[test]
    fn an_app_theme_override_beats_a_seeded_default_and_pins_brightness() {
        // Behavior 3. The override poll's `Some(Some(theme))` arm takes the
        // forced theme wholesale. The suppliers panic rather than answer, which
        // proves more than an inequality could: the arm cannot even observe the
        // seeded default or the platform brightness, so no seeded value and no
        // device preference can influence what an override resolves to.
        let forced = Theme::neutral().with_brightness(Brightness::Light);
        let decided = theme_after_override_poll(
            Some(Some(forced.clone())),
            || panic!("an active override must not consult the seeded default"),
            || panic!("an active override must not consult the platform brightness"),
        );
        assert_eq!(decided, Some((forced, true)));

        // ...and it keeps winning against a *later* `frust_set_appearance` flip
        // (the override-wins rule), exactly where the seeded default of the
        // test above followed the platform instead. The distinction that must
        // survive here is brightness, not identity: `Light` forced against a
        // `Dark` device report.
        let mut active = Theme::neutral().with_brightness(Brightness::Light);
        follow_platform_brightness(&mut active, true, Brightness::Dark);
        assert_eq!(active.brightness, Brightness::Light);
    }

    #[test]
    fn clearing_an_override_reverts_to_the_seeded_default_not_the_builtin() {
        // Behavior 4. The `Some(None)` arm with a design system's default
        // seeded: the revert lands on THAT base at the device's last reported
        // brightness (`platform_brightness`) — not on the built-in fallback,
        // and not on the cleared override's pinned brightness.
        let seeded = seeded_design_system_theme();
        let decided =
            theme_after_override_poll(Some(None), || Some(seeded.clone()), || Brightness::Dark);
        assert_eq!(
            decided,
            Some((seeded.with_brightness(Brightness::Dark), false))
        );
        // Spelled out, since this is the arm the ladder exists for: a seeded
        // shell must NOT revert to the built-in fallback.
        assert_ne!(
            decided.map(|(theme, _)| theme),
            Some(Theme::neutral().with_brightness(Brightness::Dark))
        );
    }

    #[test]
    fn clearing_an_override_with_nothing_seeded_reverts_to_the_builtin() {
        // Behavior 5. The same arm with an empty slot: the built-in fallback at
        // the platform's brightness.
        let decided = theme_after_override_poll(Some(None), || None, || Brightness::Dark);
        assert_eq!(
            decided,
            Some((Theme::neutral().with_brightness(Brightness::Dark), false))
        );
    }

    #[test]
    fn a_poll_reporting_no_change_leaves_the_active_theme_alone() {
        // The `None` arm: no `set_app_theme`/`clear_app_theme` since the last
        // tick, so the shell must not touch its theme — and must not pay the
        // process-global slot read, which would otherwise run on every
        // `CADisplayLink` tick.
        let decided = theme_after_override_poll(
            None,
            || panic!("an unchanged poll must not read the process-global default slot"),
            || panic!("an unchanged poll must not read the platform brightness"),
        );
        assert_eq!(decided, None);
    }

    // --- the reduced-motion floor (`frust_set_reduce_motion`) ---------------

    #[test]
    fn the_os_reduced_motion_report_raises_and_lowers_an_unreduced_theme() {
        // The ordinary case: every shipped baseline authors `false`, so the
        // effective value tracks the OS setting in both directions.
        assert!(effective_reduce_motion(false, true));
        assert!(!effective_reduce_motion(false, false));
    }

    #[test]
    fn a_theme_authored_reduced_stays_reduced_while_the_os_setting_is_off() {
        // The floor's whole point: the OS report may only ever ADD reduction.
        // A theme built with `reduce_motion: true` (a `ThemeBuilder::map_motion`
        // app choice) must not be un-reduced by a device that has the setting
        // off.
        assert!(effective_reduce_motion(true, false));
        assert!(effective_reduce_motion(true, true));
    }

    #[test]
    fn the_floor_applies_to_an_app_forced_override_too() {
        // Unlike brightness, an active `set_app_theme` override does NOT pin
        // this: the frame arm re-bases `authored_reduce_motion` from the
        // incoming theme and re-applies the OS report over it, so a live
        // accessibility toggle still reaches a catalog app that forced its own
        // theme. This asserts the composition that arm performs.
        let forced = Theme::neutral();
        let authored = forced.motion.reduce_motion;
        assert!(!authored, "every shipped baseline authors `false`");
        let mut active = forced;
        active.motion.reduce_motion = effective_reduce_motion(authored, true);
        assert!(active.motion.reduce_motion);
    }
}
