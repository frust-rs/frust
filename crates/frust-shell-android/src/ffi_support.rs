//! Android-specific FFI helper: the opaque-handle liveness check.
//!
//! The platform-agnostic plumbing (`guard`, `sanitize_scale`, `logical_size`,
//! the `AppTree` erasure) lives in `frust-shell-common` and is imported at
//! its call sites. What remains here is JNI-handle specific: the sentinel check
//! for the `jlong` the JVM passes back into every native call.
//!
//! This carries no `jni`/`ndk`/GPU dependency so it compiles and is unit-tested
//! on the host (`cargo test --workspace`), even though the code that calls it
//! ([`crate::jni_glue`]) is `#[cfg(target_os = "android")]`.

use std::path::{Path, PathBuf};

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
// Overlay surfaces: the switch, the per-overlay table, the frame ledger.
//
// An overlay is a translucent `SurfaceView` Kotlin stacks above a hosted
// native view; the render thread draws the frame's commands painted after
// that view onto it (`frust_render::overlay`). The JNI half — the windows,
// the `OverlaySurface`s, the render-loop wiring — lives in
// `crate::jni_glue`; the state machines it drives are here, generic over the
// window and surface types, so their ordering contracts are host-tested.
// ---------------------------------------------------------------------

/// The overlay switch: `FRUST_OVERLAYS`, read at runtime or baked at compile
/// time (`frust build --define`). `FRUST_OVERLAY_SPIKE` is still honoured as
/// an alias.
pub(crate) const OVERLAYS_ENV_VAR: &str = "FRUST_OVERLAYS";

/// How (and whether) Kotlin builds overlay surfaces — the switch's value, and
/// the code `nativeOverlayMode` hands the embedding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OverlayMode {
    /// No overlays: the differ is fed without the frame's commands, the JSON
    /// carries no overlay op and the frame path is the single-surface one.
    Off,
    /// Overlay `SurfaceView`s are `setZOrderOnTop(true)` — above the window,
    /// so above every hosted `View`. The default when switched on.
    OnTop,
    /// Overlay `SurfaceView`s are `setZOrderMediaOverlay(true)`
    /// (`FRUST_OVERLAYS=media`) — above the main surface but behind the
    /// window, kept as an A/B arm.
    MediaOverlay,
}

impl OverlayMode {
    /// The mode one switch value selects: blank, `0`, `off` or `false` is
    /// [`Self::Off`], `media` is [`Self::MediaOverlay`], anything else
    /// [`Self::OnTop`] (case-insensitive, surrounding whitespace ignored).
    pub(crate) fn from_value(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty()
            || value == "0"
            || value.eq_ignore_ascii_case("off")
            || value.eq_ignore_ascii_case("false")
        {
            Self::Off
        } else if value.eq_ignore_ascii_case("media") {
            Self::MediaOverlay
        } else {
            Self::OnTop
        }
    }

    /// Resolve the switch from its sources in priority order — the first
    /// one carrying a non-blank value decides (so a runtime `0` can switch
    /// off a baked-in value); none set is [`Self::Off`].
    pub(crate) fn resolve<'a>(sources: impl IntoIterator<Item = Option<&'a str>>) -> Self {
        sources
            .into_iter()
            .flatten()
            .find(|value| !value.trim().is_empty())
            .map_or(Self::Off, Self::from_value)
    }

    /// Whether overlays are switched on at all.
    pub(crate) fn is_on(self) -> bool {
        self != Self::Off
    }

    /// The `nativeOverlayMode` code: `0` off, `1` on top, `2` media overlay
    /// (`FrustViewHost`'s constants mirror these).
    pub(crate) fn code(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::OnTop => 1,
            Self::MediaOverlay => 2,
        }
    }
}

/// One overlay's native half: the window its `SurfaceView` delivered, and
/// the surface the render thread built on it.
#[derive(Debug)]
struct OverlayEntry<W, S> {
    /// Declared before `window` on purpose: fields drop in declaration
    /// order, so even an implicit drop releases the surface before the
    /// window it was built on.
    surface: Option<S>,
    window: Option<W>,
    /// The window's size, physical px.
    size: (u32, u32),
    /// Building a surface on this window was refused (logged once); retried
    /// only for a new window.
    refused: bool,
    /// Frames this overlay's engine refused, for the latched log.
    errors: u32,
}

