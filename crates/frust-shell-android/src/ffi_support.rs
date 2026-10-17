//! Android-specific FFI helpers, pure and host-testable.
//!
//! The platform-agnostic plumbing (`guard`, `sanitize_scale`, `logical_size`,
//! the `AppTree` erasure) lives in `frust-shell-common` and is imported at
//! its call sites. What remains here is the JNI-boundary data handling:
//!
//! - handle liveness: the sentinel check for the `jlong` the JVM passes back
//!   into every native call;
//! - touch: action-code mapping and the pointer-id to slot map;
//! - IME: edit-command codes, index normalisation and the state JSON;
//! - the platform-view wire: the command JSON the embedding consumes and its
//!   generation acknowledgement check;
//! - pipeline-cache IO: the on-disk path, the diff check and atomic
//!   read/write;
//! - pre-init helpers: the GPU pre-init join, the resolved-translucency
//!   handoff and the base clear colour.
//!
//! The overlay switch, table and ledger live in [`crate::overlay_state`].
//!
//! This carries no `jni`/`ndk`/GPU dependency so it compiles and is unit-tested
//! on the host (`cargo test --workspace`), even though the code that calls it
//! ([`crate::jni_glue`]) is `#[cfg(target_os = "android")]`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Whether an opaque handle from the JVM points at a live native side.
///
/// Kotlin initialises `handle` to `0` and every native call is guarded on it
/// (see `FrustSurfaceView`), so `0` means "no native side yet / already
/// destroyed" and must be treated as a no-op rather than dereferenced.
#[inline]
pub(crate) fn handle_is_live(handle: i64) -> bool {
    handle != 0
}

/// A pointer gesture phase, decoupled from `frust_core::PointerPhase` so this
/// module stays host-testable (the core crate is Android-gated — see the crate's
/// `Cargo.toml`). [`crate::app`] maps this onto the core phase at the one
/// Android-only call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchPhase {
    /// A finger touched down.
    Down,
    /// A finger moved while down.
    Move,
    /// A finger lifted.
    Up,
    /// The gesture was cancelled by the platform.
    Cancel,
}

/// Map the action code the Kotlin `FrustSurfaceView.onTouchEvent` sends into a
/// [`TouchPhase`].
///
/// The Kotlin side normalises `MotionEvent.actionMasked` into a fixed ABI —
/// `ACTION_DOWN`/`ACTION_POINTER_DOWN` → `0`, `ACTION_MOVE` → `1`,
/// `ACTION_UP`/`ACTION_POINTER_UP` → `2`, `ACTION_CANCEL` → `3` — so this side
/// never sees a raw Android constant. Any unrecognised code (a future action we
/// don't map, or a corrupt value) is treated as [`TouchPhase::Cancel`]: the safe
/// default, since it releases any in-flight capture rather than stranding a
/// gesture as perpetually "down".
#[inline]
pub(crate) fn touch_phase_from_action(action: i32) -> TouchPhase {
    match action {
        0 => TouchPhase::Down,
        1 => TouchPhase::Move,
        2 => TouchPhase::Up,
        // 3 is the explicit `ACTION_CANCEL`; everything else falls back to it.
        _ => TouchPhase::Cancel,
    }
}

/// The largest number of simultaneous touch contacts one gesture tracks.
///
/// Android's `MotionEvent` publishes no hard contact-count limit, but every
/// shipped touchscreen driver reports at most this many simultaneous pointers
/// in practice. **Community-approximate**: chosen generously (well above any
/// observed device) rather than measured from a published spec, so
/// [`TouchSlotMap::assign`] never has to refuse a contact on real hardware.
const MAX_TOUCH_SLOTS: usize = 10;

/// Maps Android's own `MotionEvent.getPointerId` values — stable for a
/// contact's lifetime but platform-chosen and not small/contiguous — onto the
/// small, gesture-local slot numbers [`frust_core::event::PointerId::touch`]
/// takes (`0` for the gesture's first contact, `1` for the second, …).
///
/// A slot is assigned on a contact's `Down` and freed on its `Up`/`Cancel`;
/// freeing one slot never renumbers another that is still live — a second
/// finger holding slot `1` stays at `1` after the first (slot `0`) lifts, so
/// the primary-sequence latch (`frust_core::event::PointerId`'s multi-contact
/// contract: a slot-`0` contact is the one a capture can latch onto) never
/// retargets mid-gesture. A new contact takes the **lowest currently-free**
/// slot, so the map "resets" to handing out `0` again once every contact has
/// lifted — there is no separate reset operation, it falls out of the search.
///
/// Kept in `frust-shell-android`'s host-testable `ffi_support` module (not
/// `app::input`, which is `#[cfg(target_os = "android")]` via its `frust-core`
/// dependency) so the slot-assignment rule itself is exercised by `cargo test
/// -p frust-shell-android` on every host, mirroring [`touch_phase_from_action`]
/// and [`appearance_from_dark`] above.
#[derive(Debug)]
pub(crate) struct TouchSlotMap {
    /// Index = frust slot; value = the Android `pointerId` currently occupying
    /// it, or `None` when the slot is free.
    slots: [Option<i32>; MAX_TOUCH_SLOTS],
}

impl TouchSlotMap {
    /// A fresh map with every slot free (no gesture in progress).
    pub(crate) fn new() -> Self {
        Self {
            slots: [None; MAX_TOUCH_SLOTS],
        }
    }

    /// The slot `android_pointer_id` currently occupies, if its `Down` has
    /// already been assigned one.
    pub(crate) fn slot_for(&self, android_pointer_id: i32) -> Option<u32> {
        self.slots
            .iter()
            .position(|slot| *slot == Some(android_pointer_id))
            .map(|i| i as u32)
    }

    /// Assign `android_pointer_id` the lowest currently-free slot (its
    /// `Down`). A duplicate assignment for an id already tracked returns its
    /// existing slot rather than taking a second one. When every slot is
    /// already occupied (see [`MAX_TOUCH_SLOTS`]) the last slot is reused
    /// rather than refusing the contact — a saturating fallback, not a panic.
    pub(crate) fn assign(&mut self, android_pointer_id: i32) -> u32 {
        if let Some(slot) = self.slot_for(android_pointer_id) {
            return slot;
        }
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(android_pointer_id);
                return i as u32;
            }
        }
        let last = MAX_TOUCH_SLOTS - 1;
        self.slots[last] = Some(android_pointer_id);
        last as u32
    }

    /// Free the slot `android_pointer_id` occupies (its `Up`/`Cancel`), so a
    /// later `Down` may reuse it. A release for an id this map never assigned
    /// is a harmless no-op.
    pub(crate) fn release(&mut self, android_pointer_id: i32) {
        if let Some(slot) = self.slot_for(android_pointer_id) {
            self.slots[slot as usize] = None;
        }
    }
}

/// Convert Kotlin's `Choreographer.FrameCallback` `frameTimeNanos` (a JNI
/// `jlong`, signed 64-bit) into the unsigned nanosecond count
/// [`frust_core::FrameTime::from_nanos`] takes.
///
/// `frameTimeNanos` is documented as monotonic and non-negative in practice,
/// but the JNI boundary is untrusted input: a negative value clamps to `0`
/// rather than silently wrapping into a huge `u64` via `as` (which would
/// corrupt every later `FrameTime::saturating_sub` in the same episode).
#[inline]
pub(crate) fn frame_time_nanos_from_jlong(frame_time_nanos: i64) -> u64 {
    frame_time_nanos.max(0) as u64
}

/// A light/dark appearance flag, decoupled from `frust_theme::Brightness` so
/// this module stays host-testable (that crate is Android-gated — see the
/// crate's `Cargo.toml`). [`crate::app`] maps this onto `Brightness` at the one
/// Android-only call site ([`crate::app::AndroidAppHandle::set_appearance`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Appearance {
    /// The platform reports a light appearance preference.
    Light,
    /// The platform reports a dark appearance preference.
    Dark,
}

/// Map the `nativeSetAppearance` JNI boolean (Kotlin's
/// `(resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
/// Configuration.UI_MODE_NIGHT_YES`) into an [`Appearance`] — the pure core of
/// the appearance state-machine transition.
#[inline]
pub(crate) fn appearance_from_dark(dark: bool) -> Appearance {
    if dark {
        Appearance::Dark
    } else {
        Appearance::Light
    }
}

/// The back-press consume decision: given the
/// framework's current [`handles_back`](frust_reactive::handles_back) answer,
/// whether the native `nativeOnBackPress` callback should consume this press
/// (routing it into the app via `push_back_press`) and, equivalently, what
/// `jboolean` it returns to the Kotlin `OnBackPressedDispatcher`.
///
/// The mapping is intentionally identity — the framework consumes exactly when
/// it has advertised (via `setFrameworkHandlesBack`) that it will — but it is
/// named and host-tested here rather than inlined at the JNI boundary so the
/// contract has a single documented, testable decision point (the same reason
/// [`appearance_from_dark`]/[`touch_phase_from_action`] live here). `true` =
/// consume + return `JNI_TRUE` (framework pops on next rebuild); `false` = let
/// the default dispatcher finish the activity.
#[inline]
pub(crate) fn should_consume_back_press(handles_back: bool) -> bool {
    handles_back
}

/// Normalise a raw JNI selection/composing index quintuple into the canonical
/// [`frust_core::event::EditingState`] field form (the mobile IME seam).
///
/// The indices arrive from Kotlin as **UTF-16 code units** (the Java-native unit
/// for `Editable`/`Selection`) and are passed through the `AppTree`/shell seam
/// *unchanged* — the seam itself is UTF-16 (see `EditingState`'s index-boundary
/// rule); the focused widget / `frust-text` converts them to Rust
/// byte offsets at its own boundary. This helper only canonicalises the "none"
/// sentinel: any out-of-domain negative (`< -1`, e.g. a corrupt platform value)
/// is clamped to the single `-1` marker, so a bad value can never be misread as a
/// large positive index. A valid `0` (caret at the start) is preserved.
///
/// Returned as `(selection_base, selection_extent, composing_base,
/// composing_extent)`, matching the `EditingState` field order.
#[inline]
pub(crate) fn normalize_ime_indices(
    sel_base: i32,
    sel_ext: i32,
    comp_base: i32,
    comp_ext: i32,
) -> (i32, i32, i32, i32) {
    #[inline]
    fn norm(i: i32) -> i32 {
        if i < 0 { -1 } else { i }
    }
    (
        norm(sel_base),
        norm(sel_ext),
        norm(comp_base),
        norm(comp_ext),
    )
}

