//! Accessibility: the lazily-attached accesskit adapter and the
//! generation-gated post-layout semantics push.
//!
//! The queued-action drain that feeds this adapter's activations back into the
//! tree runs inside the frame body ([`super::frame`]), before the rebuild.

use crate::accessibility::IosA11yAdapter;

use super::IosAppHandle;

impl IosAppHandle {
    /// Store the accesskit adapter for the app's `FrustView`.
    ///
    /// Called once from [`crate::ffi_glue::init_accessibility`] on the first
    /// layout, after `frust_init` returned this handle. The `unsafe`
    /// construction of the [`IosA11yAdapter`] (dynamically subclassing the view to
    /// implement the UIKit accessibility methods) happens at the FFI boundary in
    /// `ffi_glue` — the sanctioned zone for raw-pointer work — so this method is a
    /// plain, safe store: it just takes the already-constructed adapter. From the
    /// next frame on, `frame()` pushes semantics to it and routes its queued
    /// actions.
    pub(crate) fn attach_accessibility(&mut self, adapter: IosA11yAdapter) {
        self.a11y = Some(adapter);
    }

    /// Publish the current accessibility tree to the iOS accesskit adapter, if it
    /// changed since the last push (generation-gated).
    /// Must run **after** [`Self::frame`]'s layout so node bounds are valid.
    ///
    /// Gated on the semantics generation exactly like the desktop/Android
    /// adapters (see `docs/CODE_STANDARDS.md`'s Semantics Conventions): the
    /// `AppTree::semantics_if_changed` check skips reassembling+re-pushing an
    /// unchanged tree, so a static screen pays no per-frame semantics cost — this
    /// closes the iOS adapter's former "recompute-on-every-active-frame" fallback.
    /// The adapter's own `update_if_active` is a second, finer gate: it pushes
    /// only while an assistive technology is active. The assembled tree is also
    /// snapshotted inside [`IosA11yAdapter::publish`] so a late-activating AT
    /// (one that connects on a static screen this gate would otherwise skip) is
    /// served real content immediately — the Android adapter's `tree_snapshot`
    /// design.
    pub(super) fn publish_semantics(&mut self) {
        // Cheap generation gate first (immutable `a11y` borrow, released before
        // the `&mut self.app` call below), mirroring the Android shell's
        // `publish_semantics`.
        let last_gen = match self.a11y.as_ref() {
            Some(a11y) => a11y.last_pushed_gen(),
            None => return,
        };
        let Some(update) = self.app.semantics_if_changed(last_gen) else {
            return; // tree unchanged since the last push
        };
        let current_gen = self.app.semantics_generation();
        if let Some(a11y) = self.a11y.as_mut() {
            a11y.publish(update, current_gen);
        }
    }
}
