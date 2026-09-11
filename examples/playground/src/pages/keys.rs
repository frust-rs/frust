//! Keys section: an **IME keystroke probe** — can a plain frust widget act as a
//! terminal keystroke source from the soft keyboard, with the framework exactly
//! as it ships today?
//!
//! Tap the probe row, type, and watch what the platform actually delivered: the
//! raw snapshot, what the diff concluded, and the byte stream a terminal would
//! have been sent. Nothing here is timed and nothing is executed — the derived
//! bytes are shown (and logged, see [`LOG_PREFIX`]) and go nowhere else.
//!
//! # The sentinel-buffer pattern (the design under test)
//!
//! Both mobile IME bridges are **state-sync, never op-forwarding** (see
//! `docs/CODE_STANDARDS.md`'s Language Idioms): the platform owns a text buffer
//! mirror and hands the framework a whole reconciled [`EditingState`], never a
//! keystroke. A terminal needs the opposite — one byte per key. The recovery is
//! to **diff every snapshot against a known sentinel buffer** and then reset the
//! buffer, which is what this page implements:
//!
//! 1. Prime the platform mirror to [`SENTINEL`] — a fixed 2-code-unit prefix
//!    with the selection collapsed at offset [`SENTINEL_LEN16`].
//! 2. On every [`ImeEvent::ApplyEditingState`], compare the snapshot against
//!    that sentinel ([`classify`]): longer ⇒ the tail is the keystroke(s);
//!    shorter ⇒ a Backspace (the platform deleted into the prefix); a
//!    non-collapsed composing range ⇒ marked/composing text, reported but not
//!    yet committed.
//! 3. Re-publish the sentinel [`ImeState`], so the buffer never grows and the
//!    next keystroke lands at the same known offset.
//!
//! Step 3 works because the Rust→platform push is already real and
//! **synchronous** on Android: `FrustInputConnection.sync()` calls
//! `nativeImeApply`, then immediately re-reads `nativeImeState` and reconciles
//! its `Editable` against it, with an explicit wholesale-replace branch when Rust
//! returns different text (`platform/android/frust-embedding`'s
//! `FrustSurfaceView.kt`). iOS mirrors that shape: `syncToRust()` pushes and
//! `applyReconciled(_:)` folds the returned state back into its
//! `NSMutableString` (`platform/ios/FrustEmbedding`'s `FrustTextInput.swift`).
//!
//! # Why the sentinel is two ZERO WIDTH SPACEs
//!
//! [`SENTINEL`] is `U+200B U+200B`:
//!
//! - **BMP, so the index arithmetic stays trivial.** Every index at this seam is
//!   a UTF-16 code unit (`docs/CODE_STANDARDS.md`'s UTF-16-at-the-FFI-seam rule);
//!   `U+200B` is one unit, so the prefix is 2 units — while being 6 UTF-8 bytes,
//!   which is exactly why the diff must never be done on byte lengths (see
//!   [`len_utf16`]).
//! - **Not a word character, and not a space.** Two literal spaces would hand
//!   both platforms' "double-space ⇒ period" substitution a live trigger, and
//!   there is **no way to switch that off** from frust: Android hardcodes
//!   `inputType = TYPE_CLASS_TEXT` / `imeOptions = IME_ACTION_DONE` and iOS
//!   hardcodes `autocorrectionType = .yes` with the smart-punctuation traits
//!   unimplemented. A zero-width space gives the predictive engine nothing to
//!   rewrite.
//! - **Invisible if anything ever renders the buffer.** This widget never paints
//!   the buffer, but the platform's own candidate UI can surface it.
//!
//! It is still only a *sentinel*, not a guarantee: an IME that rewrites the whole
//! buffer can eat the prefix. That case is detected rather than papered over —
//! [`Detail::PrefixLost`] — and recovered by re-publishing.
//!
//! # Where this strains against the framework
//!
//! Recorded here because each one shaped the code, and none of it needed a
//! framework change:
//!
//! - **IME clienthood is widget-level only.** [`EventCtx::request_focus`] and
//!   [`EventCtx::publish_ime_state`] are reachable only from inside a
//!   `Widget::event` impl; no facade builder forwards them to an app callback (a
//!   `button`'s closure gets `&mut State`, not `&mut EventCtx`). So a keystroke
//!   source **must** be a hand-rolled `View`/`Widget` pair over `frust::authoring`
//!   — which is what [`KeyProbeWidget`] is.
//! - **The keyboard toggle cannot be a sibling widget.** A pointer `Down` that
//!   claims no focus is a *blur*: routing clears every focused sibling pod and
//!   `RenderRoot` drops `focus_active` plus the whole IME surface
//!   (`docs/CORE_ARCHITECTURE.md`'s Focus/IME Lifecycle). A separate "hide the
//!   keyboard" button would therefore hide the keyboard **by blurring**, proving
//!   nothing. Both zones here live inside the one focused widget and both call
//!   `request_focus()`, which is what makes "`active` is orthogonal to focus"
//!   demonstrable at all.
//! - **There is no programmatic focus.** An `ImeState` published during paint
//!   only reaches the shell while `focus_active` is already true, and that is
//!   only ever set by a focus request during an event pass. The page cannot open
//!   the keyboard at mount; the first tap is mandatory.
//! - **The hosting child list must be fixed-length.** Positional reconciliation
//!   preserves a focused pod only inside the stable prefix, so a log rendered as
//!   a *growing* list of children could shift the probe row's index and silently
//!   dismiss the keyboard. [`probe_children`] therefore always emits the same
//!   number of children, with the whole log as one multi-line `text`.
//! - **Enter is asymmetric, and only Android gives a `Key` event.** Android's
//!   `IME_ACTION_DONE` arrives as a real [`InputEvent::Key`]`(Enter)`, while
//!   iOS's Return arrives as a literal `"\n"` inside the buffer snapshot. Both
//!   paths are handled and both are labelled in the log (`src=`), so a capture
//!   says which one fired.
//!
//! # Deliberate non-features
//!
//! No raw hardware-key channel (no modifiers, no keycodes — that channel does not
//! exist and adding it is a framework change, not a probe). No terminal, no
//! emulator, no PTY: the derived bytes are logged and shown, never executed
//! (`terminal.rs` is the section that drives an emulator, from fixture bytes).
//! No `Widget::semantics` impl.
//!
//! # Log format
//!
//! Every line starts with [`LOG_PREFIX`] and is a whitespace-separated
//! `key=value` tail. **Every string value is wrapped in `<…>` and escaped
//! ([`escape_str`]) so no value can contain whitespace, `<`, `>`, `\` or an
//! unescaped `|`** — the `|` inside a `text=<…>` value is the sentinel boundary
//! marker and nothing else.
//!
//! Five line shapes (each is ONE line in the log; wrapped here to fit, and pinned
//! verbatim by this module's tests):
//!
//! ```text
//! playground keys ready sentinel=<\u{200b}\u{200b}> sentinel_len16=2 sel_base=2
//!     sel_ext=2 comp_base=-1 comp_ext=-1 active=0 backspace=08 enter=0d
//!     log_capacity=20
//! playground keys zone n=1 zone=<tap> active=1 focus_claimed=1 republish=1
//! playground keys ime n=2 src=<apply> class=<char> detail=<->
//!     text=<\u{200b}\u{200b}|a> len16=3 sel_base=3 sel_ext=3 comp_base=-1
//!     comp_ext=-1 derived=<a> count=1 bytes=61 republish=1 stream_len=1
//! playground keys key n=3 key=<named:enter> mods=<----> repeat=0 class=<enter>
//!     detail=<-> bytes=0d republish=1 stream_len=2
//! playground keys blur n=4 active=0
//! ```
//!
//! `n` is one monotonic counter shared by every line shape, so a capture has a
//! total order. `bytes` is lowercase hex pairs, or `-` for none. `src=` names
//! which platform path delivered the event (`apply`/`compose`/`commit`/`enabled`/
//! `disabled`), `class=`/`detail=` are the classifier's verdict ([`Class`]/
//! [`Detail`]), and `republish=1` means the sentinel went back out in that same
//! dispatch — which on Android is what `sync()` reads back synchronously.
//!
//! These lines are per-keystroke (never per-frame, so they cannot flood a
//! capture) and are the point of the section, so they are plain `log::info!`
//! rather than perf-gated; a `--release` build still strips them via the crate's
//! `lean` feature log ceiling. Read them with
//! `adb logcat -v raw | grep 'playground keys'`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use frust::authoring::text::{
    FontFamily, GenericSlot, LineHeight, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, EditCommand, EditingState, EventCtx, EventResult,
    ImeContentType, ImeEvent, ImeState, InputEvent, Key, LayoutCtx, Modifiers, NamedKey, PaintCtx,
    PaintScene, Point, PointerPhase, Rect, Size, View, Widget,
};
use frust::{
    AnyView, Axis, Color, Component, EdgeInsets, FlexChild, FlexView, Get, Padding, RwSignal, Set,
    SizedBox, Theme, any, component, inflexible, text, use_context,
};

use crate::PlaygroundState;

// ---------------------------------------------------------------------------
// The sentinel, and the bytes a keystroke derives to
// ---------------------------------------------------------------------------

/// The sentinel buffer prefix: two `U+200B` ZERO WIDTH SPACEs. See the module
/// docs for why this character and why exactly two.
pub const SENTINEL: &str = "\u{200b}\u{200b}";

/// [`SENTINEL`]'s length in **UTF-16 code units** — the unit every index at this
/// seam is measured in. Deliberately not `SENTINEL.len()` (6 bytes): a
/// byte-length diff is the first bug this pattern invites.
pub const SENTINEL_LEN16: i32 = 2;

/// The byte a derived Backspace contributes to the stream.
///
/// `0x08` (BS, `^H`). The other live convention is `0x7f` (DEL, `^?`), which is
/// what xterm and most `stty erase` defaults actually send; this probe only has
/// to be *legible and consistent*, and nothing here feeds a real PTY, so the
/// choice is one constant rather than a policy. A terminal integration would
/// re-decide it against the shell's `erase` setting.
pub const BACKSPACE_BYTE: u8 = 0x08;

/// The byte a derived Enter contributes to the stream: `0x0d` (CR), what a
/// terminal expects for Return regardless of which platform path delivered it
/// (Android's `Key(Enter)` or iOS's `"\n"`-in-buffer).
pub const ENTER_BYTE: u8 = 0x0d;

/// Prefix of every diagnostic line — see the module docs' log format.
pub const LOG_PREFIX: &str = "playground keys";

/// How many derived events the on-screen log keeps.
pub const LOG_CAPACITY: usize = 20;

/// How many derived bytes the on-screen stream ring keeps. The `stream_len`
/// field in the log carries the *total* ever derived, not this ring's length.
pub const STREAM_CAPACITY: usize = 96;