/// A semantic clipboard / selection verb crossing the `nativeEditCommand` JNI
/// boundary, decoupled from `frust_core::event::EditCommand` so this module
/// stays host-testable (the core crate is Android-gated — see the crate's
/// `Cargo.toml`). [`crate::jni_glue`] maps this onto the core command at the
/// one Android-only call site, exactly as [`TouchPhase`] and [`Appearance`]
/// are mapped.
///
/// Carries no payload on purpose: only a paste has one, and it crosses as its
/// own `jstring` argument beside the code rather than being modelled here — so
/// this stays a `Copy` discriminant and the pasted text never needs a second
/// owner on the way through the mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditCommandWire {
    /// Copy the selection to the host clipboard (wire code `0`).
    Copy,
    /// Copy the selection and delete it (wire code `1`).
    Cut,
    /// Replace the selection with the text accompanying the command (wire code `2`).
    Paste,
    /// Select the focused field's whole content (wire code `3`).
    SelectAll,
}

/// Map the command code the Kotlin `FrustSurfaceView.nativeEditCommand` sends
/// into an [`EditCommandWire`].
///
/// The numeric ABI is fixed and shared with the Kotlin side — DO NOT renumber
/// without changing both sides:
///   `0` = copy, `1` = cut, `2` = paste (text in the accompanying `jstring`),
///   `3` = select all.
///
/// An unrecognised code answers `None` — *ignored*, never defaulted. That is
/// the deliberate opposite of [`touch_phase_from_action`]'s cancel fallback: a
/// stray touch code is safest resolved to "release the gesture", but every verb
/// in this vocabulary edits a document (a paste replaces the selection, a cut
/// deletes it), so picking one for a code this side could not decode would
/// mutate the user's text on an undecodable value.
#[inline]
pub(crate) fn edit_command_from_code(cmd: i32) -> Option<EditCommandWire> {
    match cmd {
        0 => Some(EditCommandWire::Copy),
        1 => Some(EditCommandWire::Cut),
        2 => Some(EditCommandWire::Paste),
        3 => Some(EditCommandWire::SelectAll),
        _ => None,
    }
}

/// A plain, `jni`/`frust-core`-free view of the IME surface, ready to be
/// serialised into the `nativeImeState` JSON the Kotlin side parses.
///
/// Kept free of any `jni`/`ndk`/`kurbo` type so it (and [`build_ime_state_json`])
/// compiles and is unit-tested on the host. [`crate::jni_glue`] maps a
/// [`frust_core::event::ImeState`] onto this at the one Android-only call site.
///
/// Indices are UTF-16 code units (see [`normalize_ime_indices`]); `caret` is the
/// logical-pixel caret rectangle `(x, y, width, height)` for future IME candidate
/// placement, absent when no caret was published.
///
/// `content_type` is a **stable, explicitly-encoded wire string** — one of
/// `"normal"` / `"password"` / `"noSuggestions"` — never a `Debug` rendering of
/// `frust_core::event::ImeContentType` (that type's `Debug` impl exists on
/// `ImeState`, not here, but the discipline is the same one that motivated it:
/// a secret field's classification must never ride on a format that could
/// silently change shape). [`crate::jni_glue`]'s `content_type_wire` maps the
/// enum to this string at the one Android-only call site — the same split
/// [`ImeJsonState`] itself exists for (`frust_core` is Android-gated in this
/// crate's `Cargo.toml`, so this host-testable type cannot name the enum
/// directly). This is the same wire shape the iOS bridge's `ime_state_json`
/// emits under `"contentType"`.
///
/// `suppress_soft_keyboard` mirrors [`frust_core::event::ImeState::suppress_soft_keyboard`]
/// unchanged (a plain `bool`, no wire-string translation needed) into the
/// `"suppressSoftKeyboard"` JSON field [`build_ime_state_json`] emits — the
/// hint `FrustSurfaceView`'s `pollImeAfterDispatch` reads to keep a read-only
/// field's `InputConnection` alive without raising the on-screen keyboard.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ImeJsonState {
    pub active: bool,
    pub text: String,
    pub sel_base: i32,
    pub sel_ext: i32,
    pub comp_base: i32,
    pub comp_ext: i32,
    pub caret: Option<(f32, f32, f32, f32)>,
    pub content_type: &'static str,
    pub suppress_soft_keyboard: bool,
}

impl Default for ImeJsonState {
    /// The "no focused editable" surface: inactive, empty, no selection/composing,
    /// `"normal"` content type (no hint), keyboard not suppressed (nothing to
    /// suppress with no field focused).
    fn default() -> Self {
        Self {
            active: false,
            text: String::new(),
            sel_base: -1,
            sel_ext: -1,
            comp_base: -1,
            comp_ext: -1,
            caret: None,
            content_type: "normal",
            suppress_soft_keyboard: false,
        }
    }
}

/// Escape a Rust string for embedding inside a JSON double-quoted string literal.
///
/// Hand-rolled (the shell carries no `serde`/`json` dependency): escapes the two
/// structural characters (`"`, `\`), the named control escapes
/// (`\b`/`\f`/`\n`/`\r`/`\t`), and any other C0 control (`< 0x20`) as `\u00XX`.
/// Printable non-ASCII (any `char >= 0x20`) passes through verbatim as UTF-8,
/// which a JSON string permits.
pub(crate) fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Format an `f32` as a JSON-legal number, substituting a finite `0` for any
/// non-finite value (`NaN`/`±inf` have no JSON representation and would produce
/// an unparseable token).
fn json_number(v: f32) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "0".to_string()
    }
}

/// Serialise an [`ImeJsonState`] into the exact JSON wire form the Kotlin
/// `FrustSurfaceView` parses from `nativeImeState`.
///
/// Shape (indices UTF-16, caret logical px; absent caret ⇒ `null` components;
/// `suppressSoftKeyboard` a real JSON boolean, not a string):
/// `{"active":bool,"text":"...","selBase":n,"selExt":n,"compBase":n,`
/// `"compExt":n,"caretX":f,"caretY":f,"caretW":f,"caretH":f,"contentType":"...",`
/// `"suppressSoftKeyboard":bool}`.
pub(crate) fn build_ime_state_json(state: &ImeJsonState) -> String {
    let (caret_x, caret_y, caret_w, caret_h) = match state.caret {
        Some((x, y, w, h)) => (
            json_number(x),
            json_number(y),
            json_number(w),
            json_number(h),
        ),
        None => (
            "null".to_string(),
            "null".to_string(),
            "null".to_string(),
            "null".to_string(),
        ),
    };
    format!(
        "{{\"active\":{},\"text\":\"{}\",\"selBase\":{},\"selExt\":{},\"compBase\":{},\"compExt\":{},\"caretX\":{},\"caretY\":{},\"caretW\":{},\"caretH\":{},\"contentType\":\"{}\",\"suppressSoftKeyboard\":{}}}",
        state.active,
        json_escape(&state.text),
        state.sel_base,
        state.sel_ext,
        state.comp_base,
        state.comp_ext,
        caret_x,
        caret_y,
        caret_w,
        caret_h,
        json_escape(state.content_type),
        state.suppress_soft_keyboard,
    )
}

// ---------------------------------------------------------------------
// Platform-view command JSON: the host-testable
// core of `nativePlatformViewCommands`.
//
// `frust-shell-common` (which owns `ViewCommand`/`PlatformViewState`) is only
// a dependency of this crate under `cfg(target_os = "android")` (see this
// crate's `Cargo.toml`), so it cannot appear in a host-compiled signature —
// the same constraint `ImeJsonState` above exists to work around.
// `PlatformViewCommandJson` is the plain, dependency-free mirror of
// `frust_shell_common::ViewCommand`; `crate::jni_glue`'s
// `platform_view_commands_to_json` maps the real differ output onto this at
// the one Android-only call site, exactly like `ime_state_to_json` above.
// ---------------------------------------------------------------------

/// A plain, `frust-shell-common`/JNI-free view of one platform-view command,
/// ready to be serialised into the `nativePlatformViewCommands` JSON both
/// mobile shells serve, byte-identically (a frozen wire contract).
///
/// Rect/clip tuples are `(x, y, width, height)` **physical px** — already
/// scaled at the mapping site ([`crate::jni_glue`]'s
/// `platform_view_commands_to_json`), mirroring [`ImeJsonState`]'s caret rect
/// (the physical-at-FFI/logical-inside rule, applied outbound).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PlatformViewCommandJson {
    /// Mirrors `frust_shell_common::ViewCommand::Create`.
    Create {
        slot_id: u64,
        view_type: String,
        params_json: String,
        interactive: bool,
    },
    /// Mirrors `frust_shell_common::ViewCommand::Update`.
    Update {
        slot_id: u64,
        rect: (f32, f32, f32, f32),
        clip: Option<(f32, f32, f32, f32)>,
        visible: bool,
        shields: Vec<(f32, f32, f32, f32)>,
    },
    /// Mirrors `frust_shell_common::ViewCommand::UpdateParams`.
    UpdateParams { slot_id: u64, params_json: String },
    /// Mirrors `frust_shell_common::ViewCommand::Dispose`.
    Dispose { slot_id: u64 },
    /// Mirrors `frust_shell_common::ViewCommand::Overlay`: create or re-place
    /// overlay `overlay_id` directly above slot `above_slot_id`, covering
    /// `rect`, drawing the frame's commands `range_start..range_end`.
    Overlay {
        overlay_id: u64,
        above_slot_id: u64,
        rect: (f32, f32, f32, f32),
        range_start: u64,
        range_end: u64,
    },
    /// Mirrors `frust_shell_common::ViewCommand::DisposeOverlay`.
    DisposeOverlay { overlay_id: u64 },
}

