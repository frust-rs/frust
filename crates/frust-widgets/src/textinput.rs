//! The `TextInput` interactive widget: an editable text field, single-line by
//! default and optionally wrapped multi-line.
//!
//! [`text_input`] produces a [`TextInputView`] carrying the current `value`, a
//! `placeholder`, an `on_change` callback, and an optional `on_submit`. Like the
//! other interactive widgets it is a **controlled component**
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics): it never owns the durable
//! value. Each edit reports the *requested* text through `on_change`, and the
//! next `rebuild` reconciles the app-confirmed `value` back into the underlying
//! [`TextEditor`] — set-if-different, preserving the selection while the text is
//! unchanged.
//!
//! [`TextInputView::text_style`] sets the content text's style (family/weight/
//! style/size/letter-spacing/line-height, plus color when set explicitly) and
//! never touches the chrome colors or geometry (see *Chrome*). Mirroring
//! [`Text`](crate::TextView)'s [`effective_style`](TextInputWidget::effective_style),
//! a field that did not call `.text_style(...)` gets `on_surface` resolved at LAYOUT
//! time — where `TextInput`, like `Text`, bakes color into the shaped editor
//! state — falling back to black with no theme threaded: explicit > theme >
//! black. Baked-at-layout resolution is safe only under the `set_theme` →
//! `ChangeFlags::LAYOUT` contract (`docs/CODE_STANDARDS.md` Theming), which
//! `RenderRoot::set_theme` guarantees.
//!
//! # Text context ownership
//!
//! Unlike the [`Text`](crate::TextView) leaf (which shapes against the shared
//! `TextContext` threaded through `LayoutCtx`), a `TextInput` must apply edits
//! *synchronously during the event pass*, where no context is threaded, so it owns
//! its own [`TextContext`]: `on_change` and the published [`ImeState`] observe the
//! fresh editing value at once, and layout needs no threaded one.
//!
//! **App fonts still reach it.** `TextContext::new` seeds from `frust-text`'s
//! process-wide app-font record, where every `frust::register_app_fonts` payload
//! lands when the shell drains it. A font registered *later* is picked up by
//! `TextContext::sync_app_fonts` at the top of [`Widget::layout`], which rebuilds
//! the editor so its retained parley layout re-shapes too — the shape cache
//! alone would not cover it.
//!
//! # Focus, IME and blink
//!
//! A `Down` inside the field requests focus, places the caret, and publishes an
//! [`ImeState`] so the shell can drive the platform input method — the focus/IME
//! channel `docs/CORE_ARCHITECTURE.md`'s Focus/IME Lifecycle owns. Keyboard
//! editing (`Key`), IME composition/state-sync (`Ime`) and clipboard commands
//! ([`EditCommand`]) all route down that focus path, never a hit test; after any
//! edit the widget fires `on_change`, resets the caret to visible, and
//! republishes the IME surface.
//!
//! The caret blinks while focused, its phase measured in `paint` from the shell
//! frame clock ([`PaintCtx::frame_time`] — no wall-clock reads in widget code).
//! Its continuation frame is a paced (`CosmeticLoop`) request at the blink's own
//! [`BLINK_MS`] half-period ([`PaintCtx::request_frame_paced_at`]) — its own
//! slower cadence rather than the theme's cap rate, one paint per visibility
//! toggle being all a 500ms blink needs. A concurrent cap-rate request repaints it
//! more often through the MIN-lattice with no visible effect, safe at any cadence
//! because the phase is `frame_time - blink_epoch`
//! ([`caret_visible_at`](TextInputWidget::caret_visible_at)) — a pure function of
//! this frame's own timestamp, not of a delta between painted frames. An
//! edit/focus during the (clockless) event pass flags the blink for reset, the
//! next paint recording the epoch from `frame_time`.
//!
//! `reduce_motion` freezes the caret **visible** (lit, not hidden) rather than
//! mid-blink, and stops requesting blink frames while focused: unlike a purely
//! decorative loop the caret is also the edit-point cue, so freezing it dark
//! would hide where typing lands. The IME surface republishes every painted frame
//! regardless, and blinking resumes once the token clears.
//!
//! # Clipboard and selection commands
//!
//! Copy, cut, paste and select-all reach the field two ways, and both end in the
//! same [`handle_command`](TextInputWidget::handle_command):
//!
//! * as a decoded [`EditCommand`] on [`InputEvent::EditCommand`] — what a shell
//!   dispatches for a platform edit menu, an Android `ACTION_PROCESS_TEXT`, a
//!   hardware clipboard key, or a chord it decided to decode itself;
//! * as a chord this widget decodes from a plain `Key`, because a desktop shell
//!   forwards `Cmd+C` as a keystroke like any other. `ctrl` **or** `meta` plus
//!   `c`/`x`/`v`/`a` (ASCII case-insensitive) covers every desktop platform
//!   uniformly — Flutter is platform-strict here (meta on macOS/iOS, ctrl
//!   elsewhere) and a shell wanting that strictness decodes the chord and sends
//!   an [`EditCommand`] instead. [`NamedKey::Copy`]/[`Cut`](NamedKey::Cut)/
//!   [`Paste`](NamedKey::Paste), `Ctrl+Insert`, `Shift+Insert` and
//!   `Shift+Delete` are the legacy spellings of the same three verbs; `alt` is
//!   never a chord modifier, and any other chorded character is consumed rather
//!   than typed.
//!
//! **The clipboard itself lives in the shell**, so the two directions are
//! asymmetric (see [`EditCommand`]): a copy/cut answers by writing into the
//! pass's clipboard slot ([`EventCtx::write_clipboard`]), while a paste is
//! *asked for* ([`EventCtx::request_paste`]) and arrives on a later pass with
//! its text already read. Nothing here touches a host clipboard.
//!
//! **Refusals are the field's own call.** An obscured field copies and cuts
//! nothing — neither the real buffer nor its bullet mirror is worth handing out
//! — a collapsed selection makes copy and cut no-ops, and a paste sanitised down
//! to nothing ([`frust_text::sanitize_paste`], which denies newlines in a
//! single-line field and normalises CRLF in a multi-line one) inserts nothing.
//! Each is still *handled*: the verb was understood and answered with "nothing".
//! A disabled field never sees a command at all, because it never holds focus.
//! A **read-only** field does see them: it is focusable precisely so its content
//! can be selected and copied, so it answers copy and select-all in full and
//! answers cut and paste with nothing (see "Read-only mode").
//!
//! # Selection gestures and the toolbar
//!
//! Four gestures reach the selection, and only two of them raise the toolbar:
//!
//! * a **stationary long-press** ([`LONG_PRESS_MS`](crate::gesture) held within
//!   [`TOUCH_SLOP`]) selects the word under the press point and opens the
//!   toolbar — Android's gesture, and the only one a touch-only device has;
//! * a **double-tap** (a second press within
//!   [`DOUBLE_TAP_MS`](crate::gesture) and [`TOUCH_SLOP`] of the last one)
//!   selects the word and deliberately opens **nothing**: it is a selection
//!   gesture, and a bar appearing under a word the user is about to type over
//!   is in the way;
//! * a **tap inside an existing selection** keeps that selection and toggles
//!   the toolbar on the release — fire-on-up-inside like every other baseline
//!   widget, so a drag that starts inside a selection is still an ordinary
//!   caret drag;
//! * a **secondary press** claims focus (starting a session if there was none —
//!   the desktop context-menu gesture; no mobile shell delivers `Secondary`),
//!   moves no caret, and toggles the toolbar.
//!
//! Everything else puts it away: any text change, a blur or focus release, a
//! scroll, Escape, any primary `Down`, and any applied [`EditCommand`]. A
//! `Cancel` is the one exception — it clears the in-flight gesture and touches
//! neither the selection nor the toolbar, per the never-mutate-on-cancel
//! convention.
//!
//! **The long-press timer is measured across paints**, exactly as
//! [`crate::gesture`]'s is and for the same reason: only the paint pass carries
//! a clock ([`PaintCtx::frame_time`]). A press records its epoch on the first
//! paint after the `Down`, and the paint that observes the threshold crossed
//! marks the hold elapsed, latches
//! [`frust_core::mark_pending_result_flush`] and requests a plain
//! continuation frame — plain, not the blink's paced one, because a long-press
//! must fire at its threshold in wall time even on a frame-gated shell, and
//! even while `reduce_motion` has frozen the blink. The word is actually
//! selected on the next pass carrying an [`EventCtx`]: the
//! [`InputEvent::Housekeeping`] broadcast that flush produces, or an in-slop
//! `Move`/`Up` that arrives first.
//!
//! **The press runs through explicit phases** ([`Gesture`]), because a press
//! that a long-press already resolved is neither a tap nor a drag and must not
//! be mistaken for either. A primary `Down` starts it as `Tap`; wandering past
//! [`TOUCH_SLOP`] turns it into `Drag`; firing the hold turns it into
//! `HoldFired`, which **keeps the press point** so the slop guard still has
//! something to measure against while the finger stays down. That last part is
//! the whole reason the phase exists: with the point simply forgotten at the
//! fire, the very next `Move` — even a sub-pixel one — fell through to the
//! caret-drag path and re-resolved the selection from wherever the pointer
//! now was. `TOUCH_SLOP` is 18 logical px, several characters at a normal text
//! size, so that reached across a word boundary: a finger the user was holding
//! deliberately still could silently widen its own selection while the toolbar
//! stood over it advertising verbs computed from the narrower one.
//!
//! **Post-hold drag semantics are deliberate**, not inherited from the
//! caret-drag fall-through. A finger still down after a long-press:
//!
//! * **within** [`TOUCH_SLOP`] of the press point — does *nothing*. The word
//!   the hold selected stays exactly as it is, however much the finger jitters.
//! * **past** [`TOUCH_SLOP`] — extends the selection **by whole words**, and
//!   keeps tracking the finger in both directions (dragging back toward the
//!   press point shrinks it again rather than sticking at its widest). This is
//!   Android's long-press-drag behaviour.
//!
//! That word granularity is the *editor's* retained selection anchor doing the
//! work, not an op this widget picks per move: `EditOp::SelectWordAtPoint`
//! leaves the selection word-anchored, and the `EditOp::MoveToPoint { select:
//! true }` each later move issues extends from that anchor at the granularity
//! it was anchored with — the same op after a plain caret press extends by
//! cluster instead. It is therefore an assumption about the text engine rather
//! than a local invariant, and it is pinned by a test
//! (`a_drag_out_of_a_fired_hold_extends_by_word_and_tracks_back`) so a text-engine
//! change that dropped it would fail loudly here instead of quietly truncating
//! every long-press drag to the cluster under the pointer.
//!
//! **The double-tap window keeps its own clock alive under `reduce_motion`.**
//! Both halves of that window are dated from the paint clock
//! ([`PaintCtx::frame_time`], cached as the event pass's only clock), which
//! advances only while something is painting — normally the caret blink's own
//! paced request. `reduce_motion` stops those requests, so an idle focused
//! field would leave the clock frozen at the moment the tap completed and the
//! window would never elapse: a press arriving any amount of wall time later
//! would still measure zero and resolve as a double-tap. While a tap is still
//! inside its window and the blink is frozen, `paint` therefore requests plain
//! continuation frames of its own, exactly as the long-press timer does and for
//! the same reason. Outside `reduce_motion` the blink already pumps the clock,
//! so the window is quantized to the blink's 500ms cadence rather than measured
//! to the millisecond — deliberate coarseness, not a second stall.
//!
//! **The toolbar itself is somebody else's widget.** The field hosts an
//! [`OverlaySlot`] and fills it from the process-wide
//! [`selection_toolbar_builder`], so `frust-widgets` never names the view that
//! floats: no builder installed means no pod and no toolbar. The pod is mounted
//! and dropped in `rebuild` (the only pass with a `BuildCtx`) and only when the
//! *action set* changes, placed in `paint` against the selection's bounding box
//! — the union of the displayed
//! [`selection_rects`](frust_text::TextEditor::selection_rects), or the caret
//! rect when the selection is collapsed. Its verbs are the field's own call:
//! copy/cut need a selection and a field that is not
//! [`obscured`](TextInputView::obscured), cut and paste need an interactive
//! one, and select-all needs text that is not already wholly selected.
//!
//! Under [`SelectionToolbarPolicy::Native`] the field floats nothing and only
//! publishes the request ([`PaintCtx::publish_selection_toolbar`]) for the
//! shell to hand to the platform's own edit menu — but it publishes that
//! request under **both** policies, so the two routes diverge downstream of one
//! code path. The publish happens on **every** paint of a focused field, bar or
//! no bar: the verbs are a level a platform responder chain reads whenever it
//! likes (a hardware Cmd+C never raises a bar first), and
//! [`SelectionToolbarRequest::present_menu`] is the single edge inside it that
//! says a menu is wanted now. A verb the pod dispatches
//! ([`EventCtx::dispatch_edit_command`]) is drained in the same pass by
//! [`EventCtx::take_edit_commands`] and applied through the same
//! [`handle_command`](TextInputWidget::handle_command) a keyboard chord takes,
//! under the same focus gate the shell-delivered route enforces — the pod is
//! only ever mounted over a focused field, and the two routes into
//! `handle_command` must not disagree about when a verb may land.
//!
//! # The clipboard verbs and assistive technology
//!
//! The floated toolbar is a **pointer affordance**. It appears only after a
//! long-press or a tap inside a selection, and the portal deliberately
//! contributes no semantics for the pod it floats, so nothing about it is
//! reachable by a screen reader. The verbs are therefore published on the
//! field's **own** [`Widget::semantics`] node instead, as accesskit *custom*
//! actions (`accesskit::Action` has no Copy/Cut/Paste/SelectAll of its own —
//! each verb is a stable id plus a label the client reads out, see
//! [`A11Y_CUT_ID`]).
//!
//! Two rules make that route trustworthy:
//!
//! * **It never depends on the bar being up.** The actions are published from
//!   [`toolbar_actions`](TextInputWidget::toolbar_actions) alone — the same
//!   predicates the bar is built from — and not from `toolbar_open`. Gating
//!   them on the bar would mean the verbs existed only for a user who had
//!   already performed the pointer gesture that raises it. The platform
//!   edit-menu route obeys the same rule, for the same reason and by the same
//!   means (see `paint`'s publish).
//! * **Only enabled verbs are offered.** An obscured field publishes no
//!   copy/cut, a non-interactive one no cut/paste, and a fully-selected or
//!   empty field no select-all, exactly as the bar refuses them. An action
//!   offered and then refused is worse than one never offered.
//!
//! **The selection range itself is not published.** accesskit models a text
//! selection as a pair of `TextPosition`s, and a `TextPosition` must name a
//! node whose role is `Role::TextRun` — this field contributes a single leaf
//! node carrying its text as a plain value, with no per-run child nodes for
//! those positions to point at. Publishing a range would mean restructuring the
//! field's semantics into a text-run subtree, which is a larger change than
//! this one and is not attempted here; the gap is stated rather than papered
//! over with an invented range.
//!
//! **What is still missing is the dispatch, and it does not live here.** A
//! platform adapter reports an invoked custom action as an
//! `accesskit::ActionRequest` carrying `Action::CustomAction` plus the id in
//! its `data`, but the shell-to-core seam
//! (`AppTree::perform_accessibility_action`) forwards only `(node_id, action)`
//! and drops `data`, and `RenderRoot::perform_accessibility_action` models only
//! `Click` and `Focus`. Until both are widened, these actions are advertised
//! but cannot be delivered. The field's own half is complete: every verb has a
//! route into [`handle_command`](TextInputWidget::handle_command) the moment
//! one arrives as an [`InputEvent::EditCommand`], which is exactly what a shell
//! already dispatches for a platform edit menu.
//!
//! # Multi-line mode
//!
//! [`TextInputView::multiline(max_visible_lines)`](TextInputView::multiline)
//! feeds the layout width from the incoming constraints to the editor as a
//! soft-wrap width (`TextEditor::set_wrap_width`), so the field grows vertically
//! one line at a time as content wraps or newlines are inserted, capped at
//! `max_visible_lines`. Past the cap the box height freezes and the text scrolls
//! vertically by a **keep-caret-in-view** paint offset recomputed statelessly
//! each pass from the caret's line rect (momentum and a scrollbar are out of
//! scope), a rectangular clip keeping the overflow inside.
//!
//! Enter is governed by [`submit_on_enter`](TextInputView::submit_on_enter):
//! `true` (the single-line default) fires `on_submit`, `false` (the multi-line
//! default) inserts a literal newline, and **Shift+Enter always does the opposite
//! of the mode's default** (a single-line field ignores the knob and always
//! submits). On the mobile IME path a Return arrives as a `Commit` of a newline,
//! which submit-on-enter fields submit and a newline-inserting one inserts.
//!
//! # Disabled mode
//!
//! [`TextInputView::enabled(false)`](TextInputView::enabled) makes the field
//! inert and dims it. Inertness hangs off **one** hook, the focus gate: a `Down`
//! in a disabled field neither requests focus nor captures the pointer, and since
//! `Key`/`Ime` reach a widget only along the recorded focus path, refusing focus
//! makes keyboard and IME editing impossible with no per-handler guard. A field
//! disabled *while* focused releases the focus path on the first event reaching
//! it and stops behaving as focused (no caret, no blink frame, no active IME
//! surface) from the very next paint.
//!
//! Dimming multiplies the *resolved* role color's alpha rather than swapping in a
//! dedicated "disabled" token, at **both** resolution points (the layout-baked
//! glyph color in [`effective_style`](TextInputWidget::effective_style) and the
//! paint-time [`Chrome`]), so it behaves identically under every catalog and the
//! unthemed fallbacks: `on_surface_variant` is opaque under Material/Glyph but
//! translucent under Cupertino (`frust-theme`'s `ColorScheme` Cupertino arm,
//! `secondaryLabel` at alpha 153), so it is *not* a portable disabled token, and
//! it is keyed **strictly** off [`TextInputView::enabled`] at both points.
//!
//! # Read-only mode
//!
//! [`TextInputView::read_only(true)`](TextInputView::read_only) makes the field
//! uneditable **without** dimming it — the presentation `enabled(false)` cannot
//! express, since disabled conflates two orthogonal questions (editable? /
//! dimmed?) a static-but-live-styled mock must keep apart (a splash screen's
//! frozen preview must not pop to full alpha when it goes live).
//!
//! **A read-only field is focusable and copyable** — Material 3's and Apple's
//! HIG's convention, and the plain reading of a value on screen: text a user can
//! read is text they can select and copy. Two hooks say so where there used to
//! be one: [`TextInputWidget::focusable`] is `enabled` (the top of
//! [`Widget::event`], the focused-while-painting check) and
//! [`TextInputWidget::editable`] is `enabled && !read_only` (every path that
//! changes the text, plus the keyboard hint below). So a read-only field takes
//! focus on a press, places its caret, drag-selects, long-presses to a toolbar
//! offering copy and select-all, and answers the copy/select-all chords in full,
//! while it refuses typed characters, IME composition and commits, and the
//! editing and caret-motion keys. Cut and paste it answers with nothing — and a
//! paste chord it answers *without* asking the shell to read the host clipboard
//! at all: the ask itself reaches the host, and on iOS it is one of the gestures
//! that can raise the system's paste prompt, which a field that would discard
//! the answer has no business provoking. Escape still ends the session, a field
//! that can hold one needing a keyboard way out of it. Its caret is drawn but
//! does **not** blink: a blink advertises an insertion point, and this field
//! takes no insertion.
//!
//! **The keyboard.** A focused read-only field publishes
//! `ImeState { active: true, suppress_soft_keyboard: true, .. }`. Active,
//! because every shell's clipboard route hangs off the platform surface (the web
//! overlay `<input>`'s DOM `copy` listener, Android's `InputConnection`, iOS's
//! first responder) and dies with it; suppressed, because there is nothing here
//! to type into.
//!
//! The hint is an **obligation on the shell**, and it is the shell's half that
//! makes the pair mean anything: a shell that raises an on-screen keyboard must
//! read `suppress_soft_keyboard` and, when it is set, keep the platform input
//! surface it would build for `active` while leaving that keyboard down. A shell
//! with no on-screen keyboard of its own (desktop/winit) has nothing to do for
//! it.
//!
//! **No shell reads it yet.** `active` alone still drives the platform keyboard
//! on both mobile shells, and neither `ime_state_to_json` puts this field on the
//! wire at all, so it cannot reach Kotlin or Swift even in principle. Until each
//! shell is taught the hint, tapping a read-only field on Android or iOS raises
//! the soft keyboard — where before this field took no focus, it raised nothing.
//! That is a known, tracked gap in the shells, not a contract this module is
//! quietly failing to keep: everything above the seam publishes the hint
//! correctly, and a test pins it.
//!
//! `enabled(false)` remains the stronger claim, unchanged: it refuses focus
//! outright, so none of the above reaches a disabled field, and a field disabled
//! while focused still releases focus and deactivates the IME. **Dimming stays
//! keyed to `enabled` alone**, never to either hook: both resolution points
//! ([`Chrome::resolve`] and
//! [`effective_style`](TextInputWidget::effective_style)) branch on `enabled`.
//!
//! **Semantics.** Read-only is a real accessibility distinction from disabled —
//! a screen reader announces them differently — so [`Widget::semantics`] reports
//! `set_disabled()` only when `!enabled` and `set_read_only()` when
//! `enabled && read_only`, never both, and never by omitting the node
//! (`docs/CODE_STANDARDS.md` Semantics: a node with something to say keeps it).
//!
//! Read-only is orthogonal to [`obscured`](TextInputView::obscured) — never
//! touching masking, the mirror or the published `ImeContentType` hint, so a
//! read-only obscured field still reports `Role::PasswordInput` with a masked
//! a11y value — and to the *Chrome* geometry setters. The two refusals compose
//! rather than cancel: an obscured field hands out neither its secret nor its
//! bullet mirror, so a read-only obscured one is focusable and copies nothing.
//!
//! # Obscured (password) mode
//!
//! [`TextInputView::obscured(true)`](TextInputView::obscured) masks the rendered
//! glyphs with U+2022 BULLET. The [`TextEditor`]'s model text stays **real** —
//! every edit, selection, IME sync and `on_change` runs against the true buffer —
//! while the masked mirror lives in a parallel [`TextEditor`] (`mask_editor`).
//! That mirror, not the real editor, is measured, painted and hit-tested, since
//! masking only at glyph-emission time would leave the layout (and so the caret
//! rect, the field width and the pointer hit test) measured from the real text.
//! The mirror is a pure function of the real editing state, recomputed after every
//! edit, so it cannot drift; the mask is 1:1 per `char`, so offsets map with a
//! two-string walk ([`real_to_masked`]/[`masked_to_real`]) and a multi-byte
//! grapheme masks to exactly one bullet.
//!
//! **Scope boundary.** Obscuring is visual masking plus a [`Role::PasswordInput`]
//! semantics node plus an IME content-type hint: `obscured(true)` publishes
//! [`ImeContentType::Password`] on [`ImeState::content_type`], computed live on
//! every publication (event pass, paint-pass republish, disabled-while-focused
//! release), so a newly focused obscured field never has a window where it
//! publishes `Normal`. The published [`ImeState`] still carries the **real** text
//! (the platform IME mirror requires it), and the hint, not redaction, is what is
//! supposed to keep the platform's suggestion strip and learned-word dictionary
//! from seeing it — **a guarantee only as good as the shells honouring the
//! hint**, which this widget can ask for but never prove. `obscured` is the
//! field's sole content-type signal (see [`ImeContentType`] on why a shell must
//! not fall back to non-secret behaviour for an unrecognized variant).
//!
//! # Chrome
//!
//! The field's chrome **colors** (background/border/focus-accent/placeholder/
//! selection/caret) resolve from the active [`Theme`]'s `ColorScheme` (see
//! [`Chrome::resolve`]), so an app restyles them by installing a different
//! `Theme`. Its **geometry** ([`PAD_X`]/[`PAD_Y`] inner padding, [`BORDER_W`]
//! border thickness, [`RADIUS`] corner radius, [`CARET_W`] caret width) is
//! instead reachable per-instance, through [`TextInputView::padding`] and its
//! `border_width`/`corner_radius`/`caret_width` siblings; each defaults to the
//! constant it overrides, so a field calling none renders exactly as the defaults
//! do. Colors stay theme-only; geometry is builder-set rather than tokenized
//! because
//! [`PAD_X`]/[`CARET_W`] are read from the **event pass**
//! ([`TextInputWidget::editor_point`], [`TextInputWidget::current_ime_state`]),
//! which threads no `Theme` — `docs/CODE_STANDARDS.md` Theming cites these very
//! constants as that precedent — so a token would duplicate them anyway.
//!
//! **Focus treatment.** The focused state is an accent-colored border, no
//! separate halo/glow. [`TextInputView::focus_ring_width`] is the one escape hatch
//! on top — an optional border width used only while focused, defaulting to the
//! idle width — so an app reaches the "thicker, differently-colored focus outline"
//! look via a theme's `primary` role without a soft-glow primitive here.

use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::{Action, CustomAction, Role};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, EditCommand, EditingState, EventCtx,
    EventResult, FrameTime, ImeContentType, ImeEvent, ImeState, InputEvent, Key, LayoutCtx,
    NamedKey, OutsideTap, OverlayBand, OverlayInput, PaintCtx, PaintScene, PointerPhase,
    SelectionToolbarActions, SelectionToolbarPolicy, SelectionToolbarRequest, SemanticsCtx,
    TOUCH_SLOP, View, Widget, selection_toolbar_builder, selection_toolbar_policy,
};
use frust_text::{
    EditOp, EditingStateBytes, TextContext, TextEditor, TextStyle, sanitize_paste, utf16_to_byte,
};
use frust_theme::Theme;
use kurbo::{Point, Rect, Size, Vec2};
use peniko::Color;

use crate::authoring::presses;
use crate::gesture::{DOUBLE_TAP_MS, LONG_PRESS_MS};
use crate::overlay::{OverlayAlign, OverlayAnchor, OverlayPlacement, OverlaySide, OverlaySlot};

/// Default corner radius of the field chrome, in logical px. Overridable
/// per-instance with [`TextInputView::corner_radius`] — see the module docs'
/// "Chrome" section for why this is a builder default rather than a `Theme`
/// token.
const RADIUS: f64 = 6.0;
/// Default border thickness, in logical px. Overridable per-instance with
/// [`TextInputView::border_width`] (see the module docs' "Chrome" section);
/// the focused border additionally honors [`TextInputView::focus_ring_width`].
const BORDER_W: f64 = 1.5;
/// Default horizontal inner padding (chrome edge to text), in logical px.
/// Overridable per-instance with [`TextInputView::padding`] (see the module
/// docs' "Chrome" section). Read from the event pass as well as layout/paint
/// (`docs/CODE_STANDARDS.md`'s Theming conventions cite this constant as the
/// precedent for why such a metric stays a plain value, not a `Theme` token).
const PAD_X: f64 = 8.0;
/// Default vertical inner padding (chrome edge to text), in logical px.
/// Overridable per-instance with [`TextInputView::padding`] — see [`PAD_X`].
const PAD_Y: f64 = 6.0;
/// Default caret width, in logical px. Overridable per-instance with
/// [`TextInputView::caret_width`] — see [`PAD_X`] for why it stays a plain
/// value rather than a `Theme` token.
const CARET_W: f32 = 1.5;
/// Blink half-period: caret visible 500 ms, hidden 500 ms.
const BLINK_MS: f64 = 500.0;
/// Default field width when the incoming constraints are horizontally unbounded.
const DEFAULT_WIDTH: f64 = 200.0;
/// Gap between the selection's bounding box and the floated toolbar, in logical
/// px — wider than [`crate::DEFAULT_OFFSET`]'s neutral 4px because this anchor
/// is a run of text rather than a widget's own edge, and a bar sitting 4px off
/// a line of glyphs reads as touching it.
const TOOLBAR_GAP: f64 = 8.0;

/// Field background (unthemed fallback; a theme resolves this from `surface`).
const BG: Color = Color::WHITE;
/// Idle (unfocused) border color (unthemed fallback; themed from `outline`).
const BORDER: Color = Color::from_rgb8(0xD1, 0xD5, 0xDB);
/// Focused border color (unthemed fallback; themed from `primary`).
const ACCENT: Color = Color::from_rgb8(0x3B, 0x82, 0xF6);
/// Placeholder text color (unthemed fallback; themed from `on_surface_variant`).
const PLACEHOLDER: Color = Color::from_rgb8(0x9C, 0xA3, 0xAF);
/// Selection highlight color (unthemed fallback; themed from `primary` at alpha).
const SELECTION: Color = Color::from_rgb8(0xBF, 0xDB, 0xFE);
/// Caret color (unthemed fallback; themed from `primary`).
const CARET: Color = Color::from_rgb8(0x1D, 0x4E, 0xD8);
/// Alpha applied to `primary` for the themed selection highlight (a v1
/// simplification — a translucent primary stands in for a dedicated selection
/// role).
const SELECTION_ALPHA: f32 = 0.30;

/// Alpha multiplier applied to every resolved **content** color (glyphs,
/// placeholder, caret, selection) while the field is disabled.
///
/// Material 3's disabled state puts content at 38% of its enabled role color
/// (`m3.material.io` — *Styles → Color → Roles*, disabled-content opacity;
/// retrieved 2026-07-31). Applied as a *multiplier on the already-resolved
/// role*, never as a swap to a different token: `on_surface_variant` is opaque
/// under Material/Glyph but translucent under Cupertino (alpha 153 — see
/// `frust-theme`'s `ColorScheme` Cupertino arm), so a token swap would dim by
/// different amounts per design language while this multiplier does not.
const DISABLED_CONTENT_ALPHA: f32 = 0.38;
/// Alpha multiplier applied to the disabled field's **container** chrome (its
/// outline/accent border). Material 3 puts a disabled container/outline at 12%
/// (same source and retrieval date as [`DISABLED_CONTENT_ALPHA`]).
const DISABLED_CONTAINER_ALPHA: f32 = 0.12;

/// The glyph every character is replaced with in obscured (password) mode.
///
/// U+2022 BULLET is Android's `inputType=textPassword` default mask; the web
/// varies by browser and Apple's HIG names no glyph, so this follows the one
/// platform that publishes a specific character.
const MASK_CHAR: char = '\u{2022}';

/// The resolved text-field chrome colors. Themed (v1 simplification): background
/// `surface`, border `outline`, focus accent/caret `primary`, placeholder
/// `on_surface_variant`, selection `primary` at [`SELECTION_ALPHA`]. Unthemed:
/// the [`BG`]/[`BORDER`]/[`ACCENT`]/[`PLACEHOLDER`]/[`SELECTION`]/[`CARET`]
/// constants exactly, so a pre-theme app renders unchanged.
///
/// A disabled field dims the *resolved* values (see
/// [`DISABLED_CONTENT_ALPHA`]), so the themed and unthemed paths dim by the
/// same rule.
struct Chrome {
    bg: Color,
    border: Color,
    accent: Color,
    placeholder: Color,
    selection: Color,
    caret: Color,
}

impl Chrome {
    fn resolve(theme: Option<&Theme>, enabled: bool) -> Self {
        let mut chrome = match theme {
            Some(theme) => {
                let s = theme.scheme();
                Chrome {
                    bg: s.surface,
                    border: s.outline,
                    accent: s.primary,
                    placeholder: s.on_surface_variant,
                    selection: s.primary.with_alpha(SELECTION_ALPHA),
                    caret: s.primary,
                }
            }
            None => Chrome {
                bg: BG,
                border: BORDER,
                accent: ACCENT,
                placeholder: PLACEHOLDER,
                selection: SELECTION,
                caret: CARET,
            },
        };
        if !enabled {
            // `bg` is deliberately left opaque: it is this field's own
            // background painted over an arbitrary parent, so thinning it to
            // M3's 12% container value would show the parent through the field
            // rather than reading as "dimmed". The disabled cue is carried by
            // the outline and the content instead.
            chrome.border = chrome.border.multiply_alpha(DISABLED_CONTAINER_ALPHA);
            chrome.accent = chrome.accent.multiply_alpha(DISABLED_CONTAINER_ALPHA);
            chrome.placeholder = chrome.placeholder.multiply_alpha(DISABLED_CONTENT_ALPHA);
            chrome.selection = chrome.selection.multiply_alpha(DISABLED_CONTENT_ALPHA);
            chrome.caret = chrome.caret.multiply_alpha(DISABLED_CONTENT_ALPHA);
        }
        chrome
    }
}

/// The masked mirror of `text`: one [`MASK_CHAR`] per `char`, with `'\n'`
/// preserved so a multi-line field keeps its line structure (and therefore its
/// height) under masking.
fn mask_text(text: &str) -> String {
    text.chars().map(mask_char).collect()
}

/// The single character `ch` renders as while obscured.
fn mask_char(ch: char) -> char {
    if ch == '\n' { '\n' } else { MASK_CHAR }
}

/// Byte offset in the masked mirror of `text` corresponding to byte offset
/// `byte` in `text` itself.
///
/// An offset landing inside a multi-byte char snaps back to that char's start
/// (mirroring `frust_text`'s `byte_to_utf16`), so caret arithmetic across a
/// multi-byte grapheme stays exact.
fn real_to_masked(text: &str, byte: usize) -> usize {
    let mut masked = 0usize;
    for (off, ch) in text.char_indices() {
        if byte < off + ch.len_utf8() {
            return masked;
        }
        masked += mask_char(ch).len_utf8();
    }
    masked
}

/// The inverse of [`real_to_masked`]: a byte offset in the masked mirror mapped
/// back onto `text`. An offset inside a mask glyph snaps back to the start of
/// the char it stands for.
fn masked_to_real(text: &str, masked_byte: usize) -> usize {
    let mut masked = 0usize;
    for (off, ch) in text.char_indices() {
        let next = masked + mask_char(ch).len_utf8();
        if masked_byte < next {
            return off;
        }
        masked = next;
    }
    text.len()
}

/// A view-held, typed text callback (erased on build).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative text field, single-line by default. See the [module docs](self).
pub struct TextInputView<State: 'static> {
    value: String,
    placeholder: String,
    text_style: TextStyle,
    /// Whether the app set the content style explicitly (via
    /// [`TextInputView::text_style`]). When `false` and a theme is active, the
    /// glyph color resolves from the theme's `on_surface` role; an explicit
    /// style always wins (explicit > theme > black fallback).
    text_style_explicit: bool,
    /// `None` = single-line; `Some(n)` = wrapped multi-line capped at `n`
    /// visible lines (see [`TextInputView::multiline`]).
    max_visible_lines: Option<usize>,
    /// Whether Enter submits (vs. inserts a newline). `None` = use the
    /// mode-default (true single-line, false multi-line); `Some(_)` = an
    /// explicit [`TextInputView::submit_on_enter`] override.
    submit_on_enter: Option<bool>,
    /// Whether the field accepts input. `false` refuses focus (making keyboard
    /// and IME editing unreachable) and dims the chrome — see
    /// [`TextInputView::enabled`].
    enabled: bool,
    /// Whether the field refuses *editing* (never focus) without dimming — see
    /// [`TextInputView::read_only`]. Independent of `enabled`: dimming stays
    /// keyed to `enabled` alone (see the [module docs](self)' "Read-only mode"
    /// section).
    read_only: bool,
    /// Whether the rendered glyphs are masked (password mode) — see
    /// [`TextInputView::obscured`].
    obscured: bool,
    /// Horizontal inner padding — see [`TextInputView::padding`]. Defaults to
    /// [`PAD_X`].
    pad_x: f64,
    /// Vertical inner padding — see [`TextInputView::padding`]. Defaults to
    /// [`PAD_Y`].
    pad_y: f64,
    /// Border thickness — see [`TextInputView::border_width`]. Defaults to
    /// [`BORDER_W`].
    border_width: f64,
    /// Corner radius — see [`TextInputView::corner_radius`]. Defaults to
    /// [`RADIUS`].
    corner_radius: f64,
    /// Caret width — see [`TextInputView::caret_width`]. Defaults to
    /// [`CARET_W`].
    caret_width: f32,
    /// Focused-only border width override — see
    /// [`TextInputView::focus_ring_width`]. `None` = use `border_width` while
    /// focused too (unchanged appearance).
    focus_ring_width: Option<f64>,
    on_change: OnText<State>,
    on_submit: Option<OnText<State>>,
}

