//! The browser's input-method bridge: a hidden `<input>` overlay that turns
//! real composition (CJK marked text, dead keys, mobile autocorrect) into the
//! framework's own [`ImeEvent`](frust_core::event::ImeEvent) vocabulary.
//!
//! # Why an overlay at all
//!
//! winit's web backend emits no `WindowEvent::Ime` — its own source implements
//! none of the IME setters and never raises the event (upstream issue 4424 is
//! open with no timeline), so the ported desktop path in
//! [`crate::app_handler`] has nothing feeding it on this target. Composition is
//! therefore bypassed around winit entirely and read off a real DOM element,
//! the way `eframe`'s text agent and Flutter's web text-editing host both do
//! it: an input method only ever talks to a focused *editable* element, and a
//! `<canvas>` is not one.
//!
//! # Shape
//!
//! One `<input>` per session, created when the focused widget publishes an
//! active [`ImeState`] and removed when it stops. It lives in `document.body`
//! at `position: fixed`, sized and placed over the focused field's caret box
//! ([`overlay_box`]) so the browser's candidate window opens at the caret, and
//! is invisible and inert (`opacity: 0`, `pointer-events: none`), so it never
//! paints over the canvas nor swallows a click meant for it.
//!
//! # The key path stays winit's
//!
//! winit attaches its `keydown`/`keyup` listeners to the **canvas element**
//! (its `web_sys` backend's own `Canvas::add_event`), not to `document` the way
//! the two precedents above attach theirs. A focused overlay is not a canvas
//! descendant, so those listeners would go silent for as long as a text field
//! held focus — every plain key, every arrow, every backspace. The overlay
//! therefore re-dispatches each keystroke it receives onto the canvas
//! ([`forwards_to_canvas`] decides which), where winit maps it exactly as it
//! maps a keystroke typed with the canvas focused. Plain typing keeps running
//! through the existing path unchanged; only composition takes the new one.
//!
//! The copy is what winit cancels, not the original — which is what leaves the
//! browser free to keep driving the composition the original keystroke started.
//! The original's own default action is contained by the element it lands in: a
//! focused text input answers a space, an arrow or a backspace by editing
//! itself, never by scrolling or navigating the page, and the overlay's value
//! is thrown away rather than read. Tab is the one default worth naming — it
//! moves DOM focus out, which this bridge reads as the end of the session.
//!
//! # Dedupe against that key path
//!
//! Composed text must reach the tree once, not twice, and the two sources
//! overlap by construction — the browser both composes into the overlay and
//! reports the underlying keystrokes. Four rules, all keyed off the single
//! [`ComposeLatch`](crate::app_handler::ComposeLatch) this crate already
//! carries:
//!
//! 1. A keystroke the input method consumed is never forwarded to the canvas —
//!    [`forwards_to_canvas`] drops it on the DOM's own `isComposing` flag and
//!    on the `Process` key sentinel a browser reports for it.
//! 2. A DOM `input` signal only becomes framework text while the latch says a
//!    composition owns the field. Every other `input` is dropped, because those
//!    characters already reached the tree as a `WindowEvent::KeyboardInput` —
//!    *unless* rule 4 says the key path never carried that keystroke at all.
//! 3. Once a session has produced its outcome the latch refuses a second one,
//!    so the two orderings browsers use for the end of a composition
//!    (`compositionend` then `input`, or `input` alone) both deliver exactly
//!    one [`Commit`](frust_core::event::ImeEvent::Commit).
//! 4. A keystroke the browser could not name ([`UNIDENTIFIED_KEY`], what a
//!    soft keyboard reports for most of its keys) maps to nothing winit can
//!    deliver, so the key path drops it and marks that it did
//!    ([`key_path_dropped`], queued as [`DomEditEvent::KeyDropped`] so it keeps
//!    its place in the signal order). The very next signal consumes that mark:
//!    when it is a non-composing `input` the composition machinery did not
//!    claim, that `input` — and only it — becomes the keystroke instead. A
//!    mark nothing consumes dies with the signal drain that queued it, or
//!    with the next keystroke the key path does deliver; it never reaches a
//!    later keystroke's own echo.
//!
//! # The clipboard route rides the same element
//!
//! A browser hands clipboard access to the focused *editable* element and to
//! nobody else, so the overlay is also where copy, cut and paste are read —
//! the canvas could not receive them. Three more listeners, all synchronous:
//!
//! * `paste` cancels the browser's own insertion first and reads
//!   `clipboardData.getData("text/plain")` after (no permission, no promise),
//!   so no `insertFromPaste` `input` follows and the element's value never
//!   moves — not even when the read comes back with nothing. The text reaches
//!   the tree as [`DomEditEvent::Paste`].
//! * `copy`/`cut` write with `clipboardData.setData` and cancel the default
//!   action. They must finish inside the callback — Safari honours a clipboard
//!   write from nowhere else — so they cannot ask the tree anything, and take
//!   their text from one of two places instead ([`clipboard_write_action`]):
//!   the [`ClipboardHandoff`] the shell filled for the gesture already in
//!   flight, or, for a gesture the tree never saw (a browser edit menu's own
//!   copy), the selection the shell last published ([`clipboard_selection`],
//!   refreshed on every dispatch and every frame). `cut` queues
//!   [`EditCommand::Cut`](frust_core::event::EditCommand::Cut) only in that
//!   second case — a handed-off write is the answer to a gesture the widget
//!   has already applied, and asking for the delete again would be a second
//!   edit.
//! * A secret field and a collapsed selection are both refused on that second
//!   source — and a refusal cancels the event exactly as a write does
//!   ([`clipboard_write_action`]). An obscured field publishes its *real* text
//!   on its [`ImeState`] (the mask is a paint-time affair), so this refusal is
//!   what keeps a password off the host clipboard for a gesture that reached
//!   no widget to be refused by; leaving the browser's own copy to run would
//!   hand it whatever the element itself holds, which is not reliably nothing
//!   — the value is emptied only while no composition owns it, and a
//!   `type="password"` element can still carry a preedit. The refusal reads
//!   the content-type hint rather than matching a variant, for
//!   [`overlay_attributes`]'s fail-closed reason. A handed-off write carries
//!   no such refusal, and must not: it is a widget's own answer, and a widget
//!   that means to withhold an obscured field's text withholds it there.
//! * `beforecopy`/`beforecut` cancel unconditionally and answer nothing else.
//!   Cancelling one is how a page tells Blink and WebKit that it will produce
//!   the clipboard data itself, which is the only thing that enables the
//!   copy/cut command on an element whose selection is collapsed — the
//!   overlay's permanent state, since nothing here ever selects it. Gecko
//!   defines neither event, which is why nothing downstream may depend on
//!   them: they widen where the DOM answers a copy, they do not decide it.
//!
//! # A cut may not delete text it never wrote out
//!
//! A keyboard cut is answered in two halves that run inside one browser task,
//! in a fixed order. The canvas re-dispatch reaches the widget first: it puts
//! the cut text in the tree's one-shot clipboard slot *and* deletes the
//! selection in the same pass, and the shell republishes the field's surface
//! behind it. Only once that `keydown` has returned does the browser run its
//! default action and raise `cut`. The published selection is collapsed by
//! then — so a callback that re-derived its text from that snapshot would
//! write nothing, and the text the widget had already deleted would ride
//! `navigator.clipboard.writeText` alone. That call is undefined outside a
//! secure context, which is a page where the cut would destroy text it put
//! nowhere.
//!
//! So while a copy or cut keystroke is in flight the tree's own write is not
//! issued asynchronously: it is handed to the listener through
//! [`ClipboardHandoff`], which the `copy`/`cut` callback drains synchronously
//! inside the gesture — the write this design calls the reliable one, now the
//! primary route for both verbs rather than the copy's alone.
//! `navigator.clipboard.writeText` becomes the fallback the next drain issues
//! for a handoff nobody came for (an engine that raised no event), and a
//! `setData` that fails puts the text back for that same fallback to take, so
//! the gesture has a route out either way. One consequence: the two writes are
//! alternatives rather than a pair, and a copy reaches the host clipboard
//! exactly once.
//!
//! What this does not cover is an engine that raises no `cut` for the
//! overlay's collapsed selection *and* withholds the async clipboard — a
//! non-secure page on an engine with no `beforecut` to claim the verb with.
//! There every route refuses after the widget has already deleted. Holding the
//! delete back until a write is confirmed is the only answer left, and the
//! confirmation is asynchronous, so it would have to be pushed back into the
//! widget across the shell/widget seam this bridge does not cross.
//!
//! # Which clipboard gestures leave the key path
//!
//! A clipboard keystroke can reach a widget twice — once as the DOM event this
//! bridge answers, once as the key event the canvas re-dispatch produces and a
//! widget decodes into the same verb. The exclusion that prevents that follows
//! the clipboard **verb** the keystroke means, not the letter it is spelled
//! with ([`clipboard_verb`]):
//!
//! * A **paste** gesture is withheld from the re-dispatch
//!   ([`withheld_from_key_path`]). The DOM raises `paste` for it whatever the
//!   selection looks like — the command asks only whether the element is
//!   editable — and the event carries the text itself, which no key event
//!   could.
//! * A **copy** or **cut** gesture keeps the key path. Both engines enable
//!   those commands only for a page that claims the verb (a cancelled
//!   `beforecopy`/`beforecut`) or for a *ranged* selection, and the overlay has
//!   neither by construction, so the event may never arrive at all; a bridge
//!   that waited for it would drop the gesture on the floor, which is what
//!   made `Cmd`/`Ctrl`+`c` a no-op. The keystroke that keeps the key path
//!   marks the [`ClipboardHandoff`] on its way past
//!   ([`hands_write_to_dom_event`]), so the two halves of the gesture describe
//!   one write rather than two: the widget's own answer is what the DOM
//!   callback writes, and nothing is issued asynchronously unless that
//!   callback never runs ([`crate::app_handler::clipboard_write_route`]).
//!
//! A withheld keydown is dropped rather than cancelled — the browser's own
//! default action is what fires the clipboard event at all. `Ctrl`/`Cmd`+`a`
//! keeps its key path too: select-all touches no clipboard and the DOM raises
//! nothing for it.
//!
//! # Cases handled here
//!
//! * **Focus/blur races between the canvas and the overlay.** A click on the
//!   canvas moves DOM focus off the overlay before winit reports the press, so
//!   the overlay is re-asserted from the *dispatched* event rather than from a
//!   frame: [`OverlayPolicy::poll`] re-focuses on a pointer gesture or on a
//!   focus/IME generation move, and never from the frame loop, which would
//!   fight a user who deliberately tabbed away.
//! * **Autofill, autocorrect and spellcheck popping over the app.** The element
//!   is created with `autocomplete`/`autocorrect`/`autocapitalize` off and
//!   `spellcheck="false"`, and is emptied whenever it is not composing — from
//!   the frame loop as well as from a dispatched event, so a paste or a last
//!   keystroke that no further event follows cannot linger in the DOM value.
//! * **A field that holds a secret or refuses suggestions.** The focused
//!   widget's [`ImeState::content_type`](frust_core::event::ImeState) picks the
//!   element's attributes ([`overlay_attributes`]): a secret field is a real
//!   `type="password"` element, so the browser applies its own secure-entry
//!   handling and no keyboard mines the text for suggestions or word learning.
//!   A hint that moves rebuilds the element ([`overlay_needs_rebuild`]) —
//!   `type` is what the browser reads when it decides that handling — and the
//!   check runs from the frame loop as well as from a dispatched event, since
//!   a focus move a signal drove reaches the tree with no event behind it
//!   ([`stale_overlay_action`]).
//! * **A composition cancelled with Escape.** Browsers disagree about whether
//!   `compositionend` fires (and with what data) for a cancel, so Escape is
//!   read from the keystroke itself while composing
//!   ([`DomEditEvent::Cancel`]), clearing the preedit; a `compositionend`
//!   arriving afterwards is absorbed rather than re-applied.
//! * **An input method that commits through `input` with no `compositionend`.**
//!   A non-composing `insert*` signal arriving while a session is still open is
//!   taken as that session's commit.
//! * **An empty `compositionend` that is not a cancel.** Several browsers end a
//!   composition with no data and deliver the committed text on the `input`
//!   that follows. The preedit is therefore not retracted on the empty end
//!   itself: the latch holds the session one signal longer and lets an
//!   `insert*` `input` carrying data resolve it as that session's commit —
//!   every other continuation resolves it as the cancel it looked like.
//! * **Soft keyboards.** A mobile browser opens one only for a `focus()` inside
//!   a user gesture, which is why the pointer-press path re-focuses. Their keys
//!   mostly arrive unnamed, so the edits they produce reach the tree through
//!   the `input` path (dedupe rule 4) rather than through winit's key mapping,
//!   which has nothing to map them to. Their backspace is the exception,
//!   below.
//! * **An overlay torn down mid-composition.** Removing the element ends the
//!   session, and the DOM raises nothing for it once the listeners are off — so
//!   the shell synthesizes [`DomEditEvent::Teardown`] ([`teardown_signal`]),
//!   which retracts the preedit and reopens the latch exactly as a blur does.
//!
//! # Cases deliberately not handled
//!
//! * **A soft keyboard's backspace.** It arrives unnamed like its letters, but
//!   the `input` that would report the deletion never fires: the element is
//!   emptied on every frame no composition owns, so there is nothing for it
//!   to delete. Carrying it means keeping something in the element for a
//!   deletion to consume, which has to be watched on a device rather than
//!   reasoned out here; until then the keystroke is lost, and the gap is
//!   recorded in LIMITATIONS `web-ime-residual-gaps`.
//! * **Composition on a secret field.** A `type="password"` element may get no
//!   input method at all — many browsers and keyboards switch secure entry to
//!   a plain layout, as the native platforms do — so such a field takes text
//!   through the key path and rule 4 only.
//! * **The mobile viewport jump when a soft keyboard opens.** The overlay is
//!   placed in layout-viewport coordinates; a browser that scrolls or shrinks
//!   the *visual* viewport to make room for the keyboard leaves it offset from
//!   the field until the next reposition. Following that needs the
//!   `visualViewport` offset threaded through the shell's own coordinate
//!   conversion, which is a window-metrics change rather than an IME one.
//! * **A composition's own selection inside the preedit.** The DOM reports the
//!   marked text but not the caret within it, so the caret is placed at the end
//!   of the preedit — the same assumption both precedents make.
//! * **Multiple simultaneous editable fields.** One session at a time, matching
//!   what [`ImeState`] itself publishes.

