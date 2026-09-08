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
//! reports the underlying keystrokes. Three rules, all keyed off the single
//! [`ComposeLatch`](crate::app_handler::ComposeLatch) this crate already
//! carries:
//!
//! 1. A keystroke the input method consumed is never forwarded to the canvas —
//!    [`forwards_to_canvas`] drops it on the DOM's own `isComposing` flag and
//!    on the `Process` key sentinel a browser reports for it.
//! 2. A DOM `input` signal only becomes framework text while the latch says a
//!    composition owns the field. Every other `input` is dropped, because those
//!    characters already reached the tree as a `WindowEvent::KeyboardInput`.
//! 3. Once a session has produced its commit the latch refuses a second one, so
//!    the two orderings browsers use for the end of a composition
//!    (`compositionend` then `input`, or `input` alone) both deliver exactly
//!    one [`Commit`](frust_core::event::ImeEvent::Commit).
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
//!   `spellcheck="false"`, and is emptied whenever it is not composing, so a
//!   browser has neither a form context nor accumulated text to offer.
//! * **A composition cancelled with Escape.** Browsers disagree about whether
//!   `compositionend` fires (and with what data) for a cancel, so Escape is
//!   read from the keystroke itself while composing
//!   ([`DomEditEvent::Cancel`]), clearing the preedit; a `compositionend`
//!   arriving afterwards is absorbed rather than re-applied.
//! * **An input method that commits through `input` with no `compositionend`.**
//!   A non-composing `insert*` signal arriving while a session is still open is
//!   taken as that session's commit.
//! * **Soft keyboards.** A mobile browser opens one only for a `focus()` inside
//!   a user gesture, which is why the pointer-press path re-focuses.
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

use frust_core::event::ImeState;
use kurbo::Rect;

/// The DOM `key` value a browser reports for a keystroke its input method
/// consumed — the modern spelling of the legacy `keyCode: 229` signal. A key
/// carrying it produced composition, not text, so it must not reach the key
/// path.
pub const IME_PROCESS_KEY: &str = "Process";

