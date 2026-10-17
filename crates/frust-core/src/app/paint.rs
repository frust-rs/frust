//! `RenderRoot`'s paint pass: the main tree, then the floated overlay pods
//! above it, then the IME-surface and selection-toolbar resolution.

use std::rc::Rc;

use super::RenderRoot;
use crate::anim::FrameTime;
use crate::event::ImeState;
use crate::overlay::{
    OverlayEntry, OverlayHit, OverlayKey, OverlayPaintPass, sort_into_paint_order,
};
use crate::selection_toolbar::{
    SelectionToolbarActions, SelectionToolbarPass, SelectionToolbarRequest,
};
use crate::shortcuts::{OverlayStamp, ShortcutPaintPass, ShortcutTable};
use crate::view::{ChangeFlags, View};
use crate::widget::{PaintCtx, PaintOutcome, PaintScene};

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Paint the root widget into `scene`. The outcome reports a continuation
    /// frame ([`PaintCtx::request_frame`] → [`PaintOutcome::needs_frame`]) and a
    /// paint-time [`PaintCtx::request_layout`], which is also folded into the
    /// pending [`ChangeFlags`] as `LAYOUT`. `frame_time` is the shell's clock for
    /// the frame, threaded unchanged to every widget ([`PaintCtx::frame_time`]).
    ///
    /// # The overlay post-pass
    ///
    /// Surfaces registered during the main walk ([`PaintCtx::register_overlay`])
    /// are painted after it in band order, escaping their owner's paint order and
    /// clips; their routing rects are kept for the next event pass to hit-test
    /// first, and their outcomes merge into this one.
    pub fn paint(&mut self, scene: &mut dyn PaintScene, frame_time: FrameTime) -> PaintOutcome {
        // Entering clears each channel, so the registries start every paint empty.
        let overlay_pass = OverlayPaintPass::enter();
        let toolbar_pass = SelectionToolbarPass::enter();
        let shortcut_pass = ShortcutPaintPass::enter();
        // The pods compare their stamps against this root's session.
        self.publish_focus_session();
        // Number this pass (see `paint_serial`).
        self.paint_serial = self.paint_serial.wrapping_add(1);
        // Moved links change pod reads without dirtying the loser: no old subtree is clean.
        let links = (self.hover_link, self.focus_epoch);
        if links != self.painted_links {
            self.painted_links = links;
            self.paint_epoch = self.paint_epoch.wrapping_add(1);
        }

        let (mut outcome, main_tree_ime) = self.paint_main_tree(scene, frame_time);
        outcome.paint_serial = self.paint_serial;

        // Stable sort: the band decides, registration order breaks ties.
        let mut entries = overlay_pass.take();
        sort_into_paint_order(&mut entries);
        // Routing rects for the next event pass; weak pods for the next layout (`overlay_pods`).
        self.overlay_hits = entries.iter().map(OverlayHit::of).collect();
        self.overlay_pods = entries
            .iter()
            .map(|entry| Rc::downgrade(&entry.pod))
            .collect();
        let (holder, holder_ime) = self.paint_overlays(&entries, scene, frame_time, &mut outcome);
        // The root holds no overlay pod at rest.
        drop(entries);
        // After the pods, so their (stamped) declarations are in the snapshot too.
        self.shortcuts = ShortcutTable::new(shortcut_pass.take());

        self.resolve_session_surface(main_tree_ime, holder, holder_ime);

        // Last, so a field painted inside a floated pod is resolved like any other.
        self.resolve_selection_toolbar(toolbar_pass.take());

        outcome
    }

    /// Settle which branch's IME surface the shell is configured with, from the
    /// main tree's publish and the focus-holding surface's (`holder`/`holder_ime`,
    /// see [`RenderRoot::paint_overlays`]). Decided here, once both halves have
    /// painted, because the main tree publishes before ownership is known.
    ///
    /// # The rules
    ///
    /// * the owner's own publish wins, whether the owner is a surface or the tree;
    /// * an inactive publish from the owner ends the session;
    /// * an owner that published nothing leaves a standing surface standing,
    ///   unless ownership moved this pass, in either direction: then it is cleared.
    fn resolve_session_surface(
        &mut self,
        main_tree_ime: Option<ImeState>,
        holder: Option<OverlayKey>,
        holder_ime: Option<ImeState>,
    ) {
        let published = if holder.is_some() {
            holder_ime
        } else {
            main_tree_ime
        };
        let ownership_moved = holder != self.focus_surface;
        match published {
            Some(ime) if ime.active => self.store_ime_state(Some(ime)),
            Some(_) => self.release_focus_session(),
            None if ownership_moved => self.store_ime_state(None),
            None => {}
        }
        // Resolved from the links, never from the request that opened the session.
        self.focus_surface = if self.focus_active { holder } else { None };
    }

    /// Paint the main widget tree, returning its outcome and the IME surface it
    /// published (if any). Split from [`RenderRoot::paint`] for borrow scoping;
    /// the publish is settled by [`RenderRoot::resolve_session_surface`].
    fn paint_main_tree(
        &mut self,
        scene: &mut dyn PaintScene,
        frame_time: FrameTime,
    ) -> (PaintOutcome, Option<ImeState>) {
        let Some(root_id) = self.root_id else {
            return (PaintOutcome::default(), None);
        };
        // Read out before the `&mut self.tree` borrow below.
        let theme = self.theme.as_deref();
        let insets = self.insets;
        let presented_frames = self.presented_frames;
        let surface_translucent = self.surface_translucent;
        let (paint_epoch, retained_layers) = (self.paint_epoch, self.retained_layers);
        let paint_serial = self.paint_serial;
        let focus_ring = self.focus_ring_visible();
        let retained_ledger = &self.retained_ledger;
        let Some(pod) = self.tree.pod_mut(root_id) else {
            return (PaintOutcome::default(), None);
        };
        let mut ctx = PaintCtx::new(pod.origin(), pod.size());
        // The pass's global inputs (clock, theme, insets, frame count, translucency, retain).
        ctx.set_frame_time(frame_time);
        ctx.set_theme(theme);
        ctx.set_window_insets(insets);
        ctx.set_presented_frames(presented_frames);
        ctx.set_translucent(surface_translucent);
        ctx.set_paint_state(
            paint_epoch,
            retained_layers,
            Some(retained_ledger),
            paint_serial,
        );
        // Seed focus/hover from the root's mirrors and live epochs; `ChildPod::paint_child`
        // counts a deeper link only while its stamp names the live session.
        ctx.set_has_focus(self.focus_active);
        ctx.set_focus_ring(focus_ring);
        ctx.set_focus_epoch(self.focus_epoch);
        ctx.set_hovered(self.hover_active);
        ctx.set_hover_epoch(self.hover_epoch);
        pod.widget_mut().paint(&mut ctx, scene);
        pod.clear_flags();
        // Collected for `resolve_session_surface`, and only while a session is live,
        // so a lagging widget cannot resurrect the surface a blur dropped.
        let main_tree_ime = if self.focus_active {
            ctx.take_ime_state()
        } else {
            None
        };
        // Replaced every pass, so a stale platform-view frame or input shield
        // never outlives the paint that stopped publishing it.
        self.platform_view_frames = ctx.take_platform_views();
        self.input_shields = ctx.take_input_shields();
        let outcome = fold_main_tree_outcome(
            &ctx,
            &mut self.pending,
            &mut self.painted_animating,
            &mut self.deferred_frame,
        );
        (outcome, main_tree_ime)
    }

    /// Paint this pass's floated pods above the main tree, each at its own
    /// `window_rect` with the root pod's seeding, folding outcomes into `outcome`.
    /// Retires stale focus links
    /// ([`ChildPod::retire_stale_focus_link`](crate::widget::ChildPod::retire_stale_focus_link))
    /// and returns `(holder, holder's publish)` for the one surface holding a live
    /// link; other pods' publishes are dropped, and two holders resolve to none.
    fn paint_overlays(
        &mut self,
        entries: &[OverlayEntry],
        scene: &mut dyn PaintScene,
        frame_time: FrameTime,
        outcome: &mut PaintOutcome,
    ) -> (Option<OverlayKey>, Option<ImeState>) {
        if entries.is_empty() {
            return (None, None);
        }
        let (paint_epoch, retained_layers) = (self.paint_epoch, self.retained_layers);
        let paint_serial = self.paint_serial;
        let retained_ledger = &self.retained_ledger;
        let theme = self.theme.as_deref();
        let presented_frames = self.presented_frames;
        let surface_translucent = self.surface_translucent;
        let hover_active = self.hover_active;
        let hover_epoch = self.hover_epoch;
        let focus_active = self.focus_active;
        let focus_epoch = self.focus_epoch;
        let focus_ring = self.focus_ring_visible();

        // One pair, so a publish is never attributed to an entry that did not make it.
        let mut holder: Option<(OverlayKey, Option<ImeState>)> = None;
        let mut contested = false;

        for entry in entries {
            let mut ctx = PaintCtx::new(entry.window_rect.origin(), entry.window_rect.size());
            ctx.set_frame_time(frame_time);
            ctx.set_theme(theme);
            ctx.set_window_insets(entry.insets);
            ctx.set_presented_frames(presented_frames);
            ctx.set_translucent(surface_translucent);
            ctx.set_paint_state(
                paint_epoch,
                retained_layers,
                Some(retained_ledger),
                paint_serial,
            );
            // The pod's own link is ANDed onto the mirror inside `paint_child`.
            ctx.set_has_focus(focus_active);
            ctx.set_focus_ring(focus_ring);
            ctx.set_focus_epoch(focus_epoch);
            ctx.set_hovered(hover_active);
            ctx.set_hover_epoch(hover_epoch);
            let pod_holds_focus = {
                let _stamp = OverlayStamp::enter(entry.key);
                let mut pod = entry.pod.borrow_mut();
                pod.retire_stale_focus_link();
                let holds = pod.holds_live_focus();
                pod.paint_child(&mut ctx, scene);
                holds
            };

            fold_overlay_outcome(&ctx, outcome, &mut self.pending);
            // Taken unconditionally and kept only from the live holder; an inactive
            // publish from any other pod is refused too, not read as a release.
            let published = ctx.take_ime_state();
            if focus_active && pod_holds_focus {
                if holder.is_some() {
                    contested = true;
                } else {
                    holder = Some((entry.key, published));
                }
            }
            // Extend, not replace: `paint_main_tree` already filled these this pass.
            self.platform_view_frames.extend(ctx.take_platform_views());
            self.input_shields.extend(ctx.take_input_shields());
        }

        match holder {
            Some((key, published)) if !contested => (Some(key), published),
            _ => (None, None),
        }
    }

    /// Store this pass's selection-toolbar publish as a level (newest wins) and
    /// bump [`RenderRoot::selection_toolbar_generation`] only on a menu edge. The
    /// publish is refused while no focus session is active, as the IME one is.
    fn resolve_selection_toolbar(&mut self, published: Option<SelectionToolbarRequest>) {
        let next = if self.focus_active { published } else { None };
        // An absent request reads as "no menu, no verbs": one comparison covers every edge.
        let menu_edge = |request: &Option<SelectionToolbarRequest>| {
            request.map_or((false, SelectionToolbarActions::default()), |request| {
                (request.present_menu, request.actions)
            })
        };
        if menu_edge(&self.selection_toolbar) != menu_edge(&next) {
            self.selection_toolbar_gen = self.selection_toolbar_gen.wrapping_add(1);
        }
        self.selection_toolbar = next;
    }

    /// The selection-toolbar request the focused field published during the most
    /// recent [`RenderRoot::paint`], or `None` when no field is focused. A level:
    /// a shell driving the platform edit menu
    /// ([`SelectionToolbarPolicy::Native`](crate::selection_toolbar::SelectionToolbarPolicy::Native))
    /// reads it beside [`RenderRoot::ime_state`]. Present with a bare caret too
    /// (paste applies), and under the framework policy as well.
    pub fn selection_toolbar(&self) -> Option<SelectionToolbarRequest> {
        self.selection_toolbar
    }

    /// A generation bumped on every change to the menu-significant part of
    /// [`RenderRoot::selection_toolbar`] (`present_menu` and `actions`, clearing
    /// included). A moved anchor is not an edge: a shell re-reads it per frame,
    /// caching this value like [`RenderRoot::focus_ime_generation`].
    pub fn selection_toolbar_generation(&self) -> u64 {
        self.selection_toolbar_gen
    }
}