use frust_core::event::{EditCommand, ImeContentType, ImeState};
use frust_text::utf16_to_byte;
use kurbo::Rect;

/// The DOM `key` value a browser reports for a keystroke its input method
/// consumed — the modern spelling of the legacy `keyCode: 229` signal. A key
/// carrying it produced composition, not text, so it must not reach the key
/// path.
pub const IME_PROCESS_KEY: &str = "Process";

/// The DOM `key` value for a keystroke a browser could not name — what a
/// mobile soft keyboard reports for most of its keys, and what some input
/// methods report instead of [`IME_PROCESS_KEY`] while composing.
///
/// It is never forwarded to the canvas, but for two different reasons, and the
/// difference decides where the edit it produced goes:
///
/// * **While composing** it belongs to the input method, exactly like
///   [`IME_PROCESS_KEY`]: the text arrives as `Compose`/`Commit` and the
///   keystroke must not reach the tree at all.
/// * **While not composing** it is a real edit that winit's own web key mapping
///   turns into nothing — the pinned backend maps an unnamed key to no
///   `Key`/`NamedKey` at all, so re-dispatching it onto the canvas produces no
///   event. Dropping it is therefore not a choice but a fact, and the edit is
///   picked up from the `input` signal that follows instead — see
///   [`key_path_dropped`] and the module doc's dedupe rule 4.
pub const UNIDENTIFIED_KEY: &str = "Unidentified";

/// The element's `id`, so a page inspecting its own DOM (or a bug report's
/// screenshot of one) can tell what put an extra `<input>` there.
const OVERLAY_ID: &str = "frust-ime-overlay";

/// The smallest side, in CSS pixels, the overlay element is ever given.
///
/// A zero-area element is not rendered, and an element that is not rendered is
/// not a focusable area — so a field whose published caret box has collapsed
/// (an empty line, a field measured before its first layout) would otherwise
/// produce an overlay that cannot take focus at all.
pub const MIN_OVERLAY_SIDE: f64 = 1.0;

/// One signal from the overlay element, reduced to plain Rust values.
///
/// The DOM types are only nameable on `wasm32`, and the mapping below is the
/// part worth testing, so the browser half converts each event to this
/// vocabulary at the listener boundary and everything after it runs on the
/// build host as well.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomEditEvent {
    /// `compositionstart`: a session opened. Carries no text of its own.
    CompositionStart,
    /// `compositionupdate`: `data` is the whole marked text so far, not a delta.
    CompositionUpdate {
        /// The current preedit.
        data: String,
    },
    /// `compositionend`: `data` is the committed text, empty for a cancel.
    CompositionEnd {
        /// The text the input method settled on.
        data: String,
    },
    /// `input`: the element's own value changed. `input_type` is the DOM
    /// `inputType` (`insertText`, `insertCompositionText`,
    /// `deleteContentBackward`, ...), and `is_composing` is the event's own
    /// flag rather than anything this crate inferred.
    Input {
        /// The DOM `inputType`.
        input_type: String,
        /// The inserted text, absent for a deletion.
        data: Option<String>,
        /// Whether the DOM considers a composition to be in progress.
        is_composing: bool,
    },
    /// Escape pressed while a composition was open — see the module doc for why
    /// the cancel is read from the keystroke rather than from
    /// `compositionend`.
    Cancel,
    /// A `keydown` the key path could not carry ([`key_path_dropped`]). Carries
    /// no edit of its own: it marks the *next* signal as the one that must
    /// deliver the keystroke, and is queued rather than flagged so it keeps its
    /// place among the composition signals around it.
    KeyDropped,
    /// The element lost DOM focus. Any session it still held is over.
    Blur,
    /// The shell removed the element while a session was still open — see
    /// [`teardown_signal`]. Synthesized rather than observed: the listeners
    /// come off before the element does, so no `blur` is raised for it.
    Teardown,
    /// `paste`, or an asynchronous `navigator.clipboard.readText()` the tree
    /// asked for: the text the host clipboard handed over, already read by the
    /// time it is queued. An empty one is queued too — whether nothing is worth
    /// inserting is the receiving widget's rule, not this bridge's.
    Paste(String),
    /// A clipboard gesture the DOM resolved for us, as the framework verb it
    /// means — today only [`EditCommand::Cut`], queued by the `cut` listener
    /// after it has already written the selection out, so the widget deletes
    /// what the browser just took.
    EditCommand(EditCommand),
}

/// What the shell must do to the overlay element for one dispatched event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayAction {
    /// Nothing: no session, or one already in the state it should be in.
    Idle,
    /// Open a session — create the element, place it, and take DOM focus.
    Open,
    /// Keep the session: re-place the element, and take DOM focus back when
    /// `refocus` is set (a user gesture, or a focus/IME move).
    Update {
        /// Whether DOM focus must be re-asserted this pass.
        refocus: bool,
    },
    /// Close the session — remove the element.
    Close,
}

/// What one `ImeOverlay::sync` pass did, for the caller that has to keep the
/// compose latch consistent with it.
///
/// The action alone is not enough: a content-type change removes and re-creates
/// the element inside an [`OverlayAction::Update`], which ends the DOM's
/// composition exactly as a close does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlaySync {
    /// The lifecycle action the policy chose for this pass.
    pub action: OverlayAction,
    /// Whether the pass removed a live element — a close, or the teardown half
    /// of a rebuild. Feed it to [`teardown_signal`] with the latch's own state.
    pub detached: bool,
}

/// The overlay's lifecycle decision, split out from the DOM so it runs and is
/// unit-tested on the build host.
///
/// Two inputs, both already carried by [`frust_core::RenderRoot`]: whether the
/// focused widget publishes an active [`ImeState`], and the focus/IME
/// generation counter, whose *movement* is the shell's edge signal that the
/// session changed (see that type's `focus_ime_generation`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OverlayPolicy {
    generation: Option<u64>,
    open: bool,
}

impl OverlayPolicy {
    /// A policy with no session open and no generation seen yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a session is currently open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Decide the overlay's action for one **dispatched input event**.
    ///
    /// `gesture` marks a pointer press: the one moment a browser will honour a
    /// `focus()` by opening a soft keyboard, and the moment the canvas has just
    /// stolen DOM focus back from the overlay.
    ///
    /// A session opens on a gesture or on a generation move, never on a bare
    /// pointer move — otherwise a cursor crossing the canvas would pull focus
    /// back from wherever the user had deliberately put it.
    pub fn poll(&mut self, ime_active: bool, generation: u64, gesture: bool) -> OverlayAction {
        let moved = self.generation != Some(generation);
        self.generation = Some(generation);
        match (ime_active, self.open) {
            (false, false) => OverlayAction::Idle,
            (false, true) => {
                self.open = false;
                OverlayAction::Close
            }
            (true, true) => OverlayAction::Update {
                refocus: gesture || moved,
            },
            (true, false) => {
                if gesture || moved {
                    self.open = true;
                    OverlayAction::Open
                } else {
                    OverlayAction::Idle
                }
            }
        }
    }

    /// The frame loop's half: close a session the app ended on its own (a
    /// programmatic blur, a field unmounted by a rebuild), which no input event
    /// would otherwise report.
    ///
    /// Deliberately cannot *open* one: opening steals DOM focus, and a frame
    /// runs for a caret blink as readily as for a user action.
    pub fn close_if_inactive(&mut self, ime_active: bool) -> bool {
        let closing = self.open && !ime_active;
        if closing {
            self.open = false;
        }
        closing
    }

    /// Record that the element went away on its own — the user moved DOM focus
    /// off it (a tab-out, a click into browser chrome). The next gesture or
    /// generation move opens a fresh session.
    pub fn mark_closed(&mut self) {
        self.open = false;
    }
}

/// The overlay element's box, in CSS pixels relative to the viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayBox {
    /// Distance from the viewport's left edge.
    pub left: f64,
    /// Distance from the viewport's top edge.
    pub top: f64,
    /// Element width, never below [`MIN_OVERLAY_SIDE`].
    pub width: f64,
    /// Element height, never below [`MIN_OVERLAY_SIDE`].
    pub height: f64,
}

/// Place the overlay over the focused field's caret.
///
/// `caret` is the rectangle the focused widget published on its [`ImeState`],
/// in the window-logical pixels the tree lays out in — the same value the
/// desktop shell hands winit's `set_ime_cursor_area`. On this target those are
/// CSS pixels by construction: the shell lays out at `inner_size /
/// scale_factor`, and `scale_factor` is `devicePixelRatio`, so one logical
/// pixel is one CSS pixel of the canvas's own box. Adding the canvas's
/// viewport origin is therefore the whole conversion.
///
/// A field with no caret yet collapses to a minimum-size box at the canvas
/// origin, which still gives the browser a focusable element to compose into.
pub fn overlay_box(caret: Option<Rect>, canvas_left: f64, canvas_top: f64) -> OverlayBox {
    let caret = caret.unwrap_or_else(|| Rect::new(0.0, 0.0, 0.0, 0.0));
    OverlayBox {
        left: canvas_left + caret.x0,
        top: canvas_top + caret.y0,
        width: caret.width().max(MIN_OVERLAY_SIDE),
        height: caret.height().max(MIN_OVERLAY_SIDE),
    }
}

/// Whether the caret box moved far enough to be worth writing to the DOM again.
///
/// Sub-pixel movement is invisible to a candidate window and a style write is a
/// layout invalidation, so an unchanged (or barely changed) box is skipped —
/// the same change-guard shape [`crate::app_handler::sync_cursor`] applies to
/// the cursor shape.
pub fn overlay_box_moved(last: Option<OverlayBox>, current: OverlayBox) -> bool {
    let Some(last) = last else {
        return true;
    };
    const EPSILON: f64 = 0.5;
    (last.left - current.left).abs() >= EPSILON
        || (last.top - current.top).abs() >= EPSILON
        || (last.width - current.width).abs() >= EPSILON
        || (last.height - current.height).abs() >= EPSILON
}

/// Whether a keystroke the overlay received is re-dispatched onto the canvas,
/// where winit's own listener maps it.
///
/// `key` is the DOM `KeyboardEvent.key` value and `dom_is_composing` its own
/// `isComposing` flag — both read off the event rather than inferred, so the
/// browser's view of its input method wins over any state this crate keeps.
/// Composition-owned keystrokes are dropped: their text arrives as
/// [`ImeEvent::Compose`](frust_core::event::ImeEvent::Compose) /
/// [`Commit`](frust_core::event::ImeEvent::Commit) instead, and forwarding them
/// too is exactly the double delivery the module doc's dedupe rules exist to
/// prevent.
pub fn forwards_to_canvas(key: &str, dom_is_composing: bool) -> bool {
    !dom_is_composing && key != IME_PROCESS_KEY && key != UNIDENTIFIED_KEY
}

/// Whether a keystroke [`forwards_to_canvas`] refused is one whose *edit* still
/// has to reach the tree, through the `input` signal behind it.
///
/// Exactly one of the two refusals qualifies. A composition-owned keystroke
/// (`isComposing`, or [`IME_PROCESS_KEY`]) is text the composition events
/// already report, and carrying it again is the double delivery the dedupe
/// rules exist to prevent. An [`UNIDENTIFIED_KEY`] pressed with no composition
/// open is the opposite case: it is a plain edit — a soft keyboard's letter —
/// that winit's web key mapping cannot name, so unless the `input` behind it is
/// allowed through, nothing about that keystroke reaches the tree at all. (Its
/// backspace is the gap the module doc records: with nothing in the element to
/// delete, no `input` follows, and the mark it leaves dies with the drain.)
pub fn key_path_dropped(key: &str, dom_is_composing: bool) -> bool {
    !dom_is_composing && key == UNIDENTIFIED_KEY
}

/// The clipboard verb a keystroke means, whatever it is spelled with.
///
/// The vocabulary is the browser's own — these are the three gestures whose
/// default action raises a `copy`, `cut` or `paste` event — and it is also the
/// vocabulary a text widget decodes the same keystrokes into, which is what
/// makes a duplicate delivery possible at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardVerb {
    /// Put the selection on the clipboard and leave the document alone.
    Copy,
    /// Put the selection on the clipboard and delete it.
    Cut,
    /// Insert what the clipboard holds.
    Paste,
}