/// What [`OverlayTable::window_changed`] did with a delivered window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowChange {
    /// A window the overlay did not hold: any surface on the previous one was
    /// dropped first, and the render thread builds a fresh one.
    Adopted,
    /// The window it already holds, at a (possibly) new size; the extra
    /// reference was dropped and the render thread resizes.
    Resized,
}

/// The live overlays by the differ's overlay id: each one's window and the
/// surface built on it.
///
/// Shared by the UI thread, which owns the windows (the overlay
/// `SurfaceView` callbacks deliver and withdraw them), and the render thread,
/// which builds, resizes and draws the surfaces. The one invariant every
/// method keeps: **a surface is dropped before the window it was built on**
/// — a withdrawn window drops its surface first, a replaced window drops the
/// old surface before the old window goes, and [`Self::drop_surfaces`] (the
/// main surface going away) drops every surface and keeps the windows.
#[derive(Debug)]
pub(crate) struct OverlayTable<W, S> {
    entries: std::collections::BTreeMap<u64, OverlayEntry<W, S>>,
}

impl<W, S> Default for OverlayTable<W, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W, S> OverlayTable<W, S> {
    /// An empty table (`const`, for a process-global).
    pub(crate) const fn new() -> Self {
        Self {
            entries: std::collections::BTreeMap::new(),
        }
    }

    /// Overlay `id`'s `SurfaceView` delivered `window` at `size`
    /// (`surfaceCreated`/`surfaceChanged`). `same` answers whether two
    /// windows are the same platform object.
    pub(crate) fn window_changed(
        &mut self,
        id: u64,
        window: W,
        size: (u32, u32),
        same: impl FnOnce(&W, &W) -> bool,
    ) -> WindowChange {
        let entry = self.entries.entry(id).or_insert(OverlayEntry {
            surface: None,
            window: None,
            size,
            refused: false,
            errors: 0,
        });
        entry.size = size;
        if entry
            .window
            .as_ref()
            .is_some_and(|held| same(held, &window))
        {
            drop(window);
            return WindowChange::Resized;
        }
        // Surface first, then the window it was built on.
        entry.surface = None;
        entry.window = Some(window);
        entry.refused = false;
        entry.errors = 0;
        WindowChange::Adopted
    }

    /// Overlay `id`'s `SurfaceView` withdrew its window (`surfaceDestroyed`,
    /// or the overlay was disposed): drop its surface, then release the
    /// window, and forget the overlay. `false` when it held none.
    pub(crate) fn window_destroyed(&mut self, id: u64) -> bool {
        match self.entries.remove(&id) {
            Some(mut entry) => {
                entry.surface = None;
                entry.window = None;
                true
            }
            None => false,
        }
    }

    /// The main surface went away: drop every overlay's surface (they share
    /// its device and must not outlive its teardown) and keep the windows,
    /// so the next [`Self::prepare`] after the main surface returns rebuilds
    /// them. Answers how many surfaces were dropped.
    pub(crate) fn drop_surfaces(&mut self) -> usize {
        let mut dropped = 0;
        for entry in self.entries.values_mut() {
            if entry.surface.take().is_some() {
                dropped += 1;
            }
            entry.refused = false;
        }
        dropped
    }

    /// Bring every surface in line with its window, on the render thread:
    /// `resize` a live one to its window's size, `build` one where a window
    /// has none (and was not refused). Answers each refusal this call made —
    /// the caller logs those; the overlay is then skipped until a new window
    /// arrives.
    pub(crate) fn prepare<E>(
        &mut self,
        mut build: impl FnMut(&W, (u32, u32)) -> Result<S, E>,
        mut resize: impl FnMut(&mut S, (u32, u32)),
    ) -> Vec<(u64, E)> {
        let mut refusals = Vec::new();
        for (&id, entry) in &mut self.entries {
            let Some(window) = entry.window.as_ref() else {
                continue;
            };
            if let Some(surface) = entry.surface.as_mut() {
                resize(surface, entry.size);
            } else if !entry.refused {
                match build(window, entry.size) {
                    Ok(surface) => entry.surface = Some(surface),
                    Err(error) => {
                        entry.refused = true;
                        refusals.push((id, error));
                    }
                }
            }
        }
        refusals
    }