/// Whether `nativePlatformViewCommands`'s no-change fast path applies:
/// nothing has been produced since the generation Kotlin last acknowledged
/// (round-tripped back as `ack_generation`) — the differ's own
/// `acknowledge`-then-peek contract (`frust_shell_common::platform_view`'s
/// module docs) reduces to exactly this equality check once the ack has been
/// applied. A `true` here means `nativePlatformViewCommands` returns a null
/// `jstring` instead of a JSON payload, mirroring `nativeSystemUiState`'s
/// cheapness bar (a no-change poll costs one JNI call, no allocation).
#[inline]
pub(crate) fn platform_view_commands_up_to_date(generation: u64, ack_generation: u64) -> bool {
    generation == ack_generation
}

/// Format one `(x, y, width, height)` rect tuple as the JSON wire array
/// `[x,y,w,h]`, sanitising any non-finite component to `0` (mirrors
/// [`json_number`]'s NaN/inf guard — a rect crossing this boundary must
/// always be parseable JSON).
fn platform_view_rect_json((x, y, w, h): (f32, f32, f32, f32)) -> String {
    format!(
        "[{},{},{},{}]",
        json_number(x),
        json_number(y),
        json_number(w),
        json_number(h),
    )
}

/// Serialise one [`PlatformViewCommandJson`] into its JSON object — see
/// [`build_platform_view_commands_json`] for the full wire shape.
fn platform_view_command_json(cmd: &PlatformViewCommandJson) -> String {
    match cmd {
        PlatformViewCommandJson::Create {
            slot_id,
            view_type,
            params_json,
            interactive,
        } => format!(
            "{{\"op\":\"create\",\"slot\":{slot_id},\"viewType\":\"{}\",\"params\":\"{}\",\"interactive\":{interactive}}}",
            json_escape(view_type),
            json_escape(params_json),
        ),
        PlatformViewCommandJson::Update {
            slot_id,
            rect,
            clip,
            visible,
            shields,
        } => {
            let shields_json = shields
                .iter()
                .map(|s| platform_view_rect_json(*s))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "{{\"op\":\"update\",\"slot\":{slot_id},\"rect\":{},\"clip\":{},\"visible\":{visible},\"shields\":[{shields_json}]}}",
                platform_view_rect_json(*rect),
                clip.map_or_else(|| "null".to_string(), platform_view_rect_json),
            )
        }
        PlatformViewCommandJson::UpdateParams {
            slot_id,
            params_json,
        } => format!(
            "{{\"op\":\"updateParams\",\"slot\":{slot_id},\"params\":\"{}\"}}",
            json_escape(params_json),
        ),
        PlatformViewCommandJson::Dispose { slot_id } => {
            format!("{{\"op\":\"dispose\",\"slot\":{slot_id}}}")
        }
        PlatformViewCommandJson::Overlay {
            overlay_id,
            above_slot_id,
            rect,
            range_start,
            range_end,
        } => format!(
            "{{\"op\":\"overlay\",\"overlay\":{overlay_id},\"aboveSlot\":{above_slot_id},\"rect\":{},\"rangeStart\":{range_start},\"rangeEnd\":{range_end}}}",
            platform_view_rect_json(*rect),
        ),
        PlatformViewCommandJson::DisposeOverlay { overlay_id } => {
            format!("{{\"op\":\"disposeOverlay\",\"overlay\":{overlay_id}}}")
        }
    }
}

/// Serialise a whole not-yet-acknowledged platform-view command backlog into
/// the `nativePlatformViewCommands` JSON both mobile shells serve
/// byte-identically (a frozen contract; the iOS shell mirrors it exactly):
///
/// `{"generation":N,"commands":[{"op":"create","slot":N,"viewType":"...","params":"..."},`
/// `{"op":"update","slot":N,"rect":[x,y,w,h],"clip":[x,y,w,h]|null,"visible":bool},`
/// `{"op":"updateParams","slot":N,"params":"..."},{"op":"dispose","slot":N}]}`.
///
/// Two overlay ops extend the vocabulary additively — the iOS shell mirrors
/// them byte-for-byte, and a host that does not know them logs and skips them
/// like any unknown op:
///
/// `{"op":"overlay","overlay":N,"aboveSlot":N,"rect":[x,y,w,h],"rangeStart":N,"rangeEnd":N}`
/// (create or re-place the overlay surface above slot `aboveSlot`; the range
/// is the frame's command range it draws, informational for the host — the
/// render side pairs ranges with frames itself) and
/// `{"op":"disposeOverlay","overlay":N}` (tear it down). Neither op is emitted
/// unless overlays are switched on (`FRUST_OVERLAYS`).
///
/// An empty `commands` slice still yields a valid (if pointless — the caller
/// should have taken the [`platform_view_commands_up_to_date`] fast path
/// instead) `{"generation":N,"commands":[]}`.
pub(crate) fn build_platform_view_commands_json(
    generation: u64,
    commands: &[PlatformViewCommandJson],
) -> String {
    let mut cmds = String::new();
    for (i, cmd) in commands.iter().enumerate() {
        if i > 0 {
            cmds.push(',');
        }
        cmds.push_str(&platform_view_command_json(cmd));
    }
    format!("{{\"generation\":{generation},\"commands\":[{cmds}]}}")
}

// ---------------------------------------------------------------------
// Pipeline-cache persistence: pure path / diff / IO helpers.
//
// The Android shell persists wgpu's `PipelineCache` blob across launches so
// second-and-later starts skip Vulkan shader-pipeline compilation (see
// `crate::jni_glue::create_handle`). These helpers are the host-testable core of
// that path — the on-disk location, the "did the blob change" check, and the
// atomic read/write. All I/O is best-effort: the caller logs-and-ignores every
// failure, since starting from an empty cache is always a correct fallback.
// ---------------------------------------------------------------------

/// The on-disk path of the persisted pipeline-cache blob:
/// `<cache_dir>/frust/pipeline_cache.bin`.
///
/// `cache_dir` is the app's `context.cacheDir.absolutePath`, delivered across the
/// JNI boundary by `nativeInit` (see `FrustSurfaceView.surfaceCreated`). The
/// `frust` subdirectory keeps the framework's file out of the app's own cache
/// namespace.
pub(crate) fn pipeline_cache_path(cache_dir: &str) -> PathBuf {
    Path::new(cache_dir)
        .join("frust")
        .join("pipeline_cache.bin")
}

/// Whether a freshly read cache blob differs from the one loaded at startup, so
/// an unchanged blob is never rewritten (once the cache stabilises across
/// launches, no disk churn). A `None` `loaded` (no prior blob on disk) always
/// differs from any `new` payload.
pub(crate) fn pipeline_cache_differs(loaded: Option<&[u8]>, new: &[u8]) -> bool {
    loaded != Some(new)
}

/// Read the persisted pipeline-cache blob, or `None` if it is absent or
/// unreadable. Best-effort: a missing/corrupt file is treated as a cold start
/// (the renderer starts from an empty cache), never an error.
pub(crate) fn load_pipeline_cache(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// Atomically persist `data` to `path`: create the parent directory, write to a
/// process-unique temp sibling, then rename it over `path`.
///
/// The rename is an atomic swap on the same filesystem, so a concurrent reader
/// (a parallel launch) never observes a half-written file. Returns the
/// underlying `io::Error` on any failure so the caller can log it; persistence
/// is best-effort and a failure only means the next launch pays the cold-compile
/// cost again.
pub(crate) fn write_pipeline_cache_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    frust_paths::atomic_write(path, data)
}

/// The generation of the newest pipeline-cache blob this process has written.
static PIPELINE_CACHE_WRITTEN: Mutex<u64> = Mutex::new(0);

/// The last generation [`next_pipeline_cache_generation`] handed out.
static PIPELINE_CACHE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// A ticket for one blob about to be persisted, taken on the thread that read
/// the blob — so tickets follow the order the blobs were read in, whatever
/// order their detached writers later run in.
pub(crate) fn next_pipeline_cache_generation() -> u64 {
    PIPELINE_CACHE_GENERATION.fetch_add(1, Ordering::Relaxed) + 1
}

/// [`write_pipeline_cache_atomic`] for a blob read under ticket `generation`,
/// skipped (`Ok(false)`) when a blob read later has already been written.
///
/// A launch can persist more than once — after the warm-up, then on surface
/// destroy — each from its own detached writer thread. Serializing the writes
/// keeps two of them off the shared temp file, and the generation check keeps
/// an older blob from landing over a newer one.
pub(crate) fn write_pipeline_cache_in_order(
    path: &Path,
    data: &[u8],
    generation: u64,
) -> std::io::Result<bool> {
    write_in_order(&PIPELINE_CACHE_WRITTEN, generation, || {
        write_pipeline_cache_atomic(path, data)
    })
}

/// The bytes the pipeline-cache file is known to hold, shared with the
/// detached writers that update it.
///
/// A writer replaces the slot only once its write has succeeded, so a failed
/// write leaves the old bytes standing and the next persist tries again.
pub(crate) type OnDiskCache = Arc<Mutex<Option<Arc<[u8]>>>>;

/// A slot holding `loaded`, the file's contents at startup.
pub(crate) fn on_disk_cache(loaded: Option<Vec<u8>>) -> OnDiskCache {
    Arc::new(Mutex::new(loaded.map(Arc::from)))
}

/// [`pipeline_cache_differs`] against the slot's bytes.
pub(crate) fn differs_from_on_disk(slot: &OnDiskCache, new: &[u8]) -> bool {
    let held = slot.lock().unwrap_or_else(PoisonError::into_inner);
    pipeline_cache_differs(held.as_deref(), new)
}

/// [`write_pipeline_cache_in_order`] for a blob that, once it has landed,
/// becomes the bytes `on_disk` reports.
pub(crate) fn write_pipeline_cache_recording(
    path: &Path,
    data: &Arc<[u8]>,
    generation: u64,
    on_disk: &OnDiskCache,
) -> std::io::Result<bool> {
    write_in_order(&PIPELINE_CACHE_WRITTEN, generation, || {
        record_after_write(on_disk, data, || write_pipeline_cache_atomic(path, data))
    })
}