/// The clipboard verb one keystroke means, or `None` for a keystroke that means
/// no clipboard verb at all.
///
/// `ctrl`, `meta` and `shift` are the event's own `ctrlKey`, `metaKey` and
/// `shiftKey`. The two chord modifiers are taken apart rather than folded into
/// one flag because the widget's table does not treat them alike everywhere:
/// the letters answer to either (the platforms differ only in which of them
/// carries the chord), while the legacy `Insert` copy answers to `Ctrl` alone
/// — `Cmd`+`Insert` is no verb on any platform, and reading it as a copy would
/// have this bridge claim a gesture the widget behind it drops.
///
/// # The table
///
/// It is deliberately the same table `frust-widgets`' text field decodes, so
/// this bridge and the widget behind it never disagree about what a keystroke
/// meant:
///
/// | Keystroke | Verb |
/// |---|---|
/// | `Ctrl`/`Cmd`+`c` | copy |
/// | `Ctrl`/`Cmd`+`x` | cut |
/// | `Ctrl`/`Cmd`+`v` | paste |
/// | `Ctrl`+`Insert` | copy |
/// | `Shift`+`Insert` | paste |
/// | `Shift`+`Delete` | cut |
/// | `Copy` / `Cut` / `Paste` | the key's own verb |
///
/// The letters are compared case-insensitively: a browser reports `"C"` for
/// `Cmd`+`Shift`+`c`, and the chord means the same thing either way. `Shift`
/// wins on `Insert` — `Ctrl`+`Shift`+`Insert` pastes rather than copying —
/// and loses on `Delete`, where anything chorded past shift is an OS or
/// browser gesture rather than a field cut. A bare `Insert` toggles overtype,
/// which is no clipboard verb, and the dedicated keys carry their verb with no
/// modifier at all.
pub fn clipboard_verb(key: &str, ctrl: bool, meta: bool, shift: bool) -> Option<ClipboardVerb> {
    if ctrl || meta {
        if key.eq_ignore_ascii_case("c") {
            return Some(ClipboardVerb::Copy);
        }
        if key.eq_ignore_ascii_case("x") {
            return Some(ClipboardVerb::Cut);
        }
        if key.eq_ignore_ascii_case("v") {
            return Some(ClipboardVerb::Paste);
        }
    }
    match key {
        // The dedicated hardware keys: rare, but they arrive already decoded.
        "Copy" => Some(ClipboardVerb::Copy),
        "Cut" => Some(ClipboardVerb::Cut),
        "Paste" => Some(ClipboardVerb::Paste),
        // The legacy chords, which a browser still runs its own clipboard
        // command for. `Ctrl`+`Insert` is the copy on every platform that has
        // the gesture at all — the widget reads `Cmd`+`Insert` as nothing, and
        // so does this.
        "Insert" if shift => Some(ClipboardVerb::Paste),
        "Insert" if ctrl => Some(ClipboardVerb::Copy),
        "Delete" if shift && !ctrl && !meta => Some(ClipboardVerb::Cut),
        _ => None,
    }
}

/// Whether a keystroke the overlay received is withheld from the canvas
/// re-dispatch because the DOM answers it on its own.
///
/// True for a paste gesture and for nothing else. The reasoning is the module
/// doc's *Which clipboard gestures leave the key path*: `paste` is the one
/// clipboard event a browser raises whatever the element's selection looks
/// like, and it is the only one carrying data a key event could not, while
/// `copy`/`cut` may never be raised for this overlay at all and so must keep
/// the key path that can always carry them.
///
/// The keystroke is dropped rather than cancelled — the browser's own default
/// action is what *fires* the `paste` event, so cancelling the keydown would
/// leave nothing to answer.
pub fn withheld_from_key_path(key: &str, ctrl: bool, meta: bool, shift: bool) -> bool {
    clipboard_verb(key, ctrl, meta, shift) == Some(ClipboardVerb::Paste)
}

/// Whether a keystroke's clipboard write belongs to a DOM `copy`/`cut`
/// callback rather than to `navigator.clipboard.writeText`.
///
/// True for the two verbs that write — a copy and a cut — and for nothing
/// else. Both keep the key path ([`withheld_from_key_path`]), so the widget
/// answers the keystroke first and fills the tree's clipboard slot inside the
/// same browser task; this is what tells the drain behind it that the text is
/// owed to a callback still to come rather than to a promise, and so what
/// fills the [`ClipboardHandoff`].
///
/// A paste is not one of them: it carries no write at all, and it never
/// reaches the canvas in the first place.
pub fn hands_write_to_dom_event(key: &str, ctrl: bool, meta: bool, shift: bool) -> bool {
    matches!(
        clipboard_verb(key, ctrl, meta, shift),
        Some(ClipboardVerb::Copy | ClipboardVerb::Cut)
    )
}

/// The signal a torn-down overlay owes the compose latch.
///
/// Removing the element ends the browser's composition, but silently: the
/// listeners are removed first (so a focused element's own `blur` cannot
/// re-enter this bridge), and nothing else reports it. A latch left believing a
/// composition is live would then suppress the next plain keystroke and leave a
/// preedit painted in a field the user has moved on from, so the shell
/// synthesizes the end of the session — with blur semantics, since that is what
/// losing the element is.
///
/// `detached` is [`OverlaySync::detached`] (or the answer of
/// `ImeOverlay::close_if_inactive` / `ImeOverlay::replace_if_stale`);
/// `session_open` is the latch's own
/// [`has_open_session`](crate::app_handler::InputState::has_open_session) —
/// open *or* holding the grace window, since either still has a preedit to
/// retract. It is deliberately not the narrower `is_composing`, so the answer
/// does not depend on the caller having drained and settled the queue first.
pub fn teardown_signal(detached: bool, session_open: bool) -> Option<DomEditEvent> {
    (detached && session_open).then_some(DomEditEvent::Teardown)
}

/// The signal a clipboard edit owes a composition it interrupts.
///
/// A paste (or a cut) arriving while a session is still in flight displaces it:
/// the preedit is marked text the input method never committed, and the browser
/// will not commit it on the way out — a `paste` cancels the composition rather
/// than ending it with data. So the edit is preceded by the signal that retracts
/// the preedit, which is exactly what [`DomEditEvent::Cancel`] already means,
/// and the latch's own rules settle both states the same way a non-`insert*`
/// continuation does: an open session clears, and a grace window resolves as the
/// cancel it looked like. Feeding the two in order is what keeps the pasted text
/// from landing inside a marked region a later commit would replace.
///
/// `session_open` is the latch's own `has_open_session` — open *or* holding the
/// grace window, the same input [`teardown_signal`] takes and for the same
/// reason: either state still has a preedit on screen.
pub fn displaced_composition_signal(
    event: &DomEditEvent,
    session_open: bool,
) -> Option<DomEditEvent> {
    let clipboard = matches!(event, DomEditEvent::Paste(_) | DomEditEvent::EditCommand(_));
    (clipboard && session_open).then_some(DomEditEvent::Cancel)
}

/// The attributes the overlay element is created with for one content type.
///
/// The list is fixed except for what the focused field's hint decides, and it
/// is computed per `ImeOverlay::build` rather than held as a constant because
/// that hint is per-session state, not a property of the bridge.
///
/// # Why the hint is read through its predicates only
///
/// [`ImeContentType`] is `#[non_exhaustive]` and its own docs require security
/// behaviour to branch on [`ImeContentType::is_secret`] /
/// [`ImeContentType::suppresses_suggestions`] rather than on a variant match: a
/// `_` arm would silently hand a future secret variant a plain-text element.
/// Nothing here matches a variant at all, so a variant added upstream is
/// classified by its own predicates the moment it exists — the same fail-closed
/// rule the iOS and Android bridges apply to their wire strings, reached here by
/// having no fallback arm to get wrong.
///
/// # What each attribute is for
///
/// `type="password"` is the one that matters: it is what makes the browser
/// treat the element as secure entry, which is also what stops a mobile
/// keyboard offering the field's text as a suggestion or learning it. The
/// paired `autocomplete="new-password"` refuses autofill on a secret field,
/// where a plain `off` is widely ignored. `inputmode="text"` pins the plain
/// text keyboard for a field that refuses suggestions, so a browser cannot pick
/// a layout from the element type and bring its own suggestion strip with it.
/// The rest are the baseline that keeps the browser's text services off *every*
/// overlay — an autofill dropdown, an autocorrect bubble or a spellcheck
/// squiggle would render over the canvas anchored to an element the user cannot
/// see, and a suggestion strip would read text the app never showed it.
/// `tabindex="-1"` keeps the overlay out of the page's own tab order; it is
/// focused programmatically or not at all.
pub fn overlay_attributes(content_type: ImeContentType) -> Vec<(&'static str, &'static str)> {
    let secret = content_type.is_secret();
    let mut attributes = vec![
        ("id", OVERLAY_ID),
        ("type", if secret { "password" } else { "text" }),
        ("autocomplete", if secret { "new-password" } else { "off" }),
        ("autocorrect", "off"),
        ("autocapitalize", "off"),
        ("spellcheck", "false"),
        ("tabindex", "-1"),
    ];
    if content_type.suppresses_suggestions() {
        attributes.push(("inputmode", "text"));
    }
    attributes
}

/// Whether the live element must be replaced before it is used again.
///
/// `built` is the content type the element in the document was created with
/// (`None` when there is no element); `published` is what the focused field
/// says now. A browser decides secure entry — and the keyboard, suggestion and
/// autofill behaviour that rides on it — from the element it has, so a hint that
/// moved is answered with a fresh element rather than a mutated one: the
/// alternative is an element a browser has already classified as ordinary text
/// serving a field that has since declared itself secret.
pub fn overlay_needs_rebuild(built: Option<ImeContentType>, published: ImeContentType) -> bool {
    built.is_some_and(|built| built != published)
}

/// What the frame loop does with a live element whose content-type hint no
/// longer matches the focused field's — see [`stale_overlay_action`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleOverlay {
    /// The element still fits the field, or there is no element.
    Keep,
    /// Replace the element with one built for the published hint, and give
    /// the replacement the DOM focus the old one held.
    Replace,
    /// Remove the element and close the session; the next gesture or focus
    /// move opens a fresh one.
    Close,
}

/// The frame loop's answer to a content-type hint that moved with no input
/// event to carry it.
///
/// [`overlay_needs_rebuild`] is consulted from a dispatched event, but a focus
/// move a signal drove — a rebuild landing focus on an obscured field — reaches
/// the tree with no event at all, and until the next one arrived the secret
/// field would be served by an element the browser had already classified as
/// plain text. So the frame loop asks too, with one input the dispatch path
/// does not need: whether the old element actually holds DOM focus.
///
/// A replacement is focused only when the element it replaces was — that is
/// not a focus change, it is the same focus on a new element. A stale element
/// that has *lost* focus (the user tabbed out, or the `focus()` never took) is
/// closed instead: a frame never takes focus for the user, the same rule
/// [`OverlayPolicy::poll`] applies to opening a session.
pub fn stale_overlay_action(
    built: Option<ImeContentType>,
    published: ImeContentType,
    held_focus: bool,
) -> StaleOverlay {
    if !overlay_needs_rebuild(built, published) {
        StaleOverlay::Keep
    } else if held_focus {
        StaleOverlay::Replace
    } else {
        StaleOverlay::Close
    }
}

/// The content type a published surface asks for, defaulting to
/// [`ImeContentType::Normal`] when there is no surface to read one from.
pub fn published_content_type(ime: Option<&ImeState>) -> ImeContentType {
    ime.map(|state| state.content_type).unwrap_or_default()
}

/// Whether a keystroke should cancel the composition in progress instead of
/// being forwarded or composed.
pub fn cancels_composition(key: &str, latch_composing: bool) -> bool {
    latch_composing && key == "Escape"
}

/// Whether the focused widget wants an input method at all, from the surface it
/// published.
///
/// `RenderRoot` already turns an inactive surface into `None`, so this is the
/// one-line reading both halves of this module share rather than each
/// re-deciding what "no session" looks like.
pub fn session_is_active(ime: Option<&ImeState>) -> bool {
    ime.is_some_and(|state| state.active)
}

/// The text a `copy` or a `cut` may put on the clipboard when no widget answer
/// was handed to it, read straight off the surface the focused widget last
/// published — or `None` for the three refusals.
///
/// This is the second of [`clipboard_write_action`]'s two sources, and the one
/// that answers a gesture the tree never saw: a browser edit menu's copy, a
/// touch callout's cut. A gesture that did reach the tree brings its own text
/// through the [`ClipboardHandoff`] instead, which is authoritative where this
/// snapshot can be a step behind the edit the same gesture already applied.
///
/// Borrowed rather than owned so the per-frame refresh that feeds the listeners
/// allocates only when the answer actually changes.
///
/// # Why the shell slices this itself
///
/// A `copy`/`cut` callback has one chance to write the clipboard: Safari
/// honours `setData` from inside the event and nowhere else, and the answer
/// cannot wait for a dispatch into the tree and back. The published
/// [`ImeState`] is the one authoritative copy of the focused field's text and
/// selection the shell already holds, and its offsets are UTF-16 code units at
/// this seam (see [`EditingState`](frust_core::event::EditingState)), so the
/// slice is converted through [`utf16_to_byte`] rather than indexed directly —
/// an emoji ahead of the selection is two units and four bytes, and treating
/// one for the other would cut a character in half.
///
/// # The three refusals
///
/// * **No surface** — nothing is focused, so there is nothing to copy.
/// * **A secret field** — an obscured widget publishes its *real* text here (the
///   mask is a paint-time affair), so this is the guard that keeps a password
///   off the host clipboard, and it is the widget's own refusal made a second
///   time at the one boundary that cannot ask it. Read through
///   [`ImeContentType::is_secret`] for [`overlay_attributes`]'s fail-closed
///   reason: a secret variant added upstream is refused the day it exists.
/// * **A collapsed or absent selection** — copying nothing is not a copy, so
///   nothing is handed to `setData` and whatever the clipboard already held
///   stays there. The event is still cancelled, exactly as it is for a write
///   and for the secret refusal above: the browser's own default action would
///   copy the *element*, which is not reliably empty (see
///   [`ClipboardWriteAction::cancel`]).
pub fn clipboard_selection(ime: Option<&ImeState>) -> Option<&str> {
    let state = ime?;
    if state.content_type.is_secret() {
        return None;
    }
    let editing = &state.editing;
    // The `-1` sentinel is "no selection", and equal anchors are a caret.
    if editing.selection_base < 0
        || editing.selection_extent < 0
        || editing.selection_base == editing.selection_extent
    {
        return None;
    }
    // A selection dragged backwards reports its anchors reversed; the clipboard
    // wants the run of text, which has no direction.
    let first = editing.selection_base.min(editing.selection_extent) as usize;
    let last = editing.selection_base.max(editing.selection_extent) as usize;
    let start = utf16_to_byte(&editing.text, first);
    let end = utf16_to_byte(&editing.text, last);
    // `get` rather than an index: both offsets are already clamped to a char
    // boundary, and a surface that disagreed with its own text is a dropped
    // copy here rather than a panicked page.
    editing
        .text
        .get(start..end)
        .filter(|selected| !selected.is_empty())
}

