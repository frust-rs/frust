//! The plugin-internal [`NativeWidget`] runtime: ONE trait every control
//! implements, dispatched through ONE generic platform factory — **no
//! per-control Kotlin/Swift, ever**.
//!
//! Everything here is platform-agnostic — it dispatches, diffs and book-keeps,
//! handing the two platform-shaped types ([`NativeCtx`]/[`NativeView`]) straight
//! through to the trait impls — which is what makes the whole
//! create/update/dispose/event contract host-testable (this module's `tests`)
//! with no JNI/ObjC dependency at all.
//!
//! # Two kinds of implementation, one dispatch table
//!
//! [`NativeWidget`] has two families of impl and everything below serves both
//! identically: the **built-in controls** (`crate::controls`), whose props
//! arrive decoded from the slot's `params_json`, and every **public
//! [`NativeComponent`](crate::component::NativeComponent)** a plugin author
//! writes (an app crate cannot implement one — see that trait's doc for the FFI
//! wall), reaching this trait through the one `crate::component::Bridge<C>` impl
//! with already-typed props staged beside the wire. Registry, props diff gate, event routing and disposal
//! are therefore the *same* guarantees for both — the point of bridging rather
//! than growing a second runtime. That includes the event half: a component
//! attaches the same one platform listener to a view it built
//! (`crate::component::ComponentCtx::attach_listener`, bound to the slot's
//! own id), and [`NativeRuntime::on_event`] routes its events to the bridge
//! exactly as it routes a built-in control's, the bridge's answer riding the
//! same per-slot callback table ([`NativeRuntime::set_callback`]) the app-facing
//! mount builder registers into.
//!
//! # The generic-factory contract
//!
//! The framework's `platform_view` slot resolves a `viewType` to exactly one
//! factory class per platform — `FrustNativeControlFactory`, Kotlin on Android
//! over this crate's three JNI exports (`crate::android`), a Rust
//! `define_class!` ObjC class on iOS (`crate::apple::factory`), and a Rust
//! `frust_plugin::desktop::DesktopViewFactory` on macOS
//! (`crate::appkit::factory`, keyed by the same `viewType` string); see
//! `docs/NATIVE_WIDGETS_ARCHITECTURE.md` for that one-factory shape and its
//! frozen names. Which *control* a slot means rides in its `params_json`, under
//! two reserved keys the api layer injects and this module reads back:
//!
//! - [`CONTROL_KEY`] (`"__frustControl"`) — the registered kind
//!   ([`NativeRuntime::register`]) whose [`NativeWidget`] impl serves this slot.
//!   This is what replaces "one factory class per control".
//! - [`SLOT_KEY`] (`"__frustSlot"`) — the differ's own `slot_id`, injected
//!   because the platform factory's `createView` is **not** handed one (the
//!   embedding's `FrustPlatformViewFactory` contract predates this plugin);
//!   widening `createView`'s signature was the rejected, breaking alternative.
//!
//! [`with_identity`] is the encoder half of that contract, and
//! [`Params::identity`] the decoder half — the pair round-trips (see `tests`).
//!
//! # Two-phase identity: a widget mounts, `create` arrives later
//!
//! A `platform_view` widget mounting during a frust rebuild does **not** create
//! a native view: the differ (`frust_shell_common::platform_view`) turns that
//! mount into a `Create` command, and the host drains its backlog on the **next
//! post-frame poll**, on the platform main thread — that poll is what finally
//! calls the factory, and therefore [`NativeRuntime::create`]. Three
//! consequences the whole runtime is shaped around:
//!
//! 1. **Per-instance state lives here, not in the widget.** A control's
//!    [`NativeWidget::State`] is created at attach time and retained in this
//!    runtime's slot-keyed registry, because at mount time there is nothing
//!    native to hold.
//! 2. **Props coalesce until attach.** Params changes between the mount and the
//!    attach never reach a native view — the differ's `Create` carries the
//!    params as of the ingest that emitted it, and any later change rides an
//!    `UpdateParams` *after* it in the same ordered backlog. Params are
//!    therefore always the **whole** state of a slot, never a delta: replaying a
//!    backlog prefix (a surface-recreate replay, a compaction) lands in the same
//!    place.
//! 3. **An event callback is registered before its instance exists**, so an
//!    [`Instance`] is *born* with it: the api layer (`crate::api::builders`)
//!    calls [`NativeRuntime::set_callback`] from the same rebuild that mounts
//!    the slot, always before the create above, so a registration naming a slot
//!    with no live instance is parked in `pending_callbacks` and taken by
//!    [`NativeRuntime::create`] rather than dropped. Dropping it would leave a
//!    visible, tappable, permanently dead control, because nothing schedules a
//!    retry frame on an idle screen (a native tap produces no frust frame, and
//!    forcing one would forfeit Mode B's zero-frames-at-rest property). The
//!    replay direction follows: a surface-recreate replay re-creates an instance
//!    with no new registration, so a create with nothing pending inherits the
//!    callback of the instance it replaces.
//!
//! # The props diff gate is Rust-side
//!
//! [`NativeRuntime::update_params`] decodes the new params into the control's
//! typed `Props` and compares them with the last applied ones via `PartialEq`
//! **before** any platform call, so an unchanged rebuild costs one JSON decode
//! plus one comparison — nanoseconds — and **zero** FFI crossings. Field-level
//! diffing *within* a changed props struct is the impl's job in
//! [`NativeWidget::update`]: only it knows which setter is cheap (~0.8 µs,
//! invalidate-only) and which triggers a re-layout (~29 µs, `setText`).
//!
//! # Late and duplicate disposal are normal
//!
//! The differ disposes a slot on a *missing streak of ingests*, and ingests only
//! happen on gate-`Run` frames, so an unmounted control's `Dispose` can arrive
//! many frames late — or after a replacement `Create` already re-used the slot
//! id. Both platforms' dispose entry point is handed the **view object**, not a
//! slot id, so disposal resolves by identity ([`NativeRuntime::take_matching`],
//! over [`Registry::remove_matching`]): a dispose naming a view that is already
//! gone finds nothing, is a silent no-op, and never touches whatever instance
//! now occupies that slot.
//!
//! # Main-thread confinement
//!
//! Every entry point here runs on the platform main thread — the host polls its
//! command backlog there, and the platform's own listeners fire there — so the
//! runtime is a `thread_local!` ([`with_runtime`]) rather than a `Mutex`-guarded
//! global: a control's `State` may hold main-thread-only platform handles (iOS's
//! `Retained<UIView>` is `!Send`), which a global would have to forbid.
//! [`with_runtime`] is also **re-entrancy tolerant** — a platform setter that
//! synchronously fires its own listener (Android's `setChecked` is the classic
//! case) would otherwise re-enter the runtime mid-update, so a re-entrant call
//! reports `None` and is dropped with a warning instead of panicking on the
//! borrow.

use std::any::Any;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::NativeWidgetError;
use crate::events::EventPayload;
use crate::registry::{Registry, SlotId};

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
pub(crate) use self::host::{NativeCtx, NativeView};
#[cfg(target_os = "android")]
pub(crate) use crate::android::{NativeCtx, NativeView};
#[cfg(target_os = "macos")]
pub(crate) use crate::appkit::{NativeCtx, NativeView};
#[cfg(target_os = "ios")]
pub(crate) use crate::apple::{NativeCtx, NativeView};

/// The reserved `params_json` key naming which registered [`NativeWidget`]
/// kind a slot means — see the module doc's *generic-factory contract*.
pub(crate) const CONTROL_KEY: &str = "__frustControl";

/// The reserved `params_json` key carrying the differ's `slot_id` across a
/// factory contract that does not pass it — see the module doc's
/// *generic-factory contract*.
pub(crate) const SLOT_KEY: &str = "__frustSlot";

// --- params: the flat-JSON wire between the api layer and the runtime -------

/// A read-only view over one slot's `params_json`.
///
/// Deliberately a tiny hand-rolled reader rather than `serde`: this crate is a
/// platform plugin (`frust-plugin` + FFI crates only), the payload is a flat
/// object **both** ends of which are ours (the api layer encodes with
/// [`with_identity`], a [`NativeWidget`] decodes here), and hand-rolled JSON
/// at a mobile FFI boundary is the established rule
/// (`docs/CODE_STANDARDS.md`'s Language Idioms).
///
/// The scan is escape- and nesting-aware — a key that only *looks* like a key
/// because it sits inside a string value (`{"text":"\"enabled\":true"}`) is
/// never matched, and a nested object/array value is skipped as a unit — but
/// it deliberately supports no more than that: flat objects of strings,
/// numbers and booleans.
pub(crate) struct Params<'a> {
    raw: &'a str,
}

impl<'a> Params<'a> {
    /// Wrap a raw `params_json` payload. Parsing is lazy and per-key.
    pub(crate) fn new(raw: &'a str) -> Self {
        Self { raw }
    }