    /// The ids of the overlays with a live surface, ascending.
    pub(crate) fn live_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.surface.is_some())
            .map(|(&id, _)| id)
    }

    /// Whether any overlay has a live surface.
    pub(crate) fn has_live(&self) -> bool {
        self.live_ids().next().is_some()
    }

    /// The live surfaces for `cuts`, in `cuts`' order, each with its range; a
    /// cut whose overlay has no live surface is left out.
    pub(crate) fn cut_surfaces(
        &mut self,
        cuts: &[(u64, std::ops::Range<usize>)],
    ) -> Vec<(u64, std::ops::Range<usize>, &mut S)> {
        let mut live: std::collections::BTreeMap<u64, &mut S> = self
            .entries
            .iter_mut()
            .filter_map(|(&id, entry)| entry.surface.as_mut().map(|surface| (id, surface)))
            .collect();
        cuts.iter()
            .filter_map(|(id, range)| live.remove(id).map(|surface| (*id, range.clone(), surface)))
            .collect()
    }

    /// Count one engine refusal on overlay `id`; answers the running count
    /// while it is at most `log_limit` (the caller logs then), `None` past it
    /// or for an unknown overlay.
    pub(crate) fn note_error(&mut self, id: u64, log_limit: u32) -> Option<u32> {
        let entry = self.entries.get_mut(&id)?;
        entry.errors = entry.errors.saturating_add(1);
        (entry.errors <= log_limit).then_some(entry.errors)
    }

    /// How many overlays the table tracks (with or without a surface).
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// How many frames' cut lists [`OverlayLedger`] keeps for the render thread
/// to pair against — above the platform-view release gate's in-flight bound
/// (`MAX_FRAMES_IN_FLIGHT`, 12), so a frame the render thread can still draw
/// always finds its own cuts.
pub(crate) const OVERLAY_FRAMES_KEPT: usize = 16;

/// One overlay's cut for one frame: its id and the half-open command range of
/// that frame's scene it draws.
pub(crate) type OverlayCutSpec = (u64, std::ops::Range<usize>);

/// The overlays' command ranges, folded from the differ's `Overlay` /
/// `DisposeOverlay` commands on the UI thread, and published per frame for
/// the render thread.
///
/// The differ announces a range only when it changes, and the render thread
/// may draw a frame older than the UI thread's newest paint (the scene channel
/// is latest-wins, the render side runs behind), so the current ranges alone
/// would pair one frame's ranges with another frame's scene. Every painted
/// frame therefore publishes a snapshot under its own frame id
/// ([`Self::publish_frame`]) and the render thread looks its scene's id up
/// ([`Self::frame_cuts`]).
#[derive(Debug)]
pub(crate) struct OverlayLedger {
    ranges: std::collections::BTreeMap<u64, std::ops::Range<usize>>,
    folded_generation: u64,
    frames: std::collections::VecDeque<(u64, Vec<OverlayCutSpec>)>,
}

impl Default for OverlayLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayLedger {
    /// An empty ledger (`const`, for a process-global).
    pub(crate) const fn new() -> Self {
        Self {
            ranges: std::collections::BTreeMap::new(),
            folded_generation: 0,
            frames: std::collections::VecDeque::new(),
        }
    }

    /// Forget everything — a new differ (a new native handle) starts its
    /// generations and overlay ids over.
    pub(crate) fn reset(&mut self) {
        self.ranges.clear();
        self.frames.clear();
        self.folded_generation = 0;
    }

    /// Start folding the differ's backlog at its current `generation`:
    /// `Some(g)` — fold every command pushed after generation `g` (the
    /// backlog's `commands_up_to(g)` prefix is already folded), then call
    /// [`Self::finish_fold`] — or `None` when nothing new was pushed. A
    /// generation older than the one already folded means a new differ, and
    /// restarts the ledger.
    pub(crate) fn begin_fold(&mut self, generation: u64) -> Option<u64> {
        if generation < self.folded_generation {
            self.reset();
        }
        (generation != self.folded_generation).then_some(self.folded_generation)
    }

    /// Fold one `Overlay` command: overlay `id` now draws `range`.
    pub(crate) fn set(&mut self, id: u64, range: std::ops::Range<usize>) {
        self.ranges.insert(id, range);
    }