// ---------------------------------------------------------------------------
// UTF-16 index arithmetic (pure; the correctness-critical half of the diff)
// ---------------------------------------------------------------------------

/// `s`'s length in UTF-16 code units, saturating at [`i32::MAX`] (the seam's
/// index type).
pub fn len_utf16(s: &str) -> i32 {
    let n: usize = s.chars().map(char::len_utf16).sum();
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Convert a UTF-16 code-unit index into a byte offset into `s`, clamped to
/// `0..=s.len()`.
///
/// An index that lands **inside** a surrogate pair (i.e. between the two units of
/// a non-BMP character) rounds **up**, to the byte offset just past that
/// character: never a panic, and never a partial-character slice. A negative
/// index clamps to 0, which is also how the `-1` "none" sentinel degrades.
pub fn utf16_to_byte(s: &str, idx16: i32) -> usize {
    if idx16 <= 0 {
        return 0;
    }
    let target = idx16 as usize;
    let mut units = 0usize;
    for (offset, ch) in s.char_indices() {
        if units >= target {
            return offset;
        }
        units += ch.len_utf16();
    }
    s.len()
}

/// The substring of `s` covered by the UTF-16 range `from16..to16`, clamped and
/// never panicking (an inverted or out-of-range pair yields `""`). Rounding
/// follows [`utf16_to_byte`].
pub fn utf16_slice(s: &str, from16: i32, to16: i32) -> &str {
    let from = utf16_to_byte(s, from16);
    let to = utf16_to_byte(s, to16);
    if to <= from { "" } else { &s[from..to] }
}

// ---------------------------------------------------------------------------
// Escaping (pure; the log's parse contract)
// ---------------------------------------------------------------------------

/// Escape `s` for a log value: the result contains no whitespace and no `\`, `|`,
/// `<` or `>`, so it is safe both inside a `<…>` wrapper and under a whitespace
/// `key=value` split.
///
/// ASCII graphic characters pass through; space, the four reserved punctuation
/// characters, and every other ASCII control (plus `0x7f`) become `\xNN`; every
/// non-ASCII character becomes `\u{…}`. All hex is lowercase.
pub fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            ' ' => out.push_str("\\x20"),
            '\\' => out.push_str("\\x5c"),
            '|' => out.push_str("\\x7c"),
            '<' => out.push_str("\\x3c"),
            '>' => out.push_str("\\x3e"),
            c if c.is_ascii_graphic() => out.push(c),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
        }
    }
    out
}

/// Escape a byte stream the same way (each byte independently — a UTF-8 sequence
/// therefore reads as its `\xNN` units, which is what a terminal would actually
/// have received).
pub fn escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 2);
    for &b in bytes {
        match b {
            b' ' => out.push_str("\\x20"),
            b'\\' => out.push_str("\\x5c"),
            b'|' => out.push_str("\\x7c"),
            b'<' => out.push_str("\\x3c"),
            b'>' => out.push_str("\\x3e"),
            b if b.is_ascii_graphic() => out.push(b as char),
            b => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out
}

/// A field value: escaped and wrapped, so an empty string is an unambiguous `<>`
/// rather than a dangling `key=`.
fn field(s: &str) -> String {
    format!("<{}>", escape_str(s))
}

/// Lowercase hex pairs, or `-` when there are no bytes.
pub fn hex_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "-".to_string();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The raw snapshot text, escaped, with the sentinel boundary marked by a literal
/// `|` when the prefix survived.
///
/// [`escape_str`] escapes a typed `|` to `\x7c`, so the marker is the only bare
/// `|` a value can contain. A snapshot whose prefix was rewritten
/// ([`Detail::PrefixLost`]) is escaped with **no** marker — the missing `|` is
/// itself the signal.
pub fn mark_sentinel_boundary(text: &str) -> String {
    match text.strip_prefix(SENTINEL) {
        Some(rest) => format!("{}|{}", escape_str(SENTINEL), escape_str(rest)),
        None => escape_str(text),
    }
}

// ---------------------------------------------------------------------------
// Classification (pure — the whole judgement this page exists to make)
// ---------------------------------------------------------------------------

/// What the diff against the sentinel concluded. The five names are the log's
/// `class=` vocabulary and are not extended lightly — a parser keys on them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Committed text: the snapshot's tail past the sentinel.
    Char,
    /// The platform deleted into the sentinel prefix.
    Backspace,
    /// A live composing (marked) region — reported, not yet committed.
    Composing,
    /// Return: Android's `Key(Enter)`, or iOS's `"\n"` inside the snapshot.
    Enter,
    /// Nothing derivable — see the accompanying [`Detail`].
    Unknown,
}

impl Class {
    /// The `class=` token.
    pub fn name(self) -> &'static str {
        match self {
            Class::Char => "char",
            Class::Backspace => "backspace",
            Class::Composing => "composing",
            Class::Enter => "enter",
            Class::Unknown => "unknown",
        }
    }
}

/// Why a classification landed where it did — the `detail=` token. `-` for the
/// ordinary path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// Nothing to add.
    None,
    /// The snapshot **is** the sentinel: our own re-publish coming back, or a
    /// selection-only change. No keystroke.
    Echo,
    /// The snapshot no longer starts with [`SENTINEL`] — the IME rewrote the
    /// prefix. Not derivable; recovered by re-publishing.
    PrefixLost,
    /// A `Key` event with no terminal mapping (Tab, arrows, …). Logged only.
    Unmapped,
    /// An `ImeEvent::Enabled`/`Disabled` composition bracket, not an edit.
    Bracket,
}

impl Detail {
    /// The `detail=` token.
    pub fn name(self) -> &'static str {
        match self {
            Detail::None => "-",
            Detail::Echo => "echo",
            Detail::PrefixLost => "prefix-lost",
            Detail::Unmapped => "unmapped",
            Detail::Bracket => "bracket",
        }
    }
}

/// One classified event: what it was, what a terminal would have been sent, and
/// whether the sentinel has to be re-published to recover the buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derived {
    /// The classification reached.
    pub class: Class,
    /// Why (see [`Detail`]).
    pub detail: Detail,
    /// The extracted text: the committed tail, the composing region, or `""`.
    pub text: String,
    /// Repeat count — the number of Backspaces a shrink implies; 1 for a
    /// single-event class; 0 when nothing was derived.
    pub count: usize,
    /// The bytes a terminal would have been sent.
    pub bytes: Vec<u8>,
    /// Whether the sentinel must be re-published in response.
    pub republish: bool,
}

impl Derived {
    /// A classification that derives nothing (a bracket, an unmapped key, an
    /// echo), optionally still asking for a sentinel re-publish.
    fn inert(detail: Detail, republish: bool) -> Self {
        Self {
            class: Class::Unknown,
            detail,
            text: String::new(),
            count: 0,
            bytes: Vec::new(),
            republish,
        }
    }
}

/// Whether `s` is exactly a line terminator — the Return contract shared with
/// `TextInput`'s iOS `Commit("\n")` handling.
fn is_newline(s: &str) -> bool {
    s == "\n" || s == "\r" || s == "\r\n"
}

/// Classify a committed run of text (the snapshot's tail past the sentinel, a
/// winit `Commit`, or a `Key::Character`).
///
/// A run that is *exactly* a line terminator is a [`Class::Enter`]; any other run
/// is [`Class::Char`], with embedded `\n`s translated to `\r` (a terminal wants CR
/// for Return, and a pasted multi-line run is several Returns).
pub fn classify_committed(added: &str) -> Derived {
    if is_newline(added) {
        return Derived {
            class: Class::Enter,
            detail: Detail::None,
            text: added.to_string(),
            count: 1,
            bytes: vec![ENTER_BYTE],
            republish: true,
        };
    }
    let bytes = added
        .bytes()
        .map(|b| if b == b'\n' { ENTER_BYTE } else { b })
        .collect();
    Derived {
        class: Class::Char,
        detail: Detail::None,
        text: added.to_string(),
        count: 1,
        bytes,
        republish: true,
    }
}

/// Diff one platform snapshot against the sentinel — **the pattern under test**,
/// in one pure function.
///
/// Order matters, and this is the order:
///
/// 1. **A live composing range wins.** Marked text is not committed yet, so it is
///    reported and *nothing is re-published*: pinning the buffer back to the
///    sentinel mid-composition would destroy the platform's own composition state
///    (the one case where the re-publish is actively harmful).
/// 2. **Shorter than the sentinel ⇒ Backspace**, one per missing code unit.
///    Checked before the prefix test, because a Backspace *is* a prefix that no
///    longer matches.
/// 3. **Prefix intact:**
///    - tail non-empty ⇒ [`classify_committed`] on the tail;
///    - tail empty ⇒ our own echo. Re-publish only if the caret has drifted off
///      [`SENTINEL_LEN16`] (a drifted caret would put the next keystroke *before*
///      the prefix, and step 4 would have to clean it up).
/// 4. **Prefix gone ⇒ [`Detail::PrefixLost`]**, re-publish to recover.
pub fn classify(state: &EditingState) -> Derived {
    let text = &state.text;

    // 1. Composing (marked) text — report, never commit, never re-publish.
    if state.composing_base >= 0 && state.composing_extent > state.composing_base {
        return Derived {
            class: Class::Composing,
            detail: Detail::None,
            text: utf16_slice(text, state.composing_base, state.composing_extent).to_string(),
            count: 1,
            bytes: Vec::new(),
            republish: false,
        };
    }

    // 2. The buffer shrank into the prefix: one Backspace per missing unit.
    let len16 = len_utf16(text);
    if len16 < SENTINEL_LEN16 {
        let count = (SENTINEL_LEN16 - len16) as usize;
        return Derived {
            class: Class::Backspace,
            detail: Detail::None,
            text: String::new(),
            count,
            bytes: vec![BACKSPACE_BYTE; count],
            republish: true,
        };
    }

    // 3. Prefix intact: the tail is the keystroke, or this is our own echo.
    if let Some(tail) = text.strip_prefix(SENTINEL) {
        if tail.is_empty() {
            let caret_parked =
                state.selection_base == SENTINEL_LEN16 && state.selection_extent == SENTINEL_LEN16;
            return Derived::inert(Detail::Echo, !caret_parked);
        }
        return classify_committed(tail);
    }

    // 4. The IME rewrote our prefix: not derivable, but recoverable.
    Derived {
        class: Class::Unknown,
        detail: Detail::PrefixLost,
        text: text.clone(),
        count: 0,
        bytes: Vec::new(),
        republish: true,
    }
}