/// Runs `write` and, only when it succeeds, makes `data` the slot's bytes.
/// Called under [`write_in_order`]'s lock, so slot updates follow the order
/// the blobs were read in.
fn record_after_write(
    on_disk: &OnDiskCache,
    data: &Arc<[u8]>,
    write: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    write()?;
    *on_disk.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(data));
    Ok(())
}

/// Runs `job` on a detached thread named for the pipeline-cache writer. A
/// thread the OS refuses is logged and the job dropped: the persist is
/// best-effort and the caller's bookkeeping is untouched, so it is retried.
pub(crate) fn spawn_pipeline_cache_writer(job: impl FnOnce() + Send + 'static) {
    if let Err(err) = std::thread::Builder::new()
        .name("frust pipeline-cache writer".into())
        .spawn(job)
    {
        log::warn!("frust-shell-android: could not start the pipeline-cache writer: {err}");
    }
}

/// The ordering half of [`write_pipeline_cache_in_order`], over an injectable
/// `written` slot: runs `write` under the slot's lock only for a generation
/// newer than the slot holds, and records it only once the write succeeded.
fn write_in_order(
    written: &Mutex<u64>,
    generation: u64,
    write: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<bool> {
    let mut newest = written.lock().unwrap_or_else(PoisonError::into_inner);
    if generation <= *newest {
        return Ok(false);
    }
    write()?;
    *newest = generation;
    Ok(true)
}

// ---------------------------------------------------------------------
// GPU pre-init join resolution: the adopt-vs-fallback decision.
//
// `JNI_OnLoad` spawns a background thread that builds a `RenderContext` and
// creates its device (see `crate::jni_glue`); `nativeInit` joins it. This is the
// pure, host-testable core of that join's outcome handling — kept free of any
// `jni`/GPU dependency (the real `RenderContext` is an Android-only dependency)
// by staying generic over the context type.
// ---------------------------------------------------------------------

/// Resolve a joined `JNI_OnLoad` GPU pre-init outcome into the context
/// `create_handle` uses. The `joined` argument encodes three cases the
/// caller has already flattened the thread-join into:
///
/// - `Some(Some(ctx))` — the pre-init thread finished and produced a ready GPU
///   context: **adopt it**.
/// - `Some(None)` — the thread ran but its device init failed: **fall back** to a
///   freshly built context via `fresh`.
/// - `None` — no pre-init was present (`JNI_OnLoad` never ran, or a prior
///   `nativeInit` already took the handle) or the thread panicked (the caller
///   maps a join panic to `None`): **fall back**.
///
/// This is the strictly best-effort, single-shot contract the task requires — a
/// panicked or absent pre-init is never fatal, it just costs this launch the
/// synchronous GPU-init path it always had.
pub(crate) fn resolve_preinit<T>(joined: Option<Option<T>>, fresh: impl FnOnce() -> T) -> T {
    match joined {
        Some(Some(ctx)) => ctx,
        _ => fresh(),
    }
}

/// Publish a surface (re)install's **resolved** translucency onto the shared,
/// cross-thread flag the UI thread reads each frame.
///
/// `resolved` is `Some(translucent)` for a successful install — the value
/// `frust_render::SurfaceRenderer::surface_resolved_translucent` reports for
/// the surface that just went live — and `None` for a FAILED one, which stores
/// `false`: with no surface whose alpha mode we can vouch for, the safe
/// contract is the opaque Mode A one (never punch a hole you can't prove is a
/// window), and leaving the previous surface's value standing would be exactly
/// the stale-truth bug this seam exists to remove.
///
/// `Release` pairs with [`read_resolved_translucency`]'s `Acquire` so the UI
/// thread observing a store also observes everything the render thread did
/// before it.
#[inline]
pub(crate) fn publish_resolved_translucency(
    flag: &std::sync::atomic::AtomicBool,
    resolved: Option<bool>,
) {
    flag.store(
        resolved.unwrap_or(false),
        std::sync::atomic::Ordering::Release,
    );
}

/// The UI-thread half of [`publish_resolved_translucency`]: read the live
/// surface's resolved translucency before choosing this frame's base color and
/// pushing `RenderRoot::set_surface_translucent`.
///
/// Until the render thread's first install lands, this reads the
/// construction-time seed — the app's REQUEST — so the capable common case
/// renders Mode B from frame 1; a fallback downgrades it within one frame of
/// the install (the one-optimistic-frame window, documented at the field).
#[inline]
pub(crate) fn read_resolved_translucency(flag: &std::sync::atomic::AtomicBool) -> bool {
    flag.load(std::sync::atomic::Ordering::Acquire)
}

/// This frame's base clear color, chosen from the surface's **resolved**
/// translucency.
///
/// A surface that really came up translucent clears to `transparent` so a
/// native sibling view behind it shows through wherever nothing painted (Mode
/// B); everything else — including a translucency request the platform
/// refused — clears to the theme's opaque surface color, exactly as before
/// platform views existed.
///
/// Generic over the color type purely so this decision stays host-testable:
/// `peniko` is an Android-gated dependency of this crate, and this module
/// compiles on every host (see the module docs).
#[inline]
pub(crate) fn base_clear_color<C>(
    resolved_translucent: bool,
    transparent: C,
    opaque_surface: C,
) -> C {
    if resolved_translucent {
        transparent
    } else {
        opaque_surface
    }
}

// ---------------------------------------------------------------------
// Per-frame state sync: one packed word instead of five polls.
// ---------------------------------------------------------------------

/// The IME surface published since the last sync differs from the one the
/// previous sync saw (or an IME-side input arrived in between, or this is the
/// first sync of a handle). Gates Kotlin's `nativeImeState` read.
pub(crate) const FRAME_SYNC_IME_CHANGED: i64 = 1 << 0;
/// A clipboard write is waiting; the text is held for `nativeTakeClipboardWrite`.
pub(crate) const FRAME_SYNC_CLIPBOARD_WRITE: i64 = 1 << 1;
/// A paste request is waiting; held for `nativeTakePasteRequest`.
pub(crate) const FRAME_SYNC_PASTE_REQUEST: i64 = 1 << 2;
/// The system-UI override generation advanced (or this is the first sync of a
/// handle). Gates Kotlin's `nativeSystemUiState` read.
pub(crate) const FRAME_SYNC_SYSTEM_UI_CHANGED: i64 = 1 << 3;
/// The app's resolved brightness flipped (or this is the first sync of a
/// handle). Gates Kotlin's `nativeAppIsDark` read.
pub(crate) const FRAME_SYNC_DARK_CHANGED: i64 = 1 << 4;
/// Every defined `FRAME_SYNC_*` bit (bits 0-4): what `nativeFrameSync` answers
/// when gating is off, so Kotlin makes every poll every frame.
pub(crate) const FRAME_SYNC_ALL_BITS: i64 = FRAME_SYNC_IME_CHANGED
    | FRAME_SYNC_CLIPBOARD_WRITE
    | FRAME_SYNC_PASTE_REQUEST
    | FRAME_SYNC_SYSTEM_UI_CHANGED
    | FRAME_SYNC_DARK_CHANGED;

/// The frame-sync rollback environment/compile-time variable: when set to any
/// non-`"0"` value, `nativeFrameSync` stops gating and answers every bit, so
/// Kotlin polls each export every frame as it did before the gate existed.
pub(crate) const NO_FRAME_SYNC_VAR: &str = "FRUST_NO_FRAME_SYNC";

/// The pure decision [`frame_sync_gating_enabled`] negates, parsed like
/// `FRUST_NO_RENDER_THREAD`: a non-`"0"` value from either the compile-time
/// or runtime source engages the rollback.
pub(crate) fn frame_sync_rollback(compile_time: Option<&str>, runtime: Option<&str>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(compile_time) || is_set_non_zero(runtime)
}

/// Whether `nativeFrameSync` gates Kotlin's polls: `true` unless
/// [`NO_FRAME_SYNC_VAR`] is engaged. Read once and cached.
pub(crate) fn frame_sync_gating_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        !frame_sync_rollback(
            option_env!("FRUST_NO_FRAME_SYNC"),
            std::env::var(NO_FRAME_SYNC_VAR).ok().as_deref(),
        )
    })
}

// Bit 5 is deliberately unassigned: the platform-view command poll stays
// per-frame, because its peek records each poll in the scroll-sync tail and
// so is not a side-effect-free read to gate on.

/// What one per-frame sync observed, handed to [`FrameSyncLatch::sync`].
pub(crate) struct FrameObservation<'a> {
    /// The IME surface JSON `nativeImeState` would serve right now.
    pub(crate) ime_json: &'a str,
    /// A freshly drained clipboard write, if the slot held one.
    pub(crate) clipboard_write: Option<String>,
    /// Whether the paste-request slot was set when drained.
    pub(crate) paste_request: bool,
    /// The packed `(generation, mode)` word of the system-UI slot.
    pub(crate) system_ui_encoded: u64,
    /// The app's resolved brightness.
    pub(crate) dark: bool,
}

/// What Kotlin last consumed, so the next sync can say what moved.
///
/// The IME and brightness baselines are the values Kotlin last *read* — set
/// by a sync that reports the bit (Kotlin reads next) and by
/// [`FrameSyncLatch::note_ime_consumed`] / [`FrameSyncLatch::note_dark_consumed`]
/// when an export serves them outside a sync — so a state that reverts after
/// an out-of-band read still reports the bit.
///
/// The clipboard slots are one-shot edges that only the app can drain, so a
/// drain done to learn "something is pending" must not lose the value: it is
/// held here until the existing `nativeTakeClipboardWrite` /
/// `nativeTakePasteRequest` calls hand it to Kotlin.
#[derive(Debug, Default)]
pub(crate) struct FrameSyncLatch {
    handle: i64,
    ime_json: Option<String>,
    ime_dirty: bool,
    system_ui_generation: Option<u64>,
    dark: Option<bool>,
    clipboard_write: Option<String>,
    paste_request: bool,
}

