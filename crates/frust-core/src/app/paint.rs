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
use crate::view::{ChangeFlags, View};
use crate::widget::{PaintCtx, PaintOutcome, PaintScene};

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Paint the root widget into `scene`, returning whether the tree wants
    /// another frame to continue an animation.
    ///
    /// A widget whose paint advances animation state (e.g. a scroll fling) signals
    /// [`PaintCtx::request_frame`]; that flag bubbles up through the container
    /// [`ChildPod`](crate::widget::ChildPod)s and out here as
    /// [`PaintOutcome::needs_frame`], which the shell honors by scheduling the next
    /// frame (desktop `window.request_redraw()`; the mobile continuous loops
    /// already do so). Mirrors how [`RenderRoot::event`] surfaces `needs_redraw`.
    ///
    /// A widget whose animation changes its *layout* signals
    /// [`PaintCtx::request_layout`] instead (or as well); that bubbles up the same
    /// way and is folded here into the render root's pending [`ChangeFlags`]
    /// (`LAYOUT`), so the *next* frame's
    /// [`take_change_flags`](RenderRoot::take_change_flags)`().needs_layout()`
    /// reports it and the mobile intra-frame layout skip relayouts while the
    /// animation is in flight. It is also surfaced on the returned
    /// [`PaintOutcome::needs_layout`].
    ///
    /// `frame_time` is the shell's shared monotonic clock for this frame
    /// (time enters `frust-core` from the shell, never `Instant::now()` here). It
    /// is seeded onto the root [`PaintCtx`] and threaded unchanged to every child
    /// ([`crate::widget::ChildPod::paint_child`]), so an animating widget advances
    /// against one consistent timestamp — see [`PaintCtx::frame_time`].
    ///
    /// # The overlay post-pass
    ///
    /// Painting the main tree is only the first half. Widgets registering a
    /// floated surface during that walk ([`PaintCtx::register_overlay`]) are
    /// drained here and painted **after** it, in band order — which is the only
    /// way a popover, menu or tooltip escapes its owner's paint order and every
    /// ancestor's clip. Their routing rects are retained (see
    /// `RenderRoot::overlay_hits`) for the next event pass to hit-test first, and
    /// their paint outcomes merge into this pass's own, so an animating overlay
    /// keeps the frames coming exactly like an animating widget in the tree.
    pub fn paint(&mut self, scene: &mut dyn PaintScene, frame_time: FrameTime) -> PaintOutcome {
        // Open the two paint-pass channels for the whole pass. Entering CLEARS
        // each slot, which is what makes "the registry is empty at the start of
        // every paint" true by construction rather than by everyone remembering
        // to unregister; `Drop` hands an enclosing pass its own back.
        let overlay_pass = OverlayPaintPass::enter();
        let toolbar_pass = SelectionToolbarPass::enter();
        // Republish this root's focus session before anything reads it: the
        // channel mirrors one root, and the pods about to be visited must
        // compare their stamps against *this* root's session.
        self.publish_focus_session();
        // Number this pass (see `paint_serial`).
        self.paint_serial = self.paint_serial.wrapping_add(1);
        // A hover link or focus session that moved since the last paint changes
        // what pods read at paint time without having dirtied the pod that lost
        // it, so no subtree painted under the old links is clean (see
        // `PaintCtx::subtree_clean`).
        let links = (self.hover_link, self.focus_epoch);
        if links != self.painted_links {
            self.painted_links = links;
            self.paint_epoch = self.paint_epoch.wrapping_add(1);
        }

        let (mut outcome, main_tree_ime) = self.paint_main_tree(scene, frame_time);
        outcome.paint_serial = self.paint_serial;

        // Drain what the main tree registered and paint it above everything.
        // Sorting is stable, so the band decides and registration order breaks
        // ties within a band (see `crate::overlay::sort_into_paint_order`).
        let mut entries = overlay_pass.take();
        sort_into_paint_order(&mut entries);
        // Retain the routing half — never the pods — for the next event pass.
        self.overlay_hits = entries.iter().map(OverlayHit::of).collect();
        // ... and a weak handle to each pod, for the next layout pass's
        // dirty-surface check (see `overlay_pods`).
        self.overlay_pods = entries
            .iter()
            .map(|entry| Rc::downgrade(&entry.pod))
            .collect();
        let (holder, holder_ime) = self.paint_overlays(&entries, scene, frame_time, &mut outcome);
        // Release the owners' pod clones before the pass ends: the root holds no
        // overlay pod at rest, so a pod's lifetime stays exactly its owner's.
        drop(entries);

        // Both halves have now spoken, so the one question neither of them can
        // answer alone — whose surface the shell is configured with — is settled
        // in one place, with both answers in hand.
        self.resolve_session_surface(main_tree_ime, holder, holder_ime);

        // Resolve the selection-toolbar publish last, so a field that published
        // while painting *inside* a floated pod (a text input hosted in a
        // popover) is resolved by the same rule as one in the main tree.
        self.resolve_selection_toolbar(toolbar_pass.take());

        outcome
    }

    /// Settle which branch's IME surface the shell is configured with, from the
    /// two halves of the paint pass: what the main tree published, and what the
    /// floated surface holding the live focus link published (`holder`/
    /// `holder_ime`, both resolved from the pods' own stamps — see
    /// [`RenderRoot::paint_overlays`]).
    ///
    /// # Why it is decided here and not where the publish happens
    ///
    /// A publish is a claim about *the* session, and the session has exactly one
    /// owner. The main tree paints first, so at the moment it publishes, nothing
    /// yet knows whether a floated surface is about to prove that the session is
    /// no longer the tree's. Storing it there and correcting later is what
    /// produced a frame of lag with a secure field's surface standing in it —
    /// and the record the correction had to be driven from was one nothing could
    /// falsify. Deferring the decision by the width of one pass removes both.
    ///
    /// # The rules
    ///
    /// * the owner's own publish wins, whether that owner is a surface or the
    ///   tree; a publish from anywhere else was already dropped by the half that
    ///   collected it;
    /// * an **inactive** publish from the owner is the session ending, not a
    ///   value — the same reading every other release site gives it;
    /// * an owner that published **nothing** leaves a standing surface standing,
    ///   *unless* ownership moved this pass: then what stands belongs to the
    ///   branch that just lost the session, and goes with it rather than
    ///   remaining as the shell's idea of a live one. This is the rule that stops
    ///   a popover taking focus from leaving the platform keyboard configured for
    ///   the secure field underneath it — in both directions, since the session
    ///   coming back to the tree moves ownership just as much as it leaving.
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
        // Resolved from the links themselves, never from the focus request that
        // opened the session — see the field's doc.
        self.focus_surface = if self.focus_active { holder } else { None };
    }

    /// Paint the main widget tree — everything [`RenderRoot::paint`] does before
    /// the floated overlay pods get their turn.
    ///
    /// Split out from [`RenderRoot::paint`] purely for borrow scoping: this body
    /// holds `&mut self.tree` (beside disjoint borrows of the theme) for its whole
    /// length, while painting an overlay pod needs the root's fields again to
    /// merge outcomes and extend the per-pass channels. Nothing about the pass
    /// itself changed when it moved here.
    ///
    /// Reports, beside the outcome, the IME surface the tree published this pass
    /// (if any) — handed back rather than stored, because whether the tree still
    /// owns the session is not knowable until the floated pods have been visited
    /// (see [`RenderRoot::resolve_session_surface`]).
    fn paint_main_tree(
        &mut self,
        scene: &mut dyn PaintScene,
        frame_time: FrameTime,
    ) -> (PaintOutcome, Option<ImeState>) {
        let Some(root_id) = self.root_id else {
            return (PaintOutcome::default(), None);
        };
        // Disjoint field borrows: the theme (immut) vs the tree (mut).
        let theme = self.theme.as_deref();
        // Copied out before the `&mut self.tree` borrow (a disjoint `Copy` read).
        let insets = self.insets;
        // Same disjoint `Copy` read: the presented-frame count threaded to widgets.
        let presented_frames = self.presented_frames;
        // Same disjoint `Copy` read: the surface-translucency flag the
        // platform-view hole-punch reads (see `PaintCtx::is_translucent`).
        let surface_translucent = self.surface_translucent;
        let (paint_epoch, retained_layers) = (self.paint_epoch, self.retained_layers);
        let paint_serial = self.paint_serial;
        let retained_ledger = &self.retained_ledger;
        if let Some(pod) = self.tree.pod_mut(root_id) {
            let mut ctx = PaintCtx::new(pod.origin(), pod.size());
            // Seed the shared shell clock so the whole paint pass sees one time.
            ctx.set_frame_time(frame_time);
            // Lend the stored theme (type-erased) into the paint pass; widgets
            // recover it via `PaintCtx::theme_as`.
            ctx.set_theme(theme);
            // Thread the window insets down (global — see `crate::insets`).
            ctx.set_window_insets(insets);
            // Thread the shell's presented-frame count down (global; a widget
            // measuring FPS differences it — see `PaintCtx::presented_frames`).
            ctx.set_presented_frames(presented_frames);
            // Thread the surface-translucency flag down (global; the
            // platform-view hole-punch gates its rect-clear on it — see
            // `PaintCtx::is_translucent`).
            ctx.set_translucent(surface_translucent);
            ctx.set_paint_state(
                paint_epoch,
                retained_layers,
                Some(retained_ledger),
                paint_serial,
            );
            // Seed the root widget's paint-time focus from the session mirror so
            // a leaf-root editable observes its own focus, and thread the live
            // session's identity down beside it: deeper focus is resolved
            // per-pod by `ChildPod::paint_child`, which counts a recorded link
            // only while its stamp names this session.
            //
            // The mirror alone, deliberately. Narrowing the seed by *which
            // branch* the root believes owns the session was the previous shape,
            // and it de-seeded the whole tree off a record no pass could
            // falsify: a link a floated pod recorded is reached by no container's
            // blur sweep, so once one existed the tree never got seeded again.
            // The epoch answers the same question where it can actually be
            // answered — at each link, against the session that link was
            // recorded for.
            //
            // The arena root has no recorded link of its own (it is a
            // `WidgetPod`, not a `ChildPod`, and carries no stamp), so a
            // *leaf-root* editable is still seeded from the bare mirror. Every
            // real tree puts a container there, and the first `ChildPod` below it
            // composes the stamp back in.
            ctx.set_has_focus(self.focus_active);
            ctx.set_focus_epoch(self.focus_epoch);
            // Thread the hover mirror + live epoch the same way: the root widget's
            // own hover comes from the mirror (a leaf root can claim hover itself),
            // and deeper links are resolved per-pod by `ChildPod::paint_child`
            // against this epoch.
            ctx.set_hovered(self.hover_active);
            ctx.set_hover_epoch(self.hover_epoch);
            pod.widget_mut().paint(&mut ctx, scene);
            pod.clear_flags();
            // A focused editable republishes its IME surface during paint (which
            // runs after every rebuild), so a controlled change applied by the
            // rebuild — e.g. a submit clearing the field — refreshes the
            // shell-facing `ime_state` that the event pass alone would leave
            // stale.
            //
            // Collected, not stored: what the tree published is only the
            // session's if the session is still the tree's, and the pods that
            // could say otherwise have not been visited yet.
            // `resolve_session_surface` decides once both halves have spoken,
            // and routes the store through the change guard so an *unchanged*
            // republish — the overwhelmingly common case, a focused field
            // re-publishing the same surface frame after frame — moves no
            // generation and therefore fires no `focus_or_ime_changed` edge at
            // the shell.
            //
            // An **inactive** publish is not a surface refresh at all: it is the
            // publishing widget saying "this session is over" — the navigator's
            // post-pop `cleared_ime_state`, `PatternSwitcher`'s equivalent, and
            // a `TextInput` turned disabled/read-only under a live focus are the
            // three shipped producers, and every one of them is an unmount or a
            // de-focus. Storing it as `Some(inactive)` and leaving `focus_active`
            // standing is what leaked the session after a pop: the shells' gate
            // saw `ime_state().is_some()`, `is_focus_active()` kept lying, and the
            // next real focus interaction started from a corrupt baseline. So the
            // resolver takes the *full* release instead — the same one a
            // blur-on-outside-tap `Down` performs, firing exactly one edge.
            //
            // The `self.focus_active` guard stays and is applied here, at the
            // point of collection: a publish arriving when no session is live
            // describes nothing, so a widget whose pod focus was just cleared by
            // a container-routed blur (but whose internal flag lags one frame)
            // can never resurrect the `ime_state` that blur dropped — even
            // before it observes the blur via `PaintCtx::has_focus`.
            //
            // Provenance below that guard is the chain's own job now: a link
            // reads focused only while its stamp names the live session, so a
            // branch the session has left publishes nothing to collect.
            let main_tree_ime = if self.focus_active {
                ctx.take_ime_state()
            } else {
                None
            };
            // Replace (never merge) the whole platform-view collection with
            // whatever this pass published — unlike `ime_state` above there is
            // no single "the" published instance to guard behind a focus
            // check, and a pass that publishes none must clear out every
            // stale frame from the previous one (see the field's doc comment).
            self.platform_view_frames = ctx.take_platform_views();
            // Same replace-per-pass discipline for the z-shield channel: a pass
            // whose shields stopped painting reports none, so a stale shield can
            // never keep stealing input from an interactive slot (see
            // `PaintCtx::report_input_shield`).
            self.input_shields = ctx.take_input_shields();
            // Fold every paint-time layout dirtying into `pending` so the *next*
            // frame relayouts. `pending` survives to the next frame and feeds both
            // the frame gate (`has_pending_change_flags`) and the shells' layout
            // gate (`take_change_flags().needs_layout()`), so no shell change is
            // needed on any platform. Three sources: a bubbled `request_layout`;
            // an unpaced frame request (an animating subtree's pods were marked,
            // the conservative rule — the pass that follows is mostly skipped);
            // and the path bit, which also carries the one trailing paint after
            // an animation lands. A paced-only frame (`CosmeticLoop` requests
            // alone) folds nothing: a pending flag would make the frame gate run
            // it every vsync, and the pods it marked are laid out by the next
            // pass that runs. The root widget has no pod to remember its own
            // trailing paint, so `painted_animating` does that here. Only the
            // first source is reported on the outcome's `needs_layout`.
            let needs_layout = ctx.needs_layout();
            let animating = needs_layout || ctx.needs_frame();
            let unpaced = needs_layout || (ctx.needs_frame() && !ctx.needs_frame_paced_only());
            let landing = self.painted_animating && !animating;
            if unpaced || landing || ctx.is_layout_path_dirty() {
                self.pending |= ChangeFlags::LAYOUT;
            }
            self.painted_animating = animating;
            // A rebuild that ran out of flush passes owes one more frame; surface
            // it here (and clear it) so a dirty-driven shell schedules the frame
            // that finishes the flush — see the `deferred_frame` field doc.
            let deferred_frame = std::mem::take(&mut self.deferred_frame);
            let outcome = PaintOutcome {
                needs_frame: ctx.needs_frame() || deferred_frame,
                needs_layout,
                // Aggregate tick class: paced-only iff a frame was requested and
                // every request was CosmeticLoop-class. The mobile frame gate
                // may throttle such a frame; any Transition request
                // (including the LAYOUT-implying `request_layout` above) leaves
                // this false so the frame runs every vsync.
                needs_frame_paced_only: ctx.needs_frame_paced_only(),
                // ...and, when it IS paceable, how fast it asked to be re-run:
                // the MIN over every paced request this pass (`Duration::ZERO`
                // / `None` meaning the theme's own cosmetic rate). The gate
                // resolves it against the live theme's cap — see
                // `PaintCtx::request_frame_paced_at`.
                paced_interval: ctx.paced_interval(),
                // Stamped by `paint` once both halves of the pass have run.
                paint_serial: 0,
            };
            (outcome, main_tree_ime)
        } else {
            (PaintOutcome::default(), None)
        }
    }

    /// Paint the pods registered during this pass, above the main tree, and fold
    /// each one's paint outcome back into `outcome`.
    ///
    /// `entries` arrives in paint order (`Floating` band first, then `Tooltip`,
    /// registration order within each). Each pod is painted through a
    /// [`PaintCtx`] whose absolute origin is its own registered
    /// [`window_rect`](crate::overlay::OverlayEntry::window_rect) — not its
    /// owner's origin, which is the whole point of floating — carrying the same
    /// clock, theme, insets, presented count, translucency and focus/hover
    /// seeding the root pod's context carries, so a widget inside a pod cannot
    /// tell it is not in the tree.
    ///
    /// The pod borrow is taken per entry and released before the next: the root
    /// must never hold one across a pass boundary, nor across another entry's
    /// paint (two entries may belong to the same owner).
    ///
    /// It is also the one pass that can act on a floated pod's focus link at
    /// all, because it is the one pass holding the pods. Two things follow, and
    /// both happen here:
    ///
    /// * it **retires** a link the live session has already stranded
    ///   ([`ChildPod::retire_stale_focus_link`](crate::widget::ChildPod::retire_stale_focus_link)),
    ///   so the raw flag consumers that cannot consult an epoch — an owner's own
    ///   `is_focused()` read, a hand-written container's routing — stop seeing a
    ///   record of a session that has moved on;
    /// * it **resolves** which surface, if any, holds a link on the live session,
    ///   and collects that one surface's IME publish. Every other pod's publish
    ///   is dropped where it is taken: paint descends into every pod
    ///   unconditionally and the bubble up `paint_child` carries a published
    ///   surface whatever the publisher's link says, so without this the last pod
    ///   painted would decide what the shell is configured with — a surface
    ///   belonging to a field it has nothing to do with, secure-text
    ///   configuration and all.
    ///
    /// Returns `(holder, holder's publish)` for
    /// [`RenderRoot::resolve_session_surface`] to settle against the main tree's.
    /// **Two holders resolve to none:** a second live link is a contradiction the
    /// mechanism is supposed to make impossible (one claim, one chain, one
    /// stamp), and answering it by picking the topmost would let a surface speak
    /// for a session on evidence that has already failed. Reporting no holder
    /// instead means neither surface's publish is carried out of this pass —
    /// the resolver then reads the pass exactly as it reads one where nothing is
    /// floated at all.
    fn paint_overlays(
        &mut self,
        entries: &[OverlayEntry],
        scene: &mut dyn PaintScene,
        frame_time: FrameTime,
        outcome: &mut PaintOutcome,
    ) -> (Option<OverlayKey>, Option<ImeState>) {
        if entries.is_empty() {
            // Nothing is floated, so nothing floated owns the session.
            return (None, None);
        }
        // The same disjoint field borrows the main pass takes, for the same
        // reason: the theme is lent immutably into each context while other
        // fields of `self` are written.
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

        // The surface whose pod holds a link on the LIVE session, resolved as the
        // pods go past, together with the surface that pod published — one pair,
        // so the publish can never be attributed to an entry that did not make
        // it. A second live holder sets `contested` and the pair is discarded.
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
            // Seeded from the root's own mirrors exactly as the root pod's
            // context is, so a focused editable inside a floated pod observes its
            // focus (and a hovered one its hover) through the ordinary
            // `ChildPod::paint_child` composition.
            //
            // The mirror alone, deliberately: the pod's own link is ANDed onto it
            // inside `paint_child`, which is the same composition that decides
            // the main tree's, so a pod that holds no link reads unfocused
            // whatever the mirror says.
            ctx.set_has_focus(focus_active);
            ctx.set_focus_epoch(focus_epoch);
            ctx.set_hovered(hover_active);
            ctx.set_hover_epoch(hover_epoch);
            // Retired and read before the paint, in the same borrow: nothing in a
            // paint pass moves the focus path, and the answer is what decides
            // whether this pod may describe the session below.
            let pod_holds_focus = {
                let mut pod = entry.pod.borrow_mut();
                pod.retire_stale_focus_link();
                let holds = pod.holds_live_focus();
                pod.paint_child(&mut ctx, scene);
                holds
            };

            // Fold this pod's continuation-frame request into the frame's outcome
            // on the two lattices the tree's own aggregation uses: `needs_frame`
            // ORs, the class is a max-lattice (any unpaced request makes the whole
            // frame unpaced), and the paced interval is a MIN-lattice. The
            // standing aggregate's class is recovered from the outcome itself —
            // `needs_frame && !needs_frame_paced_only` is precisely "something
            // unpaced asked" — which also preserves the deferred-flush frame the
            // main pass may have folded in.
            let stood_unpaced = outcome.needs_frame && !outcome.needs_frame_paced_only;
            let entry_unpaced = ctx.needs_frame() && !ctx.needs_frame_paced_only();
            outcome.needs_frame |= ctx.needs_frame();
            outcome.needs_frame_paced_only =
                outcome.needs_frame && !(stood_unpaced || entry_unpaced);
            outcome.paced_interval = match (outcome.paced_interval, ctx.paced_interval()) {
                (Some(standing), Some(asked)) => Some(standing.min(asked)),
                (standing, asked) => standing.or(asked),
            };
            // A layout-animating widget inside a pod relayouts the next frame the
            // same way one in the tree does, and so does a pod this paint dirtied
            // (an unpaced frame request or a trailing landing paint — the path
            // bit, which `paint_child` already set for both). The floated pod's
            // own mark makes the next pass a full one (see `overlay_pod_dirty`).
            if ctx.needs_layout() {
                outcome.needs_layout = true;
                self.pending |= ChangeFlags::LAYOUT;
            }
            if ctx.is_layout_path_dirty() {
                self.pending |= ChangeFlags::LAYOUT;
            }
            // A focused editable inside a pod republishes its IME surface on every
            // paint, exactly like one in the tree, so the same rules apply
            // verbatim: collect a publish only while a session is actually active
            // (never resurrect a surface a blur cleared), and only from the pod
            // that holds a link on THAT session — the provenance a child of the
            // tree gets for free from its chain. A publish taken from any other
            // pod is dropped here, which is why the take is unconditional: the
            // per-entry context is about to be discarded either way, and leaving
            // a surface in it would only invite a later reader to trust it.
            //
            // An *inactive* publish is refused on exactly the same terms rather
            // than treated as a release: ending a session is a claim about it
            // too, and a pod that does not hold it makes neither.
            let published = ctx.take_ime_state();
            if focus_active && pod_holds_focus {
                if holder.is_some() {
                    // Two pods claiming one session. See the method doc: the
                    // contradiction is answered by attributing the session to
                    // nobody, not by ranking the claimants.
                    contested = true;
                } else {
                    holder = Some((entry.key, published));
                }
            }
            // EXTEND the two replace-per-pass channels rather than replacing them:
            // `paint_main_tree` already put this pass's tree-published frames and
            // shields there, and a platform-view slot or z-shield that happens to
            // paint inside a floated pod must survive beside them (see
            // `PaintCtx::publish_platform_view`).
            self.platform_view_frames.extend(ctx.take_platform_views());
            self.input_shields.extend(ctx.take_input_shields());
        }

        match holder {
            Some((key, published)) if !contested => (Some(key), published),
            _ => (None, None),
        }
    }

    /// Resolve this paint pass's selection-toolbar publish into the shell-facing
    /// slot, moving [`RenderRoot::selection_toolbar_generation`] only on a
    /// **menu edge**.
    ///
    /// The request carries two shapes of fact, and they are resolved differently
    /// (see [`SelectionToolbarRequest`]):
    ///
    /// * The whole request is stored as a **level**, newest wins. A shell reads
    ///   [`SelectionToolbarRequest::anchor`] on every tick it has a menu on
    ///   screen, so the stored anchor has to be the current one, not the one the
    ///   generation last moved for.
    /// * The generation moves on the **menu-significant** part alone:
    ///   [`SelectionToolbarRequest::present_menu`] and
    ///   [`SelectionToolbarRequest::actions`], with an absent request reading as
    ///   "no menu, no verbs" so appearing and disappearing are edges on the same
    ///   comparison. An anchor that merely moved is deliberately **not** an edge:
    ///   a focused field recomputes its anchor every painted frame, and a
    ///   selection dragged wider moves it on every touch sample — bumping there
    ///   would ask the platform to re-present its menu per sample.
    ///
    /// The publish is refused outright while no focus session is active, mirroring
    /// `paint`'s refusal to let a paint-time publish resurrect a cleared IME
    /// surface: the request describes the focused field, so one arriving after the
    /// blur describes a field that no longer holds anything.
    fn resolve_selection_toolbar(&mut self, published: Option<SelectionToolbarRequest>) {
        let next = if self.focus_active { published } else { None };
        // "Nothing published" is the same statement as "no menu wanted, no verbs
        // enabled" — which is what lets one comparison cover a change between two
        // requests, a first appearance, and a clearing alike.
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
    /// recent [`RenderRoot::paint`], or `None` when no field is focused at all.
    ///
    /// The shell half of the platform edit-menu route
    /// ([`SelectionToolbarPolicy::Native`](crate::selection_toolbar::SelectionToolbarPolicy::Native)):
    /// a shell reads it beside [`RenderRoot::ime_state`], answers "may I offer
    /// this verb?" from [`SelectionToolbarRequest::actions`] whenever the platform
    /// asks, and presents the host's own menu at
    /// [`SelectionToolbarRequest::anchor`] when
    /// [`SelectionToolbarRequest::present_menu`] says so. A **level**, not an edge
    /// — re-read it as often as you like; pair it with
    /// [`RenderRoot::selection_toolbar_generation`] to notice the changes worth
    /// presenting or dismissing for.
    ///
    /// Present for a focused field with no selection at all, which is not a
    /// wasted answer: paste applies to a bare caret, and a platform asking
    /// whether it may offer one needs a reply before any bar exists.
    ///
    /// A field under the framework policy publishes this too (it floats its own
    /// toolbar through [`crate::overlay`] as well), so a shell that drives the
    /// platform menu must decide on the policy, not on the presence of a request.
    pub fn selection_toolbar(&self) -> Option<SelectionToolbarRequest> {
        self.selection_toolbar
    }

    /// A monotonically-increasing generation bumped on every change to the
    /// **menu-significant** part of [`RenderRoot::selection_toolbar`] — its
    /// [`present_menu`](SelectionToolbarRequest::present_menu) flag and its
    /// [`actions`](SelectionToolbarRequest::actions) — including the clearing that
    /// a blur produces, so a shell sees the menu going away as an edge too.
    ///
    /// A moved [`anchor`](SelectionToolbarRequest::anchor) is **not** an edge: it
    /// is republished (and recomputed) every painted frame, so a shell re-reads it
    /// from [`RenderRoot::selection_toolbar`] rather than waiting for this to move
    /// — bumping on it would re-present a menu on every touch sample of a drag
    /// that widens a selection.
    ///
    /// The `focus_ime_generation` contract one channel over: a shell caches the
    /// last value it acted on and acts only when it moves, which is what keeps a
    /// standing selection — republished every single frame — from asking the
    /// platform to re-present its menu on every vsync.
    pub fn selection_toolbar_generation(&self) -> u64 {
        self.selection_toolbar_gen
    }
}