/// Classify a focus-routed [`InputEvent::Key`].
///
/// This adds **no** capability: it reads the `Key` events the framework already
/// routes down the focus path — Android's `IME_ACTION_DONE`-as-`Enter`, and every
/// real key on the desktop preview (winit), which is what makes the stream fill
/// in without a device.
pub fn classify_key(key: &Key) -> Derived {
    match key {
        Key::Named(NamedKey::Enter) => Derived {
            class: Class::Enter,
            detail: Detail::None,
            text: "\n".to_string(),
            count: 1,
            bytes: vec![ENTER_BYTE],
            republish: true,
        },
        Key::Named(NamedKey::Backspace) => Derived {
            class: Class::Backspace,
            detail: Detail::None,
            text: String::new(),
            count: 1,
            bytes: vec![BACKSPACE_BYTE],
            republish: true,
        },
        Key::Named(_) => Derived::inert(Detail::Unmapped, false),
        Key::Character(s) => classify_committed(s),
    }
}

/// The `key=` token for a logged key event.
pub fn key_token(key: &Key) -> String {
    match key {
        Key::Named(named) => format!("named:{}", named_key_name(*named)),
        Key::Character(s) => format!("char:{s}"),
    }
}

/// A [`NamedKey`]'s lowercase token (an exhaustive match, so a new variant is a
/// compile error rather than a silent `unknown`).
fn named_key_name(named: NamedKey) -> &'static str {
    match named {
        NamedKey::Enter => "enter",
        NamedKey::Backspace => "backspace",
        NamedKey::Delete => "delete",
        NamedKey::ArrowLeft => "arrowleft",
        NamedKey::ArrowRight => "arrowright",
        NamedKey::ArrowUp => "arrowup",
        NamedKey::ArrowDown => "arrowdown",
        NamedKey::Home => "home",
        NamedKey::End => "end",
        NamedKey::Escape => "escape",
        NamedKey::Tab => "tab",
        NamedKey::Copy => "copy",
        NamedKey::Cut => "cut",
        NamedKey::Paste => "paste",
        NamedKey::Insert => "insert",
    }
}

/// The `mods=` token: four slots (shift, ctrl, alt, meta), `-` when absent.
pub fn mods_token(mods: Modifiers) -> String {
    let flag = |on: bool, c: char| if on { c } else { '-' };
    [
        flag(mods.shift, 's'),
        flag(mods.ctrl, 'c'),
        flag(mods.alt, 'a'),
        flag(mods.meta, 'm'),
    ]
    .into_iter()
    .collect()
}

// ---------------------------------------------------------------------------
// The published surface
// ---------------------------------------------------------------------------

/// The sentinel editing state: [`SENTINEL`] with the selection collapsed at
/// [`SENTINEL_LEN16`] and no composing region.
pub fn sentinel_editing_state() -> EditingState {
    EditingState {
        text: SENTINEL.to_string(),
        selection_base: SENTINEL_LEN16,
        selection_extent: SENTINEL_LEN16,
        composing_base: -1,
        composing_extent: -1,
    }
}

/// The [`ImeState`] this widget publishes: always the sentinel buffer, with
/// `active` the only thing that ever varies (the toggle zone's lever — the OS
/// keyboard shows/hides while focus is untouched). `content_type` is
/// [`ImeContentType::NoSuggestions`] — the probe's sentinel buffer must never be
/// autocorrected, suggested, or learned (the same reasoning as the
/// sentinel-character choice above), and a terminal integration is not a secret
/// field, so [`ImeContentType::Password`] would be the wrong hint.
pub fn sentinel_ime_state(active: bool, caret: Option<Rect>) -> ImeState {
    ImeState {
        active,
        editing: sentinel_editing_state(),
        caret,
        content_type: ImeContentType::NoSuggestions,
    }
}

// ---------------------------------------------------------------------------
// Log lines (pure; the tests pin these formats verbatim)
// ---------------------------------------------------------------------------

/// The self-describing opening line: the sentinel definition, the byte
/// vocabulary, and the starting `ImeState.active`.
pub fn ready_line(active: bool) -> String {
    format!(
        "{LOG_PREFIX} ready sentinel={} sentinel_len16={SENTINEL_LEN16} sel_base={SENTINEL_LEN16} \
         sel_ext={SENTINEL_LEN16} comp_base=-1 comp_ext=-1 active={} backspace={:02x} \
         enter={:02x} log_capacity={LOG_CAPACITY}",
        field(SENTINEL),
        u8::from(active),
        BACKSPACE_BYTE,
        ENTER_BYTE,
    )
}

/// One IME event: the raw snapshot (boundary-marked), its indices, what the diff
/// concluded, the derived bytes, and whether the sentinel went back out.
pub fn ime_line(
    n: u64,
    src: &str,
    state: &EditingState,
    derived: &Derived,
    republished: bool,
    stream_len: usize,
) -> String {
    format!(
        "{LOG_PREFIX} ime n={n} src={} class={} detail={} text=<{}> len16={} sel_base={} \
         sel_ext={} comp_base={} comp_ext={} derived={} count={} bytes={} republish={} \
         stream_len={stream_len}",
        field(src),
        field(derived.class.name()),
        field(derived.detail.name()),
        mark_sentinel_boundary(&state.text),
        len_utf16(&state.text),
        state.selection_base,
        state.selection_extent,
        state.composing_base,
        state.composing_extent,
        field(&derived.text),
        derived.count,
        hex_bytes(&derived.bytes),
        u8::from(republished),
    )
}

/// One focus-routed key event — the line that distinguishes Android's
/// `Key(Enter)` path from iOS's `"\n"`-in-buffer path in a capture.
pub fn key_line(
    n: u64,
    key: &Key,
    mods: Modifiers,
    repeat: bool,
    derived: &Derived,
    republished: bool,
    stream_len: usize,
) -> String {
    format!(
        "{LOG_PREFIX} key n={n} key={} mods={} repeat={} class={} detail={} bytes={} \
         republish={} stream_len={stream_len}",
        field(&key_token(key)),
        field(&mods_token(mods)),
        u8::from(repeat),
        field(derived.class.name()),
        field(derived.detail.name()),
        hex_bytes(&derived.bytes),
        u8::from(republished),
    )
}

/// One tap on a probe zone. `focus_claimed` is always `1` — that both zones
/// re-claim focus is the whole reason the `active` toggle can be observed without
/// a blur (see the module docs' strain list).
pub fn zone_line(n: u64, zone: Zone, active: bool) -> String {
    format!(
        "{LOG_PREFIX} zone n={n} zone={} active={} focus_claimed=1 republish=1",
        field(zone.name()),
        u8::from(active),
    )
}

/// The edge where paint observed focus gone (a tap elsewhere blurred us and
/// `RenderRoot` dropped the whole IME surface). Edge-triggered, never per-frame.
pub fn blur_line(n: u64) -> String {
    format!("{LOG_PREFIX} blur n={n} active=0")
}

/// Emit one already-formatted line — see the module docs' log-format section for
/// why this diagnostic is a plain `log::info!`.
fn emit(line: &str) {
    log::info!("{line}");
}

// ---------------------------------------------------------------------------
// Probe state (shared between the widget and the component's view)
// ---------------------------------------------------------------------------

/// Everything the probe accumulates: the published `active` flag, the derived
/// byte stream, and the on-screen log.
///
/// Lives behind an `Rc<RefCell<_>>` shared with [`KeyProbeWidget`]: the widget
/// mutates it during the event pass and bumps one generation signal, so a
/// keystroke costs exactly one signal write.
pub struct Probe {
    /// Monotonic counter stamped into every emitted line.
    counter: u64,
    /// Bumped alongside `counter` and mirrored into the generation signal, so the
    /// view rebuilds.
    generation: u64,
    /// The `active` flag currently published (drives the OS keyboard).
    active: bool,
    /// Whether the last snapshot carried a live composing range.
    composing: bool,
    /// The derived byte stream, ring-capped at [`STREAM_CAPACITY`].
    stream: VecDeque<u8>,
    /// Total bytes ever derived (the log's `stream_len`).
    stream_total: usize,
    /// The last [`LOG_CAPACITY`] derived events, newest last.
    log: VecDeque<String>,
    /// Counters for the on-screen header.
    ime_events: u64,
    key_events: u64,
    republishes: u64,
}

impl Probe {
    /// A probe that has published nothing: no focus, keyboard hidden.
    pub fn new() -> Self {
        Self {
            counter: 0,
            generation: 0,
            active: false,
            composing: false,
            stream: VecDeque::new(),
            stream_total: 0,
            log: VecDeque::new(),
            ime_events: 0,
            key_events: 0,
            republishes: 0,
        }
    }

    /// Take the next counter value (also bumping the view generation).
    fn next_n(&mut self) -> u64 {
        self.counter += 1;
        self.generation += 1;
        self.counter
    }

    /// The generation to publish into the signal after a mutation.
    fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether the keyboard is currently asked for.
    pub fn active(&self) -> bool {
        self.active
    }

    /// Fold one classified event into the stream + on-screen log.
    fn record(&mut self, n: u64, derived: &Derived) {
        for &b in &derived.bytes {
            self.stream.push_back(b);
            self.stream_total += 1;
            if self.stream.len() > STREAM_CAPACITY {
                self.stream.pop_front();
            }
        }
        let entry = format!(
            "#{n} {} {} {} -> {}",
            derived.class.name(),
            derived.detail.name(),
            field(&derived.text),
            hex_bytes(&derived.bytes),
        );
        self.log.push_back(entry);
        while self.log.len() > LOG_CAPACITY {
            self.log.pop_front();
        }
    }

    /// The header block: what is published right now, and the running totals.
    pub fn header_text(&self) -> String {
        format!(
            "sentinel={} len16={SENTINEL_LEN16}  active={}  composing={}\nime={} keys={} \
             republished={} bytes={}",
            field(SENTINEL),
            u8::from(self.active),
            u8::from(self.composing),
            self.ime_events,
            self.key_events,
            self.republishes,
            self.stream_total,
        )
    }

    /// The decoded byte stream, escaped exactly as a terminal would have received
    /// it (newest bytes last, oldest trimmed).
    pub fn stream_text(&self) -> String {
        let bytes: Vec<u8> = self.stream.iter().copied().collect();
        format!("stream: {}", escape_bytes(&bytes))
    }

    /// The derived-event log, newest last.
    pub fn log_text(&self) -> String {
        if self.log.is_empty() {
            return "log: (tap the zone below, then type)".to_string();
        }
        let mut out = String::from("log:");
        for line in &self.log {
            out.push('\n');
            out.push_str(line);
        }
        out
    }
}

impl Default for Probe {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Zone geometry (pure)
// ---------------------------------------------------------------------------

/// The two hit zones of [`KeyProbeWidget`]. Both claim focus; they differ only in
/// what they do to `ImeState.active`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// Claim focus and ask for the keyboard (`active = true`).
    Tap,
    /// Keep focus, flip `active`.
    Toggle,
}

