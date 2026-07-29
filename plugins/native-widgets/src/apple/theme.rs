//! Theme ladder L1's Apple half: mirrors
//! `crate::android::theme`'s L1 (that module's own doc — "why a wrapped
//! Context" — is the reference account of the *problem* L1 solves; this
//! module is the same fix through UIKit's own, genuinely different
//! mechanism).
//!
//! # Why there is no `createConfigurationContext` analogue here
//!
//! Android's L1 wraps the `Context` a control is *constructed against*,
//! because a framework `Button`/`Switch`/`SeekBar` resolves its
//! platform-owned chrome (ripple colour, thumb/track resting colour) from
//! that Context's `Configuration` at construction time — there is no other
//! hook. UIKit has no equivalent "construct against a themed context" step:
//! every control is built with `UIButton::buttonWithType`/`UILabel::new`/etc,
//! taking only a [`MainThreadMarker`] (`crate::apple::ctx`'s own doc: "the
//! ObjC runtime is globally reachable"). Instead, UIKit resolves a *dynamic*
//! `UIColor`'s and every semantic-colour drawable's on-screen value from the
//! view's own `traitCollection` at DRAW time, and
//! `UIView.overrideUserInterfaceStyle` is the per-view knob that pins what
//! that trait collection reports — set once, after construction, rather than
//! supplied to a constructor.
//!
//! So this arm's L1 is: read the same [`crate::controls::DARK`] wire bit
//! Android's [`brightness_is_dark`] reads (this module's own copy, since
//! there is no cross-target module either arm could share without pulling
//! `crate::controls` into a `#[cfg]`-neutral position it does not need to be
//! in — mirroring `crate::android::fonts` vs `crate::apple::fonts`'s
//! identical per-platform-module duplication), then set
//! [`apply_user_interface_style`] on the freshly created control's own
//! top-level `UIView` — `crate::apple::factory::create_control` is the one
//! generic call site every control's `createView` passes through, the exact
//! same single-choke-point shape `crate::android::create_control` uses for
//! its own context-wrapping (`crate::android::create_control`'s own doc).
//!
//! `overrideUserInterfaceStyle` **is available since iOS 13** (Apple's
//! documentation, retrieved 2026-07-27) and applies to the whole view
//! subtree it's set on — every descendant (a `UIButton`'s internal
//! `titleLabel`, a `UISwitch`'s thumb/track drawables) resolves its own
//! dynamic colours against the override without needing the property set
//! again.
//!
//! # LIVE on this arm — iOS does not share Android's L1 limitation
//!
//! Android's L1 genuinely must bake: `createConfigurationContext` yields a
//! `Context` consumed *at View construction*, so a live brightness toggle
//! cannot re-resolve a control's platform chrome without recreating it
//! (`crate::android::theme`'s "Baked at construction, not live").
//!
//! **`overrideUserInterfaceStyle` has no such constraint** — it is a plain
//! mutable `UIView` property, settable at any time, and setting it
//! re-resolves the whole subtree. So [`apply_user_interface_style`] is called
//! from **both** `create_control` and `update_control`.
//!
//! This module was originally written the Android way, mirroring that
//! contract rather than the platform, and **an on-device gate run found the
//! bug that caused**: a control culled off-screen and later RECREATED adopts
//! whatever brightness is current at recreate time, while its never-culled
//! siblings keep the one they were born under — so the two diverge. On the
//! reported repro (launch dark, switch to light, run the 50-slot scroll
//! stress, scroll back) the recreated `UISwitch` returned Light-pinned among
//! Dark-pinned peers; toggling back to dark then drew its default thumb
//! accent-on-accent, i.e. invisible.
//!
//! Re-pinning on update fixes that by making "which brightness is this
//! control pinned to" a function of the CURRENT theme rather than of when the
//! control happened to be constructed. It also makes an in-place brightness
//! toggle re-theme platform chrome live on iOS — strictly better than the
//! Android arm, whose baked-at-construction approximation has no iOS
//! counterpart. **Do not "restore symmetry" by reverting this**;
//! the asymmetry belongs to the platforms, not to a defect.

use objc2_ui_kit::{UIUserInterfaceStyle, UIView};

use crate::controls::DARK;
use crate::runtime::Params;

/// Whether a slot's create params requested the dark half of the active
/// theme — read straight off the raw wire ([`DARK`]) before any per-control
/// typed `Props` decode runs, exactly like
/// `crate::android::theme::brightness_is_dark`. Absent (an older api layer,
/// or params from a control kind this task didn't fold theme tokens into)
/// degrades to `false` (light) rather than an error — night-qualification is
/// a cosmetic improvement, never a reason to fail control creation.
pub(crate) fn brightness_is_dark(params_json: &str) -> bool {
    Params::new(params_json).flag(DARK).unwrap_or(false)
}

/// L1: pin `view`'s (and its whole subtree's) resolved brightness via
/// `overrideUserInterfaceStyle` — see the module doc for why this is a
/// per-view property set after construction rather than a `Context` supplied
/// to one, and why it is called from **both** `create_control` and
/// `update_control` on this arm (the recreate-divergence fix).
pub(crate) fn apply_user_interface_style(view: &UIView, dark: bool) {
    let style = if dark {
        UIUserInterfaceStyle::Dark
    } else {
        UIUserInterfaceStyle::Light
    };
    view.setOverrideUserInterfaceStyle(style);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::with_identity;

    #[test]
    fn brightness_reads_the_dark_flag_off_the_raw_wire() {
        let dark = with_identity("button", 1, "\"dark\":true");
        assert!(brightness_is_dark(&dark));

        let light = with_identity("button", 1, "\"dark\":false");
        assert!(!brightness_is_dark(&light));
    }

    #[test]
    fn an_absent_dark_flag_degrades_to_light() {
        let absent = with_identity("button", 1, "\"text\":\"hi\"");
        assert!(!brightness_is_dark(&absent));
    }
}