impl FrameSyncLatch {
    pub(crate) const fn new() -> Self {
        Self {
            handle: 0,
            ime_json: None,
            ime_dirty: false,
            system_ui_generation: None,
            dark: None,
            clipboard_write: None,
            paste_request: false,
        }
    }

    /// Forget everything: the next sync reports every state bit.
    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    /// Note an input that Kotlin's IME mirror may need to reconcile against
    /// even if the published state does not move (a rejected edit): the next
    /// sync reports the IME bit once.
    pub(crate) fn mark_ime_dirty(&mut self) {
        self.ime_dirty = true;
    }

    /// Record the IME JSON `nativeImeState` just handed to Kotlin as the
    /// consumed baseline.
    pub(crate) fn note_ime_consumed(&mut self, json: &str) {
        match &mut self.ime_json {
            Some(held) => {
                held.clear();
                held.push_str(json);
            }
            None => self.ime_json = Some(json.to_owned()),
        }
    }

    /// Record the brightness `nativeAppIsDark` just handed to Kotlin as the
    /// consumed baseline.
    pub(crate) fn note_dark_consumed(&mut self, dark: bool) {
        self.dark = Some(dark);
    }

    /// Whether a drained clipboard write is still waiting to be taken.
    pub(crate) fn clipboard_write_held(&self) -> bool {
        self.clipboard_write.is_some()
    }

    /// Hand over the held clipboard write, if any.
    pub(crate) fn take_clipboard_write(&mut self) -> Option<String> {
        self.clipboard_write.take()
    }

    /// Hand over (and clear) the held paste request.
    pub(crate) fn take_paste_request(&mut self) -> bool {
        std::mem::take(&mut self.paste_request)
    }