/// What a `copy`/`cut` callback must do, decided from the two things it can
/// reach without asking the tree: the [`ClipboardHandoff`] the shell filled for
/// a gesture already in flight, and the selection snapshot it last published
/// ([`clipboard_selection`]).
///
/// The callback itself is DOM-only and so untestable on the build host; this
/// is its whole decision, lifted out where it can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardWriteAction<'a> {
    /// The text to hand `clipboardData.setData`, or `None` for a refusal —
    /// nothing handed over and no field focused, a collapsed selection, or a
    /// secret one.
    pub write: Option<&'a str>,
    /// Whether a `cut` still owes the widget the delete half of the gesture —
    /// the [`DomEditEvent::EditCommand`] its listener queues.
    ///
    /// True only for a write taken from the selection snapshot, which is what
    /// a gesture the tree never saw looks like: the browser resolved a cut of
    /// its own (an edit menu, a touch callout), the text is on the clipboard
    /// now, and nothing has deleted it yet.
    ///
    /// False for a handed-off write, and that is the whole reason the source
    /// is tracked: the tree filled the handoff by *answering* the gesture, so
    /// the deletion is already in the document and queuing a second `Cut`
    /// would edit the field twice for one keystroke.
    ///
    /// Meaningless to a `copy`, which queues nothing either way.
    pub delete_owed: bool,
    /// Whether the browser's own copy/cut must be cancelled.
    ///
    /// Always, and the refusal is why it is a field rather than an assumption
    /// at the call site. The browser's default action copies the *element*,
    /// and the element is not reliably empty: its value is cleared only while
    /// no composition owns it, so a live preedit — which a `type="password"`
    /// element can still carry — would go to the host clipboard as the answer
    /// to a copy this shell just refused. Cancelling a refusal costs nothing
    /// in every other case, because the overlay holds nothing worth copying by
    /// design.
    pub cancel: bool,
    /// Whether the callback owes a frame.
    ///
    /// Set exactly when something was written, because a write leaves the
    /// synchronous-write mark behind it and that mark is consumed by a drain
    /// ([`crate::app_handler::clipboard_write_route`]) — one that a copy would
    /// otherwise never get, since it changes no pixel and queues no signal,
    /// and the loop parks until something asks for a turn. A mark nobody comes
    /// to take is not a dedupe but a trap: the next app-driven copy of the
    /// same text would be read as that write's own echo and silently dropped.
    pub request_frame: bool,
}

/// The action a `copy`/`cut` callback takes, given whatever the shell handed
/// it for the gesture in flight and the selection it last published.
///
/// `handed_over` wins. It is the tree's own answer to *this* gesture — the
/// text the focused widget put in the clipboard slot when the re-dispatched
/// keystroke reached it — whereas `selection` is a snapshot that the same
/// gesture's edit may already have invalidated: a cut deletes its selection
/// before the browser raises `cut`, leaving the snapshot collapsed and this
/// function's only other source empty. Preferring the handoff is therefore
/// what keeps a cut from deleting text it never wrote out.
///
/// See [`ClipboardWriteAction`] for what each field means, why a refusal still
/// cancels, and why only the `selection` source leaves a delete owed.
pub fn clipboard_write_action<'a>(
    handed_over: Option<&'a str>,
    selection: Option<&'a str>,
) -> ClipboardWriteAction<'a> {
    let write = handed_over.or(selection);
    ClipboardWriteAction {
        write,
        delete_owed: handed_over.is_none() && write.is_some(),
        cancel: true,
        request_frame: write.is_some(),
    }
}

/// The one-slot handoff between the shell's clipboard drain and the overlay's
/// synchronous `copy`/`cut` write.
///
/// # What it is for
///
/// A clipboard keystroke is answered by the widget first and by the DOM
/// second, both inside one browser task (winit's web backend dispatches the
/// re-dispatched keystroke synchronously). The widget's answer — the text it
/// put in the tree's one-shot clipboard slot — is therefore already known when
/// the `copy`/`cut` callback runs, and it is the only description of the
/// gesture that a cut's own deletion has not invalidated. This carries it
/// across: the drain leaves the text here, the callback takes it, and
/// `navigator.clipboard.writeText` is left as the fallback for a gesture whose
/// event never arrives.
///
/// # The gesture mark
///
/// The drain cannot tell a keystroke's write from an app-driven one by looking
/// at the tree, so the keydown listener says so on the way past
/// ([`hands_write_to_dom_event`]) and the drain takes that mark. Without it an
/// app-driven copy — a toolbar button, with no DOM event coming for it ever —
/// would be parked here waiting for a callback that never runs, and issued a
/// frame late, outside the user gesture that a browser wants an async
/// clipboard write to ride.
///
/// Pure state, so the whole protocol is exercised on the build host; the
/// browser half only owns the [`RefCell`](std::cell::RefCell) around it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClipboardHandoff {
    gesture: bool,
    text: Option<String>,
}

impl ClipboardHandoff {
    /// An empty handoff with no gesture in flight.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a copy/cut keystroke as in flight, from the keydown listener that
    /// is about to re-dispatch it onto the canvas.
    pub fn mark_gesture(&mut self) {
        self.gesture = true;
    }

    /// Take the mark, for the drain deciding where this pass's clipboard write
    /// is owed.
    ///
    /// Taken rather than read on every drain, whether or not the tree wrote
    /// anything: a mark left standing would send the next app-driven copy —
    /// with no DOM event behind it — to a callback that never runs.
    pub fn take_gesture(&mut self) -> bool {
        std::mem::take(&mut self.gesture)
    }

    /// Leave `text` for the `copy`/`cut` callback still to come.
    ///
    /// The caller [`take`](Self::take)s first on the same pass, so nothing
    /// waiting is ever overwritten: a handoff the DOM did not come for leaves
    /// as the asynchronous fallback before this one arrives.
    pub fn offer(&mut self, text: String) {
        self.text = Some(text);
    }

    /// Take whatever is waiting — the callback claiming it for its synchronous
    /// write, or the next drain finding it unclaimed and issuing the
    /// asynchronous fallback instead.
    ///
    /// One method for both because the slot cannot tell them apart and must
    /// not: what makes a text the callback's is only that the callback got
    /// here first, inside the gesture, which is exactly when it can write.
    pub fn take(&mut self) -> Option<String> {
        self.text.take()
    }
}

#[cfg(target_arch = "wasm32")]
pub use browser::{ImeOverlay, PasteSink};

/// The browser half: the element itself, its listeners, and the queue they feed.
///
/// Gated rather than stubbed for the same reason [`crate::app_handler`]'s frame
/// loop is: everything in here owns a browser resource, and every decision it
/// makes was lifted out into the pure functions above, which are tested on the
/// build host.
#[cfg(target_arch = "wasm32")]
mod browser {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::sync::Arc;

    use frust_core::event::{EditCommand, ImeContentType, ImeState};
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{
        ClipboardEvent, CompositionEvent, Element, HtmlCanvasElement, HtmlInputElement,
        KeyboardEvent, KeyboardEventInit,
    };
    use winit::platform::web::WindowExtWebSys;
    use winit::window::Window;

    use super::{
        ClipboardHandoff, DomEditEvent, OverlayAction, OverlayBox, OverlayPolicy, OverlaySync,
        StaleOverlay, cancels_composition, clipboard_selection, clipboard_write_action,
        forwards_to_canvas, hands_write_to_dom_event, key_path_dropped, overlay_attributes,
        overlay_box, overlay_box_moved, overlay_needs_rebuild, published_content_type,
        session_is_active, stale_overlay_action, withheld_from_key_path,
    };

    /// The style declarations the overlay is created with, beyond its per-frame
    /// geometry.
    ///
    /// Invisible (`opacity`), inert to the pointer (`pointer-events`, so a
    /// click over the caret still reaches the canvas), and stripped of every
    /// default `<input>` decoration that would otherwise paint over the app.
    /// `font-size` is the one value that is not cosmetic: a mobile browser
    /// zooms the page when focus lands on an input whose text would render
    /// below 16px, which would resize the canvas out from under the shell.
    const OVERLAY_STYLE: &[(&str, &str)] = &[
        ("position", "fixed"),
        ("opacity", "0"),
        ("pointer-events", "none"),
        ("z-index", "0"),
        ("border", "none"),
        ("outline", "none"),
        ("padding", "0"),
        ("margin", "0"),
        ("background", "transparent"),
        ("color", "transparent"),
        ("caret-color", "transparent"),
        ("overflow", "hidden"),
        ("resize", "none"),
        ("font-size", "16px"),
    ];

    /// One registered DOM listener, kept alive for as long as the element is.
    ///
    /// Every listener is typed `FnMut(web_sys::Event)` and narrows the event
    /// itself, so they share one closure type and one removal path instead of
    /// needing a field per event name.
    struct Listener {
        name: &'static str,
        closure: Closure<dyn FnMut(web_sys::Event)>,
    }

    /// The signals the listeners publish to the shell, shared by `Rc` because a
    /// DOM closure outlives the call that installed it.
    #[derive(Default)]
    struct Signals {
        queue: RefCell<VecDeque<DomEditEvent>>,
        blurred: Cell<bool>,
        /// What a `copy`/`cut` callback would put on the clipboard: the focused
        /// field's selection as of the last dispatch or frame
        /// ([`clipboard_selection`]), `None` for each of its refusals.
        ///
        /// Pushed here rather than pulled from the tree because the callback
        /// cannot reach the tree: it must write the clipboard before it returns
        /// (Safari's rule), and a round trip through a dispatch is not
        /// available inside a DOM event.
        selection: RefCell<Option<String>>,
        /// The text such a callback last wrote *synchronously*, held for the one
        /// drain that follows it — the drain the write itself asks for, so that
        /// there is always one.
        ///
        /// What it guards is the echo of a cut the *DOM* resolved on its own: a
        /// browser edit menu's cut writes the clipboard from inside the
        /// callback and then asks the widget to delete what was taken, and the
        /// widget answers a `Cut` by writing the same text into the tree's own
        /// clipboard slot. Handing that back to
        /// `navigator.clipboard.writeText` would re-write text already on the
        /// clipboard, from outside the gesture that authorised it — refused on
        /// some browsers, pointless on all of them. So the shell compares the
        /// two and skips the redundant half.
        ///
        /// A keystroke's own two halves need no such comparison: the tree's
        /// write reaches the callback through [`Signals::handoff`] instead of
        /// being issued beside it, so there is never a second write to
        /// suppress.
        synchronous_write: RefCell<Option<String>>,
        /// The bridge carrying a keystroke's own clipboard write from the drain
        /// that took it out of the tree to the `copy`/`cut` callback that can
        /// write it synchronously — see [`ClipboardHandoff`], which holds the
        /// whole protocol and its reasoning.
        handoff: RefCell<ClipboardHandoff>,
    }

    /// The hidden-input IME overlay: at most one element, alive for exactly as
    /// long as the focused widget wants an input method.
    #[derive(Default)]
    pub struct ImeOverlay {
        policy: OverlayPolicy,
        element: Option<HtmlInputElement>,
        listeners: Vec<Listener>,
        signals: Rc<Signals>,
        last_box: Option<OverlayBox>,
        /// The content type the live element was created with, `None` when
        /// there is no element — the left half of [`overlay_needs_rebuild`].
        built_content_type: Option<ImeContentType>,
    }

    impl ImeOverlay {
        /// A closed overlay with no element and nothing queued.
        pub fn new() -> Self {
            Self::default()
        }

        /// Take the next DOM signal the listeners queued, oldest first.
        ///
        /// Drained by the shell before it dispatches anything else, so a
        /// composition and the keystrokes around it reach the tree in the order
        /// the browser produced them.
        pub fn next_event(&mut self) -> Option<DomEditEvent> {
            self.signals.queue.borrow_mut().pop_front()
        }

        /// Take the text a `copy`/`cut` listener already wrote to the clipboard
        /// itself, so the shell can tell a write the tree made on its own apart
        /// from the echo of that one — see [`Signals::synchronous_write`].
        ///
        /// Taken on every drain whether or not the tree wrote anything, and
        /// every synchronous write asks for the drain that takes it: together
        /// those bound the mark to exactly one drain, so a later identical copy
        /// is issued as the genuine write it is rather than skipped for a note
        /// an earlier gesture left. A teardown deliberately does not clear it —
        /// the element going away does not un-write the clipboard, and the
        /// `Cut` it queued is still in the queue behind it.
        pub fn take_synchronous_write(&self) -> Option<String> {
            self.signals.synchronous_write.borrow_mut().take()
        }

        /// Take the mark a copy/cut keystroke left on its way to the canvas:
        /// whether the write this drain is about to find in the tree belongs to
        /// a DOM callback still to come — see [`ClipboardHandoff`].
        pub fn take_clipboard_gesture(&self) -> bool {
            self.signals.handoff.borrow_mut().take_gesture()
        }

        /// Leave the tree's clipboard write for the `copy`/`cut` callback that
        /// will put it on the host clipboard synchronously, and ask for the
        /// frame that issues the asynchronous fallback if no callback comes.
        pub fn hand_off_clipboard_write(&self, window: &Arc<Window>, text: String) {
            self.signals.handoff.borrow_mut().offer(text);
            window.request_redraw();
        }