    /// The two reserved identity keys the api layer injected: the registered
    /// control kind and the differ's slot id (module doc's *generic-factory
    /// contract*).
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when either key is missing or has the
    /// wrong shape — a slot whose params carry no identity cannot be
    /// dispatched at all, so this is an error rather than a tolerated no-op.
    pub(crate) fn identity(&self) -> Result<(Cow<'a, str>, SlotId), NativeWidgetError> {
        let kind = self.string(CONTROL_KEY).ok_or_else(|| {
            NativeWidgetError::Params(format!("params carry no `{CONTROL_KEY}` control kind"))
        })?;
        let slot = self.int(SLOT_KEY).ok_or_else(|| {
            NativeWidgetError::Params(format!("params carry no `{SLOT_KEY}` slot id"))
        })?;
        let slot = SlotId::try_from(slot)
            .map_err(|_| NativeWidgetError::Params(format!("`{SLOT_KEY}` is negative ({slot})")))?;
        Ok((kind, slot))
    }

    /// A string field, with JSON escapes resolved. `None` when the key is
    /// absent or its value is not a string.
    pub(crate) fn string(&self, key: &str) -> Option<Cow<'a, str>> {
        match value_of(self.raw, key)? {
            Value::Str(raw) => Some(unescape(raw)),
            _ => None,
        }
    }

    /// An integer field. `None` when the key is absent or its value is not an
    /// integer literal.
    pub(crate) fn int(&self, key: &str) -> Option<i64> {
        match value_of(self.raw, key)? {
            Value::Number(raw) => raw.parse().ok(),
            _ => None,
        }
    }

    /// A floating-point field (an integer literal reads back fine too).
    /// `None` when the key is absent or its value is not a number.
    pub(crate) fn float(&self, key: &str) -> Option<f64> {
        match value_of(self.raw, key)? {
            Value::Number(raw) => raw.parse().ok(),
            _ => None,
        }
    }

    /// A boolean field. `None` when the key is absent or its value is not
    /// `true`/`false`.
    pub(crate) fn flag(&self, key: &str) -> Option<bool> {
        match value_of(self.raw, key)? {
            Value::Bool(v) => Some(v),
            _ => None,
        }
    }
}

/// The encoder half of the identity contract (module doc): prefix a control's
/// own flat-JSON `body` — the fields *without* the enclosing braces, e.g.
/// `"text":"Save","enabled":true` — with the two reserved identity keys.
///
/// The api layer is the production caller; the round trip through
/// [`Params::identity`] is pinned by this module's tests.
///
/// # This is also the iOS factory-registration trigger
///
/// Calling this function means one thing exactly: *the api layer is encoding
/// the params for a `platform_view` slot that will resolve this plugin's
/// factory*. On iOS that factory class is registered with the Objective-C
/// runtime **lazily**, and nothing on the host side can trigger it (see
/// [`ensure_platform_factory`] and `crate::apple::factory`'s *Registration is
/// LAZY*) — so the encode is exactly the right, and the only crate-internal,
/// place to force it: every builder calls this once per rebuild, on the same
/// rebuild that publishes its slot, a whole frame before the host's
/// post-frame poll can look the class up by name. A builder that degrades to
/// its frust-drawn placeholder (`ResolvedSurfaceMode::RefusedTranslucent`)
/// publishes no slot and never reaches here — correctly, since there is then
/// no factory lookup to be ready for. The same holds on macOS, where the
/// desktop host looks the factory up in `frust_plugin::desktop`'s registry by
/// `view_type` on the first `Create` it drains. The call is a `Once` on iOS
/// and macOS behind an inlined no-op on every other target.
pub(crate) fn with_identity(kind: &str, slot_id: SlotId, body: &str) -> String {
    ensure_platform_factory();
    let mut out = String::with_capacity(body.len() + kind.len() + 48);
    out.push('{');
    out.push('"');
    out.push_str(CONTROL_KEY);
    out.push_str("\":\"");
    out.push_str(&escape(kind));
    out.push_str("\",\"");
    out.push_str(SLOT_KEY);
    out.push_str("\":");
    out.push_str(&slot_id.to_string());
    if !body.is_empty() {
        out.push(',');
        out.push_str(body);
    }
    out.push('}');
    out
}

/// Make sure this build's platform factory exists before the host can look it
/// up — the crate's one platform-registration hook.
///
/// - **iOS**: forces the lazy Objective-C-runtime registration of the Rust
///   `define_class!` factory class (`crate::apple::ensure_registered`), which
///   `FrustViewHost` resolves by name via `NSClassFromString`. Idempotent; a
///   `Once` after the first call. See `crate::apple::factory`'s *Registration
///   is LAZY* for the ordering contract.
/// - **macOS**: registers the AppKit arm's `DesktopViewFactory` with
///   `frust_plugin::desktop::register_view_factory` under the api layer's
///   `VIEW_TYPE` (`crate::appkit::ensure_registered`), which the desktop
///   Mode-A host resolves on its first `Create` for one of this plugin's
///   slots. The desktop registry is first-registration-wins, so the `Once`
///   inside is what makes this safe to call on every encode.
/// - **Android**: nothing to do. The factory is a Kotlin class the app module
///   already carries, found through the app classloader — it exists whether or
///   not Rust has run.
/// - **Everywhere else**: nothing to do; there is no factory.
///
/// [`with_identity`] calls this on every params encode (see its doc for why
/// that is the right trigger), and `crate::api::ensure_native_factory_registered`
/// re-exports it as an explicit, app-callable front door for anyone who wants
/// registration to happen earlier still.
#[inline]
pub(crate) fn ensure_platform_factory() {
    #[cfg(target_os = "ios")]
    crate::apple::ensure_registered();
    #[cfg(target_os = "macos")]
    crate::appkit::ensure_registered();
}

/// Escape a string for embedding in a JSON string literal (the encoder half's
/// helper — the api layer's own field encoding reuses it).
pub(crate) fn escape(value: &str) -> Cow<'_, str> {
    if !value
        .chars()
        .any(|c| matches!(c, '"' | '\\') || c.is_control())
    {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len() + 8);
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// One flat-object value, as it appears in the raw payload.
enum Value<'a> {
    /// The *raw* (still escaped) contents between the quotes.
    Str(&'a str),
    /// The raw numeric literal.
    Number(&'a str),
    /// A `true`/`false` literal.
    Bool(bool),
    /// `null`, or a nested object/array (skipped as a unit).
    Other,
}

/// Scan a flat JSON object for `key`, returning its raw value.
///
/// Single pass, escape-aware, nesting-skipping — see [`Params`]'s doc for the
/// deliberate limits.
fn value_of<'a>(raw: &'a str, key: &str) -> Option<Value<'a>> {
    let bytes = raw.as_bytes();
    let mut i = skip_ws(bytes, 0);
    if bytes.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;
    loop {
        i = skip_ws(bytes, i);
        match bytes.get(i) {
            Some(b'}') | None => return None,
            Some(b',') => {
                i += 1;
                continue;
            }
            Some(b'"') => {}
            Some(_) => return None,
        }
        let (name, next) = scan_string(bytes, i)?;
        i = skip_ws(bytes, next);
        if bytes.get(i) != Some(&b':') {
            return None;
        }
        i = skip_ws(bytes, i + 1);
        let (value, next) = scan_value(raw, i)?;
        if unescape(&raw[name.clone()]) == key {
            return Some(value);
        }
        i = next;
    }
}

/// Advance past ASCII whitespace.
fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while matches!(bytes.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        i += 1;
    }
    i
}

/// Scan a quoted string starting at `i` (which must be the opening quote),
/// returning the byte range of its contents and the index just past the
/// closing quote.
fn scan_string(bytes: &[u8], i: usize) -> Option<(std::ops::Range<usize>, usize)> {
    debug_assert_eq!(bytes.get(i), Some(&b'"'));
    let start = i + 1;
    let mut j = start;
    while let Some(&b) = bytes.get(j) {
        match b {
            b'\\' => j += 2,
            b'"' => return Some((start..j, j + 1)),
            _ => j += 1,
        }
    }
    None
}

/// Scan one value starting at `i`, returning it and the index just past it.
fn scan_value(raw: &str, i: usize) -> Option<(Value<'_>, usize)> {
    let bytes = raw.as_bytes();
    match bytes.get(i)? {
        b'"' => {
            let (range, next) = scan_string(bytes, i)?;
            Some((Value::Str(&raw[range]), next))
        }
        b'{' | b'[' => Some((Value::Other, skip_nested(bytes, i)?)),
        b't' if raw[i..].starts_with("true") => Some((Value::Bool(true), i + 4)),
        b'f' if raw[i..].starts_with("false") => Some((Value::Bool(false), i + 5)),
        b'n' if raw[i..].starts_with("null") => Some((Value::Other, i + 4)),
        _ => {
            let mut j = i;
            while matches!(
                bytes.get(j),
                Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
            ) {
                j += 1;
            }
            (j > i).then(|| (Value::Number(&raw[i..j]), j))
        }
    }
}

/// Skip a nested object/array starting at `i`, returning the index just past
/// its closing bracket.
fn skip_nested(bytes: &[u8], i: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = i;
    while let Some(&b) = bytes.get(j) {
        match b {
            b'"' => {
                let (_, next) = scan_string(bytes, j)?;
                j = next;
                continue;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j + 1);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// Resolve JSON escapes in a raw string body, borrowing when there are none.
fn unescape(raw: &str) -> Cow<'_, str> {
    if !raw.contains('\\') {
        return Cow::Borrowed(raw);
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    // An unpaired surrogate (or malformed escape) becomes the
                    // replacement character rather than failing the whole
                    // decode: a control's label is not worth a dead slot.
                    None => out.push('\u{fffd}'),
                }
            }
            Some(other) => out.push(other),
            None => break,
        }
    }
    Cow::Owned(out)
}