    /// Fold one `DisposeOverlay` command.
    pub(crate) fn dispose(&mut self, id: u64) {
        self.ranges.remove(&id);
    }

    /// Close a fold [`Self::begin_fold`] opened at `generation`.
    pub(crate) fn finish_fold(&mut self, generation: u64) {
        self.folded_generation = generation;
    }

    /// The ranges as they stand, ordered by range (then id) — what the
    /// frame painted just now draws.
    pub(crate) fn snapshot(&self) -> Vec<OverlayCutSpec> {
        let mut cuts: Vec<OverlayCutSpec> = self
            .ranges
            .iter()
            .map(|(&id, range)| (id, range.clone()))
            .collect();
        cuts.sort_by_key(|(id, range)| (range.start, range.end, *id));
        cuts
    }

    /// Publish the current ranges as frame `frame_id`'s cuts, keeping the
    /// newest [`OVERLAY_FRAMES_KEPT`] frames. Republishing an id replaces
    /// its cuts.
    pub(crate) fn publish_frame(&mut self, frame_id: u64) {
        let cuts = self.snapshot();
        self.frames.retain(|(id, _)| *id != frame_id);
        self.frames.push_back((frame_id, cuts));
        while self.frames.len() > OVERLAY_FRAMES_KEPT {
            self.frames.pop_front();
        }
    }

    /// Frame `frame_id`'s published cuts, `None` when it published none (or
    /// fell out of the kept window).
    pub(crate) fn frame_cuts(&self, frame_id: u64) -> Option<&[OverlayCutSpec]> {
        self.frames
            .iter()
            .rev()
            .find(|(id, _)| *id == frame_id)
            .map(|(_, cuts)| cuts.as_slice())
    }
}

/// The cuts one segmented frame draws: one per overlay with a live surface
/// (`live`), carrying the range its frame published (`frame`, clamped to the
/// scene's `len` commands) — or an empty range at the scene's end when the
/// frame published none for it (not announced yet, disposed, or the frame's
/// cuts are gone), which draws a transparent clear and nothing else. Ordered
/// by range, so the list is the ascending, disjoint cut list
/// `SurfaceRenderer::encode_segmented` takes. An overlay the frame names but
/// that has no live surface gets no cut: its commands stay on the main
/// surface (under the native view in an opaque surface — the graceful
/// degradation).
pub(crate) fn assemble_overlay_cuts(
    frame: Option<&[OverlayCutSpec]>,
    live: impl IntoIterator<Item = u64>,
    len: usize,
) -> Vec<OverlayCutSpec> {
    let mut cuts: Vec<OverlayCutSpec> = live
        .into_iter()
        .map(|id| {
            let range = frame
                .and_then(|cuts| cuts.iter().find(|(cut, _)| *cut == id))
                .map(|(_, range)| range.start.min(len)..range.end.min(len))
                .filter(|range| range.start < range.end)
                .unwrap_or(len..len);
            (id, range)
        })
        .collect();
    cuts.sort_by_key(|(id, range)| (range.start, range.end, *id));
    cuts
}

/// Rendered overlay frames between two `frust-overlay` skew lines.
pub(crate) const OVERLAY_SKEW_EVERY: u32 = 120;

/// One `frust-overlay` skew line's worth of numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OverlaySkewReport {
    /// Frames folded into this report.
    pub(crate) frames: u32,
    /// Mean main-present → last-overlay-present delta, µs.
    pub(crate) avg_us: u64,
    /// Largest delta in the window, µs.
    pub(crate) max_us: u64,
    /// Overlays presented in the window's last frame.
    pub(crate) overlays: usize,
    /// The window's last main present, µs since the Unix epoch.
    pub(crate) main_unix_us: u128,
    /// The window's last overlay present, µs since the Unix epoch.
    pub(crate) overlay_unix_us: u128,
}

/// Main-versus-overlay present skew, accumulated per frame and reported every
/// [`OVERLAY_SKEW_EVERY`] frames — the wall-clock (`SystemTime`) gap between
/// the main surface's present and the last overlay's, read off the log on a
/// device.
#[derive(Debug, Default)]
pub(crate) struct OverlaySkew {
    frames: u32,
    sum_us: u64,
    max_us: u64,
}

impl OverlaySkew {
    /// An empty window (`const`, for a process-global).
    pub(crate) const fn new() -> Self {
        Self {
            frames: 0,
            sum_us: 0,
            max_us: 0,
        }
    }

