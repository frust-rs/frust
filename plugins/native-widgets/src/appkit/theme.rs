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
//! other two arms read ([`brightness_is_dark`]).
//!
//! # Re-pinned on EVERY update, like iOS — never baked like Android
//!
//! `NSView.appearance` is a plain mutable property, so there is nothing
//! forcing it to be baked at construction the way Android's
//! `createConfigurationContext` is. It is applied on create **and** on every
//! `update_params`: `crate::apple::theme`'s on-device gate found the bug
//! a create-only pin causes (p2-05) — a control culled off-screen and later
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
//! # The call site: [`ThemedFactory`], one choke point for every slot
//!
//! The iOS arm pins from inside its factory's typed `create_control`/
//! `update_control`. This arm pins one layer out, in [`ThemedFactory`] — a
//! [`DesktopViewFactory`] that forwards all three calls to
//! [`AppKitFactory`] unchanged and then pins the view the call names (the one
//! `create` just handed the host, or the one `update_params` was lent). That
//! keeps L1 in this module alone and covers every slot the desktop host
//! drives — the six controls, every `NativeComponent`, and a dead-slot
//! placeholder alike (pinning an empty `NSView` is harmless). [`ensure_registered`]
//! registers the wrapper, not the bare factory, under the same
//! [`VIEW_TYPE`]; the host sees no difference.

use std::sync::{Arc, Once};

use frust_plugin::desktop::{DesktopViewFactory, DesktopViewHandle, register_view_factory};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSView,
};

use super::factory::{AppKitFactory, VIEW_TYPE};
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

/// [`AppKitFactory`] with theme ladder L1 applied on the way out of `create`
/// and `update_params` — module doc's *The call site*. Stateless, like the
/// factory it wraps.
pub(crate) struct ThemedFactory(AppKitFactory);

impl DesktopViewFactory for ThemedFactory {
    fn create(&self, params_json: &str) -> Option<DesktopViewHandle> {
        let handle = self.0.create(params_json)?;
        pin(&handle, params_json);
        Some(handle)
    }

    fn update_params(&self, view: &DesktopViewHandle, params_json: &str) {
        self.0.update_params(view, params_json);
        pin(view, params_json);
    }

    fn dispose(&self, view: DesktopViewHandle) {
        self.0.dispose(view);
    }
}

/// Pin the view `handle` names to the params' brightness. A no-op off the
/// main thread: the wrapped factory already declined/ignored that call (its
/// own module doc), and `NSView` may not be touched there.
fn pin(handle: &DesktopViewHandle, params_json: &str) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    // SAFETY: `handle` is either the +1-retained `NSView` `create` just
    // produced (still owned by the handle, not yet given to the host), or the
    // host's lend of that same pointer for the duration of `update_params` —
    // live for this whole call in both cases, and only ever an `NSView`
    // (`crate::appkit::factory`'s *Retain accounting*). We are on the main
    // thread (checked above), and the reference does not outlive this call.
    let view = unsafe { &*handle.as_ptr().cast::<NSView>() };
    apply_brightness(view, brightness_is_dark(params_json));
}

/// Register [`ThemedFactory`] with the desktop platform-view registry under
/// [`VIEW_TYPE`], once per process — `crate::appkit::factory`'s own
/// registration contract (lazy, `Once`, first-registration-wins, an
/// `AlreadyRegistered` answer logged at debug), for the L1-wrapped factory.
pub(crate) fn ensure_registered() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        match register_view_factory(VIEW_TYPE, Arc::new(ThemedFactory(AppKitFactory))) {
            Ok(()) => log::info!(
                "frust-native-widgets: registered the macOS desktop view factory for {VIEW_TYPE:?}"
            ),
            Err(error) => log::debug!(
                "frust-native-widgets: macOS desktop view factory not registered ({error}) — the \
                 incumbent keeps {VIEW_TYPE:?}"
            ),
        }
    });
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

    #[test]
    fn the_themed_factory_declines_off_the_main_thread_like_the_one_it_wraps() {
        // Same shape as `crate::appkit::factory`'s off-main test: a `cargo
        // test` worker is off the process's main thread, so the wrapped
        // `create` declines and the wrapper must pass that `None` through
        // without touching a view.
        if MainThreadMarker::new().is_some() {
            return;
        }
        assert!(ThemedFactory(AppKitFactory).create("{}").is_none());
    }
}