// --- events -----------------------------------------------------------------

/// A platform listener callback, as the generic listener glue delivers it —
/// the raw `(kind, detail)` wire [`NativeRuntime::on_event`] decodes into the
/// crate's typed [`EventPayload`](crate::events::EventPayload) vocabulary
/// (`crate::events`).
///
/// **Native-widget events bypass `RenderRoot::event` entirely** (see the crate
/// doc): this is a platform interaction surfacing as a callback, not a frust
/// pointer event — no `EventCtx`, no capture/focus, none of
/// `docs/CODE_STANDARDS.md`'s Interaction Semantics apply.
///
/// The payload stays primitive on purpose: ONE
/// `dev.frust.nativewidgets.FrustNativeListener`
/// class funnels every listener interface into one native method
/// `(slotId, kind, detail)`, so a value event packs its payload into the
/// `detail` bits rather than allocating JSON on the hot path — see
/// `crate::events`'s kind table and detail codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeEvent {
    /// Which listener fired — one of `crate::events`'s `EVENT_KIND_*` codes,
    /// shared verbatim with `FrustNativeListener`'s companion constants.
    pub(crate) kind: i32,
    /// The listener's primitive payload; `0` for a kind that carries none.
    pub(crate) detail: i64,
}

// --- the trait --------------------------------------------------------------

/// One retained native control (or control *hierarchy*) driven entirely from
/// Rust.
///
/// Every method runs on the platform main thread. The trait stays **`pub(crate)`
/// — permanently**: it is this crate's
/// *internal* dispatch contract, and the app/plugin-facing shape is
/// [`crate::component::NativeComponent`], bridged onto this one by
/// `component::Bridge<C>`. Keeping the two separate is what lets the wire-facing
/// half here (a `decode_props` step, `Result` returns, an `EventPayload`
/// callback channel) keep evolving without breaking a third-party impl — and it
/// is why the built-in controls did not have to move when the public
/// trait landed.
///
/// The methods are associated functions, not `&self` methods: a registration
/// ([`NativeRuntime::register`]) names a *type*, never an instance, so there
/// is no per-kind object to borrow — everything per-instance lives in
/// [`Self::State`].
pub(crate) trait NativeWidget: 'static {
    /// The Rust-side-diffed create/update payload. `PartialEq` is the gate
    /// that keeps an unchanged rebuild from crossing the FFI boundary at all
    /// (module doc's *props diff gate*).
    type Props: Clone + PartialEq + Send + 'static;

    /// Per-instance retained state: native handles, listeners, buffers.
    /// Created at attach time, kept in the runtime's slot registry (module
    /// doc's *two-phase identity*).
    type State: 'static;

    /// Decode this control's typed props out of a slot's `params_json`.
    ///
    /// The api layer encodes the same fields; a missing optional
    /// field should fall back to the control's default rather than failing,
    /// so a params payload from an older/newer api layer degrades instead of
    /// killing the slot.
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when a **required** field is missing or
    /// malformed.
    fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError>;

    /// Build the real native view (hierarchy) for a freshly attached slot and
    /// return it together with the state that drives it.
    ///
    /// Runs inside the platform factory's `createView`, on the main thread.
    /// Every reference the control retains past this call must be owned by
    /// the returned [`NativeView`] (Android: global refs, paired-deleted on
    /// dispose — ART aborts the process at 51,200 live global refs).
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] on any platform failure; the slot is
    /// then reported dead to the host rather than half-created.
    fn create(
        ctx: &mut NativeCtx<'_, '_>,
        props: &Self::Props,
    ) -> Result<(NativeView, Self::State), NativeWidgetError>;

    /// Apply a props change to the live control with direct setters on its
    /// retained handles.
    ///
    /// Only called when `old != new` (module doc's *props diff gate*);
    /// field-level diffing inside is the impl's job, since only it knows
    /// which setter is cheap and which forces a re-layout.
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] on any platform failure; the runtime
    /// keeps the previously applied props as the diff baseline so the change
    /// is retried on the next differing params.
    fn update(
        ctx: &mut NativeCtx<'_, '_>,
        state: &mut Self::State,
        old: &Self::Props,
        new: &Self::Props,
    ) -> Result<(), NativeWidgetError>;

    /// A platform listener fired for this instance (main thread): decode the
    /// raw `(kind, detail)` pair into the crate's typed
    /// [`EventPayload`](crate::events::EventPayload) vocabulary, or return
    /// `None` to swallow an event this crate has no vocabulary for.
    /// [`NativeRuntime::on_event`] forwards whatever this returns to the
    /// slot's registered `Arc<dyn Fn(EventPayload) + Send + Sync>` callback
    /// (the app-facing api wraps that into a signal write).
    ///
    /// **This `None` seam is not the echo guard.** A programmatic
    /// echo — the listener a control's own `update` provokes synchronously —
    /// never reaches this method at all: it re-enters the thread-local
    /// runtime, fails `try_borrow_mut`, and is dropped one layer up in
    /// [`with_runtime`]. `crate::controls`'s module doc carries the full
    /// account, including why that guard is incidental to the call site
    /// rather than a designed invariant; `tests::the_thread_local_runtime_is_reentrancy_tolerant`
    /// pins it.
    ///
    /// Defaults to `None` unconditionally: a display-only control (Label,
    /// ProgressBar, Image) emits nothing and never overrides this.
    fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
        let _ = (state, event);
        None
    }

    /// The slot is going away: detach listeners and release anything the
    /// state owns beyond the [`NativeView`] itself (which the runtime drops
    /// immediately afterwards — the paired delete).
    ///
    /// # Errors
    /// [`NativeWidgetError::Platform`] on a platform failure. Teardown has
    /// nobody to report to, so the runtime only logs it; the view's
    /// references are released either way.
    fn dispose(ctx: &mut NativeCtx<'_, '_>, state: Self::State) -> Result<(), NativeWidgetError>;
}

// --- type erasure -----------------------------------------------------------

/// One registered kind's dispatch entry: the [`NativeWidget`] impl's methods
/// with `Props`/`State` erased to `dyn Any`, so a single `HashMap` can hold
/// every control the backend serves.
///
/// Plain function pointers (no boxing, `Copy`): each is a monomorphised shim
/// below, and the erasure is *internal* — a downcast here can only fail if
/// the runtime paired an instance with the wrong kind's vtable, which is the
/// documented panic-on-mismatch contract for type erasure
/// (`docs/CODE_STANDARDS.md`'s Language Idioms).
#[derive(Clone, Copy)]
struct KindVTable {
    decode: DecodeFn,
    create: CreateFn,
    update: UpdateFn,
    props_eq: PropsEqFn,
    on_event: EventFn,
    dispose: DisposeFn,
}

