//! Configuration knobs and shell-pushed inputs on [`RenderRoot`]: layout skip and audit,
//! retained layers, theme, insets, presented frames and surface translucency.

use std::any::Any;

use super::RenderRoot;
use crate::insets::WindowInsets;
use crate::view::{ChangeFlags, View};

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Switch the layout skip on (the default) or off; `false` lays out every pod every pass.
    pub fn set_layout_skip(&mut self, skip: bool) {
        self.layout_skip = skip;
    }

    /// Whether layout passes may skip clean pods ([`RenderRoot::set_layout_skip`]).
    pub fn layout_skip(&self) -> bool {
        self.layout_skip
    }

    /// Turn the layout audit (a debug aid that doubles layout cost) on or off; off by default.
    /// With the skip on, layout re-runs forced and warns `layout-audit:` per pod moved >1e-3 px.
    pub fn set_layout_audit(&mut self, audit: bool) {
        self.layout_audit.enabled = audit;
    }

    /// Whether the layout audit is on ([`RenderRoot::set_layout_audit`]).
    pub fn layout_audit(&self) -> bool {
        self.layout_audit.enabled
    }

    /// Pods the layout audit has found with mismatched geometry since creation, every occurrence
    /// counted (unlike the rate-limited log).
    pub fn layout_audit_mismatches(&self) -> u64 {
        self.layout_audit.mismatches
    }

    /// The global layout epoch the next layout pass runs under, for tooling and tests.
    pub fn layout_epoch(&self) -> u64 {
        self.layout_epoch
    }

    /// Invalidate every cached layout and paint: the next pass lays out the whole tree.
    pub(super) fn bump_layout_epoch(&mut self) {
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
        self.paint_epoch = self.paint_epoch.wrapping_add(1);
    }

    /// Enable or disable retained layers (default on); off, every boundary repaints its child.
    pub fn set_retained_layers(&mut self, enabled: bool) {
        self.retained_layers = enabled;
    }

    /// Hand the root the renderer's resident-key ledger from the previous frame (one frame stale
    /// by design); cloned only when it differs.
    pub fn set_retained_ledger(&mut self, ledger: &crate::retained::RetainedLedger) {
        if self.retained_ledger != *ledger {
            self.retained_ledger.clone_from(ledger);
        }
    }

    /// Whether retained layers are enabled ([`RenderRoot::set_retained_layers`]).
    pub fn retained_layers(&self) -> bool {
        self.retained_layers
    }

    /// Whether any floated pod the last paint drew is layout-dirty (a borrowed one counts).
    pub(super) fn overlay_pod_dirty(&self) -> bool {
        self.overlay_pods.iter().any(|weak| {
            weak.upgrade()
                .is_some_and(|pod| pod.try_borrow().map_or(true, |pod| pod.is_layout_dirty()))
        })
    }

    /// Store (replace) the app's type-erased theme, recovered by widgets via `theme_as`. Marks
    /// `LAYOUT | PAINT` and invalidates every cached layout: widgets bake themed state at layout.
    pub fn set_theme(&mut self, theme: Box<dyn Any>) {
        self.theme = Some(theme);
        self.pending |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        self.bump_layout_epoch();
        self.semantics_gen = self.semantics_gen.wrapping_add(1);
    }

    /// Store the window's insets, recovered by widgets via `window_insets`. An actual change
    /// dirties exactly like [`RenderRoot::set_theme`]; pushing the current value is a no-op.
    pub fn set_insets(&mut self, insets: WindowInsets) {
        if self.insets == insets {
            return;
        }
        self.insets = insets;
        self.pending |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        self.bump_layout_epoch();
        self.semantics_gen = self.semantics_gen.wrapping_add(1);
    }

    /// The window's insets currently threaded into the layout/paint passes.
    pub fn insets(&self) -> WindowInsets {
        self.insets
    }

    /// Store the shell's presented-frame count (pushed once per UI frame, before `paint`). Dirties
    /// nothing: a ticking count marked dirty would keep the mobile frame gate from ever idling.
    pub fn set_presented_frames(&mut self, presented: u64) {
        self.presented_frames = Some(presented);
    }

    /// The presented-frame count currently threaded into the paint pass, or
    /// `None` if no shell has pushed one.
    pub fn presented_frames(&self) -> Option<u64> {
        self.presented_frames
    }

    /// Store the surface's *resolved* translucency (what the backend granted, not what the app
    /// asked: a refused request must stay opaque, or every platform-view slot punches a black
    /// hole). An actual change marks `PAINT` and invalidates retained paint; no relayout.
    pub fn set_surface_translucent(&mut self, translucent: bool) {
        if self.surface_translucent == translucent {
            return;
        }
        self.surface_translucent = translucent;
        self.paint_epoch = self.paint_epoch.wrapping_add(1);
        self.pending |= ChangeFlags::PAINT;
    }

    /// Whether the shell's GPU surface is currently marked translucent.
    pub fn is_surface_translucent(&self) -> bool {
        self.surface_translucent
    }
}