impl Zone {
    /// The `zone=` token.
    pub fn name(self) -> &'static str {
        match self {
            Zone::Tap => "tap",
            Zone::Toggle => "toggle",
        }
    }
}

/// Fraction of the row's width the `active` toggle takes (right-hand side).
const TOGGLE_FRACTION: f64 = 0.36;

/// Gap between the two zones, in logical px. Purely cosmetic: the hit test splits
/// at [`zone_split`], so the gap belongs to whichever side it falls on.
const ZONE_GAP: f64 = 8.0;

/// The row's height in logical px.
const ZONE_HEIGHT: f64 = 96.0;

/// Corner radius of a zone.
const ZONE_RADIUS: f64 = 10.0;

/// Focus-ring width in logical px.
const RING_WIDTH: f64 = 2.0;

/// Width used when the parent hands down an unbounded width (a unit test, a
/// degenerate parent). Every real frame is width-bounded.
const FALLBACK_WIDTH: f64 = 320.0;

/// The x coordinate the two zones split at.
pub fn zone_split(width: f64) -> f64 {
    (width * (1.0 - TOGGLE_FRACTION)).max(0.0)
}

/// Which zone `position` (widget-local) falls in, or `None` when it is outside
/// the widget.
pub fn zone_at(position: Point, size: Size) -> Option<Zone> {
    if position.x < 0.0 || position.y < 0.0 || position.x >= size.width || position.y >= size.height
    {
        return None;
    }
    Some(if position.x < zone_split(size.width) {
        Zone::Tap
    } else {
        Zone::Toggle
    })
}

// ---------------------------------------------------------------------------
// Palette + labels (literal, not themed — see the note on `LABEL_INK`)
// ---------------------------------------------------------------------------

/// Zone label ink.
///
/// The probe row's colours are literal constants rather than `Theme` tokens: no
/// design token covers "IME probe zone", the row has to read identically under
/// either app brightness, and `EventCtx` carries no theme at all — so the paint
/// pass must not resolve something the hit test cannot see. The page chrome
/// around the row stays fully themed.
const LABEL_INK: Color = Color::from_rgb8(0xE6, 0xE8, 0xEA);
/// Tap-zone fill.
const TAP_FILL: Color = Color::from_rgb8(0x1C, 0x2A, 0x3A);
/// Toggle-zone fill while the keyboard is asked for.
const TOGGLE_FILL_ON: Color = Color::from_rgb8(0x24, 0x3A, 0x26);
/// Toggle-zone fill while it is suppressed.
const TOGGLE_FILL_OFF: Color = Color::from_rgb8(0x3A, 0x26, 0x24);
/// Ring colour while this widget holds the focus path.
const FOCUS_RING: Color = Color::from_rgb8(0x7F, 0xD1, 0xFF);
/// Ring colour while it does not.
const BLUR_RING: Color = Color::from_rgb8(0x44, 0x4A, 0x52);

/// Label size in logical px.
const LABEL_SIZE: f32 = 13.0;
/// Padding from a zone's top-left corner to its label.
const LABEL_PAD: f64 = 10.0;

/// Tap-zone label.
const TAP_LABEL: &str = "tap: focus + keyboard";
/// Toggle-zone label while `active` is set.
const TOGGLE_LABEL_ON: &str = "active=1\nhide";
/// Toggle-zone label while it is clear.
const TOGGLE_LABEL_OFF: &str = "active=0\nshow";

/// The monospace stack the probe's text is shaped in, so an escaped stream lines
/// up. The same stack the Terminal section shapes its grid with.
fn mono() -> FontFamily {
    FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace)
}

// ---------------------------------------------------------------------------
// Page + Component
// ---------------------------------------------------------------------------

/// See the page-fn contract in [`crate::pages`]. The Keys section reads no shared
/// signal — it only mounts [`KeysPage`]'s own retained state.
pub fn page(_state: &PlaygroundState) -> AnyView<PlaygroundState> {
    any(component(KeysPage))
}

/// The Keys section's own [`Component`]: owns the shared [`Probe`] and the one
/// generation signal a keystroke writes.
struct KeysPage;

/// [`KeysPage`]'s retained state.
struct KeysPageState {
    /// Bumped once per probe event — the only signal this page writes.
    generation: RwSignal<u64>,
    /// Shared with [`KeyProbeWidget`]: mutated in the event pass, read here.
    probe: Rc<RefCell<Probe>>,
}

impl Component for KeysPage {
    type State = KeysPageState;

    fn init(&self) -> KeysPageState {
        let probe = Probe::new();
        // Self-describing capture: the sentinel definition and the starting
        // `active` go out before any event can.
        emit(&ready_line(probe.active()));
        KeysPageState {
            generation: RwSignal::new(0),
            probe: Rc::new(RefCell::new(probe)),
        }
    }

    fn build(&self, state: &mut KeysPageState) -> AnyView<KeysPageState> {
        // Tracked read: a probe event's signal write wakes this rebuild, and
        // nothing else does.
        let generation = state.generation.get();

        let (header, stream, log) = {
            let probe = state.probe.borrow();
            (probe.header_text(), probe.stream_text(), probe.log_text())
        };

        any(Padding(
            EdgeInsets::all(CONTENT_PAD),
            FlexView::new(
                Axis::Vertical,
                probe_children(
                    header,
                    stream,
                    log,
                    KeyProbeView {
                        generation,
                        probe: Rc::clone(&state.probe),
                        bump: state.generation,
                    },
                ),
            ),
        ))
    }
}

/// How many children [`probe_children`] always returns — see its docs.
pub const CHILD_COUNT: usize = 9;

/// Which of those children is the probe row (the focused pod).
pub const PROBE_ROW_INDEX: usize = 7;

/// Padding around the whole probe column, in logical px.
const CONTENT_PAD: f64 = 16.0;

/// Gap between the header/stream blocks, in logical px.
const BLOCK_GAP: f64 = 8.0;

/// Breathing room under the probe row so the last log lines are not flush
/// against the section's bottom edge.
const BOTTOM_GAP: f64 = 24.0;

/// Build the probe column's children.
///
/// **The list is fixed-length ([`CHILD_COUNT`]) with the probe row pinned at
/// [`PROBE_ROW_INDEX`], and that is a correctness requirement, not tidiness.**
/// `Flex` reconciles an unkeyed child list positionally and preserves a focused
/// pod only inside the *stable prefix*, so a log rendered as a growing list of
/// children would shift the probe row's index as it filled and silently dismiss
/// the keyboard mid-probe. The whole log is therefore ONE multi-line `text`.
fn probe_children(
    header: String,
    stream: String,
    log: String,
    row: KeyProbeView,
) -> Vec<FlexChild<KeysPageState>> {
    vec![
        inflexible(any(text("Keys").size(13.0).color(accent()))),
        inflexible(any(text(
            "An IME keystroke probe: the platform hands frust a whole reconciled buffer, \
                 never a keystroke, so the keystroke is recovered by diffing every snapshot \
                 against a fixed sentinel and pinning the buffer back. Tap the blue zone, then \
                 type.",
        )
        .size(11.0)
        .color(muted()))),
        inflexible(text(header).size(12.0).family(mono())),
        inflexible(SizedBox(None, Some(BLOCK_GAP))),
        inflexible(text(stream).size(12.0).family(mono())),
        inflexible(SizedBox(None, Some(BLOCK_GAP))),
        inflexible(text(log).size(10.0).family(mono())),
        inflexible(row),
        inflexible(SizedBox(None, Some(BOTTOM_GAP))),
    ]
}

/// Live-theme accent-text role (`primary`), falling back to the Material baseline
/// pre-context.
fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .primary
}

/// A muted caption ink.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant
}

// ---------------------------------------------------------------------------
// KeyProbeView / KeyProbeWidget — the IME client
// ---------------------------------------------------------------------------

/// The declarative half of the probe row. See [`KeyProbeWidget`].
pub struct KeyProbeView {
    /// The probe generation this view was built at; a change re-shapes the labels
    /// (the `active` flag is baked into one of them).
    pub generation: u64,
    /// The shared probe state.
    pub probe: Rc<RefCell<Probe>>,
    /// The generation signal to bump after a probe event.
    pub bump: RwSignal<u64>,
}

/// The retained probe row: **the IME client**.
///
/// Everything that makes a widget an IME client happens here and nowhere else —
/// [`EventCtx::request_focus`] on a pointer `Down`,
/// [`EventCtx::publish_ime_state`] with the sentinel, and the
/// [`ImeEvent::ApplyEditingState`] diff. Two hit zones, both focus-claiming (see
/// the module docs for why the `active` toggle cannot be a sibling widget).
pub struct KeyProbeWidget {
    generation: u64,
    probe: Rc<RefCell<Probe>>,
    bump: RwSignal<u64>,
    /// `(generation, size)` the labels were shaped for.
    shaped: Option<(u64, Size)>,
    tap_label: Option<TextLayout>,
    toggle_label: Option<TextLayout>,
    /// Last absolute paint origin — `EventCtx::origin()` is only
    /// parent-relative, so the published caret is derived from this instead.
    paint_origin: Point,
    /// Focus as of the last paint, for the blur edge.
    was_focused: bool,
}

impl<State: 'static> View<State> for KeyProbeView {
    type Element = KeyProbeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> KeyProbeWidget {
        KeyProbeWidget {
            generation: self.generation,
            probe: Rc::clone(&self.probe),
            bump: self.bump,
            shaped: None,
            tap_label: None,
            toggle_label: None,
            paint_origin: Point::ZERO,
            was_focused: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut KeyProbeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.generation == self.generation {
            return ChangeFlags::NONE;
        }
        element.generation = self.generation;
        // LAYOUT, not just PAINT: the labels re-shape in `layout`, and Android's
        // intra-frame layout skip would otherwise freeze the toggle's label.
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }
}

impl KeyProbeWidget {
    /// The caret rect handed to the platform for candidate placement: a thin bar
    /// at the tap zone's left edge, in absolute coordinates (see
    /// [`Self::paint_origin`]).
    fn caret(&self, size: Size) -> Option<Rect> {
        let x = self.paint_origin.x + LABEL_PAD;
        let y = self.paint_origin.y + LABEL_PAD;
        Some(Rect::new(x, y, x + 1.0, y + size.height.min(24.0)))
    }

    /// Publish the sentinel surface with the probe's current `active` flag — the
    /// step that pins the platform buffer back to a known state.
    fn publish(&self, ctx: &mut EventCtx, active: bool) {
        let caret = self.caret(ctx.size());
        ctx.publish_ime_state(sentinel_ime_state(active, caret));
    }

    /// Wake the view rebuild after a probe mutation (one signal write per event).
    fn wake(&self) {
        let generation = self.probe.borrow().generation();
        self.bump.set(generation);
    }