    /// Fold one frame whose main surface presented at `main` and whose last
    /// overlay (of `overlays`) presented at `last_overlay`; answers a report
    /// every [`OVERLAY_SKEW_EVERY`] frames, then starts a new window. A clock
    /// that stepped backwards in between counts as zero skew.
    pub(crate) fn record(
        &mut self,
        main: std::time::SystemTime,
        last_overlay: std::time::SystemTime,
        overlays: usize,
    ) -> Option<OverlaySkewReport> {
        let delta = last_overlay
            .duration_since(main)
            .unwrap_or_default()
            .as_micros();
        let delta = u64::try_from(delta).unwrap_or(u64::MAX);
        self.frames += 1;
        self.sum_us = self.sum_us.saturating_add(delta);
        self.max_us = self.max_us.max(delta);
        if self.frames < OVERLAY_SKEW_EVERY {
            return None;
        }
        let unix_us = |at: std::time::SystemTime| {
            at.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros()
        };
        let report = OverlaySkewReport {
            frames: self.frames,
            avg_us: self.sum_us / u64::from(self.frames),
            max_us: self.max_us,
            overlays,
            main_unix_us: unix_us(main),
            overlay_unix_us: unix_us(last_overlay),
        };
        *self = Self::new();
        Some(report)
    }
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

