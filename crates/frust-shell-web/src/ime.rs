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
//!    claim, that `input` — and only it — becomes the keystroke instead.
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
//!   `type` is what the browser reads when it decides that handling.
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
//!   which has nothing to map them to.
//! * **An overlay torn down mid-composition.** Removing the element ends the
//!   session, and the DOM raises nothing for it once the listeners are off — so
//!   the shell synthesizes [`DomEditEvent::Teardown`] ([`teardown_signal`]),
//!   which retracts the preedit and reopens the latch exactly as a blur does.
//!
//! # Cases deliberately not handled
//!
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

use frust_core::event::{ImeContentType, ImeState};
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
/// open is the opposite case: it is a plain edit — a soft keyboard's letter or
/// backspace — that winit's web key mapping cannot name, so unless the `input`
/// behind it is allowed through, nothing about that keystroke reaches the tree
/// at all.
pub fn key_path_dropped(key: &str, dom_is_composing: bool) -> bool {
    !dom_is_composing && key == UNIDENTIFIED_KEY
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
/// `detached` is [`OverlaySync::detached`] (or `ImeOverlay::close_if_inactive`'s
/// answer); `composing` is the latch's own
/// [`is_composing`](crate::app_handler::ComposeLatch::is_composing).
pub fn teardown_signal(detached: bool, composing: bool) -> Option<DomEditEvent> {
    (detached && composing).then_some(DomEditEvent::Teardown)
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

#[cfg(target_arch = "wasm32")]
pub use browser::ImeOverlay;

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

    use frust_core::event::{ImeContentType, ImeState};
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{
        CompositionEvent, HtmlCanvasElement, HtmlInputElement, KeyboardEvent, KeyboardEventInit,
    };
    use winit::platform::web::WindowExtWebSys;
    use winit::window::Window;

    use super::{
        DomEditEvent, OverlayAction, OverlayBox, OverlayPolicy, OverlaySync, cancels_composition,
        forwards_to_canvas, key_path_dropped, overlay_attributes, overlay_box, overlay_box_moved,
        overlay_needs_rebuild, published_content_type, session_is_active,
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
        /// never wake to apply it.
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
                    if cancels_composition(&key, composing) {
                        push(&signals, DomEditEvent::Cancel, &window);
                        return;
                    }
                    if forwards_to_canvas(&key, composing) {
                        redispatch(canvas.as_ref(), "keydown", key_event);
                    } else if key_path_dropped(&key, composing) {
                        // Queued rather than flagged: the `input` that carries
                        // this keystroke's edit is queued too, and the mark is
                        // only honoured by the signal immediately behind it.
                        push(&signals, DomEditEvent::KeyDropped, &window);
                    }
                }
            });
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

    /// Queue one signal and ask for the frame that applies it.
    fn push(signals: &Rc<Signals>, event: DomEditEvent, window: &Arc<Window>) {
        signals.queue.borrow_mut().push_back(event);
        window.request_redraw();
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
        DomEditEvent, MIN_OVERLAY_SIDE, OverlayAction, OverlayBox, OverlayPolicy,
        cancels_composition, forwards_to_canvas, key_path_dropped, overlay_attributes, overlay_box,
        overlay_box_moved, overlay_needs_rebuild, published_content_type, session_is_active,
        teardown_signal,
    };
    use frust_core::event::{EditingState, ImeContentType, ImeState};
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
        // Nothing to end: no composition, or no element removed.
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
}