    /// A pointer `Down` in one of the two zones.
    fn on_zone(&mut self, ctx: &mut EventCtx, zone: Zone) -> EventResult {
        // BOTH zones re-claim focus. A `Down` that claims none is a blur:
        // `RenderRoot::event` would drop `focus_active` and the whole IME
        // surface, so a non-claiming toggle would "hide the keyboard" by blurring
        // and prove nothing about `active`.
        ctx.request_focus();
        let (n, active) = {
            let mut probe = self.probe.borrow_mut();
            probe.active = match zone {
                Zone::Tap => true,
                Zone::Toggle => !probe.active,
            };
            probe.republishes += 1;
            (probe.next_n(), probe.active)
        };
        self.publish(ctx, active);
        emit(&zone_line(n, zone, active));
        self.was_focused = true;
        self.wake();
        ctx.request_redraw();
        EventResult::Handled
    }

    /// Fold one classified event into the probe, log it, and re-publish the
    /// sentinel when the classification asked for it.
    fn absorb(
        &mut self,
        ctx: &mut EventCtx,
        derived: &Derived,
        log: impl FnOnce(u64, bool) -> String,
    ) {
        let (n, republish, active) = {
            let mut probe = self.probe.borrow_mut();
            let n = probe.next_n();
            probe.record(n, derived);
            if derived.republish {
                probe.republishes += 1;
            }
            (n, derived.republish, probe.active)
        };
        if republish {
            self.publish(ctx, active);
        }
        emit(&log(n, republish));
        self.wake();
        ctx.request_redraw();
    }

    /// An IME event carrying (or standing in for) a whole snapshot.
    fn on_ime(&mut self, ctx: &mut EventCtx, event: &ImeEvent) -> EventResult {
        // Compose/Commit are the desktop (winit) shapes; ApplyEditingState is the
        // mobile one. Enabled/Disabled are composition brackets. Each is logged
        // through ONE line shape, with `src=` naming the path — a capture
        // therefore says which platform path actually fired.
        let (src, state, derived) = match event {
            ImeEvent::ApplyEditingState(state) => ("apply", state.clone(), classify(state)),
            ImeEvent::Compose { text, .. } => (
                "compose",
                EditingState {
                    text: text.clone(),
                    selection_base: -1,
                    selection_extent: -1,
                    composing_base: 0,
                    composing_extent: len_utf16(text),
                },
                Derived {
                    class: Class::Composing,
                    detail: Detail::None,
                    text: text.clone(),
                    count: 1,
                    bytes: Vec::new(),
                    republish: false,
                },
            ),
            ImeEvent::Commit(committed) => (
                "commit",
                EditingState {
                    text: committed.clone(),
                    selection_base: len_utf16(committed),
                    selection_extent: len_utf16(committed),
                    composing_base: -1,
                    composing_extent: -1,
                },
                classify_committed(committed),
            ),
            ImeEvent::Enabled => (
                "enabled",
                sentinel_editing_state(),
                Derived::inert(Detail::Bracket, false),
            ),
            ImeEvent::Disabled => (
                "disabled",
                sentinel_editing_state(),
                Derived::inert(Detail::Bracket, false),
            ),
        };

        self.probe.borrow_mut().ime_events += 1;
        self.probe.borrow_mut().composing = derived.class == Class::Composing;
        let stream_total_after = {
            let probe = self.probe.borrow();
            probe.stream_total + derived.bytes.len()
        };
        let for_log = (src, state.clone(), derived.clone());
        self.absorb(ctx, &derived, move |n, republished| {
            ime_line(
                n,
                for_log.0,
                &for_log.1,
                &for_log.2,
                republished,
                stream_total_after,
            )
        });
        EventResult::Handled
    }

    /// A focus-routed key event (Android's Enter action; every key on desktop).
    fn on_key(
        &mut self,
        ctx: &mut EventCtx,
        key: &Key,
        mods: Modifiers,
        repeat: bool,
    ) -> EventResult {
        let derived = classify_key(key);
        self.probe.borrow_mut().key_events += 1;
        let stream_total_after = {
            let probe = self.probe.borrow();
            probe.stream_total + derived.bytes.len()
        };
        let for_log = (key.clone(), derived.clone());
        self.absorb(ctx, &derived, move |n, republished| {
            key_line(
                n,
                &for_log.0,
                mods,
                repeat,
                &for_log.1,
                republished,
                stream_total_after,
            )
        });
        EventResult::Handled
    }

    /// An `EditCommand` routed through focus.
    fn on_edit_command(&mut self, ctx: &mut EventCtx, cmd: &EditCommand) -> EventResult {
        match cmd {
            EditCommand::Paste(text) => {
                let derived = classify_committed(text);
                self.probe.borrow_mut().key_events += 1;
                let stream_total_after = {
                    let probe = self.probe.borrow();
                    probe.stream_total + derived.bytes.len()
                };
                let for_log = (text.clone(), derived.clone());
                self.absorb(ctx, &derived, move |n, republished| {
                    format!(
                        "{LOG_PREFIX} edit n={n} cmd=paste class={} detail={} bytes={} \
                         republish={} stream_len={stream_total_after}",
                        field(for_log.1.class.name()),
                        field(for_log.1.detail.name()),
                        hex_bytes(&for_log.1.bytes),
                        u8::from(republished),
                    )
                });
                EventResult::Handled
            }
            EditCommand::Copy | EditCommand::Cut | EditCommand::SelectAll => {
                // These operations don't produce input bytes for a terminal.
                // Copy and Cut only manage clipboard; SelectAll only affects UI selection.
                // This widget focuses on character input and paste events.
                EventResult::Ignored
            }
        }
    }

    /// Re-shape the two zone labels (the toggle's carries the `active` flag).
    fn reshape(&mut self, ctx: &mut LayoutCtx, size: Size) {
        let active = self.probe.borrow().active;
        let style = TextStyle {
            family: mono(),
            line_height: LineHeight::FontSizeRelative(1.25),
            ..TextStyle::new(LABEL_SIZE, LABEL_INK)
        };
        let toggle = if active {
            TOGGLE_LABEL_ON
        } else {
            TOGGLE_LABEL_OFF
        };
        let text_ctx = ctx.text_context::<TextContext>();
        self.tap_label = Some(text_ctx.layout(TAP_LABEL, &style, None));
        self.toggle_label = Some(text_ctx.layout(toggle, &style, None));
        self.shaped = Some((self.generation, size));
    }
}