        /// Take back a handed-off write no `copy`/`cut` callback claimed — the
        /// gesture is over, so the text is owed to
        /// `navigator.clipboard.writeText` after all.
        ///
        /// Called at the top of every drain, so the fallback is decided on the
        /// first turn after the gesture: the keystroke's own `keyup`, or the
        /// frame the handoff asked for.
        pub fn take_unclaimed_clipboard_write(&self) -> Option<String> {
            self.signals.handoff.borrow_mut().take()
        }

        /// A handle that can queue a [`DomEditEvent::Paste`] after this borrow
        /// has ended — what an asynchronous `navigator.clipboard.readText()`
        /// resolves into. See [`PasteSink`].
        pub fn paste_sink(&self, window: &Arc<Window>) -> PasteSink {
            PasteSink {
                signals: Rc::clone(&self.signals),
                window: Arc::clone(window),
            }
        }

        /// Refresh what a `copy`/`cut` callback would write, from the surface
        /// the focused widget published this pass.
        ///
        /// Runs from both lifecycle paths (a dispatched event and the frame
        /// loop), because a selection moves under either: a drag reaches the
        /// tree as an event, a signal-driven select-all does not. The guard is
        /// not an optimization but a bound on cost — a field's selected text
        /// would otherwise be cloned on every frame a session is open, for a
        /// value that changes only when the user moves the selection.
        fn publish_selection(&self, ime: Option<&ImeState>) {
            let selection = clipboard_selection(ime);
            let mut slot = self.signals.selection.borrow_mut();
            if slot.as_deref() != selection {
                *slot = selection.map(str::to_owned);
            }
        }

        /// Reconcile the element against the focused widget's published
        /// surface, for one dispatched input event.
        ///
        /// `gesture` marks a pointer press and `composing` is the shell's
        /// compose latch — the element is emptied whenever no composition owns
        /// it, so a session never starts on top of the last one's text.
        ///
        /// The returned [`OverlaySync::detached`] is what the caller feeds to
        /// [`teardown_signal`](super::teardown_signal): both a close and the
        /// rebuild a content-type change forces remove a live element, and a
        /// composition the latch still holds has to be ended with it.
        pub fn sync(
            &mut self,
            window: &Arc<Window>,
            ime: Option<&ImeState>,
            generation: u64,
            gesture: bool,
            composing: bool,
        ) -> OverlaySync {
            // A blur that already tore the session down is observed before the
            // policy runs, so this pass sees the true state of the DOM rather
            // than re-placing an element the user has left. The blur itself is
            // queued for the latch, so this teardown owes it no signal.
            if self.signals.blurred.replace(false) {
                self.teardown();
                self.policy.mark_closed();
            }

            // Ahead of the lifecycle below, so a close leaves the listeners of
            // an element that is about to go away with nothing to copy.
            self.publish_selection(ime);

            let mut detached = false;
            let action = self
                .policy
                .poll(session_is_active(ime), generation, gesture);
            match action {
                OverlayAction::Idle => {}
                OverlayAction::Open => {
                    // No content-type check is owed here: every path that
                    // leaves the policy closed (a blur, a failed create, a
                    // close) removes the element first, so an opening session
                    // never inherits one built for a different hint.
                    if self.element.is_none() {
                        self.build(window, ime);
                    }
                    if self.element.is_none() {
                        // The DOM refused the element (no body yet, a create
                        // that failed). Give the session back so a later
                        // gesture opens a fresh one rather than leaving the
                        // policy believing an element it does not have is live.
                        self.policy.mark_closed();
                        return OverlaySync {
                            action: OverlayAction::Idle,
                            detached,
                        };
                    }
                    self.place(window, ime);
                    self.clear_value(composing);
                    self.focus();
                }
                OverlayAction::Update { refocus } => {
                    // A field that changed its content type needs the browser
                    // to re-read it, which it only does for an element it has
                    // not classified yet. The replacement always takes focus
                    // back, `refocus` or not: the element that held it is gone.
                    let rebuilt =
                        overlay_needs_rebuild(self.built_content_type, published_content_type(ime));
                    if rebuilt {
                        self.teardown();
                        detached = true;
                        self.build(window, ime);
                        if self.element.is_none() {
                            self.policy.mark_closed();
                            return OverlaySync {
                                action: OverlayAction::Idle,
                                detached,
                            };
                        }
                    }
                    self.place(window, ime);
                    self.clear_value(composing);
                    if refocus || rebuilt {
                        self.focus();
                    }
                }
                OverlayAction::Close => {
                    detached = self.element.is_some();
                    self.teardown();
                }
            }
            OverlaySync { action, detached }
        }

        /// The frame loop's half of the lifecycle: drop a session the app ended
        /// without an input event of its own. Returns whether it closed one —
        /// the caller pairs that with [`teardown_signal`](super::teardown_signal).
        pub fn close_if_inactive(&mut self, ime: Option<&ImeState>) -> bool {
            let closing = self.policy.close_if_inactive(session_is_active(ime));
            if closing {
                self.teardown();
            }
            closing
        }

        /// The frame loop's half of the content-type contract: replace an
        /// element built for a hint the focused field no longer publishes, or
        /// close it when it no longer holds focus — see
        /// [`stale_overlay_action`](super::stale_overlay_action). Returns
        /// whether a live element was removed, for
        /// [`teardown_signal`](super::teardown_signal).
        ///
        /// Runs before [`ImeOverlay::reposition`], so the frame places the
        /// replacement rather than the element it removed.
        pub fn replace_if_stale(&mut self, window: &Arc<Window>, ime: Option<&ImeState>) -> bool {
            if !session_is_active(ime) {
                return false;
            }
            let Some(element) = self.element.as_ref() else {
                return false;
            };
            let action = stale_overlay_action(
                self.built_content_type,
                published_content_type(ime),
                holds_dom_focus(element),
            );
            match action {
                StaleOverlay::Keep => false,
                StaleOverlay::Replace => {
                    self.teardown();
                    self.build(window, ime);
                    if self.element.is_none() {
                        // Same as a failed open: hand the session back so a
                        // later gesture tries again.
                        self.policy.mark_closed();
                    } else {
                        self.place(window, ime);
                        self.focus();
                    }
                    true
                }
                StaleOverlay::Close => {
                    self.teardown();
                    self.policy.mark_closed();
                    true
                }
            }
        }

        /// Follow the caret while a session is open, and drop any value the
        /// element is still holding. Focus is left alone.
        ///
        /// The frame loop's other half: composition produces no winit event, so
        /// a preedit growing under the user moves the caret with no dispatch to
        /// re-place the element from — and a candidate window left behind at
        /// the caret's old position is the visible symptom. The emptying runs
        /// here too, not only from [`ImeOverlay::sync`], because the last
        /// keystroke of a session (or a paste into it) is followed by no
        /// further dispatched event: without this pass its characters sit in
        /// the DOM value for as long as the field stays focused, where a
        /// suggestion strip can still read them. Focus is deliberately not
        /// asserted here; see [`OverlayPolicy::poll`] for why a frame may never
        /// take it.
        pub fn reposition(
            &mut self,
            window: &Arc<Window>,
            ime: Option<&ImeState>,
            composing: bool,
        ) {
            self.publish_selection(ime);
            if self.element.is_none() || !session_is_active(ime) {
                return;
            }
            self.place(window, ime);
            self.clear_value(composing);
        }

        /// Whether an element is currently in the document.
        pub fn is_attached(&self) -> bool {
            self.element.is_some()
        }

        /// Create the element, style it, install its listeners and append it to
        /// the document body.
        ///
        /// The attributes come from the focused field's own content-type hint
        /// ([`overlay_attributes`]) rather than from a constant, which is why
        /// the published surface is threaded in here: `type="password"` is only
        /// read by a browser when it classifies the element, so it has to be
        /// set on creation.
        ///
        /// Every step is fallible in a DOM that may not have a body yet; a
        /// failure logs and leaves `element` unset rather than panicking a
        /// page, and [`ImeOverlay::sync`] hands the session back so a later
        /// gesture tries again.
        fn build(&mut self, window: &Arc<Window>, ime: Option<&ImeState>) {
            let Some(document) = web_sys::window().and_then(|w| w.document()) else {
                log::debug!("frust-shell-web: no document to host the IME overlay");
                return;
            };
            let Some(body) = document.body() else {
                log::debug!("frust-shell-web: no document body to host the IME overlay");
                return;
            };
            let Ok(element) = document.create_element("input") else {
                log::debug!("frust-shell-web: could not create the IME overlay element");
                return;
            };
            let Ok(element) = element.dyn_into::<HtmlInputElement>() else {
                log::debug!("frust-shell-web: the IME overlay element is not an input");
                return;
            };

            let content_type = published_content_type(ime);
            for (name, value) in overlay_attributes(content_type) {
                let _ = element.set_attribute(name, value);
            }
            let style = element.style();
            for (name, value) in OVERLAY_STYLE {
                let _ = style.set_property(name, value);
            }
            if body.append_child(&element).is_err() {
                log::debug!("frust-shell-web: could not attach the IME overlay element");
                return;
            }

            self.install_listeners(&element, window);
            self.element = Some(element);
            self.built_content_type = Some(content_type);
            self.last_box = None;
        }

        /// Wire every DOM signal the bridge reads onto `element`.
        ///
        /// Each listener converts its event to a [`DomEditEvent`] and asks for
        /// one frame: a composition produces no winit event at all, so without
        /// that request the loop — which parks on `ControlFlow::Wait` — would
        /// never wake to apply it. The `copy` listener is the one that queues
        /// nothing — the clipboard is written inside the callback and the
        /// document does not change — but it asks for the frame all the same,
        /// because that turn is what takes the mark its write leaves behind
        /// (see `write_selection`). The two `before*` listeners are the only
        /// ones that neither queue nor ask for anything.
        fn install_listeners(&mut self, element: &HtmlInputElement, window: &Arc<Window>) {
            let canvas = window.canvas();

            self.add_listener(element, "compositionstart", {
                let signals = Rc::clone(&self.signals);
                let window = Arc::clone(window);
                move |_event| {
                    push(&signals, DomEditEvent::CompositionStart, &window);
                }
            });
            self.add_listener(element, "compositionupdate", {
                let signals = Rc::clone(&self.signals);
                let window = Arc::clone(window);
                move |event| {
                    let data = composition_data(&event);
                    push(&signals, DomEditEvent::CompositionUpdate { data }, &window);
                }
            });
            self.add_listener(element, "compositionend", {
                let signals = Rc::clone(&self.signals);
                let window = Arc::clone(window);
                move |event| {
                    let data = composition_data(&event);
                    push(&signals, DomEditEvent::CompositionEnd { data }, &window);
                }
            });
            self.add_listener(element, "input", {
                let signals = Rc::clone(&self.signals);
                let window = Arc::clone(window);
                move |event| {
                    let Some(input) = event.dyn_ref::<web_sys::InputEvent>() else {
                        return;
                    };
                    push(
                        &signals,
                        DomEditEvent::Input {
                            input_type: input.input_type(),
                            data: input.data(),
                            is_composing: input.is_composing(),
                        },
                        &window,
                    );
                }
            });
            self.add_listener(element, "blur", {
                let signals = Rc::clone(&self.signals);
                let window = Arc::clone(window);
                move |_event| {
                    signals.blurred.set(true);
                    push(&signals, DomEditEvent::Blur, &window);
                }
            });

            // The clipboard trio. A browser delivers these to the focused
            // editable element and to nothing else, which is why they hang off
            // the overlay rather than off the canvas or the document — see the
            // module doc's clipboard section.
            self.add_listener(element, "paste", {
                let signals = Rc::clone(&self.signals);
                let window = Arc::clone(window);
                move |event| {
                    // Cancel the browser's own insertion first, and whatever
                    // the read below answers: the overlay's value is thrown
                    // away rather than read, and an `insertFromPaste` `input`
                    // behind this would be taken for a keystroke by the carry
                    // rule. A paste this bridge cannot carry is a dropped
                    // paste, never one the element applies to itself.
                    event.prevent_default();
                    let Some(text) = pasted_text(&event) else {
                        return;
                    };
                    // The text reaches the tree from here instead.
                    push(&signals, DomEditEvent::Paste(text), &window);
                }
            });
            self.add_listener(element, "copy", {
                let signals = Rc::clone(&self.signals);
                let window = Arc::clone(window);
                move |event| {
                    // Nothing is queued: the selection is already on the
                    // clipboard and the document is unchanged. The frame this
                    // asks for is not for the document but for the mark the
                    // write leaves — see `write_selection`.
                    write_selection(&signals, &event, &window);
                }
            });
            self.add_listener(element, "cut", {
                let signals = Rc::clone(&self.signals);
                let window = Arc::clone(window);
                move |event| {
                    // Only a write that actually happened may delete anything —
                    // a refused cut (a secret field, no selection) leaves the
                    // document alone as well as the clipboard — and only one
                    // the *DOM* resolved on its own: a write handed over by the
                    // shell is the answer to a gesture the widget has already
                    // applied, so asking for the delete again would cut twice.
                    if write_selection(&signals, &event, &window) {
                        push(
                            &signals,
                            DomEditEvent::EditCommand(EditCommand::Cut),
                            &window,
                        );
                    }
                }
            });

            // The two enablement queries behind them. A browser asks whether
            // the page will handle a copy/cut before it decides the command is
            // available at all — when it opens an edit menu, when it validates
            // a chord — and a cancelled query is the page saying it will. That
            // is the only answer that enables the command here, since the
            // element's selection is collapsed at every instant a query can
            // arrive, and it is what lets the `copy`/`cut` listeners above run
            // in Blink and WebKit. Gecko raises neither event (it defines no
            // such name), so this widens the engines that reach the DOM route
            // without being load-bearing for any of them: a copy or a cut the
            // DOM never reports still arrives through the key path.
            for name in ["beforecopy", "beforecut"] {
                self.add_listener(element, name, |event| {
                    // Unconditional: the query carries no selection of its own
                    // to judge, and refusing the verb here would only hand it
                    // back to a default action that copies the element.
                    event.prevent_default();
                });
            }

            // The two key listeners carry the forwarding contract: winit's own
            // handlers sit on the canvas, which a focused overlay is not a
            // descendant of, so a keystroke reaches them only if it is
            // re-dispatched there.
            self.add_listener(element, "keydown", {
                let signals = Rc::clone(&self.signals);
                let window = Arc::clone(window);
                let canvas = canvas.clone();
                move |event| {
                    let Some(key_event) = event.dyn_ref::<KeyboardEvent>() else {
                        return;
                    };
                    let key = key_event.key();
                    let composing = key_event.is_composing();
                    let ctrl = key_event.ctrl_key();
                    let meta = key_event.meta_key();
                    let shift = key_event.shift_key();
                    if withheld_from_key_path(&key, ctrl, meta, shift) {
                        // A paste gesture: the DOM raises `paste` for it
                        // reliably and the listener above answers it, so
                        // forwarding it as well would hand a widget the same
                        // verb a second time. A copy or a cut is not withheld
                        // — that event may never come. Deliberately not
                        // cancelled: the browser's own default action is what
                        // fires the `paste` at all.
                        return;
                    }
                    if cancels_composition(&key, composing) {
                        push(&signals, DomEditEvent::Cancel, &window);
                        return;
                    }
                    if forwards_to_canvas(&key, composing) {
                        if hands_write_to_dom_event(&key, ctrl, meta, shift) {
                            // A copy or a cut, about to reach the widget that
                            // answers it — which happens before this listener
                            // returns, and so before the browser raises the
                            // `copy`/`cut` this marks as the one that will do
                            // the writing. The drain behind the widget's answer
                            // hands the text to that callback rather than
                            // issuing it asynchronously beside it: one gesture,
                            // one write, and a cut's write on the only route
                            // that needs no secure context. See
                            // `ClipboardHandoff`.
                            //
                            // Marked here rather than above so it describes a
                            // keystroke the widget really is about to see: one
                            // the composition owns reaches no widget, and a DOM
                            // event raised for it is a gesture the tree never
                            // saw, answered from the published selection.
                            signals.handoff.borrow_mut().mark_gesture();
                        }
                        redispatch(canvas.as_ref(), "keydown", key_event);
                    } else if key_path_dropped(&key, composing) {
                        // Queued rather than flagged: the `input` that carries
                        // this keystroke's edit is queued too, and the mark is
                        // only honoured by the signal immediately behind it.
                        push(&signals, DomEditEvent::KeyDropped, &window);
                    }
                }
            });
            // No clipboard exclusion here: a release carries no editing
            // semantics at all on this shell (`map_key_event` maps only
            // `Pressed`), so the copy of a `Cmd`+`v` keyup reaches winit and
            // becomes nothing.
            self.add_listener(element, "keyup", {
                let canvas = canvas.clone();
                move |event| {
                    let Some(key_event) = event.dyn_ref::<KeyboardEvent>() else {
                        return;
                    };
                    if forwards_to_canvas(&key_event.key(), key_event.is_composing()) {
                        redispatch(canvas.as_ref(), "keyup", key_event);
                    }
                }
            });
        }

