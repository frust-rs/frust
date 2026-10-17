use std::any::Any;

use super::RenderRoot;
use crate::insets::WindowInsets;
use crate::view::{ChangeFlags, View};

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Switch the layout skip on (the default) or off.
    ///
    /// With the skip on, a layout pass lays out only the pods that are dirty,
    /// whose constraints changed, or that last laid out under an older layout
    /// epoch — see [`ChildPod::layout_child`](crate::widget::ChildPod::layout_child) for the rule and its dirty
    /// sources. `false` forces every pod to lay out on every pass, exactly the
    /// pre-skip behaviour: the kill switch a shell exposes (this crate reads no
    /// environment variable itself). Takes effect from the next
    /// [`RenderRoot::layout`]; dirties nothing by itself.
    pub fn set_layout_skip(&mut self, skip: bool) {
        self.layout_skip = skip;
    }

    /// Whether layout passes may skip clean pods ([`RenderRoot::set_layout_skip`]).
    pub fn layout_skip(&self) -> bool {
        self.layout_skip
    }

    /// Turn the layout audit on or off (off by default; a debug tool).
    ///
    /// When on, and only while the skip is enabled ([`RenderRoot::set_layout_skip`]),
    /// every [`RenderRoot::layout`] pass proves the skip rule: after the normal
    /// (skipping) pass it snapshots every pod's geometry, runs a second, **forced**
    /// full pass over the same constraints, and compares. A pod whose origin
    /// (relative to its parent) or size differs by more than `1e-3` logical px
    /// changed its layout without signalling dirty, which is exactly the bug the
    /// skip would turn into stale geometry. Each is counted on
    /// [`RenderRoot::layout_audit_mismatches`] and logged at `warn` through the
    /// `log` crate, prefixed `layout-audit:` and naming the widget type and debug
    /// label, at most once per `(type, label)` per 60 audited frames.
    ///
    /// **Cost:** two layout passes per frame, the second laying out the whole
    /// tree, so this erases the skip's savings; never leave it on in a shipped
    /// build. The forced pass's geometry is the geometry that stays, so the audit
    /// never ships wrong geometry. The forced pass does not bump the layout epoch;
    /// like any forced pass it leaves every pod clean (`layout_dirty` cleared,
    /// `last_constraints`/`last_epoch`/`last_insets` refreshed to the current
    /// values), which is the state the next frame's skip decisions start from.
    /// This crate reads no environment variable; shells wire `FRUST_LAYOUT_AUDIT`.
    /// With the audit off the cost is one `bool` check per layout pass.
    pub fn set_layout_audit(&mut self, audit: bool) {
        self.layout_audit.enabled = audit;
    }

    /// Whether the layout audit is on ([`RenderRoot::set_layout_audit`]).
    pub fn layout_audit(&self) -> bool {
        self.layout_audit.enabled
    }

    /// Total pods the layout audit has found with geometry that differed between
    /// the skipping and the forced pass, since the root was created. Counts every
    /// occurrence, unlike the rate-limited log.
    pub fn layout_audit_mismatches(&self) -> u64 {
        self.layout_audit.mismatches
    }

    /// The global layout epoch the next layout pass runs under — read-only, for
    /// tooling and tests. It moves on a theme swap, an inset change, a window
    /// resize, and a dirty floated surface (see [`ChildPod::layout_child`](crate::widget::ChildPod::layout_child)).
    pub fn layout_epoch(&self) -> u64 {
        self.layout_epoch
    }

    /// Invalidate every pod's cached layout: the next pass lays out the whole
    /// tree.
    pub(super) fn bump_layout_epoch(&mut self) {
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
        self.paint_epoch = self.paint_epoch.wrapping_add(1);
    }

    /// Enable or disable retained layers (default on). When off,
    /// [`PaintCtx::subtree_clean`](crate::widget::PaintCtx::subtree_clean) answers `false` for every pod, so a boundary
    /// widget repaints its child every frame. The core never reads the
    /// environment: a shell wires `FRUST_NO_RETAINED_LAYERS` to this.
    pub fn set_retained_layers(&mut self, enabled: bool) {
        self.retained_layers = enabled;
    }

    /// Hand the root the renderer's resident-key ledger, as published after the
    /// previous frame (one frame stale by design). Consulted by
    /// [`PaintCtx::retained_available`](crate::widget::PaintCtx::retained_available); defaults to empty (nothing resident).
    /// Cloned only when it differs from the stored one.
    pub fn set_retained_ledger(&mut self, ledger: &crate::retained::RetainedLedger) {
        if self.retained_ledger != *ledger {
            self.retained_ledger.clone_from(ledger);
        }
    }

    /// Whether retained layers are enabled ([`RenderRoot::set_retained_layers`]).
    pub fn retained_layers(&self) -> bool {
        self.retained_layers
    }

    /// Whether any floated surface the last paint drew is layout-dirty (see
    /// `overlay_pods`). A pod already borrowed elsewhere — impossible between
    /// passes, but cheap to tolerate — counts as dirty.
    pub(super) fn overlay_pod_dirty(&self) -> bool {
        self.overlay_pods.iter().any(|weak| {
            weak.upgrade()
                .is_some_and(|pod| pod.try_borrow().map_or(true, |pod| pod.is_layout_dirty()))
        })
    }

    /// Store the app's active theme, threaded into every subsequent
    /// layout/paint pass as `Option<&dyn Any>` and recovered by widgets via
    /// [`crate::widget::PaintCtx::theme_as`]/[`crate::widget::LayoutCtx::theme_as`].
    ///
    /// The theme is boxed **type-erased** (`Box<dyn Any>`) so this crate stays
    /// independent of `frust-theme`; the shell boxes the concrete `Theme`
    /// (and re-boxes it on a live appearance change, e.g. dark-mode toggle).
    /// Calling again replaces the stored theme.
    ///
    /// Marks `LAYOUT | PAINT` pending (drained by
    /// [`RenderRoot::take_change_flags`]): a theme swap can change baked-in
    /// paint state a widget resolves at layout time (e.g. `Text`'s themed
    /// glyph color, cached into its `TextLayout` — see
    /// `frust-widgets::text`), so a shell that later gates layout/paint on
    /// this seam must still see a bare `set_theme` as dirty even though no
    /// view changed.
    pub fn set_theme(&mut self, theme: Box<dyn Any>) {
        self.theme = Some(theme);
        self.pending |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        // A theme is a global layout input no constraint carries: every cached
        // layout is stale (the layout skip — see `layout_epoch`).
        self.bump_layout_epoch();
        // A theme swap can change semantics-visible state (e.g. a relabelled or
        // re-bounded node once layout re-runs); treat it as semantics-dirty too.
        self.semantics_gen = self.semantics_gen.wrapping_add(1);
    }

    /// Store the window's insets ([`WindowInsets`]), threaded into every
    /// subsequent layout/paint pass and recovered by widgets via
    /// [`crate::widget::LayoutCtx::window_insets`]/
    /// [`crate::widget::PaintCtx::window_insets`].
    ///
    /// Mirrors [`RenderRoot::set_theme`]'s dirty-tracking contract: a change
    /// marks `LAYOUT | PAINT` pending (drained by
    /// [`RenderRoot::take_change_flags`]) so a shell gating layout/paint on that
    /// seam still relayouts when the insets move — a `SafeArea` widget resolves
    /// its inset at layout time, so the mobile layout-skip gate must see a bare
    /// `set_insets` as dirty even though no view changed (the same reasoning as
    /// the theme swap — see `docs/ARCHITECTURE.md`'s Theme delivery and Frame
    /// gate). A change also bumps the semantics generation, since a moved inset
    /// shifts laid-out node bounds.
    ///
    /// No-op guarded by [`WindowInsets`]'s `PartialEq`: pushing the current
    /// value marks nothing dirty, so a shell that polls the platform insets
    /// every frame and forwards unconditionally never forces a needless
    /// relayout. (A shell may also skip the call itself by comparing first —
    /// this is the same guard, held on the core side.)
    pub fn set_insets(&mut self, insets: WindowInsets) {
        if self.insets == insets {
            return;
        }
        self.insets = insets;
        self.pending |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        // Global and constraint-free, like the theme: invalidate every cached
        // layout (see `layout_epoch`).
        self.bump_layout_epoch();
        // A moved inset shifts laid-out node bounds once layout re-runs; treat
        // it as semantics-dirty too (mirrors `set_theme`).
        self.semantics_gen = self.semantics_gen.wrapping_add(1);
    }

    /// The window's insets currently threaded into the layout/paint passes.
    pub fn insets(&self) -> WindowInsets {
        self.insets
    }

    /// Store the shell's running count of frames the render thread has actually
    /// presented, threaded into every subsequent paint pass and recovered by
    /// widgets via [`crate::widget::PaintCtx::presented_frames`]. A shell loads
    /// the atomic its render side increments (once per presented frame) and
    /// pushes it here once per UI frame, before `paint`.
    ///
    /// **Deliberately dirties nothing.** Unlike [`RenderRoot::set_theme`] and
    /// [`RenderRoot::set_insets`] — which mark `LAYOUT | PAINT` pending because a
    /// widget bakes their value in at layout time — this setter marks NO
    /// [`ChangeFlags`] and bumps NO semantics generation. The presented count is
    /// a paint-only *observation* a widget reads live every paint (never baked at
    /// layout), so treating it as dirty would be wrong twice over: it would force
    /// a needless relayout, and — critically — on the mobile shells a
    /// monotonically ticking counter would keep the frame gate's pending-flags
    /// input perpetually true, so the menu would never idle (the 32s-idle
    /// behavior must survive). Keeping this setter dirt-free
    /// is exactly what keeps the frame gate unaware of it (see
    /// `docs/ARCHITECTURE.md`'s Frame gate).
    pub fn set_presented_frames(&mut self, presented: u64) {
        self.presented_frames = Some(presented);
    }

    /// The presented-frame count currently threaded into the paint pass, or
    /// `None` if no shell has pushed one.
    pub fn presented_frames(&self) -> Option<u64> {
        self.presented_frames
    }

    /// Store whether the shell's GPU surface is translucent (alpha-channel,
    /// "Mode B"), threaded into every subsequent paint pass and recovered by
    /// widgets through [`crate::widget::PaintCtx::is_translucent`]. A shell
    /// pushes the surface's **resolved** translucency here — what the GPU
    /// backend reports after the surface is installed, not what the app
    /// requested via `frust-shell-common::surface_mode`'s latch: a translucency
    /// request the platform refuses must degrade to the opaque contract, or
    /// every `platform_view` slot punches a hole in an opaque swapchain
    /// (black rectangles). Every desktop app leaves the default `false`
    /// (opaque, "Mode A").
    ///
    /// Marks `PAINT` pending on an actual change (`PartialEq`-guarded, mirroring
    /// [`RenderRoot::set_insets`]'s no-op guard): translucency is read purely at
    /// paint time (the hole-punch runs in `paint`, never baked at layout), so a
    /// flip must repaint but need not relayout. A flip is rare but **real**: a
    /// surface (re)install can resolve differently from the previous one, and
    /// both mobile shells re-push this every frame (the no-op-if-unchanged
    /// guard is what makes that free).
    pub fn set_surface_translucent(&mut self, translucent: bool) {
        if self.surface_translucent == translucent {
            return;
        }
        self.surface_translucent = translucent;
        // Translucency is read at paint time only: no relayout, but every
        // retained subtree is stale.
        self.paint_epoch = self.paint_epoch.wrapping_add(1);
        self.pending |= ChangeFlags::PAINT;
    }

    /// Whether the shell's GPU surface is currently marked translucent.
    pub fn is_surface_translucent(&self) -> bool {
        self.surface_translucent
    }
}