impl Widget for KeyProbeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            FALLBACK_WIDTH
        };
        let size = bc.constrain(Size::new(width, ZONE_HEIGHT));
        if self.shaped != Some((self.generation, size)) {
            self.reshape(ctx, size);
        }
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        self.paint_origin = origin;

        let focused = ctx.has_focus();
        // Blur edge: a tap elsewhere cleared the recorded focus path without ever
        // reaching `event()` (`PaintCtx::has_focus` is authoritative —
        // `docs/CODE_STANDARDS.md`'s focus conventions). One-shot, so waking the
        // rebuild here cannot loop.
        if self.was_focused && !focused {
            self.was_focused = false;
            let n = {
                let mut probe = self.probe.borrow_mut();
                probe.active = false;
                probe.composing = false;
                probe.next_n()
            };
            emit(&blur_line(n));
            self.wake();
        } else if focused {
            self.was_focused = true;
        }

        let split = zone_split(size.width);
        let ring = if focused { FOCUS_RING } else { BLUR_RING };
        let active = self.probe.borrow().active;

        let tap_rect = Rect::new(
            origin.x,
            origin.y,
            origin.x + split - ZONE_GAP / 2.0,
            origin.y + size.height,
        );
        let toggle_rect = Rect::new(
            origin.x + split + ZONE_GAP / 2.0,
            origin.y,
            origin.x + size.width,
            origin.y + size.height,
        );
        let toggle_fill = if active {
            TOGGLE_FILL_ON
        } else {
            TOGGLE_FILL_OFF
        };

        for (rect, fill, label) in [
            (tap_rect, TAP_FILL, self.tap_label.as_ref()),
            (toggle_rect, toggle_fill, self.toggle_label.as_ref()),
        ] {
            if rect.width() <= 0.0 {
                continue;
            }
            // Ring, then an inset fill on top of it — `PaintScene` has no
            // stroked-rect primitive.
            scene.fill_rounded_rect(
                rect.origin(),
                Size::new(rect.width(), rect.height()),
                ZONE_RADIUS,
                ring,
            );
            let inner = rect.inset(-RING_WIDTH);
            scene.fill_rounded_rect(
                inner.origin(),
                Size::new(inner.width().max(0.0), inner.height().max(0.0)),
                ZONE_RADIUS,
                fill,
            );
            if let Some(layout) = label {
                let at = Point::new(rect.x0 + LABEL_PAD, rect.y0 + LABEL_PAD);
                for run in layout.to_scene_runs(at) {
                    scene.draw_glyph_run(run);
                }
            }
        }
        // No `request_frame`: the probe is event-driven, so it costs zero frames
        // at rest.
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Pointer(p) => {
                if p.phase != PointerPhase::Down {
                    // No capture is taken: there is no drag semantic here, and
                    // `Cancel` must never touch state anyway.
                    return EventResult::Ignored;
                }
                match zone_at(p.position, ctx.size()) {
                    Some(zone) => self.on_zone(ctx, zone),
                    // Outside our bounds (only reachable as a root widget): a
                    // blur. Claim nothing and let `RenderRoot` clear the surface.
                    None => EventResult::Ignored,
                }
            }
            InputEvent::Key(k) => {
                if !ctx.has_focus() {
                    return EventResult::Ignored;
                }
                self.on_key(ctx, &k.key, k.modifiers, k.repeat)
            }
            InputEvent::Ime(e) => {
                if !ctx.has_focus() {
                    return EventResult::Ignored;
                }
                self.on_ime(ctx, e)
            }
            InputEvent::Scroll { .. } => EventResult::Ignored,
            InputEvent::EditCommand(cmd) => {
                if !ctx.has_focus() {
                    return EventResult::Ignored;
                }
                self.on_edit_command(ctx, cmd)
            }
            // Not user input — this widget queues no deferred callback for the
            // broadcast to run, so it simply falls through (see
            // `InputEvent::Housekeeping`'s own doc for the routing contract).
            InputEvent::Housekeeping => EventResult::Ignored,
            // A broadcast for some portal owner elsewhere; this probe hosts none.
            InputEvent::Overlay(_) => EventResult::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::{FrameTime, KeyEvent, PointerButton, PointerEvent, RenderRoot};
    use frust_reactive::ReactiveRuntime;
    use frust_text::TextContext as RawTextContext;
    use kurbo::BezPath;
    use peniko::Brush;
    use reactive_graph::owner::Owner;
    use std::any::Any;

    // -----------------------------------------------------------------
    // The sentinel itself
    // -----------------------------------------------------------------

    #[test]
    fn sentinel_is_two_bmp_units_but_six_bytes() {
        // The whole reason the diff is UTF-16-indexed and never byte-indexed.
        assert_eq!(SENTINEL.chars().count(), 2);
        assert_eq!(len_utf16(SENTINEL), SENTINEL_LEN16);
        assert_eq!(SENTINEL.len(), 6, "6 UTF-8 bytes, 2 UTF-16 units");
        for ch in SENTINEL.chars() {
            assert_eq!(ch.len_utf16(), 1, "BMP only: no surrogate arithmetic");
        }
    }

    #[test]
    fn sentinel_state_is_collapsed_past_the_prefix_with_no_composing_range() {
        let state = sentinel_editing_state();
        assert_eq!(state.text, SENTINEL);
        assert_eq!(state.selection_base, SENTINEL_LEN16);
        assert_eq!(state.selection_extent, SENTINEL_LEN16);
        assert_eq!(state.composing_base, -1);
        assert_eq!(state.composing_extent, -1);

        let ime = sentinel_ime_state(true, None);
        assert!(ime.active);
        assert_eq!(ime.editing, state);
    }

    // -----------------------------------------------------------------
    // UTF-16 index arithmetic
    // -----------------------------------------------------------------

    #[test]
    fn len_utf16_counts_units_not_bytes_or_chars() {
        assert_eq!(len_utf16(""), 0);
        assert_eq!(len_utf16("abc"), 3);
        // 1 unit, 3 bytes.
        assert_eq!(len_utf16("\u{200b}"), 1);
        // 2 units (surrogate pair), 4 bytes, 1 char.
        assert_eq!(len_utf16("\u{1f600}"), 2);
        assert_eq!("\u{1f600}".len(), 4);
        assert_eq!("\u{1f600}".chars().count(), 1);
    }

    #[test]
    fn utf16_to_byte_clamps_and_rounds_a_surrogate_midpoint_up() {
        let s = "ab\u{1f600}c"; // units: a=0 b=1 emoji=2..4 c=4 ; bytes: 0,1,2..6,6
        assert_eq!(
            utf16_to_byte(s, -5),
            0,
            "the -1 'none' sentinel clamps to 0"
        );
        assert_eq!(utf16_to_byte(s, 0), 0);
        assert_eq!(utf16_to_byte(s, 2), 2, "the emoji starts at byte 2");
        // Index 3 is the emoji's low surrogate: round up past the char.
        assert_eq!(utf16_to_byte(s, 3), 6);
        assert_eq!(utf16_to_byte(s, 4), 6);
        assert_eq!(utf16_to_byte(s, 99), s.len(), "past the end clamps");
    }

    #[test]
    fn utf16_slice_never_panics_and_never_splits_a_character() {
        let s = "ab\u{1f600}c";
        assert_eq!(utf16_slice(s, 0, 2), "ab");
        assert_eq!(utf16_slice(s, 2, 4), "\u{1f600}");
        // A midpoint start/end still yields whole characters.
        assert_eq!(utf16_slice(s, 2, 3), "\u{1f600}");
        assert_eq!(utf16_slice(s, 3, 5), "c");
        // Inverted / out of range.
        assert_eq!(utf16_slice(s, 4, 2), "");
        assert_eq!(utf16_slice(s, -1, 0), "");
        assert_eq!(utf16_slice("", 0, 9), "");
    }

    // -----------------------------------------------------------------
    // Classification — the pattern under test
    // -----------------------------------------------------------------

    /// A snapshot of `text` with the caret collapsed at its end and no composing
    /// range (the ordinary mobile shape).
    fn snapshot(text: &str) -> EditingState {
        let end = len_utf16(text);
        EditingState {
            text: text.to_string(),
            selection_base: end,
            selection_extent: end,
            composing_base: -1,
            composing_extent: -1,
        }
    }

    #[test]
    fn one_typed_ascii_char_derives_its_byte_and_asks_for_a_republish() {
        let derived = classify(&snapshot(&format!("{SENTINEL}a")));
        assert_eq!(derived.class, Class::Char);
        assert_eq!(derived.detail, Detail::None);
        assert_eq!(derived.text, "a");
        assert_eq!(derived.bytes, vec![b'a']);
        assert!(derived.republish, "the buffer must be pinned back");
    }

    #[test]
    fn a_non_bmp_keystroke_derives_its_utf8_bytes_despite_utf16_indices() {
        // The multi-byte case: an emoji is 2 UTF-16 units and 4 UTF-8 bytes, so
        // the platform reports sel=4 while the tail is one char.
        let text = format!("{SENTINEL}\u{1f600}");
        let state = EditingState {
            text: text.clone(),
            selection_base: 4,
            selection_extent: 4,
            composing_base: -1,
            composing_extent: -1,
        };
        let derived = classify(&state);
        assert_eq!(derived.class, Class::Char);
        assert_eq!(derived.text, "\u{1f600}");
        assert_eq!(hex_bytes(&derived.bytes), "f09f9880");
        assert_eq!(len_utf16(&text), 4, "2 sentinel units + 2 surrogate units");
    }

    #[test]
    fn a_shrunk_buffer_is_a_backspace_and_an_empty_one_is_two() {
        // One prefix unit deleted.
        let one = classify(&snapshot("\u{200b}"));
        assert_eq!(one.class, Class::Backspace);
        assert_eq!(one.count, 1);
        assert_eq!(one.bytes, vec![BACKSPACE_BYTE]);
        assert!(one.republish);

        // The empty-buffer case: the whole prefix is gone (a fast double
        // backspace, or a platform that clears rather than deletes).
        let empty = classify(&snapshot(""));
        assert_eq!(empty.class, Class::Backspace);
        assert_eq!(empty.count, 2, "one Backspace per missing code unit");
        assert_eq!(empty.bytes, vec![BACKSPACE_BYTE, BACKSPACE_BYTE]);
        assert!(empty.republish, "the sentinel must be restored");
    }

    #[test]
    fn a_live_composing_range_reports_without_committing_or_republishing() {
        let text = format!("{SENTINEL}ni");
        let state = EditingState {
            text,
            selection_base: 4,
            selection_extent: 4,
            composing_base: 2,
            composing_extent: 4,
        };
        let derived = classify(&state);
        assert_eq!(derived.class, Class::Composing);
        assert_eq!(derived.text, "ni");
        assert!(derived.bytes.is_empty(), "composing text is not committed");
        assert!(
            !derived.republish,
            "re-publishing mid-composition would destroy the platform's composition state — \
             the one case the pattern must not pin"
        );
    }

    #[test]
    fn a_composing_range_over_a_surrogate_pair_slices_whole_characters() {
        let text = format!("{SENTINEL}\u{1f600}");
        let state = EditingState {
            text,
            selection_base: 4,
            selection_extent: 4,
            composing_base: 2,
            composing_extent: 4,
        };
        assert_eq!(classify(&state).text, "\u{1f600}");
    }

    #[test]
    fn our_own_echo_is_inert_unless_the_caret_drifted() {
        // Exactly the sentinel, caret parked where we put it: nothing to do.
        let parked = classify(&sentinel_editing_state());
        assert_eq!(parked.class, Class::Unknown);
        assert_eq!(parked.detail, Detail::Echo);
        assert!(parked.bytes.is_empty());
        assert!(!parked.republish, "an echo must not ping-pong");

        // Same text, caret moved to 0: the next keystroke would land BEFORE the
        // prefix, so the sentinel goes back out to reset it.
        let drifted = EditingState {
            selection_base: 0,
            selection_extent: 0,
            ..sentinel_editing_state()
        };
        let derived = classify(&drifted);
        assert_eq!(derived.detail, Detail::Echo);
        assert!(derived.republish, "a drifted caret must be re-parked");
    }

    #[test]
    fn a_rewritten_prefix_is_detected_and_recovered_not_mis_derived() {
        // Predictive text replaced the whole buffer: no keystroke is derivable,
        // but the probe must recover rather than wedge.
        let derived = classify(&snapshot("hello"));
        assert_eq!(derived.class, Class::Unknown);
        assert_eq!(derived.detail, Detail::PrefixLost);
        assert!(derived.bytes.is_empty(), "never guess a keystroke");
        assert!(derived.republish);

        // A keystroke inserted BEFORE the prefix is the same case.
        let before = classify(&snapshot(&format!("a{SENTINEL}")));
        assert_eq!(before.detail, Detail::PrefixLost);
    }

    #[test]
    fn a_newline_in_the_buffer_is_enter_and_carries_cr() {
        for nl in ["\n", "\r", "\r\n"] {
            let derived = classify(&snapshot(&format!("{SENTINEL}{nl}")));
            assert_eq!(derived.class, Class::Enter, "{nl:?}");
            assert_eq!(derived.bytes, vec![ENTER_BYTE], "{nl:?}");
        }
        // A multi-character run containing a newline stays `char`, with the
        // newline translated to CR (a paste is several Returns, not one).
        let pasted = classify(&snapshot(&format!("{SENTINEL}ab\nc")));
        assert_eq!(pasted.class, Class::Char);
        assert_eq!(pasted.bytes, vec![b'a', b'b', ENTER_BYTE, b'c']);
    }

    #[test]
    fn key_events_map_only_where_a_terminal_byte_exists() {
        let enter = classify_key(&Key::Named(NamedKey::Enter));
        assert_eq!(enter.class, Class::Enter);
        assert_eq!(enter.bytes, vec![ENTER_BYTE]);

        let back = classify_key(&Key::Named(NamedKey::Backspace));
        assert_eq!(back.class, Class::Backspace);
        assert_eq!(back.bytes, vec![BACKSPACE_BYTE]);

        let typed = classify_key(&Key::Character("x".to_string()));
        assert_eq!(typed.class, Class::Char);
        assert_eq!(typed.bytes, vec![b'x']);

        // winit delivers Return as a character on some platforms.
        assert_eq!(
            classify_key(&Key::Character("\n".to_string())).class,
            Class::Enter
        );

        // Everything else is logged, never invented.
        let tab = classify_key(&Key::Named(NamedKey::Tab));
        assert_eq!(tab.class, Class::Unknown);
        assert_eq!(tab.detail, Detail::Unmapped);
        assert!(tab.bytes.is_empty());
        assert!(!tab.republish);
    }

    // -----------------------------------------------------------------
    // Escaping + the log's parse contract
    // -----------------------------------------------------------------

    #[test]
    fn escaping_leaves_no_whitespace_or_reserved_punctuation() {
        assert_eq!(escape_str("a b"), "a\\x20b");
        assert_eq!(escape_str("a|b"), "a\\x7cb");
        assert_eq!(escape_str("a\\b"), "a\\x5cb");
        assert_eq!(escape_str("<>"), "\\x3c\\x3e");
        assert_eq!(escape_str("\t\u{7f}"), "\\x09\\x7f");
        assert_eq!(escape_str("\u{200b}"), "\\u{200b}");
        assert_eq!(escape_str("\u{1f600}"), "\\u{1f600}");
        assert_eq!(escape_bytes(&[0x08, b'a', 0x0d]), "\\x08a\\x0d");
        assert_eq!(hex_bytes(&[]), "-");
        assert_eq!(hex_bytes(&[0x0d, 0xff]), "0dff");
    }

    #[test]
    fn the_sentinel_boundary_marker_is_the_only_bare_pipe_in_a_value() {
        let marked = mark_sentinel_boundary(&format!("{SENTINEL}a|b"));
        assert_eq!(marked, "\\u{200b}\\u{200b}|a\\x7cb");
        assert_eq!(marked.matches('|').count(), 1, "one boundary, one bar");
        // No prefix, no marker — the missing bar is the signal.
        assert_eq!(mark_sentinel_boundary("hello"), "hello");
        assert!(!mark_sentinel_boundary("hello").contains('|'));
    }

    /// Split a line into its `key=value` tail.
    fn kv(line: &str) -> Vec<(String, String)> {
        line.split_whitespace()
            .filter_map(|tok| {
                tok.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect()
    }

    #[test]
    fn every_line_shape_is_prefixed_parseable_and_space_free() {
        let state = EditingState {
            text: format!("{SENTINEL}a b"),
            selection_base: 5,
            selection_extent: 5,
            composing_base: -1,
            composing_extent: -1,
        };
        let derived = classify(&state);
        let lines = [
            ready_line(false),
            zone_line(1, Zone::Tap, true),
            ime_line(2, "apply", &state, &derived, true, 3),
            key_line(
                3,
                &Key::Named(NamedKey::Enter),
                Modifiers::default(),
                false,
                &classify_key(&Key::Named(NamedKey::Enter)),
                true,
                4,
            ),
            blur_line(4),
        ];
        for line in &lines {
            assert!(line.starts_with(LOG_PREFIX), "{line}");
            // Every token after the shape word is `key=value`, and no value
            // carries whitespace (the escaper's contract).
            for (_, value) in kv(line) {
                assert!(!value.is_empty(), "{line}");
            }
        }
    }

    #[test]
    fn the_ime_line_carries_every_field_the_probe_must_report() {
        let state = EditingState {
            text: format!("{SENTINEL}a"),
            selection_base: 3,
            selection_extent: 3,
            composing_base: -1,
            composing_extent: -1,
        };
        let derived = classify(&state);
        let line = ime_line(7, "apply", &state, &derived, true, 1);
        assert_eq!(
            line,
            "playground keys ime n=7 src=<apply> class=<char> detail=<-> \
             text=<\\u{200b}\\u{200b}|a> len16=3 sel_base=3 sel_ext=3 comp_base=-1 \
             comp_ext=-1 derived=<a> count=1 bytes=61 republish=1 stream_len=1"
        );
        // A space inside the typed text must not create a bogus token.
        let spaced = EditingState {
            text: format!("{SENTINEL} "),
            selection_base: 3,
            selection_extent: 3,
            ..state.clone()
        };
        let spaced_line = ime_line(8, "apply", &spaced, &classify(&spaced), true, 2);
        let fields = kv(&spaced_line);
        assert!(
            fields
                .iter()
                .any(|(k, v)| k == "text" && v == "<\\u{200b}\\u{200b}|\\x20>"),
            "{spaced_line}"
        );
        assert!(fields.iter().any(|(k, v)| k == "bytes" && v == "20"));
    }

    #[test]
    fn the_ready_line_is_self_describing() {
        assert_eq!(
            ready_line(false),
            "playground keys ready sentinel=<\\u{200b}\\u{200b}> sentinel_len16=2 sel_base=2 \
             sel_ext=2 comp_base=-1 comp_ext=-1 active=0 backspace=08 enter=0d log_capacity=20"
        );
    }

    #[test]
    fn the_key_line_names_the_key_and_its_modifiers() {
        let key = Key::Named(NamedKey::Enter);
        let line = key_line(
            2,
            &key,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            true,
            &classify_key(&key),
            true,
            5,
        );
        assert_eq!(
            line,
            "playground keys key n=2 key=<named:enter> mods=<-c--> repeat=1 class=<enter> \
             detail=<-> bytes=0d republish=1 stream_len=5"
        );
        assert_eq!(mods_token(Modifiers::default()), "----");
        assert_eq!(key_token(&Key::Character("a".to_string())), "char:a");
    }

    // -----------------------------------------------------------------
    // Probe bookkeeping
    // -----------------------------------------------------------------

    #[test]
    fn the_probe_rings_its_log_and_stream_but_counts_every_byte() {
        let mut probe = Probe::new();
        for i in 0..(LOG_CAPACITY + STREAM_CAPACITY + 5) {
            let n = probe.next_n();
            assert_eq!(n, i as u64 + 1, "the counter is monotonic");
            probe.record(n, &classify_committed("a"));
        }
        assert_eq!(probe.log.len(), LOG_CAPACITY, "the log is ring-capped");
        assert_eq!(probe.stream.len(), STREAM_CAPACITY, "so is the stream");
        assert_eq!(
            probe.stream_total,
            LOG_CAPACITY + STREAM_CAPACITY + 5,
            "but the total counts every derived byte"
        );
        assert!(probe.stream_text().starts_with("stream: aaa"));
        assert!(probe.log_text().starts_with("log:\n#"));
        assert!(probe.header_text().contains("active=0"));
    }

    #[test]
    fn an_empty_probe_prompts_instead_of_showing_an_empty_log() {
        let probe = Probe::new();
        assert!(probe.log_text().contains("tap the zone"));
        assert_eq!(probe.stream_text(), "stream: ");
        assert!(!probe.active());
    }

    // -----------------------------------------------------------------
    // Zone geometry
    // -----------------------------------------------------------------

    #[test]
    fn the_zones_split_the_row_with_no_dead_space() {
        let size = Size::new(400.0, ZONE_HEIGHT);
        let split = zone_split(size.width);
        assert!((split - 256.0).abs() < 0.001, "{split}");
        assert_eq!(zone_at(Point::new(0.0, 0.0), size), Some(Zone::Tap));
        assert_eq!(
            zone_at(Point::new(split - 1.0, 10.0), size),
            Some(Zone::Tap)
        );
        assert_eq!(zone_at(Point::new(split, 10.0), size), Some(Zone::Toggle));
        assert_eq!(
            zone_at(Point::new(size.width - 1.0, 10.0), size),
            Some(Zone::Toggle)
        );
        // Outside the widget entirely.
        assert_eq!(zone_at(Point::new(-1.0, 10.0), size), None);
        assert_eq!(zone_at(Point::new(10.0, ZONE_HEIGHT), size), None);
        assert_eq!(zone_at(Point::new(size.width, 10.0), size), None);
    }

    // -----------------------------------------------------------------
    // The widget, end to end through a real RenderRoot (no GPU, no device)
    // -----------------------------------------------------------------

    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        glyph_runs: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
            self.rounded.push((origin, size, color));
        }
        fn draw_glyph_run(&mut self, _run: frust_scene::GlyphRun) {
            self.glyph_runs += 1;
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
    }

    /// Install the reactive runtime + an ambient owner (needed for `RwSignal`).
    fn setup() -> Owner {
        let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
        let ambient = Owner::new();
        ambient.set();
        ambient
    }

    const ROOT: Size = Size::new(400.0, ZONE_HEIGHT);

    fn down(x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn apply(text: &str, sel: i32) -> InputEvent {
        InputEvent::Ime(ImeEvent::ApplyEditingState(EditingState {
            text: text.to_string(),
            selection_base: sel,
            selection_extent: sel,
            composing_base: -1,
            composing_extent: -1,
        }))
    }

    /// A `RenderRoot` whose ROOT widget is the probe row, so focus/IME routing is
    /// the real thing (`RenderRoot::event` seeds `has_focus` from `focus_active`)
    /// without depending on the page's column layout.
    #[allow(clippy::type_complexity)]
    fn probe_root() -> (
        RenderRoot<(), KeyProbeView>,
        Rc<RefCell<Probe>>,
        impl FnMut(&mut ()) -> KeyProbeView,
    ) {
        let probe = Rc::new(RefCell::new(Probe::new()));
        let bump = RwSignal::new(0u64);
        let logic = {
            let probe = Rc::clone(&probe);
            move |_s: &mut ()| KeyProbeView {
                generation: 0,
                probe: Rc::clone(&probe),
                bump,
            }
        };
        (RenderRoot::new(), probe, logic)
    }

    #[test]
    fn a_tap_claims_focus_and_publishes_the_sentinel_with_the_keyboard_on() {
        let _owner = setup();
        let (mut root, probe, mut logic) = probe_root();
        let mut tcx = RawTextContext::new();
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(ROOT, &mut tcx as &mut dyn Any);

        // Nothing published before the first tap: there is no programmatic focus,
        // so a probe cannot summon the keyboard at mount.
        assert!(!root.is_focus_active());
        assert!(root.ime_state().is_none());

        let outcome = root.event(&mut (), &down(20.0, 40.0));
        assert!(outcome.handled);
        assert!(root.is_focus_active(), "the tap zone claimed focus");
        let ime = root.ime_state().expect("a sentinel surface was published");
        assert!(ime.active, "the keyboard is asked for");
        assert_eq!(ime.editing, sentinel_editing_state());
        assert!(ime.caret.is_some(), "a caret rect for candidate placement");
        assert!(probe.borrow().active());
    }

    #[test]
    fn a_keystroke_snapshot_derives_a_byte_and_republishes_exactly_the_sentinel() {
        // THE pattern test: this is what Android's `FrustInputConnection.sync()`
        // reads back synchronously after `nativeImeApply`.
        let _owner = setup();
        let (mut root, probe, mut logic) = probe_root();
        let mut tcx = RawTextContext::new();
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(ROOT, &mut tcx as &mut dyn Any);
        root.event(&mut (), &down(20.0, 40.0));

        let outcome = root.event(&mut (), &apply(&format!("{SENTINEL}a"), 3));
        assert!(outcome.handled, "a focused probe consumes the snapshot");
        let ime = root.ime_state().expect("still published");
        assert_eq!(
            ime.editing,
            sentinel_editing_state(),
            "the buffer is pinned back, so it can never grow"
        );
        assert!(ime.active);
        assert_eq!(
            probe.borrow().stream.iter().copied().collect::<Vec<u8>>(),
            vec![b'a']
        );
        assert_eq!(probe.borrow().stream_total, 1);

        // A second keystroke lands at the same offset — the property the
        // re-publish exists to guarantee.
        root.event(&mut (), &apply(&format!("{SENTINEL}b"), 3));
        assert_eq!(
            probe.borrow().stream.iter().copied().collect::<Vec<u8>>(),
            vec![b'a', b'b']
        );

        // A backspace into the prefix, then the restored sentinel's echo.
        root.event(&mut (), &apply("\u{200b}", 1));
        assert_eq!(
            probe.borrow().stream.iter().copied().collect::<Vec<u8>>(),
            vec![b'a', b'b', BACKSPACE_BYTE]
        );
        assert_eq!(
            root.ime_state().map(|s| s.editing),
            Some(sentinel_editing_state())
        );
    }

    #[test]
    fn the_toggle_zone_hides_the_keyboard_without_dropping_focus() {
        // The reason both zones live in ONE widget.
        let _owner = setup();
        let (mut root, probe, mut logic) = probe_root();
        let mut tcx = RawTextContext::new();
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(ROOT, &mut tcx as &mut dyn Any);

        root.event(&mut (), &down(20.0, 40.0));
        assert!(root.ime_state().expect("published").active);

        // A tap in the toggle zone: keyboard off, focus KEPT.
        root.event(&mut (), &down(ROOT.width - 10.0, 40.0));
        assert!(root.is_focus_active(), "focus survives the toggle");
        assert!(!probe.borrow().active());
        // Publishing an INACTIVE surface is a full IME session release at the
        // root, not a value update (`docs/CODE_STANDARDS.md`'s focus/IME
        // conventions), so the root's published surface clears — while the focus
        // path stays exactly where it was. That is the whole point: the keyboard
        // goes away without a blur.
        assert!(
            !root.ime_state().is_some_and(|s| s.active),
            "the keyboard is not asked for"
        );

        // A focused-but-inactive probe still classifies whatever arrives.
        root.event(&mut (), &apply(&format!("{SENTINEL}z"), 3));
        assert_eq!(probe.borrow().stream_total, 1);

        // And toggling back re-asks for the keyboard.
        root.event(&mut (), &down(ROOT.width - 10.0, 40.0));
        assert!(root.ime_state().expect("published").active);
    }

    #[test]
    fn an_ime_snapshot_without_focus_is_ignored() {
        let _owner = setup();
        let (mut root, probe, mut logic) = probe_root();
        let mut tcx = RawTextContext::new();
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(ROOT, &mut tcx as &mut dyn Any);

        let outcome = root.event(&mut (), &apply(&format!("{SENTINEL}a"), 3));
        assert!(!outcome.handled, "focus-gated: nothing to apply to");
        assert_eq!(probe.borrow().stream_total, 0);
        assert!(root.ime_state().is_none());
    }

    #[test]
    fn a_key_event_derives_its_byte_only_while_focused() {
        let _owner = setup();
        let (mut root, probe, mut logic) = probe_root();
        let mut tcx = RawTextContext::new();
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(ROOT, &mut tcx as &mut dyn Any);

        let enter = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        assert!(!root.event(&mut (), &enter).handled, "unfocused: ignored");

        root.event(&mut (), &down(20.0, 40.0));
        assert!(root.event(&mut (), &enter).handled);
        assert_eq!(
            probe.borrow().stream.iter().copied().collect::<Vec<u8>>(),
            vec![ENTER_BYTE],
            "Android's IME_ACTION_DONE path lands here"
        );
        assert_eq!(probe.borrow().key_events, 1);
    }

    #[test]
    fn a_tap_outside_the_zones_blurs_and_clears_the_whole_surface() {
        let _owner = setup();
        let (mut root, _probe, mut logic) = probe_root();
        let mut tcx = RawTextContext::new();
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(ROOT, &mut tcx as &mut dyn Any);

        root.event(&mut (), &down(20.0, 40.0));
        assert!(root.ime_state().is_some());

        // A `Down` this widget claims no focus for is a blur — the exact reason
        // the `active` toggle cannot be a sibling widget.
        root.event(&mut (), &down(-5.0, -5.0));
        assert!(!root.is_focus_active());
        assert!(
            root.ime_state().is_none(),
            "RenderRoot drops the whole published surface on blur"
        );
    }

    #[test]
    fn the_probe_row_paints_two_zones_and_requests_no_frames() {
        let _owner = setup();
        let (mut root, probe, mut logic) = probe_root();
        let mut tcx = RawTextContext::new();
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(ROOT, &mut tcx as &mut dyn Any);

        let mut rec = Recorder::default();
        let outcome = root.paint(&mut rec, FrameTime::from_nanos(0));
        // Two zones, each a ring + an inset fill.
        assert_eq!(rec.rounded.len(), 4, "{:?}", rec.rounded);
        assert_eq!(rec.rounded[0].2, BLUR_RING, "unfocused ring first");
        assert_eq!(rec.rounded[1].2, TAP_FILL);
        assert_eq!(rec.rounded[3].2, TOGGLE_FILL_OFF, "keyboard not asked for");
        assert!(rec.glyph_runs >= 2, "both zone labels shaped");
        assert!(
            !outcome.needs_frame,
            "an event-driven probe costs zero frames at rest"
        );

        // Focused + active re-shapes the toggle label and re-colours both.
        root.event(&mut (), &down(20.0, 40.0));
        assert!(probe.borrow().active());
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(ROOT, &mut tcx as &mut dyn Any);
        let mut rec = Recorder::default();
        root.paint(&mut rec, FrameTime::from_nanos(16_000_000));
        assert_eq!(rec.rounded[0].2, FOCUS_RING, "focused ring");
        assert_eq!(rec.rounded[3].2, TOGGLE_FILL_ON, "keyboard asked for");
    }

    #[test]
    fn a_new_generation_forces_layout_not_just_paint() {
        // The labels re-shape in `layout`, so a bare PAINT would freeze the
        // toggle's `active=` label under Android's intra-frame layout skip.
        let _owner = setup();
        let probe = Rc::new(RefCell::new(Probe::new()));
        let bump = RwSignal::new(0u64);
        let make = |generation: u64| KeyProbeView {
            generation,
            probe: Rc::clone(&probe),
            bump,
        };
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let v0 = make(0);
        let mut widget = View::<()>::build(&v0, &mut ctx);

        assert_eq!(
            View::<()>::rebuild(&make(0), &v0, &mut widget, &mut ctx),
            ChangeFlags::NONE,
            "an unchanged generation is free"
        );
        let bumped = View::<()>::rebuild(&make(1), &v0, &mut widget, &mut ctx);
        assert!(bumped.needs_layout(), "a probe event must relayout");
        assert_eq!(widget.generation, 1);
    }

    #[test]
    fn the_child_list_is_fixed_length_whatever_the_log_holds() {
        // The focus-retention trap: the probe row's index must not move as the
        // log fills, or the positional reconciler drops its focused pod (see
        // `probe_children`'s docs).
        let _owner = setup();
        let bump = RwSignal::new(0u64);
        let row = |probe: &Rc<RefCell<Probe>>| KeyProbeView {
            generation: 0,
            probe: Rc::clone(probe),
            bump,
        };

        let empty = Rc::new(RefCell::new(Probe::new()));
        let full = Rc::new(RefCell::new(Probe::new()));
        {
            let mut probe = full.borrow_mut();
            for _ in 0..(LOG_CAPACITY * 3) {
                let n = probe.next_n();
                probe.record(n, &classify_committed("a"));
            }
            assert_eq!(probe.log.len(), LOG_CAPACITY);
        }

        for probe in [&empty, &full] {
            let (header, stream, log) = {
                let p = probe.borrow();
                (p.header_text(), p.stream_text(), p.log_text())
            };
            let children = probe_children(header, stream, log, row(probe));
            assert_eq!(
                children.len(),
                CHILD_COUNT,
                "the column's length must never depend on the log's contents"
            );
        }
        const { assert!(PROBE_ROW_INDEX < CHILD_COUNT) };
    }

    #[test]
    fn the_component_mounts_and_paints_its_whole_surface() {
        let _owner = setup();
        let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
        let mut logic = |_s: &mut ()| any(component(KeysPage));
        let mut tcx = RawTextContext::new();
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(Size::new(400.0, 800.0), &mut tcx as &mut dyn Any);
        let mut rec = Recorder::default();
        let outcome = root.paint(&mut rec, FrameTime::from_nanos(0));
        assert!(rec.glyph_runs > 0, "header/stream/log/labels all paint");
        assert_eq!(rec.rounded.len(), 4, "the probe row's two zones");
        assert!(!outcome.needs_frame, "inert until touched");
    }

    #[test]
    fn a_tap_through_the_mounted_component_reaches_the_probe_row() {
        // The nesting the on-device build actually uses: Component → Padding →
        // Flex → the probe row, several container hops of focus/IME bubbling.
        // `ime_state()` at the root is exactly what `nativeImeState` serialises.
        let _owner = setup();
        let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
        let mut logic = |_s: &mut ()| any(component(KeysPage));
        let mut tcx = RawTextContext::new();
        let window = Size::new(400.0, 800.0);
        root.rebuild(&mut logic, &mut ());
        root.layout_with_text(window, &mut tcx as &mut dyn Any);

        // The row's y depends on how tall the shaped text above it came out, so
        // scan down the column rather than hard-coding an offset: a tap on plain
        // text is Ignored and mutates nothing, and the first HANDLED tap is the
        // probe row by construction (it is the only interactive child).
        let x = CONTENT_PAD + 10.0;
        let hit_y = (0..(window.height as i64))
            .step_by(4)
            .find(|y| root.event(&mut (), &down(x, *y as f64)).handled);
        let hit_y = hit_y.expect("some tap in the column must reach the probe row");

        assert!(root.is_focus_active());
        let ime = root
            .ime_state()
            .expect("the sentinel surface bubbled up through every container hop");
        assert!(ime.active);
        assert_eq!(ime.editing, sentinel_editing_state());

        // A snapshot now routes back down the recorded focus path and is pinned.
        assert!(
            root.event(&mut (), &apply(&format!("{SENTINEL}q"), 3))
                .handled
        );
        assert_eq!(
            root.ime_state().map(|s| s.editing),
            Some(sentinel_editing_state())
        );

        // The log filling (25 snapshots, past LOG_CAPACITY) must not dismiss the
        // keyboard — the fixed-length-child-list invariant, end to end.
        for _ in 0..25 {
            root.rebuild(&mut logic, &mut ());
            root.layout_with_text(window, &mut tcx as &mut dyn Any);
            assert!(
                root.event(&mut (), &apply(&format!("{SENTINEL}x"), 3))
                    .handled
            );
        }
        assert!(root.is_focus_active(), "focus survived a full log");
        assert!(root.ime_state().is_some_and(|s| s.active));
        // Focus retention is about the row's INDEX in the child list, not its
        // position: a taller log pushes the row further down the (scrolling)
        // column, which is why `hit_y` is deliberately not re-asserted here.
        let _ = hit_y;
    }
}