        /// Register one closure as a DOM listener and keep it alive.
        fn add_listener<F>(&mut self, element: &HtmlInputElement, name: &'static str, handler: F)
        where
            F: 'static + FnMut(web_sys::Event),
        {
            let closure = Closure::<dyn FnMut(web_sys::Event)>::new(handler);
            if element
                .add_event_listener_with_callback(name, closure.as_ref().unchecked_ref())
                .is_err()
            {
                log::debug!("frust-shell-web: could not listen for {name} on the IME overlay");
                return;
            }
            self.listeners.push(Listener { name, closure });
        }

        /// Move the element over the focused field's caret, skipping the style
        /// write when it has not meaningfully moved.
        fn place(&mut self, window: &Arc<Window>, ime: Option<&ImeState>) {
            let Some(element) = self.element.as_ref() else {
                return;
            };
            let origin = window
                .canvas()
                .map(|canvas| {
                    let rect = canvas.get_bounding_client_rect();
                    (rect.left(), rect.top())
                })
                .unwrap_or((0.0, 0.0));
            let placed = overlay_box(ime.and_then(|state| state.caret), origin.0, origin.1);
            if !overlay_box_moved(self.last_box, placed) {
                return;
            }
            let style = element.style();
            let _ = style.set_property("left", &format!("{}px", placed.left));
            let _ = style.set_property("top", &format!("{}px", placed.top));
            let _ = style.set_property("width", &format!("{}px", placed.width));
            let _ = style.set_property("height", &format!("{}px", placed.height));
            self.last_box = Some(placed);
        }

        /// Empty the element unless a composition is mid-flight, so the next
        /// session starts from nothing and no mobile suggestion strip has text
        /// to mine.
        fn clear_value(&self, composing: bool) {
            if composing {
                return;
            }
            if let Some(element) = self.element.as_ref()
                && !element.value().is_empty()
            {
                element.set_value("");
            }
        }

        /// Take DOM focus, which is what arms the browser's input method and,
        /// inside a user gesture, opens a mobile soft keyboard.
        fn focus(&self) {
            if let Some(element) = self.element.as_ref()
                && element.focus().is_err()
            {
                log::debug!("frust-shell-web: the IME overlay refused focus");
            }
        }

        /// Remove the element and every listener on it, and forget the placed
        /// box and the content type it was built with so a later session
        /// re-writes both from scratch.
        ///
        /// Listeners come off **before** the element does, so detaching a
        /// focused overlay cannot re-enter this bridge through its own `blur`
        /// handler and mark a session that is already gone as blurred. That
        /// ordering is also why this raises nothing the compose latch could
        /// observe: a caller that tears an element down while a composition is
        /// open owes the latch [`DomEditEvent::Teardown`], which
        /// [`teardown_signal`](super::teardown_signal) decides.
        fn teardown(&mut self) {
            if let Some(element) = self.element.take() {
                for listener in &self.listeners {
                    let _ = element.remove_event_listener_with_callback(
                        listener.name,
                        listener.closure.as_ref().unchecked_ref(),
                    );
                }
                element.remove();
            }
            self.listeners.clear();
            self.built_content_type = None;
            self.last_box = None;
            self.signals.blurred.set(false);
            // The listeners that would have read it are gone; a fresh session
            // publishes its own before its first event.
            *self.signals.selection.borrow_mut() = None;
        }
    }

    impl Drop for ImeOverlay {
        /// A page that drops the shell must not leave an orphaned `<input>` (and
        /// its listeners) behind in the document.
        ///
        /// Unlike the other two teardown sites this one owes the compose latch
        /// nothing: the overlay is a field of the shell handler, so it is only
        /// ever dropped with the handler — and with it the latch that would
        /// have held the session and the widget tree that would have shown the
        /// preedit.
        fn drop(&mut self) {
            self.teardown();
        }
    }

    /// Whether `element` is the document's active element — the one input
    /// [`stale_overlay_action`] needs that only the DOM can answer.
    fn holds_dom_focus(element: &HtmlInputElement) -> bool {
        web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.active_element())
            .is_some_and(|active| &active == AsRef::<Element>::as_ref(element))
    }

    /// A handle onto the overlay's signal queue that outlives the borrow it was
    /// taken from.
    ///
    /// The one asynchronous edge in this bridge: a paste the *tree* asked for
    /// (a toolbar tap, an edit menu) is answered by
    /// `navigator.clipboard.readText()`, whose promise resolves in a later task,
    /// long after the `&mut ImeOverlay` that started it is gone. Shaped as a
    /// handle rather than a callback so the queue stays the single ordering
    /// point: the resolved text is queued exactly like a `paste` listener's,
    /// asks for the frame that drains it, and reaches the tree through the same
    /// path.
    ///
    /// It outlives the element, too, and deliberately: a session torn down
    /// between the request and its answer leaves the queue standing, and the
    /// paste is delivered to whatever holds focus then — the same thing that
    /// happens to a clipboard read on every other shell.
    pub struct PasteSink {
        signals: Rc<Signals>,
        window: Arc<Window>,
    }

    impl PasteSink {
        /// Queue the text a clipboard read resolved to, and ask for the frame
        /// that drains it.
        pub fn deliver(&self, text: String) {
            push(&self.signals, DomEditEvent::Paste(text), &self.window);
        }
    }

    /// Queue one signal and ask for the frame that applies it.
    fn push(signals: &Rc<Signals>, event: DomEditEvent, window: &Arc<Window>) {
        signals.queue.borrow_mut().push_back(event);
        window.request_redraw();
    }

    /// The plain text a `paste` event carries, or `None` when the browser
    /// offered none.
    ///
    /// `text/plain` only, deliberately: the framework's whole paste vocabulary
    /// is [`EditCommand::Paste`](frust_core::event::EditCommand::Paste), which
    /// carries a `String`, so asking for `text/html` would produce something no
    /// widget could apply. An empty string is a real answer (the clipboard held
    /// nothing, or nothing textual) and is carried through — whether it is worth
    /// inserting is the receiving widget's rule.
    ///
    /// This read needs no permission and no promise: the text is on the event
    /// because the user's own gesture put it there.
    fn pasted_text(event: &web_sys::Event) -> Option<String> {
        let Some(clipboard) = event.dyn_ref::<ClipboardEvent>() else {
            return None;
        };
        let Some(data) = clipboard.clipboard_data() else {
            log::debug!("frust-shell-web: a paste arrived with no clipboardData; it is dropped");
            return None;
        };
        match data.get_data("text/plain") {
            Ok(text) => Some(text),
            Err(err) => {
                log::warn!(
                    "frust-shell-web: the browser refused the pasted text ({err:?}); \
                     the paste is dropped"
                );
                None
            }
        }
    }

    /// Put this gesture's text on the clipboard from inside a `copy`/`cut`
    /// callback, reporting whether a `cut` must still ask the widget to delete
    /// what was taken.
    ///
    /// Everything here has to finish before the callback returns — that is the
    /// only window in which Safari honours a clipboard write — so neither
    /// source can be the tree itself: the text is either the one the shell
    /// handed over for the gesture already in flight ([`ClipboardHandoff`]) or
    /// the snapshot it republishes each pass
    /// ([`ImeOverlay::publish_selection`]). The decision between them is
    /// [`clipboard_write_action`]'s, where it is testable on the build host;
    /// this is the DOM half that carries it out.
    ///
    /// The event is cancelled on every path, refusals included: the browser's
    /// own copy takes the *element*, which holds a live preedit whenever a
    /// composition owns it, so leaving the default action to run is what would
    /// put a secret field's marked text on the host clipboard. A write also
    /// asks for a frame, which is the drain that takes the mark it leaves.
    ///
    /// A handed-off write that the browser then refuses is put back rather
    /// than dropped: the widget has already deleted it, and the drain behind
    /// this callback is what offers it to `navigator.clipboard.writeText`
    /// instead. Losing it here is the one outcome a cut may not have.
    fn write_selection(
        signals: &Rc<Signals>,
        event: &web_sys::Event,
        window: &Arc<Window>,
    ) -> bool {
        let handed_over = signals.handoff.borrow_mut().take();
        let selection = signals.selection.borrow().clone();
        let action = clipboard_write_action(handed_over.as_deref(), selection.as_deref());
        if action.cancel {
            event.prevent_default();
        }
        let Some(text) = action.write else {
            return false;
        };
        let restore = |signals: &Rc<Signals>| {
            if let Some(text) = handed_over.clone() {
                signals.handoff.borrow_mut().offer(text);
                // The drain that takes it back has to come from somewhere: this
                // callback may be the last thing in the task.
                window.request_redraw();
            }
        };
        let Some(clipboard) = event.dyn_ref::<ClipboardEvent>() else {
            restore(signals);
            return false;
        };
        let Some(data) = clipboard.clipboard_data() else {
            log::debug!(
                "frust-shell-web: a copy/cut arrived with no clipboardData; nothing is written \
                 and the browser's own action stays cancelled"
            );
            restore(signals);
            return false;
        };
        if let Err(err) = data.set_data("text/plain", text) {
            log::warn!(
                "frust-shell-web: the browser refused the clipboard write ({err:?}); \
                 a write handed over for a keystroke falls back to the asynchronous \
                 clipboard, and a copy read off the selection is lost"
            );
            restore(signals);
            return false;
        }
        *signals.synchronous_write.borrow_mut() = Some(text.to_owned());
        if action.request_frame {
            // Not for anything to paint: this is the turn on which the shell
            // takes the mark above, and a copy queues no signal that would
            // otherwise ask for one.
            window.request_redraw();
        }
        action.delete_owed
    }

    /// The marked/committed text off a composition event, empty when the
    /// browser reports none (a cancel).
    fn composition_data(event: &web_sys::Event) -> String {
        event
            .dyn_ref::<CompositionEvent>()
            .and_then(CompositionEvent::data)
            .unwrap_or_default()
    }

    /// Re-raise `event` on the canvas, where winit's own keyboard listener maps
    /// it into `WindowEvent::KeyboardInput`.
    ///
    /// A faithful copy of every field winit reads (`key`, `code`, `location`,
    /// `repeat` and the four modifier flags), because that mapping — not this
    /// crate — decides what the keystroke becomes. The copy does not bubble:
    /// winit's listener is on the canvas itself, and letting it climb further
    /// would hand the same keystroke to the document twice.
    fn redispatch(canvas: Option<&HtmlCanvasElement>, name: &str, event: &KeyboardEvent) {
        let Some(canvas) = canvas else {
            return;
        };
        let init = KeyboardEventInit::new();
        init.set_key(&event.key());
        init.set_code(&event.code());
        init.set_location(event.location());
        init.set_repeat(event.repeat());
        init.set_ctrl_key(event.ctrl_key());
        init.set_shift_key(event.shift_key());
        init.set_alt_key(event.alt_key());
        init.set_meta_key(event.meta_key());
        init.set_bubbles(false);
        init.set_cancelable(true);
        let Ok(copy) = KeyboardEvent::new_with_keyboard_event_init_dict(name, &init) else {
            return;
        };
        let _ = canvas.dispatch_event(&copy);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClipboardHandoff, ClipboardVerb, DomEditEvent, MIN_OVERLAY_SIDE, OverlayAction, OverlayBox,
        OverlayPolicy, StaleOverlay, cancels_composition, clipboard_selection, clipboard_verb,
        clipboard_write_action, displaced_composition_signal, forwards_to_canvas,
        hands_write_to_dom_event, key_path_dropped, overlay_attributes, overlay_box,
        overlay_box_moved, overlay_needs_rebuild, published_content_type, session_is_active,
        stale_overlay_action, teardown_signal, withheld_from_key_path,
    };
    use frust_core::event::{EditCommand, EditingState, ImeContentType, ImeState};
    use kurbo::Rect;