/// Erased [`NativeWidget::decode_props`] (see [`KindVTable`]).
type DecodeFn = fn(&Params<'_>) -> Result<Box<dyn Any>, NativeWidgetError>;
/// Erased [`NativeWidget::create`]: `(ctx, props) -> (view, state)`.
type CreateFn =
    fn(&mut NativeCtx<'_, '_>, &dyn Any) -> Result<(NativeView, Box<dyn Any>), NativeWidgetError>;
/// Erased [`NativeWidget::update`]: `(ctx, state, old props, new props)`.
type UpdateFn =
    fn(&mut NativeCtx<'_, '_>, &mut dyn Any, &dyn Any, &dyn Any) -> Result<(), NativeWidgetError>;
/// Erased `Props: PartialEq` — the diff gate itself.
type PropsEqFn = fn(&dyn Any, &dyn Any) -> bool;
/// Erased [`NativeWidget::on_event`]: `(state, event) -> decoded payload`.
type EventFn = fn(&mut dyn Any, NativeEvent) -> Option<EventPayload>;
/// Erased [`NativeWidget::dispose`]: `(ctx, state)`, taking the state by
/// value.
type DisposeFn = fn(&mut NativeCtx<'_, '_>, Box<dyn Any>) -> Result<(), NativeWidgetError>;

impl KindVTable {
    /// The vtable for `W` — every entry a monomorphised shim that downcasts
    /// back to `W`'s own types.
    fn of<W: NativeWidget>() -> Self {
        Self {
            decode: |params| W::decode_props(params).map(|p| Box::new(p) as Box<dyn Any>),
            create: |ctx, props| {
                let (view, state) = W::create(ctx, downcast::<W::Props>(props, "props"))?;
                Ok((view, Box::new(state) as Box<dyn Any>))
            },
            update: |ctx, state, old, new| {
                W::update(
                    ctx,
                    downcast_mut::<W::State>(state, "state"),
                    downcast::<W::Props>(old, "old props"),
                    downcast::<W::Props>(new, "new props"),
                )
            },
            props_eq: |a, b| downcast::<W::Props>(a, "props") == downcast::<W::Props>(b, "props"),
            on_event: |state, event| W::on_event(downcast_mut::<W::State>(state, "state"), event),
            dispose: |ctx, state| {
                W::dispose(
                    ctx,
                    *state
                        .downcast::<W::State>()
                        .unwrap_or_else(|_| panic!("{ERASURE_BUG}: state")),
                )
            },
        }
    }
}

/// Panic message prefix for the erasure invariant (see [`KindVTable`]).
const ERASURE_BUG: &str = "frust-native-widgets: instance paired with another kind's vtable";

/// Recover a concrete erased value; see [`KindVTable`]'s panic-on-mismatch
/// contract.
fn downcast<'a, T: 'static>(value: &'a dyn Any, what: &str) -> &'a T {
    value
        .downcast_ref::<T>()
        .unwrap_or_else(|| panic!("{ERASURE_BUG}: {what}"))
}

/// [`downcast`]'s mutable counterpart.
fn downcast_mut<'a, T: 'static>(value: &'a mut dyn Any, what: &str) -> &'a mut T {
    value
        .downcast_mut::<T>()
        .unwrap_or_else(|| panic!("{ERASURE_BUG}: {what}"))
}

// --- instances --------------------------------------------------------------

/// One live control: its native view, its last applied props (the diff
/// baseline), its retained state, the vtable that types all three, and the
/// app-facing callback its decoded events are forwarded to.
pub(crate) struct Instance {
    kind: &'static str,
    view: NativeView,
    props: Box<dyn Any>,
    state: Box<dyn Any>,
    vtable: KindVTable,
    /// The callback a slot's decoded [`EventPayload`]s are handed to. Set at
    /// birth from [`NativeRuntime::create`]'s pending table (module doc's
    /// *two-phase identity* 3) — the app-facing api registers via
    /// [`NativeRuntime::set_callback`] a frame or more earlier — and `None`
    /// only for a slot nobody ever registered one for. Not type-erased like
    /// `props`/`state`: its type
    /// (`Arc<dyn Fn(EventPayload) + Send + Sync>`) is already uniform across
    /// every control kind.
    callback: Option<Arc<dyn Fn(EventPayload) + Send + Sync>>,
}

impl Instance {
    /// The native view this instance owns — what a platform dispose export
    /// matches against by identity, and what the create export hands back to
    /// the factory.
    pub(crate) fn view(&self) -> &NativeView {
        &self.view
    }

    /// The registered kind serving this instance.
    pub(crate) fn kind(&self) -> &'static str {
        self.kind
    }

    /// Tear the instance down: the control's own `dispose` first (detach
    /// listeners, release anything the state owns), then the view's retained
    /// references, dropped here — the paired delete, in that order.
    ///
    /// # Errors
    /// Whatever the control's [`NativeWidget::dispose`] reported. The view's
    /// references are released regardless.
    pub(crate) fn dispose(self, ctx: &mut NativeCtx<'_, '_>) -> Result<(), NativeWidgetError> {
        let Self {
            kind: _,
            view,
            props: _,
            state,
            vtable,
            callback: _,
        } = self;
        let outcome = (vtable.dispose)(ctx, state);
        // Explicit, and explicitly *after* the control's own teardown: on
        // Android this releases every global reference the control retained
        // (`Global`'s `Drop`) — the paired delete. `allow` because the host
        // stand-in is a plain struct with nothing to release.
        #[allow(clippy::drop_non_drop)]
        drop(view);
        outcome
    }
}

/// What [`NativeRuntime::update_params`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateOutcome {
    /// Props differed and the control applied them.
    Applied,
    /// Props compared equal — the diff gate stopped before any platform call
    /// (module doc's *props diff gate*).
    Unchanged,
    /// No live instance for that slot: a params update for a slot whose
    /// create failed, or which was already disposed. Tolerated, never an
    /// error.
    UnknownSlot,
}

/// What [`NativeRuntime::dispose_slot`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisposeOutcome {
    /// The instance was found and torn down.
    Disposed,
    /// Nothing to tear down — a late or duplicate dispose (module doc).
    NotFound,
}

/// What [`NativeRuntime::on_event`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EventOutcome {
    /// Routed to the slot's control.
    Delivered,
    /// The slot has no live instance — a listener firing during teardown.
    UnknownSlot,
}

// --- the runtime ------------------------------------------------------------

/// The dispatch table plus the live-instance registry — the whole runtime.
///
/// Constructed once per thread (see [`with_runtime`]); host tests build their
/// own instead.
pub(crate) struct NativeRuntime {
    kinds: HashMap<&'static str, KindVTable>,
    instances: Registry<Instance>,
    /// Callbacks registered for slots whose native [`Instance`] does not
    /// exist **yet** — the normal case, since a registration rides the
    /// rebuild that mounts the slot and the create only arrives on the
    /// host's next post-frame poll (module doc's *two-phase identity* 3).
    /// [`Self::create`] drains a slot's entry into the instance it builds. An
    /// entry here and a live instance for the same slot are mutually
    /// exclusive — [`Self::set_callback`] writes to exactly one of the two.
    ///
    /// **Not bounded by [`Self::dispose_slot`] alone.** An earlier
    /// version of this doc claimed `dispose_slot` clearing this table was
    /// enough to keep it from stranding a closure — wrong for the same
    /// reason `crate::controls::image`'s "bounded by the live image slots"
    /// claim was: Android's production dispose
    /// (`FrustNativeControlFactory.disposeView` →
    /// `crate::android::dispose_control` → [`Self::take_matching`]) resolves
    /// by view identity and never calls `dispose_slot` at all, so a slot
    /// disposed while culled and then re-registered — still mounted, so it
    /// never runs `View::teardown` — parks a second pending entry
    /// `dispose_slot` never sees. [`Self::forget_pending_callback`] is the
    /// actual bound: registered via `on_cleanup` in each interactive
    /// builder's `Component::init` (`crate::api::builders`), it runs exactly
    /// once per mounted Component regardless of how many times (if any) the
    /// native create/dispose pair ran on the platform side in between.
    /// `dispose_slot`'s own clear stays a secondary safety net for its own
    /// callers (the create-rollback branch, tests), not the production
    /// reclaim path.
    pending_callbacks: HashMap<SlotId, Arc<dyn Fn(EventPayload) + Send + Sync>>,
}

impl NativeRuntime {
    /// An empty runtime: no kinds registered, no live instances.
    pub(crate) fn new() -> Self {
        Self {
            kinds: HashMap::new(),
            instances: Registry::new(),
            pending_callbacks: HashMap::new(),
        }
    }

    /// Register `W` under `kind` — the string the api layer injects as
    /// [`CONTROL_KEY`].
    ///
    /// Explicit registration is deliberate: `inventory`-style auto-registration
    /// is banned here (link-time discovery is exactly the kind of thing that
    /// silently fails on a device build).
    ///
    /// Re-registering a kind replaces its vtable and returns `false`, which
    /// only a double-registration bug can produce; live instances of the old
    /// vtable keep their own copy and stay coherent.
    pub(crate) fn register<W: NativeWidget>(&mut self, kind: &'static str) -> bool {
        let fresh = self.kinds.insert(kind, KindVTable::of::<W>()).is_none();
        if !fresh {
            log::warn!("frust-native-widgets: control kind '{kind}' registered twice");
        }
        fresh
    }

    /// Register `W` under `kind` **only if that kind is still free** — the
    /// first-wins rule the public registration path
    /// (`crate::component::register_component`) needs, and the one difference
    /// between it and [`Self::register`].
    ///
    /// A third-party [`NativeComponent`](crate::component::NativeComponent)
    /// must not be able to shadow one of the built-in controls (or another
    /// plugin's component) by claiming a kind string already taken: the
    /// backend registers its own kinds when the thread's runtime is first
    /// touched ([`seeded_runtime`]), so a collision here is either a
    /// double-registration bug or a name clash, and in both cases keeping the
    /// incumbent is the safe answer. Returns whether the registration was
    /// accepted.
    pub(crate) fn register_if_free<W: NativeWidget>(&mut self, kind: &'static str) -> bool {
        if self.kinds.contains_key(kind) {
            log::warn!(
                "frust-native-widgets: control kind '{kind}' is already registered — keeping the \
                 existing one"
            );
            return false;
        }
        self.register::<W>(kind)
    }

    /// Whether `kind` has a registered [`NativeWidget`].
    pub(crate) fn is_registered(&self, kind: &str) -> bool {
        self.kinds.contains_key(kind)
    }