/// The DOM `key` value for a keystroke a browser could not name. Some input
/// methods report it instead of [`IME_PROCESS_KEY`] while composing, and winit
/// maps it to nothing useful either way, so it is dropped on the same rule.
pub const UNIDENTIFIED_KEY: &str = "Unidentified";

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
    /// The element lost DOM focus. Any session it still held is over.
    Blur,
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

    use frust_core::event::ImeState;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{
        CompositionEvent, HtmlCanvasElement, HtmlInputElement, KeyboardEvent, KeyboardEventInit,
    };
    use winit::platform::web::WindowExtWebSys;
    use winit::window::Window;

    use super::{
        DomEditEvent, OverlayAction, OverlayBox, OverlayPolicy, cancels_composition,
        forwards_to_canvas, overlay_box, overlay_box_moved, session_is_active,
    };

    /// The element's `id`, so a page inspecting its own DOM (or a bug report's
    /// screenshot of one) can tell what put an extra `<input>` there.
    const OVERLAY_ID: &str = "frust-ime-overlay";

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

    /// Attributes that keep the browser's own text services off the element.
    ///
    /// An autofill dropdown, an autocorrect bubble or a spellcheck squiggle
    /// would each render over the canvas anchored to an element the user cannot
    /// see, and the suggestion strip of a mobile keyboard would read text the
    /// app never showed it. `tabindex="-1"` keeps the overlay out of the page's
    /// own tab order — it is focused programmatically or not at all.
    const OVERLAY_ATTRIBUTES: &[(&str, &str)] = &[
        ("id", OVERLAY_ID),
        ("type", "text"),
        ("autocomplete", "off"),
        ("autocorrect", "off"),
        ("autocapitalize", "off"),
        ("spellcheck", "false"),
        ("tabindex", "-1"),
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
        pub fn sync(
            &mut self,
            window: &Arc<Window>,
            ime: Option<&ImeState>,
            generation: u64,
            gesture: bool,
            composing: bool,
        ) -> OverlayAction {
            // A blur that already tore the session down is observed before the
            // policy runs, so this pass sees the true state of the DOM rather
            // than re-placing an element the user has left.
            if self.signals.blurred.replace(false) {
                self.teardown();
                self.policy.mark_closed();
            }

            let action = self
                .policy
                .poll(session_is_active(ime), generation, gesture);
            match action {
                OverlayAction::Idle => {}
                OverlayAction::Open => {
                    if self.element.is_none() {
                        self.build(window);
                    }
                    if self.element.is_none() {
                        // The DOM refused the element (no body yet, a create
                        // that failed). Give the session back so a later
                        // gesture opens a fresh one rather than leaving the
                        // policy believing an element it does not have is live.
                        self.policy.mark_closed();
                        return OverlayAction::Idle;
                    }
                    self.place(window, ime);
                    self.clear_value(composing);
                    self.focus();
                }
                OverlayAction::Update { refocus } => {
                    self.place(window, ime);
                    self.clear_value(composing);
                    if refocus {
                        self.focus();
                    }
                }
                OverlayAction::Close => self.teardown(),
            }
            action
        }

        /// The frame loop's half of the lifecycle: drop a session the app ended
        /// without an input event of its own. Returns whether it closed one.
        pub fn close_if_inactive(&mut self, ime: Option<&ImeState>) -> bool {
            let closing = self.policy.close_if_inactive(session_is_active(ime));
            if closing {
                self.teardown();
            }
            closing
        }

        /// Follow the caret while a session is open, without touching focus.
        ///
        /// The frame loop's other half: composition produces no winit event, so
        /// a preedit growing under the user moves the caret with no dispatch to
        /// re-place the element from — and a candidate window left behind at
        /// the caret's old position is the visible symptom. Focus is
        /// deliberately not asserted here; see [`OverlayPolicy::poll`] for why
        /// a frame may never take it.
        pub fn reposition(&mut self, window: &Arc<Window>, ime: Option<&ImeState>) {
            if self.element.is_some() && session_is_active(ime) {
                self.place(window, ime);
            }
        }

        /// Whether an element is currently in the document.
        pub fn is_attached(&self) -> bool {
            self.element.is_some()
        }

        /// Create the element, style it, install its listeners and append it to
        /// the document body.
        ///
        /// Every step is fallible in a DOM that may not have a body yet; a
        /// failure logs and leaves `element` unset rather than panicking a
        /// page, and [`ImeOverlay::sync`] hands the session back so a later
        /// gesture tries again.
        fn build(&mut self, window: &Arc<Window>) {
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

            for (name, value) in OVERLAY_ATTRIBUTES {
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
        /// box so a later session re-writes its geometry from scratch.
        ///
        /// Listeners come off **before** the element does, so detaching a
        /// focused overlay cannot re-enter this bridge through its own `blur`
        /// handler and mark a session that is already gone as blurred.
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
            self.last_box = None;
            self.signals.blurred.set(false);
        }
    }

    impl Drop for ImeOverlay {
        /// A page that drops the shell must not leave an orphaned `<input>` (and
        /// its listeners) behind in the document.
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
        MIN_OVERLAY_SIDE, OverlayAction, OverlayBox, OverlayPolicy, cancels_composition,
        forwards_to_canvas, overlay_box, overlay_box_moved, session_is_active,
    };
    use frust_core::event::{EditingState, ImeState};
    use kurbo::Rect;

    fn active_surface(caret: Option<Rect>) -> ImeState {
        ImeState {
            active: true,
            editing: EditingState::default(),
            caret,
            ..ImeState::default()
        }
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
        assert!(!forwards_to_canvas("Unidentified", false));
    }

    #[test]
    fn escape_cancels_only_while_a_composition_is_open() {
        assert!(cancels_composition("Escape", true));
        assert!(!cancels_composition("Escape", false));
        assert!(!cancels_composition("a", true));
    }
}
