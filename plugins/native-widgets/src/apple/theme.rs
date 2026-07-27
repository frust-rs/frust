//! Theme ladder L1's Apple half (native-widgets Phase 2, p2-04): mirrors
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
//! # Baked at construction, not live — the same documented limitation
//!
//! Exactly like Android's L1 (`crate::android::theme`'s own "Baked at
//! construction, not live" section): [`apply_user_interface_style`] is
//! called only from `create_control`, never from `update_control`. A live
//! in-place brightness toggle does not recreate the control, so the
//! override — and therefore how the *platform's own* chrome (nothing this
//! crate's L2 setters explicitly colour) resolves — stays pinned to
//! whichever brightness the control was first created under. Mirroring the
//! *contract*, not inventing a stronger one: PLAN.md's Theme-mismatch risk
//! row records this the same way for both platforms.

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
/// per-view property set after construction rather than a Context supplied
/// to one, and why it is called exactly once, from `create_control` only.
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