    /// Pull one numeric field's raw text out of a flat JSON object — enough
    /// to read an overlay op back without a JSON dependency.
    fn json_field<'a>(object: &'a str, key: &str) -> &'a str {
        let needle = format!("\"{key}\":");
        let start = object
            .find(&needle)
            .unwrap_or_else(|| panic!("{key} in {object}"))
            + needle.len();
        let rest = &object[start..];
        let end = if rest.starts_with('[') {
            rest.find(']').expect("closing bracket") + 1
        } else {
            rest.find([',', '}']).expect("field end")
        };
        &rest[..end]
    }

    /// Read an `overlay`/`disposeOverlay` op back into the value that wrote it.
    fn parse_overlay_op(object: &str) -> PlatformViewCommandJson {
        let number = |key: &str| json_field(object, key).parse::<u64>().expect("number");
        match json_field(object, "op") {
            "\"overlay\"" => {
                let rect: Vec<f32> = json_field(object, "rect")
                    .trim_matches(['[', ']'])
                    .split(',')
                    .map(|v| v.parse().expect("rect component"))
                    .collect();
                PlatformViewCommandJson::Overlay {
                    overlay_id: number("overlay"),
                    above_slot_id: number("aboveSlot"),
                    rect: (rect[0], rect[1], rect[2], rect[3]),
                    range_start: number("rangeStart"),
                    range_end: number("rangeEnd"),
                }
            }
            "\"disposeOverlay\"" => PlatformViewCommandJson::DisposeOverlay {
                overlay_id: number("overlay"),
            },
            other => panic!("not an overlay op: {other}"),
        }
    }

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
    fn platform_view_json_overlay_ops_round_trip() {
        let ops = [
            PlatformViewCommandJson::Overlay {
                overlay_id: 1,
                above_slot_id: 3,
                rect: (0.0, 0.0, 720.5, 1600.0),
                range_start: 12,
                range_end: 12,
            },
            PlatformViewCommandJson::Overlay {
                overlay_id: u64::from(u32::MAX) + 5,
                above_slot_id: 42,
                rect: (10.0, 20.0, 30.0, 40.0),
                range_start: 0,
                range_end: 9000,
            },
            PlatformViewCommandJson::DisposeOverlay { overlay_id: 17 },
        ];
        for op in ops {
            let json = build_platform_view_commands_json(1, std::slice::from_ref(&op));
            let object = json
                .strip_prefix("{\"generation\":1,\"commands\":[")
                .and_then(|rest| rest.strip_suffix("]}"))
                .expect("one-command batch");
            assert_eq!(parse_overlay_op(object), op, "{json}");
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

    #[test]
    fn overlay_mode_reads_the_switch_value() {
        for off in ["", "  ", "0", "off", "OFF", "false"] {
            assert_eq!(OverlayMode::from_value(off), OverlayMode::Off, "{off:?}");
        }
        for on in ["1", "on", "yes", "top"] {
            assert_eq!(OverlayMode::from_value(on), OverlayMode::OnTop, "{on:?}");
        }
        assert_eq!(
            OverlayMode::from_value(" Media "),
            OverlayMode::MediaOverlay
        );
        assert_eq!(OverlayMode::Off.code(), 0);
        assert_eq!(OverlayMode::OnTop.code(), 1);
        assert_eq!(OverlayMode::MediaOverlay.code(), 2);
        assert!(!OverlayMode::Off.is_on());
        assert!(OverlayMode::MediaOverlay.is_on());
    }

    #[test]
    fn overlay_mode_first_set_source_decides() {
        assert_eq!(OverlayMode::resolve([None, None, None]), OverlayMode::Off);
        // The runtime value outranks a baked-in one, including a runtime `0`.
        assert_eq!(
            OverlayMode::resolve([Some("0"), Some("1"), None]),
            OverlayMode::Off
        );
        assert_eq!(
            OverlayMode::resolve([None, Some("media"), Some("1")]),
            OverlayMode::MediaOverlay
        );
        // A blank source is not set; the spike alias answers last.
        assert_eq!(
            OverlayMode::resolve([Some(" "), None, Some("1")]),
            OverlayMode::OnTop
        );
    }

    type Log = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    /// A window or surface stand-in that records its own drop.
    #[derive(Debug)]
    struct Tracked {
        name: String,
        log: Log,
        size: (u32, u32),
    }

    impl Tracked {
        fn new(log: &Log, name: &str) -> Self {
            Self {
                name: name.to_string(),
                log: log.clone(),
                size: (0, 0),
            }
        }
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.log.borrow_mut().push(format!("drop {}", self.name));
        }
    }

    fn same(a: &Tracked, b: &Tracked) -> bool {
        a.name == b.name
    }

    /// Build a surface named after its window.
    fn build_on(log: &Log) -> impl FnMut(&Tracked, (u32, u32)) -> Result<Tracked, String> + '_ {
        move |window, size| {
            let mut surface = Tracked::new(log, &format!("surface@{}", window.name));
            surface.size = size;
            Ok(surface)
        }
    }

    fn resize(surface: &mut Tracked, size: (u32, u32)) {
        surface.size = size;
    }

    fn drops(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.borrow_mut())
    }

    #[test]
    fn overlay_table_attach_builds_and_resizes_on_the_render_thread() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        assert_eq!(
            table.window_changed(1, Tracked::new(&log, "w1"), (100, 200), same),
            WindowChange::Adopted
        );
        // No surface until the render thread prepares.
        assert!(!table.has_live());
        assert!(table.prepare(build_on(&log), resize).is_empty());
        assert_eq!(table.live_ids().collect::<Vec<_>>(), vec![1]);

        // The same window again is a resize: the extra reference goes, the
        // surface stays and picks the size up at the next prepare.
        assert_eq!(
            table.window_changed(1, Tracked::new(&log, "w1"), (300, 400), same),
            WindowChange::Resized
        );
        assert_eq!(drops(&log), vec!["drop w1"]);
        table.prepare(build_on(&log), resize);
        let cut = table.cut_surfaces(&[(1, 0..4)]);
        assert_eq!(cut.len(), 1);
        assert_eq!(cut[0].2.size, (300, 400));
        assert_eq!(cut[0].2.name, "surface@w1");
    }

    #[test]
    fn overlay_table_drops_the_surface_before_its_window() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        table.window_changed(1, Tracked::new(&log, "w1"), (1, 1), same);
        table.prepare(build_on(&log), resize);

        // A new window replaces the old one: old surface, then old window.
        assert_eq!(
            table.window_changed(1, Tracked::new(&log, "w2"), (1, 1), same),
            WindowChange::Adopted
        );
        assert_eq!(drops(&log), vec!["drop surface@w1", "drop w1"]);
        assert!(
            !table.has_live(),
            "the new window waits for a fresh surface"
        );
        table.prepare(build_on(&log), resize);

        // Withdrawn: surface, then window, and the overlay is forgotten.
        assert!(table.window_destroyed(1));
        assert_eq!(drops(&log), vec!["drop surface@w2", "drop w2"]);
        assert_eq!(table.len(), 0);
        assert!(!table.window_destroyed(1), "a second withdraw is a no-op");

        // Dropping the whole table keeps the order too.
        table.window_changed(2, Tracked::new(&log, "w3"), (1, 1), same);
        table.prepare(build_on(&log), resize);
        drop(table);
        assert_eq!(drops(&log), vec!["drop surface@w3", "drop w3"]);
    }

    #[test]
    fn overlay_table_follows_the_main_surface_through_a_recreate() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        table.window_changed(1, Tracked::new(&log, "w1"), (1, 1), same);
        table.window_changed(2, Tracked::new(&log, "w2"), (1, 1), same);
        table.prepare(build_on(&log), resize);

        // The main surface goes away: every surface goes, every window stays.
        assert_eq!(table.drop_surfaces(), 2);
        assert_eq!(drops(&log), vec!["drop surface@w1", "drop surface@w2"]);
        assert!(!table.has_live());
        assert_eq!(table.len(), 2);

        // It comes back: the next prepare rebuilds on the kept windows.
        table.prepare(build_on(&log), resize);
        assert_eq!(table.live_ids().collect::<Vec<_>>(), vec![1, 2]);
        assert!(drops(&log).is_empty());
    }

    #[test]
    fn overlay_table_refuses_once_per_window() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        table.window_changed(1, Tracked::new(&log, "w1"), (1, 1), same);
        let attempts = std::cell::Cell::new(0);
        let mut refuse = |_: &Tracked, _: (u32, u32)| -> Result<Tracked, &'static str> {
            attempts.set(attempts.get() + 1);
            Err("no translucent mode")
        };
        assert_eq!(
            table.prepare(&mut refuse, resize),
            vec![(1, "no translucent mode")]
        );
        // Not retried for the same window...
        assert!(table.prepare(&mut refuse, resize).is_empty());
        assert_eq!(attempts.get(), 1);
        // ...but a new window gets a fresh attempt.
        table.window_changed(1, Tracked::new(&log, "w2"), (1, 1), same);
        assert_eq!(table.prepare(&mut refuse, resize).len(), 1);
        assert_eq!(attempts.get(), 2);
        assert!(!table.has_live());
    }

    #[test]
    fn overlay_table_cuts_follow_the_cut_order_and_skip_dead_overlays() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        for (id, name) in [(1, "a"), (2, "b"), (3, "c")] {
            table.window_changed(id, Tracked::new(&log, name), (1, 1), same);
        }
        table.prepare(build_on(&log), resize);
        table.window_destroyed(2);
        let cuts = table.cut_surfaces(&[(3, 5..9), (2, 9..12), (1, 12..12)]);
        let order: Vec<(u64, std::ops::Range<usize>, String)> = cuts
            .into_iter()
            .map(|(id, range, surface)| (id, range, surface.name.clone()))
            .collect();
        assert_eq!(
            order,
            vec![
                (3, 5..9, "surface@c".to_string()),
                (1, 12..12, "surface@a".to_string())
            ]
        );
    }

    #[test]
    fn overlay_table_latches_error_logging() {
        let log = Log::default();
        let mut table = OverlayTable::<Tracked, Tracked>::new();
        table.window_changed(1, Tracked::new(&log, "w1"), (1, 1), same);
        assert_eq!(table.note_error(1, 2), Some(1));
        assert_eq!(table.note_error(1, 2), Some(2));
        assert_eq!(table.note_error(1, 2), None);
        assert_eq!(table.note_error(9, 2), None);
        // A new window resets the count.
        table.window_changed(1, Tracked::new(&log, "w2"), (1, 1), same);
        assert_eq!(table.note_error(1, 2), Some(1));
    }

    #[test]
    fn overlay_ledger_folds_publishes_and_pairs_frames() {
        let mut ledger = OverlayLedger::new();
        // Generation 1: overlay 1 appears.
        assert_eq!(ledger.begin_fold(1), Some(0));
        ledger.set(1, 10..20);
        ledger.finish_fold(1);
        ledger.publish_frame(1);
        // Nothing new: no fold.
        assert_eq!(ledger.begin_fold(1), None);
        // Generation 2: overlay 2 appears below it, overlay 1's range moves.
        assert_eq!(ledger.begin_fold(2), Some(1));
        ledger.set(2, 4..8);
        ledger.set(1, 12..22);
        ledger.finish_fold(2);
        ledger.publish_frame(2);
        // Generation 3: overlay 1 is disposed.
        ledger.begin_fold(3);
        ledger.dispose(1);
        ledger.finish_fold(3);
        ledger.publish_frame(3);

        // Each frame keeps the ranges it was painted with, sorted by range.
        assert_eq!(ledger.frame_cuts(1), Some(&[(1, 10..20)][..]));
        assert_eq!(ledger.frame_cuts(2), Some(&[(2, 4..8), (1, 12..22)][..]));
        assert_eq!(ledger.frame_cuts(3), Some(&[(2, 4..8)][..]));
        assert_eq!(ledger.frame_cuts(4), None);
    }

    #[test]
    fn overlay_ledger_keeps_a_bounded_window_of_frames() {
        let mut ledger = OverlayLedger::new();
        ledger.begin_fold(1);
        ledger.set(1, 0..1);
        ledger.finish_fold(1);
        for frame in 1..=(OVERLAY_FRAMES_KEPT as u64 + 4) {
            ledger.publish_frame(frame);
        }
        assert_eq!(ledger.frame_cuts(4), None);
        assert!(ledger.frame_cuts(5).is_some());
        assert!(ledger.frame_cuts(OVERLAY_FRAMES_KEPT as u64 + 4).is_some());
        // Republishing an id replaces, never duplicates.
        ledger.begin_fold(2);
        ledger.set(1, 3..4);
        ledger.finish_fold(2);
        ledger.publish_frame(20);
        assert_eq!(ledger.frame_cuts(20), Some(&[(1, 3..4)][..]));
    }

    #[test]
    fn overlay_ledger_restarts_for_a_new_differ() {
        let mut ledger = OverlayLedger::new();
        ledger.begin_fold(5);
        ledger.set(3, 1..2);
        ledger.finish_fold(5);
        ledger.publish_frame(9);
        // A new differ's generations start over: the old ranges go with it.
        assert_eq!(ledger.begin_fold(2), Some(0));
        assert!(ledger.snapshot().is_empty());
        assert_eq!(ledger.frame_cuts(9), None);
        ledger.reset();
        assert_eq!(ledger.begin_fold(0), None);
    }

    #[test]
    fn overlay_cuts_pair_live_surfaces_with_their_frame() {
        let frame = [(2, 4..8), (1, 12..22), (5, 30..40)];
        // Overlay 5 has no live surface: no cut, so the main surface keeps
        // drawing its range. Overlay 7 is live but the frame names no range:
        // a transparent clear at the end.
        let cuts = assemble_overlay_cuts(Some(&frame), [1, 2, 7], 25);
        assert_eq!(cuts, vec![(2, 4..8), (1, 12..22), (7, 25..25)]);
        // A range past the scene clamps; one that clamps empty clears.
        let cuts = assemble_overlay_cuts(Some(&frame), [1, 5], 15);
        assert_eq!(cuts, vec![(1, 12..15), (5, 15..15)]);
        // A frame with no published cuts: every live overlay clears.
        assert_eq!(
            assemble_overlay_cuts(None, [3, 1], 6),
            vec![(1, 6..6), (3, 6..6)]
        );
        assert!(assemble_overlay_cuts(Some(&frame), [], 50).is_empty());
    }

    #[test]
    fn overlay_skew_reports_every_window() {
        use std::time::{Duration, SystemTime};
        let mut skew = OverlaySkew::default();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        for frame in 0..OVERLAY_SKEW_EVERY - 1 {
            let late = Duration::from_micros(if frame == 3 { 900 } else { 100 });
            assert_eq!(skew.record(base, base + late, 2), None);
        }
        // A clock step backwards is zero skew, never a panic.
        let report = skew
            .record(base, base - Duration::from_micros(5), 1)
            .expect("a full window reports");
        assert_eq!(report.frames, OVERLAY_SKEW_EVERY);
        assert_eq!(report.max_us, 900);
        assert_eq!(
            report.avg_us,
            (100 * u64::from(OVERLAY_SKEW_EVERY - 2) + 900) / u64::from(OVERLAY_SKEW_EVERY)
        );
        assert_eq!(report.overlays, 1);
        assert_eq!(report.main_unix_us, 1_000_000_000);
        // The window restarts.
        assert_eq!(skew.record(base, base, 1), None);
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
}
