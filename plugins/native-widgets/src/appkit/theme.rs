//! Theme ladder L1's macOS half: pin every hosted control's `NSAppearance` to
//! the APP's `Brightness`, not the Mac's system appearance — the AppKit twin
//! of `crate::apple::theme`'s `overrideUserInterfaceStyle` (that module's doc
//! is the reference account; this one records only what differs).
//!
//! # `NSView.appearance`, set after construction
//!
//! Like UIKit, AppKit has no "construct against a themed context" step: every
//! control resolves its dynamic `NSColor`s and its bezel/track/thumb drawing
//! from its `effectiveAppearance` at DRAW time, and `NSView.appearance` (the
//! `NSAppearanceCustomization` protocol every `NSView` adopts) is the
//! per-view knob that pins it. Setting it applies to the whole subtree, so a
//! composite's children (`crate::demo`'s card) follow their root. With the
//! property unset (`nil`) a view inherits its window's — i.e. the Mac's
//! system — appearance, which is exactly the leak L1 closes: an app whose
//! theme is Light must draw light controls on a Mac running Dark Mode.
//!
//! [`apply_brightness`] picks `NSAppearanceNameDarkAqua` or
//! `NSAppearanceNameAqua` from the same [`crate::controls::DARK`] wire bit the
//! other two arms read ([`brightness_is_dark`]) — carried not only by the six
//! controls' own `params_for` (`crate::api::builders`) but by every
//! `NativeComponent` slot's params too, folded in by
//! `crate::component::component_params` from `crate::api::mount`'s
//! `ambient_dark` (the same `use_context::<Theme>()` the six builders read).
//! Without that, a component root such as `crate::demo::DemoCard` would carry
//! no `dark` key at all and stay permanently pinned to the light appearance
//! regardless of the app's theme.
//!
//! # Re-pinned on EVERY update, like iOS — never baked like Android
//!
//! `NSView.appearance` is a plain mutable property, so there is nothing
//! forcing it to be baked at construction the way Android's
//! `createConfigurationContext` is. It is applied on create **and** on every
//! `update_params`: `crate::apple::theme`'s on-device gate found the bug
//! a create-only pin causes — a control culled off-screen and later
//! RECREATED adopts whatever brightness is current at recreate time while its
//! never-culled siblings keep the one they were born under, so the two
//! diverge (the reported repro drew a recreated switch's thumb
//! accent-on-accent, i.e. invisible). Re-pinning per update makes "which
//! appearance is this control pinned to" a function of the CURRENT theme, and
//! makes an in-place brightness toggle re-theme platform chrome live. Do not
//! "restore symmetry" with Android's construction-time bake — the asymmetry
//! belongs to the platforms (`docs/PLUGINS_CODE_STANDARDS.md`'s Plugin
//! Conventions). `Unchanged` updates re-pin too: brightness lives on the wire,
//! not in any control's diffed `Props`, so a props-equal update can still
//! carry a new brightness.
//!
//! # The call site: inside `AppKitFactory` itself, like iOS
//!
//! Both Apple arms now pin from inside the one factory: the iOS arm from its
//! typed `create_control`/`update_control`, this arm from
//! `crate::appkit::factory`'s `create`/`update_control`
//! (`crate::appkit::factory`'s own module doc — *Theme ladder L1*). That
//! covers every slot the desktop host drives — the six controls, every
//! `NativeComponent`, and a dead-slot placeholder alike (pinning an empty
//! `NSView` is harmless) — with no separate wrapper and no second
//! registration: `crate::appkit::factory::ensure_registered` is the one
//! registration function.

use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSView,
};

use crate::controls::DARK;
use crate::runtime::Params;

/// Whether a slot's params requested the dark half of the active theme — read
/// straight off the raw wire ([`DARK`]), exactly like
/// `crate::apple::theme::brightness_is_dark`. Absent degrades to `false`
/// (light): appearance pinning is cosmetic, never a reason to fail a slot.
pub(crate) fn brightness_is_dark(params_json: &str) -> bool {
    Params::new(params_json).flag(DARK).unwrap_or(false)
}

/// L1: pin `view`'s (and its whole subtree's) appearance to Dark Aqua or Aqua
/// — see the module doc for why this is a per-view property and why it runs on
/// every update, not only at create.
///
/// `appearanceNamed:` answering `nil` (it does not for the two standard
/// names) degrades to clearing the pin, i.e. following the window — logged,
/// never a failure.
pub(crate) fn apply_brightness(view: &NSView, dark: bool) {
    // SAFETY: both names are AppKit's own immutable `NSString` constants,
    // initialised by the linked framework before any Rust code can run; they
    // are only read here, never written.
    let name = unsafe {
        if dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    };
    let appearance = NSAppearance::appearanceNamed(name);
    if appearance.is_none() {
        log::warn!(
            "frust-native-widgets: macOS NSAppearance::appearanceNamed({name}) returned nil — \
             the view follows its window's appearance"
        );
    }
    view.setAppearance(appearance.as_deref());
    log::debug!(
        "frust-native-widgets: macOS L1 appearance -> {}",
        if dark { "DarkAqua" } else { "Aqua" }
    );
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
