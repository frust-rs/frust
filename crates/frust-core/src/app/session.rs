use super::{RenderRoot, advance_focus_epoch};
use crate::event::{CursorIcon, ImeState, PointerId};
use crate::view::View;

/// Change-guarded write of the shell-facing IME surface: replaces `slot` and
/// bumps `generation` **only** when the value actually moves ([`ImeState`] is
/// `PartialEq`).
///
/// A free function over the two fields rather than a `&mut self` method, so the
/// change guard has exactly one implementation whatever borrows its caller
/// happens to hold; [`RenderRoot::store_ime_state`] is the `&mut self` form, and
/// is what every current caller goes through.
///
/// The change guard is load-bearing, not an optimisation: the paint pass
/// re-publishes the focused widget's IME surface every frame, so an
/// unconditional bump would make the shell's `focus_or_ime_changed` edge fire on
/// every vsync for the whole life of a focus session — the level-input behavior
/// the generation exists to replace.
fn store_ime_state_in(slot: &mut Option<ImeState>, generation: &mut u64, next: Option<ImeState>) {
    if *slot != next {
        *slot = next;
        *generation = generation.wrapping_add(1);
    }
}

/// Release the whole focus/IME session: drop `focus_active` **and** the
/// shell-facing surface together, bumping `generation` **exactly once** if
/// either actually moved.
///
/// The paired form of [`RenderRoot::set_focus_active`]`(false)` +
/// [`RenderRoot::store_ime_state`]`(None)`, and the single primitive every
/// release site goes through — the blur-on-outside-tap `Down`, an explicit
/// [`EventCtx::release_focus`](crate::event::EventCtx::release_focus), a widget
/// publishing an *inactive* surface (see [`RenderRoot::paint`]), and the
/// generic-unmount orphan drain in [`RenderRoot::rebuild`]. Keeping them on one
/// primitive is what makes "a session ends" mean the same thing everywhere,
/// rather than four hand-assembled pairs that can drift apart.
///
/// **One release is one edge.** The two field writers bump on each field's own
/// change, so calling them in sequence would move
/// [`focus_ime_generation`](RenderRoot::focus_ime_generation) *twice* for the
/// ordinary release (focus `true`→`false` and surface `Some`→`None`). A shell
/// only ever compares the value, so two bumps and one bump raise the same single
/// `focus_or_ime_changed` edge — but a counter that moves once per observable
/// transition is the contract the field doc states, and is what the release
/// tests pin. The change guard itself is unchanged: an already-released root
/// writes the same values back and moves nothing.
///
/// **A release ends the session's identity too.** On an actual move it advances
/// `focus_epoch` and republishes it, which strands every recorded focus link at
/// once — the chain the session ran through, and a floated surface's link that
/// no container's blur sweep can reach. That is why the release has to own the
/// epoch rather than leave it to the caller: a session cleared without moving
/// its identity leaves links behind that still name it.
///
/// A free function over the fields (not a `&mut self` method) for the same
/// reason [`store_ime_state_in`] is — one implementation of the contract,
/// whatever borrows the caller holds. [`RenderRoot::release_focus_session`] is
/// the method form, and is what every current caller goes through.
pub(super) fn release_focus_session_in(
    focus_active: &mut bool,
    ime_slot: &mut Option<ImeState>,
    generation: &mut u64,
    focus_epoch: &mut u64,
    root_identity: u64,
) {
    let moved = *focus_active || ime_slot.is_some();
    *focus_active = false;
    *ime_slot = None;
    if moved {
        *generation = generation.wrapping_add(1);
        // A session that ends strands every link recorded against it, wherever
        // in (or off) the tree it sits — the one clearing sweep no container can
        // be asked to run. Only on an actual release: a `Down` on already-blurred
        // chrome is the commonest event there is and must move nothing.
        *focus_epoch = advance_focus_epoch(*focus_epoch);
        crate::widget::set_live_focus_session(root_identity, *focus_epoch, *focus_epoch);
    }
}