/// Create a controlled text field showing `value` that fires
/// `on_change(state, new_text)` on every edit.
///
/// The field is a controlled component: it reports the requested text through
/// `on_change` and adopts the app-confirmed `value` on the next rebuild — it is
/// never its own source of truth. Add a submit handler with
/// [`TextInputView::on_submit`] and a placeholder with
/// [`TextInputView::placeholder`].
pub fn text_input<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    on_change: F,
) -> TextInputView<State> {
    TextInputView {
        value: value.into(),
        placeholder: String::new(),
        text_style: TextStyle::default(),
        text_style_explicit: false,
        max_visible_lines: None,
        submit_on_enter: None,
        enabled: true,
        read_only: false,
        obscured: false,
        pad_x: PAD_X,
        pad_y: PAD_Y,
        border_width: BORDER_W,
        corner_radius: RADIUS,
        caret_width: CARET_W,
        focus_ring_width: None,
        on_change: Rc::new(on_change),
        on_submit: None,
    }
}

/// PascalCase alias for [`text_input`].
#[allow(non_snake_case)]
pub fn TextInput<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    on_change: F,
) -> TextInputView<State> {
    text_input(value, on_change)
}

impl<State: 'static> TextInputView<State> {
    /// Set the placeholder shown when the field is empty and unfocused.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set the submit handler fired on Enter (with the current text); focus is
    /// kept.
    pub fn on_submit<F: Fn(&mut State, String) + 'static>(mut self, on_submit: F) -> Self {
        self.on_submit = Some(Rc::new(on_submit));
        self
    }

    /// Set the content text's style (family/weight/style/size/color/
    /// letter-spacing/line-height), applied to the field's [`TextEditor`].
    /// Marks the color as explicitly set, so it wins over the themed default
    /// (explicit > theme > black fallback — see the [module docs](self)).
    /// Default (no call) resolves the color from the active theme's
    /// `on_surface` role, falling back to black with no theme threaded. Does
    /// not affect the chrome colors or padding/caret constants.
    pub fn text_style(mut self, text_style: TextStyle) -> Self {
        self.text_style = text_style;
        self.text_style_explicit = true;
        self
    }

    /// Make this a wrapped multi-line field that grows up to `max_visible_lines`
    /// lines tall, then scrolls internally to keep the caret in view (see the
    /// [module docs](self)). `max_visible_lines` is clamped to at least 1.
    ///
    /// Switches the default Enter behavior to insert a newline rather than
    /// submit; override with [`submit_on_enter`](Self::submit_on_enter).
    pub fn multiline(mut self, max_visible_lines: usize) -> Self {
        self.max_visible_lines = Some(max_visible_lines.max(1));
        self
    }

    /// Set whether Enter submits (`true`) or inserts a newline (`false`),
    /// overriding the mode default (submit single-line, newline multi-line).
    /// Shift+Enter always does the opposite. A single-line field always submits
    /// on Enter regardless of this setting.
    pub fn submit_on_enter(mut self, submit_on_enter: bool) -> Self {
        self.submit_on_enter = Some(submit_on_enter);
        self
    }

    /// Set whether the field accepts input (default `true`).
    ///
    /// A disabled field refuses focus, so a tap neither places the caret nor
    /// raises the keyboard and keyboard/IME editing cannot reach it at all; it
    /// paints no caret and dims its text, placeholder and outline (see the
    /// [module docs](self)). A field disabled while focused drops its focus.
    ///
    /// This is **disabled**, not *read-only*: a read-only field stays focusable
    /// and copyable (Material 3's and Apple's HIG's convention — see
    /// [`read_only`](Self::read_only)), where this option refuses focus
    /// outright.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set whether the field is read-only (default `false`): its text cannot be
    /// changed, but it is still focusable and copyable, and it paints at **full
    /// alpha** rather than dimmed.
    ///
    /// This is the presentation `enabled(false)` cannot express: an undimmed
    /// field that takes no edits, e.g. a static mock that should look identical
    /// before and after a live handoff. Unlike
    /// [`enabled(false)`](Self::enabled) it suppresses only editing — a press
    /// focuses it, a drag selects, copy and select-all work, and cut and paste
    /// are answered with nothing. A focused read-only field asks the shell for
    /// the platform input surface (its clipboard route) with the on-screen
    /// keyboard suppressed. See the [module docs](self)' "Read-only mode"
    /// section for the full contract, the semantics distinction from disabled,
    /// and the interaction with `obscured`/the chrome geometry setters.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Mask the rendered text with U+2022 BULLET (password mode, default
    /// `false`).
    ///
    /// The underlying value is untouched — `on_change` and the published
    /// `ImeState` still carry the real text, and caret/selection/editing
    /// arithmetic is unchanged, including across multi-byte graphemes. The
    /// field reports itself to accessibility as
    /// [`Role::PasswordInput`]. See the [module docs](self) for what obscuring
    /// deliberately does *not* do (no platform password keyboard, no autofill).
    pub fn obscured(mut self, obscured: bool) -> Self {
        self.obscured = obscured;
        self
    }

    /// Override the chrome's inner padding (chrome edge to text), in logical
    /// px. Defaults to [`PAD_X`]/[`PAD_Y`] (8×6) — a field that never calls
    /// this renders identically to before this setter existed. Independent of
    /// [`text_style`](Self::text_style)'s glyph metrics; see the module docs'
    /// "Chrome" section for why this is a per-instance value rather than a
    /// `Theme` token.
    pub fn padding(mut self, x: f64, y: f64) -> Self {
        self.pad_x = x;
        self.pad_y = y;
        self
    }

    /// Override the chrome's border thickness, in logical px. Defaults to
    /// [`BORDER_W`] (1.5). Affects only the idle/unfocused border unless
    /// [`focus_ring_width`](Self::focus_ring_width) is left unset, in which
    /// case the focused border uses this width too (unchanged relative
    /// behavior). See the module docs' "Chrome" section.
    pub fn border_width(mut self, width: f64) -> Self {
        self.border_width = width;
        self
    }

    /// Override the chrome's corner radius, in logical px. Defaults to
    /// [`RADIUS`] (6.0). See the module docs' "Chrome" section.
    pub fn corner_radius(mut self, radius: f64) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Override the caret width, in logical px. Defaults to [`CARET_W`]
    /// (1.5). See the module docs' "Chrome" section.
    pub fn caret_width(mut self, width: f32) -> Self {
        self.caret_width = width;
        self
    }

    /// Use `width` as the border thickness only while the field is focused,
    /// instead of [`border_width`](Self::border_width). Unset by default, so
    /// a focused field's border is the same width as its idle border,
    /// matching the current behavior exactly. This is the narrow focus-ring
    /// escape hatch described in the module docs' "Chrome" section — combined
    /// with a theme's accent color, it reaches a thicker/differently-colored
    /// focus outline without this widget growing a second rendering
    /// primitive (a halo) it does not otherwise have.
    pub fn focus_ring_width(mut self, width: f64) -> Self {
        self.focus_ring_width = Some(width);
        self
    }
}

/// Stable accesskit custom-action ids for the four clipboard verbs the field
/// publishes on its own semantics node (see [`TextInputWidget::semantics`]).
///
/// **Stable is the whole point.** An assistive-technology client holds on to
/// the id it was offered and sends that number back when the user picks the
/// action, so renumbering these would silently re-point a remembered "Copy" at
/// some other verb. They are ordinary small integers because that is what
/// [`accesskit::ActionData::CustomAction`] carries.
const A11Y_CUT_ID: i32 = 1;
/// See [`A11Y_CUT_ID`].
const A11Y_COPY_ID: i32 = 2;
/// See [`A11Y_CUT_ID`].
const A11Y_PASTE_ID: i32 = 3;
/// See [`A11Y_CUT_ID`].
const A11Y_SELECT_ALL_ID: i32 = 4;

/// The live long-press timer for one press — the field's own copy of
/// [`crate::gesture`]'s paint-clock recogniser, in the one shape a text field
/// needs (see the module docs' "Selection gestures and the toolbar").
///
/// `Copy` so an event arm can read it by value rather than holding a borrow of
/// the widget it is about to reassign, exactly as `gesture.rs`'s own recogniser
/// state does.
#[derive(Clone, Copy)]
struct HoldState {
    /// The frame time the press was first painted at, seeded by that paint
    /// because the event pass that armed the hold carries no clock. `None`
    /// until the press has been painted once.
    started_at: Option<FrameTime>,
    /// Set by the paint that observes `frame_time - started_at` reaching
    /// [`LONG_PRESS_MS`]; the word is selected on the next pass that carries an
    /// [`EventCtx`].
    elapsed: bool,
}

/// Which phase the live primary press is in — the gesture state machine's own
/// word for what the next `Move` is allowed to do.
///
/// This is an explicit phase because "where the press landed" and "is this
/// press still a tap" are two different questions, and one `Option<Point>`
/// used to answer both: taking the point to say *the hold already fired* also
/// said *this press became a drag*, which disarmed the slop guard for the rest
/// of the gesture. [`Gesture::HoldFired`] carries the press point precisely so
/// that guard outlives the fire.
///
/// `Copy` for [`HoldState`]'s reason: an event arm reads the phase by value
/// rather than holding a borrow of the widget it is about to reassign.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Gesture {
    /// No live primary press.
    None,
    /// A press is down and has stayed within [`TOUCH_SLOP`] of `at`: still a
    /// *tap*, and a tap must not drag the selection around. A release while
    /// the press is in this phase is what seeds the double-tap window and what
    /// resolves the tap-in-selection toolbar toggle.
    Tap {
        /// Widget-local position the press landed at.
        at: Point,
    },
    /// The press wandered past [`TOUCH_SLOP`]: every later `Move` extends the
    /// selection to the pointer. *How far* each move extends is the editor's
    /// call rather than this phase's — see the module docs' "Selection
    /// gestures and the toolbar" on the retained selection granularity.
    Drag,
    /// The long-press fired and the finger is **still down**. The gesture has
    /// resolved: it is no longer a tap (it seeds no double-tap and toggles no
    /// toolbar on release), but it is not a drag either — a finger holding
    /// still within [`TOUCH_SLOP`] of `at` must leave the word it just
    /// selected exactly as it is, however much it jitters.
    HoldFired {
        /// Widget-local position the press landed at — the point the word was
        /// selected from, and what the surviving slop guard measures against.
        at: Point,
    },
}

impl Gesture {
    /// Where the press landed while it is still a *tap*, and `None` in every
    /// other phase.
    ///
    /// The double-tap window and the tap-in-selection toggle both key off this
    /// rather than off a bare stored point: a press that wandered, and one a
    /// long-press already resolved, are not taps and must seed neither.
    fn tap_point(self) -> Option<Point> {
        match self {
            Gesture::Tap { at } => Some(at),
            Gesture::None | Gesture::Drag | Gesture::HoldFired { .. } => None,
        }
    }
}

/// The retained widget for a [`TextInputView`].
pub struct TextInputWidget {
    /// The editing engine, always holding the **real** (never masked) text.
    /// Driven through the widget-owned `text_ctx`.
    editor: TextEditor,
    /// The masked mirror of `editor`, present only while obscured. A pure
    /// function of `editor`'s editing state (re-derived by
    /// [`sync_mask`](Self::sync_mask) after every edit, so it cannot drift),
    /// and the editor every *display* read goes through — see
    /// [`display`](Self::display) and the [module docs](self).
    mask_editor: Option<TextEditor>,
    /// The widget's own font/layout context — see the [module docs](self).
    text_ctx: TextContext,
    /// The declared (builder) style — family/weight/style/size/letter-spacing/
    /// line-height, plus the app's own color when [`text_style_explicit`] is
    /// `true`. Layout metrics (`content_height`/`text_top`/`line_height`) read
    /// this directly; the *color* actually installed on `editor` may differ —
    /// see [`applied_style`](Self::applied_style).
    style: TextStyle,
    /// Whether the app set the content style explicitly — see [`TextInputView`].
    text_style_explicit: bool,
    /// The style last installed on `editor` (the resolved effective style —
    /// see [`effective_style`](Self::effective_style)). Compared against the
    /// freshly-resolved style at each `layout` to detect a theme swap or a
    /// rebuilt `style`/`text_style_explicit`, in which case [`apply_style`]
    /// rebuilds `editor` with the new style.
    ///
    /// [`apply_style`]: Self::apply_style
    applied_style: TextStyle,
    placeholder: String,
    /// `None` = single-line; `Some(n)` = wrapped multi-line capped at `n`
    /// visible lines. Drives the wrap-width feed in `layout`, the height cap,
    /// and the keep-caret-in-view scroll offset (see the [module docs](self)).
    max_visible_lines: Option<usize>,
    /// The soft-wrap width last installed on `editor` (`None` sentinel = not
    /// yet applied / editor just rebuilt). `layout` only calls
    /// [`TextEditor::set_wrap_width`] when the desired width differs from this —
    /// mirroring `Text`'s cached-shape reuse. parley's
    /// `PlainEditor::set_width` unconditionally marks its layout dirty and the
    /// next `refresh_layout` re-shapes, so an unconditional per-pass call
    /// re-shaped every frame; guarding it here reshapes only on an actual
    /// width/mode change (edits still reshape via their own `apply`).
    applied_wrap_width: Option<Option<f32>>,
    /// Resolved Enter behavior: `true` submits, `false` inserts a newline.
    /// Defaults to true single-line / false multi-line; Shift+Enter inverts it
    /// (multi-line only — a single-line field always submits).
    submit_on_enter: bool,
    /// Whether the field accepts input (see [`TextInputView::enabled`]). It
    /// alone is [`focusable`](Self::focusable) (the gate for the whole `event()`
    /// pass) and it alone drives both theme-resolution points' dimming — see
    /// [`Chrome::resolve`]/[`effective_style`](Self::effective_style).
    enabled: bool,
    /// Whether the field is read-only (see [`TextInputView::read_only`]).
    /// Withholds [`editable`](Self::editable) alongside `enabled` — but never
    /// focus, and never dimming, which is the whole point of the flag (see the
    /// [module docs](self)' "Read-only mode" section).
    read_only: bool,
    /// Whether the rendered glyphs are masked (see [`TextInputView::obscured`]).
    /// Kept alongside `mask_editor` (which it decides) so `rebuild` can compare
    /// it and `semantics` can pick its role without inspecting the mirror.
    obscured: bool,
    /// Set by a `rebuild` that disabled a field holding focus: the focus path
    /// lives on the widget's *pod*, which `rebuild` cannot reach (`BuildCtx`
    /// carries no focus seam), so the release is deferred to the first `event()`
    /// that arrives — meanwhile `paint` already refuses to behave as focused (no
    /// caret, no blink frame, an inactive IME surface). Read-only does not set
    /// it: that field keeps its session (see the [module docs](self)' "Read-only
    /// mode" section).
    release_focus_pending: bool,
    /// The widget's event-pass view of its focus: set on a `Down` inside,
    /// cleared on Escape / a blur `Down` this widget observes. NOT authoritative
    /// for painting — `paint` reads `PaintCtx::has_focus()` (the pod-recorded
    /// focus path, ancestor-composed) and self-corrects this flag when a
    /// container-routed blur never reached `event()` (one-paint convergence).
    focused: bool,
    /// Armed by a `Down` inside (alongside `capture_pointer`) to drive
    /// drag-selection; cleared on `Up`/`Cancel`.
    captured: bool,
    /// The frame time the caret was last reset to visible (the blink phase's
    /// epoch). Recorded from the paint-pass [`PaintCtx::frame_time`], since the
    /// event pass carries no clock — see `blink_reset_pending`.
    blink_epoch: FrameTime,
    /// Set when an edit/focus during the (clockless) event pass requests a blink
    /// reset; the next paint records `blink_epoch` from `frame_time` and clears
    /// this. `true` initially so the first painted frame seeds the epoch.
    blink_reset_pending: bool,
    /// Resolved horizontal inner padding — see [`TextInputView::padding`].
    /// Read from both the event pass (`editor_point`/`current_ime_state`) and
    /// layout/paint (see [`PAD_X`]).
    pad_x: f64,
    /// Resolved vertical inner padding — see [`TextInputView::padding`] and
    /// [`PAD_Y`].
    pad_y: f64,
    /// Resolved border thickness — see [`TextInputView::border_width`] and
    /// [`BORDER_W`]. The idle border width; `paint` widens it to
    /// `focus_ring_width` instead while focused, when set.
    border_width: f64,
    /// Resolved corner radius — see [`TextInputView::corner_radius`] and
    /// [`RADIUS`].
    corner_radius: f64,
    /// Resolved caret width — see [`TextInputView::caret_width`] and
    /// [`CARET_W`].
    caret_width: f32,
    /// Focused-only border width override — see
    /// [`TextInputView::focus_ring_width`]. `None` = `paint` uses
    /// `border_width` while focused too (unchanged appearance).
    focus_ring_width: Option<f64>,
    /// The portal slot the selection toolbar floats through — see the [module
    /// docs](self)' "Selection gestures and the toolbar". `()`-stated: the pod
    /// is built by a process-global builder that knows nothing about this
    /// field's application state, and speaks back through the edit-command
    /// queue rather than a callback.
    toolbar: OverlaySlot<()>,
    /// The view currently mounted in `toolbar`, kept so the next `rebuild` has
    /// something to reconcile (or tear down) against — the builder hands back a
    /// fresh view each call, so there is nothing else to diff with.
    toolbar_view: Option<AnyView<()>>,
    /// The action set `toolbar_view` was built for, and the whole rebuild
    /// guard: an anchor that moved or a selection that grew within the same
    /// verbs re-places the pod without rebuilding it. `None` = nothing is
    /// wanted (which is also the state a wanted-but-unbuildable toolbar records,
    /// so a missing builder is complained about once per state change rather
    /// than once per frame — see [`TextInputWidget::sync_toolbar`]).
    toolbar_view_actions: Option<SelectionToolbarActions>,
    /// Whether the field currently wants its selection toolbar shown. The
    /// gestures raise it, the hide rules drop it, and `rebuild` turns it into a
    /// mounted pod (see the [module docs](self)).
    toolbar_open: bool,
    /// The selection's bounding box in **absolute window space** as of the last
    /// paint — what was published, and what the request handed to the builder
    /// on the next `rebuild` carries. A rebuild runs before the paint that
    /// would refresh it, so this is deliberately one frame behind; the pod's
    /// actual placement is recomputed from the live anchor every paint.
    toolbar_anchor: Rect,
    /// The window size the last `layout` saw — the second half of the builder's
    /// argument pair (a toolbar may size or clamp itself against the window it
    /// floats in), recorded here because `View::rebuild` sees no `LayoutCtx`.
    window_size: Size,
    /// Which phase the live primary press is in — see [`Gesture`]. The slop
    /// guard, the double-tap window and the tap-in-selection toggle all read
    /// it, and it is what keeps a fired long-press distinguishable from a
    /// press that wandered into a drag.
    gesture: Gesture,
    /// The live long-press timer, armed by a primary `Down` inside and cleared
    /// when it fires, when the press drags past the slop, or when it ends.
    hold: Option<HoldState>,
    /// Set when the live press landed inside an existing non-collapsed
    /// selection, carrying whether the toolbar was open at that moment — the
    /// toggle's memory, since the press itself already applied the hide rule.
    /// The release opens the toolbar iff it was closed.
    tap_in_selection: Option<bool>,
    /// Set when the light-dismiss notification closed the toolbar for a press
    /// that is *about to* arrive here as well.
    ///
    /// A press outside every floated surface reaches this field **twice**: the
    /// root delivers [`OutsideTap::Notify`] first, as an overlay broadcast, and
    /// the press itself second (the registration does not consume it). By the
    /// time the `Down` arm runs, `toolbar_open` has therefore already been
    /// cleared — so the tap-in-selection toggle reads this instead, and a
    /// second tap on a selection closes the bar rather than re-opening it.
    /// Taken by that arm, and cleared by any blur, so it cannot go stale across
    /// a press that landed on a sibling and never reached this field at all.
    outside_press_dismissed: bool,
    /// The last completed in-slop tap: where it was, and the frame time of the
    /// last paint before it (the event pass has no clock of its own). A press
    /// within [`DOUBLE_TAP_MS`] and [`TOUCH_SLOP`] of it is a double-tap.
    last_tap: Option<(Point, FrameTime)>,
    /// The frame time of the most recent paint — the event pass's only clock,
    /// and what both halves of `last_tap` are measured with.
    last_frame_time: FrameTime,
    on_change: crate::authoring::ErasedArgCallback<String>,
    on_submit: Option<crate::authoring::ErasedArgCallback<String>>,
}

/// A closed portal slot configured the way a selection toolbar wants it.
///
/// `Floating` (a toolbar is not a tooltip: it is the thing the user aims at),
/// `Interactive` (its whole purpose is being tapped), and
/// [`OutsideTap::Notify`] **without** consuming — a press elsewhere dismisses
/// the bar *and* still lands, so the tap that puts it away also moves the caret
/// where the user pointed. Placement is above the selection, centred, at
/// [`TOOLBAR_GAP`], flipping below and clamping inside the window when there is
/// no room (the [`OverlayPlacement`] defaults for both).
fn new_toolbar_slot() -> OverlaySlot<()> {
    let mut slot = OverlaySlot::new();
    slot.set_band(OverlayBand::Floating);
    slot.set_input(OverlayInput::Interactive);
    slot.set_outside_tap(OutsideTap::Notify { consume: false });
    slot.set_placement(
        OverlayPlacement::on(OverlaySide::Top)
            .align(OverlayAlign::Center)
            .offset(TOOLBAR_GAP),
    );
    slot
}

/// Whether `pos` (widget-local) lies within a `size`-sized field.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// Which named keys a focusable-but-not-editable field still answers.
///
/// The clipboard verbs, and Escape — the keyboard's way out of a session such a
/// field can now hold. Everything else either changes the text or moves the
/// caret through `apply_edit`, and stays refused exactly as it was while a
/// read-only field refused focus outright. Cut and paste are answered *here* and
/// refused further down (`handle_command`, `request_paste_if_editable`), so the
/// verb is understood and answered with nothing rather than ignored.
///
/// Matched exhaustively (no `_` arm) on purpose: a named key added to
/// [`NamedKey`] is a compile error here until it is classified.
fn answered_while_read_only(named: NamedKey, modifiers: frust_core::Modifiers) -> bool {
    match named {
        NamedKey::Copy | NamedKey::Cut | NamedKey::Paste | NamedKey::Insert | NamedKey::Escape => {
            true
        }
        // Shift+Delete is the legacy cut chord; a plain (or otherwise chorded)
        // Delete is a forward delete, which is an edit.
        NamedKey::Delete => modifiers.shift && !modifiers.ctrl && !modifiers.meta,
        NamedKey::Enter
        | NamedKey::Backspace
        | NamedKey::ArrowLeft
        | NamedKey::ArrowRight
        | NamedKey::ArrowUp
        | NamedKey::ArrowDown
        | NamedKey::Home
        | NamedKey::End
        | NamedKey::Tab => false,
    }
}

impl TextInputWidget {
    /// Whether the field can take focus — `enabled` alone, so a **read-only**
    /// field passes.
    ///
    /// [`Widget::event`]'s top gate and `paint`'s `focused` computation read
    /// this: a read-only field is focusable so its content can be selected and
    /// copied (what Material 3 and Apple's HIG both keep), and the clipboard
    /// verbs reach it the way every focus-routed event does — along the focus
    /// path it is now allowed to hold. What read-only withholds is
    /// [`editable`](Self::editable), not this.
    ///
    /// Dimming deliberately uses neither hook — see [`Chrome::resolve`] and
    /// [`effective_style`](Self::effective_style), which key off `enabled`
    /// alone (the [module docs](self)' "Read-only mode" section).
    fn focusable(&self) -> bool {
        self.enabled
    }

    /// Whether the field's text may actually change — `false` if disabled *or*
    /// read-only.
    ///
    /// Every mutating path keys off this rather than off focus: typing, IME
    /// composition and commits, the editing and caret-motion named keys, and
    /// the `Cut`/`Paste` halves of [`handle_command`](Self::handle_command)
    /// (answered, with nothing, rather than left for someone else). So does
    /// [`ImeState::suppress_soft_keyboard`], which is how a focused read-only
    /// field keeps the platform surface its copy route needs without the
    /// on-screen keyboard it has no use for.
    fn editable(&self) -> bool {
        self.enabled && !self.read_only
    }

    /// The style to shape the editor with: `style` unchanged when the color was
    /// set explicitly (or no theme is active), otherwise `style` with its color
    /// replaced by the theme's `on_surface` role. Mirrors
    /// [`crate::TextWidget`]'s `effective_style` — resolving the color here at
    /// LAYOUT time (where `TextInput`, like `Text`, bakes the glyph brush into
    /// the editor's shaped state) keeps the unthemed path pixel-identical to
    /// before this retrofit.
    ///
    /// While disabled, whatever color that resolution produced is dimmed by
    /// [`DISABLED_CONTENT_ALPHA`] — the layout half of the two-point dimming
    /// (the other is [`Chrome::resolve`]), and the reason a change to `enabled`
    /// must report `ChangeFlags::LAYOUT`.
    fn effective_style(&self, theme: Option<&Theme>) -> TextStyle {
        let mut style = if self.text_style_explicit {
            self.style.clone()
        } else {
            match theme {
                Some(theme) => {
                    let mut style = self.style.clone();
                    style.color = theme.scheme().on_surface;
                    style
                }
                None => self.style.clone(),
            }
        };
        if !self.enabled {
            style.color = style.color.multiply_alpha(DISABLED_CONTENT_ALPHA);
        }
        style
    }

    /// The editor every *display* read goes through: the masked mirror while
    /// obscured, the real editor otherwise. Layout metrics, glyph runs,
    /// selection rects, the caret rect and the pointer hit test all key off
    /// this, so masking changes what is measured and not just what is drawn
    /// (see the [module docs](self)).
    fn display(&self) -> &TextEditor {
        self.mask_editor.as_ref().unwrap_or(&self.editor)
    }

    /// (Re)create the masked mirror to match `obscured`, then seed it from the
    /// current editing state. Resets `applied_wrap_width` so the next `layout`
    /// re-installs the soft-wrap width on both editors (a freshly built
    /// [`TextEditor`] carries none, and that install is also what refreshes the
    /// new mirror's layout).
    fn rebuild_mask_editor(&mut self) {
        let style = self.applied_style.clone();
        self.mask_editor = self.obscured.then(|| TextEditor::new(&style));
        self.applied_wrap_width = None;
        self.sync_mask();
    }

    /// Re-derive the masked mirror from the real editing state. A no-op when
    /// not obscured, and (via [`EditOp::ApplyEditingState`]'s value-equality
    /// short-circuit) when nothing changed. Because the mirror is *derived*
    /// rather than edited in parallel, it can never drift from the real buffer.
    fn sync_mask(&mut self) {
        let Some(mask) = self.mask_editor.as_mut() else {
            return;
        };
        let real = self.editor.editing_state_bytes();
        let state = EditingStateBytes {
            text: mask_text(&real.text),
            base: real_to_masked(&real.text, real.base),
            extent: real_to_masked(&real.text, real.extent),
            composing: real
                .composing
                .map(|r| real_to_masked(&real.text, r.start)..real_to_masked(&real.text, r.end)),
        };
        mask.apply(EditOp::ApplyEditingState(state), &mut self.text_ctx);
    }

    /// Rebuild `editor` with `style`, preserving the current editing state
    /// (text/selection/composing) across the reconstruction — `TextEditor`
    /// exposes no post-construction style setter, so a style change (an app
    /// rebuild with a different `.text_style(...)`, or a theme swap re-resolving
    /// the themed color at the next `layout`) reconstructs the editor rather
    /// than mutating it in place.
    ///
    /// A desktop-path IME preedit (parley's own `raw_compose`) does not survive
    /// a mid-composition style swap — `ApplyEditingState` re-seeds it as a
    /// platform-tracked composing region instead (still reported correctly by
    /// `editing_state_utf16`/`editing_state_bytes`), a narrow, acceptable edge
    /// case since changing `text_style` mid-keystroke-composition is not a
    /// realistic app pattern.
    fn apply_style(&mut self, style: TextStyle) {
        let state = self.editor.editing_state_bytes();
        self.editor = TextEditor::new(&style);
        self.editor
            .apply(EditOp::ApplyEditingState(state), &mut self.text_ctx);
        self.applied_style = style;
        // The freshly built editor carries no wrap width (`TextEditor::new`
        // resets it to single-line); force `layout` to re-install the desired
        // width on the next pass. (`rebuild_mask_editor` re-asserts this too —
        // the masked mirror must be rebuilt with the same new style.)
        self.applied_wrap_width = None;
        self.rebuild_mask_editor();
    }

    /// The single-line text height from the editor's refreshed metrics, floored
    /// to a sensible line height for an empty field.
    fn content_height(&self) -> f64 {
        self.display()
            .layout_size()
            .height
            .max(self.style.size as f64 * 1.25)
    }

    /// The top-left of the text content within a `height`-tall field (vertically
    /// centered, never above the top padding). Single-line placement.
    fn text_top(&self, height: f64) -> f64 {
        ((height - self.content_height()) / 2.0).max(self.pad_y)
    }

    /// Height of one text line from the editor's own metrics, falling back to a
    /// sensible line height before the first layout / when the field is empty.
    fn line_height(&self) -> f64 {
        let h = self.display().layout_size().height;
        let n = self.display().line_count();
        if h > 0.0 && n > 0 {
            h / n as f64
        } else {
            self.style.size as f64 * 1.25
        }
    }

    /// The vertical scroll offset (content shifted up, in logical px) that keeps
    /// the caret's line in view once the content outgrows the visible box. Zero
    /// in single-line mode or while the content fits. Recomputed statelessly
    /// each pass from the caret rect — a v1 keep-caret-in-view stand-in for a
    /// full scroll composition (see the [module docs](self)).
    fn scroll_y(&self, field_height: f64) -> f64 {
        if self.max_visible_lines.is_none() {
            return 0.0;
        }
        let visible = (field_height - 2.0 * self.pad_y).max(0.0);
        let content = self.display().layout_size().height;
        if content <= visible {
            return 0.0;
        }
        let max_off = content - visible;
        let (y0, y1) = match self.display().cursor_rect(self.caret_width) {
            Some(c) => (c.y0, c.y1),
            None => (0.0, 0.0),
        };
        // Reveal the caret's bottom edge, clamp to the scrollable range, then
        // pull back up if that hid the caret's top edge (caret taller motion).
        let mut off = if y1 > visible { y1 - visible } else { 0.0 };
        off = off.clamp(0.0, max_off);
        if y0 < off {
            off = y0.clamp(0.0, max_off);
        }
        off
    }

    /// The y of the text content's top within a `height`-tall field: the
    /// single-line centered placement, or the multi-line top-padded placement
    /// shifted up by the keep-caret-in-view scroll offset.
    fn content_origin_y(&self, height: f64) -> f64 {
        if self.max_visible_lines.is_some() {
            self.pad_y - self.scroll_y(height)
        } else {
            self.text_top(height)
        }
    }

    /// Reset the blink so the caret is visible from the next painted frame
    /// (called on any edit / focus, during the clockless event pass). The actual
    /// epoch is recorded from `frame_time` on the next paint.
    fn reset_blink(&mut self) {
        self.blink_reset_pending = true;
    }

    /// Whether the caret is in its visible half-cycle at frame time `now`, phase
    /// measured from `blink_epoch`.
    fn caret_visible_at(&self, now: FrameTime) -> bool {
        let elapsed_ms = now.saturating_sub(self.blink_epoch).as_secs_f64() * 1000.0;
        ((elapsed_ms / BLINK_MS) as u64).is_multiple_of(2)
    }

    /// Replace the whole editing value (controlled reconcile / initial seed),
    /// placing the caret at the end. No callback fires.
    fn set_controlled_value(&mut self, value: &str) {
        let op = EditOp::ApplyEditingState(EditingStateBytes {
            text: value.to_string(),
            base: value.len(),
            extent: value.len(),
            composing: None,
        });
        self.editor.apply(op, &mut self.text_ctx);
        self.sync_mask();
    }

    /// Apply one editing op, then run the after-edit bookkeeping: reset the
    /// blink, fire `on_change` if the text actually changed, and republish the
    /// IME surface. Used for both text edits and selection-only moves.
    fn apply_edit(&mut self, ctx: &mut EventCtx, op: EditOp) {
        let before = self.editor.text().to_string();
        self.editor.apply(op, &mut self.text_ctx);
        self.finish_edit(ctx, before);
    }

    /// Shared after-edit bookkeeping (see [`apply_edit`](Self::apply_edit)),
    /// factored out so a multi-op edit (an IME commit) reports once.
    fn finish_edit(&mut self, ctx: &mut EventCtx, before: String) {
        // Re-derive the masked mirror first: the IME surface published below
        // (and any metric read this pass) takes its caret rect from it.
        self.sync_mask();
        self.reset_blink();
        let after = self.editor.text().to_string();
        if after != before {
            (self.on_change)(ctx, after);
            // Hide rule: a toolbar offers verbs for a *selection*, and an edit
            // is what makes that selection stale — it either replaced the run
            // the bar was pointing at or moved it. Selection-only edits (a
            // caret move, a drag, a word select) leave it alone, which is what
            // lets a long-press select and open in the same pass.
            self.hide_toolbar(ctx);
        }
        self.publish(ctx);
        ctx.request_redraw();
    }

    /// Build the current editing state + caret (window coordinates) for the
    /// widget laid out at `origin`/`size`, so the shell can drive the platform
    /// IME. Shared by the event-pass [`publish`](Self::publish) and the
    /// paint-pass republish (see [`Widget::paint`]).
    fn current_ime_state(&self, origin: Point, size: Size) -> ImeState {
        let es = self.editor.editing_state_utf16();
        let editing = EditingState {
            text: es.text,
            selection_base: es.selection_base,
            selection_extent: es.selection_extent,
            composing_base: es.composing_base,
            composing_extent: es.composing_extent,
        };
        let offset = origin.to_vec2() + Vec2::new(self.pad_x, self.content_origin_y(size.height));
        // The caret rect is a *screen* placement hint, so it comes from the
        // displayed (possibly masked) layout — while `editing` above stays the
        // real text the platform IME mirror needs.
        let caret = self.display().cursor_rect(self.caret_width).map(|c| {
            Rect::new(
                c.x0 + offset.x,
                c.y0 + offset.y,
                c.x1 + offset.x,
                c.y1 + offset.y,
            )
        });
        ImeState {
            active: true,
            editing,
            caret,
            // `obscured` is the field's *only* content-type signal for now (no
            // builder exposes an override — see the module docs' rationale).
            // Computed live from `self.obscured` on every call, so there is no
            // window where a masked field publishes `Normal`: the very first
            // `ImeState` a newly focused obscured field emits already carries
            // `Password`, which is what actually closes the suggestion-strip
            // leak (a field that starts `Normal` and flips a frame later has
            // already leaked to the IME).
            content_type: if self.obscured {
                ImeContentType::Password
            } else {
                ImeContentType::Normal
            },
            // A read-only field publishes an *active* surface like any other —
            // the shells' clipboard routes (the web overlay's DOM `copy`
            // listener, Android's `InputConnection`, iOS's first responder) all
            // hang off it — and asks only that the on-screen keyboard stay down,
            // having nothing to type into.
            suppress_soft_keyboard: !self.editable(),
        }
    }