    /// Fold one observation into the latch and return the packed state word.
    pub(crate) fn sync(&mut self, handle: i64, observed: FrameObservation<'_>) -> i64 {
        if self.handle != handle {
            self.reset();
            self.handle = handle;
        }
        let mut bits = 0;

        let ime_moved = self.ime_dirty || self.ime_json.as_deref() != Some(observed.ime_json);
        if ime_moved {
            bits |= FRAME_SYNC_IME_CHANGED;
            self.note_ime_consumed(observed.ime_json);
        }
        self.ime_dirty = false;

        if let Some(text) = observed.clipboard_write {
            self.clipboard_write = Some(text);
        }
        if self.clipboard_write.is_some() {
            bits |= FRAME_SYNC_CLIPBOARD_WRITE;
        }
        self.paste_request |= observed.paste_request;
        if self.paste_request {
            bits |= FRAME_SYNC_PASTE_REQUEST;
        }

        let generation = observed.system_ui_encoded >> 8;
        if self.system_ui_generation != Some(generation) {
            self.system_ui_generation = Some(generation);
            bits |= FRAME_SYNC_SYSTEM_UI_CHANGED;
        }
        if self.dark != Some(observed.dark) {
            self.dark = Some(observed.dark);
            bits |= FRAME_SYNC_DARK_CHANGED;
        }
        bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn resolved_translucency_handoff_survives_a_fake_install_sequence() {
        // The Arc<AtomicBool> handoff the render-thread split runs on, driven
        // here with FAKE installs so it is host-testable (the real
        // `install_surface` needs a GPU and an `ANativeWindow`).
        //
        // Seeded from the REQUEST: the app asked for translucency, so frame 1
        // renders Mode B optimistically rather than flashing opaque while the
        // render thread is still creating the surface.
        let flag = AtomicBool::new(true);
        assert!(read_resolved_translucency(&flag));

        // A real resolution can only ever downgrade it: this device advertised
        // no translucent alpha mode, so the surface came up opaque.
        publish_resolved_translucency(&flag, Some(false));
        assert!(
            !read_resolved_translucency(&flag),
            "the UI thread must observe a render-thread downgrade"
        );
        assert_eq!(
            base_clear_color(read_resolved_translucency(&flag), "TRANSPARENT", "surface"),
            "surface",
            "a fallback-to-opaque surface keeps the opaque theme base color"
        );

        // A capable reinstall (e.g. after a rotation) restores Mode B.
        publish_resolved_translucency(&flag, Some(true));
        assert!(read_resolved_translucency(&flag));
        assert_eq!(
            base_clear_color(read_resolved_translucency(&flag), "TRANSPARENT", "surface"),
            "TRANSPARENT"
        );

        // A FAILED reinstall clears it rather than leaving the previous
        // surface's `true` standing.
        publish_resolved_translucency(&flag, None);
        assert!(!read_resolved_translucency(&flag));
    }

    #[test]
    fn resolve_preinit_adopts_a_ready_context() {
        // Pre-init succeeded: the pre-built context is adopted, `fresh` unused.
        let ctx = resolve_preinit(Some(Some(42)), || panic!("fresh must not run"));
        assert_eq!(ctx, 42);
    }

    #[test]
    fn resolve_preinit_falls_back_when_device_init_failed() {
        // `Some(None)`: the thread ran but device init failed on it (the
        // "poisoned result" case) — build fresh synchronously.
        let ctx = resolve_preinit(Some(None), || 7);
        assert_eq!(ctx, 7);
    }

    #[test]
    fn resolve_preinit_falls_back_when_absent_or_panicked() {
        // `None`: no pre-init spawned / already taken, or the thread panicked
        // (mapped to `None` by the caller) — both take the fresh path.
        let ctx = resolve_preinit(None, || 9);
        assert_eq!(ctx, 9);
    }

    #[test]
    fn zero_handle_is_not_live() {
        assert!(!handle_is_live(0));
    }

    #[test]
    fn nonzero_handle_is_live() {
        assert!(handle_is_live(1));
        assert!(handle_is_live(-1));
        assert!(handle_is_live(0x7fff_ffff_ffff_ffff));
    }

    #[test]
    fn touch_slot_map_first_finger_gets_slot_zero() {
        let mut map = TouchSlotMap::new();
        assert_eq!(map.assign(/* android pointerId */ 7), 0);
        assert_eq!(map.slot_for(7), Some(0));
    }

    #[test]
    fn touch_slot_map_second_finger_gets_slot_one() {
        let mut map = TouchSlotMap::new();
        assert_eq!(map.assign(7), 0);
        assert_eq!(
            map.assign(3),
            1,
            "a second simultaneous contact takes slot 1"
        );
    }

    #[test]
    fn touch_slot_map_release_of_first_keeps_second_at_its_slot() {
        let mut map = TouchSlotMap::new();
        map.assign(7); // slot 0
        map.assign(3); // slot 1
        map.release(7); // first finger lifts
        assert_eq!(
            map.slot_for(3),
            Some(1),
            "the surviving contact must not be renumbered down to slot 0"
        );
        assert_eq!(map.slot_for(7), None);
    }

    #[test]
    fn touch_slot_map_all_up_resets_the_next_down_to_zero() {
        let mut map = TouchSlotMap::new();
        map.assign(7);
        map.assign(3);
        map.release(7);
        map.release(3);
        assert_eq!(
            map.assign(42),
            0,
            "once every contact has lifted, the next Down starts over at slot 0"
        );
    }

    #[test]
    fn touch_slot_map_a_freed_slot_is_reused_by_a_later_contact() {
        let mut map = TouchSlotMap::new();
        map.assign(7); // slot 0
        map.assign(3); // slot 1
        map.release(7); // slot 0 now free, slot 1 still held
        assert_eq!(
            map.assign(9),
            0,
            "a new contact takes the lowest free slot, not the next unused number"
        );
        assert_eq!(
            map.slot_for(3),
            Some(1),
            "the untouched contact is unaffected"
        );
    }

    #[test]
    fn touch_slot_map_duplicate_assign_returns_the_existing_slot() {
        let mut map = TouchSlotMap::new();
        assert_eq!(map.assign(7), 0);
        assert_eq!(
            map.assign(7),
            0,
            "re-assigning an already-tracked id must not take a second slot"
        );
    }

    #[test]
    fn touch_slot_map_release_of_an_unknown_id_is_a_no_op() {
        let mut map = TouchSlotMap::new();
        map.assign(7);
        map.release(999); // never assigned
        assert_eq!(
            map.slot_for(7),
            Some(0),
            "an untracked release must not disturb slot 0"
        );
    }

    #[test]
    fn touch_action_codes_map_to_phases() {
        assert_eq!(touch_phase_from_action(0), TouchPhase::Down);
        assert_eq!(touch_phase_from_action(1), TouchPhase::Move);
        assert_eq!(touch_phase_from_action(2), TouchPhase::Up);
        assert_eq!(touch_phase_from_action(3), TouchPhase::Cancel);
    }

    #[test]
    fn unknown_touch_action_falls_back_to_cancel() {
        // A future/corrupt code must release capture, not strand a "down".
        assert_eq!(touch_phase_from_action(4), TouchPhase::Cancel);
        assert_eq!(touch_phase_from_action(-1), TouchPhase::Cancel);
        assert_eq!(touch_phase_from_action(i32::MAX), TouchPhase::Cancel);
    }

    #[test]
    fn frame_time_nanos_passes_positive_values_through() {
        assert_eq!(frame_time_nanos_from_jlong(0), 0);
        assert_eq!(frame_time_nanos_from_jlong(1), 1);
        assert_eq!(frame_time_nanos_from_jlong(i64::MAX), i64::MAX as u64);
    }

    #[test]
    fn frame_time_nanos_clamps_negative_to_zero() {
        // A corrupt/negative `jlong` must clamp to `0`, never wrap to a huge
        // `u64` via `as`.
        assert_eq!(frame_time_nanos_from_jlong(-1), 0);
        assert_eq!(frame_time_nanos_from_jlong(i64::MIN), 0);
    }

    #[test]
    fn appearance_from_dark_maps_true_to_dark() {
        // init light -> set dark -> the transition Rust's brightness flip applies.
        assert_eq!(appearance_from_dark(false), Appearance::Light);
        assert_eq!(appearance_from_dark(true), Appearance::Dark);
    }

    #[test]
    fn back_press_consume_mirrors_handles_back() {
        // The framework consumes a back press exactly when it advertised it
        // would (`handles_back`); otherwise the platform dispatcher exits.
        assert!(
            should_consume_back_press(true),
            "handles_back=true -> consume + return JNI_TRUE (framework pops)"
        );
        assert!(
            !should_consume_back_press(false),
            "handles_back=false -> return JNI_FALSE (activity finishes)"
        );
    }

    #[test]
    fn edit_command_codes_map_to_verbs() {
        assert_eq!(edit_command_from_code(0), Some(EditCommandWire::Copy));
        assert_eq!(edit_command_from_code(1), Some(EditCommandWire::Cut));
        assert_eq!(edit_command_from_code(2), Some(EditCommandWire::Paste));
        assert_eq!(edit_command_from_code(3), Some(EditCommandWire::SelectAll));
    }

    #[test]
    fn an_unknown_edit_command_code_is_ignored_not_defaulted() {
        // A code this side cannot decode must reach no widget: every verb in
        // this vocabulary edits a document, so a fallback verb would mutate the
        // user's text on a value that was never understood.
        assert_eq!(edit_command_from_code(4), None);
        assert_eq!(edit_command_from_code(-1), None);
        assert_eq!(edit_command_from_code(i32::MAX), None);
        assert_eq!(edit_command_from_code(i32::MIN), None);
    }

    #[test]
    fn ime_indices_pass_valid_values_through_unchanged() {
        // UTF-16 units cross the seam unchanged (incl. a valid 0 caret).
        assert_eq!(normalize_ime_indices(0, 0, -1, -1), (0, 0, -1, -1));
        assert_eq!(normalize_ime_indices(3, 5, 3, 5), (3, 5, 3, 5));
        assert_eq!(normalize_ime_indices(0, 12, -1, -1), (0, 12, -1, -1));
    }

    #[test]
    fn ime_indices_clamp_out_of_domain_negatives_to_none() {
        // Any `< -1` (corrupt platform value) collapses to the `-1` sentinel so
        // it can never be misread as a large positive index.
        assert_eq!(normalize_ime_indices(-1, -1, -1, -1), (-1, -1, -1, -1));
        assert_eq!(
            normalize_ime_indices(-2, -100, i32::MIN, -3),
            (-1, -1, -1, -1)
        );
        assert_eq!(normalize_ime_indices(-5, 4, -1, -9), (-1, 4, -1, -1));
    }

    #[test]
    fn json_escape_handles_quotes_and_backslashes() {
        assert_eq!(json_escape(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(json_escape(r"a\b"), r"a\\b");
        assert_eq!(json_escape("tab\tnew\nline"), "tab\\tnew\\nline");
        assert_eq!(json_escape("\u{08}\u{0c}\r"), "\\b\\f\\r");
    }

    #[test]
    fn json_escape_emits_control_chars_as_unicode_escapes() {
        // A bare C0 control (NUL, unit separator) must become `\u00XX`.
        assert_eq!(json_escape("\u{00}"), "\\u0000");
        assert_eq!(json_escape("\u{1f}"), "\\u001f");
    }

    #[test]
    fn json_escape_passes_printable_unicode_through() {
        // Multi-byte UTF-8 (accent, CJK, astral emoji) is valid inside a JSON
        // string and must not be escaped.
        assert_eq!(json_escape("café 日本語 😀"), "café 日本語 😀");
    }

    #[test]
    fn build_ime_state_json_with_caret() {
        let state = ImeJsonState {
            active: true,
            text: "hi".to_string(),
            sel_base: 2,
            sel_ext: 2,
            comp_base: -1,
            comp_ext: -1,
            caret: Some((1.5, 2.0, 0.0, 10.0)),
            content_type: "normal",
            suppress_soft_keyboard: false,
        };
        assert_eq!(
            build_ime_state_json(&state),
            "{\"active\":true,\"text\":\"hi\",\"selBase\":2,\"selExt\":2,\"compBase\":-1,\"compExt\":-1,\"caretX\":1.5,\"caretY\":2,\"caretW\":0,\"caretH\":10,\"contentType\":\"normal\",\"suppressSoftKeyboard\":false}"
        );
    }

    #[test]
    fn build_ime_state_json_without_caret_emits_nulls_and_escapes_text() {
        let state = ImeJsonState {
            active: false,
            text: "a\"b".to_string(),
            sel_base: -1,
            sel_ext: -1,
            comp_base: -1,
            comp_ext: -1,
            caret: None,
            content_type: "normal",
            suppress_soft_keyboard: false,
        };
        assert_eq!(
            build_ime_state_json(&state),
            "{\"active\":false,\"text\":\"a\\\"b\",\"selBase\":-1,\"selExt\":-1,\"compBase\":-1,\"compExt\":-1,\"caretX\":null,\"caretY\":null,\"caretW\":null,\"caretH\":null,\"contentType\":\"normal\",\"suppressSoftKeyboard\":false}"
        );
    }

    #[test]
    fn build_ime_state_json_default_is_inactive_empty() {
        assert_eq!(
            build_ime_state_json(&ImeJsonState::default()),
            "{\"active\":false,\"text\":\"\",\"selBase\":-1,\"selExt\":-1,\"compBase\":-1,\"compExt\":-1,\"caretX\":null,\"caretY\":null,\"caretW\":null,\"caretH\":null,\"contentType\":\"normal\",\"suppressSoftKeyboard\":false}"
        );
    }

    #[test]
    fn build_ime_state_json_encodes_suppress_soft_keyboard_true() {
        // The flag rides as a real JSON boolean (not a quoted string) so
        // `JSONObject.optBoolean` on the Kotlin side parses it directly.
        let state = ImeJsonState {
            suppress_soft_keyboard: true,
            ..ImeJsonState::default()
        };
        assert!(build_ime_state_json(&state).contains("\"suppressSoftKeyboard\":true"));
    }

    #[test]
    fn build_ime_state_json_encodes_suppress_soft_keyboard_false() {
        let state = ImeJsonState {
            suppress_soft_keyboard: false,
            ..ImeJsonState::default()
        };
        assert!(build_ime_state_json(&state).contains("\"suppressSoftKeyboard\":false"));
    }

    #[test]
    fn build_ime_state_json_sanitizes_non_finite_caret() {
        let state = ImeJsonState {
            caret: Some((f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 4.0)),
            ..ImeJsonState::default()
        };
        // Non-finite caret components must not emit an unparseable JSON token.
        let json = build_ime_state_json(&state);
        assert!(
            json.contains("\"caretX\":0,\"caretY\":0,\"caretW\":0,\"caretH\":4"),
            "{json}"
        );
        assert!(!json.contains("NaN") && !json.contains("inf"), "{json}");
    }

    #[test]
    fn build_ime_state_json_encodes_content_type_password() {
        let state = ImeJsonState {
            content_type: "password",
            ..ImeJsonState::default()
        };
        assert!(
            build_ime_state_json(&state).contains("\"contentType\":\"password\""),
            "password content type must ride the JSON DTO explicitly, not via Debug"
        );
    }

    #[test]
    fn build_ime_state_json_encodes_content_type_no_suggestions() {
        let state = ImeJsonState {
            content_type: "noSuggestions",
            ..ImeJsonState::default()
        };
        assert!(build_ime_state_json(&state).contains("\"contentType\":\"noSuggestions\""));
    }

    #[test]
    fn build_ime_state_json_encodes_content_type_normal() {
        let state = ImeJsonState {
            content_type: "normal",
            ..ImeJsonState::default()
        };
        assert!(build_ime_state_json(&state).contains("\"contentType\":\"normal\""));
    }

    // -----------------------------------------------------------------
    // Platform-view command JSON
    // -----------------------------------------------------------------

    #[test]
    fn platform_view_json_create_golden() {
        let json = build_platform_view_commands_json(
            1,
            &[PlatformViewCommandJson::Create {
                slot_id: 3,
                view_type: "dev.frust.MapFactory".to_string(),
                params_json: "{\"style\":\"dark\"}".to_string(),
                interactive: false,
            }],
        );
        assert_eq!(
            json,
            "{\"generation\":1,\"commands\":[{\"op\":\"create\",\"slot\":3,\"viewType\":\"dev.frust.MapFactory\",\"params\":\"{\\\"style\\\":\\\"dark\\\"}\",\"interactive\":false}]}"
        );
    }

    #[test]
    fn platform_view_json_update_golden_with_clip() {
        let json = build_platform_view_commands_json(
            2,
            &[PlatformViewCommandJson::Update {
                slot_id: 3,
                rect: (0.0, 10.0, 100.0, 200.0),
                clip: Some((0.0, 10.0, 50.0, 200.0)),
                visible: true,
                shields: Vec::new(),
            }],
        );
        assert_eq!(
            json,
            "{\"generation\":2,\"commands\":[{\"op\":\"update\",\"slot\":3,\"rect\":[0,10,100,200],\"clip\":[0,10,50,200],\"visible\":true,\"shields\":[]}]}"
        );
    }

    #[test]
    fn platform_view_json_update_golden_without_clip_is_null() {
        let json = build_platform_view_commands_json(
            3,
            &[PlatformViewCommandJson::Update {
                slot_id: 1,
                rect: (0.0, 0.0, 10.0, 10.0),
                clip: None,
                visible: false,
                shields: Vec::new(),
            }],
        );
        assert_eq!(
            json,
            "{\"generation\":3,\"commands\":[{\"op\":\"update\",\"slot\":1,\"rect\":[0,0,10,10],\"clip\":null,\"visible\":false,\"shields\":[]}]}"
        );
    }

    #[test]
    fn platform_view_json_update_params_golden() {
        let json = build_platform_view_commands_json(
            4,
            &[PlatformViewCommandJson::UpdateParams {
                slot_id: 5,
                params_json: "{\"a\":2}".to_string(),
            }],
        );
        assert_eq!(
            json,
            "{\"generation\":4,\"commands\":[{\"op\":\"updateParams\",\"slot\":5,\"params\":\"{\\\"a\\\":2}\"}]}"
        );
    }

    #[test]
    fn platform_view_json_dispose_golden() {
        let json = build_platform_view_commands_json(
            5,
            &[PlatformViewCommandJson::Dispose { slot_id: 3 }],
        );
        assert_eq!(
            json,
            "{\"generation\":5,\"commands\":[{\"op\":\"dispose\",\"slot\":3}]}"
        );
    }

    #[test]
    fn platform_view_json_escapes_quotes_and_backslashes_in_view_type_and_params() {
        let json = build_platform_view_commands_json(
            1,
            &[PlatformViewCommandJson::Create {
                slot_id: 1,
                view_type: "dev.frust.\"Weird\"Factory".to_string(),
                params_json: r"a\b".to_string(),
                interactive: false,
            }],
        );
        assert!(
            json.contains(r#""viewType":"dev.frust.\"Weird\"Factory""#),
            "{json}"
        );
        assert!(json.contains(r#""params":"a\\b""#), "{json}");
    }

    #[test]
    fn platform_view_json_multiple_commands_in_one_batch() {
        let json = build_platform_view_commands_json(
            7,
            &[
                PlatformViewCommandJson::Create {
                    slot_id: 3,
                    view_type: "dev.frust.XFactory".to_string(),
                    params_json: String::new(),
                    interactive: false,
                },
                PlatformViewCommandJson::Update {
                    slot_id: 3,
                    rect: (0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    visible: true,
                    shields: Vec::new(),
                },
            ],
        );
        assert_eq!(
            json,
            "{\"generation\":7,\"commands\":[{\"op\":\"create\",\"slot\":3,\"viewType\":\"dev.frust.XFactory\",\"params\":\"\",\"interactive\":false},{\"op\":\"update\",\"slot\":3,\"rect\":[0,0,10,10],\"clip\":null,\"visible\":true,\"shields\":[]}]}"
        );
    }

    #[test]
    fn platform_view_json_empty_backlog_is_a_valid_empty_array() {
        assert_eq!(
            build_platform_view_commands_json(0, &[]),
            "{\"generation\":0,\"commands\":[]}"
        );
    }

    #[test]
    fn platform_view_rect_json_sanitizes_non_finite_components() {
        let json = build_platform_view_commands_json(
            1,
            &[PlatformViewCommandJson::Update {
                slot_id: 1,
                rect: (f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 4.0),
                clip: None,
                visible: true,
                shields: Vec::new(),
            }],
        );
        assert!(json.contains("\"rect\":[0,0,0,4]"), "{json}");
        assert!(!json.contains("NaN") && !json.contains("inf"), "{json}");
    }

    #[test]
    fn platform_view_up_to_date_is_generation_equality() {
        assert!(platform_view_commands_up_to_date(0, 0));
        assert!(platform_view_commands_up_to_date(5, 5));
        assert!(!platform_view_commands_up_to_date(6, 5));
        assert!(!platform_view_commands_up_to_date(5, 6));
    }

    /// The "ack compaction round-trip" host-tested at this crate's
    /// boundary: `frust_shell_common::PlatformViewState`'s own compaction
    /// logic is exhaustively unit-tested in that (non-target-gated) crate
    /// (`acknowledge_compacts_the_backlog` and friends) — it isn't
    /// reachable from this crate's host build at all, since
    /// `frust-shell-common` is only a dependency of `frust-shell-android`
    /// under `cfg(target_os = "android")` (this crate's `Cargo.toml`). What
    /// IS host-testable here is the JSON layer's half of the round trip:
    /// Kotlin polls generation N, gets a payload; it round-trips N back as
    /// `ack_generation`; a second poll with nothing new must take the
    /// no-change fast path rather than re-serializing an (empty) payload.
    #[test]
    fn platform_view_ack_round_trip_at_the_json_layer() {
        let (generation, commands) = (
            3u64,
            vec![PlatformViewCommandJson::Create {
                slot_id: 1,
                view_type: "dev.frust.X".to_string(),
                params_json: String::new(),
                interactive: false,
            }],
        );
        // First poll: Kotlin's ack_generation starts at 0 (nothing seen yet)
        // — not up to date, so it gets the real JSON payload.
        assert!(!platform_view_commands_up_to_date(generation, 0));
        let first_json = build_platform_view_commands_json(generation, &commands);
        assert!(first_json.contains("\"op\":\"create\""));

        // Kotlin round-trips `generation` back as its next `ack_generation`;
        // nothing new was produced in between (the differ's own generation
        // is still 3) — this must be the no-change fast path (null jstring
        // at the JNI layer, tested here as the boolean the export branches
        // on).
        assert!(platform_view_commands_up_to_date(generation, generation));
    }

    // -----------------------------------------------------------------
    // Overlay ops (JSON) and the overlay state machines
    // -----------------------------------------------------------------

    #[test]
    fn platform_view_json_overlay_golden() {
        let json = build_platform_view_commands_json(
            8,
            &[PlatformViewCommandJson::Overlay {
                overlay_id: 2,
                above_slot_id: 7,
                rect: (0.0, 0.0, 1080.0, 2340.0),
                range_start: 41,
                range_end: 96,
            }],
        );
        assert_eq!(
            json,
            "{\"generation\":8,\"commands\":[{\"op\":\"overlay\",\"overlay\":2,\"aboveSlot\":7,\"rect\":[0,0,1080,2340],\"rangeStart\":41,\"rangeEnd\":96}]}"
        );
    }

    #[test]
    fn platform_view_json_dispose_overlay_golden() {
        let json = build_platform_view_commands_json(
            9,
            &[PlatformViewCommandJson::DisposeOverlay { overlay_id: 2 }],
        );
        assert_eq!(
            json,
            "{\"generation\":9,\"commands\":[{\"op\":\"disposeOverlay\",\"overlay\":2}]}"
        );
    }

    #[test]
    fn platform_view_json_overlay_ops_serialise_to_exact_strings() {
        let cases = [
            (
                PlatformViewCommandJson::Overlay {
                    overlay_id: 1,
                    above_slot_id: 3,
                    rect: (0.0, 0.0, 720.5, 1600.0),
                    range_start: 12,
                    range_end: 12,
                },
                "{\"op\":\"overlay\",\"overlay\":1,\"aboveSlot\":3,\"rect\":[0,0,720.5,1600],\"rangeStart\":12,\"rangeEnd\":12}",
            ),
            (
                PlatformViewCommandJson::Overlay {
                    overlay_id: u64::from(u32::MAX) + 5,
                    above_slot_id: 42,
                    rect: (10.0, 20.0, 30.0, 40.0),
                    range_start: 0,
                    range_end: 9000,
                },
                "{\"op\":\"overlay\",\"overlay\":4294967300,\"aboveSlot\":42,\"rect\":[10,20,30,40],\"rangeStart\":0,\"rangeEnd\":9000}",
            ),
            (
                PlatformViewCommandJson::DisposeOverlay { overlay_id: 17 },
                "{\"op\":\"disposeOverlay\",\"overlay\":17}",
            ),
        ];
        for (op, expected) in cases {
            let json = build_platform_view_commands_json(1, std::slice::from_ref(&op));
            assert_eq!(
                json,
                format!("{{\"generation\":1,\"commands\":[{expected}]}}")
            );
        }
    }

    /// The overlay ops are additive: a batch carrying them serialises the
    /// four frozen ops exactly as before, in order, around them.
    #[test]
    fn platform_view_json_overlay_ops_leave_the_frozen_ops_unchanged() {
        let frozen = [
            PlatformViewCommandJson::Create {
                slot_id: 3,
                view_type: "dev.frust.XFactory".to_string(),
                params_json: String::new(),
                interactive: true,
            },
            PlatformViewCommandJson::Update {
                slot_id: 3,
                rect: (0.0, 0.0, 10.0, 10.0),
                clip: None,
                visible: true,
                shields: vec![(1.0, 2.0, 3.0, 4.0)],
            },
            PlatformViewCommandJson::UpdateParams {
                slot_id: 3,
                params_json: "{}".to_string(),
            },
            PlatformViewCommandJson::Dispose { slot_id: 3 },
        ];
        let alone = build_platform_view_commands_json(4, &frozen);
        let mut mixed = frozen.to_vec();
        mixed.insert(
            2,
            PlatformViewCommandJson::Overlay {
                overlay_id: 1,
                above_slot_id: 3,
                rect: (0.0, 0.0, 10.0, 10.0),
                range_start: 5,
                range_end: 8,
            },
        );
        mixed.push(PlatformViewCommandJson::DisposeOverlay { overlay_id: 1 });
        let with_overlays = build_platform_view_commands_json(4, &mixed);
        let overlay = "{\"op\":\"overlay\",\"overlay\":1,\"aboveSlot\":3,\"rect\":[0,0,10,10],\"rangeStart\":5,\"rangeEnd\":8},";
        let dispose = ",{\"op\":\"disposeOverlay\",\"overlay\":1}";
        assert_eq!(
            with_overlays
                .replacen(overlay, "", 1)
                .replacen(dispose, "", 1),
            alone
        );
        assert_eq!(
            alone,
            "{\"generation\":4,\"commands\":[{\"op\":\"create\",\"slot\":3,\"viewType\":\"dev.frust.XFactory\",\"params\":\"\",\"interactive\":true},{\"op\":\"update\",\"slot\":3,\"rect\":[0,0,10,10],\"clip\":null,\"visible\":true,\"shields\":[[1,2,3,4]]},{\"op\":\"updateParams\",\"slot\":3,\"params\":\"{}\"},{\"op\":\"dispose\",\"slot\":3}]}"
        );
    }

    // -----------------------------------------------------------------
    // Pipeline-cache persistence helpers
    // -----------------------------------------------------------------

    /// A fresh, unique temp directory for a round-trip I/O test. Not cleaned up
    /// eagerly — the OS reaps the temp dir; each test uses a distinct path so
    /// parallel runs never collide.
    fn unique_temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("frust-pcache-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn pipeline_cache_path_is_under_frust_subdir() {
        let path = pipeline_cache_path("/data/user/0/it.f0x.demo/cache");
        assert!(path.ends_with("frust/pipeline_cache.bin"), "{path:?}");
        assert_eq!(
            path,
            Path::new("/data/user/0/it.f0x.demo/cache/frust/pipeline_cache.bin")
        );
    }

    #[test]
    fn pipeline_cache_differs_reports_changes() {
        // No prior blob always differs from any new payload.
        assert!(pipeline_cache_differs(None, b"anything"));
        assert!(pipeline_cache_differs(None, b""));
        // Identical bytes do not differ (skip the rewrite).
        assert!(!pipeline_cache_differs(Some(b"same"), b"same"));
        assert!(!pipeline_cache_differs(Some(b""), b""));
        // Any content or length change differs.
        assert!(pipeline_cache_differs(Some(b"old"), b"new"));
        assert!(pipeline_cache_differs(Some(b"short"), b"shorter-plus"));
    }

    #[test]
    fn load_missing_pipeline_cache_returns_none() {
        let dir = unique_temp_dir();
        let path = dir.join("frust").join("pipeline_cache.bin");
        assert_eq!(load_pipeline_cache(&path), None);
    }

    #[test]
    fn write_then_load_pipeline_cache_round_trips() {
        let dir = unique_temp_dir();
        // The parent `frust/` dir does not exist yet — the write must create it.
        let path = dir.join("frust").join("pipeline_cache.bin");
        let blob = vec![0u8, 1, 2, 3, 250, 251, 252, 253];
        write_pipeline_cache_atomic(&path, &blob).unwrap();
        assert!(path.exists(), "atomic write must create the target file");
        assert_eq!(load_pipeline_cache(&path), Some(blob));
    }

    #[test]
    fn an_older_blob_never_lands_over_a_newer_one() {
        let written = Mutex::new(0);
        let landed = std::cell::RefCell::new(Vec::new());
        let write = |blob: &'static str| {
            let landed = &landed;
            move || {
                landed.borrow_mut().push(blob);
                Ok(())
            }
        };
        assert!(write_in_order(&written, 2, write("second")).unwrap());
        // The blob read first reaches its writer last: it is dropped.
        assert!(!write_in_order(&written, 1, write("first")).unwrap());
        assert!(!write_in_order(&written, 2, write("second again")).unwrap());
        assert!(write_in_order(&written, 3, write("third")).unwrap());
        assert_eq!(*landed.borrow(), ["second", "third"]);
    }

    #[test]
    fn a_failed_write_does_not_count_as_written() {
        let written = Mutex::new(0);
        let failed = write_in_order(&written, 1, || Err(std::io::Error::other("disk full")));
        assert!(failed.is_err());
        assert!(write_in_order(&written, 1, || Ok(())).unwrap());
    }

    #[test]
    fn the_on_disk_slot_changes_only_after_a_successful_write() {
        let slot = on_disk_cache(Some(b"old".to_vec()));
        let new: Arc<[u8]> = Arc::from(&b"new"[..]);
        assert!(differs_from_on_disk(&slot, b"new"));
        assert!(!differs_from_on_disk(&slot, b"old"));

        let failed = record_after_write(&slot, &new, || Err(std::io::Error::other("disk full")));
        assert!(failed.is_err());
        assert!(
            differs_from_on_disk(&slot, b"new"),
            "a failed write is retried"
        );

        record_after_write(&slot, &new, || Ok(())).unwrap();
        assert!(!differs_from_on_disk(&slot, b"new"));
        assert!(differs_from_on_disk(&slot, b"old"));
        assert!(differs_from_on_disk(&on_disk_cache(None), b""));
    }

    #[test]
    fn generations_increase_and_the_in_order_write_round_trips() {
        let first = next_pipeline_cache_generation();
        let second = next_pipeline_cache_generation();
        assert!(second > first);
        let dir = unique_temp_dir();
        let path = dir.join("frust").join("pipeline_cache.bin");
        assert!(
            write_pipeline_cache_in_order(&path, b"blob", next_pipeline_cache_generation())
                .unwrap()
        );
        assert_eq!(load_pipeline_cache(&path), Some(b"blob".to_vec()));

        let slot = on_disk_cache(None);
        let next: Arc<[u8]> = Arc::from(&b"next"[..]);
        assert!(
            write_pipeline_cache_recording(&path, &next, next_pipeline_cache_generation(), &slot)
                .unwrap()
        );
        assert_eq!(load_pipeline_cache(&path), Some(b"next".to_vec()));
        assert!(!differs_from_on_disk(&slot, b"next"));
    }

    #[test]
    fn write_pipeline_cache_overwrites_existing() {
        let dir = unique_temp_dir();
        let path = dir.join("frust").join("pipeline_cache.bin");
        write_pipeline_cache_atomic(&path, b"first version").unwrap();
        write_pipeline_cache_atomic(&path, b"second").unwrap();
        assert_eq!(load_pipeline_cache(&path), Some(b"second".to_vec()));
        // The temp sibling must not linger after a successful rename.
        // frust_paths::atomic_write uses extension format: `.tmp.<pid>`
        let leftover = dir
            .join("frust")
            .join(format!("pipeline_cache.tmp.{}", std::process::id()));
        assert!(!leftover.exists(), "temp file should be renamed away");
    }

    fn observation(json: &str) -> FrameObservation<'_> {
        FrameObservation {
            ime_json: json,
            clipboard_write: None,
            paste_request: false,
            system_ui_encoded: 0,
            dark: false,
        }
    }

    #[test]
    fn frame_sync_first_sync_reports_every_state_bit() {
        let mut latch = FrameSyncLatch::new();
        let bits = latch.sync(7, observation("{}"));
        assert_eq!(
            bits,
            FRAME_SYNC_IME_CHANGED | FRAME_SYNC_SYSTEM_UI_CHANGED | FRAME_SYNC_DARK_CHANGED
        );
    }

    #[test]
    fn frame_sync_idle_frame_is_zero() {
        let mut latch = FrameSyncLatch::new();
        latch.sync(7, observation("{}"));
        assert_eq!(latch.sync(7, observation("{}")), 0);
        assert_eq!(latch.sync(7, observation("{}")), 0);
    }

    #[test]
    fn frame_sync_ime_bit_follows_the_published_json_and_dirty_marks() {
        let mut latch = FrameSyncLatch::new();
        latch.sync(7, observation("a"));
        assert_eq!(latch.sync(7, observation("b")), FRAME_SYNC_IME_CHANGED);
        assert_eq!(latch.sync(7, observation("b")), 0);
        latch.mark_ime_dirty();
        assert_eq!(latch.sync(7, observation("b")), FRAME_SYNC_IME_CHANGED);
        assert_eq!(latch.sync(7, observation("b")), 0);
    }

    #[test]
    fn frame_sync_system_ui_and_dark_bits_fire_on_change_only() {
        let mut latch = FrameSyncLatch::new();
        latch.sync(7, observation("{}"));
        let mut next = observation("{}");
        next.system_ui_encoded = (3 << 8) | 1;
        assert_eq!(latch.sync(7, next), FRAME_SYNC_SYSTEM_UI_CHANGED);
        let mut same_generation_other_mode = observation("{}");
        same_generation_other_mode.system_ui_encoded = (3 << 8) | 2;
        assert_eq!(latch.sync(7, same_generation_other_mode), 0);
        let mut flipped = observation("{}");
        flipped.system_ui_encoded = 3 << 8;
        flipped.dark = true;
        assert_eq!(latch.sync(7, flipped), FRAME_SYNC_DARK_CHANGED);
    }

    #[test]
    fn frame_sync_holds_drained_clipboard_values_until_taken() {
        let mut latch = FrameSyncLatch::new();
        latch.sync(7, observation("{}"));
        let mut copy = observation("{}");
        copy.clipboard_write = Some("hello".to_string());
        copy.paste_request = true;
        assert_eq!(
            latch.sync(7, copy),
            FRAME_SYNC_CLIPBOARD_WRITE | FRAME_SYNC_PASTE_REQUEST
        );
        // Still pending on the next sync until Kotlin takes them.
        assert!(latch.clipboard_write_held());
        assert_eq!(
            latch.sync(7, observation("{}")),
            FRAME_SYNC_CLIPBOARD_WRITE | FRAME_SYNC_PASTE_REQUEST
        );
        assert_eq!(latch.take_clipboard_write().as_deref(), Some("hello"));
        assert!(latch.take_paste_request());
        assert!(!latch.take_paste_request());
        assert_eq!(latch.sync(7, observation("{}")), 0);
    }

    #[test]
    fn frame_sync_ime_bit_fires_when_state_reverts_after_an_out_of_band_read() {
        let mut latch = FrameSyncLatch::new();
        assert_ne!(latch.sync(7, observation("A")) & FRAME_SYNC_IME_CHANGED, 0);
        latch.note_ime_consumed("B");
        assert_eq!(latch.sync(7, observation("A")), FRAME_SYNC_IME_CHANGED);
        assert_eq!(latch.sync(7, observation("A")), 0);
    }

    #[test]
    fn frame_sync_dark_bit_fires_when_state_reverts_after_an_out_of_band_read() {
        let mut latch = FrameSyncLatch::new();
        latch.sync(7, observation("{}"));
        latch.note_dark_consumed(true);
        assert_eq!(latch.sync(7, observation("{}")), FRAME_SYNC_DARK_CHANGED);
        assert_eq!(latch.sync(7, observation("{}")), 0);
    }

    #[test]
    fn frame_sync_rollback_parses_like_the_render_thread_switch() {
        assert!(!frame_sync_rollback(None, None));
        assert!(frame_sync_rollback(Some("1"), None));
        assert!(frame_sync_rollback(None, Some("1")));
        assert!(!frame_sync_rollback(Some("0"), Some("0")));
        assert_eq!(FRAME_SYNC_ALL_BITS, 0x1F);
    }

    #[test]
    fn frame_sync_a_new_handle_starts_over() {
        let mut latch = FrameSyncLatch::new();
        latch.sync(7, observation("{}"));
        assert_eq!(latch.sync(7, observation("{}")), 0);
        let bits = latch.sync(8, observation("{}"));
        assert_eq!(
            bits,
            FRAME_SYNC_IME_CHANGED | FRAME_SYNC_SYSTEM_UI_CHANGED | FRAME_SYNC_DARK_CHANGED
        );
    }
}