/// Fold the main tree's paint-time layout dirtying into `pending` (`request_layout`, an unpaced
/// frame, the path bit, the root's landing paint) and build its outcome. A paced-only frame folds
/// nothing, or the frame gate would run it every vsync. Writes `painted_animating`, drains
/// `deferred_frame`.
fn fold_main_tree_outcome(
    ctx: &PaintCtx<'_>,
    pending: &mut ChangeFlags,
    painted_animating: &mut bool,
    deferred_frame: &mut bool,
) -> PaintOutcome {
    let needs_layout = ctx.needs_layout();
    let animating = needs_layout || ctx.needs_frame();
    let unpaced = needs_layout || (ctx.needs_frame() && !ctx.needs_frame_paced_only());
    let landing = *painted_animating && !animating;
    if unpaced || landing || ctx.is_layout_path_dirty() {
        *pending |= ChangeFlags::LAYOUT;
    }
    *painted_animating = animating;
    // A rebuild that ran out of flush passes owes one more frame.
    let deferred_frame = std::mem::take(deferred_frame);
    PaintOutcome {
        needs_frame: ctx.needs_frame() || deferred_frame,
        needs_layout,
        needs_frame_paced_only: ctx.needs_frame_paced_only(),
        paced_interval: ctx.paced_interval(),
        // Stamped by `paint` once both halves of the pass have run.
        paint_serial: 0,
    }
}

/// Fold one floated pod's paint into `outcome`: the frame request ORed, its class on the max
/// lattice, the paced interval on the MIN lattice; layout dirtying as in the tree, into `pending`
/// too (the pod's own mark makes the next pass a full one).
fn fold_overlay_outcome(ctx: &PaintCtx<'_>, outcome: &mut PaintOutcome, pending: &mut ChangeFlags) {
    let stood_unpaced = outcome.needs_frame && !outcome.needs_frame_paced_only;
    let entry_unpaced = ctx.needs_frame() && !ctx.needs_frame_paced_only();
    outcome.needs_frame |= ctx.needs_frame();
    outcome.needs_frame_paced_only = outcome.needs_frame && !(stood_unpaced || entry_unpaced);
    outcome.paced_interval = match (outcome.paced_interval, ctx.paced_interval()) {
        (Some(standing), Some(asked)) => Some(standing.min(asked)),
        (standing, asked) => standing.or(asked),
    };
    if ctx.needs_layout() {
        outcome.needs_layout = true;
        *pending |= ChangeFlags::LAYOUT;
    }
    if ctx.is_layout_path_dirty() {
        *pending |= ChangeFlags::LAYOUT;
    }
}