impl<State: 'static, V: View<State>> RenderRoot<State, V> {
    /// Whether a captured pointer gesture is currently in flight.
    pub fn is_pointer_captured(&self) -> bool {
        self.capture_claimant.is_some()
    }

    /// The contact that claimed the pointer capture in flight — the only one
    /// whose `Up`/`Cancel` can end it — or `None` while nothing is captured.
    pub fn pointer_capture_claimant(&self) -> Option<PointerId> {
        self.capture_claimant
    }

    /// Whether the capture in flight routes the gesture's **other** contacts to
    /// its captor — the captor opted in with [`EventCtx::capture_contacts`](crate::event::EventCtx::capture_contacts) on
    /// the `Down` it captured with, and no container has since taken the
    /// gesture over from it ([`EventCtx::release_captured_child`](crate::event::EventCtx::release_captured_child)). `false`
    /// while nothing is captured.
    pub fn pointer_capture_contacts(&self) -> bool {
        self.capture_contacts
    }

    /// Whether some widget in the tree currently holds keyboard/IME focus.
    pub fn is_focus_active(&self) -> bool {
        self.focus_active
    }

    /// Whether some widget in the tree currently holds the hover link — i.e.
    /// whether the last hover pass (an uncaptured [`PointerPhase::Move`](crate::event::PointerPhase::Move)) left the
    /// pointer over a widget that claimed it.
    ///
    /// The hover analog of [`RenderRoot::is_focus_active`], and a level accessor
    /// like it: hover is not a session (nothing has to be released), so there is no
    /// generation counterpart. `false` for any app whose widgets never call
    /// [`EventCtx::claim_hover`](crate::event::EventCtx::claim_hover). A touch app
    /// can still see it go `true` transiently: nothing distinguishes a touch
    /// contact from a mouse here, so an uncaptured touch drag over a
    /// non-capturing claimant is an ordinary hover pass — ended by the `Up` at
    /// lift (see `docs/LIMITATIONS.md`'s `hover-window-leave-standing`).
    ///
    /// A [`RenderRoot::rebuild`] that removes the claimant ends the link too, so
    /// this never reports a hover held by a widget that no longer exists — the
    /// hover counterpart of the unmount focus release (see that method).
    pub fn is_hover_active(&self) -> bool {
        self.hover_active
    }

    /// The cursor the tree last asked the host to show — what a desktop shell
    /// pushes to its window (`frust-shell-desktop` maps it onto winit's own
    /// cursor icons).
    ///
    /// A **level** accessor like [`RenderRoot::is_hover_active`], not an edge one:
    /// the value re-resolves on every pointer [`PointerPhase::Move`](crate::event::PointerPhase::Move) and stands
    /// unchanged through every other pass, so a shell caches what it last applied
    /// and calls the platform only when this differs. There is deliberately no
    /// generation counter — a cursor is a *value*, not a session, and an unmoved
    /// cursor is indistinguishable from one re-resolved to the same shape.
    ///
    /// [`CursorIcon::Default`] before the first `Move`, and after any `Move` in
    /// which no widget called
    /// [`EventCtx::set_cursor`](crate::event::EventCtx::set_cursor) — so any app
    /// whose widgets never request a cursor reads `Default` forever, and the mobile
    /// shells never read this at all regardless of what resolves here.
    ///
    /// **Residual:** a widget that is torn down (or moves out from under a
    /// stationary pointer) while its request stands leaves the last shape in
    /// place until the next `Move` re-resolves it — the same self-correction
    /// window hover has, and for the same reason: nothing re-resolves without
    /// pointer motion.
    pub fn cursor(&self) -> CursorIcon {
        self.cursor
    }

    /// Take (and clear) the text the tree asked the shell to put on the host
    /// clipboard — the drain a shell performs immediately after every
    /// [`RenderRoot::event`], beside [`cursor()`](RenderRoot::cursor) and
    /// [`ime_state()`](RenderRoot::ime_state).
    ///
    /// `Some` exactly when some widget called
    /// [`EventCtx::write_clipboard`](crate::event::EventCtx::write_clipboard)
    /// during a pass since the last drain (answering a
    /// [`EditCommand::Copy`](crate::event::EditCommand::Copy)/[`Cut`](crate::event::EditCommand::Cut),
    /// or a chord the widget decoded itself). The shell hands the text to its host
    /// clipboard — winit's `arboard` on desktop, `ClipboardManager` on Android,
    /// `UIPasteboard` on iOS — and does nothing at all on `None`.
    ///
    /// **Destructive**, unlike [`cursor()`](RenderRoot::cursor): a clipboard write
    /// is an edge, not a standing level, so a caller that drains and drops the
    /// result loses that write. Draining twice after one pass yields `None` the
    /// second time.
    ///
    /// A widget that never copies leaves this `None` forever, so a shell with no
    /// clipboard (the mobile shells before their own clipboard work lands) may
    /// call it and discard the result, or not call it at all.
    pub fn take_clipboard_write(&mut self) -> Option<String> {
        self.pending_clipboard_write.take()
    }

    /// Take (and clear) whether the tree asked the shell to read the host
    /// clipboard back to it — drained beside
    /// [`take_clipboard_write`](RenderRoot::take_clipboard_write) after every
    /// [`RenderRoot::event`].
    ///
    /// `true` exactly when some widget called
    /// [`EventCtx::request_paste`](crate::event::EventCtx::request_paste) during a
    /// pass since the last drain. The shell answers by reading its host clipboard
    /// and dispatching
    /// [`InputEvent::EditCommand`](crate::event::InputEvent::EditCommand)`(`[`EditCommand::Paste`](crate::event::EditCommand::Paste)`(text))`
    /// — a *new* dispatch, because the read may be asynchronous and the pass that
    /// asked is over. That answer carries no identity of its own and is
    /// focus-routed to whoever holds focus when it lands: a release in between
    /// drops it harmlessly, but a focus *move* in between lands it in the new
    /// field rather than the one that asked. A synchronous read has no such
    /// window; an asynchronous one snapshots
    /// [`focus_epoch`](RenderRoot::focus_epoch) at this drain and discards an
    /// answer whose epoch no longer matches — not
    /// [`focus_ime_generation`](RenderRoot::focus_ime_generation), which also
    /// moves within a single session.
    ///
    /// **Destructive**, for [`take_clipboard_write`](RenderRoot::take_clipboard_write)'s
    /// reason. A pass may both write and request (a cut that immediately re-reads,
    /// or a widget answering two chords) — the two drains are independent.
    pub fn take_paste_request(&mut self) -> bool {
        std::mem::take(&mut self.pending_paste_request)
    }

    /// The IME surface the focused widget published, for the shell to drive the
    /// platform input method (winit `set_ime_cursor_area`, Android
    /// `updateSelection`, iOS `inputDelegate`). `None` when nothing is focused or
    /// the focused widget publishes no IME surface.
    ///
    /// Written by the focused widget through [`EventCtx::publish_ime_state`](crate::event::EventCtx::publish_ime_state) during
    /// the event pass and refreshed on every event; it survives a rebuild (so the
    /// shell can query it between frames) and is cleared when focus is lost.
    ///
    /// # `None` is the only "no session" form — an inactive surface is never stored
    ///
    /// A widget publishing `ImeState { active: false, .. }` is ending the session,
    /// not describing it, so both publish paths turn that into a full release
    /// (see [`RenderRoot::paint`]) and this returns `None` rather than
    /// `Some(inactive)`. A shell therefore never has to distinguish the two, and
    /// `is_some()` means "a live IME session" with no second check.
    ///
    /// **The platform still sees the keyboard-hide.** All three shells already
    /// map `None` onto the inactive form on the way out, so the observable wire
    /// behavior is unchanged: `frust-shell-android`'s `ime_state_to_json` returns
    /// `ImeJsonState::default()` (`active:false`, empty text, `-1` indices, null
    /// caret, `"normal"`) and `frust-shell-ios`' returns the byte-identical
    /// `ime_state_json(false, "", -1, -1, -1, -1, None, "normal")` — exactly what
    /// the navigator's own cleared surface serialised to before. Kotlin's
    /// `pollImeAfterDispatch` and Swift's `syncImeFocus` both branch on `active`
    /// alone (an inactive surface's text/caret/content-type are ignored), and the
    /// desktop shell's `sync_ime` reads `is_some_and(|s| s.active)`. Dropping the
    /// inactive surface's payload also stops a disabled *secret* field's text
    /// riding to the platform after its session ended.
    pub fn ime_state(&self) -> Option<ImeState> {
        self.ime_state.clone()
    }

    /// The focus/IME session generation — bumped on every **actual** change of
    /// [`is_focus_active`](RenderRoot::is_focus_active) or
    /// [`ime_state`](RenderRoot::ime_state), and on nothing else.
    ///
    /// The *edge* counterpart of those two level accessors, for a shell that
    /// needs "did the focus/IME session move since I last looked?" rather than
    /// "is something focused?". A shell caches the value it last saw and
    /// compares (mirroring [`semantics_generation`](RenderRoot::semantics_generation)'s
    /// cheap dirty gate) — that comparison is the mobile frame gate's
    /// `FrameInputs::focus_or_ime_changed` input.
    ///
    /// A same-value write never moves it: re-publishing an identical IME
    /// surface (which the paint pass does on every frame a field stays focused)
    /// or re-blurring an already-blurred root is not an edge. Wrapping is
    /// deliberate and harmless — a comparison, never an ordering.
    ///
    /// # Not the session's identity
    ///
    /// This counts *changes to the published surface*, not *sessions*, and the
    /// two come apart in both directions — see
    /// [`focus_epoch`](RenderRoot::focus_epoch), which is what to reach for when
    /// the question is "is this still the same focus session?". Answering that
    /// one from this counter is wrong whenever focus moves between two fields
    /// without the published value changing.
    pub fn focus_ime_generation(&self) -> u64 {
        self.focus_ime_gen
    }

    /// The live focus session's **identity** — advanced once per honoured focus
    /// claim and once per session release, and by nothing else.
    ///
    /// The neighbour of [`focus_ime_generation`](RenderRoot::focus_ime_generation)
    /// and easy to mistake for it, so: that one counts *changes to the published
    /// surface* (the focus flag, or the [`ImeState`] value), this one counts
    /// *sessions*. They come apart in both directions, which is why both exist:
    ///
    /// * Focus moving from one field to another moves this one and can leave
    ///   that one completely still. Claiming focus while some field already
    ///   holds it writes `true` over `true`, and the surface the new field
    ///   publishes may compare equal to the old field's ([`ImeState`] is
    ///   `{active, editing, caret, content_type}` and names no widget) — or may
    ///   not be published at all, since a widget is free to take focus and
    ///   publish nothing, which leaves the previous field's surface standing.
    /// * An edit landing, a caret moving, or the field being repositioned under
    ///   the user moves that one and leaves this one still: the session is the
    ///   same session throughout.
    ///
    /// So a caller binding an asynchronous answer to the session that asked for
    /// it wants this one; a caller asking "must I run a frame, or re-sync the
    /// platform IME?" wants that one.
    ///
    /// **Never `0`.** The counter is built at `1` and steps *past* `0` on wrap,
    /// because `0` is a never-claimed [`ChildPod`](crate::widget::ChildPod)'s
    /// stamp and a root publishing it would hand every unclaimed pod in the tree
    /// a live link. A caller is therefore free to use `0` as its own "no root /
    /// no answer" sentinel with no risk of colliding with a live value. Wrapping
    /// is otherwise deliberate and harmless: the value is compared for equality,
    /// never ordered.
    pub fn focus_epoch(&self) -> u64 {
        self.focus_epoch
    }

    /// Set the root's focus flag, bumping [`RenderRoot::focus_ime_generation`]
    /// only when the value actually moves.
    ///
    /// One of the two writers of `focus_active` outside construction (the other
    /// is [`release_focus_session_in`], which clears it together with the IME
    /// surface as one edge): every focus/blur arm of [`RenderRoot::event`] goes
    /// through one of them, so the edge generation cannot drift from the state it
    /// describes. In practice this one only ever *sets* focus — a clear is always
    /// a session release.
    pub(super) fn set_focus_active(&mut self, active: bool) {
        if self.focus_active != active {
            self.focus_active = active;
            self.focus_ime_gen = self.focus_ime_gen.wrapping_add(1);
        }
    }

    /// Store (or clear) the shell-facing IME surface, bumping
    /// [`RenderRoot::focus_ime_generation`] only when the stored value actually
    /// moves — the `&mut self` form of [`store_ime_state_in`], for the event
    /// pass (the paint pass holds disjoint field borrows and calls that
    /// function directly).
    pub(super) fn store_ime_state(&mut self, ime: Option<ImeState>) {
        store_ime_state_in(&mut self.ime_state, &mut self.focus_ime_gen, ime);
    }

    /// End the focus/IME session: clear `focus_active` and drop the shell-facing
    /// surface together, moving [`RenderRoot::focus_ime_generation`] exactly once
    /// if either was set. The `&mut self` form of [`release_focus_session_in`]
    /// (whose doc carries the full contract), for the event and rebuild passes;
    /// the paint pass holds disjoint field borrows and calls that function
    /// directly.
    ///
    /// Idempotent: releasing an already-released root writes the same values
    /// back and fires no edge.
    pub(super) fn release_focus_session(&mut self) {
        release_focus_session_in(
            &mut self.focus_active,
            &mut self.ime_state,
            &mut self.focus_ime_gen,
            &mut self.focus_epoch,
            self.root_identity,
        );
        // No session, no owner. A paint pass that releases re-resolves the owner
        // from the pods before it ends anyway (see
        // `RenderRoot::resolve_session_surface`), so this write is the event and
        // rebuild passes' own.
        self.focus_surface = None;
        // A selection toolbar describes the *focused* field's selection, so the
        // session ending is the toolbar ending — and it must end on the event
        // pass that blurred, not a frame later when the next paint happens to
        // publish nothing. Change-guarded like every other edge here: releasing
        // an already-toolbarless root moves no generation.
        if self.selection_toolbar.take().is_some() {
            self.selection_toolbar_gen = self.selection_toolbar_gen.wrapping_add(1);
        }
    }

    /// Publish this root's live focus session so the pods can compare their own
    /// stamps against it — the focus counterpart of the `(root, epoch)` pair
    /// `crate::event::set_live_hover_link` publishes for hover.
    ///
    /// Called at the head of every pass, not only when the session moves: the
    /// channel mirrors one root, so a second root driving passes on the same
    /// thread would otherwise leave this one's pods comparing against a session
    /// that is none of their business. Re-publishing an unchanged triple costs a
    /// `Cell` write and notifies nothing.
    ///
    /// Publishes the session "at rest" — the live epoch and the epoch a claim
    /// would take are the same value. [`RenderRoot::event`] publishes the two
    /// apart for the length of its dispatch; see that method.
    pub(super) fn publish_focus_session(&self) {
        crate::widget::set_live_focus_session(
            self.root_identity,
            self.focus_epoch,
            self.focus_epoch,
        );
    }

    /// End the standing hover link outright, outside any hover pass: advance the
    /// epoch (which strands every stamp in the tree at once, so no container has
    /// to be told) and clear the mirror.
    ///
    /// Hover's analog of [`RenderRoot::release_focus_session`], and idempotent in
    /// the same way — ending a hover nothing holds writes the same mirror back and
    /// costs one epoch. There is no generation counter to move: hover is not a
    /// session a shell mirrors (see [`RenderRoot::is_hover_active`]).
    ///
    /// The one caller is [`RenderRoot::rebuild`]'s severed-claimant drain; a hover
    /// pass ends its own link inline, where it also decides the *new* one.
    pub(super) fn end_hover_link(&mut self) {
        self.hover_epoch = self.hover_epoch.wrapping_add(1);
        self.hover_active = false;
        self.hover_link = None;
        crate::event::set_live_hover_link(self.root_identity, 0);
    }
}