    fn active_surface(caret: Option<Rect>) -> ImeState {
        ImeState {
            active: true,
            editing: EditingState::default(),
            caret,
            ..ImeState::default()
        }
    }

    fn surface_with(content_type: ImeContentType) -> ImeState {
        ImeState {
            active: true,
            content_type,
            ..ImeState::default()
        }
    }

    fn selected(text: &str, base: i32, extent: i32, content_type: ImeContentType) -> ImeState {
        ImeState {
            active: true,
            editing: EditingState {
                text: text.to_string(),
                selection_base: base,
                selection_extent: extent,
                ..EditingState::default()
            },
            content_type,
            ..ImeState::default()
        }
    }

    fn attribute(content_type: ImeContentType, name: &str) -> Option<&'static str> {
        overlay_attributes(content_type)
            .into_iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value)
    }

    // --- session lifecycle ---

    #[test]
    fn a_focused_field_opens_a_session_on_the_press_that_focused_it() {
        let mut policy = OverlayPolicy::new();
        assert_eq!(policy.poll(false, 0, false), OverlayAction::Idle);
        assert_eq!(policy.poll(true, 1, true), OverlayAction::Open);
        assert!(policy.is_open());
    }

    #[test]
    fn a_blurred_field_closes_the_session_exactly_once() {
        let mut policy = OverlayPolicy::new();
        policy.poll(true, 1, true);
        assert_eq!(policy.poll(false, 2, false), OverlayAction::Close);
        assert!(!policy.is_open());
        // The next event has no session left to close.
        assert_eq!(policy.poll(false, 2, false), OverlayAction::Idle);
    }

    #[test]
    fn a_pointer_move_over_the_canvas_never_opens_a_session_on_its_own() {
        // A generation that has already been seen plus no gesture is exactly a
        // cursor crossing the canvas after the user tabbed away: re-opening
        // would pull DOM focus back out of whatever they moved it to.
        let mut policy = OverlayPolicy::new();
        policy.poll(true, 7, true);
        policy.mark_closed();
        assert_eq!(policy.poll(true, 7, false), OverlayAction::Idle);
        assert!(!policy.is_open());
    }

    #[test]
    fn a_focus_move_reopens_a_session_the_dom_closed_without_a_gesture() {
        let mut policy = OverlayPolicy::new();
        policy.poll(true, 7, true);
        policy.mark_closed();
        assert_eq!(policy.poll(true, 8, false), OverlayAction::Open);
    }

    #[test]
    fn a_steady_session_only_refocuses_on_a_gesture_or_a_focus_move() {
        let mut policy = OverlayPolicy::new();
        policy.poll(true, 1, true);
        assert_eq!(
            policy.poll(true, 1, false),
            OverlayAction::Update { refocus: false }
        );
        assert_eq!(
            policy.poll(true, 1, true),
            OverlayAction::Update { refocus: true }
        );
        assert_eq!(
            policy.poll(true, 2, false),
            OverlayAction::Update { refocus: true }
        );
    }

    #[test]
    fn the_frame_loop_can_close_a_session_but_never_open_one() {
        let mut policy = OverlayPolicy::new();
        policy.poll(true, 1, true);
        assert!(policy.close_if_inactive(false));
        assert!(!policy.is_open());
        // Nothing to close twice, and an inactive-to-active flip is not this
        // path's to act on.
        assert!(!policy.close_if_inactive(false));
        assert!(!policy.close_if_inactive(true));
        assert!(!policy.is_open());
    }

    #[test]
    fn only_an_active_published_surface_is_a_session() {
        assert!(!session_is_active(None));
        assert!(session_is_active(Some(&active_surface(None))));
        assert!(!session_is_active(Some(&ImeState::default())));
    }

    // --- placement ---

    #[test]
    fn the_overlay_sits_at_the_caret_in_viewport_coordinates() {
        let placed = overlay_box(Some(Rect::new(30.0, 12.0, 32.0, 30.0)), 100.0, 50.0);
        assert_eq!(
            placed,
            OverlayBox {
                left: 130.0,
                top: 62.0,
                width: 2.0,
                height: 18.0,
            }
        );
    }

    #[test]
    fn a_collapsed_caret_still_produces_a_focusable_box() {
        let placed = overlay_box(Some(Rect::new(4.0, 4.0, 4.0, 4.0)), 0.0, 0.0);
        assert_eq!(placed.width, MIN_OVERLAY_SIDE);
        assert_eq!(placed.height, MIN_OVERLAY_SIDE);

        let unplaced = overlay_box(None, 8.0, 9.0);
        assert_eq!(unplaced.left, 8.0);
        assert_eq!(unplaced.top, 9.0);
        assert_eq!(unplaced.width, MIN_OVERLAY_SIDE);
        assert_eq!(unplaced.height, MIN_OVERLAY_SIDE);
    }

    #[test]
    fn a_sub_pixel_move_is_not_written_back_to_the_dom() {
        let first = overlay_box(Some(Rect::new(10.0, 10.0, 12.0, 28.0)), 0.0, 0.0);
        assert!(overlay_box_moved(None, first));
        assert!(!overlay_box_moved(Some(first), first));

        let nudged = overlay_box(Some(Rect::new(10.2, 10.0, 12.2, 28.0)), 0.0, 0.0);
        assert!(!overlay_box_moved(Some(first), nudged));

        let moved = overlay_box(Some(Rect::new(40.0, 10.0, 42.0, 28.0)), 0.0, 0.0);
        assert!(overlay_box_moved(Some(first), moved));
    }

    // --- the key path ---

    #[test]
    fn a_plain_keystroke_is_forwarded_to_the_canvas() {
        assert!(forwards_to_canvas("a", false));
        assert!(forwards_to_canvas("Backspace", false));
        assert!(forwards_to_canvas("ArrowLeft", false));
        assert!(forwards_to_canvas("Enter", false));
    }

    #[test]
    fn a_keystroke_the_input_method_owns_is_never_forwarded() {
        // Both signals a browser uses for "the input method consumed this".
        assert!(!forwards_to_canvas("a", true));
        assert!(!forwards_to_canvas("Process", false));
    }

    #[test]
    fn an_unnamed_keystroke_is_carried_by_the_input_path_instead_of_the_canvas() {
        // Not forwarded either way — winit's web mapping has no key to make of
        // it — but only the non-composing one is an edit still owed to the
        // tree. While composing it is the input method's, and the composition
        // events already report that text.
        assert!(!forwards_to_canvas("Unidentified", false));
        assert!(key_path_dropped("Unidentified", false));

        assert!(!forwards_to_canvas("Unidentified", true));
        assert!(!key_path_dropped("Unidentified", true));

        // Nothing else is carried: a named key reached winit through the
        // canvas, and `Process` is composition text.
        assert!(!key_path_dropped("a", false));
        assert!(!key_path_dropped("Backspace", false));
        assert!(!key_path_dropped("Process", false));
    }

    #[test]
    fn escape_cancels_only_while_a_composition_is_open() {
        assert!(cancels_composition("Escape", true));
        assert!(!cancels_composition("Escape", false));
        assert!(!cancels_composition("a", true));
    }

    // --- teardown ---

    #[test]
    fn a_teardown_ends_the_session_only_when_one_was_open() {
        // The element going away is the end of the composition, and nothing in
        // the DOM reports it once the listeners are off.
        assert_eq!(
            teardown_signal(true, true),
            Some(DomEditEvent::Teardown),
            "a composition survived the element it was being typed into"
        );
        // Nothing to end: no session in flight, or no element removed.
        assert_eq!(teardown_signal(true, false), None);
        assert_eq!(teardown_signal(false, true), None);
        assert_eq!(teardown_signal(false, false), None);
    }

    // --- the content-type hint ---

    #[test]
    fn an_obscured_fields_overlay_is_secret_typed_and_a_plain_fields_is_not() {
        assert_eq!(
            attribute(ImeContentType::Password, "type"),
            Some("password")
        );
        assert_eq!(
            attribute(ImeContentType::Password, "autocomplete"),
            Some("new-password"),
            "a secret field must refuse autofill, which a plain `off` does not"
        );
        assert_eq!(
            attribute(ImeContentType::Password, "inputmode"),
            Some("text")
        );

        assert_eq!(attribute(ImeContentType::Normal, "type"), Some("text"));
        assert_eq!(
            attribute(ImeContentType::Normal, "autocomplete"),
            Some("off")
        );
        assert_eq!(
            attribute(ImeContentType::Normal, "inputmode"),
            None,
            "an ordinary field leaves the keyboard choice to the browser"
        );

        // Non-secret but suggestion-refusing: a plain-text element, pinned to
        // the plain-text keyboard.
        assert_eq!(
            attribute(ImeContentType::NoSuggestions, "type"),
            Some("text")
        );
        assert_eq!(
            attribute(ImeContentType::NoSuggestions, "inputmode"),
            Some("text")
        );
        assert_eq!(attribute(ImeContentType::Terminal, "type"), Some("text"));
        assert_eq!(
            attribute(ImeContentType::Terminal, "inputmode"),
            Some("text")
        );

        // The baseline that keeps the browser's text services off every
        // overlay is unconditional.
        for hint in [
            ImeContentType::Normal,
            ImeContentType::Password,
            ImeContentType::NoSuggestions,
            ImeContentType::Terminal,
        ] {
            assert_eq!(attribute(hint, "spellcheck"), Some("false"));
            assert_eq!(attribute(hint, "autocorrect"), Some("off"));
            assert_eq!(attribute(hint, "autocapitalize"), Some("off"));
            assert_eq!(attribute(hint, "tabindex"), Some("-1"));
        }
    }

    #[test]
    fn a_hint_change_requests_a_rebuild() {
        // A browser classifies the element it has; the hint that moved has to
        // reach it as a new one.
        assert!(overlay_needs_rebuild(
            Some(ImeContentType::Normal),
            ImeContentType::Password
        ));
        assert!(overlay_needs_rebuild(
            Some(ImeContentType::Password),
            ImeContentType::Normal
        ));
        // An unchanged hint keeps the live element (and its DOM focus).
        assert!(!overlay_needs_rebuild(
            Some(ImeContentType::Password),
            ImeContentType::Password
        ));
        // Nothing built yet is an open, not a rebuild.
        assert!(!overlay_needs_rebuild(None, ImeContentType::Password));
    }

    #[test]
    fn a_stale_element_is_replaced_only_while_it_holds_focus() {
        // The hint moved under a focused element: the replacement inherits
        // the focus, so the secret field is served by a secure element before
        // any further input arrives.
        assert_eq!(
            stale_overlay_action(Some(ImeContentType::Normal), ImeContentType::Password, true),
            StaleOverlay::Replace
        );
        // The hint moved but the element had already lost focus: a frame never
        // takes focus for the user, so the session closes instead.
        assert_eq!(
            stale_overlay_action(
                Some(ImeContentType::Normal),
                ImeContentType::Password,
                false
            ),
            StaleOverlay::Close
        );
        // An unchanged hint keeps the element whether or not it holds focus.
        assert_eq!(
            stale_overlay_action(
                Some(ImeContentType::Password),
                ImeContentType::Password,
                false
            ),
            StaleOverlay::Keep
        );
        // No element is nothing to replace: opening is the dispatch path's.
        assert_eq!(
            stale_overlay_action(None, ImeContentType::Password, true),
            StaleOverlay::Keep
        );
    }

    #[test]
    fn a_field_that_published_no_hint_reads_as_ordinary_text() {
        assert_eq!(published_content_type(None), ImeContentType::Normal);
        assert_eq!(
            published_content_type(Some(&active_surface(None))),
            ImeContentType::Normal
        );
        assert_eq!(
            published_content_type(Some(&surface_with(ImeContentType::Password))),
            ImeContentType::Password
        );
    }

    // --- the clipboard route ---

    /// `(key, ctrl, meta, shift, verb)` — every gesture a browser runs a
    /// clipboard command for, and the near misses around them.
    ///
    /// It is the table `frust-widgets`' text field decodes from the same
    /// keystrokes; the two must agree, or a gesture the widget answers as one
    /// verb would be classified here as another. The two chord modifiers are
    /// separate columns because the widget's table separates them: its letters
    /// answer to `ctrl` or `meta`, its `Insert` copy to `ctrl` alone.
    const VERB_TABLE: &[(&str, bool, bool, bool, Option<ClipboardVerb>)] = &[
        // The platform chords, in both polarities of the modifier.
        ("c", true, false, false, Some(ClipboardVerb::Copy)),
        ("x", true, false, false, Some(ClipboardVerb::Cut)),
        ("v", true, false, false, Some(ClipboardVerb::Paste)),
        ("c", false, true, false, Some(ClipboardVerb::Copy)),
        ("x", false, true, false, Some(ClipboardVerb::Cut)),
        ("v", false, true, false, Some(ClipboardVerb::Paste)),
        ("c", false, false, false, None),
        ("x", false, false, false, None),
        ("v", false, false, false, None),
        // Shift adds the capital a browser reports, not a different verb.
        ("C", true, false, true, Some(ClipboardVerb::Copy)),
        ("X", false, true, true, Some(ClipboardVerb::Cut)),
        ("V", true, false, true, Some(ClipboardVerb::Paste)),
        // The dedicated keys carry their verb with no modifier at all.
        ("Copy", false, false, false, Some(ClipboardVerb::Copy)),
        ("Cut", false, false, false, Some(ClipboardVerb::Cut)),
        ("Paste", false, false, false, Some(ClipboardVerb::Paste)),
        ("Copy", true, true, true, Some(ClipboardVerb::Copy)),
        // The legacy Insert chords. Shift wins when both are held, exactly as
        // the widget decodes it, and a bare Insert is overtype, not a verb.
        // `Cmd`+`Insert` is nobody's gesture: the widget reads only `ctrl`
        // there, and a Mac with a PC keyboard attached is where the two tables
        // would otherwise disagree.
        ("Insert", true, false, false, Some(ClipboardVerb::Copy)),
        ("Insert", false, true, false, None),
        ("Insert", true, true, false, Some(ClipboardVerb::Copy)),
        ("Insert", false, false, true, Some(ClipboardVerb::Paste)),
        ("Insert", true, false, true, Some(ClipboardVerb::Paste)),
        ("Insert", false, true, true, Some(ClipboardVerb::Paste)),
        ("Insert", false, false, false, None),
        // Shift+Delete is the legacy cut, but only on its own: anything
        // chorded past shift is an OS or browser gesture, and a plain Delete
        // is a forward delete.
        ("Delete", false, false, true, Some(ClipboardVerb::Cut)),
        ("Delete", true, false, true, None),
        ("Delete", false, true, true, None),
        ("Delete", false, false, false, None),
        // Select-all reaches no clipboard and the DOM raises nothing for it.
        ("a", true, false, false, None),
        ("a", false, true, true, None),
        // Ordinary typing, and a key that means nothing clipboard-shaped.
        ("a", false, false, false, None),
        ("Escape", true, false, false, None),
        ("Enter", false, false, true, None),
    ];

    #[test]
    fn every_clipboard_gesture_reads_as_the_verb_it_means() {
        for &(key, ctrl, meta, shift, verb) in VERB_TABLE {
            assert_eq!(
                clipboard_verb(key, ctrl, meta, shift),
                verb,
                "key={key:?} ctrl={ctrl} meta={meta} shift={shift}"
            );
        }
    }

    #[test]
    fn the_legacy_insert_copy_answers_to_ctrl_alone() {
        // `frust-widgets`' own decode reads `NamedKey::Insert` as a copy for
        // `modifiers.ctrl` and for nothing else, so `Cmd`+`Insert` — reachable
        // on a Mac with an external PC keyboard — is no verb at all. Reading it
        // as a copy here would have the bridge claim a gesture the widget
        // behind it drops, and the two tables are asserted identical.
        assert_eq!(
            clipboard_verb("Insert", false, true, false),
            None,
            "Cmd+Insert is not the widget's copy"
        );
        assert!(
            !hands_write_to_dom_event("Insert", false, true, false),
            "a gesture that is no verb hands nothing to a DOM callback"
        );
        // The chord that *is* the legacy copy, and the shift row that outranks
        // it, both unchanged.
        assert_eq!(
            clipboard_verb("Insert", true, false, false),
            Some(ClipboardVerb::Copy)
        );
        assert_eq!(
            clipboard_verb("Insert", true, false, true),
            Some(ClipboardVerb::Paste)
        );
        // The letters do answer to either modifier — the platforms differ only
        // in which one carries the chord, and the widget decodes both.
        assert_eq!(
            clipboard_verb("c", false, true, false),
            Some(ClipboardVerb::Copy)
        );
        assert_eq!(
            clipboard_verb("c", true, false, false),
            Some(ClipboardVerb::Copy)
        );
    }

    #[test]
    fn only_a_paste_gesture_leaves_the_key_path() {
        // A paste is withheld: the DOM raises `paste` for it whatever the
        // element's selection looks like, and re-dispatching it as well would
        // deliver the gesture twice. Everything else — a copy, a cut, and every
        // keystroke that is no clipboard verb at all — keeps the key path,
        // because the DOM may raise nothing for it.
        for &(key, ctrl, meta, shift, verb) in VERB_TABLE {
            let withheld = withheld_from_key_path(key, ctrl, meta, shift);
            assert_eq!(
                withheld,
                verb == Some(ClipboardVerb::Paste),
                "key={key:?} ctrl={ctrl} meta={meta} shift={shift}"
            );
            // The two verbs that write are exactly the two whose write the DOM
            // callback makes, so the keystroke marks the handoff on its way
            // past and the drain behind it hands the text over.
            assert_eq!(
                hands_write_to_dom_event(key, ctrl, meta, shift),
                matches!(verb, Some(ClipboardVerb::Copy | ClipboardVerb::Cut)),
                "key={key:?} ctrl={ctrl} meta={meta} shift={shift}"
            );
        }
        // Named for the regression they are: a browser enables its copy/cut
        // command only for a page that claims the verb or for a ranged
        // selection, and the overlay offers neither, so withholding these
        // dropped the gesture entirely.
        assert!(!withheld_from_key_path("c", true, false, false));
        assert!(!withheld_from_key_path("x", true, false, false));
        // Shift+Insert is a paste like any other: carrying it on the key path
        // as well as answering the DOM `paste` inserted the clipboard twice.
        assert!(withheld_from_key_path("Insert", false, false, true));
    }

    #[test]
    fn a_copy_the_shell_refuses_still_cancels_the_browsers_own() {
        // What the `copy`/`cut` listener decides, from the snapshot
        // `publish_selection` left it — the secret case end to end, not just
        // `clipboard_selection` in isolation.
        let secret = selected("hunter2", 0, 7, ImeContentType::Password);
        let refusal = clipboard_write_action(None, clipboard_selection(Some(&secret)));
        assert_eq!(refusal.write, None, "a secret field hands out nothing");
        assert!(
            refusal.cancel,
            "the browser's own copy takes the element, which holds a live \
             preedit while a composition owns it — a password's included"
        );
        assert!(
            !refusal.request_frame,
            "a refusal writes nothing and so leaves no mark for a drain to take"
        );
        assert!(
            !refusal.delete_owed,
            "a cut that wrote nothing may delete nothing"
        );

        // The same refusal for a collapsed selection and for no field at all.
        let caret = selected("abc", 2, 2, ImeContentType::Normal);
        assert_eq!(
            clipboard_write_action(None, clipboard_selection(Some(&caret))).write,
            None
        );
        assert!(clipboard_write_action(None, clipboard_selection(Some(&caret))).cancel);
        assert!(clipboard_write_action(None, clipboard_selection(None)).cancel);

        // An ordinary field writes, cancels, and owes the drain that takes its
        // mark.
        let plain = selected("hunter2", 0, 7, ImeContentType::Normal);
        let write = clipboard_write_action(None, clipboard_selection(Some(&plain)));
        assert_eq!(write.write, Some("hunter2"));
        assert!(write.cancel);
        assert!(write.request_frame);
    }

    #[test]
    fn a_cut_writes_the_text_the_widget_took_not_the_selection_it_left() {
        // The exact state a keyboard cut leaves behind when the browser gets
        // round to raising `cut`: the widget has already written "abc" into the
        // tree's slot (the shell holds it as a handoff) and already deleted the
        // selection, so the surface the shell last published carries a
        // collapsed one and `clipboard_selection` answers nothing.
        let after_delete = selected("", 0, 0, ImeContentType::Normal);
        let snapshot = clipboard_selection(Some(&after_delete));
        assert_eq!(snapshot, None, "the cut collapsed its own selection");

        let action = clipboard_write_action(Some("abc"), snapshot);
        assert_eq!(
            action.write,
            Some("abc"),
            "the handoff is what the callback writes: deriving the text from \
             the snapshot here writes nothing, and the text is already gone \
             from the document"
        );
        assert!(action.cancel);
        assert!(action.request_frame);
        assert!(
            !action.delete_owed,
            "the widget applied this gesture already — a second Cut would \
             delete a second time"
        );

        // A copy is the same shape minus the deletion: both sources agree, and
        // the handoff still wins, so there is one write rather than one per
        // source.
        let copied = selected("abc", 0, 3, ImeContentType::Normal);
        let copy = clipboard_write_action(Some("abc"), clipboard_selection(Some(&copied)));
        assert_eq!(copy.write, Some("abc"));
        assert!(!copy.delete_owed);
    }

    #[test]
    fn a_cut_the_dom_resolved_alone_still_owes_the_widget_its_delete() {
        // No handoff: nothing reached the tree, so this is a gesture the
        // browser resolved on its own — an edit menu's cut, a touch callout —
        // and the published selection is the only description of it there is.
        let selection = selected("abc", 0, 3, ImeContentType::Normal);
        let action = clipboard_write_action(None, clipboard_selection(Some(&selection)));
        assert_eq!(action.write, Some("abc"));
        assert!(
            action.delete_owed,
            "the clipboard has the text and nothing has deleted it yet"
        );
    }

    #[test]
    fn a_handed_over_write_is_issued_asynchronously_only_when_no_callback_takes_it() {
        // The keystroke marks the gesture on its way to the canvas, and the
        // drain behind it takes that mark exactly once.
        let mut handoff = ClipboardHandoff::new();
        handoff.mark_gesture();
        assert!(
            handoff.take_gesture(),
            "the drain reads the keystroke's mark"
        );
        assert!(
            !handoff.take_gesture(),
            "a mark left standing would park the next app-driven copy for a \
             callback that never comes"
        );

        // The engine that raises the event: the callback takes the text inside
        // the gesture, and the drain that follows finds nothing to fall back
        // with — one write reaches the host.
        handoff.offer("abc".to_string());
        assert_eq!(handoff.take(), Some("abc".to_string()), "the callback's");
        assert_eq!(
            handoff.take(),
            None,
            "no asynchronous write is owed for text the DOM already wrote"
        );

        // The engine that raises none (no beforecut to claim the verb with, a
        // collapsed overlay selection): nothing claimed the text, so the next
        // drain finds it and `writeText` gets its chance after all.
        handoff.offer("abc".to_string());
        assert_eq!(
            handoff.take(),
            Some("abc".to_string()),
            "the fallback the drain issues"
        );
        assert_eq!(handoff.take(), None);
    }

    #[test]
    fn a_copy_slices_the_selection_by_utf16_offsets() {
        // "a😀bc" is 5 UTF-16 units and 7 bytes: the emoji is one unit pair and
        // four bytes, so a byte-indexed slice of the same numbers would cut it
        // in half (and panic).
        let surface = selected("a😀bc", 1, 4, ImeContentType::Normal);
        assert_eq!(clipboard_selection(Some(&surface)), Some("😀b"));

        // A selection dragged backwards reports its anchors reversed; the
        // clipboard wants the run of text, which has no direction.
        let backwards = selected("a😀bc", 4, 1, ImeContentType::Normal);
        assert_eq!(clipboard_selection(Some(&backwards)), Some("😀b"));

        // The whole field, and a selection that ends past the text it was
        // published for (a surface racing its own edit) — clamped, never
        // panicking.
        let whole = selected("a😀bc", 0, 5, ImeContentType::Normal);
        assert_eq!(clipboard_selection(Some(&whole)), Some("a😀bc"));
        let overrun = selected("a😀bc", 0, 99, ImeContentType::Normal);
        assert_eq!(clipboard_selection(Some(&overrun)), Some("a😀bc"));
    }

    #[test]
    fn a_secret_field_refuses_to_hand_its_selection_to_the_clipboard() {
        // An obscured widget publishes its *real* text here — the mask is a
        // paint-time affair — so this refusal is the whole of what keeps a
        // password off the host clipboard on this shell.
        let secret = selected("hunter2", 0, 7, ImeContentType::Password);
        assert_eq!(clipboard_selection(Some(&secret)), None);
        // The same text in an ordinary field is an ordinary copy.
        let plain = selected("hunter2", 0, 7, ImeContentType::Normal);
        assert_eq!(clipboard_selection(Some(&plain)), Some("hunter2"));
    }

    #[test]
    fn a_collapsed_selection_or_no_field_at_all_is_not_a_copy() {
        // A caret is not a selection: copying nothing must not overwrite what
        // the clipboard already holds.
        let caret = selected("abc", 2, 2, ImeContentType::Normal);
        assert_eq!(clipboard_selection(Some(&caret)), None);
        // The `-1` sentinel is "no selection at all".
        let none = selected("abc", -1, -1, ImeContentType::Normal);
        assert_eq!(clipboard_selection(Some(&none)), None);
        // Nothing focused.
        assert_eq!(clipboard_selection(None), None);
    }

    #[test]
    fn a_clipboard_edit_retracts_a_composition_it_lands_on() {
        // The preedit is marked text the input method never committed, and a
        // paste is not the commit — so the session ends the way a cancel does,
        // before the pasted text lands.
        let paste = DomEditEvent::Paste("hi".to_string());
        assert_eq!(
            displaced_composition_signal(&paste, true),
            Some(DomEditEvent::Cancel)
        );
        assert_eq!(
            displaced_composition_signal(&DomEditEvent::EditCommand(EditCommand::Cut), true),
            Some(DomEditEvent::Cancel)
        );
        // Nothing in flight: the ordinary case owes no retraction.
        assert_eq!(displaced_composition_signal(&paste, false), None);
        // Every other signal is the composition machinery's own, and settles
        // through the latch's own rules rather than this one.
        assert_eq!(
            displaced_composition_signal(
                &DomEditEvent::CompositionEnd {
                    data: String::new()
                },
                true
            ),
            None
        );
        assert_eq!(
            displaced_composition_signal(&DomEditEvent::Blur, true),
            None
        );
    }
}