    /// Attach a slot: dispatch on the params' identity, decode the control's
    /// props, build the native view, and retain the instance under the
    /// slot id. Returns the slot id the params named.
    ///
    /// A create for a slot id that is **already live** (a surface-recreate
    /// replay, or a rapid recreate) builds the replacement first and only
    /// then replaces and disposes the previous instance, so a failed create
    /// leaves the existing control untouched.
    ///
    /// The new instance is **born with its event callback** (module doc's
    /// *two-phase identity* 3), from two sources in order: this slot's
    /// pending registration, else — for a replay, which re-creates the
    /// instance with no new registration at all — the callback of the
    /// instance being replaced. Neither source requests a frame; on an idle
    /// screen there would be none to request.
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the identity keys are missing,
    /// [`NativeWidgetError::UnknownControl`] when no kind is registered under
    /// that name, or whatever the control's own `decode_props`/`create`
    /// reported.
    pub(crate) fn create(
        &mut self,
        ctx: &mut NativeCtx<'_, '_>,
        params_json: &str,
    ) -> Result<SlotId, NativeWidgetError> {
        let params = Params::new(params_json);
        let (kind, slot_id) = params.identity()?;
        let (kind, vtable) = self
            .kinds
            .get_key_value(kind.as_ref())
            .map(|(name, vtable)| (*name, *vtable))
            .ok_or_else(|| NativeWidgetError::UnknownControl(kind.into_owned()))?;

        let props = (vtable.decode)(&params)?;
        let (view, state) = (vtable.create)(ctx, &*props)?;
        // Taken only now that the create actually succeeded: a failed create
        // must leave the registration pending for the next attempt. Pending
        // first, the replaced instance's own callback second (the replay
        // path) — see this method's doc.
        let callback = self
            .pending_callbacks
            .remove(&slot_id)
            .or_else(|| self.instances.get(slot_id).and_then(|p| p.callback.clone()));
        let instance = Instance {
            kind,
            view,
            props,
            state,
            vtable,
            callback,
        };

        if let Some(previous) = self.instances.insert(slot_id, instance) {
            log::warn!(
                "frust-native-widgets: create for live slot {slot_id} ('{kind}') — replacing"
            );
            if let Err(e) = previous.dispose(ctx) {
                log::warn!("frust-native-widgets: disposing replaced slot {slot_id}: {e}");
            }
        }
        Ok(slot_id)
    }

    /// The live instance for `slot_id`, if any.
    pub(crate) fn instance(&self, slot_id: SlotId) -> Option<&Instance> {
        self.instances.get(slot_id)
    }

    /// Apply a params change to the slot the params name (module doc's *props
    /// diff gate*).
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the identity keys are missing or the
    /// params name a different kind than the live instance, or whatever the
    /// control's own `decode_props`/`update` reported.
    pub(crate) fn update_params(
        &mut self,
        ctx: &mut NativeCtx<'_, '_>,
        params_json: &str,
    ) -> Result<UpdateOutcome, NativeWidgetError> {
        let params = Params::new(params_json);
        let (kind, slot_id) = params.identity()?;
        let Some(instance) = self.instances.get_mut(slot_id) else {
            // Tolerated: a params update can outlive the slot it names (a
            // create that failed, a dispose already applied).
            return Ok(UpdateOutcome::UnknownSlot);
        };
        if instance.kind != kind {
            return Err(NativeWidgetError::Params(format!(
                "slot {slot_id} is a '{}' but its params now say '{kind}'",
                instance.kind
            )));
        }

        let vtable = instance.vtable;
        let new_props = (vtable.decode)(&params)?;
        if (vtable.props_eq)(&*instance.props, &*new_props) {
            return Ok(UpdateOutcome::Unchanged);
        }
        (vtable.update)(ctx, &mut *instance.state, &*instance.props, &*new_props)?;
        instance.props = new_props;
        Ok(UpdateOutcome::Applied)
    }

    /// Route a platform listener callback to its slot's control, decode it
    /// into the typed [`EventPayload`] vocabulary via the control's own
    /// [`NativeWidget::on_event`], and — unless that decode returned `None`
    /// (an event this crate has no vocabulary for) — invoke the slot's
    /// registered callback with it.
    ///
    /// That `None` is **not** how a programmatic echo is suppressed:
    /// an echo never reaches this method, because it re-enters
    /// [`with_runtime`] mid-borrow and is dropped there. See
    /// [`NativeWidget::on_event`]'s doc and `crate::controls`'s module doc.
    ///
    /// **Routing is by slot id alone.** This method asks nothing about which
    /// view or listener produced the event, so an id that *names* a live slot
    /// is delivered whether or not the listener carrying it was ever attached
    /// to that slot's view; Android's `nativeOnEvent` export validates only
    /// that the incoming `jlong` is non-negative before it gets here
    /// (`crate::android`'s `validate_event_slot_id` → `SlotId::try_from`).
    /// A public component's slot adds one gate of its own past this point:
    /// `crate::component`'s bridge delivers only the event families the
    /// component itself attached a listener for
    /// (`crate::component::ComponentCtx::attach_listener`), so a fabricated id
    /// naming a component that attached nothing stops there; one naming a slot
    /// that did attach that family is indistinguishable from the real
    /// listener, for components and the built-in controls alike.
    ///
    /// **Bypasses `RenderRoot::event` entirely** (crate doc): this is a
    /// platform interaction surfacing as a callback, never a frust pointer
    /// event.
    pub(crate) fn on_event(&mut self, slot_id: SlotId, event: NativeEvent) -> EventOutcome {
        let Some(instance) = self.instances.get_mut(slot_id) else {
            return EventOutcome::UnknownSlot;
        };
        let payload = (instance.vtable.on_event)(&mut *instance.state, event);
        // Clone the callback out (and let `instance`'s borrow end here) so
        // invoking it — arbitrary app code — never holds a live borrow of
        // `self.instances`; a callback that re-enters `with_runtime` is
        // caught by its own re-entrancy tolerance either way.
        let callback = payload
            .is_some()
            .then(|| instance.callback.clone())
            .flatten();
        if let (Some(payload), Some(callback)) = (payload, callback) {
            callback(payload);
        }
        EventOutcome::Delivered
    }

    /// Register (or replace) the callback a slot's decoded [`EventPayload`]s
    /// are handed to, invoked on the platform main thread from
    /// [`Self::on_event`] — the seam the app-facing api wraps into a
    /// signal write.
    ///
    /// A slot with **no live instance yet** — the ordinary case, since this
    /// runs during the rebuild that mounts the slot and the create only on
    /// the host's next post-frame poll (module doc's *two-phase identity* 3)
    /// — parks the callback in `pending_callbacks` for [`Self::create`] to
    /// take, rather than dropping it. Returns whether the registration was
    /// accepted: always `true` today (live or deferred, it is kept), with the
    /// bool retained so the three call sites that already ignore it stay
    /// correct if a genuine rejection is ever added.
    pub(crate) fn set_callback(
        &mut self,
        slot_id: SlotId,
        callback: Arc<dyn Fn(EventPayload) + Send + Sync>,
    ) -> bool {
        match self.instances.get_mut(slot_id) {
            Some(instance) => {
                instance.callback = Some(callback);
                // Keeps "pending and live are mutually exclusive" true even
                // if an earlier create failed after its slot was registered:
                // a stale entry would otherwise shadow this newer callback at
                // the next create.
                self.pending_callbacks.remove(&slot_id);
                true
            }
            None => {
                self.pending_callbacks.insert(slot_id, callback);
                true
            }
        }
    }

    /// Remove — without disposing — the instance whose view satisfies
    /// `is_match`, the identity-based lookup a platform dispose export needs
    /// (it is handed the view object, never a slot id).
    ///
    /// Separate from disposal so the caller can resolve identity with the
    /// platform's own comparison (Android's `Env::is_same_object`) *before*
    /// building the [`NativeCtx`] the teardown itself needs — the borrow
    /// split that keeps both halves safe. Pass the result to
    /// [`Instance::dispose`].
    pub(crate) fn take_matching(
        &mut self,
        is_match: impl FnMut(&NativeView) -> bool,
    ) -> Option<(SlotId, Instance)> {
        let mut is_match = is_match;
        self.instances
            .remove_matching(|instance| is_match(&instance.view))
    }

    /// Tear down the instance for `slot_id`, if any. A late or duplicate
    /// dispose reports [`DisposeOutcome::NotFound`] and does nothing (module
    /// doc's *late and duplicate disposal*) — except for one thing it always
    /// does: drop any *pending* registration for the slot too.
    ///
    /// **Not the production reclaim path for [`Self::pending_callbacks`].**
    /// Android's production dispose resolves by view identity
    /// through [`Self::take_matching`] and never calls this method — it is
    /// reached only by the create-rollback branch (a successful create whose
    /// local reference then fails to hand back to Kotlin) and by tests. A
    /// slot disposed the ordinary way can still strand a *later* pending
    /// registration this method never sees;
    /// [`Self::forget_pending_callback`] is what actually bounds
    /// `pending_callbacks` for the process lifetime.
    pub(crate) fn dispose_slot(
        &mut self,
        ctx: &mut NativeCtx<'_, '_>,
        slot_id: SlotId,
    ) -> DisposeOutcome {
        self.pending_callbacks.remove(&slot_id);
        let Some(instance) = self.instances.remove(slot_id) else {
            return DisposeOutcome::NotFound;
        };
        if let Err(e) = instance.dispose(ctx) {
            log::warn!("frust-native-widgets: disposing slot {slot_id}: {e}");
        }
        DisposeOutcome::Disposed
    }

    /// Remove `slot`'s pending callback registration unconditionally,
    /// whatever the table currently holds for it — the Component-teardown
    /// reaper for [`Self::pending_callbacks`], applying the same remedy
    /// `crate::controls::image::retire` uses to the identical leak
    /// shape one table over. Registered via `on_cleanup` in each interactive
    /// builder's `Component::init` (`crate::api::builders`), so it runs
    /// exactly once per mounted Component regardless of how many times (if
    /// any) the native create/dispose pair actually ran in between — the
    /// case [`Self::dispose_slot`]'s own clear cannot see, since Android's
    /// production dispose (`crate::android::dispose_control`) never calls
    /// it.
    ///
    /// Idempotent: a slot with no pending entry (already taken by
    /// [`Self::create`], already cleared, or never registered) is a silent
    /// no-op.
    pub(crate) fn forget_pending_callback(&mut self, slot: SlotId) {
        self.pending_callbacks.remove(&slot);
    }