    /// Publish the current editing state + caret during the event pass so the
    /// shell can drive the platform IME.
    fn publish(&self, ctx: &mut EventCtx) {
        ctx.publish_ime_state(self.current_ime_state(ctx.origin(), ctx.size()));
    }

    /// Translate a widget-local pointer position into the editor's layout-local
    /// coordinate space (used for caret placement / drag-selection).
    fn editor_point(&self, pos: Point, height: f64) -> (f32, f32) {
        (
            (pos.x - self.pad_x) as f32,
            (pos.y - self.content_origin_y(height)) as f32,
        )
    }

    /// Apply a **pointer-resolved** edit op — one whose coordinates address the
    /// layout the user is looking at rather than the buffer.
    ///
    /// Unobscured this is just `op` on the real editor. Obscured, the point has
    /// to be resolved against the *masked* layout — the glyphs actually on
    /// screen, whose advances differ from the real text's — and the resulting
    /// offsets mapped back onto the real buffer, so a press lands on the same
    /// character it visually points at. Both pointer gestures that reach the
    /// editor ([`move_to_point`](Self::move_to_point) and
    /// [`select_word_at_point`](Self::select_word_at_point)) need exactly that
    /// translation, which is why it lives here once.
    fn apply_display_op(&mut self, ctx: &mut EventCtx, op: EditOp) {
        if self.mask_editor.is_none() {
            self.apply_edit(ctx, op);
            return;
        }
        let (masked_base, masked_extent) = {
            let mask = self
                .mask_editor
                .as_mut()
                .expect("obscured field has a mask editor");
            mask.apply(op, &mut self.text_ctx);
            let m = mask.editing_state_bytes();
            (m.base, m.extent)
        };
        let real = self.editor.editing_state_bytes();
        let before = real.text.clone();
        let state = EditingStateBytes {
            base: masked_to_real(&real.text, masked_base),
            extent: masked_to_real(&real.text, masked_extent),
            ..real
        };
        self.editor
            .apply(EditOp::ApplyEditingState(state), &mut self.text_ctx);
        self.finish_edit(ctx, before);
    }

    /// Place (or extend the selection to) the caret nearest a widget-local
    /// pointer position. Masking-aware — see
    /// [`apply_display_op`](Self::apply_display_op).
    fn move_to_point(&mut self, ctx: &mut EventCtx, x: f32, y: f32, select: bool) {
        self.apply_display_op(ctx, EditOp::MoveToPoint { x, y, select });
    }

    /// Select the whole word under a widget-local pointer position — what a
    /// long-press and a double-tap both resolve to. Masking-aware for
    /// [`move_to_point`](Self::move_to_point)'s reason: an obscured field's
    /// "word" is a run of bullets, and it is the mask's advances that decide
    /// which run the press is inside.
    fn select_word_at_point(&mut self, ctx: &mut EventCtx, x: f32, y: f32) {
        self.apply_display_op(ctx, EditOp::SelectWordAtPoint { x, y });
    }

    /// Put the toolbar away, repainting only if it was actually up — every hide
    /// rule in the module docs funnels through here.
    fn hide_toolbar(&mut self, ctx: &mut EventCtx) {
        if self.toolbar_open {
            self.toolbar_open = false;
            ctx.request_redraw();
        }
    }

    /// Forget the in-flight gesture: the hold timer, the press phase and the
    /// tap-in-selection candidate. Touches neither the selection nor the
    /// toolbar, which is what makes it the whole of a `Cancel`'s work.
    fn clear_gesture(&mut self) {
        self.hold = None;
        self.gesture = Gesture::None;
        self.tap_in_selection = None;
    }

    /// Whether a press at `pos` continues the last tap into a double-tap:
    /// within [`TOUCH_SLOP`] of it and within [`DOUBLE_TAP_MS`] of when it
    /// landed, both measured against the paint clock (`last_frame_time`),
    /// since the event pass carries none.
    fn is_double_tap(&self, pos: Point) -> bool {
        self.last_tap.is_some_and(|(prev, at)| {
            (pos - prev).hypot() <= TOUCH_SLOP
                && self.last_frame_time.saturating_sub(at).as_secs_f64() * 1000.0 <= DOUBLE_TAP_MS
        })
    }

    /// The offset from the field's own origin to the text content's — what a
    /// layout-local rect is translated by to become widget-local.
    fn content_offset(&self, height: f64) -> Vec2 {
        Vec2::new(self.pad_x, self.content_origin_y(height))
    }

    /// Whether a widget-local `pos` lands inside the current selection.
    ///
    /// Tested against the *displayed* selection rects (the masked mirror's,
    /// while obscured) for [`move_to_point`](Self::move_to_point)'s reason: the
    /// user is pointing at glyphs, not at byte offsets. Always false for a
    /// collapsed selection, which has no rects at all.
    fn point_in_selection(&self, pos: Point, height: f64) -> bool {
        let off = self.content_offset(height);
        self.display()
            .selection_rects()
            .iter()
            .any(|r| (*r + off).contains(pos))
    }

    /// Where the toolbar is anchored, in **widget-local** space: the bounding
    /// box of the displayed selection, or the caret rect while the selection is
    /// collapsed (a secondary press with no selection still needs somewhere to
    /// hang the bar).
    fn selection_anchor(&self, height: f64) -> Rect {
        let rects = self.display().selection_rects();
        let bounds = rects
            .into_iter()
            .reduce(|a, b| a.union(b))
            .or_else(|| self.display().cursor_rect(self.caret_width))
            .unwrap_or(Rect::ZERO);
        bounds + self.content_offset(height)
    }

    /// Which verbs the toolbar may offer for the current state.
    ///
    /// The field's own call, not the toolbar's (see
    /// [`SelectionToolbarActions`]): an obscured field hands out neither the
    /// real buffer nor its bullet mirror, so copy and cut are refused there
    /// exactly as [`clipboard_selection`](Self::clipboard_selection) refuses
    /// them; cut and paste additionally need an [`editable`](Self::editable)
    /// field, which is what leaves a read-only one offering copy and select-all
    /// alone; and select-all is pointless with no text or with all of it
    /// already selected.
    fn toolbar_actions(&self) -> SelectionToolbarActions {
        let text = self.editor.text();
        let selected = self.editor.selected_text();
        let has_selection = selected.is_some();
        // A selection is one contiguous slice of the buffer, so covering its
        // whole length is the same statement as covering all of it.
        let all_selected = selected.is_some_and(|s| s.len() == text.len());
        SelectionToolbarActions {
            copy: has_selection && !self.obscured,
            cut: has_selection && !self.obscured && self.editable(),
            paste: self.editable(),
            select_all: !text.is_empty() && !all_selected,
        }
    }

    /// Fire a long-press whose threshold a paint already observed, reporting
    /// whether it fired.
    ///
    /// Called from the first pass that carries an [`EventCtx`] — the
    /// [`InputEvent::Housekeeping`] broadcast the marking paint latched, or an
    /// in-slop `Move`/`Up` that arrived first, whichever wins the race (the
    /// other finds the hold already cleared and is a no-op).
    fn fire_hold(&mut self, ctx: &mut EventCtx) -> bool {
        if !self.hold.is_some_and(|hold| hold.elapsed) {
            return false;
        }
        self.hold = None;
        // The gesture resolved as a long-press: it is no longer a tap (so it
        // seeds no double-tap) and no longer a toggle candidate (so the release
        // does not close what this just opened).
        self.tap_in_selection = None;
        let Some(pos) = self.gesture.tap_point() else {
            return false;
        };
        // Resolved, *not* forgotten: the press point stays so the `Move` arm's
        // slop guard still has something to measure against while the finger
        // is down. Dropping it here is what used to let the very next in-slop
        // move re-resolve the selection from the pointer.
        self.gesture = Gesture::HoldFired { at: pos };
        let (x, y) = self.editor_point(pos, ctx.size().height);
        // Selects first, opens second: the selection is what the toolbar's own
        // verbs are computed from, and `finish_edit` inside here resets the
        // blink and republishes the IME surface as any other edit does.
        self.select_word_at_point(ctx, x, y);
        self.toolbar_open = true;
        ctx.request_redraw();
        true
    }

    /// Mount, reconcile or drop the toolbar pod — the `View::rebuild` half of
    /// hosting it (the only pass carrying a [`BuildCtx`]).
    ///
    /// Keyed on the **action set** alone: a moved anchor or a selection that
    /// grew within the same verbs re-places the existing pod rather than
    /// rebuilding it, so the toolbar does not flicker while a drag extends a
    /// selection. A wanted-but-unbuildable toolbar (nothing installed in the
    /// process-wide builder slot) records the want anyway, so the diagnostic
    /// below fires once per state change rather than once per frame.
    fn sync_toolbar(&mut self, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        // Under the Native policy the platform draws it, so the field floats
        // nothing and only keeps publishing the request from `paint`.
        // Wanted by any *focusable* field, read-only included: on a touch
        // device the bar is the only copy affordance there is, and
        // `toolbar_actions` is what withholds the verbs a read-only field must
        // not offer.
        let wanted = (self.toolbar_open
            && self.focused
            && self.focusable()
            && selection_toolbar_policy() == SelectionToolbarPolicy::Framework)
            .then(|| self.toolbar_actions());
        if wanted == self.toolbar_view_actions {
            return ChangeFlags::NONE;
        }
        let next = wanted.and_then(|actions| match selection_toolbar_builder() {
            Some(builder) => Some(builder(
                &SelectionToolbarRequest {
                    anchor: self.toolbar_anchor,
                    actions,
                    // Always true here: a builder is called only for a bar that
                    // is wanted, so the flag the platform route reads as an edge
                    // carries no information on this side of the seam.
                    present_menu: true,
                },
                self.window_size,
            )),
            None => {
                // Nothing to float: no design system (and no app) installed a
                // builder, so the framework route has no view to draw. Said
                // once, in a debug build only — it is a wiring gap worth
                // hearing about, not an error a release build can act on.
                #[cfg(debug_assertions)]
                eprintln!(
                    "frust-widgets: a text field wants a selection toolbar but no builder is \
                     installed; nothing will float (install one with \
                     frust_core::set_selection_toolbar_builder)"
                );
                None
            }
        });
        let prev = self.toolbar_view.take();
        let flags = self.toolbar.rebuild(prev.as_ref(), next.as_ref(), ctx);
        self.toolbar_view = next;
        self.toolbar_view_actions = wanted;
        flags
    }

    /// Handle a keyboard key event (already focus-gated by the caller).
    fn handle_key(
        &mut self,
        ctx: &mut EventCtx,
        key: &Key,
        modifiers: frust_core::Modifiers,
    ) -> EventResult {
        match key {
            Key::Character(s) => {
                if modifiers.ctrl || modifiers.meta {
                    // Chord decoding is **platform-uniform**: ctrl OR meta arms
                    // the clipboard verbs on every OS, so a Mac keyboard's Cmd+C
                    // works under Linux and a terminal habit's Ctrl+C works on
                    // macOS. Flutter is platform-strict instead (meta on
                    // macOS/iOS, ctrl elsewhere); a shell that wants exactly that
                    // decodes the chord itself and dispatches an
                    // [`EditCommand`] — the route the platform edit menus and the
                    // hardware clipboard keys already take, and the one that
                    // always wins, since this branch only sees what a shell chose
                    // to forward as a key. Alt is deliberately not a chord
                    // modifier: it composes characters.
                    if s.eq_ignore_ascii_case("c") {
                        return self.handle_command(ctx, &EditCommand::Copy);
                    }
                    if s.eq_ignore_ascii_case("x") {
                        return self.handle_command(ctx, &EditCommand::Cut);
                    }
                    if s.eq_ignore_ascii_case("v") {
                        self.request_paste_if_editable(ctx);
                        return EventResult::Handled;
                    }
                    if s.eq_ignore_ascii_case("a") {
                        return self.handle_command(ctx, &EditCommand::SelectAll);
                    }
                    // Every other chorded character (Cmd+Z, Ctrl+B, …) stays
                    // consumed rather than typed: a chord is never literal text,
                    // and swallowing it here keeps an unimplemented verb from
                    // inserting a stray letter.
                    return EventResult::Handled;
                }
                if !self.editable() {
                    // Focusable but not editable: a read-only field takes no
                    // text from the keyboard, and refuses it unconsumed exactly
                    // as it did when it refused focus outright.
                    return EventResult::Ignored;
                }
                self.apply_edit(ctx, EditOp::Insert(s.clone()));
                EventResult::Handled
            }
            Key::Named(named) => {
                if !self.editable() && !answered_while_read_only(*named, modifiers) {
                    return EventResult::Ignored;
                }
                let select = modifiers.shift;
                match named {
                    NamedKey::Backspace => self.apply_edit(ctx, EditOp::Backdelete),
                    NamedKey::Delete => {
                        // Shift+Delete is the legacy cut chord (Windows/Linux/
                        // X11), but only on its own: Ctrl+Delete and
                        // Ctrl+Shift+Delete are OS/browser-level gestures, never
                        // a field cut, so anything chorded past shift falls
                        // through to the plain forward-delete.
                        if modifiers.shift && !modifiers.ctrl && !modifiers.meta {
                            return self.handle_command(ctx, &EditCommand::Cut);
                        }
                        self.apply_edit(ctx, EditOp::Delete)
                    }
                    NamedKey::ArrowLeft => self.apply_edit(ctx, EditOp::MoveLeft { select }),
                    NamedKey::ArrowRight => self.apply_edit(ctx, EditOp::MoveRight { select }),
                    NamedKey::Home => self.apply_edit(ctx, EditOp::Home { select }),
                    NamedKey::End => self.apply_edit(ctx, EditOp::End { select }),
                    NamedKey::Enter => {
                        // Single-line always submits (Enter is never a newline).
                        // Multi-line: the resolved `submit_on_enter`, inverted by
                        // Shift, decides submit vs. insert-newline.
                        let submit = if self.max_visible_lines.is_some() {
                            self.submit_on_enter ^ modifiers.shift
                        } else {
                            true
                        };
                        if submit {
                            let text = self.editor.text().to_string();
                            if let Some(cb) = &mut self.on_submit {
                                cb(ctx, text);
                            }
                            ctx.request_redraw();
                        } else {
                            self.apply_edit(ctx, EditOp::InsertNewline);
                        }
                    }
                    NamedKey::Escape => {
                        ctx.release_focus();
                        self.focused = false;
                        // Escape dismisses the session, and the toolbar with it
                        // — it is the field's, not a surface of its own.
                        self.hide_toolbar(ctx);
                        ctx.request_redraw();
                    }
                    // Vertical motion drives the caret across lines in multi-line
                    // mode; in single-line mode there is only one line, so it (and
                    // Tab traversal) are no-ops.
                    NamedKey::ArrowUp => {
                        if self.max_visible_lines.is_some() {
                            self.apply_edit(ctx, EditOp::MoveUp { select });
                        } else {
                            return EventResult::Ignored;
                        }
                    }
                    NamedKey::ArrowDown => {
                        if self.max_visible_lines.is_some() {
                            self.apply_edit(ctx, EditOp::MoveDown { select });
                        } else {
                            return EventResult::Ignored;
                        }
                    }
                    NamedKey::Tab => {
                        return EventResult::Ignored;
                    }
                    // The dedicated hardware clipboard keys arrive already
                    // decoded, so they need no chord to read — they resolve to
                    // exactly the verbs the chords above do.
                    NamedKey::Copy => return self.handle_command(ctx, &EditCommand::Copy),
                    NamedKey::Cut => return self.handle_command(ctx, &EditCommand::Cut),
                    NamedKey::Paste => self.request_paste_if_editable(ctx),
                    NamedKey::Insert => {
                        // The legacy Insert chords: Shift+Insert pastes,
                        // Ctrl+Insert copies (shift wins when both are held).
                        // A bare Insert would toggle overtype, which this field
                        // does not implement, so it is left unconsumed rather
                        // than silently swallowed.
                        if modifiers.shift {
                            self.request_paste_if_editable(ctx);
                        } else if modifiers.ctrl {
                            return self.handle_command(ctx, &EditCommand::Copy);
                        } else {
                            return EventResult::Ignored;
                        }
                    }
                }
                EventResult::Handled
            }
        }
    }

    /// Ask the shell to read the host clipboard, but only for a field that
    /// could act on the answer.
    ///
    /// Only the shell can read it, so a paste is *asked for* here and arrives
    /// on a later pass as [`EditCommand::Paste`] (`EventCtx::request_paste`).
    /// The asking is not free — it reaches the host clipboard, and on iOS it is
    /// one of the gestures that can raise the system's paste prompt — so a
    /// read-only field, whose `EditCommand::Paste` would change nothing anyway,
    /// consumes its paste chord and asks for nothing.
    fn request_paste_if_editable(&self, ctx: &mut EventCtx) {
        if self.editable() {
            ctx.request_paste();
        }
    }

    /// The text a copy or a cut may hand the host clipboard: the current
    /// selection, or `None` when the selection is collapsed **or** the field is
    /// obscured.
    ///
    /// The obscured refusal reads neither editor: not the real one (the secret
    /// is not this widget's to hand out — the whole point of the mode) and not
    /// the masked mirror either, since a run of bullets is a worse answer than
    /// no answer at all — it looks like a successful copy and pastes garbage.
    fn clipboard_selection(&self) -> Option<String> {
        if self.obscured {
            return None;
        }
        self.editor.selected_text().map(str::to_owned)
    }

    /// Handle a decoded clipboard/selection command (already focus-gated by the
    /// caller) — see the [module docs](self)' "Clipboard and selection commands".
    ///
    /// Every arm funnels through one [`finish_edit`](Self::finish_edit), so the
    /// after-edit bookkeeping is a keystroke's: the blink resets, the masked
    /// mirror re-derives, the IME surface republishes, a redraw is requested, and
    /// `on_change` fires **exactly once and only if the text actually changed** —
    /// which is what keeps a copy (and a refused cut, and a paste emptied by
    /// sanitising) from reporting an edit that never happened. The controlled-
    /// value contract is untouched: like every other edit here, a command reports
    /// a *requested* value and the app's own `on_change` value still wins on the
    /// next `rebuild`.
    ///
    /// # Refusals
    ///
    /// Each one still returns [`EventResult::Handled`]: the command was
    /// understood and answered with "nothing", which is not the same as leaving
    /// it for someone else.
    ///
    /// * **Read-only** — cut and paste change nothing and write nothing, since
    ///   both would rewrite a buffer the field does not hand out for rewriting;
    ///   copy and select-all run exactly as on an editable field, which is the
    ///   whole point of keeping a read-only field focusable.
    /// * **Obscured** — copy and cut write nothing and change nothing
    ///   ([`clipboard_selection`](Self::clipboard_selection)). Paste is
    ///   unaffected: writing *into* a password field is ordinary.
    /// * **Collapsed selection** — copy and cut are no-ops. A cut in particular
    ///   must not fall back to deleting a grapheme the way its `Backdelete`
    ///   would if the selection were empty.
    /// * **Empty after sanitising** — a paste of nothing but newlines into a
    ///   single-line field inserts nothing rather than applying an empty edit.
    fn handle_command(&mut self, ctx: &mut EventCtx, cmd: &EditCommand) -> EventResult {
        // A disabled field holds no focus path for this command to route along
        // ([`Widget::event`]'s top gate releases it) — this pins that invariant
        // rather than re-testing it. A read-only field *does* hold one, which is
        // why the mutating arms below carry their own `editable` check.
        debug_assert!(self.focusable(), "an EditCommand reached a disabled field");
        let before = self.editor.text().to_string();
        match cmd {
            EditCommand::Copy => {
                if let Some(text) = self.clipboard_selection() {
                    ctx.write_clipboard(text);
                }
            }
            EditCommand::Cut => {
                // Understood and answered with nothing on a read-only field:
                // neither half of a cut (the clipboard write *or* the delete)
                // may happen, since handing out the text while failing to
                // remove it would be a copy wearing a cut's name.
                if self.editable()
                    && let Some(text) = self.clipboard_selection()
                {
                    ctx.write_clipboard(text);
                    // `Backdelete` over a non-collapsed selection removes the
                    // selection itself, so the write and the delete describe the
                    // same run of text.
                    self.editor.apply(EditOp::Backdelete, &mut self.text_ctx);
                }
            }
            EditCommand::Paste(text) => {
                // A single-line field denies newlines outright and a multi-line
                // one normalises CRLF/CR — `frust_text::sanitize_paste` owns both
                // rules, and `max_visible_lines` is what says which field this is.
                let text = sanitize_paste(text, self.max_visible_lines.is_none());
                if self.editable() && !text.is_empty() {
                    // `Insert` replaces the selection, exactly like typing does.
                    self.editor
                        .apply(EditOp::Insert(text.into_owned()), &mut self.text_ctx);
                }
            }
            EditCommand::SelectAll => {
                self.editor.apply(EditOp::SelectAll, &mut self.text_ctx);
            }
        }
        self.finish_edit(ctx, before);
        // A verb answered is a toolbar spent, whichever route delivered it (a
        // chord, a platform edit menu, or the floated toolbar's own item): the
        // bar offered these four and one of them has now been taken.
        self.hide_toolbar(ctx);
        EventResult::Handled
    }