    /// How many controls are currently live — the leak bar every
    /// create/dispose cycle must return to `0`.
    pub(crate) fn live_count(&self) -> usize {
        self.instances.live_count()
    }
}

impl Default for NativeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

thread_local! {
    /// The main thread's runtime (module doc's *main-thread confinement*).
    /// Seeded with the platform backend's own control kinds on first touch.
    static RUNTIME: RefCell<NativeRuntime> = RefCell::new(seeded_runtime());
}

/// A runtime with every kind this build's platform backend serves.
fn seeded_runtime() -> NativeRuntime {
    #[allow(unused_mut)]
    let mut runtime = NativeRuntime::new();
    #[cfg(target_os = "android")]
    crate::android::register_controls(&mut runtime);
    #[cfg(target_os = "ios")]
    crate::apple::register_controls(&mut runtime);
    #[cfg(target_os = "macos")]
    crate::appkit::register_controls(&mut runtime);
    runtime
}

/// Run `f` against the calling thread's runtime.
///
/// Returns `None` when the runtime is **already borrowed on this thread** —
/// the re-entrancy case the module doc describes (a platform setter firing its
/// own listener synchronously). Dropping that callback is strictly better than
/// panicking near an FFI boundary, and the control-side fix is the
/// set-without-notify idiom.
pub(crate) fn with_runtime<T>(f: impl FnOnce(&mut NativeRuntime) -> T) -> Option<T> {
    RUNTIME
        .try_with(|cell| match cell.try_borrow_mut() {
            Ok(mut runtime) => Some(f(&mut runtime)),
            Err(_) => {
                log::warn!(
                    "frust-native-widgets: re-entrant runtime call ignored (a platform setter \
                     fired its own listener?)"
                );
                None
            }
        })
        .unwrap_or_default()
}

// --- host stand-ins for the platform types ----------------------------------

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
pub(crate) mod host {
    //! Host stand-ins for the two platform-shaped types the runtime threads
    //! through untouched, so the whole dispatch/diff/lifecycle contract above
    //! is exercised by ordinary `cargo test` on a machine with no JNI and no
    //! Objective-C runtime at all.
    //!
    //! All three platform arms swap in a real pair — `crate::android`'s
    //! `Env`-borrowing context plus its global-ref handle, `crate::apple`'s
    //! `MainThreadMarker` context plus its `Retained<UIView>` handle, and
    //! `crate::appkit`'s `MainThreadMarker` context plus its
    //! `Retained<NSView>` handle — so this module is the arm-less hosts'
    //! (Linux/Windows/web) only, and the tests below run there, not on macOS.

    use std::marker::PhantomData;

    /// Stand-in for the platform's scoped call context. Records what a
    /// control did, so a test can assert *that a platform call would have
    /// happened* — which is exactly what the props diff gate is about.
    pub(crate) struct NativeCtx<'local, 'env> {
        calls: &'env mut Vec<String>,
        _frame: PhantomData<&'local ()>,
    }

    impl<'local, 'env> NativeCtx<'local, 'env> {
        /// A context recording into `calls`.
        pub(crate) fn new(calls: &'env mut Vec<String>) -> Self {
            Self {
                calls,
                _frame: PhantomData,
            }
        }
    }

    impl NativeCtx<'_, '_> {
        /// Record one would-be platform call.
        pub(crate) fn record(&mut self, call: impl Into<String>) {
            self.calls.push(call.into());
        }
    }

    /// Stand-in for a retained native view. `identity` plays the role
    /// `Env::is_same_object` plays on Android: what a dispose resolves
    /// against.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct NativeView {
        /// This view's identity, as a dispose would name it.
        pub(crate) identity: u64,
    }
}