    /// Handle an IME event (already focus-gated by the caller).
    ///
    /// # Empty text is a retraction, not a composition
    ///
    /// Every shell spells "drop the preedit, insert nothing" as a `Compose`
    /// (or, from a platform that reports a commit for it, a `Commit`) with
    /// empty text: a cancelled composition, a field blurred mid-composition,
    /// an input method that ended a session with nothing to show for it. The
    /// editor has a primitive for exactly that — [`EditOp::ClearCompose`] —
    /// and it is the one that must be used, because `EditOp::Compose` is a
    /// *set-the-marked-text* operation whose backing editor asserts the text
    /// is non-empty. Routing an empty retraction through it panics a debug
    /// build on every platform that produces one.
    fn handle_ime(&mut self, ctx: &mut EventCtx, event: &ImeEvent) -> EventResult {
        match event {
            // The caret a retraction carries is meaningless — there is no
            // marked text left to place it in — so it is deliberately unread.
            ImeEvent::Compose { text, .. } if text.is_empty() => {
                self.apply_edit(ctx, EditOp::ClearCompose);
                EventResult::Handled
            }
            ImeEvent::Compose { text, cursor } => {
                self.apply_edit(
                    ctx,
                    EditOp::Compose {
                        text: text.clone(),
                        cursor: *cursor,
                    },
                );
                EventResult::Handled
            }
            ImeEvent::Commit(s) => {
                // iOS Return contract: the Return key on a
                // UITextInput arrives as `insertText("\n")` → `Commit("\n")`. On a
                // single-line (or submit-on-enter) widget a lone newline commit means
                // SUBMIT, exactly like `NamedKey::Enter` — it must never insert a
                // literal '\n'. A newline-inserting multi-line field instead lands
                // the literal newline, matching its `NamedKey::Enter` behavior.
                if s == "\n" || s == "\r" || s == "\r\n" {
                    if self.max_visible_lines.is_some() && !self.submit_on_enter {
                        self.apply_edit(ctx, EditOp::InsertNewline);
                        return EventResult::Handled;
                    }
                    let text = self.editor.text().to_string();
                    if let Some(cb) = &mut self.on_submit {
                        cb(ctx, text);
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                // An empty commit inserts nothing, so it is the same retraction
                // an empty `Compose` is — and reaches the same assertion if it
                // goes through the compose machinery below.
                if s.is_empty() {
                    self.apply_edit(ctx, EditOp::ClearCompose);
                    return EventResult::Handled;
                }
                // Commit the given text via the compose machinery so it replaces
                // any active preedit and lands at the caret in one edit.
                let before = self.editor.text().to_string();
                self.editor.apply(
                    EditOp::Compose {
                        text: s.clone(),
                        cursor: None,
                    },
                    &mut self.text_ctx,
                );
                self.editor.apply(EditOp::FinishCompose, &mut self.text_ctx);
                self.finish_edit(ctx, before);
                EventResult::Handled
            }
            ImeEvent::ApplyEditingState(state) => {
                self.apply_edit(ctx, editing_state_to_op(state));
                EventResult::Handled
            }
            // Bracket a composition session: nothing to mutate here.
            ImeEvent::Enabled | ImeEvent::Disabled => EventResult::Handled,
        }
    }
}

/// Resolve the effective Enter-submits behavior: an explicit
/// [`TextInputView::submit_on_enter`] wins, otherwise the mode default (submit
/// single-line, insert-newline multi-line).
fn resolve_submit_on_enter(
    max_visible_lines: Option<usize>,
    submit_on_enter: Option<bool>,
) -> bool {
    submit_on_enter.unwrap_or(max_visible_lines.is_none())
}

/// Convert a shell-facing (UTF-16-indexed) [`EditingState`] into the byte-indexed
/// [`EditOp::ApplyEditingState`] the editor consumes.
fn editing_state_to_op(state: &EditingState) -> EditOp {
    let text = &state.text;
    let base = utf16_to_byte(text, state.selection_base.max(0) as usize);
    let extent = utf16_to_byte(text, state.selection_extent.max(0) as usize);
    let composing = if state.composing_base >= 0 && state.composing_extent >= 0 {
        Some(
            utf16_to_byte(text, state.composing_base as usize)
                ..utf16_to_byte(text, state.composing_extent as usize),
        )
    } else {
        None
    };
    EditOp::ApplyEditingState(EditingStateBytes {
        text: text.clone(),
        base,
        extent,
        composing,
    })
}

impl<State: 'static> View<State> for TextInputView<State> {
    type Element = TextInputWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TextInputWidget {
        let style = self.text_style.clone();
        let mut text_ctx = TextContext::new();
        let mut editor = TextEditor::new(&style);
        // Seed the initial controlled value (and refresh the layout metrics).
        editor.apply(
            EditOp::ApplyEditingState(EditingStateBytes {
                text: self.value.clone(),
                base: self.value.len(),
                extent: self.value.len(),
                composing: None,
            }),
            &mut text_ctx,
        );
        let mut widget = TextInputWidget {
            editor,
            mask_editor: None,
            text_ctx,
            style: style.clone(),
            text_style_explicit: self.text_style_explicit,
            applied_style: style,
            placeholder: self.placeholder.clone(),
            max_visible_lines: self.max_visible_lines,
            applied_wrap_width: None,
            submit_on_enter: resolve_submit_on_enter(self.max_visible_lines, self.submit_on_enter),
            enabled: self.enabled,
            read_only: self.read_only,
            obscured: self.obscured,
            release_focus_pending: false,
            focused: false,
            captured: false,
            blink_epoch: FrameTime::ZERO,
            blink_reset_pending: true,
            pad_x: self.pad_x,
            pad_y: self.pad_y,
            border_width: self.border_width,
            corner_radius: self.corner_radius,
            caret_width: self.caret_width,
            focus_ring_width: self.focus_ring_width,
            toolbar: new_toolbar_slot(),
            toolbar_view: None,
            toolbar_view_actions: None,
            toolbar_open: false,
            toolbar_anchor: Rect::ZERO,
            window_size: Size::ZERO,
            gesture: Gesture::None,
            hold: None,
            tap_in_selection: None,
            outside_press_dismissed: false,
            last_tap: None,
            last_frame_time: FrameTime::ZERO,
            on_change: crate::authoring::erase_callback_arg(&self.on_change),
            on_submit: self
                .on_submit
                .as_ref()
                .map(crate::authoring::erase_callback_arg::<State, String>),
        };
        // Seeds the masked mirror when built obscured (a no-op otherwise).
        widget.rebuild_mask_editor();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TextInputWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable; reinstall the erased adapters unconditionally.
        element.on_change = crate::authoring::erase_callback_arg(&self.on_change);
        element.on_submit = self
            .on_submit
            .as_ref()
            .map(crate::authoring::erase_callback_arg::<State, String>);

        let mut flags = ChangeFlags::NONE;
        if prev.placeholder != self.placeholder {
            element.placeholder = self.placeholder.clone();
            flags |= ChangeFlags::PAINT;
        }
        // Reconcile the multi-line configuration. A change to the visible-line
        // cap changes the height clamp (relayout), and either knob can change
        // the resolved Enter behavior.
        if prev.max_visible_lines != self.max_visible_lines {
            element.max_visible_lines = self.max_visible_lines;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.submit_on_enter =
            resolve_submit_on_enter(self.max_visible_lines, self.submit_on_enter);
        // Enabled reconcile. LAYOUT (not just PAINT) because the disabled dim
        // is baked into the glyph color at layout time — the same
        // `set_theme` -> `ChangeFlags::LAYOUT` contract `text_style` rides.
        // A field disabled *while focused* must not be stranded focused: clear
        // the widget's own view of it now and flag the pod-level release for
        // the first event that reaches us (`BuildCtx` has no focus seam).
        if prev.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                element.release_focus_pending = element.focused;
                element.focused = false;
                element.captured = false;
                // A field that stops being interactive mid-press keeps no
                // gesture in flight either: the hold timer is capture-gated, so
                // it could not fire anyway, and leaving it armed would hand the
                // next press a stale epoch.
                element.clear_gesture();
                element.toolbar_open = false;
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Read-only reconcile: unlike `enabled`, PAINT only — `read_only`
        // never dims (see the module docs' "Read-only mode" section), so no
        // glyph color is baked differently at layout time. And unlike a field
        // disabled while focused, a field made read-only while focused keeps
        // its session: it is still `focusable`, and that session is what its
        // content is selected and copied through. Nothing is unwound here —
        // the next paint publishes a keyboard-suppressed surface (so the soft
        // keyboard goes away) and `sync_toolbar` below recomputes the verbs,
        // both from the flag this line just moved across.
        if prev.read_only != self.read_only {
            element.read_only = self.read_only;
            flags |= ChangeFlags::PAINT;
        }
        // Obscured reconcile: the masked mirror is what gets *measured*, and a
        // mask glyph's advance differs from the character it replaces, so this
        // resizes the field as well as repainting it.
        if prev.obscured != self.obscured {
            element.obscured = self.obscured;
            element.rebuild_mask_editor();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Text style reconcile: a changed declared style or explicit-
        // flag invalidates the style actually installed on the editor. The
        // editor itself is only rebuilt in `layout` (`effective_style`/
        // `apply_style`), mirroring `Text::rebuild`'s cache-invalidate-now,
        // resolve-at-layout split — this is also what makes a bare theme swap
        // (no view change at all, so `rebuild` never runs) still pick up the
        // new color, since `layout` always re-resolves against `applied_style`.
        if prev.text_style != self.text_style
            || prev.text_style_explicit != self.text_style_explicit
        {
            element.style = self.text_style.clone();
            element.text_style_explicit = self.text_style_explicit;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Controlled reconcile: adopt the app-confirmed value only when it differs
        // from what the editor currently holds, so an accepted edit leaves the
        // selection untouched and a rejected/normalized one is pulled back in.
        if self.value != element.editor.text() {
            element.set_controlled_value(&self.value);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Chrome geometry reconcile (see the module docs' "Chrome" section).
        // Padding feeds the resolved field height and the multi-line wrap
        // width, so it needs a relayout; the rest (border width, corner
        // radius, caret width, the focus-ring override) are paint-only — none
        // of them change the `Size` `layout` returns.
        if prev.pad_x != self.pad_x || prev.pad_y != self.pad_y {
            element.pad_x = self.pad_x;
            element.pad_y = self.pad_y;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.border_width != self.border_width {
            element.border_width = self.border_width;
            flags |= ChangeFlags::PAINT;
        }
        if prev.corner_radius != self.corner_radius {
            element.corner_radius = self.corner_radius;
            flags |= ChangeFlags::PAINT;
        }
        if prev.caret_width != self.caret_width {
            element.caret_width = self.caret_width;
            flags |= ChangeFlags::PAINT;
        }
        if prev.focus_ring_width != self.focus_ring_width {
            element.focus_ring_width = self.focus_ring_width;
            flags |= ChangeFlags::PAINT;
        }
        // Last: the toolbar's want is computed from the state everything above
        // may have just changed (a field turned disabled wants no toolbar, one
        // turned read-only wants a shorter set of verbs, and a controlled
        // reconcile changes which verbs apply).
        flags |= element.sync_toolbar(ctx);
        flags
    }

    fn teardown(&self, element: &mut TextInputWidget, ctx: &mut BuildCtx<'_>) {
        // The pod is a widget the field mounted; an unmounted field has to tear
        // it down itself, since nothing else holds a view to tear it through.
        let prev = element.toolbar_view.take();
        element.toolbar.rebuild(prev.as_ref(), None, ctx);
        element.toolbar_view_actions = None;
    }
}

impl Widget for TextInputWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Late-registered app fonts (a shell's per-frame font drain, after this
        // widget's private context was built) — a lock and a length compare
        // when nothing is pending. On an actual registration the editor's own
        // retained parley layout is still shaped against the old faces, so it
        // has to be rebuilt: `apply_style` with the unchanged style does
        // exactly that (and takes the masked mirror with it), carrying the same
        // narrow mid-composition caveat a theme swap does — see `apply_style`.
        let fonts_changed = self.text_ctx.sync_app_fonts();
        // Resolve the themed style and rebuild the editor if it drifted from
        // what's currently installed (a theme swap, or a rebuild-invalidated
        // `style`/`text_style_explicit` — see `effective_style`/`apply_style`).
        let effective = self.effective_style(Theme::from_layout_ctx(ctx));
        if fonts_changed || effective != self.applied_style {
            self.apply_style(effective);
        }
        // Catch-all mirror refresh: a rebuild-driven value/obscured change lands
        // before this pass, and everything measured below reads `display()`.
        // Cheap when already in sync (value-equality short-circuit).
        self.sync_mask();

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            DEFAULT_WIDTH
        };

        // Desired soft-wrap width: `Some(px)` reflows the content in multi-line
        // mode; `None` restores the single-line default (so `content_height`/
        // `content_origin_y` agree with the centered single-line placement,
        // resetting any stale wrap left by a dropped `.multiline(..)`). Only
        // re-install it when it actually changed — parley re-shapes on every
        // `set_width` regardless, so an unconditional per-pass call was the
        // TextInput mirror of `Text`'s re-shape-every-frame defect.
        let desired_wrap: Option<f32> = self
            .max_visible_lines
            .map(|_| (width - 2.0 * self.pad_x).max(0.0) as f32);
        if self.applied_wrap_width != Some(desired_wrap) {
            self.editor.set_wrap_width(desired_wrap, &mut self.text_ctx);
            // The masked mirror wraps at the same width (and this is also the
            // call that refreshes a freshly rebuilt mirror's layout).
            if let Some(mask) = self.mask_editor.as_mut() {
                mask.set_wrap_width(desired_wrap, &mut self.text_ctx);
            }
            self.applied_wrap_width = Some(desired_wrap);
        }

        let height = match self.max_visible_lines {
            Some(max_lines) => {
                // Clamp the reported content height to the [1, max_lines] line
                // band (+ padding); overflow scrolls in paint.
                let line_h = self.line_height();
                let content = self.display().layout_size().height.max(line_h);
                let capped = content.min(line_h * max_lines as f64);
                capped + 2.0 * self.pad_y
            }
            None => self.content_height() + 2.0 * self.pad_y,
        };
        // The floated toolbar is laid out against the **window**, never this
        // field's own constraints — it escapes the field's box entirely (see
        // `OverlaySlot::layout`). The window size is recorded for the builder,
        // which `View::rebuild` calls with no `LayoutCtx` in reach.
        self.window_size = ctx.window_size();
        self.toolbar.layout(ctx);
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let chrome = Chrome::resolve(theme, self.enabled);

        // Record the blink epoch from the shared frame clock once per pending
        // reset (a focus/edit flagged it during the clockless event pass).
        let now = ctx.frame_time();
        if self.blink_reset_pending {
            self.blink_epoch = now;
            self.blink_reset_pending = false;
        }
        // The event pass has no clock of its own, so it reads the last painted
        // frame's — that is what dates a completed tap for the double-tap
        // window (see the module docs' "Selection gestures and the toolbar").
        self.last_frame_time = now;

        // The long-press timer, measured across paints exactly as
        // `crate::gesture`'s is: seed the epoch on the press's first painted
        // frame, mark it elapsed on the frame that crosses the threshold, and
        // latch the deferred-callback flush so the very next `RenderRoot::rebuild`
        // dispatches the `Housekeeping` broadcast the fire rides out on.
        //
        // The continuation frames are **plain** requests, not the caret blink's
        // paced one: a long-press has to fire at its threshold in wall time, so
        // it must not be throttled by a frame gate, and it must keep running
        // under `reduce_motion`, which stops the blink's requests entirely.
        if self.captured
            && let Some(hold) = self.hold.as_mut()
        {
            let start = *hold.started_at.get_or_insert(now);
            if !hold.elapsed {
                let held_ms = now.saturating_sub(start).as_secs_f64() * 1000.0;
                if held_ms >= LONG_PRESS_MS {
                    hold.elapsed = true;
                    frust_core::mark_pending_result_flush();
                }
                // Requested on the crossing frame too: on a dirty-driven
                // desktop shell that second request is what actually reaches
                // the rebuild that drains the latch, with no further input.
                ctx.request_frame();
            }
        }

        // The double-tap window is measured against this same paint clock, and
        // that clock only advances while something is painting. While focused
        // the caret blink normally keeps it moving on its own — but
        // `reduce_motion` freezes the caret and drops its frame requests
        // entirely, and an otherwise idle field then leaves `last_frame_time`
        // standing exactly where the completed tap dated itself. The window
        // would never elapse: a press arriving any amount of wall time later
        // would still measure zero and resolve as a double-tap.
        //
        // So while a tap is still within its window and nothing else is
        // pumping the clock, the field asks for its own continuation frames —
        // the same plain (unpaced) request the long-press timer above makes,
        // for the same reason. A gesture threshold has to be measured in wall
        // time even with every animation switched off. Bounded by the window
        // itself: once it has elapsed, the condition stops holding and the
        // field goes back to rest.
        if reduce_motion
            && let Some((_, at)) = self.last_tap
            && now.saturating_sub(at).as_secs_f64() * 1000.0 <= DOUBLE_TAP_MS
        {
            ctx.request_frame();
        }

        // The pod's recorded focus path (threaded in via `PaintCtx::has_focus`)
        // is authoritative — not our own `self.focused`, which lags after a
        // *container-routed* blur (a sibling tap clears the pod's focus without
        // ever calling our `event()`). Observe that here and self-correct so the
        // widget converges one frame after the blur: the accent border, caret,
        // caret-blink continuation frame, and IME republish below all key off
        // `focused`, so they stop together and the field stops resurrecting the
        // IME surface the blur cleared.
        // A disabled field never *behaves* as focused, even if the pod's focus
        // path is still recorded (a `rebuild` that disabled a focused field
        // cannot reach it — see `release_focus_pending`). Read-only is
        // deliberately not part of this test: such a field holds a real
        // session, it just holds one with no keyboard over it.
        let focused = ctx.has_focus() && self.focusable();
        if self.focused && !focused {
            self.focused = false;
            // The toolbar belongs to the session that was just blurred out from
            // under us, so it goes with it: the pod is dropped on the next
            // rebuild, and nothing is registered from this paint on. The
            // toggle's memory goes too — a press that dismissed the bar and
            // then landed on a sibling never reaches the `Down` arm that would
            // otherwise take it, and this is where that press ends up observed.
            self.toolbar_open = false;
            self.outside_press_dismissed = false;
        }

        // Chrome: a border-colored rounded rect with an inset background fills in
        // as the frame (there is no stroke-rect primitive on `PaintScene`).
        // The border width itself widens to `focus_ring_width` while focused,
        // when set — the seam's narrow focus-ring escape hatch (see the module
        // docs' "Chrome" section); unset, it stays `border_width` either way,
        // matching the pre-seam behavior exactly.
        let border_color = if focused {
            chrome.accent
        } else {
            chrome.border
        };
        let border_w = if focused {
            self.focus_ring_width.unwrap_or(self.border_width)
        } else {
            self.border_width
        };
        scene.fill_rounded_rect(origin, size, self.corner_radius, border_color);
        scene.fill_rounded_rect(
            Point::new(origin.x + border_w, origin.y + border_w),
            Size::new(
                (size.width - 2.0 * border_w).max(0.0),
                (size.height - 2.0 * border_w).max(0.0),
            ),
            (self.corner_radius - border_w).max(0.0),
            chrome.bg,
        );

        let text_origin = Point::new(
            origin.x + self.pad_x,
            origin.y + self.content_origin_y(size.height),
        );

        // Multi-line content can overflow the capped box; clip the text band so
        // scrolled-out lines stay inside the field. Popped at the end of paint.
        let clip_content = self.max_visible_lines.is_some();
        if clip_content {
            scene.push_clip(
                Point::new(origin.x + self.border_width, origin.y + self.pad_y),
                Size::new(
                    (size.width - 2.0 * self.border_width).max(0.0),
                    (size.height - 2.0 * self.pad_y).max(0.0),
                ),
            );
        }

        if self.editor.text().is_empty() && !focused {
            // Placeholder: shaped on demand through the widget-owned context.
            if !self.placeholder.is_empty() {
                let mut ph_style = self.style.clone();
                ph_style.color = chrome.placeholder;
                let layout = self.text_ctx.layout(&self.placeholder, &ph_style, None);
                for run in layout.to_scene_runs(text_origin) {
                    scene.draw_glyph_run(run);
                }
            }
        } else {
            let off = text_origin.to_vec2();
            // Selection highlights sit behind the glyphs. Both come from the
            // displayed layout — the masked mirror while obscured.
            for r in self.display().selection_rects() {
                scene.fill_rect(
                    Point::new(r.x0 + off.x, r.y0 + off.y),
                    Size::new(r.width(), r.height()),
                    chrome.selection,
                );
            }
            for run in self.display().to_scene_runs(text_origin) {
                scene.draw_glyph_run(run);
            }
        }

        // Caret: blink while focused. A paced (CosmeticLoop) continuation
        // request at the blink's own [`BLINK_MS`] half-period keeps the desktop
        // shell's wait-loop scheduling paints so the blink animates (the mobile
        // shells' continuous loops already do), while letting the mobile frame
        // gate throttle the cadence — the blink is an indefinite decorative
        // toggle with no endpoint, the same classification as the
        // design-system skeleton/progress/dots/toast/spinner loops, but naming
        // its own slower cadence instead of the theme's cap rate (module docs'
        // "Focus, IME and blink" section). At rest (unfocused) we stop
        // signalling. `reduce_motion` freezes the caret **visible** and stops
        // requesting blink frames entirely — it is a position cue, not a purely
        // decorative loop, so it is the one exception among the paced loops
        // that freezes lit rather than dark.
        if focused {
            // A read-only field's caret is drawn steady. The blink advertises an
            // insertion point, and a field that accepts no insertion has none to
            // advertise — the caret is there to mark where a selection starts,
            // so it stays lit and the field asks for no continuation frames at
            // all (`reduce_motion` freezes it lit for the same reason).
            let blinks = self.editable() && !reduce_motion;
            if blinks {
                ctx.request_frame_paced_at(Duration::from_millis(BLINK_MS as u64));
            }
            // Republish the IME surface every painted frame while focused, so a
            // controlled change applied by a rebuild (a submit clearing the
            // field) refreshes the shell-facing state the event pass would
            // otherwise leave stale — the mobile IME mirror relies on this to
            // observe the clear (see `PaintCtx::publish_ime_state`).
            ctx.publish_ime_state(self.current_ime_state(origin, size));
            let caret_visible = !blinks || self.caret_visible_at(now);
            if caret_visible && let Some(c) = self.display().cursor_rect(self.caret_width) {
                let off = text_origin.to_vec2();
                scene.fill_rect(
                    Point::new(c.x0 + off.x, c.y0 + off.y),
                    Size::new(c.width(), c.height()),
                    chrome.caret,
                );
            }
        } else if !self.focusable() && ctx.has_focus() {
            // Disabled while still holding the pod's focus path: publish an
            // *inactive* IME surface so the shell dismisses the keyboard on the
            // very next frame rather than waiting for the event-pass release
            // (`release_focus_pending`). No caret, and no frame request — a
            // disabled field is at rest. A read-only field never reaches this
            // arm (it is focusable, so the branch above claims it) and must
            // not: it keeps an *active* surface with the keyboard suppressed,
            // which is what leaves the shell's clipboard route wired.
            let mut ime = self.current_ime_state(origin, size);
            ime.active = false;
            ime.caret = None;
            ctx.publish_ime_state(ime);
        }

        if clip_content {
            scene.pop_clip();
        }

        // The selection toolbar, outside the clip because it is not drawn here
        // at all: the root paints every registered pod after the whole main
        // tree, which is the only way the bar escapes this field's box.
        if focused {
            // Recomputed every painted frame, open or not: the anchor an
            // ancestor scrolled or a relayout moved follows for free, and a
            // toolbar opened during the *next* event pass is built (one rebuild
            // later) against a rect that is already current.
            let local_anchor = self.selection_anchor(size.height);
            self.toolbar_anchor = local_anchor + origin.to_vec2();
            // Published on every paint of a focused field, bar or no bar, and
            // under **both** policies: it costs one pointer-sized write and
            // keeps one code path where the platform edit-menu route needs the
            // same facts (see `frust_core::selection_toolbar`).
            //
            // Publishing only while the bar stood is what used to make the
            // platform route disagree with the accessibility one: the module
            // docs' rule that the verbs "never depend on the bar being up"
            // holds for both now. A host responder chain answers "may I offer
            // Paste?" from `actions` whenever it asks — a hardware Cmd+V
            // arrives with nothing on screen, and on iOS it is one of the two
            // paste routes the system exempts from its own permission alert —
            // so gating the answer on a pointer gesture the user never made
            // left every hardware shortcut dead.
            //
            // With no selection the anchor is the caret rect
            // (`selection_anchor`), which is the right place to hang a
            // paste-only menu; `present_menu` carries the bar's own open/closed
            // state as the one edge in the request, so the level below it may
            // change every frame without asking anyone to present anything.
            ctx.publish_selection_toolbar(SelectionToolbarRequest {
                anchor: self.toolbar_anchor,
                actions: self.toolbar_actions(),
                present_menu: self.toolbar_open,
            });
            if self.toolbar_open && selection_toolbar_policy() == SelectionToolbarPolicy::Framework
            {
                // The slot takes the anchor in the field's own local space
                // and lifts it into window space with the paint origin.
                self.toolbar.set_anchor(OverlayAnchor::Rect(local_anchor));
                self.toolbar.paint(ctx, size);
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The focus gate IS the disabled gate: refusing focus here is what
        // makes `Key`/`Ime` (focus-routed, never hit-tested) unreachable, and a
        // disabled field consumes nothing on the way — it is inert, not a
        // shield. A read-only field passes this gate and reaches everything
        // below, so each mutating path carries its own `editable` guard
        // instead (`handle_key`, the `Ime` arm, `handle_command`'s cut/paste).
        if !self.focusable() {
            if ctx.has_focus() || self.release_focus_pending {
                ctx.release_focus();
                self.release_focus_pending = false;
                self.focused = false;
                self.captured = false;
                ctx.request_redraw();
            }
            return EventResult::Ignored;
        }
        // The floated toolbar gets first refusal: its own input arrives as an
        // `InputEvent::Overlay` broadcast addressed to this slot's key, and the
        // slot answers `Some` for exactly what belongs to the surface.
        if let Some(result) = self.toolbar.event(ctx, event, &mut ()) {
            // Drained in the same pass the pod dispatched them in — the queue is
            // pass-scoped, not a mailbox (`EventCtx::dispatch_edit_command`).
            // Drained unconditionally, *then* gated: leaving commands sitting in
            // a pass-scoped queue would hand them to whoever drains it next.
            let commands = ctx.take_edit_commands();
            // The same focus gate the shell-delivered `InputEvent::EditCommand`
            // route enforces, and for the same reason: a field answers a
            // clipboard verb only for a session it actually holds. The pod is
            // only ever mounted while this field is focused, so this is the
            // invariant restated rather than a case seen in practice — but the
            // two routes ending in `handle_command` must not disagree about
            // when a verb is allowed to land.
            if !commands.is_empty() && ctx.has_focus() {
                self.focused = true;
                // A clipboard verb is not part of any tap sequence, exactly as
                // on the shell-delivered route.
                self.last_tap = None;
                for cmd in &commands {
                    self.handle_command(ctx, cmd);
                }
                // Re-claim the session the tap was aimed at. The root never
                // blurs on an overlay press, so this is belt-and-braces rather
                // than the load-bearing half — what matters is that a verb
                // taken from the bar leaves the field exactly as focused as it
                // found it, which is the whole reason the bar is reachable.
                if ctx.has_focus() {
                    ctx.request_focus();
                }
            }
            // A press that landed on nothing floated: the light-dismiss signal
            // this slot registered `OutsideTap::Notify` for. The press itself
            // is not consumed, so the field's own `Down` arm still runs below —
            // this only closes the bar early enough that an outside press on a
            // *sibling* widget closes it too.
            if self.toolbar.take_outside_down() {
                self.outside_press_dismissed = self.toolbar_open;
                self.hide_toolbar(ctx);
            }
            return result;
        }
        match event {
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if inside(p.position, ctx.size()) {
                        // A secondary press is the desktop context-menu gesture
                        // (no mobile shell delivers one): it claims focus —
                        // *starting* a session when there was none, which is
                        // what makes right-clicking an idle field useful —
                        // moves no caret, and toggles the toolbar over whatever
                        // is selected, or over the caret when nothing is. The
                        // caret deliberately stays put where a browser would
                        // relocate it: the menu opens on the selection the user
                        // already has, and a right-click that silently collapsed
                        // it would be the worse surprise.
                        if !presses(p) {
                            ctx.request_focus();
                            self.focused = true;
                            self.toolbar_open = !self.toolbar_open;
                            // A context press is not part of any tap sequence.
                            self.clear_gesture();
                            self.last_tap = None;
                            self.reset_blink();
                            self.publish(ctx);
                            ctx.request_redraw();
                            return EventResult::Handled;
                        }
                        ctx.request_focus();
                        ctx.capture_pointer();
                        self.focused = true;
                        self.captured = true;
                        // Hide rule: every primary press puts the bar away
                        // first. What the press turns out to be (a toggle, a
                        // double-tap, a hold) decides on its own whether to put
                        // one back up — `was_open` is the toggle's memory.
                        let was_open =
                            self.toolbar_open || std::mem::take(&mut self.outside_press_dismissed);
                        self.toolbar_open = false;
                        self.gesture = Gesture::Tap { at: p.position };
                        self.hold = None;
                        self.tap_in_selection = None;
                        let (x, y) = self.editor_point(p.position, ctx.size().height);
                        // A second press on the same spot within the double-tap
                        // window selects the word and opens **nothing**: the
                        // user is selecting, and a bar over the word they are
                        // about to type over is in the way.
                        if self.is_double_tap(p.position) {
                            self.last_tap = None;
                            self.select_word_at_point(ctx, x, y);
                            return EventResult::Handled;
                        }
                        self.last_tap = None;
                        self.hold = Some(HoldState {
                            started_at: None,
                            elapsed: false,
                        });
                        // A press that lands *inside* an existing selection
                        // neither moves the caret nor collapses it: it is
                        // either the start of the toolbar toggle (resolved on
                        // the release, fire-on-up-inside like every other
                        // baseline widget) or the start of an ordinary caret
                        // drag, and only the `Move` past the slop can say which.
                        if self.point_in_selection(p.position, ctx.size().height) {
                            self.tap_in_selection = Some(was_open);
                            ctx.request_redraw();
                            return EventResult::Handled;
                        }
                        self.move_to_point(ctx, x, y, false);
                        EventResult::Handled
                    } else {
                        // A `Down` outside our bounds that still reaches us (we are
                        // the root) is a blur: drop focus. Nested, the container's
                        // routing clears our focus path instead.
                        if self.focused {
                            ctx.release_focus();
                            self.focused = false;
                            self.hide_toolbar(ctx);
                            ctx.request_redraw();
                        }
                        self.clear_gesture();
                        self.last_tap = None;
                        self.outside_press_dismissed = false;
                        EventResult::Ignored
                    }
                }
                PointerPhase::Move => {
                    if !self.captured {
                        return EventResult::Ignored;
                    }
                    // The slop guard, and it outlives the long-press fire:
                    // **both** a still-a-tap press and an already-fired hold
                    // refuse to touch the selection while the finger stays
                    // within `TOUCH_SLOP` of where it landed. Only a press
                    // that actually wandered that far extends anything.
                    match self.gesture {
                        Gesture::Tap { at } => {
                            if (p.position - at).hypot() <= TOUCH_SLOP {
                                // The one thing an in-slop move can do is
                                // deliver a hold whose threshold a paint
                                // already observed (fire-on-move-arrival, so
                                // the word is selected the instant a held
                                // finger jitters rather than waiting out
                                // another frame).
                                self.fire_hold(ctx);
                                return EventResult::Handled;
                            }
                            // Past the slop: it became a drag. The hold is
                            // cancelled, the tap-in-selection candidate with it
                            // (a drag out of a selection is an ordinary caret
                            // drag), and the press stops being a tap for the
                            // double-tap window's purposes.
                            self.hold = None;
                            self.tap_in_selection = None;
                            self.gesture = Gesture::Drag;
                        }
                        Gesture::HoldFired { at } => {
                            if (p.position - at).hypot() <= TOUCH_SLOP {
                                // A held finger jittering over the word it just
                                // selected. `TOUCH_SLOP` is 18 logical px —
                                // several characters wide at a normal text
                                // size, and wide enough to span a word boundary
                                // — so re-resolving the selection from here
                                // would silently redraw it under a finger the
                                // user is holding deliberately still.
                                return EventResult::Handled;
                            }
                            // The finger left the slop: the user is now
                            // dragging the long-press selection outward, which
                            // is an extend like any other (see the module docs'
                            // "Selection gestures and the toolbar" for the
                            // granularity that extend carries). Becoming a
                            // `Drag` is what lets a finger brought back toward
                            // the press point shrink the selection again
                            // instead of freezing it at its widest.
                            self.gesture = Gesture::Drag;
                        }
                        Gesture::Drag | Gesture::None => {}
                    }
                    let (x, y) = self.editor_point(p.position, ctx.size().height);
                    self.move_to_point(ctx, x, y, true);
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    if !self.captured {
                        return EventResult::Ignored;
                    }
                    self.captured = false;
                    // Last chance for a hold whose threshold elapsed with no
                    // pass to fire it on; it consumes the release outright, so
                    // a long-press never also resolves as a tap.
                    if !self.fire_hold(ctx) {
                        // A release inside a selection the press never left
                        // toggles the toolbar: up it goes when the press found
                        // it down, and the press's own hide rule is what closes
                        // it the second time.
                        if let Some(was_open) = self.tap_in_selection.take()
                            && self.gesture.tap_point().is_some()
                        {
                            self.toolbar_open = !was_open;
                        }
                        // Seed the double-tap window, dated by the last painted
                        // frame — a press that wandered past the slop, and one
                        // a long-press resolved, are no longer taps
                        // (`Gesture::tap_point`) and seed nothing.
                        if let Some(down) = self.gesture.tap_point() {
                            self.last_tap = Some((down, self.last_frame_time));
                        }
                    }
                    self.clear_gesture();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if !self.captured {
                        return EventResult::Ignored;
                    }
                    // A `Cancel` must never touch application state: only clear the
                    // drag flag and the in-flight gesture (never the selection,
                    // and never the toolbar — a gesture stolen mid-press says
                    // nothing about whether the bar should still be up), and
                    // request a redraw.
                    self.captured = false;
                    self.clear_gesture();
                    self.last_tap = None;
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            InputEvent::Key(k) => {
                if !ctx.has_focus() {
                    return EventResult::Ignored;
                }
                self.focused = true;
                self.last_tap = None;
                self.handle_key(ctx, &k.key, k.modifiers)
            }
            InputEvent::Ime(e) => {
                if !ctx.has_focus() {
                    return EventResult::Ignored;
                }
                // A read-only field publishes an *active* surface — that is what
                // keeps the shells' clipboard routes wired — but takes no text
                // from it. A composition or commit that arrives anyway is
                // refused exactly as it was when the field held no session.
                if !self.editable() {
                    return EventResult::Ignored;
                }
                self.focused = true;
                self.last_tap = None;
                self.handle_ime(ctx, e)
            }
            // A clipboard verb is focus-routed like `Key`/`Ime` and gated the
            // same way: an unfocused field ignores it rather than answering for
            // a selection the user is not looking at.
            InputEvent::EditCommand(cmd) => {
                if !ctx.has_focus() {
                    return EventResult::Ignored;
                }
                self.focused = true;
                self.last_tap = None;
                self.handle_command(ctx, cmd)
            }
            // Hide rule: the anchor just moved out from under the bar. The
            // scroll itself is still somebody else's (this field scrolls
            // nothing of its own horizontally, and its multi-line offset is
            // caret-driven), so it stays unconsumed.
            InputEvent::Scroll { .. } => {
                self.hide_toolbar(ctx);
                self.last_tap = None;
                EventResult::Ignored
            }
            // The delivery vehicle for a long-press whose threshold an earlier
            // paint observed (see the module docs' "Selection gestures and the
            // toolbar"): the soonest pass carrying a real `EventCtx` when no
            // pointer event arrived first. Still never consumed — a broadcast
            // reports `Ignored` whatever it did.
            InputEvent::Housekeeping => {
                self.fire_hold(ctx);
                EventResult::Ignored
            }
            // A floated surface's own input, already offered to the slot above:
            // reaching this arm means the broadcast was addressed to some other
            // owner's surface, which is none of this field's business.
            InputEvent::Overlay(_) => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // A single-line TextInput node exposing its current text as `value`.
        // accesskit tracks focus at the tree level, so a focused field records
        // itself as the pass's focus node rather than carrying a per-node flag.
        //
        // Obscured, the role becomes `PasswordInput` (the one piece of password
        // semantics this option delivers) and the reported value is the *masked*
        // mirror — an assistive-tech client reads the node value verbatim, so
        // publishing the real secret there would defeat the masking.
        let role = if self.obscured {
            Role::PasswordInput
        } else {
            Role::TextInput
        };
        let value = if self.obscured {
            mask_text(self.editor.text())
        } else {
            self.editor.text().to_string()
        };
        // Disabled and read-only are reported as distinct accesskit node
        // states, never conflated (a screen reader announces them
        // differently — see the module docs' "Read-only mode" section): a
        // disabled field says `set_disabled()`, a read-only *enabled* field
        // says `set_read_only()`. `!enabled` wins if somehow both flags are
        // set — a disabled field is the stronger claim.
        // The clipboard verbs, published on the field's OWN node.
        //
        // They are deliberately NOT keyed to `toolbar_open`: the floating
        // toolbar is a *pointer* affordance, and gating the accessible route on
        // it would mean the verbs existed only for a user who had already
        // performed the long-press that raises it. The floated pod contributes
        // no semantics of its own (the overlay portal's deliberate choice), so
        // this node is the only place the verbs can live.
        //
        // The enabled set is `toolbar_actions()` — the same predicates the bar
        // itself is built from, read once here so the two routes cannot offer
        // different verbs for the same state.
        //
        // accesskit models these as *custom* actions: its `Action` enum has no
        // Copy/Cut/Paste/SelectAll of its own, so each verb is an id plus a
        // label the client reads out (see `A11Y_CUT_ID` on why the ids are
        // stable). Only the enabled ones are published — an action offered and
        // then refused is worse than one never offered.
        //
        // Gated on `focusable()` as a whole, the way mounting the bar is: a
        // disabled field never holds the focus a verb routes along, and
        // `toolbar_actions()` alone would still offer `copy` over a selection it
        // happened to be showing. A read-only field passes — it is copyable, and
        // `toolbar_actions()` has already withheld the cut and paste it must not
        // offer. Focus itself is deliberately *not* required — a client explores
        // the tree before it acts, and a field that advertised nothing until
        // focused would not be discovered.
        let verbs = self.toolbar_actions();
        let offered: Vec<CustomAction> = [
            (A11Y_CUT_ID, "Cut", verbs.cut),
            (A11Y_COPY_ID, "Copy", verbs.copy),
            (A11Y_PASTE_ID, "Paste", verbs.paste),
            (A11Y_SELECT_ALL_ID, "Select all", verbs.select_all),
        ]
        .into_iter()
        .filter(|(_, _, enabled)| *enabled && self.focusable())
        .map(|(id, description, _)| CustomAction {
            id,
            description: description.into(),
        })
        .collect();
        let id = ctx.push_node(role, |node| {
            node.set_value(value);
            if !self.enabled {
                node.set_disabled();
            } else if self.read_only {
                node.set_read_only();
            }
            if !offered.is_empty() {
                node.add_action(Action::CustomAction);
                node.set_custom_actions(offered);
            }
        });
        if self.focused {
            ctx.set_focused(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{
        FrameTime, KeyEvent, Modifiers, PointerButton, PointerEvent, RenderRoot, ScrollDelta,
        SelectionToolbarBuilder, set_selection_toolbar_builder, set_selection_toolbar_policy,
    };
    use std::any::Any;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct AppState {
        value: String,
        changes: u32,
        submits: u32,
        last_submit: String,
        reject: bool,
    }

    fn app_logic(state: &mut AppState) -> TextInputView<AppState> {
        text_input(state.value.clone(), |s: &mut AppState, v: String| {
            s.changes += 1;
            if !s.reject {
                s.value = v;
            }
        })
        .placeholder("type here")
        .on_submit(|s: &mut AppState, v: String| {
            s.submits += 1;
            s.last_submit = v;
        })
    }

    /// Build + lay out a render root over `app_logic`, ready for events.
    fn harness(state: &mut AppState) -> RenderRoot<AppState, TextInputView<AppState>> {
        let mut root = RenderRoot::new();
        root.rebuild(&mut app_logic, state);
        root.layout(Size::new(300.0, 200.0));
        root
    }

    fn widget(root: &RenderRoot<AppState, TextInputView<AppState>>) -> &TextInputWidget {
        let id = root.root_id().expect("root built");
        let w = root.tree().pod(id).expect("root pod").widget();
        (w as &dyn Any)
            .downcast_ref::<TextInputWidget>()
            .expect("root is a TextInputWidget")
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// The same event on the secondary (right) button.
    fn secondary_pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Secondary,
        })
    }

    fn ch(text: &str) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character(text.to_string()),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn named(key: NamedKey, modifiers: Modifiers) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(key),
            modifiers,
            repeat: false,
        })
    }

    #[test]
    fn tap_focuses_and_publishes_ime_state() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());

        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        assert!(root.is_focus_active(), "tap inside focuses the field");
        assert!(widget(&root).focused);
        let ime = root.ime_state().expect("focus publishes an IME surface");
        assert!(ime.active);
        assert!(ime.caret.is_some(), "an IME surface carries a caret rect");
    }

    #[test]
    fn a_secondary_press_focuses_opens_the_toolbar_and_moves_no_caret() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);

        // Unfocused: a right-click is the desktop context-menu gesture, so it
        // *starts* a session (a menu over a field nobody is editing is still
        // the field's menu) and opens the toolbar over the caret.
        root.event(
            &mut state,
            &secondary_pointer(PointerPhase::Down, 10.0, 10.0),
        );
        assert!(root.is_focus_active(), "a context press claims focus");
        assert!(widget(&root).focused);
        assert!(
            root.ime_state().is_some_and(|ime| ime.active),
            "and publishes the session's IME surface like any other focus claim"
        );
        assert!(widget(&root).toolbar_open, "and opens the toolbar");

        // Again: the toggle puts it away, leaving the session standing.
        root.event(
            &mut state,
            &secondary_pointer(PointerPhase::Down, 10.0, 10.0),
        );
        assert!(
            !widget(&root).toolbar_open,
            "a second context press closes it"
        );
        assert!(root.is_focus_active(), "and never blurs the field");

        // Focused and typed into: a right-click anywhere in the field leaves the
        // caret where it is, and leaves the live session alone — it is consumed
        // as a tap *inside* the field, never read as an outside-tap blur.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        for c in ["a", "b", "c"] {
            root.event(&mut state, &ch(c));
        }
        let caret = widget(&root).editor.editing_state_bytes().extent;
        assert_eq!(caret, 3, "the caret sits after the typed text");

        let outcome = root.event(
            &mut state,
            &secondary_pointer(PointerPhase::Down, 1.0, 10.0),
        );

        assert!(outcome.handled, "the field still swallows the press");
        assert_eq!(
            widget(&root).editor.editing_state_bytes().extent,
            caret,
            "a right-click places no caret"
        );
        assert!(!widget(&root).captured, "and starts no selection drag");
        assert!(root.is_focus_active(), "the typing session survives it");
        assert!(widget(&root).focused);
        assert!(widget(&root).toolbar_open, "and it opens the toolbar");
    }

    #[test]
    fn typing_inserts_and_fires_on_change() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));

        assert_eq!(state.value, "hi", "on_change fed each char into app state");
        assert_eq!(state.changes, 2);
        assert_eq!(widget(&root).editor.text(), "hi");
    }

    #[test]
    fn keys_ignored_while_unfocused() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        // No prior focus: a key is not consumed and does not edit.
        let outcome = root.event(&mut state, &ch("x"));
        assert!(!outcome.handled);
        assert_eq!(state.changes, 0);
        assert_eq!(widget(&root).editor.text(), "");
    }

    #[test]
    fn backspace_over_emoji_removes_grapheme() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("a"));
        root.event(&mut state, &ch("\u{1F600}")); // grinning face (4 bytes)
        assert_eq!(widget(&root).editor.text(), "a\u{1F600}");

        root.event(
            &mut state,
            &named(NamedKey::Backspace, Modifiers::default()),
        );

        // The whole emoji code point is removed as a unit (the editor's
        // grapheme integrity), not a single byte.
        assert_eq!(widget(&root).editor.text(), "a");
        assert_eq!(state.value, "a");
    }

    #[test]
    fn arrows_with_shift_extend_selection() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        for c in ["a", "b", "c"] {
            root.event(&mut state, &ch(c));
        }
        let changes_before = state.changes;

        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        root.event(&mut state, &named(NamedKey::ArrowLeft, shift));

        let es = widget(&root).editor.editing_state_bytes();
        assert_ne!(es.base, es.extent, "shift+arrow extends the selection");
        assert_eq!(
            state.changes, changes_before,
            "a selection-only move does not fire on_change"
        );
    }

    #[test]
    fn enter_fires_on_submit_once_and_keeps_focus() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));

        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));

        assert_eq!(state.submits, 1, "Enter fires on_submit exactly once");
        assert_eq!(state.last_submit, "hi");
        assert!(root.is_focus_active(), "submit keeps focus");
    }

    #[test]
    fn newline_commit_submits_instead_of_inserting() {
        // The iOS Return path: UITextInput's Return arrives as insertText("\n")
        // → ImeEvent::Commit("\n"). Must behave exactly like NamedKey::Enter on
        // this single-line widget — submit, keep the text newline-free.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));

        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit("\n".to_string())),
        );

        assert_eq!(state.submits, 1, "newline commit fires on_submit once");
        assert_eq!(state.last_submit, "hi");
        assert!(
            !widget(&root).editor.text().contains('\n'),
            "no literal newline lands in the single-line field"
        );
        assert!(root.is_focus_active(), "submit keeps focus");

        // CRLF variant (some platforms/hardware keyboards): same behavior.
        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit("\r\n".to_string())),
        );
        assert_eq!(state.submits, 2);
        assert!(!widget(&root).editor.text().contains('\r'));
    }

    #[test]
    fn escape_releases_focus() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.is_focus_active());

        root.event(&mut state, &named(NamedKey::Escape, Modifiers::default()));

        assert!(!root.is_focus_active(), "Escape blurs the field");
        assert!(root.ime_state().is_none());
        assert!(!widget(&root).focused);
    }

    #[test]
    fn blur_via_outside_tap_unpublishes_ime_state() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.ime_state().is_some());

        // Tap outside the field's height (still reaches the root widget).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 150.0));

        assert!(!root.is_focus_active(), "outside tap blurs the field");
        assert!(root.ime_state().is_none());
        assert!(!widget(&root).focused);
    }

    #[test]
    fn meta_a_selects_all_without_typing() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        for c in ["a", "b", "c"] {
            root.event(&mut state, &ch(c));
        }
        let changes_before = state.changes;

        let meta = Modifiers {
            meta: true,
            ..Modifiers::default()
        };
        root.event(
            &mut state,
            &InputEvent::Key(KeyEvent {
                key: Key::Character("a".to_string()),
                modifiers: meta,
                repeat: false,
            }),
        );

        let es = widget(&root).editor.editing_state_bytes();
        assert_eq!(es.base.min(es.extent), 0);
        assert_eq!(es.base.max(es.extent), 3, "whole buffer selected");
        assert_eq!(
            state.changes, changes_before,
            "select-all does not type an 'a'"
        );
        assert_eq!(widget(&root).editor.text(), "abc");
    }

    #[test]
    fn controlled_reconcile_rejected_value_shows_app_value() {
        let mut state = AppState {
            reject: true,
            ..AppState::default()
        };
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        // The app rejects the edit: on_change fires but `value` stays empty while
        // the editor already holds "x".
        root.event(&mut state, &ch("x"));
        assert_eq!(state.changes, 1);
        assert_eq!(state.value, "");
        assert_eq!(widget(&root).editor.text(), "x");

        // The next rebuild reconciles the editor back to the app's (empty) value.
        root.rebuild(&mut app_logic, &mut state);
        assert_eq!(
            widget(&root).editor.text(),
            "",
            "a rejected edit is pulled back to the app's value on rebuild"
        );
    }

    #[test]
    fn controlled_reconcile_same_value_preserves_selection() {
        let mut state = AppState {
            value: "ab".to_string(),
            ..AppState::default()
        };
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        let meta = Modifiers {
            meta: true,
            ..Modifiers::default()
        };
        root.event(
            &mut state,
            &InputEvent::Key(KeyEvent {
                key: Key::Character("a".to_string()),
                modifiers: meta,
                repeat: false,
            }),
        );
        let before = widget(&root).editor.editing_state_bytes();
        assert_ne!(before.base, before.extent);

        // Rebuild with the unchanged value: no reconcile, selection preserved.
        root.rebuild(&mut app_logic, &mut state);
        let after = widget(&root).editor.editing_state_bytes();
        assert_eq!(
            (before.base, before.extent),
            (after.base, after.extent),
            "an unchanged value leaves the selection intact"
        );
    }

    #[test]
    fn ime_apply_editing_state_syncs_value() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        // Mobile state-sync path: push a whole editing value (UTF-16 indexed).
        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::ApplyEditingState(EditingState {
                text: "hello".to_string(),
                selection_base: 5,
                selection_extent: 5,
                composing_base: -1,
                composing_extent: -1,
            })),
        );

        assert_eq!(widget(&root).editor.text(), "hello");
        assert_eq!(state.value, "hello", "on_change reflects the synced value");
    }

    #[test]
    fn ime_empty_compose_retracts_the_preedit_without_panicking() {
        // Every shell spells a cancelled/abandoned composition as a `Compose`
        // with empty text. The editor's set-marked-text primitive asserts the
        // text is non-empty, so this test is the assertion: in a debug build
        // (which is what `cargo test` runs) the old routing panicked here.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Compose {
                text: "ni".to_string(),
                cursor: Some((2, 2)),
            }),
        );
        assert_eq!(widget(&root).editor.text(), "ni", "the preedit is live");

        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Compose {
                text: String::new(),
                cursor: None,
            }),
        );
        assert_eq!(
            widget(&root).editor.text(),
            "",
            "the marked text is retracted, not replaced with an empty preedit"
        );
        assert_eq!(
            widget(&root).editor.editing_state_bytes().composing,
            None,
            "and no composing region is left behind"
        );
        assert_eq!(state.value, "");

        // A retraction with nothing marked is a no-op, not a second panic.
        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Compose {
                text: String::new(),
                cursor: None,
            }),
        );
        assert_eq!(widget(&root).editor.text(), "");
    }

    #[test]
    fn ime_empty_commit_retracts_the_preedit_without_panicking() {
        // The same defect through the commit arm, which reaches the same
        // primitive: a platform that reports an empty commit for a composition
        // that produced nothing must retract, not assert.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Compose {
                text: "ni".to_string(),
                cursor: None,
            }),
        );
        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit(String::new())),
        );

        assert_eq!(widget(&root).editor.text(), "");
        assert_eq!(widget(&root).editor.editing_state_bytes().composing, None);
    }

    #[test]
    fn ime_commit_inserts_text() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit("ni".to_string())),
        );

        assert_eq!(widget(&root).editor.text(), "ni");
        assert_eq!(state.value, "ni");
    }

    #[test]
    fn cancel_disarms_drag_without_touching_state() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(widget(&root).captured);
        let changes_before = state.changes;

        root.event(&mut state, &pointer(PointerPhase::Cancel, 10.0, 10.0));

        assert!(!widget(&root).captured, "Cancel disarms the drag");
        assert_eq!(
            state.changes, changes_before,
            "Cancel must not fire on_change"
        );
    }

    #[test]
    fn blink_requests_frame_only_while_focused() {
        // A fresh, unfocused field does not ask for continuation frames.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        let mut sink = NullScene;
        assert!(
            !root.paint(&mut sink, FrameTime::ZERO).needs_frame,
            "an unfocused field is at rest"
        );

        // Once focused, paint pumps the blink and asks for the next frame — a
        // paced (CosmeticLoop) request, the same classification the
        // design-system decorative loops (skeleton/progress/dots/toast) use,
        // since the blink is an indefinite toggle with no endpoint the mobile
        // frame gate may throttle. Focused with a *completed* tap: a finger
        // still down is a live long-press timer, whose continuation frames are
        // deliberately unpaced (see `the_long_press_timer_requests_plain_frames_
        // while_the_blink_paces`).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(outcome.needs_frame, "a focused field blinks its caret");
        assert!(
            outcome.needs_frame_paced_only,
            "the caret blink is a CosmeticLoop request — the frame gate must be able to pace it"
        );
    }

    #[test]
    fn reduce_motion_keeps_the_double_tap_window_pumping_its_own_clock() {
        // The double-tap window is dated from the paint clock, and under
        // reduce_motion the caret blink — normally the only thing keeping that
        // clock moving on an idle focused field — stops requesting frames
        // entirely. Without a request of its own the window would never
        // elapse, and a press arriving any amount of wall time later would
        // still measure zero against a frozen clock and resolve as a
        // double-tap.
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut root = harness(&mut state);
        let mut theme = Theme::neutral();
        theme.motion.reduce_motion = true;
        root.set_theme(Box::new(theme));

        // Date the completed tap from a painted frame at t=0.
        root.paint(&mut NullScene, ft_ms(0.0));
        tap(&mut root, &mut state, 20.0, 10.0);
        assert!(
            widget(&root).last_tap.is_some(),
            "sanity: the tap seeded a double-tap window"
        );

        // Inside the window: the field asks for the frame that will advance
        // the clock, even though the caret is frozen and asking for nothing.
        let inside = root.paint(&mut NullScene, ft_ms(50.0));
        assert!(
            inside.needs_frame,
            "a live double-tap window must pump its own clock under reduce_motion"
        );

        // Past it: the field goes back to rest rather than spinning frames.
        let outside = root.paint(&mut NullScene, ft_ms(DOUBLE_TAP_MS + 100.0));
        assert!(
            !outside.needs_frame,
            "an elapsed window stops asking — the request is bounded by the window"
        );

        // And the window really has elapsed: a press this late is an ordinary
        // caret placement, not a second tap selecting the word.
        root.event(&mut state, &pointer(PointerPhase::Down, 20.0, 10.0));
        assert_eq!(
            selection(&root),
            None,
            "past DOUBLE_TAP_MS the press is just a press"
        );
    }

    #[test]
    fn blink_paces_at_its_own_500ms_interval() {
        // The caret names its own (slower) cadence via
        // `request_frame_paced_at` rather than a bare `request_frame_paced`
        // (the theme's cosmetic-loop cap) — asserted on the paint outcome's
        // `paced_interval`, mirroring `frust-core`'s
        // `paint_surfaces_the_requested_paced_interval_on_outcome`.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        let mut sink = NullScene;
        // A completed tap, so no long-press timer is live — see
        // `blink_requests_frame_only_while_focused`.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(outcome.needs_frame_paced_only);
        assert_eq!(
            outcome.paced_interval,
            Some(Duration::from_millis(BLINK_MS as u64)),
            "the caret paces at its own 500ms half-period, not the theme cap"
        );
    }

    #[test]
    fn reduce_motion_freezes_the_caret_visible_and_stops_requesting_frames() {
        // reduce_motion is the one exception among the paced loops: the caret
        // freezes VISIBLE (it is a position cue), not hidden, and stops
        // requesting blink frames entirely while frozen.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        let mut theme = Theme::neutral();
        theme.motion.reduce_motion = true;
        root.set_theme(Box::new(theme));
        let caret_color = Theme::neutral().scheme().primary;

        // Focus seeds `blink_epoch` at the first paint (t=0). A completed tap,
        // so the long-press timer — which keeps requesting plain frames
        // precisely *because* reduce_motion must not stop a gesture from
        // firing — is not live here.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        root.paint(&mut NullScene, ft_ms(0.0));

        // Mid-cycle from that epoch — an unfrozen caret would be hidden here
        // (see `caret_visibility_toggles_across_the_blink_period`) — but
        // reduce_motion must still paint it, and request no continuation frame.
        let mut mid = CaretRecorder {
            caret_color: Some(caret_color),
            caret_fills: 0,
        };
        let outcome = root.paint(&mut mid, ft_ms(BLINK_MS + 10.0));
        assert_eq!(
            mid.caret_fills, 1,
            "reduce_motion freezes the caret visible, even mid-blink-cycle"
        );
        assert!(
            !outcome.needs_frame,
            "a frozen caret must not request a continuation frame"
        );

        // Clearing the token resumes the ordinary paced blink.
        let mut theme = Theme::neutral();
        theme.motion.reduce_motion = false;
        root.set_theme(Box::new(theme));
        let resumed = root.paint(&mut NullScene, ft_ms(2.0 * BLINK_MS + 20.0));
        assert!(
            resumed.needs_frame_paced_only,
            "blinking resumes once reduce_motion clears"
        );
        assert_eq!(
            resumed.paced_interval,
            Some(Duration::from_millis(BLINK_MS as u64))
        );
    }

    /// A `FrameTime` `ms` milliseconds from the origin.
    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    #[test]
    fn caret_visibility_toggles_across_the_blink_period() {
        let mut state = AppState::default();
        let root = harness(&mut state);
        let w = widget(&root);
        // Deterministic phase math off the reset epoch (blink_epoch == ZERO).
        assert!(
            w.caret_visible_at(ft_ms(0.0)),
            "visible at the start of the cycle"
        );
        assert!(w.caret_visible_at(ft_ms(BLINK_MS - 1.0)));
        assert!(
            !w.caret_visible_at(ft_ms(BLINK_MS + 1.0)),
            "hidden mid-cycle"
        );
        assert!(
            w.caret_visible_at(ft_ms(2.0 * BLINK_MS + 1.0)),
            "visible again"
        );
    }

    /// A scene that counts caret fills — the caret is the only bare `fill_rect`
    /// emitted with the caret color once a non-empty selection isn't present.
    #[derive(Default)]
    struct CaretRecorder {
        caret_color: Option<Color>,
        caret_fills: usize,
    }

    impl PaintScene for CaretRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, color: Color) {
            if Some(color) == self.caret_color {
                self.caret_fills += 1;
            }
        }
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    #[test]
    fn caret_blink_phase_advances_from_paint_frame_time() {
        // Two-frame blink test: the caret is painted in the visible half
        // of the cycle and absent in the hidden half, with the phase measured
        // purely from the injected `RenderRoot::paint` frame time — proving the
        // blink advances off the shell clock, not a hidden wall clock.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        // Focus so the caret is painted; the focus Down flags a blink reset.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        // Frame 1 at t=0: seeds blink_epoch=0 and paints the caret (visible half).
        let mut f1 = CaretRecorder {
            caret_color: Some(CARET),
            caret_fills: 0,
        };
        root.paint(&mut f1, ft_ms(0.0));
        assert_eq!(f1.caret_fills, 1, "caret visible at the start of the cycle");

        // Frame 2 mid-cycle (t = BLINK_MS + a bit): the caret is hidden.
        let mut f2 = CaretRecorder {
            caret_color: Some(CARET),
            caret_fills: 0,
        };
        root.paint(&mut f2, ft_ms(BLINK_MS + 10.0));
        assert_eq!(
            f2.caret_fills, 0,
            "caret hidden mid-cycle (phase from paint time)"
        );

        // Frame 3 in the next visible half proves the phase keeps advancing.
        let mut f3 = CaretRecorder {
            caret_color: Some(CARET),
            caret_fills: 0,
        };
        root.paint(&mut f3, ft_ms(2.0 * BLINK_MS + 10.0));
        assert_eq!(f3.caret_fills, 1, "caret visible again in the next cycle");
    }

    // --- Themed chrome ---

    /// Records rounded-rect (chrome) and rect (selection/caret) fill colors.
    #[derive(Default)]
    struct ChromeRecorder {
        rrects: Vec<Color>,
        rects: Vec<Color>,
    }

    impl PaintScene for ChromeRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, color: Color) {
            self.rects.push(color);
        }
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, color: Color) {
            self.rrects.push(color);
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, _run: frust_scene::GlyphRun) {}
    }

    /// Paint a focused field (so the accent border + caret show) at frame time 0.
    fn paint_chrome(root: &mut RenderRoot<AppState, TextInputView<AppState>>) -> ChromeRecorder {
        let mut rec = ChromeRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        rec
    }

    #[test]
    fn unthemed_chrome_uses_fallback_constants() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let rec = paint_chrome(&mut root);
        // Border (focused → accent) then background.
        assert_eq!(rec.rrects, vec![ACCENT, BG]);
        // The only bare rect on an empty focused field is the caret.
        assert_eq!(rec.rects, vec![CARET]);
    }

    #[test]
    fn themed_chrome_resolves_roles() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.set_theme(Box::new(Theme::neutral()));
        let theme = Theme::neutral();
        let scheme = theme.scheme();
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let rec = paint_chrome(&mut root);
        assert_eq!(
            rec.rrects,
            vec![scheme.primary, scheme.surface],
            "focused border is primary, background is surface"
        );
        assert_eq!(rec.rects, vec![scheme.primary], "caret is primary");
    }

    // --- Chrome geometry seam: padding / border_width / corner_radius /
    // caret_width / focus_ring_width ---

    /// Records rounded-rect (chrome) and rect (selection/caret) fill
    /// *geometry* — origin/size/radius/color — so the seam's effect on the
    /// actual painted chrome can be asserted, not just that the builder
    /// accepted a value.
    #[derive(Default)]
    struct ChromeGeometryRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        rects: Vec<(Point, Size, Color)>,
    }

    impl PaintScene for ChromeGeometryRecorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, _run: frust_scene::GlyphRun) {}
    }

    #[test]
    fn default_chrome_geometry_matches_the_unthemed_constants() {
        // Pins the resolved default geometry: a field that calls none of the
        // geometry setters must render byte-for-byte at the constants.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        let field_size = root.layout(Size::new(300.0, 200.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        let mut rec = ChromeGeometryRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);

        let (outer_origin, outer_size, outer_radius, _) = rec.rrects[0];
        let (inner_origin, inner_size, inner_radius, _) = rec.rrects[1];
        assert_eq!(outer_size, field_size, "outer chrome rect covers the field");
        assert_eq!(outer_radius, RADIUS, "default corner radius is RADIUS");
        assert_eq!(
            inner_origin.x - outer_origin.x,
            BORDER_W,
            "default border width is BORDER_W"
        );
        assert_eq!(inner_origin.y - outer_origin.y, BORDER_W);
        assert_eq!(
            inner_size,
            Size::new(
                outer_size.width - 2.0 * BORDER_W,
                outer_size.height - 2.0 * BORDER_W
            )
        );
        assert_eq!(inner_radius, RADIUS - BORDER_W);

        let (_, caret_size, _) = *rec.rects.last().expect("caret painted while focused");
        assert!(
            (caret_size.width - CARET_W as f64).abs() < 1e-6,
            "default caret width is CARET_W"
        );
    }

    #[test]
    fn custom_padding_changes_the_resolved_height_and_published_caret_offset() {
        let mut default_state = AppState::default();
        let mut default_root = harness(&mut default_state);
        let default_size = default_root.layout(Size::new(300.0, 200.0));
        default_root.event(&mut default_state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let default_caret_x = default_root
            .ime_state()
            .expect("focused")
            .caret
            .expect("caret rect")
            .x0;

        let mut state = AppState::default();
        let mut logic = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v)
                .padding(30.0, 40.0)
        };
        let mut root = RenderRoot::new();
        root.rebuild(&mut logic, &mut state);
        let custom_size = root.layout(Size::new(300.0, 200.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let custom_caret_x = root
            .ime_state()
            .expect("focused")
            .caret
            .expect("caret rect")
            .x0;

        assert_eq!(
            custom_size.height - default_size.height,
            2.0 * (40.0 - PAD_Y),
            "vertical padding is reflected in the resolved field height, not just accepted"
        );
        assert!(
            (custom_caret_x - default_caret_x - (30.0 - PAD_X)).abs() < 1e-6,
            "horizontal padding shifts the published caret rect"
        );
    }

    #[test]
    fn custom_border_width_and_corner_radius_resize_the_painted_chrome() {
        let mut state = AppState::default();
        let mut logic = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v)
                .border_width(4.0)
                .corner_radius(2.0)
        };
        let mut root = RenderRoot::new();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(300.0, 200.0));

        let mut rec = ChromeGeometryRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);

        let (outer_origin, outer_size, outer_radius, _) = rec.rrects[0];
        let (inner_origin, inner_size, inner_radius, _) = rec.rrects[1];
        assert_eq!(
            outer_radius, 2.0,
            "corner_radius reaches the painted outer rect"
        );
        assert_eq!(
            inner_radius, 0.0,
            "inner radius clamps at 0 once border_width exceeds corner_radius"
        );
        assert_eq!(
            inner_origin.x - outer_origin.x,
            4.0,
            "border_width insets the fill"
        );
        assert_eq!(inner_origin.y - outer_origin.y, 4.0);
        assert_eq!(
            inner_size,
            Size::new(outer_size.width - 8.0, outer_size.height - 8.0)
        );
    }

    #[test]
    fn custom_caret_width_changes_the_painted_caret_rect() {
        let mut state = AppState::default();
        let mut logic = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v).caret_width(6.0)
        };
        let mut root = RenderRoot::new();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(300.0, 200.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        let mut rec = ChromeGeometryRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        let (_, caret_size, _) = *rec.rects.last().expect("caret painted");
        assert!(
            (caret_size.width - 6.0).abs() < 1e-6,
            "caret_width is reflected in the painted caret rect, not just accepted by the builder"
        );
    }

    #[test]
    fn custom_focus_ring_width_only_widens_the_border_while_focused() {
        // The focus treatment specifically: unfocused, the idle border_width
        // is unaffected; focused, focus_ring_width takes over.
        let mut state = AppState::default();
        let mut logic = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v)
                .focus_ring_width(5.0)
        };
        let mut root = RenderRoot::new();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(300.0, 200.0));

        let mut idle = ChromeGeometryRecorder::default();
        root.paint(&mut idle, FrameTime::ZERO);
        let (idle_outer, _, _, _) = idle.rrects[0];
        let (idle_inner, _, _, _) = idle.rrects[1];
        assert_eq!(
            idle_inner.x - idle_outer.x,
            BORDER_W,
            "idle border stays the default width when unfocused"
        );

        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let mut focused = ChromeGeometryRecorder::default();
        root.paint(&mut focused, FrameTime::ZERO);
        let (focused_outer, _, _, _) = focused.rrects[0];
        let (focused_inner, _, _, _) = focused.rrects[1];
        assert_eq!(
            focused_inner.x - focused_outer.x,
            5.0,
            "the focused appearance responds to focus_ring_width"
        );
    }

    #[test]
    fn geometry_field_changes_request_the_right_change_flags() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);

        // Padding changes the resolved `Size`, so it must relayout.
        let mut padded = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v)
                .padding(20.0, 20.0)
        };
        let flags = root.rebuild(&mut padded, &mut state);
        assert!(flags.needs_layout(), "a padding change must relayout");

        // Border width alone is paint-only geometry — it never resizes the
        // field, so it must not force a relayout on top of an unrelated
        // padding change that already did.
        let mut bordered = |s: &mut AppState| {
            text_input(s.value.clone(), |s: &mut AppState, v: String| s.value = v)
                .padding(20.0, 20.0)
                .border_width(4.0)
        };
        let flags = root.rebuild(&mut bordered, &mut state);
        assert!(flags.needs_paint());
        assert!(
            !flags.needs_layout(),
            "border_width alone does not resize the field"
        );
    }

    #[test]
    fn paint_refreshes_ime_state_after_a_controlled_clear() {
        // The mobile IME mirror relies on `ime_state()` tracking the field even
        // when an app-driven controlled change (a submit clearing the draft) is
        // applied by a *rebuild* rather than an event. The event pass alone
        // leaves the published state stale (it only refreshes on edits); the
        // focused widget republishes during paint, which runs after every
        // rebuild. This is the regression guard for that refresh.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        let mut sink = NullScene;

        // Focus and type "hi" through the event pass — ime_state now reads "hi".
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));
        assert_eq!(root.ime_state().expect("focused").editing.text, "hi");

        // Simulate an app-driven controlled clear (what `on_submit` → clear draft
        // does): set the controlled value to "" and run a frame with NO event.
        state.value.clear();
        root.rebuild(&mut app_logic, &mut state);
        root.layout(Size::new(300.0, 200.0));
        root.paint(&mut sink, FrameTime::ZERO);

        let ime = root
            .ime_state()
            .expect("still focused, so still publishing");
        assert_eq!(
            ime.editing.text, "",
            "paint refreshes the shell-facing IME state to the controlled-cleared value \
             (without this, the mobile IME mirror re-pushes the stale text)"
        );
    }

    // --- Themed text color ---

    /// Records each glyph run's solid brush color (mirrors `text.rs`'s
    /// `GlyphRecorder`).
    #[derive(Default)]
    struct TextGlyphRecorder {
        colors: Vec<Color>,
    }

    impl PaintScene for TextGlyphRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
            if let peniko::Brush::Solid(color) = run.brush {
                self.colors.push(color);
            }
        }
    }

    /// Paint `root` and return the first content glyph run's brush color.
    fn painted_text_color(root: &mut RenderRoot<AppState, TextInputView<AppState>>) -> Color {
        let mut rec = TextGlyphRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        *rec.colors.first().expect("one glyph run painted")
    }

    // --- Placeholder family resolution ---

    /// Records font bytes from each glyph run, enabling font-resolution testing.
    #[derive(Default)]
    struct PlaceholderFontRecorder {
        font_bytes: Vec<Vec<u8>>,
    }

    impl PaintScene for PlaceholderFontRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
            // Capture the font bytes from this glyph run.
            self.font_bytes.push(run.font.font().data.as_ref().to_vec());
        }
    }

    /// Paint `root` and return the font bytes of the first glyph run's font.
    fn painted_placeholder_font_bytes(
        root: &mut RenderRoot<AppState, TextInputView<AppState>>,
    ) -> Vec<u8> {
        let mut rec = PlaceholderFontRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        rec.font_bytes
            .first()
            .expect("one glyph run painted")
            .clone()
    }

    #[test]
    fn unthemed_text_input_keeps_black_default() {
        // Parity: with no theme threaded in, the glyph color stays exactly the
        // TextStyle default (black) — unchanged from before this retrofit.
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut root = harness(&mut state);
        assert_eq!(painted_text_color(&mut root), Color::BLACK);
    }

    #[test]
    fn themed_dark_text_input_resolves_on_surface() {
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut root = harness(&mut state);

        let mut theme = Theme::neutral();
        theme.brightness = frust_theme::Brightness::Dark;
        let expected = theme.scheme().on_surface;
        assert_ne!(
            expected,
            Color::BLACK,
            "fixture sanity: dark on_surface must differ from the black fallback"
        );
        root.set_theme(Box::new(theme));
        root.layout(Size::new(300.0, 200.0));

        assert_eq!(
            painted_text_color(&mut root),
            expected,
            "a themed field's glyphs resolve to on_surface(dark), not black"
        );
    }

    #[test]
    fn explicit_text_style_wins_over_theme() {
        let custom = Color::from_rgb8(10, 20, 30);
        fn logic(state: &mut AppState) -> TextInputView<AppState> {
            text_input(state.value.clone(), |s: &mut AppState, v: String| {
                s.value = v;
            })
            .text_style(TextStyle::new(16.0, Color::from_rgb8(10, 20, 30)))
        }

        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut root: RenderRoot<AppState, TextInputView<AppState>> = RenderRoot::new();
        root.rebuild(&mut logic, &mut state);
        root.set_theme(Box::new(Theme::neutral()));
        root.layout(Size::new(300.0, 200.0));

        assert_eq!(
            painted_text_color(&mut root),
            custom,
            "an explicit .text_style() color wins over the themed default"
        );
    }

    #[test]
    fn rebuild_with_new_text_style_relayouts_with_new_metrics() {
        // rebuild() must reconcile a changed text_style: a larger font size
        // rebuilds the underlying TextEditor (via `apply_style`) and grows the
        // field's measured height on the next layout.
        fn logic_a(state: &mut AppState) -> TextInputView<AppState> {
            text_input(state.value.clone(), |s: &mut AppState, v: String| {
                s.value = v;
            })
            .text_style(TextStyle::new(16.0, Color::BLACK))
        }
        fn logic_b(state: &mut AppState) -> TextInputView<AppState> {
            text_input(state.value.clone(), |s: &mut AppState, v: String| {
                s.value = v;
            })
            .text_style(TextStyle::new(40.0, Color::BLACK))
        }

        let mut state = AppState::default();
        let mut root: RenderRoot<AppState, TextInputView<AppState>> = RenderRoot::new();
        root.rebuild(&mut logic_a, &mut state);
        let size_a = root.layout(Size::new(300.0, 200.0));

        root.rebuild(&mut logic_b, &mut state);
        let size_b = root.layout(Size::new(300.0, 200.0));

        assert!(
            size_b.height > size_a.height,
            "a rebuild with a larger text_style size grows the field height \
             ({size_a:?} -> {size_b:?})"
        );
    }

    #[test]
    fn theme_swap_with_no_view_change_repaints_new_glyph_color() {
        // Mirrors `text.rs`'s regression of the same name: a bare `set_theme`
        // with no view change must still re-resolve the baked color at the
        // next layout, per the `set_theme` -> `ChangeFlags::LAYOUT` contract.
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut root = harness(&mut state);

        let mut theme_a = Theme::neutral();
        theme_a.brightness = frust_theme::Brightness::Light;
        let color_a = theme_a.scheme().on_surface;
        root.set_theme(Box::new(theme_a));
        root.layout(Size::new(300.0, 200.0));
        assert_eq!(painted_text_color(&mut root), color_a);

        let mut theme_b = Theme::neutral();
        theme_b.brightness = frust_theme::Brightness::Dark;
        let color_b = theme_b.scheme().on_surface;
        assert_ne!(
            color_a, color_b,
            "fixture sanity: themes must actually differ"
        );
        root.set_theme(Box::new(theme_b));
        root.layout(Size::new(300.0, 200.0));

        assert_eq!(
            painted_text_color(&mut root),
            color_b,
            "a bare theme swap (no view change) must re-resolve the themed glyph \
             color at the next layout"
        );
    }

    // --- Nested-blur regression ---
    //
    // A `TextInput` nested in a `Column` beside a `Button`. Focusing the field
    // then tapping the sibling is a *container-routed* blur: `route_event`
    // clears the field pod's recorded focus path, but the widget's `event()` is
    // never called on that dispatch. If the widget-internal `focused` flag
    // stayed set, the next `paint` would republish the (already-cleared) IME
    // surface and pump a caret-blink continuation frame — resurrecting the
    // dismissed keyboard and keeping the desktop wait-loop spinning forever.
    // Threading the pod's focus into paint (`PaintCtx::has_focus`) is what makes
    // the widget observe the blur and converge.

    #[derive(Default)]
    struct NestedState {
        value: String,
    }

    fn nested_logic(state: &mut NestedState) -> crate::FlexView<NestedState> {
        use frust_core::any;
        crate::Column(vec![
            any(text_input(
                state.value.clone(),
                |s: &mut NestedState, v: String| {
                    s.value = v;
                },
            )),
            any(crate::button("ok", |_s: &mut NestedState| {})),
        ])
    }

    #[test]
    fn nested_blur_clears_ime_and_idles_paint() {
        let mut state = NestedState::default();
        let mut root: RenderRoot<NestedState, crate::FlexView<NestedState>> = RenderRoot::new();
        root.rebuild(&mut nested_logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);

        // Focus the field with a full tap (Down+Up) inside its bounds, at the
        // top of the column. The Up releases the field's pointer capture, so the
        // next Down is hit-tested afresh instead of routing back to the field.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert!(root.is_focus_active(), "tap inside the field focuses it");
        assert!(
            root.ime_state().is_some(),
            "focusing the nested field publishes an IME surface"
        );

        // Tap the sibling button, below the field (a container-routed blur): the
        // field's pod focus is cleared by `route_event`, but its own `event()` is
        // never called on this dispatch. The `Up` completes the button's own
        // tap cycle (`Button` has a press-scale
        // animation that lazily launches at the next `paint` — completing the
        // gesture here, with no paint in between, cancels the retarget before
        // it ever launches, so this stays a pure blur probe rather than also
        // asserting anything about the button's own animation).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 45.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 45.0));

        // Leg 1 — the blur event clears the shell-facing focus + IME surface.
        assert!(!root.is_focus_active(), "the sibling tap blurs the field");
        assert!(
            root.ime_state().is_none(),
            "container-routed blur clears the IME surface"
        );

        // Legs 2 & 3 — a subsequent paint must NOT resurrect the cleared IME
        // surface, and must report the tree at rest (no stale caret-blink frame).
        let mut sink = NullScene;
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(
            root.ime_state().is_none(),
            "paint must not republish the cleared IME surface (F1 resurrection)"
        );
        assert!(
            !outcome.needs_frame,
            "a blurred field paints at rest — the desktop wait-loop idles"
        );
    }

    /// Two-level nesting: the field sits inside an INNER Column, whose pod is a
    /// child of the outer Column beside the button. A blur tap on the button
    /// clears the focus link at the outer level only (the inner Column's pod) —
    /// the field's own pod flag deep in the blurred subtree legitimately stays
    /// stale, which is exactly the case `paint_child`'s ancestor-composed
    /// `has_focus` seeding must cover.
    fn deep_nested_logic(state: &mut NestedState) -> crate::FlexView<NestedState> {
        use frust_core::any;
        crate::Column(vec![
            any(crate::Column(vec![any(text_input(
                state.value.clone(),
                |s: &mut NestedState, v: String| {
                    s.value = v;
                },
            ))])),
            any(crate::button("ok", |_s: &mut NestedState| {})),
        ])
    }

    #[test]
    fn deep_nested_blur_idles_paint_despite_stale_inner_focus_flag() {
        let mut state = NestedState::default();
        let mut root: RenderRoot<NestedState, crate::FlexView<NestedState>> = RenderRoot::new();
        root.rebuild(&mut deep_nested_logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);

        // Focus the field (full Down+Up tap — the Up releases capture so the
        // blur Down hit-tests afresh; see nested_blur_clears_ime_and_idles_paint).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert!(root.is_focus_active());
        assert!(root.ime_state().is_some());

        // Blur via the OUTER-level sibling: route_event clears the focused link
        // at the outer Column (the inner Column's pod); the field's own pod flag
        // two levels down stays stale. The `Up` completes the button's own tap
        // cycle (see `nested_blur_clears_ime_and_idles_paint`'s identical note)
        // so this stays a pure blur probe.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 45.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 45.0));
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());

        // Paint: the cleared outer link must force has_focus == false for the
        // whole subtree (ancestor-composed seeding), so the stale deep flag
        // cannot re-arm the caret blink or republish the IME surface.
        let mut sink = NullScene;
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(
            root.ime_state().is_none(),
            "deep-nested stale focus flag must not resurrect the IME surface"
        );
        assert!(
            !outcome.needs_frame,
            "deep-nested stale focus flag must not busy-loop the desktop shell"
        );
    }

    // --- Cross-branch unmount: a stale flag must not kill a live session ------
    //
    // The shipped shape is a scrollable list above a persistent composer. The
    // user taps a row's field, then taps the composer: `route_event`'s
    // blur-on-outside-tap breaks the focus link at the NEAREST COMMON ANCESTOR
    // (the outer Column's pod for the list branch), so the row field's own pod
    // flag, one level deeper, legitimately stays set. The list then recycles /
    // filters / shrinks and the generic reconciler tears that row down.
    //
    // The row owns no session — the composer does. A release here is a
    // user-visible input regression: the keyboard drops mid-typing in a field
    // nothing touched. The effective-focus chain threaded through `BuildCtx`
    // (`has_focus`/`with_focus_link`) is what tells the two apart, and both
    // directions are pinned below over the same fixture.

    #[derive(Default)]
    struct BranchState {
        row: String,
        composer: String,
        show_row: bool,
        show_composer: bool,
    }

    /// Outer Column: [ inner Column (the "list", 0-or-1 row field), composer
    /// field ]. Each field's initial text names its branch, so `ime_state`
    /// identifies which one owns the session.
    fn branches_logic(state: &mut BranchState) -> crate::FlexView<BranchState> {
        use frust_core::{AnyView, any};
        let mut rows: Vec<AnyView<BranchState>> = Vec::new();
        if state.show_row {
            rows.push(any(text_input(
                state.row.clone(),
                |s: &mut BranchState, v: String| s.row = v,
            )));
        }
        let mut outer: Vec<AnyView<BranchState>> = vec![any(crate::Column(rows))];
        if state.show_composer {
            outer.push(any(text_input(
                state.composer.clone(),
                |s: &mut BranchState, v: String| s.composer = v,
            )));
        }
        crate::Column(outer)
    }

    /// Build the two-branch tree with both fields present, focus the ROW field,
    /// then focus the COMPOSER — leaving the row's own pod flag stale below the
    /// cleared outer link. Returns the root, its text context and the state.
    fn focus_row_then_composer() -> (
        RenderRoot<BranchState, crate::FlexView<BranchState>>,
        frust_text::TextContext,
        BranchState,
    ) {
        let mut state = BranchState {
            row: "row".to_string(),
            composer: "composer".to_string(),
            show_row: true,
            show_composer: true,
        };
        let mut root: RenderRoot<BranchState, crate::FlexView<BranchState>> = RenderRoot::new();
        root.rebuild(&mut branches_logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);

        // Tap the row field (full Down+Up so the Up releases its capture and the
        // next Down hit-tests afresh — see nested_blur_clears_ime_and_idles_paint).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(
            root.ime_state().map(|s| s.editing.text),
            Some("row".to_string()),
            "the row field owns the session first"
        );

        // Tap the composer: focus moves branches. The blur clears the link at the
        // outer Column only; the row field's flag inside the inner Column stays.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 50.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 50.0));
        assert!(root.is_focus_active());
        assert_eq!(
            root.ime_state().map(|s| s.editing.text),
            Some("composer".to_string()),
            "the composer now owns the session"
        );
        (root, tcx, state)
    }

    #[test]
    fn tearing_down_a_stale_focused_branch_leaves_the_live_session_alone() {
        let (mut root, _tcx, mut state) = focus_row_then_composer();
        let ime_before = root.ime_state();
        let gen_before = root.focus_ime_generation();

        // The list shrinks: the row field — whose stale flag is still set — is
        // torn down by the generic reconciler.
        state.show_row = false;
        root.rebuild(&mut branches_logic, &mut state);

        assert!(
            root.is_focus_active(),
            "unmounting a stale-flagged row must not blur the composer the user is typing in"
        );
        assert_eq!(
            root.ime_state(),
            ime_before,
            "the live IME surface is untouched by an unrelated branch's unmount"
        );
        assert_eq!(
            root.focus_ime_generation(),
            gen_before,
            "zero spurious edges: the shell's frame gate must see no focus/IME change at all"
        );
    }

    #[test]
    fn tearing_down_the_live_focused_branch_still_releases_the_session() {
        // The twin of the test above over the same fixture: when the pod that
        // actually holds the session dies, the release must still happen — the
        // narrowed gate must not have turned into "never mark".
        let (mut root, _tcx, mut state) = focus_row_then_composer();
        let gen_before = root.focus_ime_generation();

        state.show_composer = false;
        root.rebuild(&mut branches_logic, &mut state);

        assert!(
            !root.is_focus_active(),
            "the focused composer's unmount releases the session"
        );
        assert_eq!(
            root.ime_state(),
            None,
            "the shell-facing surface dies with the widget that published it"
        );
        assert_eq!(
            root.focus_ime_generation(),
            gen_before.wrapping_add(1),
            "one orphaned live focus path is exactly one edge"
        );
    }

    // --- Wrapper type swap: the severing path `rebuild_child` owns -----------
    //
    // `Padding(if editing { text_input } else { text })` is the single-child
    // wrapper shape: the view swaps the padded child's concrete type, so the
    // focused widget is torn down inside `AnyView::rebuild` and a fresh,
    // non-focusable one takes its pod. Nothing about that reaches the root by
    // itself — no event, no publish — so without the orphan mark the keyboard
    // stays up over an idle screen, `is_focus_active()` keeps reporting a
    // session, and `Key`/`Ime` events keep routing into a widget that ignores
    // them. Both directions are pinned below over one fixture, exactly like the
    // cross-branch unmount pair above.

    #[derive(Default)]
    struct WrapState {
        row: String,
        composer: String,
        row_editing: bool,
    }

    /// Outer Column: [ Padding-wrapped slot, composer field ]. The slot holds a
    /// `text_input` while `row_editing` and a plain `text` label otherwise, so
    /// flipping the flag is an `AnyView` type swap inside the wrapper — the
    /// `rebuild_child` path, not the multi-child reconciler's.
    fn wrapped_slot_logic(state: &mut WrapState) -> crate::FlexView<WrapState> {
        use frust_core::{AnyView, any};
        // The branch is taken *inside* the wrapper, so the wrapper's own view
        // type is stable across the flip and only its child's concrete type
        // changes — the `rebuild_child` swap. (Branching outside and handing
        // `Padding` a pre-erased `AnyView` would double-erase the child and
        // hide the swap from the wrapper entirely; see the note in the
        // completion summary.)
        let slot: AnyView<WrapState> = if state.row_editing {
            any(crate::Padding(
                crate::EdgeInsets::all(0.0),
                text_input(state.row.clone(), |s: &mut WrapState, v: String| s.row = v),
            ))
        } else {
            any(crate::Padding(
                crate::EdgeInsets::all(0.0),
                crate::text(state.row.clone()),
            ))
        };
        crate::Column(vec![
            slot,
            any(text_input(
                state.composer.clone(),
                |s: &mut WrapState, v: String| s.composer = v,
            )),
        ])
    }

    /// Build the fixture and focus the **wrapped** field, leaving it (and the
    /// wrapper pod above it) on the live focus chain.
    fn focus_the_wrapped_field() -> (
        RenderRoot<WrapState, crate::FlexView<WrapState>>,
        frust_text::TextContext,
        WrapState,
    ) {
        let mut state = WrapState {
            row: "row".to_string(),
            composer: "composer".to_string(),
            row_editing: true,
        };
        let mut root: RenderRoot<WrapState, crate::FlexView<WrapState>> = RenderRoot::new();
        root.rebuild(&mut wrapped_slot_logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);

        // Full Down+Up so the Up releases the capture and a later Down
        // hit-tests afresh (see the cross-branch fixture above).
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(
            root.ime_state().map(|s| s.editing.text),
            Some("row".to_string()),
            "the wrapped field owns the session"
        );
        (root, tcx, state)
    }

    #[test]
    fn swapping_a_live_focused_wrapper_child_releases_the_session() {
        let (mut root, _tcx, mut state) = focus_the_wrapped_field();
        let gen_before = root.focus_ime_generation();

        // The wrapper's child type-swaps out from under the focused field.
        state.row_editing = false;
        root.rebuild(&mut wrapped_slot_logic, &mut state);

        assert!(
            !root.is_focus_active(),
            "the root's focus mirror must not outlive the type-swapped field"
        );
        assert_eq!(
            root.ime_state(),
            None,
            "the shell-facing surface dies with the widget that published it"
        );
        assert_eq!(
            root.focus_ime_generation(),
            gen_before.wrapping_add(1),
            "one severed live focus path is exactly one edge"
        );
    }

    #[test]
    fn swapping_a_stale_focused_wrapper_child_leaves_the_live_session_alone() {
        let (mut root, _tcx, mut state) = focus_the_wrapped_field();

        // Focus moves to the composer: the blur clears the link at the outer
        // Column (the wrapper's own pod), leaving the wrapped field's flag one
        // level deeper legitimately stale.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 50.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 50.0));
        assert_eq!(
            root.ime_state().map(|s| s.editing.text),
            Some("composer".to_string()),
            "the composer now owns the session"
        );
        let ime_before = root.ime_state();
        let gen_before = root.focus_ime_generation();

        // The same swap as the twin above, now on a dead branch.
        state.row_editing = false;
        root.rebuild(&mut wrapped_slot_logic, &mut state);

        assert!(
            root.is_focus_active(),
            "swapping a stale-flagged wrapper child must not blur the composer \
             the user is typing in"
        );
        assert_eq!(
            root.ime_state(),
            ime_before,
            "the live IME surface is untouched by an unrelated branch's swap"
        );
        assert_eq!(
            root.focus_ime_generation(),
            gen_before,
            "zero spurious edges: the shell's frame gate must see no focus/IME change"
        );
    }

    /// A no-op paint sink for `needs_frame` assertions (glyph/rect output is not
    /// under test here).
    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    // --- Multi-line mode ---

    /// A 3-visible-line multi-line field (newline-on-Enter default).
    fn multiline_logic(state: &mut AppState) -> TextInputView<AppState> {
        text_input(state.value.clone(), |s: &mut AppState, v: String| {
            s.changes += 1;
            if !s.reject {
                s.value = v;
            }
        })
        .multiline(3)
        .on_submit(|s: &mut AppState, v: String| {
            s.submits += 1;
            s.last_submit = v;
        })
    }

    /// A multi-line field that submits on Enter (newline only on Shift+Enter).
    fn multiline_submit_logic(state: &mut AppState) -> TextInputView<AppState> {
        text_input(state.value.clone(), |s: &mut AppState, v: String| {
            s.changes += 1;
            s.value = v;
        })
        .multiline(3)
        .submit_on_enter(true)
        .on_submit(|s: &mut AppState, v: String| {
            s.submits += 1;
            s.last_submit = v;
        })
    }

    /// A tall window so the field's natural (capped) height is never clamped by
    /// the root constraints; returns the laid-out field height.
    fn relayout(root: &mut RenderRoot<AppState, TextInputView<AppState>>) -> f64 {
        root.layout(Size::new(300.0, 800.0)).height
    }

    #[test]
    fn multiline_height_grows_per_line_up_to_cap_then_stops() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        let h1 = relayout(&mut root); // one (empty) line

        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        // Enter inserts a newline in this newline-on-Enter field.
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        let h2 = relayout(&mut root); // two lines
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        let h3 = relayout(&mut root); // three lines (== the cap)

        assert!(h2 > h1, "a second line grows the field: {h1} -> {h2}");
        assert!(h3 > h2, "a third line grows the field: {h2} -> {h3}");

        // A fourth and fifth line exceed the 3-line cap: the box height freezes.
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        let h4 = relayout(&mut root);
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        let h5 = relayout(&mut root);
        assert_eq!(h4, h3, "height stops growing at the visible-line cap");
        assert_eq!(h5, h4, "still capped past the cap");
    }

    #[test]
    fn multiline_scroll_offset_engages_only_past_the_cap() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        // Within the cap (two lines) the content fits: no scroll.
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        let h = relayout(&mut root);
        assert_eq!(
            widget(&root).scroll_y(h),
            0.0,
            "content within the cap never scrolls"
        );

        // Push past the cap: the caret (on the last line) must be kept in view by
        // a positive scroll offset.
        for _ in 0..4 {
            root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        }
        let h = relayout(&mut root);
        assert!(
            widget(&root).scroll_y(h) > 0.0,
            "content past the cap scrolls to keep the caret in view"
        );
    }

    #[test]
    fn enter_inserts_newline_in_multiline_by_default() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        root.event(&mut state, &ch("a"));
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        root.event(&mut state, &ch("b"));

        assert_eq!(
            widget(&root).editor.text(),
            "a\nb",
            "Enter inserts a newline"
        );
        assert_eq!(state.submits, 0, "a newline-on-Enter field does not submit");
        assert_eq!(state.value, "a\nb", "the newline flows through on_change");
    }

    #[test]
    fn shift_enter_submits_in_a_newline_on_enter_multiline() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));

        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        root.event(&mut state, &named(NamedKey::Enter, shift));

        assert_eq!(state.submits, 1, "Shift+Enter inverts the default → submit");
        assert_eq!(state.last_submit, "hi");
        assert!(
            !widget(&root).editor.text().contains('\n'),
            "Shift+Enter must not insert a newline here"
        );
    }

    #[test]
    fn submit_on_enter_multiline_submits_and_shift_enter_inserts_newline() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_submit_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("h"));
        root.event(&mut state, &ch("i"));

        // Plain Enter submits (explicit submit_on_enter(true)).
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        assert_eq!(
            state.submits, 1,
            "Enter submits when submit_on_enter is set"
        );
        assert!(!widget(&root).editor.text().contains('\n'));

        // Shift+Enter inverts it → insert a newline instead.
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        root.event(&mut state, &named(NamedKey::Enter, shift));
        assert_eq!(state.submits, 1, "Shift+Enter does not submit here");
        assert!(
            widget(&root).editor.text().contains('\n'),
            "Shift+Enter inserts a newline when submit_on_enter is set"
        );
    }

    #[test]
    fn ime_newline_commit_inserts_newline_in_multiline() {
        // The iOS Return path (Commit("\n")) inserts a literal newline in a
        // newline-on-Enter multi-line field, rather than submitting.
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("a"));

        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit("\n".to_string())),
        );

        assert!(
            widget(&root).editor.text().contains('\n'),
            "a newline commit lands a literal newline in a multi-line field"
        );
        assert_eq!(state.submits, 0, "the newline commit does not submit");
    }

    #[test]
    fn arrow_up_down_move_caret_across_lines_in_multiline() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        root.layout(Size::new(300.0, 800.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("a"));
        root.event(&mut state, &ch("b"));
        root.event(&mut state, &named(NamedKey::Enter, Modifiers::default()));
        root.event(&mut state, &ch("c"));
        root.event(&mut state, &ch("d")); // caret at end of line 2 (byte 5)
        assert_eq!(widget(&root).editor.editing_state_bytes().extent, 5);

        // ArrowUp crosses onto line 1 (byte offset within [0, 2]).
        root.event(&mut state, &named(NamedKey::ArrowUp, Modifiers::default()));
        let up = widget(&root).editor.editing_state_bytes().extent;
        assert!(up <= 2, "ArrowUp moves the caret onto line 1, got {up}");

        // ArrowDown returns to line 2 (byte offset >= 3, past the newline).
        root.event(
            &mut state,
            &named(NamedKey::ArrowDown, Modifiers::default()),
        );
        let down = widget(&root).editor.editing_state_bytes().extent;
        assert!(
            down >= 3,
            "ArrowDown moves the caret back to line 2, got {down}"
        );
    }

    /// Regression: `Widget::layout`'s single-line branch must reset the
    /// editor's wrap width, or a field rebuilt from multiline into single-line
    /// keeps the stale wrapped layout — `content_height` (and, downstream, the
    /// text's vertical placement) stays wrong until something else happens to
    /// touch the wrap width again.
    #[test]
    fn multiline_to_single_line_rebuild_resets_wrap_width() {
        // Long enough to wrap across several lines at a narrow width.
        let long_text = "one two three four five six seven eight nine ten".to_string();
        let mut state = AppState {
            value: long_text.clone(),
            ..AppState::default()
        };
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        let multi_height = root.layout(Size::new(120.0, 800.0)).height;
        assert!(
            widget(&root).editor.line_count() > 1,
            "seed text must actually wrap for this regression to be meaningful"
        );

        // Rebuild the *same* root into a single-line field (multiline dropped)
        // with the identical text — the value is unchanged, so `rebuild` never
        // calls `set_controlled_value`; only `max_visible_lines` flips to
        // `None`, exercising the single-line `layout` branch on a widget whose
        // editor still carries the old wrap width.
        root.rebuild(&mut app_logic, &mut state);
        let single_height = root.layout(Size::new(120.0, 800.0)).height;

        assert_eq!(
            widget(&root).editor.line_count(),
            1,
            "single-line relayout must reset the wrap width so the editor \
             reflows back onto one line"
        );
        assert!(
            single_height < multi_height,
            "single-line relayout must collapse the stale wrapped height: \
             multi={multi_height}, single={single_height}"
        );

        // Cross-check against a field built single-line from scratch with the
        // same text/width: the rebuilt-down field must match it exactly, not
        // merely be smaller.
        let mut fresh_state = AppState {
            value: long_text,
            ..AppState::default()
        };
        let mut fresh_root = RenderRoot::new();
        fresh_root.rebuild(&mut app_logic, &mut fresh_state);
        let fresh_height = fresh_root.layout(Size::new(120.0, 800.0)).height;
        assert_eq!(
            single_height, fresh_height,
            "a multiline->single-line rebuild must match a field built \
             single-line from scratch"
        );

        // Layout and caret placement agree: the origin's centering formula is
        // now measured against the corrected (single-line) content height.
        let w = widget(&root);
        assert_eq!(
            w.content_origin_y(single_height),
            w.text_top(single_height),
            "single-line mode must use the centered single-line placement"
        );
    }

    #[test]
    fn placeholder_inherits_font_family_from_style() {
        // Verify that an empty unfocused field shows a placeholder with the input's
        // configured font family, weight, and letter-spacing — not the default style.
        use frust_text::{FontFamily, FontWeight};

        let mut state = AppState::default();
        let mut root = RenderRoot::new();

        // Build with a custom font family style.
        let custom_family = FontFamily::named("Monospace");
        let custom_style = TextStyle {
            family: custom_family.clone(),
            weight: FontWeight::BOLD,
            size: 18.0,
            letter_spacing: 1.5,
            ..TextStyle::default()
        };

        root.rebuild(
            &mut |s: &mut AppState| {
                text_input(s.value.clone(), |_s: &mut AppState, _v: String| {})
                    .placeholder("Enter text")
                    .text_style(custom_style.clone())
            },
            &mut state,
        );
        root.layout(Size::new(300.0, 200.0));

        let w = widget(&root);
        // Verify the widget's configured style has the custom family.
        assert_eq!(
            w.style.family, custom_family,
            "widget style should have the custom family"
        );
        assert_eq!(
            w.style.weight,
            FontWeight::BOLD,
            "widget style should have the custom weight"
        );
        assert_eq!(
            w.style.letter_spacing, 1.5,
            "widget style should have the custom letter-spacing"
        );

        // Paint the widget (the placeholder will be rendered since the field is empty and unfocused).
        let mut sink = NullScene;
        root.paint(&mut sink, FrameTime::ZERO);

        // The test verifies that the placeholder is laid out without crashing and
        // the field's style is correctly applied. A proper pixel-level assertion
        // would require inspecting glyph runs directly (which RecordingScene doesn't
        // support), but the layout success itself proves the family was accepted.
    }

    #[test]
    fn placeholder_font_reflects_configured_style_family() {
        // Genuine shaped-output assertion: verify that an empty,
        // unfocused field's placeholder shapes with the input's configured font
        // family, not a fallback. Two TextInputs—one with default family (SystemUi),
        // one with an explicit named family—must resolve to different fonts in
        // their placeholder runs. If `ph_style` regresses to `TextStyle::new(size,
        // color)`, both would drop the family and resolve to the same default font,
        // causing this assertion to fail.
        use frust_text::{FontFamily, GenericSlot};

        // First TextInput: default style (no explicit family).
        // The placeholder will shape with the default family (SystemUi).
        let mut state_default = AppState::default();
        let mut root_default = RenderRoot::new();
        root_default.rebuild(
            &mut |s: &mut AppState| {
                text_input(s.value.clone(), |_s: &mut AppState, _v: String| {}).placeholder("test")
            },
            &mut state_default,
        );
        root_default.layout(Size::new(300.0, 200.0));
        let default_font = painted_placeholder_font_bytes(&mut root_default);

        // Second TextInput: explicit monospace family using stack_with_generic.
        // Monospace is a generic family available on all platforms; it will
        // resolve to a different system font than SystemUi on any test host.
        let monospace_style = TextStyle {
            family: FontFamily::stack_with_generic(Vec::<String>::new(), GenericSlot::Monospace),
            ..TextStyle::default()
        };
        let mut state_monospace = AppState::default();
        let mut root_monospace = RenderRoot::new();
        root_monospace.rebuild(
            &mut |s: &mut AppState| {
                text_input(s.value.clone(), |_s: &mut AppState, _v: String| {})
                    .placeholder("test")
                    .text_style(monospace_style.clone())
            },
            &mut state_monospace,
        );
        root_monospace.layout(Size::new(300.0, 200.0));
        let monospace_font = painted_placeholder_font_bytes(&mut root_monospace);

        // Assert: the two placeholders resolved to different fonts.
        // If ph_style regressed (losing the family), both would use SystemUi
        // and resolve to the same font.
        assert_ne!(
            default_font, monospace_font,
            "placeholder with Monospace family should resolve to a different font \
             than the default SystemUi family; if this fails, ph_style likely \
             regressed to not preserving the configured family"
        );
    }

    // --- App-font parity (the shell drain must reach the private context) ---
    //
    // A `TextInput` owns a private `TextContext` (module docs' Text context
    // ownership). The defect this section pins: if that private context were a
    // bare `TextContext::new()`, an app font registered through
    // `frust::register_app_fonts` — drained by the shell into the *shell-owned*
    // context, the one `LayoutCtx::text_context` threads to `Text` — would never
    // reach the field, which would silently fall back to a platform face.
    //
    // `frust-widgets` cannot call `frust::register_app_fonts` (its registry
    // lives in `frust-shell-common`, above this crate), so these tests
    // reproduce the drain's single observable effect instead:
    // `FontRegistryWatcher::drain_into` is a `TextContext::register_fonts` call
    // on the shell-owned context, and nothing else.

    /// The registered test font's bytes — the same public-domain subsetted
    /// asset `frust-text`'s own registration tests use (and which
    /// `frust-shell-common` likewise includes cross-crate rather than
    /// duplicating a font file per crate).
    const TUFFY: &[u8] = include_bytes!("../../frust-text/tests/fonts/Tuffy-Subset.ttf");

    /// The same face with its `name` table rewritten to "Helvetica". Used for
    /// the late-registration test so the *pre-registration* leg is meaningful
    /// on any host: "Helvetica" resolves to whatever the platform has (or a
    /// fallback) before registration, and to these exact bytes after, per
    /// fontique's registered-shadows-system-family rule.
    const TUFFY_AS_HELVETICA: &[u8] =
        include_bytes!("../../frust-text/tests/fonts/Tuffy-As-Helvetica.ttf");

    /// Tuffy's digit advance at 24px, in logical px — measured from this exact
    /// asset through this exact parley pin (see `docs/DEVELOPMENT.md`'s
    /// Version-Pin Policy). Hard-coded so the assertion is on the *shaped
    /// metric*, not merely on "some font was registered": the device symptom
    /// behind this bug was a wrong per-glyph advance (1-em fallback boxes)
    /// while registration itself reported success.
    const TUFFY_DIGIT_ADVANCE_24PX: f32 = 13.3125;

    /// Slack on [`TUFFY_DIGIT_ADVANCE_24PX`]: parley lays out with
    /// `quantize = true`, so a run's successive x deltas land on subpixel
    /// boundaries and one digit pair in ten reads ~0.12px short of the nominal
    /// advance. Far tighter than any real font swap (a fallback face differs by
    /// whole pixels at this size).
    const ADVANCE_TOLERANCE: f32 = 0.25;

    /// The digits string every advance assertion shapes — uniform-width in
    /// Tuffy, so one expected advance covers every glyph in the run.
    const DIGITS: &str = "0123456789";

    /// Records each painted glyph run's resolved font bytes and per-glyph
    /// advances (successive x deltas) — shaped output, not registration state.
    #[derive(Default)]
    struct ShapedRunRecorder {
        runs: Vec<(Vec<u8>, Vec<f32>)>,
    }

    impl PaintScene for ShapedRunRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
            let advances = run.glyphs.windows(2).map(|w| w[1].x - w[0].x).collect();
            self.runs
                .push((run.font.font().data.as_ref().to_vec(), advances));
        }
    }

    /// Paint `root` and return the first glyph run's `(font bytes, advances)`.
    fn painted_shaped_run(
        root: &mut RenderRoot<AppState, TextInputView<AppState>>,
    ) -> (Vec<u8>, Vec<f32>) {
        let mut rec = ShapedRunRecorder::default();
        root.paint(&mut rec, FrameTime::ZERO);
        rec.runs.first().expect("one glyph run painted").clone()
    }

    /// A 24px style in `family`.
    fn family_style(family: frust_text::FontFamily) -> TextStyle {
        TextStyle {
            family,
            ..TextStyle::new(24.0, Color::BLACK)
        }
    }

    /// A laid-out field carrying `value` (empty = the placeholder path) in
    /// `family`, built *now* — i.e. with whatever fonts are registered at call
    /// time, which is the ordering under test.
    fn field_in_family(
        state: &mut AppState,
        family: frust_text::FontFamily,
    ) -> RenderRoot<AppState, TextInputView<AppState>> {
        let style = family_style(family);
        let mut root = RenderRoot::new();
        root.rebuild(
            &mut |s: &mut AppState| {
                text_input(s.value.clone(), |_s: &mut AppState, _v: String| {})
                    .placeholder(DIGITS)
                    .text_style(style.clone())
            },
            state,
        );
        root.layout(Size::new(600.0, 200.0));
        root
    }

    /// Assert the shaped run resolved to `expected`'s exact bytes.
    ///
    /// Compared behind a `bool` rather than `assert_eq!` on purpose: an
    /// `assert_eq!` between two font files prints both in full, which is tens
    /// of megabytes of failure output per failing test.
    fn assert_same_font(font: &[u8], expected: &[u8], what: &str) {
        assert!(
            font == expected,
            "{what}: shaped against a different face than the registered app \
             font ({} bytes shaped vs {} expected)",
            font.len(),
            expected.len()
        );
    }

    /// Assert the shaped run resolved to something *other* than `other`'s bytes.
    fn assert_other_font(font: &[u8], other: &[u8], what: &str) {
        assert!(
            font != other,
            "{what}: expected a different face, got the same {} bytes",
            font.len()
        );
    }

    /// Assert every advance in `advances` is `expected` (within quantization
    /// slack — see [`ADVANCE_TOLERANCE`]).
    fn assert_uniform_advance(advances: &[f32], expected: f32, what: &str) {
        assert!(!advances.is_empty(), "{what}: no advances measured");
        for a in advances {
            assert!(
                (a - expected).abs() <= ADVANCE_TOLERANCE,
                "{what}: shaped advance {a} != the registered font's own {expected} \
                 (all advances: {advances:?})"
            );
        }
    }

    #[test]
    fn app_registered_font_shapes_the_field_content() {
        // The shell's construction-time drain, reproduced exactly.
        let mut shell_ctx = frust_text::TextContext::new();
        shell_ctx
            .register_fonts(TUFFY.to_vec())
            .expect("valid TTF bytes must register");

        // A field built *after* that drain — the real ordering: a design system
        // registers its fonts from `app!`'s setup block, before the first
        // rebuild builds any widget.
        let mut state = AppState {
            value: DIGITS.to_string(),
            ..AppState::default()
        };
        let mut root = field_in_family(&mut state, frust_text::FontFamily::named("Tuffy"));
        let (font, advances) = painted_shaped_run(&mut root);

        assert_same_font(&font, TUFFY, "content");
        assert_uniform_advance(&advances, TUFFY_DIGIT_ADVANCE_24PX, "content");

        // Negative control: an unregistered family name resolves to a fallback
        // face, whose bytes cannot be the registered asset's.
        let mut fallback_state = AppState {
            value: DIGITS.to_string(),
            ..AppState::default()
        };
        let mut fallback_root = field_in_family(
            &mut fallback_state,
            frust_text::FontFamily::named("Frust No Such Family"),
        );
        let (fallback_font, fallback_advances) = painted_shaped_run(&mut fallback_root);
        assert_other_font(&fallback_font, TUFFY, "unregistered-family control");
        assert_ne!(
            fallback_advances, advances,
            "fixture sanity: the fallback face must shape these digits to \
             different metrics, or this test could pass without the app font"
        );
    }

    #[test]
    fn app_registered_font_shapes_the_placeholder() {
        // The placeholder shapes on its own path (`text_ctx.layout` in `paint`,
        // not the editor's retained layout), so it needs its own coverage.
        let mut shell_ctx = frust_text::TextContext::new();
        shell_ctx
            .register_fonts(TUFFY.to_vec())
            .expect("valid TTF bytes must register");

        // Empty value + never focused = the placeholder is what gets painted.
        let mut state = AppState::default();
        let mut root = field_in_family(&mut state, frust_text::FontFamily::named("Tuffy"));
        let (font, advances) = painted_shaped_run(&mut root);

        assert_same_font(&font, TUFFY, "placeholder");
        assert_uniform_advance(&advances, TUFFY_DIGIT_ADVANCE_24PX, "placeholder");
    }

    #[test]
    fn font_registered_after_build_reshapes_the_field_at_the_next_layout() {
        // The shells' *per-frame* late drain: a font can register after this
        // widget's private context was built. `layout`'s `sync_app_fonts` picks
        // it up and rebuilds the editor, whose retained parley layout would
        // otherwise stay shaped against the old faces (clearing the private
        // context's shape cache alone would not cover it).
        let mut state = AppState {
            value: DIGITS.to_string(),
            ..AppState::default()
        };
        let mut root = field_in_family(&mut state, frust_text::FontFamily::named("Helvetica"));
        let (before_font, before_advances) = painted_shaped_run(&mut root);
        assert_other_font(
            &before_font,
            TUFFY_AS_HELVETICA,
            "pre-registration control (nothing has registered this face yet)",
        );

        let mut shell_ctx = frust_text::TextContext::new();
        shell_ctx
            .register_fonts(TUFFY_AS_HELVETICA.to_vec())
            .expect("valid TTF bytes must register");

        // The shell forces LAYOUT on a `drain_into` that applied something;
        // this is that relayout.
        root.layout(Size::new(600.0, 200.0));
        let (after_font, after_advances) = painted_shaped_run(&mut root);

        // A registered family shadows any system "Helvetica" (fontique 0.11),
        // so this holds on every host.
        assert_same_font(&after_font, TUFFY_AS_HELVETICA, "late-registered");
        assert_uniform_advance(&after_advances, TUFFY_DIGIT_ADVANCE_24PX, "late-registered");
        assert_ne!(
            after_advances, before_advances,
            "the late registration must actually change the shaped metrics"
        );
    }

    // --- enabled(false) ---

    /// The standard fixture field, with `enabled`/`obscured`/`read_only`
    /// dialled in.
    fn options_logic(
        enabled: bool,
        obscured: bool,
        read_only: bool,
    ) -> impl FnMut(&mut AppState) -> TextInputView<AppState> {
        move |state: &mut AppState| {
            text_input(state.value.clone(), |s: &mut AppState, v: String| {
                s.changes += 1;
                s.value = v;
            })
            .placeholder("type here")
            .enabled(enabled)
            .obscured(obscured)
            .read_only(read_only)
        }
    }

    /// Build + lay out a root over `logic`.
    fn options_root(
        logic: &mut impl FnMut(&mut AppState) -> TextInputView<AppState>,
        state: &mut AppState,
    ) -> RenderRoot<AppState, TextInputView<AppState>> {
        let mut root = RenderRoot::new();
        root.rebuild(logic, state);
        root.layout(Size::new(300.0, 200.0));
        root
    }

    #[test]
    fn disabled_field_refuses_focus_and_stays_inert() {
        let mut state = AppState::default();
        let mut logic = options_logic(false, false, false);
        let mut root = options_root(&mut logic, &mut state);

        let outcome = root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(!outcome.handled, "a disabled field consumes nothing");
        assert!(
            !root.is_focus_active(),
            "a tap must not focus a disabled field"
        );
        assert!(!widget(&root).focused);
        assert!(!widget(&root).captured, "no drag capture either");
        assert!(root.ime_state().is_none(), "no IME surface is published");

        // Keys and IME are unreachable (they route down the focus path, which
        // the tap never claimed) and edit nothing even when injected directly.
        root.event(&mut state, &ch("x"));
        root.event(
            &mut state,
            &InputEvent::Ime(ImeEvent::Commit("ni".to_string())),
        );
        assert_eq!(widget(&root).editor.text(), "");
        assert_eq!(state.changes, 0);

        // No caret is painted, and the field is at rest.
        let mut rec = CaretRecorder {
            caret_color: Some(CARET.multiply_alpha(DISABLED_CONTENT_ALPHA)),
            caret_fills: 0,
        };
        let outcome = root.paint(&mut rec, FrameTime::ZERO);
        assert_eq!(rec.caret_fills, 0, "a disabled field paints no caret");
        assert!(!outcome.needs_frame, "a disabled field never blinks");
    }

    #[test]
    fn disabling_a_focused_field_releases_focus_and_deactivates_ime() {
        let mut state = AppState::default();
        let mut enabled_logic = options_logic(true, false, false);
        let mut root = options_root(&mut enabled_logic, &mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.is_focus_active());
        assert!(root.ime_state().expect("focused").active);

        // Flip the flag on a rebuild while the field holds focus.
        let mut disabled_logic = options_logic(false, false, false);
        let flags = root.rebuild(&mut disabled_logic, &mut state);
        assert!(
            flags.needs_layout(),
            "the disabled dim is baked at layout, so the flip must relayout"
        );
        root.layout(Size::new(300.0, 200.0));
        assert!(
            !widget(&root).focused,
            "the widget stops considering itself focused immediately"
        );

        // Leg 1 — the next paint already refuses to act focused and hands the
        // shell an inactive IME surface (dismissing the keyboard). The root reads
        // that inactive publish as a session release, so it drops the surface
        // outright (both mobile bridges serialise `None` to the same inactive
        // wire form — see `RenderRoot::ime_state`) and clears its focus mirror
        // rather than leaving it standing over a field that stopped editing.
        let mut sink = NullScene;
        let outcome = root.paint(&mut sink, FrameTime::ZERO);
        assert!(!outcome.needs_frame, "a disabled field paints at rest");
        assert!(
            root.ime_state().is_none(),
            "the published inactive surface releases the session"
        );
        assert!(
            !root.is_focus_active(),
            "the root's focus mirror goes with it"
        );

        // Leg 2 — the first event that reaches the field releases the pod-level
        // focus path, and edits nothing on the way.
        root.event(&mut state, &ch("x"));
        assert!(
            !root.is_focus_active(),
            "the stranded focus path is released"
        );
        assert!(root.ime_state().is_none());
        assert_eq!(widget(&root).editor.text(), "");
    }

    /// A design-system-shaped theme whose `on_surface_variant` role is itself
    /// **translucent** — the shape an iOS-style design system installs (its
    /// `secondaryLabel` token really is `rgba(.., 0.60)`). Built inline here
    /// because no design language ships in this crate any more. The dim under
    /// test is an alpha *multiplier* on the resolved role, so it has to behave
    /// identically over an already-translucent one — which is exactly what a
    /// token *swap* would not do.
    fn translucent_role_theme() -> Theme {
        Theme::builder(Theme::neutral())
            .design_language(frust_theme::DesignLanguage::Cupertino)
            .map_colors_light(|c| frust_theme::ColorScheme {
                on_surface_variant: c.on_surface_variant.multiply_alpha(0.6),
                ..c
            })
            .build()
    }

    #[test]
    fn disabled_dims_content_and_outline_in_every_design_language() {
        // The dim is an alpha multiplier on the *resolved* role, so it must
        // behave identically unthemed, under the neutral baseline, and under a
        // design system whose own `on_surface_variant` is already translucent
        // (see `translucent_role_theme` — the reason a token swap would not be
        // portable).
        let translucent = translucent_role_theme();
        assert!(
            translucent.scheme().on_surface_variant.components[3] < 1.0,
            "fixture sanity: the third arm's role must really be translucent"
        );
        let languages: Vec<(&str, Option<Theme>)> = vec![
            ("unthemed", None),
            ("neutral", Some(Theme::neutral())),
            ("translucent-role", Some(translucent)),
        ];
        for (name, theme) in languages {
            let enabled = Chrome::resolve(theme.as_ref(), true);
            let disabled = Chrome::resolve(theme.as_ref(), false);
            assert_eq!(
                disabled.placeholder,
                enabled.placeholder.multiply_alpha(DISABLED_CONTENT_ALPHA),
                "{name}: disabled placeholder is the enabled role at 38%"
            );
            assert_eq!(
                disabled.border,
                enabled.border.multiply_alpha(DISABLED_CONTAINER_ALPHA),
                "{name}: disabled outline is the enabled role at 12%"
            );
            assert!(
                disabled.placeholder.components[3] < enabled.placeholder.components[3],
                "{name}: the disabled placeholder must actually be more transparent"
            );
            assert_eq!(
                disabled.bg, enabled.bg,
                "{name}: the container fill stays opaque (see Chrome::resolve)"
            );
        }
    }

    #[test]
    fn disabled_dims_the_layout_baked_glyph_color() {
        // The second resolution point: the glyph color is baked into the shaped
        // editor state at LAYOUT time, so dimming has to happen there too.
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(false, false, false);
        let mut root = options_root(&mut logic, &mut state);
        root.set_theme(Box::new(Theme::neutral()));
        root.layout(Size::new(300.0, 200.0));

        let expected = Theme::neutral()
            .scheme()
            .on_surface
            .multiply_alpha(DISABLED_CONTENT_ALPHA);
        assert_eq!(
            painted_text_color(&mut root),
            expected,
            "a disabled field's glyphs are on_surface at 38%"
        );
    }

    #[test]
    fn disabled_dims_the_unthemed_fallback_glyph_color() {
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(false, false, false);
        let mut root = options_root(&mut logic, &mut state);
        assert_eq!(
            painted_text_color(&mut root),
            Color::BLACK.multiply_alpha(DISABLED_CONTENT_ALPHA),
            "with no theme threaded the black fallback dims by the same rule"
        );
    }

    // --- read_only(true) ---

    /// A read-only field over `value`, ready for events.
    fn read_only_root(
        state: &mut AppState,
        logic: &mut impl FnMut(&mut AppState) -> TextInputView<AppState>,
    ) -> RenderRoot<AppState, TextInputView<AppState>> {
        options_root(logic, state)
    }

    /// Drag-select the whole buffer of an already-built field, the way a user
    /// produces a selection — [`focused_with_selection`] without the typing a
    /// read-only field would refuse.
    fn drag_select_all(
        state: &mut AppState,
        root: &mut RenderRoot<AppState, TextInputView<AppState>>,
    ) {
        root.event(state, &pointer(PointerPhase::Down, 0.0, 10.0));
        root.event(state, &pointer(PointerPhase::Move, 290.0, 10.0));
        root.event(state, &pointer(PointerPhase::Up, 290.0, 10.0));
    }

    #[test]
    fn read_only_takes_focus_on_a_press_and_paints_focused() {
        // A read-only field is focusable so its content can be selected and
        // copied; what it withholds is editing, not the session.
        let mut state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = read_only_root(&mut state, &mut logic);

        let outcome = root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(
            outcome.handled,
            "the press is consumed, like any focus claim"
        );
        assert!(root.is_focus_active(), "a press focuses a read-only field");
        assert!(widget(&root).focused);
        assert!(widget(&root).captured, "and captures, so a drag can select");

        let rec = paint_chrome(&mut root);
        assert_eq!(
            rec.rrects[0], ACCENT,
            "a focused read-only field paints the focused accent border"
        );
        assert_eq!(rec.rects, vec![CARET], "and draws its caret");
    }

    #[test]
    fn a_read_only_caret_is_drawn_steady_and_asks_for_no_blink_frames() {
        // The blink advertises an insertion point; a field that takes no
        // insertion has none to advertise, so the caret marks where a selection
        // would start and the field otherwise sits at rest.
        let mut state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = read_only_root(&mut state, &mut logic);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));

        for ms in [0.0, BLINK_MS + 10.0, 2.0 * BLINK_MS + 10.0] {
            let mut rec = CaretRecorder {
                caret_color: Some(CARET),
                caret_fills: 0,
            };
            let outcome = root.paint(&mut rec, ft_ms(ms));
            assert_eq!(rec.caret_fills, 1, "the caret stays lit at t={ms}ms");
            assert!(
                !outcome.needs_frame,
                "no continuation frame is asked for at t={ms}ms"
            );
        }
    }

    #[test]
    fn a_focused_read_only_field_publishes_an_active_keyboard_suppressed_surface() {
        // Active so each shell's clipboard route (which hangs off the platform
        // surface) stays wired; suppressed so no on-screen keyboard comes up.
        let mut state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = read_only_root(&mut state, &mut logic);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        let ime = root.ime_state().expect("a focused read-only field has one");
        assert!(ime.active, "the surface is live, not released");
        assert!(ime.suppress_soft_keyboard);
        assert_eq!(ime.editing.text, "abc");

        // An editable field says nothing, exactly as it did before the hint.
        let mut state = AppState::default();
        let mut editable_logic = options_logic(true, false, false);
        let mut root = options_root(&mut editable_logic, &mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let ime = root.ime_state().expect("a focused editable field has one");
        assert!(ime.active);
        assert!(
            !ime.suppress_soft_keyboard,
            "an editable field wants its keyboard"
        );
    }

    #[test]
    fn read_only_copies_a_drag_selection_and_selects_all_from_the_chords() {
        let mut state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = read_only_root(&mut state, &mut logic);
        drag_select_all(&mut state, &mut root);
        assert_eq!(
            selection(&root).as_deref(),
            Some("abc"),
            "a drag selects on a read-only field"
        );

        root.event(&mut state, &chord("c", meta()));
        assert_eq!(
            root.take_clipboard_write().as_deref(),
            Some("abc"),
            "copy is the verb read-only exists to allow"
        );
        assert_eq!(state.changes, 0, "a copy is not an edit");

        // Select-all reaches the same buffer from a fresh session, where a
        // press placed a caret and selected nothing.
        let mut state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = read_only_root(&mut state, &mut logic);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(selection(&root), None, "sanity: nothing selected yet");

        root.event(&mut state, &chord("a", meta()));
        assert_eq!(selection(&root).as_deref(), Some("abc"));
        assert_eq!(state.changes, 0);
    }

    #[test]
    fn read_only_answers_cut_and_paste_with_nothing() {
        // Consumed, not ignored: the verb was understood. What it may not do is
        // change the buffer, write half a cut to the clipboard, or report an
        // edit that never happened.
        let mut state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = read_only_root(&mut state, &mut logic);
        drag_select_all(&mut state, &mut root);

        let outcome = root.event(&mut state, &chord("x", meta()));
        assert!(outcome.handled, "the cut chord is consumed");
        assert_eq!(
            root.take_clipboard_write(),
            None,
            "a refused cut writes nothing — half a cut is not a copy"
        );

        let outcome = root.event(&mut state, &edit(EditCommand::Paste("zz".to_string())));
        assert!(outcome.handled, "the paste command is consumed");

        assert_eq!(
            widget(&root).editor.text(),
            "abc",
            "the buffer is untouched"
        );
        assert_eq!(state.changes, 0, "and no on_change fires for either");
        assert_eq!(state.value, "abc");
    }

    #[test]
    fn read_only_still_refuses_typing_and_ime_commits() {
        let mut state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = read_only_root(&mut state, &mut logic);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.is_focus_active(), "refused while genuinely focused");

        for event in [
            ch("x"),
            InputEvent::Ime(ImeEvent::Commit("ni".to_string())),
            named(NamedKey::Backspace, Modifiers::default()),
            named(NamedKey::Delete, Modifiers::default()),
            named(NamedKey::Enter, Modifiers::default()),
        ] {
            let outcome = root.event(&mut state, &event);
            assert!(
                !outcome.handled,
                "an edit is refused unconsumed, as it was when the field \
                 refused focus outright: {event:?}"
            );
        }
        assert_eq!(widget(&root).editor.text(), "abc");
        assert_eq!(state.changes, 0);
        assert_eq!(state.submits, 0, "Enter submits nothing either");
    }

    #[test]
    fn escape_ends_a_read_only_session_without_touching_the_text() {
        // A field that can hold a session needs a keyboard way out of it, so
        // Escape stays answered where the editing keys do not. It ends the
        // session and takes the toolbar with it, and mutates nothing on the way.
        let mut state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = read_only_root(&mut state, &mut logic);
        root.event(
            &mut state,
            &secondary_pointer(PointerPhase::Down, 20.0, 10.0),
        );
        assert!(root.is_focus_active(), "the context press opened a session");
        assert!(widget(&root).toolbar_open, "…with the bar up");

        let outcome = root.event(&mut state, &named(NamedKey::Escape, Modifiers::default()));

        assert!(
            outcome.handled,
            "Escape is answered, not left for an ancestor"
        );
        assert!(!root.is_focus_active(), "the session is over");
        assert!(!widget(&root).focused);
        assert!(
            !widget(&root).toolbar_open,
            "the toolbar goes with the session it belonged to"
        );
        assert_eq!(widget(&root).editor.text(), "abc", "and nothing was edited");
        assert_eq!(state.changes, 0);
    }

    #[test]
    fn disabled_still_refuses_focus_where_read_only_no_longer_does() {
        // `enabled(false)` is the stronger claim and is unchanged by the
        // read-only split: no focus, no caret, no surface, nothing consumed.
        let mut state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(false, false, false);
        let mut root = options_root(&mut logic, &mut state);

        let outcome = root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(!outcome.handled, "a disabled field consumes nothing");
        assert!(!root.is_focus_active(), "and takes no focus");
        assert!(root.ime_state().is_none(), "so it publishes no surface");

        let mut rec = CaretRecorder {
            caret_color: Some(CARET),
            caret_fills: 0,
        };
        let outcome = root.paint(&mut rec, FrameTime::ZERO);
        assert_eq!(rec.caret_fills, 0, "a disabled field paints no caret");
        assert!(!outcome.needs_frame);
    }

    #[test]
    fn making_a_focused_field_read_only_keeps_the_session_and_drops_the_keyboard() {
        // Unlike a field disabled while focused, this one keeps its session:
        // the text is still on screen and still worth selecting. What goes is
        // the on-screen keyboard, asked for on the very next published surface.
        let mut state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut live_logic = options_logic(true, false, false);
        let mut root = options_root(&mut live_logic, &mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(root.is_focus_active());
        let ime = root.ime_state().expect("focused");
        assert!(ime.active && !ime.suppress_soft_keyboard);

        // Flip the flag on a rebuild while the field holds focus.
        let mut read_only_logic = options_logic(true, false, true);
        let flags = root.rebuild(&mut read_only_logic, &mut state);
        assert!(
            !flags.needs_layout(),
            "read-only never dims, so the flip is PAINT-only, unlike disabled"
        );
        root.layout(Size::new(300.0, 200.0));
        assert!(
            widget(&root).focused,
            "the widget keeps considering itself focused"
        );

        // Leg 1 — the next paint keeps the session and re-publishes it with the
        // keyboard suppressed; the caret is still drawn, now steady.
        let mut rec = CaretRecorder {
            caret_color: Some(CARET),
            caret_fills: 0,
        };
        root.paint(&mut rec, FrameTime::ZERO);
        assert_eq!(rec.caret_fills, 1, "the caret survives the flip");
        let ime = root.ime_state().expect("the session survives the flip");
        assert!(ime.active, "the surface stays live for the clipboard route");
        assert!(ime.suppress_soft_keyboard, "but the keyboard is asked down");
        assert!(root.is_focus_active(), "the root's focus mirror stays");

        // Leg 2 — and the keyboard events it can no longer act on are refused
        // without disturbing any of that.
        root.event(&mut state, &ch("x"));
        assert!(root.is_focus_active(), "a refused key does not blur");
        assert_eq!(widget(&root).editor.text(), "abc");
        assert_eq!(state.changes, 0);
    }

    #[test]
    fn read_only_paints_full_alpha_chrome_in_every_design_language() {
        // The first of the two dimming resolution points: `Chrome::resolve`,
        // observed here through the actual `paint` pass (not called directly),
        // so the assertion also proves `paint` feeds it `enabled` alone.
        let languages: Vec<(&str, Option<Theme>)> = vec![
            ("unthemed", None),
            ("neutral", Some(Theme::neutral())),
            ("translucent-role", Some(translucent_role_theme())),
        ];
        for (name, theme) in languages {
            let mut state = AppState::default();
            let mut logic = options_logic(true, false, true);
            let mut root = options_root(&mut logic, &mut state);
            if let Some(theme) = theme.clone() {
                root.set_theme(Box::new(theme));
            }
            let live_border = Chrome::resolve(theme.as_ref(), true).border;
            let rec = paint_chrome(&mut root);
            assert_eq!(
                rec.rrects[0], live_border,
                "{name}: a read-only field's idle border matches the fully-\
                 enabled resolved border — not `DISABLED_CONTAINER_ALPHA`-dimmed"
            );
        }
    }

    #[test]
    fn read_only_paints_full_alpha_layout_baked_glyph_color() {
        // The second of the two dimming resolution points:
        // `effective_style`'s LAYOUT-time bake.
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = options_root(&mut logic, &mut state);
        root.set_theme(Box::new(Theme::neutral()));
        root.layout(Size::new(300.0, 200.0));

        assert_eq!(
            painted_text_color(&mut root),
            Theme::neutral().scheme().on_surface,
            "a read-only field's glyphs stay full-alpha on_surface, not dimmed"
        );
    }

    #[test]
    fn read_only_paints_full_alpha_unthemed_fallback_glyph_color() {
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = options_root(&mut logic, &mut state);
        assert_eq!(
            painted_text_color(&mut root),
            Color::BLACK,
            "with no theme threaded, read-only stays the undimmed black fallback"
        );
    }

    #[test]
    fn switching_a_read_only_field_to_live_shows_no_alpha_change() {
        // The motivating case: a static mock rendered read-only then switched
        // interactive at a live handoff must show **no** alpha change at
        // either resolution point — the pop `enabled(false)` would have
        // produced.
        let mut state = AppState {
            value: "hi".to_string(),
            ..AppState::default()
        };
        let mut read_only_logic = options_logic(true, false, true);
        let mut root = options_root(&mut read_only_logic, &mut state);
        let before_glyph = painted_text_color(&mut root);
        let before_border = paint_chrome(&mut root).rrects[0];

        let mut live_logic = options_logic(true, false, false);
        root.rebuild(&mut live_logic, &mut state);
        root.layout(Size::new(300.0, 200.0));
        let after_glyph = painted_text_color(&mut root);
        let after_border = paint_chrome(&mut root).rrects[0];

        assert_eq!(
            before_glyph, after_glyph,
            "glyph color must not change across the read-only -> live handoff"
        );
        assert_eq!(
            before_border, after_border,
            "border color must not change across the read-only -> live handoff"
        );
        assert_eq!(
            before_glyph,
            Color::BLACK,
            "sanity: read-only starts undimmed"
        );
    }

    /// The clipboard verbs a field's own semantics node currently offers, as
    /// `(id, label)` pairs in publication order.
    fn a11y_verbs(
        root: &RenderRoot<AppState, TextInputView<AppState>>,
    ) -> (bool, Vec<(i32, String)>) {
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| matches!(n.role(), Role::TextInput | Role::PasswordInput))
            .expect("a text field node");
        (
            node.supports_action(Action::CustomAction),
            node.custom_actions()
                .iter()
                .map(|a| (a.id, a.description.to_string()))
                .collect(),
        )
    }

    #[test]
    fn the_clipboard_verbs_ride_the_field_s_own_node_with_the_toolbar_closed() {
        // The accessible route must not depend on the floating toolbar, which
        // is a pointer affordance and contributes no semantics of its own.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        focused_with_selection(&mut state, &mut root, "abc");
        assert!(
            !widget(&root).toolbar_open,
            "sanity: a drag-select raises no bar, so this is the closed case"
        );

        let (supports, offered) = a11y_verbs(&root);
        assert!(
            supports,
            "the field advertises that it takes custom actions at all"
        );
        assert_eq!(
            offered,
            vec![
                (A11Y_CUT_ID, "Cut".to_string()),
                (A11Y_COPY_ID, "Copy".to_string()),
                (A11Y_PASTE_ID, "Paste".to_string()),
            ],
            "an interactive field with all of its text selected offers cut/copy/\
             paste, and no select-all — exactly the bar's own enabled set"
        );

        // Invoking one reaches `handle_command`: resolve the advertised id the
        // way a dispatcher would, deliver it as the `EditCommand` a shell
        // already sends for a platform edit menu, and watch the verb land.
        let copy_id = offered
            .iter()
            .find(|(_, label)| label == "Copy")
            .expect("copy was offered")
            .0;
        let cmd = match copy_id {
            A11Y_CUT_ID => EditCommand::Cut,
            A11Y_COPY_ID => EditCommand::Copy,
            A11Y_SELECT_ALL_ID => EditCommand::SelectAll,
            other => panic!("the published id {other} resolves to no verb"),
        };
        root.event(&mut state, &edit(cmd));
        assert_eq!(
            root.take_clipboard_write().as_deref(),
            Some("abc"),
            "the advertised id resolves to a verb that reaches handle_command"
        );
    }

    #[test]
    fn the_published_verbs_track_the_field_s_own_refusals() {
        // Empty and unfocused: nothing to copy or select, but a paste would
        // land, so paste alone is offered.
        let mut state = AppState::default();
        let mut logic = options_logic(true, false, false);
        let root = options_root(&mut logic, &mut state);
        assert_eq!(
            a11y_verbs(&root).1,
            vec![(A11Y_PASTE_ID, "Paste".to_string())],
            "an empty field offers only paste"
        );

        // Obscured: the mirror is no more handable than the real buffer, so a
        // selection buys neither copy nor cut.
        let mut state = AppState {
            value: "hunter2".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, true, false);
        let mut root = options_root(&mut logic, &mut state);
        focused_with_selection(&mut state, &mut root, "");
        assert!(
            selection(&root).is_some(),
            "sanity: the refusal below is only meaningful over a real selection"
        );
        assert_eq!(
            a11y_verbs(&root).1,
            vec![(A11Y_PASTE_ID, "Paste".to_string())],
            "an obscured field hands out neither the buffer nor its bullets"
        );

        // Read-only: copy is fine over a selection, cut and paste are not —
        // the field is focusable and copyable, so the accessible route carries
        // exactly the verbs it will actually answer.
        let mut state = AppState {
            value: "abc".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, true);
        let mut root = options_root(&mut logic, &mut state);
        drag_select_all(&mut state, &mut root);
        assert_eq!(
            a11y_verbs(&root).1,
            vec![(A11Y_COPY_ID, "Copy".to_string())],
            "a read-only field offers copy over its selection, never cut or paste"
        );

        // Disabled: same reasoning, and the stronger claim.
        let mut state = AppState {
            value: "abc".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(false, false, false);
        let root = options_root(&mut logic, &mut state);
        assert!(
            a11y_verbs(&root).1.is_empty(),
            "a disabled field advertises no verbs either"
        );
    }

    #[test]
    fn read_only_reports_read_only_not_disabled_semantics() {
        let mut state = AppState::default();
        let mut logic = options_logic(true, false, true);
        let root = options_root(&mut logic, &mut state);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TextInput)
            .expect("a text field node");
        assert!(
            node.is_read_only(),
            "a read-only field says so to a11y, distinct from disabled"
        );
        assert!(
            !node.is_disabled(),
            "read-only is not the same claim as disabled"
        );
    }

    #[test]
    fn disabled_wins_semantics_over_read_only_when_both_are_set() {
        let mut state = AppState::default();
        let mut logic = options_logic(false, false, true);
        let root = options_root(&mut logic, &mut state);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TextInput)
            .expect("a text field node");
        assert!(node.is_disabled(), "disabled is the stronger claim");
        assert!(
            !node.is_read_only(),
            "disabled and read-only are never both reported"
        );
    }

    #[test]
    fn read_only_obscured_field_stays_password_role_and_copies_nothing() {
        // Orthogonality: `read_only` never touches masking or the IME
        // content-type hint. The two refusals compose rather than cancel — the
        // field focuses like any read-only one, and hands out neither the
        // secret nor its bullets.
        let mut state = AppState {
            value: "hunter2".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, true, true);
        let mut root = options_root(&mut logic, &mut state);

        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::PasswordInput)
            .expect("read-only + obscured still contributes Role::PasswordInput");
        assert_eq!(node.value(), Some("\u{2022}".repeat(7).as_str()));
        assert!(node.is_read_only());

        drag_select_all(&mut state, &mut root);
        assert!(
            root.is_focus_active(),
            "it focuses like any read-only field"
        );
        assert!(
            selection(&root).is_some(),
            "sanity: the refusal below is only meaningful over a real selection"
        );

        let ime = root.ime_state().expect("focused");
        assert_eq!(
            ime.content_type,
            ImeContentType::Password,
            "the content-type hint is read-only's business to leave alone"
        );
        assert!(ime.suppress_soft_keyboard);

        root.event(&mut state, &chord("c", meta()));
        assert_eq!(
            root.take_clipboard_write(),
            None,
            "an obscured field copies nothing, read-only or not"
        );
        assert!(
            a11y_verbs(&root).1.is_empty(),
            "and advertises no verb it would refuse"
        );
    }

    // --- obscured(true) ---

    #[test]
    fn mask_offsets_map_1_to_1_per_char_across_a_multi_byte_grapheme() {
        // "a😀b": 1 + 4 + 1 real bytes; masked "•••" is 3 × 3 bytes.
        let text = "a\u{1F600}b";
        assert_eq!(mask_text(text), "\u{2022}\u{2022}\u{2022}");
        for (real, masked) in [(0, 0), (1, 3), (5, 6), (6, 9)] {
            assert_eq!(real_to_masked(text, real), masked, "real {real} -> masked");
            assert_eq!(
                masked_to_real(text, masked),
                real,
                "masked {masked} -> real"
            );
        }
        // Interior offsets snap back to the enclosing character's start, both
        // ways (the emoji spans real bytes 1..5 and masked bytes 3..6).
        assert_eq!(real_to_masked(text, 3), 3, "mid-emoji snaps to its start");
        assert_eq!(masked_to_real(text, 4), 1, "mid-mask snaps to its start");
        // A newline is preserved so a multi-line field keeps its line count.
        assert_eq!(mask_text("a\nb"), "\u{2022}\n\u{2022}");
    }

    #[test]
    fn obscured_paints_bullets_and_keeps_the_real_value() {
        /// Records the glyph ids of every painted run, in order.
        #[derive(Default)]
        struct GlyphIdRecorder {
            ids: Vec<u16>,
        }
        impl PaintScene for GlyphIdRecorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn draw_glyph_run(&mut self, run: frust_scene::GlyphRun) {
                self.ids.extend(run.glyphs.iter().map(|g| g.id as u16));
            }
        }

        fn ids(root: &mut RenderRoot<AppState, TextInputView<AppState>>) -> Vec<u16> {
            let mut rec = GlyphIdRecorder::default();
            root.paint(&mut rec, FrameTime::ZERO);
            rec.ids
        }

        // An obscured field holding "abc" must paint exactly what a plain field
        // holding "•••" paints — and nothing of what "abc" paints.
        let mut secret_state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut secret_logic = options_logic(true, true, false);
        let mut secret = options_root(&mut secret_logic, &mut secret_state);

        let mut bullets_state = AppState {
            value: "\u{2022}\u{2022}\u{2022}".to_string(),
            ..AppState::default()
        };
        let mut plain_logic = options_logic(true, false, false);
        let mut bullets = options_root(&mut plain_logic, &mut bullets_state);

        let mut clear_state = AppState {
            value: "abc".to_string(),
            ..AppState::default()
        };
        let mut clear = options_root(&mut plain_logic, &mut clear_state);

        assert_eq!(
            ids(&mut secret),
            ids(&mut bullets),
            "an obscured field paints bullets"
        );
        assert_ne!(
            ids(&mut secret),
            ids(&mut clear),
            "…and not the real characters"
        );
        assert_eq!(
            widget(&secret).editor.text(),
            "abc",
            "the model text is untouched by masking"
        );
    }

    #[test]
    fn obscured_measures_from_the_masked_text_not_the_real_one() {
        // Masking at glyph-emission time only would leave the field measured
        // from the real text: an 'i'-heavy secret would size like 'i's while
        // painting bullets. The mirror is what gets measured, so an obscured
        // field's width matches the equivalent bullet string's.
        let mut secret_state = AppState {
            value: "iiiiiiiiii".to_string(),
            ..AppState::default()
        };
        let mut secret_logic = options_logic(true, true, false);
        let secret = options_root(&mut secret_logic, &mut secret_state);

        let mut bullets_state = AppState {
            value: "\u{2022}".repeat(10),
            ..AppState::default()
        };
        let mut plain_logic = options_logic(true, false, false);
        let bullets = options_root(&mut plain_logic, &mut bullets_state);

        let masked_w = widget(&secret).display().layout_size().width;
        let bullets_w = widget(&bullets).display().layout_size().width;
        let real_w = widget(&secret).editor.layout_size().width;
        assert_eq!(masked_w, bullets_w, "measured from the masked mirror");
        assert!(
            masked_w > real_w,
            "sanity: bullets are wider than 'i's ({masked_w} vs {real_w})"
        );
    }

    #[test]
    fn obscured_editing_matches_unobscured_across_a_multi_byte_grapheme() {
        // The masking must not disturb the editing arithmetic: run the same
        // key sequence on an obscured and a clear field and require identical
        // editing state at every step, including over a 4-byte emoji.
        let mut secret_state = AppState::default();
        let mut secret_logic = options_logic(true, true, false);
        let mut secret = options_root(&mut secret_logic, &mut secret_state);
        let mut clear_state = AppState::default();
        let mut clear_logic = options_logic(true, false, false);
        let mut clear = options_root(&mut clear_logic, &mut clear_state);

        let plain = Modifiers::default();
        let meta = Modifiers {
            meta: true,
            ..Modifiers::default()
        };
        let events = vec![
            pointer(PointerPhase::Down, 10.0, 10.0),
            ch("a"),
            ch("\u{1F600}"),
            ch("b"),
            named(NamedKey::ArrowLeft, plain),
            named(NamedKey::Backspace, plain),
            named(NamedKey::End, plain),
            named(NamedKey::Backspace, plain),
            InputEvent::Key(KeyEvent {
                key: Key::Character("a".to_string()),
                modifiers: meta,
                repeat: false,
            }),
        ];
        for (i, event) in events.iter().enumerate() {
            secret.event(&mut secret_state, event);
            clear.event(&mut clear_state, event);
            assert_eq!(
                widget(&secret).editor.editing_state_bytes(),
                widget(&clear).editor.editing_state_bytes(),
                "editing state diverged at step {i}"
            );
        }
        // The emoji was deleted as one grapheme in both, and the app saw the
        // real text throughout.
        assert_eq!(secret_state.value, "a");
        assert_eq!(secret_state.value, clear_state.value);
        assert_eq!(secret_state.changes, clear_state.changes);
    }

    #[test]
    fn obscured_caret_rect_follows_the_masked_layout() {
        // The caret must sit where the *bullets* end, not where the real text
        // would have ended.
        let mut secret_state = AppState {
            value: "iiii".to_string(),
            ..AppState::default()
        };
        let mut secret_logic = options_logic(true, true, false);
        let mut secret = options_root(&mut secret_logic, &mut secret_state);
        secret.event(&mut secret_state, &pointer(PointerPhase::Down, 290.0, 10.0));
        let masked_caret = secret
            .ime_state()
            .expect("focused")
            .caret
            .expect("a caret rect");

        let mut bullets_state = AppState {
            value: "\u{2022}".repeat(4),
            ..AppState::default()
        };
        let mut plain_logic = options_logic(true, false, false);
        let mut bullets = options_root(&mut plain_logic, &mut bullets_state);
        bullets.event(
            &mut bullets_state,
            &pointer(PointerPhase::Down, 290.0, 10.0),
        );
        let bullets_caret = bullets
            .ime_state()
            .expect("focused")
            .caret
            .expect("a caret rect");

        assert_eq!(
            masked_caret, bullets_caret,
            "the obscured caret tracks the masked glyphs"
        );
        assert_eq!(
            secret.ime_state().expect("focused").editing.text,
            "iiii",
            "the IME surface still carries the real text (see the module docs)"
        );
    }

    #[test]
    fn obscured_tap_places_the_caret_from_the_masked_hit_test() {
        // A tap between the first and second bullet must land on real byte 1,
        // resolved against the masked advances (bullets are much wider than
        // 'i's, so hit-testing the real layout would overshoot to the end).
        let mut state = AppState {
            value: "iiiiiiii".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, true, false);
        let mut root = options_root(&mut logic, &mut state);
        // Mid-way through the first mask glyph -> caret before or after char 0.
        let half_bullet = widget(&root).display().layout_size().width / 16.0;
        root.event(
            &mut state,
            &pointer(PointerPhase::Down, PAD_X + half_bullet, 10.0),
        );
        let extent = widget(&root).editor.editing_state_bytes().extent;
        assert!(
            extent <= 1,
            "a tap inside the first mask glyph lands at byte 0 or 1, got {extent}"
        );
    }

    #[test]
    fn obscured_reports_password_role_with_a_masked_value() {
        let mut state = AppState {
            value: "hunter2".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, true, false);
        let root = options_root(&mut logic, &mut state);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::PasswordInput)
            .expect("an obscured field contributes a Role::PasswordInput node");
        assert_eq!(
            node.value(),
            Some("\u{2022}".repeat(7).as_str()),
            "the a11y value is masked too — a client reads it verbatim"
        );

        // Enabled + clear stays a plain TextInput node carrying the real value.
        let mut plain_logic = options_logic(true, false, false);
        let plain = options_root(&mut plain_logic, &mut state);
        let update = plain.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TextInput)
            .expect("a clear field stays Role::TextInput");
        assert_eq!(node.value(), Some("hunter2"));
        assert!(!node.is_disabled(), "an enabled field is not disabled");
    }

    #[test]
    fn disabled_reports_disabled_semantics() {
        let mut state = AppState::default();
        let mut logic = options_logic(false, false, false);
        let root = options_root(&mut logic, &mut state);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TextInput)
            .expect("a text field node");
        assert!(node.is_disabled(), "a disabled field says so to a11y");
    }

    #[test]
    fn toggling_obscured_relayouts_and_keeps_the_value() {
        let mut state = AppState {
            value: "iiiiiiii".to_string(),
            ..AppState::default()
        };
        let mut clear_logic = options_logic(true, false, false);
        let mut root = options_root(&mut clear_logic, &mut state);
        let clear_width = widget(&root).display().layout_size().width;

        let mut secret_logic = options_logic(true, true, false);
        let flags = root.rebuild(&mut secret_logic, &mut state);
        assert!(
            flags.needs_layout(),
            "masking changes the measured text, so the flip must relayout"
        );
        root.layout(Size::new(300.0, 200.0));
        assert!(
            widget(&root).display().layout_size().width > clear_width,
            "the masked mirror is measured after the flip"
        );
        assert_eq!(widget(&root).editor.text(), "iiiiiiii");

        // …and back again.
        root.rebuild(&mut clear_logic, &mut state);
        root.layout(Size::new(300.0, 200.0));
        assert_eq!(widget(&root).display().layout_size().width, clear_width);
        assert!(widget(&root).mask_editor.is_none());
    }

    // --- content_type (widget half) ---
    //
    // These lock the widget's IME content-type mapping only — the leak this
    // closes lives at the platform seam, and no widget-level test can observe
    // whether a shell actually honours the hint. See the module docs' scope
    // boundary and `ImeContentType`'s own docs.

    #[test]
    fn obscured_publishes_password_content_type_across_focus_edit_and_refocus() {
        let mut state = AppState {
            value: "hunter2".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, true, false);
        let mut root = options_root(&mut logic, &mut state);

        // Focus.
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert_eq!(
            root.ime_state().expect("focused").content_type,
            ImeContentType::Password,
            "an obscured field's very first published IME surface must already \
             be Password"
        );

        // Edit.
        root.event(&mut state, &ch("x"));
        assert_eq!(
            root.ime_state().expect("still focused").content_type,
            ImeContentType::Password,
            "content type must not drop on edit"
        );

        // Blur, then re-focus.
        root.event(&mut state, &named(NamedKey::Escape, Modifiers::default()));
        assert!(root.ime_state().is_none(), "blur unpublishes the surface");
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert_eq!(
            root.ime_state().expect("re-focused").content_type,
            ImeContentType::Password,
            "content type must be correct again on re-focus, not just the first time"
        );
    }

    #[test]
    fn unobscured_field_publishes_the_default_no_hint_content_type() {
        // No behaviour change for the common case: a plain field's published
        // surface keeps the `Normal` default it always had.
        let mut state = AppState::default();
        let mut root = harness(&mut state);

        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert_eq!(
            root.ime_state().expect("focused").content_type,
            ImeContentType::Normal
        );

        root.event(&mut state, &ch("h"));
        assert_eq!(
            root.ime_state().expect("still focused").content_type,
            ImeContentType::Normal
        );
    }

    #[test]
    fn obscured_content_type_is_correct_on_the_first_publication_after_focus() {
        // The ordering guarantee the security fix rests on: a field that starts
        // `Normal` and flips to `Password` a frame later has already leaked to
        // the platform IME (see the module docs). Assert there is no such
        // window by checking the *very first* `ImeState` a freshly built,
        // never-before-focused obscured field emits — a single `Down` event,
        // nothing before it.
        let mut state = AppState {
            value: "hunter2".to_string(),
            ..AppState::default()
        };
        let mut logic = options_logic(true, true, false);
        let mut root = options_root(&mut logic, &mut state);
        assert!(
            root.ime_state().is_none(),
            "an unfocused field publishes nothing yet"
        );

        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        let ime = root
            .ime_state()
            .expect("focusing publishes the first IME surface");
        assert_eq!(
            ime.content_type,
            ImeContentType::Password,
            "the first-ever publication for an obscured field must already \
             carry Password"
        );
        assert_eq!(
            ime.editing.text, "hunter2",
            "sanity: this is the real first publication, not a stale one"
        );
    }

    // --- Clipboard and selection commands ---

    /// A decoded clipboard verb, focus-routed like a key — the same event
    /// `frust-testing`'s `edit_command` builds, spelled locally because
    /// `frust-widgets` takes no edge on that crate.
    fn edit(cmd: EditCommand) -> InputEvent {
        InputEvent::EditCommand(cmd)
    }

    /// Focus the field, type `text`, then drag-select the whole buffer — the
    /// selection a copy/cut acts on, produced the way a user produces it.
    fn focused_with_selection(
        state: &mut AppState,
        root: &mut RenderRoot<AppState, TextInputView<AppState>>,
        text: &str,
    ) {
        root.event(state, &pointer(PointerPhase::Down, 10.0, 10.0));
        for c in text.chars() {
            root.event(state, &ch(&c.to_string()));
        }
        // Press at the left edge (before the first glyph), drag past the last.
        root.event(state, &pointer(PointerPhase::Down, 0.0, 10.0));
        root.event(state, &pointer(PointerPhase::Move, 290.0, 10.0));
        root.event(state, &pointer(PointerPhase::Up, 290.0, 10.0));
    }

    /// The ctrl/meta chord modifier the clipboard shortcuts key off.
    fn meta() -> Modifiers {
        Modifiers {
            meta: true,
            ..Modifiers::default()
        }
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    fn chord(text: &str, modifiers: Modifiers) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character(text.to_string()),
            modifiers,
            repeat: false,
        })
    }

    #[test]
    fn copy_writes_the_selection_and_edits_nothing() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        focused_with_selection(&mut state, &mut root, "abc");
        assert_eq!(
            widget(&root).editor.selected_text(),
            Some("abc"),
            "the drag selected the whole buffer"
        );
        let changes = state.changes;

        root.event(&mut state, &edit(EditCommand::Copy));

        assert_eq!(root.take_clipboard_write().as_deref(), Some("abc"));
        assert_eq!(widget(&root).editor.text(), "abc", "a copy mutates nothing");
        assert_eq!(state.changes, changes, "a copy is not an edit");
        assert_eq!(state.value, "abc");
    }

    #[test]
    fn cut_writes_the_selection_removes_it_and_reports_one_change() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        focused_with_selection(&mut state, &mut root, "abc");
        let changes = state.changes;

        root.event(&mut state, &edit(EditCommand::Cut));

        assert_eq!(root.take_clipboard_write().as_deref(), Some("abc"));
        assert_eq!(widget(&root).editor.text(), "", "the selection is gone");
        assert_eq!(state.changes, changes + 1, "one on_change, not two");
        assert_eq!(state.value, "");
    }

    #[test]
    fn copy_and_cut_do_nothing_without_a_selection() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        for c in ["a", "b"] {
            root.event(&mut state, &ch(c));
        }
        let changes = state.changes;

        root.event(&mut state, &edit(EditCommand::Copy));
        assert!(root.take_clipboard_write().is_none());
        // A collapsed cut must not fall back to deleting the grapheme its
        // `Backdelete` would otherwise take.
        root.event(&mut state, &edit(EditCommand::Cut));
        assert!(root.take_clipboard_write().is_none());

        assert_eq!(widget(&root).editor.text(), "ab");
        assert_eq!(state.changes, changes, "neither verb fired on_change");
    }

    #[test]
    fn paste_into_a_single_line_field_strips_newlines_and_replaces_the_selection() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        focused_with_selection(&mut state, &mut root, "xy");
        let changes = state.changes;

        root.event(&mut state, &edit(EditCommand::Paste("a\nb".to_string())));

        assert_eq!(
            widget(&root).editor.text(),
            "ab",
            "the newline is denied and the selection replaced"
        );
        assert_eq!(state.changes, changes + 1, "one edit, one on_change");
        assert_eq!(state.value, "ab");
    }

    #[test]
    fn paste_into_a_multiline_field_keeps_the_newline() {
        let mut state = AppState::default();
        let mut root = RenderRoot::new();
        root.rebuild(&mut multiline_logic, &mut state);
        root.layout(Size::new(300.0, 200.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));

        root.event(&mut state, &edit(EditCommand::Paste("a\nb".to_string())));

        assert_eq!(widget(&root).editor.text(), "a\nb");
        assert_eq!(state.changes, 1);
    }

    #[test]
    fn a_paste_sanitised_to_nothing_changes_nothing() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("a"));
        let changes = state.changes;

        let outcome = root.event(&mut state, &edit(EditCommand::Paste("\n".to_string())));

        assert!(outcome.handled, "the verb was understood, and answered");
        assert_eq!(widget(&root).editor.text(), "a", "nothing was inserted");
        assert_eq!(state.changes, changes, "an empty paste is not an edit");
    }

    #[test]
    fn select_all_via_the_command_selects_the_whole_buffer_without_typing() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        for c in ["a", "b", "c"] {
            root.event(&mut state, &ch(c));
        }
        let changes = state.changes;

        root.event(&mut state, &edit(EditCommand::SelectAll));

        assert_eq!(widget(&root).editor.selected_text(), Some("abc"));
        assert_eq!(state.changes, changes);
        assert_eq!(widget(&root).editor.text(), "abc");
    }

    #[test]
    fn an_obscured_field_refuses_copy_and_cut_but_still_pastes() {
        let mut state = AppState::default();
        let mut logic = options_logic(true, true, false);
        let mut root = options_root(&mut logic, &mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        for c in ["a", "b", "c"] {
            root.event(&mut state, &ch(c));
        }
        root.event(&mut state, &edit(EditCommand::SelectAll));
        assert_eq!(widget(&root).editor.selected_text(), Some("abc"));
        let changes = state.changes;

        // Neither the real buffer nor the bullet mirror may reach the clipboard.
        root.event(&mut state, &edit(EditCommand::Copy));
        assert!(root.take_clipboard_write().is_none());
        root.event(&mut state, &edit(EditCommand::Cut));
        assert!(root.take_clipboard_write().is_none());
        assert_eq!(
            widget(&root).editor.text(),
            "abc",
            "a refused cut deletes nothing"
        );
        assert_eq!(state.changes, changes);

        // Writing *into* a password field is ordinary: the paste still lands,
        // replacing the (still intact) selection.
        root.event(&mut state, &edit(EditCommand::Paste("zz".to_string())));
        assert_eq!(widget(&root).editor.text(), "zz");
        assert_eq!(state.changes, changes + 1);
    }

    #[test]
    fn meta_c_copies_and_meta_x_cuts() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        focused_with_selection(&mut state, &mut root, "abc");

        root.event(&mut state, &chord("c", meta()));
        assert_eq!(root.take_clipboard_write().as_deref(), Some("abc"));
        assert_eq!(widget(&root).editor.text(), "abc");

        root.event(&mut state, &chord("X", meta()));
        assert_eq!(
            root.take_clipboard_write().as_deref(),
            Some("abc"),
            "the chord is ASCII case-insensitive"
        );
        assert_eq!(widget(&root).editor.text(), "");
    }

    #[test]
    fn ctrl_v_and_shift_insert_request_a_paste_and_type_nothing() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert!(!root.take_paste_request());

        root.event(&mut state, &chord("v", ctrl()));
        assert!(root.take_paste_request(), "Ctrl+V asks the shell to read");
        assert_eq!(widget(&root).editor.text(), "", "and types no 'v'");

        root.event(&mut state, &named(NamedKey::Insert, shift()));
        assert!(
            root.take_paste_request(),
            "Shift+Insert is the legacy paste"
        );

        root.event(&mut state, &named(NamedKey::Paste, Modifiers::default()));
        assert!(root.take_paste_request(), "so is the hardware Paste key");

        assert_eq!(state.changes, 0, "asking for a paste is not an edit");
    }

    #[test]
    fn ctrl_insert_copies_and_shift_delete_cuts() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        focused_with_selection(&mut state, &mut root, "abc");

        root.event(&mut state, &named(NamedKey::Insert, ctrl()));
        assert_eq!(root.take_clipboard_write().as_deref(), Some("abc"));
        assert_eq!(widget(&root).editor.text(), "abc");

        root.event(&mut state, &named(NamedKey::Delete, shift()));
        assert_eq!(root.take_clipboard_write().as_deref(), Some("abc"));
        assert_eq!(widget(&root).editor.text(), "", "Shift+Delete is a cut");
    }

    #[test]
    fn ctrl_shift_delete_stays_a_forward_delete() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        for c in ["a", "b"] {
            root.event(&mut state, &ch(c));
        }
        // Caret home, then Ctrl+Shift+Delete: an OS-level gesture, never a cut.
        root.event(&mut state, &named(NamedKey::Home, Modifiers::default()));
        root.event(
            &mut state,
            &named(
                NamedKey::Delete,
                Modifiers {
                    shift: true,
                    ctrl: true,
                    ..Modifiers::default()
                },
            ),
        );

        assert!(root.take_clipboard_write().is_none(), "nothing was copied");
        assert_eq!(widget(&root).editor.text(), "b", "the grapheme ahead went");
    }

    #[test]
    fn the_hardware_copy_and_cut_keys_resolve_to_the_same_verbs() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        focused_with_selection(&mut state, &mut root, "abc");

        root.event(&mut state, &named(NamedKey::Copy, Modifiers::default()));
        assert_eq!(root.take_clipboard_write().as_deref(), Some("abc"));
        assert_eq!(widget(&root).editor.text(), "abc");

        root.event(&mut state, &named(NamedKey::Cut, Modifiers::default()));
        assert_eq!(root.take_clipboard_write().as_deref(), Some("abc"));
        assert_eq!(widget(&root).editor.text(), "");
    }

    #[test]
    fn a_bare_insert_edits_nothing_and_is_left_unconsumed() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("a"));

        let outcome = root.event(&mut state, &named(NamedKey::Insert, Modifiers::default()));

        assert!(!outcome.handled, "no overtype mode to toggle: not ours");
        assert!(!root.take_paste_request());
        assert_eq!(widget(&root).editor.text(), "a");
    }

    #[test]
    fn an_unrecognized_chord_is_still_consumed_and_never_typed() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut state, &ch("a"));
        let changes = state.changes;

        let outcome = root.event(&mut state, &chord("z", meta()));

        assert!(
            outcome.handled,
            "an undecoded chord is swallowed, not typed"
        );
        assert_eq!(widget(&root).editor.text(), "a", "no 'z' was inserted");
        assert_eq!(state.changes, changes);
        assert!(root.take_clipboard_write().is_none());
        assert!(!root.take_paste_request());
    }

    #[test]
    fn an_unfocused_field_ignores_every_edit_command() {
        let mut state = AppState::default();
        let mut root = harness(&mut state);

        for cmd in [
            EditCommand::SelectAll,
            EditCommand::Paste("hi".to_string()),
            EditCommand::Copy,
            EditCommand::Cut,
        ] {
            let outcome = root.event(&mut state, &edit(cmd));
            assert!(!outcome.handled, "a focus-routed verb reaches nobody");
        }

        assert!(root.take_clipboard_write().is_none());
        assert_eq!(widget(&root).editor.text(), "");
        assert_eq!(state.changes, 0);
        assert!(!widget(&root).focused);
    }

    // -----------------------------------------------------------------------
    // Selection gestures and the floated toolbar
    // -----------------------------------------------------------------------

    /// Serialises every test that writes the process-global selection-toolbar
    /// slots. Rust runs a crate's tests in parallel threads sharing one process,
    /// so two of them installing a builder would see each other's writes —
    /// `frust_core::selection_toolbar`'s own tests keep the identical lock for
    /// the identical reason.
    static TOOLBAR_LOCK: Mutex<()> = Mutex::new(());

    /// What the toolbar double recorded. `Arc<Mutex<_>>` rather than the
    /// `Rc<RefCell<_>>` a pod fixture would normally use, because a
    /// [`SelectionToolbarBuilder`] is a process-global `Send + Sync` closure.
    type ToolbarLog = Arc<Mutex<Vec<String>>>;

    /// The double's fixed size, so a placement assertion has a rect to expect.
    const PROBE_SIZE: Size = Size::new(120.0, 40.0);
    /// The colour the double fills itself with — how a scene log tells the
    /// floated pod's paint apart from the field's own.
    const PROBE_COLOR: Color = Color::from_rgb8(0x11, 0x22, 0x33);
    /// The window every toolbar test lays out in (the `harness` window).
    const TOOLBAR_WINDOW: Size = Size::new(300.0, 200.0);

    /// A recording paint sink that keeps colours and shapes apart, so a test can
    /// ask both "was the pod painted?" and "where?".
    #[derive(Default)]
    struct SceneLog {
        fills: Vec<(Point, Size, Color)>,
        rounded: Vec<(Point, Size)>,
    }

    impl PaintScene for SceneLog {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.fills.push((o, s, c));
        }
        fn fill_rounded_rect(&mut self, o: Point, s: Size, _r: f64, _c: Color) {
            self.rounded.push((o, s));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    impl SceneLog {
        /// The pod's own fill, if it was painted at all.
        fn probe(&self) -> Option<(Point, Size)> {
            self.fills
                .iter()
                .find(|(_, _, c)| *c == PROBE_COLOR)
                .map(|(o, s, _)| (*o, *s))
        }

        /// Whether the pod's fill was the **last** thing painted — i.e. it
        /// floated above the whole main tree instead of being drawn in place.
        fn probe_painted_last(&self) -> bool {
            self.fills.last().is_some_and(|(_, _, c)| *c == PROBE_COLOR)
        }

        /// The field's own chrome rect (the first rounded rect it fills), which
        /// is where the field was laid out.
        fn field_rect(&self) -> Rect {
            let (o, s) = self
                .rounded
                .first()
                .copied()
                .expect("the field always paints its chrome");
            Rect::from_origin_size(o, s)
        }
    }

    /// The floated toolbar double: a fixed-size rect that records the presses it
    /// receives and dispatches [`EditCommand::Copy`] on the release, standing in
    /// for whatever a design system installs.
    ///
    /// Deliberately **not** `crate::selection_toolbar`'s real bar: what is under
    /// test here is the field's hosting of a pod — placement, routing, the
    /// command drain — not anyone's button layout.
    struct ProbeToolbar {
        log: ToolbarLog,
    }

    /// The double's retained widget.
    struct ProbeToolbarWidget {
        log: ToolbarLog,
    }

    impl View<()> for ProbeToolbar {
        type Element = ProbeToolbarWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeToolbarWidget {
            ProbeToolbarWidget {
                log: Arc::clone(&self.log),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut ProbeToolbarWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.log = Arc::clone(&self.log);
            ChangeFlags::NONE
        }
    }

    impl Widget for ProbeToolbarWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(PROBE_SIZE)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), PROBE_COLOR);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                self.log
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(format!("{:?}@{},{}", p.phase, p.position.x, p.position.y));
                // Fire on the release, like the real bar's items do.
                if p.phase == PointerPhase::Up {
                    ctx.dispatch_edit_command(EditCommand::Copy);
                }
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    /// Install the double in the process-global builder slot (and assert the
    /// framework route), handing back its log. Call under [`TOOLBAR_LOCK`].
    fn install_probe_toolbar() -> ToolbarLog {
        let log: ToolbarLog = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&log);
        let builder: SelectionToolbarBuilder =
            Arc::new(move |_request: &SelectionToolbarRequest, _window: Size| {
                frust_core::any(ProbeToolbar {
                    log: Arc::clone(&captured),
                })
            });
        set_selection_toolbar_builder(builder);
        set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);
        log
    }

    /// How many presses the double has seen.
    fn probe_presses(log: &ToolbarLog) -> usize {
        log.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// One whole frame: rebuild — the only pass carrying a `BuildCtx`, so the
    /// only one that can mount or drop the toolbar pod — then layout, then paint
    /// at `ms`. The loop every shipping shell runs.
    fn toolbar_frame(
        root: &mut RenderRoot<AppState, TextInputView<AppState>>,
        logic: &mut impl FnMut(&mut AppState) -> TextInputView<AppState>,
        state: &mut AppState,
        ms: f64,
    ) -> SceneLog {
        root.rebuild(logic, state);
        root.layout(TOOLBAR_WINDOW);
        let mut scene = SceneLog::default();
        root.paint(&mut scene, ft_ms(ms));
        scene
    }

    /// A complete in-slop tap at `(x, y)`.
    fn tap(
        root: &mut RenderRoot<AppState, TextInputView<AppState>>,
        state: &mut AppState,
        x: f64,
        y: f64,
    ) {
        root.event(state, &pointer(PointerPhase::Down, x, y));
        root.event(state, &pointer(PointerPhase::Up, x, y));
    }

    /// The selected text, or `None` — spelled out so an assertion reads as the
    /// selection rather than as an editor call.
    fn selection(root: &RenderRoot<AppState, TextInputView<AppState>>) -> Option<String> {
        widget(root).editor.selected_text().map(str::to_owned)
    }

    /// Open the bar with a context press and paint it, asserting a pod really
    /// got registered — the starting position each hide-rule leg needs.
    fn open_toolbar(
        root: &mut RenderRoot<AppState, TextInputView<AppState>>,
        logic: &mut impl FnMut(&mut AppState) -> TextInputView<AppState>,
        state: &mut AppState,
        ms: f64,
    ) {
        root.event(state, &secondary_pointer(PointerPhase::Down, 20.0, 10.0));
        assert!(widget(root).toolbar_open, "the leg starts with the bar up");
        let scene = toolbar_frame(root, logic, state, ms);
        assert!(scene.probe().is_some(), "…and with a pod really registered");
    }

    #[test]
    fn a_stationary_long_press_selects_the_word_and_floats_the_toolbar() {
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = install_probe_toolbar();
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, false);
        let mut root = options_root(&mut logic, &mut state);
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);

        // The press arms the hold; its first painted frame seeds the epoch.
        root.event(&mut state, &pointer(PointerPhase::Down, 20.0, 10.0));
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);
        assert!(
            !widget(&root).toolbar_open,
            "nothing fires before the threshold"
        );

        // The frame past the threshold marks it elapsed and latches the flush
        // that becomes the `Housekeeping` broadcast the fire rides out on.
        toolbar_frame(&mut root, &mut logic, &mut state, LONG_PRESS_MS + 50.0);
        assert!(
            frust_core::has_pending_result_flush(),
            "the crossing paint latches the deferred-callback flush"
        );
        root.event(&mut state, &InputEvent::Housekeeping);

        assert_eq!(
            selection(&root).as_deref(),
            Some("hello"),
            "the word under the press point is selected"
        );
        assert!(widget(&root).toolbar_open, "and the toolbar is asked for");

        // The pod is mounted by the next rebuild and registered by its paint,
        // above the whole field rather than inside it.
        let scene = toolbar_frame(&mut root, &mut logic, &mut state, LONG_PRESS_MS + 70.0);
        assert!(
            scene.probe_painted_last(),
            "the floated pod paints after the main tree: {:?}",
            scene.fills
        );

        // The published request is the selection's own bounding box, absolute.
        let field = scene.field_rect();
        let w = widget(&root);
        let expected = w
            .display()
            .selection_rects()
            .into_iter()
            .reduce(|a, b| a.union(b))
            .expect("a non-collapsed selection has rects")
            + Vec2::new(w.pad_x, w.content_origin_y(field.height()))
            + field.origin().to_vec2();
        let published = root
            .selection_toolbar()
            .expect("an open toolbar publishes its request under either policy");
        assert_eq!(published.anchor, expected);
        assert_eq!(
            published.actions,
            SelectionToolbarActions {
                copy: true,
                cut: true,
                paste: true,
                select_all: true,
            },
            "a word selected out of a longer line enables all four verbs"
        );
    }

    /// Drive a field to "the long-press fired, the finger is still down",
    /// pressing at `press_x`, and hand back the root/state to move from there.
    fn held_word(
        logic: &mut impl FnMut(&mut AppState) -> TextInputView<AppState>,
        state: &mut AppState,
        press_x: f64,
    ) -> RenderRoot<AppState, TextInputView<AppState>> {
        let mut root = options_root(logic, state);
        toolbar_frame(&mut root, logic, state, 0.0);
        root.event(state, &pointer(PointerPhase::Down, press_x, 10.0));
        toolbar_frame(&mut root, logic, state, 0.0);
        toolbar_frame(&mut root, logic, state, LONG_PRESS_MS + 50.0);
        root.event(state, &InputEvent::Housekeeping);
        assert_eq!(
            selection(&root).as_deref(),
            Some("hello"),
            "the leg starts from a fired hold over the first word"
        );
        root
    }

    #[test]
    fn an_in_slop_move_after_the_hold_fired_leaves_the_selected_word_alone() {
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = install_probe_toolbar();
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, false);
        // Press inside "hello", four characters in.
        let mut root = held_word(&mut logic, &mut state, 30.0);
        assert!(widget(&root).toolbar_open, "and with the bar raised");
        assert_eq!(
            widget(&root).gesture,
            Gesture::HoldFired {
                at: Point::new(30.0, 10.0)
            },
            "a fired hold keeps its press point rather than forgetting it"
        );

        // The finger is still down and jitters 14px — comfortably inside the
        // 18px TOUCH_SLOP that makes a press "stationary", yet several
        // characters wide at this text size, so it reaches across the word
        // boundary into "world". The selection must not notice.
        let jitter = Point::new(44.0, 10.0);
        assert!(
            (jitter - Point::new(30.0, 10.0)).hypot() <= TOUCH_SLOP,
            "the leg is only meaningful while the jitter stays in slop"
        );
        root.event(&mut state, &pointer(PointerPhase::Move, jitter.x, jitter.y));

        assert_eq!(
            selection(&root).as_deref(),
            Some("hello"),
            "a held finger jittering in slop must not re-resolve the selection"
        );
        assert_eq!(
            widget(&root).gesture,
            Gesture::HoldFired {
                at: Point::new(30.0, 10.0)
            },
            "and the press stays resolved-but-stationary, not promoted to a drag"
        );
        assert!(
            widget(&root).toolbar_open,
            "so the bar still stands over the selection its verbs were built from"
        );

        // The release must not resolve as a tap either: a long-press seeds no
        // double-tap window and toggles no toolbar.
        root.event(&mut state, &pointer(PointerPhase::Up, jitter.x, jitter.y));
        assert!(
            widget(&root).last_tap.is_none(),
            "a long-press is not a tap"
        );
        assert!(
            widget(&root).toolbar_open,
            "and the release does not close it"
        );
    }

    #[test]
    fn a_drag_out_of_a_fired_hold_extends_by_word_and_tracks_back() {
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = install_probe_toolbar();
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, false);
        let mut root = held_word(&mut logic, &mut state, 30.0);

        // Past the slop, into the second word. The post-hold drag is
        // word-granular: it swallows "world" whole rather than cutting the
        // selection off at the cluster under the pointer. This pins the
        // retained-granularity assumption the module docs call out — a text
        // engine that dropped it would truncate every long-press drag here
        // instead of failing loudly.
        let out = 30.0 + TOUCH_SLOP + 20.0;
        root.event(&mut state, &pointer(PointerPhase::Move, out, 10.0));
        assert_eq!(
            widget(&root).gesture,
            Gesture::Drag,
            "leaving the slop promotes the resolved hold to a drag"
        );
        assert_eq!(
            selection(&root).as_deref(),
            Some("hello world"),
            "the drag extends by whole words, not to the cluster under the pointer"
        );

        // Dragging back toward the press point shrinks it again — the promotion
        // to `Drag` is what keeps the selection tracking the finger instead of
        // freezing at its widest.
        root.event(&mut state, &pointer(PointerPhase::Move, 30.0, 10.0));
        assert_eq!(
            selection(&root).as_deref(),
            Some("hello"),
            "a finger brought back shrinks the selection rather than sticking"
        );
    }

    #[test]
    fn a_drag_past_the_slop_cancels_the_hold_and_selects_instead() {
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = install_probe_toolbar();
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, false);
        let mut root = options_root(&mut logic, &mut state);
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);

        root.event(&mut state, &pointer(PointerPhase::Down, 20.0, 10.0));
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);
        // Past the slop well before the threshold: the gesture became a drag.
        root.event(
            &mut state,
            &pointer(PointerPhase::Move, 20.0 + TOUCH_SLOP + 80.0, 10.0),
        );
        toolbar_frame(&mut root, &mut logic, &mut state, 200.0);
        toolbar_frame(&mut root, &mut logic, &mut state, LONG_PRESS_MS + 100.0);
        assert!(
            !frust_core::has_pending_result_flush(),
            "a cancelled hold latches nothing, however long the finger stays down"
        );

        root.event(&mut state, &InputEvent::Housekeeping);
        assert!(
            !widget(&root).toolbar_open,
            "a drag raises no toolbar of its own"
        );
        let selected = selection(&root).expect("the drag extended a selection");
        assert!(
            selected.ends_with("world"),
            "the selection followed the pointer to the end of the line, rather than \
             snapping to the word under the press: {selected:?}"
        );
    }

    #[test]
    fn a_double_tap_selects_the_word_and_a_late_second_tap_only_places_the_caret() {
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = install_probe_toolbar();
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, false);
        let mut root = options_root(&mut logic, &mut state);
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);

        // A tap, then a second one on the same spot a second later: past the
        // window, so it is an ordinary caret placement. This leg runs first,
        // while nothing is selected — a second press landing *inside* a
        // selection is the toggle gesture, which owns that case.
        tap(&mut root, &mut state, 20.0, 10.0);
        toolbar_frame(&mut root, &mut logic, &mut state, 900.0);
        root.event(&mut state, &pointer(PointerPhase::Down, 21.0, 10.0));
        assert_eq!(
            selection(&root),
            None,
            "past DOUBLE_TAP_MS the press is just a press"
        );
        root.event(&mut state, &pointer(PointerPhase::Up, 21.0, 10.0));

        // The same pair inside the window: the word under it is selected.
        toolbar_frame(&mut root, &mut logic, &mut state, 1000.0);
        root.event(&mut state, &pointer(PointerPhase::Down, 21.0, 10.0));
        assert_eq!(
            selection(&root).as_deref(),
            Some("hello"),
            "the second press of a double-tap selects the word"
        );
        assert!(
            !widget(&root).toolbar_open,
            "and deliberately raises nothing over the word it just picked"
        );
    }

    #[test]
    fn a_tap_inside_the_selection_toggles_the_toolbar_and_keeps_the_selection() {
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = install_probe_toolbar();
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, false);
        let mut root = options_root(&mut logic, &mut state);
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);

        // Get a selection the honest way, then leave the double-tap window.
        tap(&mut root, &mut state, 20.0, 10.0);
        toolbar_frame(&mut root, &mut logic, &mut state, 100.0);
        tap(&mut root, &mut state, 21.0, 10.0);
        assert_eq!(selection(&root).as_deref(), Some("hello"));
        toolbar_frame(&mut root, &mut logic, &mut state, 600.0);

        // A press inside that selection leaves it exactly alone…
        root.event(&mut state, &pointer(PointerPhase::Down, 20.0, 10.0));
        assert_eq!(
            selection(&root).as_deref(),
            Some("hello"),
            "the press neither collapses the selection nor moves the caret"
        );
        assert!(
            !widget(&root).toolbar_open,
            "and nothing opens on the press itself"
        );
        // …and the release raises the bar, fire-on-up-inside.
        root.event(&mut state, &pointer(PointerPhase::Up, 20.0, 10.0));
        assert!(widget(&root).toolbar_open, "the release opens the toolbar");
        assert_eq!(selection(&root).as_deref(), Some("hello"));

        // The same tap again toggles it away, still without disturbing the
        // selection it is pointing at.
        toolbar_frame(&mut root, &mut logic, &mut state, 1200.0);
        tap(&mut root, &mut state, 20.0, 10.0);
        assert!(
            !widget(&root).toolbar_open,
            "a second tap inside the selection closes it"
        );
        assert_eq!(selection(&root).as_deref(), Some("hello"));
    }

    #[test]
    fn a_press_inside_the_floated_toolbar_reaches_the_pod_and_its_copy_reaches_the_field() {
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let log = install_probe_toolbar();
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, false);
        let mut root = options_root(&mut logic, &mut state);
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);

        // Select a word, then open the bar with a context press — which opens no
        // capture, so the root's overlay pre-pass is reachable afterwards.
        tap(&mut root, &mut state, 20.0, 10.0);
        toolbar_frame(&mut root, &mut logic, &mut state, 100.0);
        tap(&mut root, &mut state, 21.0, 10.0);
        assert_eq!(selection(&root).as_deref(), Some("hello"));
        root.event(
            &mut state,
            &secondary_pointer(PointerPhase::Down, 20.0, 10.0),
        );
        let scene = toolbar_frame(&mut root, &mut logic, &mut state, 200.0);
        assert!(
            scene.probe_painted_last(),
            "the pod is registered and floated"
        );

        // Placed against the selection: centred on it, clear of it by the gap,
        // and never covering it.
        let placed = widget(&root).toolbar.window_rect();
        let anchor = root
            .selection_toolbar()
            .expect("the request is published while the bar is up")
            .anchor;
        assert_eq!(placed.size(), PROBE_SIZE, "placement never resizes the pod");
        assert_eq!(
            placed.y0,
            anchor.y1 + TOOLBAR_GAP,
            "flipped below the selection and the gap clear of it — a field at the \
             top of the window has no room above it"
        );
        assert_eq!(
            placed.x0,
            crate::DEFAULT_PADDING,
            "centred on the selection where it fits and shifted back inside the \
             window's padding where it does not — a 120px bar centred on a word \
             near the leading edge hangs outside"
        );

        // A press inside it reaches the pod, and the field keeps its session.
        let hit = placed.center();
        root.event(&mut state, &pointer(PointerPhase::Down, hit.x, hit.y));
        assert_eq!(probe_presses(&log), 1, "the press was routed into the pod");
        assert!(
            root.is_focus_active(),
            "a press on the bar never blurs the field it acts on"
        );
        assert!(
            widget(&root).toolbar_open,
            "and a press alone takes no verb"
        );

        // The release dispatches Copy, which the field drains and applies.
        root.event(&mut state, &pointer(PointerPhase::Up, hit.x, hit.y));
        assert_eq!(probe_presses(&log), 2);
        assert_eq!(
            root.take_clipboard_write().as_deref(),
            Some("hello"),
            "the pod's dispatched verb was applied by the field, not by the pod"
        );
        assert!(
            !widget(&root).toolbar_open,
            "a verb taken closes the bar that offered it"
        );
        assert!(root.is_focus_active(), "with the session still standing");
        assert!(
            toolbar_frame(&mut root, &mut logic, &mut state, 300.0)
                .probe()
                .is_none(),
            "and the pod is gone from the very next paint"
        );
    }

    #[test]
    fn typing_escape_scrolling_and_an_outside_press_each_put_the_toolbar_away() {
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = install_probe_toolbar();
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, false);
        let mut root = options_root(&mut logic, &mut state);
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);

        // Each leg opens the bar, paints it (so a pod is really registered),
        // applies one hide rule, and demands it be gone — both from the widget's
        // own state and from the next painted frame.

        // A text change: the selection the bar was pointing at is stale.
        open_toolbar(&mut root, &mut logic, &mut state, 100.0);
        root.event(&mut state, &ch("x"));
        assert!(!widget(&root).toolbar_open, "typing puts it away");
        assert!(
            toolbar_frame(&mut root, &mut logic, &mut state, 120.0)
                .probe()
                .is_none(),
            "and nothing is registered on the next paint"
        );

        // Escape: the session goes, and the bar with it.
        open_toolbar(&mut root, &mut logic, &mut state, 200.0);
        root.event(&mut state, &named(NamedKey::Escape, Modifiers::default()));
        assert!(!widget(&root).toolbar_open, "Escape puts it away");
        assert!(!root.is_focus_active());
        assert!(
            toolbar_frame(&mut root, &mut logic, &mut state, 220.0)
                .probe()
                .is_none()
        );

        // A scroll: the anchor moved out from under it.
        open_toolbar(&mut root, &mut logic, &mut state, 300.0);
        root.event(
            &mut state,
            &InputEvent::Scroll {
                position: Point::new(20.0, 10.0),
                delta: ScrollDelta::Pixels(0.0, -40.0),
            },
        );
        assert!(!widget(&root).toolbar_open, "a scroll puts it away");
        assert!(
            toolbar_frame(&mut root, &mut logic, &mut state, 320.0)
                .probe()
                .is_none()
        );

        // A primary press outside the field: the blur takes it too.
        open_toolbar(&mut root, &mut logic, &mut state, 400.0);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 150.0));
        assert!(!widget(&root).toolbar_open, "an outside press puts it away");
        assert!(
            !root.is_focus_active(),
            "and blurs the field as it always did"
        );
        assert!(
            toolbar_frame(&mut root, &mut logic, &mut state, 420.0)
                .probe()
                .is_none()
        );
    }

    #[test]
    fn an_obscured_field_offers_neither_copy_nor_cut_but_still_offers_paste() {
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = install_probe_toolbar();
        let mut state = AppState {
            value: "secret".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, true, false);
        let mut root = options_root(&mut logic, &mut state);
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);

        // Focus, select everything, and ask for the bar.
        tap(&mut root, &mut state, 20.0, 10.0);
        root.event(&mut state, &edit(EditCommand::SelectAll));
        assert_eq!(selection(&root).as_deref(), Some("secret"));
        root.event(
            &mut state,
            &secondary_pointer(PointerPhase::Down, 20.0, 10.0),
        );
        toolbar_frame(&mut root, &mut logic, &mut state, 100.0);

        let published = root
            .selection_toolbar()
            .expect("an obscured field publishes its request like any other");
        assert_eq!(
            published.actions,
            SelectionToolbarActions {
                copy: false,
                cut: false,
                paste: true,
                select_all: false,
            },
            "the secret is not this widget's to hand out, but writing into it is \
             ordinary — and everything is already selected"
        );
    }

    #[test]
    fn a_focused_field_publishes_its_verbs_with_no_bar_up() {
        // The fact a platform responder chain answers "may I offer Paste?"
        // from. It asks whenever it likes — a hardware Cmd+V arrives with
        // nothing on screen and never raises a bar first — so a publish gated
        // on the bar left every hardware clipboard shortcut unanswerable.
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = install_probe_toolbar();
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, false);
        let mut root = options_root(&mut logic, &mut state);
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);

        // An ordinary tap to focus: no long press, no context press, no bar.
        tap(&mut root, &mut state, 20.0, 10.0);
        let scene = toolbar_frame(&mut root, &mut logic, &mut state, 100.0);
        assert!(widget(&root).focused, "the tap focused the field");
        assert!(!widget(&root).toolbar_open, "and raised no bar");
        assert!(scene.probe().is_none(), "so nothing floated either");
        assert_eq!(selection(&root), None, "a plain tap selects nothing");

        let published = root
            .selection_toolbar()
            .expect("a focused field publishes whether or not a bar is up");
        assert_eq!(
            published.actions,
            widget(&root).toolbar_actions(),
            "the published verbs are the field's own, computed from its state \
             rather than from the bar — the rule the accesskit route already kept"
        );
        assert!(
            published.actions.paste,
            "a bare caret in an interactive field is exactly what paste is for"
        );
        assert!(
            !published.present_menu,
            "…while nothing asked for a menu, so nothing asks a shell to present one"
        );
        assert_eq!(
            published.anchor,
            widget(&root).toolbar_anchor,
            "anchored on the caret rect with the selection collapsed"
        );

        // And the bar going up is the same request with the one flag raised.
        root.event(
            &mut state,
            &secondary_pointer(PointerPhase::Down, 20.0, 10.0),
        );
        toolbar_frame(&mut root, &mut logic, &mut state, 200.0);
        assert!(
            root.selection_toolbar()
                .expect("still focused, still publishing")
                .present_menu
        );
    }

    #[test]
    fn the_native_policy_publishes_the_request_and_floats_nothing() {
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = install_probe_toolbar();
        set_selection_toolbar_policy(SelectionToolbarPolicy::Native);
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, false);
        let mut root = options_root(&mut logic, &mut state);
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);

        tap(&mut root, &mut state, 20.0, 10.0);
        toolbar_frame(&mut root, &mut logic, &mut state, 100.0);
        tap(&mut root, &mut state, 21.0, 10.0);
        assert_eq!(selection(&root).as_deref(), Some("hello"));
        root.event(
            &mut state,
            &secondary_pointer(PointerPhase::Down, 20.0, 10.0),
        );
        assert!(widget(&root).toolbar_open);

        let scene = toolbar_frame(&mut root, &mut logic, &mut state, 200.0);
        assert!(
            scene.probe().is_none(),
            "the platform draws it: the field floats nothing at all"
        );
        assert!(
            !widget(&root).toolbar.is_open(),
            "and mounts no pod to float"
        );
        assert!(
            root.selection_toolbar().is_some(),
            "…while the request a shell hands the platform is published all the same"
        );

        // Leave the slot as the rest of the process expects to find it.
        set_selection_toolbar_policy(SelectionToolbarPolicy::Framework);
    }

    #[test]
    fn the_long_press_timer_requests_plain_frames_while_the_blink_paces() {
        // A finger still down is a live long-press timer, and its continuation
        // frames are deliberately NOT the blink's paced ones: a frame gate that
        // throttled them would delay the gesture past its own threshold, and
        // `reduce_motion` — which stops the blink's requests entirely — must not
        // stop a gesture from firing at all.
        let mut state = AppState::default();
        let mut root = harness(&mut state);
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let held = root.paint(&mut NullScene, ft_ms(0.0));
        assert!(held.needs_frame, "a live hold keeps the frames coming");
        assert!(
            !held.needs_frame_paced_only,
            "a gesture clock is not a cosmetic loop the frame gate may pace"
        );

        // Released: the hold is gone and only the caret's paced request is left.
        root.event(&mut state, &pointer(PointerPhase::Up, 10.0, 10.0));
        let idle = root.paint(&mut NullScene, ft_ms(20.0));
        assert!(
            idle.needs_frame_paced_only,
            "with no hold in flight the blink is the only thing asking"
        );

        // Frozen blink, live hold: the plain request survives reduce_motion.
        let mut theme = Theme::neutral();
        theme.motion.reduce_motion = true;
        root.set_theme(Box::new(theme));
        // Past the double-tap window, so the next press is an ordinary one that
        // arms a hold rather than a word-selecting second tap.
        root.paint(&mut NullScene, ft_ms(400.0));
        root.event(&mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        let frozen = root.paint(&mut NullScene, ft_ms(420.0));
        assert!(
            frozen.needs_frame,
            "a frozen blink must not freeze the long-press timer"
        );
        assert!(!frozen.needs_frame_paced_only);
    }

    #[test]
    fn a_cancel_clears_the_hold_without_touching_the_selection_or_the_toolbar() {
        let _guard = TOOLBAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _log = install_probe_toolbar();
        let mut state = AppState {
            value: "hello world".to_string(),
            ..Default::default()
        };
        let mut logic = options_logic(true, false, false);
        let mut root = options_root(&mut logic, &mut state);
        toolbar_frame(&mut root, &mut logic, &mut state, 0.0);

        // A selection, and a bar up over it.
        tap(&mut root, &mut state, 20.0, 10.0);
        toolbar_frame(&mut root, &mut logic, &mut state, 100.0);
        tap(&mut root, &mut state, 21.0, 10.0);
        assert_eq!(selection(&root).as_deref(), Some("hello"));
        open_toolbar(&mut root, &mut logic, &mut state, 600.0);

        // A gesture stolen with no press of ours in flight says nothing about
        // the bar: a Cancel is not one of the hide rules.
        root.event(&mut state, &pointer(PointerPhase::Cancel, 20.0, 10.0));
        assert!(
            widget(&root).toolbar_open,
            "a Cancel never puts the toolbar away"
        );

        // And a press the platform steals mid-hold disarms the gesture without
        // touching the selection it was made over.
        root.event(&mut state, &pointer(PointerPhase::Down, 20.0, 10.0));
        root.event(&mut state, &pointer(PointerPhase::Cancel, 20.0, 10.0));
        assert_eq!(
            selection(&root).as_deref(),
            Some("hello"),
            "a Cancel never touches application state"
        );
        assert!(!widget(&root).captured, "it does disarm the drag");
        assert!(widget(&root).hold.is_none(), "and the hold timer with it");
        assert_eq!(widget(&root).gesture, Gesture::None);
        assert!(widget(&root).tap_in_selection.is_none());
    }
}