// Gated on the host arm, not merely on `test`: these exercise the runtime
// through the [`host`] stand-ins above, which a platform build replaces with
// the real (platform-only) types — macOS included, since it has a real arm.
#[cfg(all(
    test,
    not(any(target_os = "android", target_os = "ios", target_os = "macos"))
))]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::events::EVENT_KIND_CLICK;

    /// A control whose "platform calls" are strings recorded into the host
    /// [`NativeCtx`] — enough to assert the runtime's dispatch, diff gate and
    /// lifecycle without any FFI.
    struct FakeControl;

    #[derive(Clone, Debug, PartialEq)]
    struct FakeProps {
        label: String,
        enabled: bool,
    }

    struct FakeState {
        identity: u64,
        events: Vec<NativeEvent>,
    }

    const FAKE_KIND: &str = "fake";

    /// The sentinel [`FakeProps::label`] that makes [`FakeControl::create`]
    /// fail — the failed-create-variant regression test's opt-in.
    const FAIL_CREATE_LABEL: &str = "FAIL_CREATE";

    impl NativeWidget for FakeControl {
        type Props = FakeProps;
        type State = FakeState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            Ok(FakeProps {
                label: params
                    .string("label")
                    .ok_or_else(|| NativeWidgetError::Params("no `label`".into()))?
                    .into_owned(),
                enabled: params.flag("enabled").unwrap_or(true),
            })
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            // A sentinel label the failed-create-variant test below opts
            // into — no production props ever carry it, and no other test
            // in this module uses this label.
            if props.label == FAIL_CREATE_LABEL {
                ctx.record("create failed".to_string());
                return Err(NativeWidgetError::Platform("fake create failure".into()));
            }
            let identity = params_identity_counter();
            ctx.record(format!("create {} '{}'", identity, props.label));
            Ok((
                NativeView { identity },
                FakeState {
                    identity,
                    events: Vec::new(),
                },
            ))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            ctx.record(format!(
                "update {} '{}' -> '{}'",
                state.identity, old.label, new.label
            ));
            Ok(())
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            state.events.push(event);
            // A stand-in payload — enough for the dispatch/callback tests
            // below, which don't care about decode fidelity (that's
            // `crate::events`'s and each control's own job).
            Some(EventPayload::Click)
        }

        fn dispose(
            ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            ctx.record(format!("dispose {}", state.identity));
            Ok(())
        }
    }

    /// Monotonic stand-in for "the platform handed us a fresh object".
    fn params_identity_counter() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn runtime() -> NativeRuntime {
        let mut runtime = NativeRuntime::new();
        assert!(runtime.register::<FakeControl>(FAKE_KIND));
        runtime
    }

    fn params(slot: SlotId, label: &str, enabled: bool) -> String {
        with_identity(
            FAKE_KIND,
            slot,
            &format!("\"label\":\"{}\",\"enabled\":{enabled}", escape(label)),
        )
    }

    /// Where a [`recorder`] callback appends what it was handed. Its
    /// `Arc` doubles as the leak probe (see [`recorder`]).
    type FiredLog = Arc<Mutex<Vec<EventPayload>>>;

    /// The callback shape [`NativeRuntime::set_callback`] stores.
    type TestCallback = Arc<dyn Fn(EventPayload) + Send + Sync>;

    /// A callback recording every payload it is handed, plus the log it
    /// appends to. The log's `Arc` is also the leak probe: once the runtime
    /// has dropped the closure, its strong count is back to the one reference
    /// the test itself holds.
    fn recorder() -> (FiredLog, TestCallback) {
        let fired: FiredLog = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&fired);
        (
            fired,
            Arc::new(move |payload| sink.lock().unwrap().push(payload)),
        )
    }

    /// The one event every callback test below fires.
    const CLICK: NativeEvent = NativeEvent {
        kind: EVENT_KIND_CLICK,
        detail: 0,
    };

    // --- params / identity ---------------------------------------------

    #[test]
    fn identity_injection_round_trips() {
        let raw = params(7, "Save", true);
        let parsed = Params::new(&raw);
        let (kind, slot) = parsed.identity().expect("identity present");
        assert_eq!(kind, FAKE_KIND);
        assert_eq!(slot, 7);
        assert_eq!(parsed.string("label").unwrap(), "Save");
        assert_eq!(parsed.flag("enabled"), Some(true));
    }

    #[test]
    fn identity_survives_an_escaped_body() {
        let raw = params(3, "say \"hi\"\n", false);
        let parsed = Params::new(&raw);
        assert_eq!(parsed.identity().unwrap(), (Cow::Borrowed(FAKE_KIND), 3));
        assert_eq!(parsed.string("label").unwrap(), "say \"hi\"\n");
        assert_eq!(parsed.flag("enabled"), Some(false));
    }

    #[test]
    fn a_key_inside_a_string_value_is_not_matched() {
        // The naive `find("\"slot\":")` scan would read the id out of the
        // *text* below; the scanner must not.
        let raw = format!(
            "{{\"{CONTROL_KEY}\":\"fake\",\"{SLOT_KEY}\":9,\"label\":\"\\\"{SLOT_KEY}\\\":42\"}}"
        );
        let parsed = Params::new(&raw);
        assert_eq!(parsed.int(SLOT_KEY), Some(9));
        assert_eq!(parsed.string("label").unwrap(), "\"__frustSlot\":42");
    }

    #[test]
    fn nested_values_are_skipped_as_a_unit() {
        let raw = format!(
            "{{\"{CONTROL_KEY}\":\"fake\",\"nested\":{{\"{SLOT_KEY}\":1,\"a\":[1,2]}},\
             \"{SLOT_KEY}\":5,\"size\":12.5}}"
        );
        let parsed = Params::new(&raw);
        assert_eq!(parsed.identity().unwrap().1, 5);
        assert_eq!(parsed.float("size"), Some(12.5));
        assert!(parsed.string("nested").is_none());
    }

    #[test]
    fn params_without_identity_are_an_error() {
        assert!(matches!(
            Params::new("{\"label\":\"x\"}").identity(),
            Err(NativeWidgetError::Params(_))
        ));
        assert!(matches!(
            Params::new(&format!("{{\"{CONTROL_KEY}\":\"fake\"}}")).identity(),
            Err(NativeWidgetError::Params(_))
        ));
        assert!(matches!(
            Params::new("not json at all").identity(),
            Err(NativeWidgetError::Params(_))
        ));
    }

    // --- dispatch ------------------------------------------------------

    #[test]
    fn create_dispatches_by_kind_and_retains_the_instance() {
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);

        let slot = runtime
            .create(&mut ctx, &params(1, "Save", true))
            .expect("create");

        assert_eq!(slot, 1);
        assert_eq!(runtime.live_count(), 1);
        assert_eq!(runtime.instance(1).map(Instance::kind), Some(FAKE_KIND));
        assert_eq!(calls.len(), 1, "exactly one platform create: {calls:?}");
        assert!(calls[0].ends_with("'Save'"));
    }

    #[test]
    fn create_for_an_unregistered_kind_is_an_error() {
        let mut runtime = NativeRuntime::new();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);

        let err = runtime
            .create(&mut ctx, &params(1, "Save", true))
            .expect_err("no kinds registered");

        assert!(matches!(err, NativeWidgetError::UnknownControl(kind) if kind == FAKE_KIND));
        assert_eq!(runtime.live_count(), 0);
        assert!(calls.is_empty(), "nothing crossed the boundary");
    }

    #[test]
    fn create_for_a_live_slot_replaces_and_disposes_the_previous() {
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);

        runtime.create(&mut ctx, &params(4, "first", true)).unwrap();
        runtime
            .create(&mut ctx, &params(4, "second", true))
            .unwrap();

        assert_eq!(runtime.live_count(), 1, "the slot holds one instance");
        // The replacement is created BEFORE the previous is disposed, so a
        // failing create can never destroy a live control.
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert!(calls[1].ends_with("'second'"));
        assert!(calls[2].starts_with("dispose"));
    }

    // --- the props diff gate -------------------------------------------

    #[test]
    fn unchanged_props_never_reach_the_platform() {
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        runtime.create(&mut ctx, &params(2, "Save", true)).unwrap();
        calls.clear();
        let mut ctx = NativeCtx::new(&mut calls);

        // Byte-identical params, and params that differ only in key order /
        // whitespace — both decode to equal props.
        let outcome = runtime
            .update_params(&mut ctx, &params(2, "Save", true))
            .unwrap();
        assert_eq!(outcome, UpdateOutcome::Unchanged);

        let reordered = format!(
            "{{ \"enabled\":true, \"label\":\"Save\", \"{SLOT_KEY}\":2, \"{CONTROL_KEY}\":\"fake\" }}"
        );
        assert_eq!(
            runtime.update_params(&mut ctx, &reordered).unwrap(),
            UpdateOutcome::Unchanged
        );
        assert!(calls.is_empty(), "the diff gate crossed nothing: {calls:?}");
    }

    #[test]
    fn changed_props_apply_once_and_rebase_the_gate() {
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        runtime.create(&mut ctx, &params(2, "Save", true)).unwrap();
        calls.clear();
        let mut ctx = NativeCtx::new(&mut calls);

        assert_eq!(
            runtime
                .update_params(&mut ctx, &params(2, "Saved", true))
                .unwrap(),
            UpdateOutcome::Applied
        );
        // The applied props become the new baseline: repeating them is free.
        assert_eq!(
            runtime
                .update_params(&mut ctx, &params(2, "Saved", true))
                .unwrap(),
            UpdateOutcome::Unchanged
        );

        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(calls[0].contains("'Save' -> 'Saved'"));
    }

    #[test]
    fn update_for_an_unknown_slot_is_tolerated() {
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);

        assert_eq!(
            runtime
                .update_params(&mut ctx, &params(99, "ghost", true))
                .unwrap(),
            UpdateOutcome::UnknownSlot
        );
        assert!(calls.is_empty());
    }

    #[test]
    fn update_naming_a_different_kind_is_an_error() {
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        runtime.create(&mut ctx, &params(2, "Save", true)).unwrap();

        let other = with_identity("other", 2, "\"label\":\"x\"");
        assert!(matches!(
            runtime.update_params(&mut ctx, &other),
            Err(NativeWidgetError::Params(_))
        ));
    }

    // --- lifecycle ------------------------------------------------------

    #[test]
    fn create_dispose_cycles_return_to_zero_live_instances() {
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);

        for slot in 0..50u64 {
            runtime.create(&mut ctx, &params(slot, "x", true)).unwrap();
            assert_eq!(
                runtime.dispose_slot(&mut ctx, slot),
                DisposeOutcome::Disposed
            );
        }
        assert_eq!(runtime.live_count(), 0);
    }

    #[test]
    fn duplicate_and_unknown_disposes_are_silent_no_ops() {
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        runtime.create(&mut ctx, &params(1, "x", true)).unwrap();

        assert_eq!(runtime.dispose_slot(&mut ctx, 1), DisposeOutcome::Disposed);
        assert_eq!(runtime.dispose_slot(&mut ctx, 1), DisposeOutcome::NotFound);
        assert_eq!(
            runtime.dispose_slot(&mut ctx, 404),
            DisposeOutcome::NotFound
        );
        assert_eq!(runtime.live_count(), 0);
    }

    #[test]
    fn a_late_dispose_for_a_replaced_view_never_touches_the_live_one() {
        // The idle-deferred-dispose case: the Dispose command for the first
        // view arrives only after a replacement Create already re-used the
        // slot id. It names the OLD view object, which identity resolution
        // must simply not find.
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);

        runtime.create(&mut ctx, &params(8, "first", true)).unwrap();
        let stale = runtime.instance(8).unwrap().view().identity;
        runtime
            .create(&mut ctx, &params(8, "second", true))
            .unwrap();
        let live = runtime.instance(8).unwrap().view().identity;
        assert_ne!(stale, live);

        assert!(
            runtime
                .take_matching(|view| view.identity == stale)
                .is_none(),
            "a dispose naming the replaced view finds nothing"
        );
        assert_eq!(runtime.live_count(), 1);
        assert_eq!(runtime.instance(8).unwrap().view().identity, live);

        // The correctly-targeted dispose still works.
        let (slot, instance) = runtime
            .take_matching(|view| view.identity == live)
            .expect("live view matched");
        assert_eq!(slot, 8);
        instance.dispose(&mut ctx).unwrap();
        assert_eq!(runtime.live_count(), 0);
    }

    // --- events ---------------------------------------------------------

    #[test]
    fn events_route_to_their_slot_and_unknown_slots_are_tolerated() {
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        runtime.create(&mut ctx, &params(1, "a", true)).unwrap();
        runtime.create(&mut ctx, &params(2, "b", true)).unwrap();

        let click = NativeEvent {
            kind: EVENT_KIND_CLICK,
            detail: 0,
        };
        assert_eq!(runtime.on_event(2, click), EventOutcome::Delivered);
        assert_eq!(runtime.on_event(77, click), EventOutcome::UnknownSlot);

        let state_of = |runtime: &NativeRuntime, slot: SlotId| {
            runtime
                .instance(slot)
                .unwrap()
                .state
                .downcast_ref::<FakeState>()
                .unwrap()
                .events
                .clone()
        };
        assert!(state_of(&runtime, 1).is_empty(), "no cross-talk");
        assert_eq!(state_of(&runtime, 2), vec![click]);
    }

    // --- callbacks --------------------------------------------------------

    #[test]
    fn a_registered_callback_fires_only_for_its_own_slot() {
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        runtime.create(&mut ctx, &params(1, "a", true)).unwrap();
        runtime.create(&mut ctx, &params(2, "b", true)).unwrap();

        let fired: Arc<Mutex<Vec<EventPayload>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&fired);
        assert!(runtime.set_callback(
            2,
            Arc::new(move |payload| recorder.lock().unwrap().push(payload))
        ));
        // Slot 1 never gets a callback registered — its event must not
        // panic and must not show up in `fired`.

        let click = NativeEvent {
            kind: EVENT_KIND_CLICK,
            detail: 0,
        };
        assert_eq!(runtime.on_event(1, click), EventOutcome::Delivered);
        assert_eq!(runtime.on_event(2, click), EventOutcome::Delivered);

        assert_eq!(*fired.lock().unwrap(), vec![EventPayload::Click]);
    }

    #[test]
    fn a_callback_registered_before_create_is_born_with_the_instance() {
        // The production order: the api layer registers during the
        // rebuild that mounts the slot; the host's post-frame poll creates
        // the native view only afterwards. Before the pending table this
        // registration was dropped on the floor — permanently, since nothing
        // schedules the rebuild that would retry it.
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        let (fired, callback) = recorder();

        assert!(runtime.set_callback(5, callback));
        assert_eq!(runtime.pending_callbacks.len(), 1, "deferred, not dropped");
        assert_eq!(runtime.live_count(), 0);

        runtime.create(&mut ctx, &params(5, "Save", true)).unwrap();
        assert!(
            runtime.pending_callbacks.is_empty(),
            "the create took the pending registration"
        );

        assert_eq!(runtime.on_event(5, CLICK), EventOutcome::Delivered);
        assert_eq!(*fired.lock().unwrap(), vec![EventPayload::Click]);
    }

    #[test]
    fn a_callback_registered_after_create_still_attaches() {
        // The other order — a registration reaching an already-live slot goes
        // straight onto the instance and leaves nothing pending.
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        let (fired, callback) = recorder();

        runtime.create(&mut ctx, &params(6, "Save", true)).unwrap();
        assert!(runtime.set_callback(6, callback));
        assert!(runtime.pending_callbacks.is_empty());

        assert_eq!(runtime.on_event(6, CLICK), EventOutcome::Delivered);
        assert_eq!(*fired.lock().unwrap(), vec![EventPayload::Click]);
    }

    #[test]
    fn a_replay_create_inherits_the_replaced_instance_callback() {
        // The regression pin for a later amendment: a surface-recreate replay
        // re-creates the slot with NO intervening registration (no frust
        // rebuild ran), so the only surviving copy of the callback is the
        // instance being replaced.
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        let (fired, callback) = recorder();

        runtime.create(&mut ctx, &params(7, "first", true)).unwrap();
        assert!(runtime.set_callback(7, callback));

        runtime
            .create(&mut ctx, &params(7, "second", true))
            .unwrap();
        assert_eq!(runtime.live_count(), 1, "the replay replaced in place");

        assert_eq!(runtime.on_event(7, CLICK), EventOutcome::Delivered);
        assert_eq!(*fired.lock().unwrap(), vec![EventPayload::Click]);
    }

    #[test]
    fn a_pending_registration_outranks_the_replaced_instance_callback() {
        // `create`'s documented order: pending first, previous second. A live
        // instance clears its own pending entry, so the two only coexist by
        // construction — seeded directly here to pin the order.
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        let (stale, stale_callback) = recorder();
        let (fresh, fresh_callback) = recorder();

        runtime.create(&mut ctx, &params(8, "first", true)).unwrap();
        assert!(runtime.set_callback(8, stale_callback));
        runtime.pending_callbacks.insert(8, fresh_callback);

        runtime
            .create(&mut ctx, &params(8, "second", true))
            .unwrap();

        assert_eq!(runtime.on_event(8, CLICK), EventOutcome::Delivered);
        assert_eq!(*fresh.lock().unwrap(), vec![EventPayload::Click]);
        assert!(stale.lock().unwrap().is_empty(), "the older callback lost");
    }

    #[test]
    fn disposing_a_slot_that_never_created_drops_its_pending_callback() {
        // A slot mounted and unmounted between two host polls: its create
        // never runs, so only the dispose can release the closure.
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        let (fired, callback) = recorder();

        assert!(runtime.set_callback(9, callback));
        assert_eq!(runtime.pending_callbacks.len(), 1);
        assert_eq!(
            Arc::strong_count(&fired),
            2,
            "the runtime holds the closure"
        );

        assert_eq!(runtime.dispose_slot(&mut ctx, 9), DisposeOutcome::NotFound);
        assert!(
            runtime.pending_callbacks.is_empty(),
            "no closure stranded for the process lifetime"
        );
        assert_eq!(Arc::strong_count(&fired), 1, "the closure was dropped");

        // And a later create re-using the slot id starts callback-free.
        runtime.create(&mut ctx, &params(9, "x", true)).unwrap();
        assert_eq!(runtime.on_event(9, CLICK), EventOutcome::Delivered);
        assert!(fired.lock().unwrap().is_empty());
    }

    // --- `forget_pending_callback` is the actual bound -----------------------
    //
    // `dispose_slot`'s own clear (the test above) is a secondary safety net
    // for its own callers — it is not Android's production dispose path
    // (`crate::android::dispose_control`, which resolves by view identity
    // via `take_matching` and never calls `dispose_slot` at all). The two
    // tests below are the pin: a happy-path test would not have caught
    // either shape of the leak.

    #[test]
    fn the_culled_dispose_then_republish_leak_is_reaped_on_component_teardown() {
        // Mirrors `image.rs`'s identically-shaped regression test.
        // Steps 1-5 reproduce the original defect trace, using `take_matching`
        // (not `dispose_slot`) for the culled dispose — the real production path.
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);

        // Step 1: mount — create takes the pending entry, the instance is
        // born with the callback.
        let (first_fired, first_callback) = recorder();
        assert!(runtime.set_callback(20, first_callback));
        runtime.create(&mut ctx, &params(20, "x", true)).unwrap();
        assert!(
            runtime.pending_callbacks.is_empty(),
            "the create took the pending registration"
        );
        drop(first_fired);

        // Step 2: culled while off-screen. The differ's missing-streak
        // Dispose resolves by identity via `take_matching` — exactly what
        // `crate::android::dispose_control` does — and never touches
        // `pending_callbacks`. The widget stays mounted (a culled slot never
        // runs `View::teardown`).
        let identity = runtime.instance(20).unwrap().view().identity;
        let (slot, instance) = runtime
            .take_matching(|view| view.identity == identity)
            .expect("the culled dispose finds the live view by identity");
        assert_eq!(slot, 20);
        instance.dispose(&mut ctx).unwrap();
        assert_eq!(runtime.live_count(), 0);

        // Step 3: still mounted, the next rebuild re-registers — no live
        // instance exists, so this parks a fresh pending entry.
        let (fresh, fresh_callback) = recorder();
        assert!(runtime.set_callback(20, fresh_callback));
        assert_eq!(runtime.pending_callbacks.len(), 1);
        assert_eq!(
            Arc::strong_count(&fresh),
            2,
            "the runtime holds the closure"
        );

        // Steps 4/5: navigate away — `View::teardown` disposes the
        // Component's owner, running the `on_cleanup` closure every
        // interactive builder registers in `init` (`api::builders`), which
        // calls `forget_pending_callback` unconditionally.
        runtime.forget_pending_callback(20);
        assert!(
            runtime.pending_callbacks.is_empty(),
            "without forget_pending_callback the fresh entry would strand \
             its closure for the process lifetime — a defect that once shipped"
        );
        assert_eq!(
            Arc::strong_count(&fresh),
            1,
            "the closure was actually dropped, not just unreachable"
        );

        // And a later create re-using the slot id starts callback-free.
        runtime.create(&mut ctx, &params(20, "y", true)).unwrap();
        assert_eq!(runtime.on_event(20, CLICK), EventOutcome::Delivered);
        assert!(fresh.lock().unwrap().is_empty(), "reaped, never re-fires");
    }

    #[test]
    fn a_failed_create_leaves_the_entry_pending_and_teardown_still_reaps_it() {
        // The narrower permanent variant the task calls out: a create that
        // FAILS leaves the registration pending forever (Kotlin marks the
        // slot dead, so no create ever retries it) — only Component
        // teardown's `forget_pending_callback` can reclaim it.
        let mut runtime = runtime();
        let mut calls = Vec::new();
        let mut ctx = NativeCtx::new(&mut calls);
        let (fired, callback) = recorder();

        assert!(runtime.set_callback(30, callback));
        assert_eq!(runtime.pending_callbacks.len(), 1);

        let err = runtime
            .create(&mut ctx, &params(30, FAIL_CREATE_LABEL, true))
            .expect_err("the fake control's create fails for this sentinel label");
        assert!(matches!(err, NativeWidgetError::Platform(_)));
        assert_eq!(runtime.live_count(), 0, "no instance was ever retained");
        assert_eq!(
            runtime.pending_callbacks.len(),
            1,
            "a failed create must not touch the pending entry — nothing \
             schedules a retry, so dropping it here would strand it \
             instead of just leaving it pending"
        );
        assert_eq!(Arc::strong_count(&fired), 2, "the runtime still holds it");

        runtime.forget_pending_callback(30);
        assert!(
            runtime.pending_callbacks.is_empty(),
            "Component teardown still reaps a pending entry whose create \
             never once succeeded"
        );
        assert_eq!(Arc::strong_count(&fired), 1, "the closure was dropped");

        // Idempotent: a second call is a silent no-op.
        runtime.forget_pending_callback(30);
        assert!(runtime.pending_callbacks.is_empty());
    }

    #[test]
    fn registering_the_same_kind_twice_reports_it() {
        let mut runtime = runtime();
        assert!(!runtime.register::<FakeControl>(FAKE_KIND));
        assert!(runtime.is_registered(FAKE_KIND));
        assert!(!runtime.is_registered("nope"));
    }

    #[test]
    fn the_thread_local_runtime_is_reentrancy_tolerant() {
        let outer = with_runtime(|_| {
            // A platform setter firing its own listener re-enters here.
            let inner = with_runtime(|_| "should not run");
            assert!(
                inner.is_none(),
                "a re-entrant call is dropped, not panicked"
            );
            "outer ran"
        });
        assert_eq!(outer, Some("outer ran"));
    }
}
