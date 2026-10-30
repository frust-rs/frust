//! iOS-specific FFI helpers: the pure, host-testable predicates the C-ABI
//! runtime is built from.
//!
//! The platform-agnostic plumbing (`guard`, `sanitize_scale`, `logical_size`,
//! the `AppTree` erasure) lives in `frust-shell-common` and is imported at its
//! call sites. What remains here is iOS-handle specific but still pure: the
//! null-pointer sentinel check for the opaque handle the Swift side passes back
//! into every call, and the paused/ready gate that decides whether a frame does
//! any work.
//!
//! Neither carries a `render`/GPU dependency, so both compile and are unit-tested
//! on the host (`cargo test --workspace`), even though the code that calls them
//! ([`crate::ffi_glue`], [`crate::app`]) is `#[cfg(target_os = "ios")]`.

use std::ffi::c_void;
use std::fmt::Write as _;

use accesskit::{Node, NodeId, TreeUpdate};
#[cfg(test)]
use accesskit::{Tree, TreeId};
use frust_render::SurfacePhase;

/// Assemble an `accesskit::TreeUpdate` from the flat pieces of a
/// `frust_core::SemanticsUpdate`.
///
/// This is the pure, host-testable core of the iOS accesskit push: the iOS
/// adapter ([`crate::accessibility`]) calls it inside `Adapter::update_if_active`
/// to turn a semantics pass into the incremental update accesskit consumes. It
/// lives here (not in the target-gated adapter) so its correctness is covered by
/// `cargo test --workspace` on the host, even though the `accesskit_ios` adapter
/// that consumes the result is only ever compiled for iOS.
///
/// The assembly itself is the shared
/// [`frust_shell_common::tree_update_from_parts`], so all three shells push the
/// same shape: the whole tree every update (node ids are stable across frames),
/// `tree` rooted at `root`, `tree_id` `ROOT`, and `focus` as the caller's
/// already-resolved focus id (`SemanticsUpdate::focus_id()` — the focused node,
/// or the root when nothing is focused). `nodes` is moved through unchanged
/// except for the root's transform.
///
/// # Coordinate contract
///
/// Semantics bounds are **logical px** (= points on iOS), but `accesskit_ios`
/// divides bounds by the view's `contentScaleFactor` and multiplies hit-test
/// points by it, so it defines the tree space as **physical px**. The
/// conversion is one `Affine::scale(scale)` transform on the root: accesskit
/// applies ancestor transforms to every descendant's bounding box and
/// `accesskit_ios` inverts it for hit-testing. Raw node bounds stay logical.
/// At `scale == 1.0` (or with no root in `nodes`) nothing is touched. `scale`
/// must already be `sanitize_scale`d.
pub(crate) fn build_tree_update(
    nodes: Vec<(NodeId, Node)>,
    root: NodeId,
    focus: NodeId,
    scale: f64,
) -> TreeUpdate {
    frust_shell_common::tree_update_from_parts(nodes, root, focus, scale)
}

/// Whether an opaque handle from the Swift side is the null sentinel.
///
/// The generated Swift app initialises its handle to `nil`/`null` and every call
/// after `frust_init` fails is guarded on it, so a null handle means "no
/// native side yet / already destroyed" and must be treated as a no-op rather
/// than dereferenced.
#[inline]
pub(crate) fn handle_is_null(handle: *mut c_void) -> bool {
    handle.is_null()
}

/// A pointer gesture phase, decoupled from `frust_core::PointerPhase` so this
/// module stays host-testable (the core crate is iOS-gated — see the crate's
/// `Cargo.toml`). [`crate::app`] maps this onto the core phase at the one
/// iOS-only call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchPhase {
    /// A touch began.
    Began,
    /// A touch moved while down.
    Moved,
    /// A touch ended.
    Ended,
    /// The touch was cancelled by the system.
    Cancelled,
}

/// Map the phase code the Swift `FrustView` touch overrides send into a
/// [`TouchPhase`].
///
/// The Swift side sends a fixed ABI — `touchesBegan` → `0`, `touchesMoved` → `1`,
/// `touchesEnded` → `2`, `touchesCancelled` → `3` — so this side never sees a
/// UIKit `UITouch.Phase`. Any unrecognised code (a future phase we don't map, or
/// a corrupt value) is treated as [`TouchPhase::Cancelled`]: the safe default,
/// since it releases any in-flight capture rather than stranding a gesture as
/// perpetually "began".
#[inline]
pub(crate) fn touch_phase_from_code(phase: u32) -> TouchPhase {
    match phase {
        0 => TouchPhase::Began,
        1 => TouchPhase::Moved,
        2 => TouchPhase::Ended,
        // 3 is the explicit `cancelled`; everything else falls back to it.
        _ => TouchPhase::Cancelled,
    }
}

/// A light/dark appearance flag, decoupled from `frust_theme::Brightness` so
/// this module stays host-testable (that crate is iOS-gated — see the crate's
/// `Cargo.toml`). [`crate::app`] maps this onto `Brightness` at the one
/// iOS-only call site ([`crate::app::IosAppHandle::set_appearance`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Appearance {
    /// The platform reports a light appearance preference.
    Light,
    /// The platform reports a dark appearance preference.
    Dark,
}

/// Map the `frust_set_appearance` C-ABI flag (Swift's `traitCollection.
/// userInterfaceStyle == .dark`) into an [`Appearance`] — the pure core of the
/// appearance state-machine transition.
#[inline]
pub(crate) fn appearance_from_dark(dark: bool) -> Appearance {
    if dark {
        Appearance::Dark
    } else {
        Appearance::Light
    }
}

/// Whether a frame should run its rebuild → layout → paint → render pass.
///
/// A frame does work only when the surface is `SurfaceReady` *and* the app is not
/// paused. The paused gate is the enforcement point for the iOS rule that Metal
/// command submission from a backgrounded app can get the process killed
/// (belt-and-suspenders with the Swift side pausing its `CADisplayLink`); the
/// ready gate mirrors the desktop/Android shells' "no rendering outside
/// SurfaceReady" rule.
#[inline]
pub(crate) fn should_render_frame(surface_ready: bool, paused: bool) -> bool {
    surface_ready && !paused
}

/// Cap on consecutive failed surface-recreate attempts per `SurfaceLost` episode.
///
/// `render_frame` fires at CADisplayLink cadence (60–120 Hz); without a cap, a
/// persistently-failing recreate would retry blocking GPU work plus an ERROR log
/// every frame, forever — the same failure shape the generated Swift side latches
/// against for a failed `frust_init`. Three attempts is enough to ride out a
/// transient loss; after that the shell degrades to a single logged failure and
/// stops (a successful recreate resets the counter).
pub(crate) const MAX_RECREATE_ATTEMPTS: u8 = 3;

/// Whether a lost surface should be recreated before the next frame/resize.
///
/// Unlike Android (which pairs surface loss with a `surfaceDestroyed`/
/// `surfaceCreated` window cycle), iOS keeps the same `CAMetalLayer` for the app's
/// whole lifetime, so nothing external re-drives surface creation after a
/// `SurfaceLost`. The shell therefore self-heals: on the next `frust_resize`
/// or `frust_render_frame` it recreates the surface from the retained layer.
/// This predicate is that trigger, and it is deliberately narrow:
///
/// - only in [`SurfacePhase::SurfaceLost`] (a `NoSurface` handle never exists
///   post-init, and `SurfaceReady` needs no recovery);
/// - never while `paused` — recreation is real Metal work, and the paused gate
///   exists precisely because GPU submission from a backgrounded iOS app can get
///   the process killed. A loss that coincides with backgrounding recovers on the
///   first frame/resize after `frust_resume`;
/// - only while under [`MAX_RECREATE_ATTEMPTS`] consecutive failures, so a
///   persistently-failing recreate cannot become a per-frame retry storm.
#[inline]
pub(crate) fn should_recreate_surface(
    phase: SurfacePhase,
    paused: bool,
    failed_attempts: u8,
) -> bool {
    matches!(phase, SurfacePhase::SurfaceLost) && !paused && failed_attempts < MAX_RECREATE_ATTEMPTS
}

/// `frust_render_frame`'s "keep driving frames" return code.
pub(crate) const FRAME_ALIVE: u8 = 1;

/// `frust_render_frame`'s "fatal — stop the CADisplayLink" return code.
pub(crate) const FRAME_FATAL: u8 = 0;

/// Map the render thread's fatal-flag state onto `frust_render_frame`'s `u8`
/// return: [`FRAME_FATAL`] (`0`) when a fatal first-surface install failure was
/// signalled, else [`FRAME_ALIVE`] (`1`).
///
/// A first-surface install failure is unrecoverable (an incapable GPU/driver
/// can't change mid-process), so the Swift side treats a `0` return exactly like
/// a failed `frust_init` — latching `initFailed` and invalidating its
/// `CADisplayLink`. The return is a plain `uint8_t` (not C `_Bool`), matching
/// this crate's `frust_set_appearance` convention of not pulling in `<stdbool.h>`
/// at the bridging header.
#[inline]
pub(crate) fn frame_liveness_signal(fatal: bool) -> u8 {
    if fatal { FRAME_FATAL } else { FRAME_ALIVE }
}

/// The focused field's caret rectangle in **logical** pixels (view-local), the
/// geometry the Swift `UITextInput` bridge approximates `caretRect(for:)` /
/// `firstRect(for:)` from. Logical points equal view points on iOS (touch
/// coordinates cross the FFI as `touch.location(in:)`, already logical), so the
/// Swift side consumes these without any scale conversion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CaretRect {
    /// Left edge (logical px).
    pub x: f32,
    /// Top edge (logical px).
    pub y: f32,
    /// Width (logical px).
    pub w: f32,
    /// Height (logical px).
    pub h: f32,
}

/// Serialize the focused field's IME surface into the Flutter-canonical JSON the
/// Swift `UITextInput` bridge consumes (the same shape the Android bridge's
/// `nativeImeState` emits):
///
/// ```text
/// {"active":bool,"text":"...","selBase":n,"selExt":n,"compBase":n,"compExt":n,
///  "caretX":f,"caretY":f,"caretW":f,"caretH":f,"contentType":"normal",
///  "suppressSoftKeyboard":bool}
/// ```
///
/// Selection/composing indices are UTF-16 code units (the platform-native unit,
/// passed opaquely through the `EditingState` shell seam — see
/// `frust_core::event::EditingState`); `-1` denotes "none". The caret is
/// logical-pixel; an absent (or non-finite) caret serializes the four caret
/// fields as `null`. JSON is hand-rolled (no `serde` in the shell) with a tiny
/// escaper — see [`json_escape_into`].
///
/// `content_type` is a **stable, explicitly-encoded wire string** — one of
/// `"normal"` / `"password"` / `"noSuggestions"` — never a `Debug` rendering
/// of `frust_core::event::ImeContentType` (that type's `Debug` impl exists on
/// `ImeState`, not here, but the discipline is the same one that motivated
/// it: a secret field's classification must never ride on a format that could
/// silently change shape). The caller ([`crate::ffi_glue::ime_state_to_json`])
/// maps the enum to this string; unrecognized/future strings on the Swift side
/// must fail closed to the most restrictive (`"password"`-equivalent)
/// handling, never fall through to `"normal"`.
///
/// `suppress_soft_keyboard` is appended **last**, so every field before it
/// keeps the position it already had and the Android bridge — which spells
/// the key `"suppressSoftKeyboard"` and places it in the same slot — stays a
/// byte-for-byte mirror (the claim both `content_type_wire` docs make). It
/// defaults to `false`, so a field publishing no hint reads exactly as it
/// always did. Set, it asks the Swift side to keep the first responder — the
/// route `copy:` and the system edit menu travel — while handing UIKit an
/// empty `inputView`, so a field whose text cannot be typed into raises no
/// on-screen keyboard.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ime_state_json(
    active: bool,
    text: &str,
    sel_base: i32,
    sel_ext: i32,
    comp_base: i32,
    comp_ext: i32,
    caret: Option<CaretRect>,
    content_type: &str,
    suppress_soft_keyboard: bool,
) -> String {
    let mut out = String::with_capacity(text.len() + 128);
    out.push_str("{\"active\":");
    out.push_str(if active { "true" } else { "false" });
    out.push_str(",\"text\":\"");
    json_escape_into(text, &mut out);
    out.push_str("\",\"selBase\":");
    let _ = write!(out, "{sel_base}");
    out.push_str(",\"selExt\":");
    let _ = write!(out, "{sel_ext}");
    out.push_str(",\"compBase\":");
    let _ = write!(out, "{comp_base}");
    out.push_str(",\"compExt\":");
    let _ = write!(out, "{comp_ext}");
    let (cx, cy, cw, ch) = match caret {
        Some(c) => (Some(c.x), Some(c.y), Some(c.w), Some(c.h)),
        None => (None, None, None, None),
    };
    out.push_str(",\"caretX\":");
    push_num_or_null(&mut out, cx);
    out.push_str(",\"caretY\":");
    push_num_or_null(&mut out, cy);
    out.push_str(",\"caretW\":");
    push_num_or_null(&mut out, cw);
    out.push_str(",\"caretH\":");
    push_num_or_null(&mut out, ch);
    out.push_str(",\"contentType\":\"");
    json_escape_into(content_type, &mut out);
    out.push_str("\",\"suppressSoftKeyboard\":");
    out.push_str(if suppress_soft_keyboard {
        "true"
    } else {
        "false"
    });
    out.push('}');
    out
}

/// Append `v` as a JSON number, or `null` when absent or non-finite (a NaN/inf
/// caret must never produce invalid JSON).
fn push_num_or_null(out: &mut String, v: Option<f32>) {
    match v {
        Some(f) if f.is_finite() => {
            let _ = write!(out, "{f}");
        }
        _ => out.push_str("null"),
    }
}

/// Append `s` to `out` with JSON string escaping (RFC 8259): the two mandatory
/// escapes (`"`, `\`), the short escapes for the common control characters, and
/// `\u00XX` for any other C0 control. Non-ASCII scalars pass through as UTF-8
/// (valid inside a JSON string), so CJK / emoji text is emitted verbatim.
pub(crate) fn json_escape_into(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

/// One clipboard/selection verb the host's edit menu (or a hardware-keyboard
/// chord routed through UIKit's responder chain) asked for, decoupled from
/// `frust_core::event::EditCommand` so this module stays host-testable (the
/// core crate is iOS-gated — see the crate's `Cargo.toml`, the same reasoning
/// [`TouchPhase`]/[`Appearance`] above follow).
///
/// Carries no payload: only a paste has one, and the wire passes that text as
/// `frust_edit_command`'s separate C-string argument rather than widening this
/// enum — which keeps the mapping a pure `u8` decision this module can test
/// without a string allocation. [`crate::ffi_glue::edit_command`] pairs the two
/// back together into the core `EditCommand` at the one iOS-only call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditCommandKind {
    /// Copy the selection to the host pasteboard.
    Copy,
    /// Copy the selection to the host pasteboard and delete it.
    Cut,
    /// Replace the selection with the text the host read for us.
    Paste,
    /// Select the focused field's whole content.
    SelectAll,
}

/// `frust_edit_command`'s wire code for [`EditCommandKind::Copy`].
///
/// **DO NOT renumber.** These four constants are a fixed numeric ABI shared
/// with the Swift `FrustView` edit actions (`copy(_:)`/`cut(_:)`/`paste(_:)`/
/// `selectAll(_:)`), exactly like the touch-phase codes
/// [`touch_phase_from_code`] decodes: the Swift side compiles the literal into
/// the app binary, so changing a value here silently re-points a shipped
/// caller's verb at a different one.
pub(crate) const EDIT_COMMAND_COPY: u8 = 0;
/// `frust_edit_command`'s wire code for [`EditCommandKind::Cut`] — see
/// [`EDIT_COMMAND_COPY`] for the do-not-renumber rule.
pub(crate) const EDIT_COMMAND_CUT: u8 = 1;
/// `frust_edit_command`'s wire code for [`EditCommandKind::Paste`] — see
/// [`EDIT_COMMAND_COPY`] for the do-not-renumber rule.
pub(crate) const EDIT_COMMAND_PASTE: u8 = 2;
/// `frust_edit_command`'s wire code for [`EditCommandKind::SelectAll`] — see
/// [`EDIT_COMMAND_COPY`] for the do-not-renumber rule.
pub(crate) const EDIT_COMMAND_SELECT_ALL: u8 = 3;

/// Map `frust_edit_command`'s `cmd` code onto an [`EditCommandKind`], or `None`
/// for a code this ABI does not define.
///
/// **Unrecognised codes are dropped, not defaulted** — the one place this
/// module deliberately parts company with [`touch_phase_from_code`], which
/// folds an unknown code into `Cancelled`. A touch phase has a safe default
/// (cancelling releases capture, which is strictly the *less* destructive
/// reading); a clipboard verb has none. Every variant above either mutates the
/// document or discloses its contents, so a corrupt code guessed as `Cut` would
/// delete a selection and one guessed as `Paste` would overwrite it. Refusing
/// is the only answer that cannot damage the user's text.
#[inline]
pub(crate) fn edit_command_from_code(cmd: u8) -> Option<EditCommandKind> {
    match cmd {
        EDIT_COMMAND_COPY => Some(EditCommandKind::Copy),
        EDIT_COMMAND_CUT => Some(EditCommandKind::Cut),
        EDIT_COMMAND_PASTE => Some(EditCommandKind::Paste),
        EDIT_COMMAND_SELECT_ALL => Some(EditCommandKind::SelectAll),
        _ => None,
    }
}

/// The selection's bounding rect in **logical** points, the anchor the Swift
/// side presents the system edit menu against.
///
/// Decoupled from `kurbo::Rect` for [`CaretRect`]'s reason (`kurbo` is an
/// iOS-gated dependency of this crate) and carried in the same unit as that
/// caret rect — logical px equal view points on iOS — so the Swift side
/// consumes both with no scale conversion. The core publishes the anchor in
/// **absolute** logical window space, and `FrustView` fills the window, so the
/// rect is already in the view's own coordinates when it arrives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ToolbarAnchor {
    /// Left edge (logical points).
    pub x: f32,
    /// Top edge (logical points).
    pub y: f32,
    /// Width (logical points).
    pub w: f32,
    /// Height (logical points).
    pub h: f32,
}

/// Which clipboard verbs the focused field says apply to its current selection,
/// decoupled from `frust_core::selection_toolbar::SelectionToolbarActions` for
/// [`ToolbarAnchor`]'s reason.
///
/// The **field's** call, not the menu's: an obscured field refuses copy and cut,
/// a read-only one refuses cut and paste. The Swift side answers
/// `canPerformAction(_:with:)` from these, which is what makes UIKit's own menu
/// offer exactly the verbs the framework would have offered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ToolbarActions {
    /// Copy the selection.
    pub copy: bool,
    /// Cut the selection.
    pub cut: bool,
    /// Replace the selection with the pasteboard's contents.
    pub paste: bool,
    /// Select the field's whole content.
    pub select_all: bool,
}

/// Serialize the focused field's selection-toolbar request into the JSON
/// `frust_selection_toolbar_json` returns:
///
/// ```text
/// {"generation":7,"presentMenu":true,"x":12,"y":40,"w":86,"h":18,
///  "copy":true,"cut":true,"paste":true,"selectAll":false}
/// ```
///
/// `generation` is `AppTree::selection_toolbar_generation`'s counter, which
/// moves when the *menu-significant* part of the request changes — `presentMenu`
/// or the verbs, never the anchor alone; the Swift side caches the value it last
/// acted on and presents or dismisses only when it moves, which is what stops a
/// standing selection — republished every frame it stands — from re-presenting
/// the system menu on every vsync.
///
/// `presentMenu` is the request's one edge: `true` means the field is asking for
/// the system menu now, `false` means it is only reporting where it is and what
/// applies. The four geometry fields are logical points (see [`ToolbarAnchor`]),
/// the four verb fields are the field's own enabled set (see [`ToolbarActions`])
/// and are a level the responder chain answers `canPerformAction` from whether
/// or not any menu is up.
///
/// Total, unlike [`platform_view_commands_json`]: the "no field is focused"
/// answer is the FFI's null return, not a shape this builder has to express, so
/// a caller with a request always gets a string. JSON is hand-rolled (no `serde`
/// in the shell, `docs/CODE_STANDARDS.md`) beside [`ime_state_json`].
pub(crate) fn selection_toolbar_json(
    generation: u64,
    present_menu: bool,
    anchor: ToolbarAnchor,
    actions: ToolbarActions,
) -> String {
    let mut out = String::with_capacity(148);
    out.push_str("{\"generation\":");
    let _ = write!(out, "{generation}");
    out.push_str(",\"presentMenu\":");
    push_bool(&mut out, present_menu);
    out.push_str(",\"x\":");
    push_finite_f32(&mut out, anchor.x);
    out.push_str(",\"y\":");
    push_finite_f32(&mut out, anchor.y);
    out.push_str(",\"w\":");
    push_finite_f32(&mut out, anchor.w);
    out.push_str(",\"h\":");
    push_finite_f32(&mut out, anchor.h);
    out.push_str(",\"copy\":");
    push_bool(&mut out, actions.copy);
    out.push_str(",\"cut\":");
    push_bool(&mut out, actions.cut);
    out.push_str(",\"paste\":");
    push_bool(&mut out, actions.paste);
    out.push_str(",\"selectAll\":");
    push_bool(&mut out, actions.select_all);
    out.push('}');
    out
}

/// Append `v` as a JSON number, substituting a finite `0` for a non-finite one.
///
/// The `f32` twin of [`push_finite`], and it degrades the same way for the same
/// reason: the anchor is a *required* field (unlike [`ime_state_json`]'s
/// optional caret, which nulls), so a `NaN`/`±inf` component must not emit an
/// unparseable token and cost the Swift side the whole menu. A menu anchored at
/// the view's origin is a visible glitch; a batch that fails to parse is a
/// silently missing system menu with nothing to see.
fn push_finite_f32(out: &mut String, v: f32) {
    if v.is_finite() {
        let _ = write!(out, "{v}");
    } else {
        out.push('0');
    }
}

/// Append `v` as a JSON boolean literal.
fn push_bool(out: &mut String, v: bool) {
    out.push_str(if v { "true" } else { "false" });
}

/// A logical-or-physical rect, decoupled from `kurbo::Rect` so this module
/// stays host-testable (`kurbo` is an iOS-gated dependency in this crate —
/// see the crate's `Cargo.toml`, mirroring [`CaretRect`] above). The caller
/// ([`crate::app::IosAppHandle`]) has already converted logical px into
/// **physical** px (`* scale`) before building one of these — the
/// physical-at-FFI/logical-inside rule, applied outbound (matches
/// `frust-shell-android`'s `nativePlatformViewCommands` JSON).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PvRect {
    /// Left edge (physical px).
    pub x: f64,
    /// Top edge (physical px).
    pub y: f64,
    /// Width (physical px).
    pub w: f64,
    /// Height (physical px).
    pub h: f64,
}

/// One platform-view compositor command, decoupled from
/// `frust_shell_common::ViewCommand` so this module stays host-testable
/// (`frust-shell-common` is an iOS-gated dependency — see the crate's
/// `Cargo.toml`, the same reasoning [`TouchPhase`]/[`Appearance`] above
/// follow). [`crate::app::IosAppHandle`] maps the real `ViewCommand` onto
/// this shape at the one iOS-only call site, physical-px-converting each
/// rect/clip in the same pass.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PlatformViewCommand {
    /// Create a new native view for `slot_id`.
    Create {
        slot_id: u64,
        view_type: String,
        params_json: String,
        interactive: bool,
    },
    /// Place/resize/clip/show-or-hide an already-created slot.
    Update {
        slot_id: u64,
        rect: PvRect,
        clip: Option<PvRect>,
        visible: bool,
        shields: Vec<PvRect>,
    },
    /// `params_json` changed with no necessary rect/clip/visible change.
    UpdateParams { slot_id: u64, params_json: String },
    /// Tear down a slot's native view entirely.
    Dispose { slot_id: u64 },
    /// Create or re-place overlay `overlay_id` directly above slot
    /// `above_slot_id`, covering `rect`; the render side draws the frame's
    /// commands `range_start..range_end` onto it (informational for the host,
    /// which pairs nothing — see [`platform_view_commands_json`]).
    Overlay {
        overlay_id: u64,
        above_slot_id: u64,
        rect: PvRect,
        range_start: u64,
        range_end: u64,
    },
    /// Tear overlay `overlay_id` down.
    DisposeOverlay { overlay_id: u64 },
}

/// Serialize the platform-view command backlog into the wire JSON
/// `frust_platform_view_commands_json` returns — byte-identical schema to
/// `frust-shell-android`'s `nativePlatformViewCommands`, so a shared
/// Kotlin/Swift-side parser need not branch on platform:
///
/// ```text
/// {"generation":7,"commands":[
///  {"op":"create","slot":3,"viewType":"dev.frust.XFactory","params":"{...}"},
///  {"op":"update","slot":3,"rect":[x,y,w,h],"clip":[x,y,w,h]|null,"visible":true},
///  {"op":"updateParams","slot":3,"params":"{...}"},
///  {"op":"dispose","slot":3}]}
/// ```
///
/// Two overlay ops extend the vocabulary additively, byte-identical to the
/// Android shell's (a host that does not know them skips them like any unknown
/// op), and are emitted only with the overlay switch on (`FRUST_OVERLAYS`):
///
/// ```text
///  {"op":"overlay","overlay":1,"aboveSlot":3,"rect":[x,y,w,h],"rangeStart":12,"rangeEnd":40}
///  {"op":"disposeOverlay","overlay":1}
/// ```
///
/// `rect`/`clip` are `[x,y,w,h]` arrays — origin then size, already physical
/// px (see [`PvRect`]). Returns `None` when `generation == ack_generation`
/// (nothing new since the caller's last ack) — the shell's cheap no-change
/// fast path, mirroring [`frust_shell_common::system_ui`]'s (crate-external,
/// see [`crate::ffi_glue::system_ui_state`]) null/no-generation-change
/// convention. JSON is hand-rolled (no `serde` in the shell,
/// `docs/CODE_STANDARDS.md`) reusing [`json_escape_into`].
pub(crate) fn platform_view_commands_json(
    generation: u64,
    ack_generation: u64,
    commands: &[PlatformViewCommand],
) -> Option<String> {
    if generation == ack_generation {
        return None;
    }
    let mut out = String::with_capacity(64 + commands.len() * 96);
    out.push_str("{\"generation\":");
    let _ = write!(out, "{generation}");
    out.push_str(",\"commands\":[");
    for (i, cmd) in commands.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_command_json(&mut out, cmd);
    }
    out.push_str("]}");
    Some(out)
}

fn push_command_json(out: &mut String, cmd: &PlatformViewCommand) {
    match cmd {
        PlatformViewCommand::Create {
            slot_id,
            view_type,
            params_json,
            interactive,
        } => {
            out.push_str("{\"op\":\"create\",\"slot\":");
            let _ = write!(out, "{slot_id}");
            out.push_str(",\"viewType\":\"");
            json_escape_into(view_type, out);
            out.push_str("\",\"params\":\"");
            json_escape_into(params_json, out);
            out.push_str("\",\"interactive\":");
            out.push_str(if *interactive { "true" } else { "false" });
            out.push('}');
        }
        PlatformViewCommand::Update {
            slot_id,
            rect,
            clip,
            visible,
            shields,
        } => {
            out.push_str("{\"op\":\"update\",\"slot\":");
            let _ = write!(out, "{slot_id}");
            out.push_str(",\"rect\":");
            push_rect_json(out, *rect);
            out.push_str(",\"clip\":");
            match clip {
                Some(c) => push_rect_json(out, *c),
                None => out.push_str("null"),
            }
            out.push_str(",\"visible\":");
            out.push_str(if *visible { "true" } else { "false" });
            out.push_str(",\"shields\":[");
            for (i, s) in shields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                push_rect_json(out, *s);
            }
            out.push_str("]}");
        }
        PlatformViewCommand::UpdateParams {
            slot_id,
            params_json,
        } => {
            out.push_str("{\"op\":\"updateParams\",\"slot\":");
            let _ = write!(out, "{slot_id}");
            out.push_str(",\"params\":\"");
            json_escape_into(params_json, out);
            out.push_str("\"}");
        }
        PlatformViewCommand::Dispose { slot_id } => {
            out.push_str("{\"op\":\"dispose\",\"slot\":");
            let _ = write!(out, "{slot_id}");
            out.push('}');
        }
        PlatformViewCommand::Overlay {
            overlay_id,
            above_slot_id,
            rect,
            range_start,
            range_end,
        } => {
            let _ = write!(
                out,
                "{{\"op\":\"overlay\",\"overlay\":{overlay_id},\"aboveSlot\":{above_slot_id},\"rect\":"
            );
            push_rect_json(out, *rect);
            let _ = write!(
                out,
                ",\"rangeStart\":{range_start},\"rangeEnd\":{range_end}}}"
            );
        }
        PlatformViewCommand::DisposeOverlay { overlay_id } => {
            let _ = write!(
                out,
                "{{\"op\":\"disposeOverlay\",\"overlay\":{overlay_id}}}"
            );
        }
    }
}

/// Append one [`PvRect`] as the wire array `[x,y,w,h]`, **sanitizing any
/// non-finite component to `0`** — `NaN`/`±inf` have no JSON representation
/// and would emit an unparseable token, breaking the whole batch for the Swift
/// host. Parity with `frust-shell-android`'s `platform_view_rect_json`
/// and with [`push_num_or_null`]'s caret guard above.
fn push_rect_json(out: &mut String, rect: PvRect) {
    out.push('[');
    push_finite(out, rect.x);
    out.push(',');
    push_finite(out, rect.y);
    out.push(',');
    push_finite(out, rect.w);
    out.push(',');
    push_finite(out, rect.h);
    out.push(']');
}

/// Append `v` as a JSON number, substituting a finite `0` for a non-finite
/// value (see [`push_rect_json`]). A rect is a required field (unlike the
/// optional caret), so it degrades to `0` rather than `null`, matching
/// Android's `json_number`.
fn push_finite(out: &mut String, v: f64) {
    if v.is_finite() {
        let _ = write!(out, "{v}");
    } else {
        out.push('0');
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
/// `peniko` is an iOS-gated dependency of this crate, and this module compiles
/// on every host (see the crate docs' Layering note).
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
// Overlay surfaces: the host-testable halves of `crate::ffi_glue`'s overlay
// host around the shared `frust_shell_common::overlay` core — the layer
// hand-back Swift drains, the fold of the differ's overlay ranges and the
// present parking slot. Generic over the layer, surface and present types, so
// their ordering contracts are tested here with no GPU and no UIKit.
// ---------------------------------------------------------------------

pub(crate) use frust_shell_common::overlay::OverlayCutSpec;
use frust_shell_common::overlay::{FrameTake, Landed, OverlayTable, Withdraw};

/// The shared [`OverlayTable`] plus what the iOS host keeps beside it: the
/// layers whose surface is gone, queued for Swift to release their views
/// (`frust_overlay_take_released`), and the flying overlays whose surface the
/// next land drops. Their parked present must be dropped before that land: a
/// `wgpu::SurfaceTexture` must not outlive its surface.
#[derive(Debug)]
pub(crate) struct OverlayHost<L, S> {
    table: OverlayTable<L, S>,
    /// Each attached overlay's layer, to tell a resize from a replacement.
    layers: std::collections::BTreeMap<u64, L>,
    /// Overlays taken out for the frame in progress.
    flying: std::collections::BTreeSet<u64>,
    /// Flying overlays withdrawn or given a new layer.
    retiring: std::collections::BTreeSet<u64>,
    /// Layers handed back, oldest first.
    released: std::collections::VecDeque<L>,
}

impl<L, S> Default for OverlayHost<L, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<L, S> OverlayHost<L, S> {
    /// An empty host (`const`, for a process-global).
    pub(crate) const fn new() -> Self {
        Self {
            table: OverlayTable::new(),
            layers: std::collections::BTreeMap::new(),
            flying: std::collections::BTreeSet::new(),
            retiring: std::collections::BTreeSet::new(),
            released: std::collections::VecDeque::new(),
        }
    }

    /// Forget every overlay and every queued release (teardown: Swift drops
    /// every view with the handle). The table drops each surface first.
    pub(crate) fn clear(&mut self) {
        *self = Self::new();
    }

    /// The main surface is going away: see [`OverlayTable::drop_surfaces`].
    pub(crate) fn drop_surfaces(&mut self) -> usize {
        self.table.drop_surfaces()
    }

    /// See [`OverlayTable::note_error`].
    pub(crate) fn note_error(&mut self, id: u64, log_limit: u32) -> Option<u32> {
        self.table.note_error(id, log_limit)
    }

    /// See [`OverlayTable::note_unservable`].
    pub(crate) fn note_unservable(&mut self, id: u64) -> bool {
        self.table.note_unservable(id)
    }

    /// Whether no overlay is tracked.
    pub(crate) fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// Ahead of a frame: see [`OverlayTable::take_for_frame`].
    pub(crate) fn take_for_frame<E>(
        &mut self,
        build: impl FnMut(&L, (u32, u32)) -> Result<S, E>,
        resize: impl FnMut(&mut S, (u32, u32)),
    ) -> FrameTake<S, E> {
        let take = self.table.take_for_frame(build, resize);
        self.flying.extend(take.0.iter().map(|(id, _)| *id));
        take
    }

    /// The overlays among `flying` whose surface the coming [`Self::land`]
    /// drops — the caller drops their parked presents first.
    pub(crate) fn retiring_among(&self, flying: &[(u64, S)]) -> Vec<u64> {
        flying
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| self.retiring.contains(id))
            .collect()
    }

    /// Hand the frame's surfaces back; the layers that frees are queued for
    /// release. Answers how many surfaces came back for an unknown overlay.
    pub(crate) fn land(&mut self, flying: Vec<(u64, S)>) -> usize {
        for (id, _) in &flying {
            self.flying.remove(id);
            self.retiring.remove(id);
        }
        let Landed {
            released_layers,
            orphaned,
        } = self.table.land(flying);
        self.released
            .extend(released_layers.into_iter().map(|(_, layer)| layer));
        orphaned
    }

    /// Move up to `out.len()` released layers into `out`, oldest first;
    /// answers how many.
    pub(crate) fn take_released(&mut self, out: &mut [L]) -> usize {
        let count = out.len().min(self.released.len());
        for (slot, layer) in out.iter_mut().zip(self.released.drain(..count)) {
            *slot = layer;
        }
        count
    }
}

impl<L: Copy + PartialEq, S> OverlayHost<L, S> {
    /// Overlay `id`'s attached layer, if any.
    pub(crate) fn layer(&self, id: u64) -> Option<L> {
        self.layers.get(&id).copied()
    }

    /// Overlay `id`'s view delivered `layer` at `size` physical px; answers
    /// whether a frame is owed. A replaced layer is queued for release once
    /// its surface is gone — at once, or by the land of the frame drawing it.
    pub(crate) fn attach(&mut self, id: u64, layer: L, size: (u32, u32)) -> bool {
        if let Some(old) = self.layers.insert(id, layer).filter(|old| *old != layer) {
            // Only the first replacement mid-frame owns the flying surface;
            // a later one never got a surface and is free at once.
            if !(self.flying.contains(&id) && self.retiring.insert(id)) {
                self.released.push_back(old);
            }
        }
        self.table.publish(id, layer, size)
    }

    /// Overlay `id`'s view is going away. Never blocks: a layer whose
    /// surface is flying is queued by that frame's land instead of now.
    pub(crate) fn detach(&mut self, id: u64) -> Withdraw<L> {
        self.layers.remove(&id);
        let outcome = self.table.withdraw(id);
        match outcome {
            Withdraw::Released(layer) => self.released.push_back(layer),
            Withdraw::Deferred => {
                self.retiring.insert(id);
            }
            Withdraw::Unknown => {}
        }
        outcome
    }
}

/// The overlays' current command ranges, folded on the UI thread from the
/// differ's `Overlay`/`DisposeOverlay` commands as they are pushed — the
/// differ announces a range only when it changes, so each painted frame
/// snapshots the fold ([`Self::cuts`]) and carries it with its scene to the
/// render thread, which therefore never pairs one frame's ranges with another
/// frame's commands.
#[derive(Debug, Default)]
pub(crate) struct OverlayRanges {
    ranges: std::collections::BTreeMap<u64, std::ops::Range<usize>>,
    folded_generation: u64,
}

impl OverlayRanges {
    /// An empty fold (`const`, for a process-global).
    pub(crate) const fn new() -> Self {
        Self {
            ranges: std::collections::BTreeMap::new(),
            folded_generation: 0,
        }
    }

    /// Forget everything — a new differ starts its generations and overlay
    /// ids over.
    pub(crate) fn reset(&mut self) {
        self.ranges.clear();
        self.folded_generation = 0;
    }

    /// Start folding the differ's backlog at its current `generation`:
    /// `Some(g)` — fold every command pushed after generation `g` (the
    /// backlog's `commands_up_to(g)` prefix is already folded), then call
    /// [`Self::finish_fold`] — or `None` when nothing new was pushed. A
    /// generation older than the folded one means a new differ, and restarts
    /// the fold.
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

    /// The ranges as they stand, ordered by range (then id) — what the frame
    /// painted just now draws.
    pub(crate) fn cuts(&self) -> Vec<OverlayCutSpec> {
        let mut cuts: Vec<OverlayCutSpec> = self
            .ranges
            .iter()
            .map(|(&id, range)| (id, range.clone()))
            .collect();
        cuts.sort_by_key(|(id, range)| (range.start, range.end, *id));
        cuts
    }
}

/// The present parking slot behind present-sync: at most one submitted frame
/// (its main present and every overlay's) waiting for the UI thread to present
/// inside its `CATransaction` — `crate::app::PresentHandoff`'s storage, generic
/// over the parked value so its discipline is host-tested.
///
/// The discipline is the fix for a swapchain held across an acquire or a
/// configure (wgpu reports both as validation errors): the render thread
/// **waits for the slot to drain** ([`Self::wait_drained`]) before it acquires
/// or reconfigures anything, dropping a leftover only once a bounded wait has
/// expired; the UI thread **presents while holding the slot**
/// ([`Self::take_with`]), so a frame it took can never still be in its hands
/// when the render thread next touches a swapchain; and every lifecycle door
/// clears it ([`Self::clear`]).
#[derive(Debug, Default)]
pub(crate) struct PresentSlot<T> {
    slot: std::sync::Mutex<Option<T>>,
    drained: std::sync::Condvar,
}

impl<T> PresentSlot<T> {
    /// An empty slot.
    pub(crate) fn new() -> Self {
        Self {
            slot: std::sync::Mutex::new(None),
            drained: std::sync::Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<T>> {
        // Poisoning needs a panic while the slot is held — a take/store of an
        // `Option` or a present — which leaves no invariant to corrupt.
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Park `value` (render side), answering the value it displaced — a frame
    /// the UI thread never presented, which the caller drops. Empty after a
    /// [`Self::wait_drained`], so `None` in practice.
    pub(crate) fn store(&self, value: T) -> Option<T> {
        self.lock().replace(value)
    }

    /// Take the parked value and consume it with `present` **while still
    /// holding the slot** (UI side), then wake a render thread waiting for
    /// the slot to drain. `None` when nothing was parked.
    pub(crate) fn take_with<R>(&self, present: impl FnOnce(T) -> R) -> Option<R> {
        let mut slot = self.lock();
        let answer = slot.take().map(present);
        drop(slot);
        self.drained.notify_all();
        answer
    }

    /// Wait up to `timeout` for the UI thread to take the parked value
    /// (render side, before an acquire or a configure), then answer whatever
    /// is still parked — taken out of the slot, for the caller to drop: a
    /// frame the UI thread did not present in time. `None` when the slot was
    /// or became empty.
    pub(crate) fn wait_drained(&self, timeout: std::time::Duration) -> Option<T> {
        let slot = self.lock();
        let (mut slot, _) = self
            .drained
            .wait_timeout_while(slot, timeout, |parked| parked.is_some())
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.take()
    }

    /// Take whatever is parked without presenting it (a lifecycle door), and
    /// wake a waiter.
    pub(crate) fn clear(&self) -> Option<T> {
        let taken = self.lock().take();
        self.drained.notify_all();
        taken
    }

    /// Edit the parked value in place (dropping one overlay's present).
    pub(crate) fn with_parked(&self, edit: impl FnOnce(&mut T)) {
        if let Some(parked) = self.lock().as_mut() {
            edit(parked);
        }
    }

    /// Whether a value is parked.
    #[cfg(test)]
    pub(crate) fn is_parked(&self) -> bool {
        self.lock().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_handle_is_null() {
        assert!(handle_is_null(std::ptr::null_mut()));
    }

    #[test]
    fn nonnull_handle_is_not_null() {
        // The address of a real local is enough — the pointer is never
        // dereferenced here, only tested against the null sentinel.
        let mut local = 0u8;
        let ptr = (&mut local as *mut u8).cast::<c_void>();
        assert!(!handle_is_null(ptr));
    }

    #[test]
    fn touch_phase_codes_map_to_phases() {
        assert_eq!(touch_phase_from_code(0), TouchPhase::Began);
        assert_eq!(touch_phase_from_code(1), TouchPhase::Moved);
        assert_eq!(touch_phase_from_code(2), TouchPhase::Ended);
        assert_eq!(touch_phase_from_code(3), TouchPhase::Cancelled);
    }

    #[test]
    fn unknown_touch_phase_falls_back_to_cancelled() {
        // A future/corrupt code must release capture, not strand a "began".
        assert_eq!(touch_phase_from_code(4), TouchPhase::Cancelled);
        assert_eq!(touch_phase_from_code(u32::MAX), TouchPhase::Cancelled);
    }

    #[test]
    fn appearance_from_dark_maps_true_to_dark() {
        // init light -> set dark -> the transition Rust's brightness flip applies.
        assert_eq!(appearance_from_dark(false), Appearance::Light);
        assert_eq!(appearance_from_dark(true), Appearance::Dark);
    }

    #[test]
    fn frame_runs_only_when_ready_and_not_paused() {
        assert!(should_render_frame(true, false));
    }

    #[test]
    fn paused_frame_is_a_noop_even_when_ready() {
        // The paused gate is the process-kill guard: a paused app must never
        // submit a frame, regardless of surface readiness.
        assert!(!should_render_frame(true, true));
    }

    #[test]
    fn frame_is_a_noop_when_surface_not_ready() {
        assert!(!should_render_frame(false, false));
        assert!(!should_render_frame(false, true));
    }

    #[test]
    fn frame_liveness_signal_encodes_fatal_as_zero() {
        // 0 = fatal (Swift latches initFailed + invalidates the CADisplayLink);
        // 1 = keep driving frames. The non-obvious 0-means-stop encoding is why
        // this mapping is a named, tested helper rather than an inline literal.
        assert_eq!(frame_liveness_signal(true), FRAME_FATAL);
        assert_eq!(frame_liveness_signal(true), 0);
        assert_eq!(frame_liveness_signal(false), FRAME_ALIVE);
        assert_eq!(frame_liveness_signal(false), 1);
    }

    #[test]
    fn lost_surface_is_recreated() {
        // The self-recovery trigger: `SurfaceLost`, unpaused, under the cap.
        assert!(should_recreate_surface(SurfacePhase::SurfaceLost, false, 0));
        assert!(should_recreate_surface(
            SurfacePhase::SurfaceLost,
            false,
            MAX_RECREATE_ATTEMPTS - 1
        ));
    }

    #[test]
    fn ready_and_nosurface_are_not_recreated() {
        assert!(!should_recreate_surface(
            SurfacePhase::SurfaceReady,
            false,
            0
        ));
        assert!(!should_recreate_surface(SurfacePhase::NoSurface, false, 0));
    }

    #[test]
    fn paused_app_never_recreates() {
        // The paused gate is the process-kill guard; recreation is GPU work and
        // must wait for resume, exactly like frame submission.
        assert!(!should_recreate_surface(SurfacePhase::SurfaceLost, true, 0));
    }

    #[test]
    fn recreate_stops_at_the_attempt_cap() {
        assert!(!should_recreate_surface(
            SurfacePhase::SurfaceLost,
            false,
            MAX_RECREATE_ATTEMPTS
        ));
        assert!(!should_recreate_surface(
            SurfacePhase::SurfaceLost,
            false,
            u8::MAX
        ));
    }

    fn escape(s: &str) -> String {
        let mut out = String::new();
        json_escape_into(s, &mut out);
        out
    }

    #[test]
    fn json_escaper_handles_quotes_and_backslashes() {
        assert_eq!(escape(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    #[test]
    fn json_escaper_handles_control_characters() {
        assert_eq!(escape("a\nb\tc\rd"), "a\\nb\\tc\\rd");
        // A bare NUL / other C0 control becomes a \u00XX escape.
        assert_eq!(escape("\u{00}\u{1f}"), "\\u0000\\u001f");
        assert_eq!(escape("\u{08}\u{0c}"), "\\b\\f");
    }

    #[test]
    fn json_escaper_passes_unicode_through_as_utf8() {
        // CJK + emoji (astral) are valid inside a JSON string verbatim.
        assert_eq!(escape("日本語😀"), "日本語😀");
    }

    #[test]
    fn ime_json_with_caret_is_well_formed() {
        let json = ime_state_json(
            true,
            "hi",
            0,
            2,
            -1,
            -1,
            Some(CaretRect {
                x: 4.0,
                y: 8.0,
                w: 2.0,
                h: 16.0,
            }),
            "normal",
            false,
        );
        assert_eq!(
            json,
            r#"{"active":true,"text":"hi","selBase":0,"selExt":2,"compBase":-1,"compExt":-1,"caretX":4,"caretY":8,"caretW":2,"caretH":16,"contentType":"normal","suppressSoftKeyboard":false}"#
        );
    }

    #[test]
    fn ime_json_without_caret_emits_nulls() {
        let json = ime_state_json(false, "", -1, -1, -1, -1, None, "normal", false);
        assert_eq!(
            json,
            r#"{"active":false,"text":"","selBase":-1,"selExt":-1,"compBase":-1,"compExt":-1,"caretX":null,"caretY":null,"caretW":null,"caretH":null,"contentType":"normal","suppressSoftKeyboard":false}"#
        );
    }

    #[test]
    fn ime_json_escapes_text_and_keeps_utf16_indices() {
        // Text with a quote is escaped; composing indices survive verbatim.
        let json = ime_state_json(true, "a\"b", 1, 1, 0, 3, None, "normal", false);
        assert!(json.contains(r#""text":"a\"b""#));
        assert!(json.contains(r#""compBase":0,"compExt":3"#));
    }

    #[test]
    fn tree_update_carries_full_tree_metadata_and_focus() {
        use accesskit::Role;
        let root = NodeId(1);
        let button = NodeId(0x2_0000);
        let nodes = vec![
            (button, Node::new(Role::Button)),
            (root, Node::new(Role::Window)),
        ];
        let update = build_tree_update(nodes, root, button, 1.0);
        // A full-tree update MUST carry `tree` metadata rooted at `root`.
        assert_eq!(update.tree, Some(Tree::new(root)));
        // Single root tree — no subtrees.
        assert_eq!(update.tree_id, TreeId::ROOT);
        // The resolved focus id is threaded straight through.
        assert_eq!(update.focus, button);
        // Nodes are moved through unchanged, order preserved.
        assert_eq!(update.nodes.len(), 2);
        assert_eq!(update.nodes[0].0, button);
        assert_eq!(update.nodes[1].0, root);
    }

    #[test]
    fn tree_update_defaults_focus_to_root_when_unfocused() {
        // `SemanticsUpdate::focus_id()` returns the root when nothing is focused;
        // this asserts the assembled update names the root as focus (accesskit's
        // `focus` field is non-optional).
        use accesskit::Role;
        let root = NodeId(1);
        let nodes = vec![(root, Node::new(Role::Window))];
        let update = build_tree_update(nodes, root, root, 1.0);
        assert_eq!(update.focus, root);
    }

    fn scaled_update(scale: f64) -> TreeUpdate {
        use accesskit::{Rect, Role};
        let (root, button) = (NodeId(1), NodeId(2));
        let mut root_node = Node::new(Role::Window);
        root_node.set_bounds(Rect::new(0.0, 0.0, 375.0, 667.0));
        let mut button_node = Node::new(Role::Button);
        button_node.set_bounds(Rect::new(312.0, 587.0, 374.0, 667.0));
        let nodes = vec![(root, root_node), (button, button_node)];
        build_tree_update(nodes, root, button, scale)
    }

    #[test]
    fn tree_update_scales_the_root_transform_by_the_display_scale() {
        use accesskit::{Affine, Rect};
        for scale in [2.0, 3.0] {
            let update = scaled_update(scale);
            assert_eq!(update.nodes[0].1.transform(), Some(&Affine::scale(scale)));
            assert_eq!(update.nodes[1].1.transform(), None);
            assert_eq!(
                update.nodes[1].1.bounds(),
                Some(Rect::new(312.0, 587.0, 374.0, 667.0))
            );
        }
        // 1x stays byte-identical: no transform anywhere.
        assert_eq!(scaled_update(1.0).nodes[0].1.transform(), None);
        // A node list without the root must not panic.
        let update = build_tree_update(
            vec![(NodeId(2), Node::new(accesskit::Role::Button))],
            NodeId(1),
            NodeId(1),
            2.0,
        );
        assert_eq!(update.nodes[0].1.transform(), None);
    }

    #[test]
    fn tree_update_allows_an_empty_node_list() {
        // A bare (unbuilt) tree still assembles a valid update — the adapter push
        // must never panic on an empty semantics pass.
        let root = NodeId(1);
        let update = build_tree_update(Vec::new(), root, root, 2.0);
        assert!(update.nodes.is_empty());
        assert_eq!(update.tree, Some(Tree::new(root)));
    }

    #[test]
    fn ime_json_nonfinite_caret_is_null() {
        let json = ime_state_json(
            true,
            "x",
            0,
            0,
            -1,
            -1,
            Some(CaretRect {
                x: f32::NAN,
                y: f32::INFINITY,
                w: 2.0,
                h: 10.0,
            }),
            "normal",
            false,
        );
        // Non-finite components degrade to null rather than emitting `NaN`/`inf`
        // (which are not valid JSON); finite ones still serialize.
        assert!(json.contains(r#""caretX":null,"caretY":null,"caretW":2,"caretH":10"#));
    }

    #[test]
    fn ime_json_encodes_content_type_password() {
        let json = ime_state_json(true, "hunter2", 7, 7, -1, -1, None, "password", false);
        assert!(json.contains(r#""contentType":"password""#));
    }

    #[test]
    fn ime_json_encodes_content_type_no_suggestions() {
        let json = ime_state_json(true, "ABC-123", 7, 7, -1, -1, None, "noSuggestions", false);
        assert!(json.contains(r#""contentType":"noSuggestions""#));
    }

    #[test]
    fn ime_json_encodes_the_soft_keyboard_suppression_hint() {
        // A focused read-only field: active (so the first responder, and with
        // it `copy:` and the system edit menu, stay wired) with the keyboard
        // asked down. Asserted as an exact string because the Android bridge
        // emits the same bytes — the trailing key must stay last, and spelled
        // this way, for that mirror to hold.
        let json = ime_state_json(true, "read-only", 0, 9, -1, -1, None, "normal", true);
        assert_eq!(
            json,
            r#"{"active":true,"text":"read-only","selBase":0,"selExt":9,"compBase":-1,"compExt":-1,"caretX":null,"caretY":null,"caretW":null,"caretH":null,"contentType":"normal","suppressSoftKeyboard":true}"#
        );
    }

    // --- Edit-command wire ABI --------------

    #[test]
    fn edit_command_codes_map_to_their_verbs() {
        // The fixed ABI the Swift `FrustView` edit actions compile against.
        // Pinned by literal, not by the named constants alone: a test written
        // against the constants would pass just as happily if every value were
        // renumbered together, which is precisely the shipped-caller breakage
        // the do-not-renumber rule exists to stop.
        assert_eq!(edit_command_from_code(0), Some(EditCommandKind::Copy));
        assert_eq!(edit_command_from_code(1), Some(EditCommandKind::Cut));
        assert_eq!(edit_command_from_code(2), Some(EditCommandKind::Paste));
        assert_eq!(edit_command_from_code(3), Some(EditCommandKind::SelectAll));
    }

    #[test]
    fn edit_command_constants_agree_with_the_wire_codes() {
        assert_eq!(EDIT_COMMAND_COPY, 0);
        assert_eq!(EDIT_COMMAND_CUT, 1);
        assert_eq!(EDIT_COMMAND_PASTE, 2);
        assert_eq!(EDIT_COMMAND_SELECT_ALL, 3);
    }

    #[test]
    fn unknown_edit_command_codes_are_refused_not_defaulted() {
        // Deliberately unlike `touch_phase_from_code`'s fallback: every verb in
        // this vocabulary either mutates the document or discloses it, so a
        // corrupt code must reach no widget at all rather than be guessed into
        // a destructive one.
        assert_eq!(edit_command_from_code(4), None);
        assert_eq!(edit_command_from_code(u8::MAX), None);
    }

    // --- Selection-toolbar JSON --------------

    fn anchor() -> ToolbarAnchor {
        ToolbarAnchor {
            x: 12.0,
            y: 40.0,
            w: 86.0,
            h: 18.0,
        }
    }

    #[test]
    fn selection_toolbar_json_golden() {
        // Field names are the contract the Swift poll reads by key — a rename
        // (or a `select_all`-style spelling for `selectAll`) silently turns
        // every verb into "absent", so the whole object is pinned verbatim
        // rather than probed with `contains`.
        let json = selection_toolbar_json(
            7,
            true,
            anchor(),
            ToolbarActions {
                copy: true,
                cut: true,
                paste: true,
                select_all: false,
            },
        );
        assert_eq!(
            json,
            r#"{"generation":7,"presentMenu":true,"x":12,"y":40,"w":86,"h":18,"copy":true,"cut":true,"paste":true,"selectAll":false}"#
        );
    }

    #[test]
    fn selection_toolbar_json_carries_each_verb_independently() {
        // One verb true at a time, so a builder that wrote the same flag into
        // four slots (or paired two of them up) cannot pass.
        let only = |pick: fn(&mut ToolbarActions)| {
            let mut actions = ToolbarActions::default();
            pick(&mut actions);
            selection_toolbar_json(1, true, anchor(), actions)
        };
        let copy = only(|a| a.copy = true);
        assert!(copy.contains(r#""copy":true"#));
        assert!(copy.contains(r#""cut":false,"paste":false,"selectAll":false"#));

        let cut = only(|a| a.cut = true);
        assert!(cut.contains(r#""copy":false,"cut":true,"paste":false,"selectAll":false"#));

        let paste = only(|a| a.paste = true);
        assert!(paste.contains(r#""copy":false,"cut":false,"paste":true,"selectAll":false"#));

        let select_all = only(|a| a.select_all = true);
        assert!(select_all.contains(r#""copy":false,"cut":false,"paste":false,"selectAll":true"#));
    }

    #[test]
    fn selection_toolbar_json_carries_the_present_flag_both_ways() {
        // The request's one edge, and the one field a builder could hardcode
        // and still pass the golden above: `true` asks the Swift side to put
        // the system menu up, `false` says the field is only reporting where it
        // is and which verbs apply. Hardcoding it would re-present the menu on
        // every verb change of a field nobody long-pressed.
        assert!(
            selection_toolbar_json(1, true, anchor(), ToolbarActions::default())
                .contains(r#""presentMenu":true"#)
        );
        assert!(
            selection_toolbar_json(1, false, anchor(), ToolbarActions::default())
                .contains(r#""presentMenu":false"#)
        );
    }

    #[test]
    fn selection_toolbar_json_reports_the_generation_it_was_given() {
        // The generation is the whole present-once mechanism on the Swift side:
        // a builder that hard-coded or dropped it would re-present the system
        // menu on every vsync a selection stands.
        assert!(
            selection_toolbar_json(0, true, anchor(), ToolbarActions::default())
                .starts_with(r#"{"generation":0,"#)
        );
        assert!(
            selection_toolbar_json(u64::MAX, true, anchor(), ToolbarActions::default())
                .starts_with(&format!(r#"{{"generation":{},"#, u64::MAX))
        );
    }

    #[test]
    fn selection_toolbar_json_keeps_the_anchor_components_in_order() {
        // x/y/w/h are four same-typed numbers in a row — the one shape where a
        // transposed pair produces perfectly valid JSON and a menu anchored
        // somewhere else entirely.
        let json = selection_toolbar_json(
            3,
            true,
            ToolbarAnchor {
                x: 1.0,
                y: 2.0,
                w: 3.0,
                h: 4.0,
            },
            ToolbarActions::default(),
        );
        assert!(json.contains(r#""x":1,"y":2,"w":3,"h":4"#));
    }

    #[test]
    fn selection_toolbar_json_sanitizes_a_non_finite_anchor() {
        // A required field, so it degrades to a finite `0` (the
        // `push_rect_json` rule) rather than to `null` (the optional caret's):
        // `NaN`/`inf` are not JSON, and an unparseable object costs the Swift
        // side the entire menu.
        let json = selection_toolbar_json(
            1,
            true,
            ToolbarAnchor {
                x: f32::NAN,
                y: f32::INFINITY,
                w: f32::NEG_INFINITY,
                h: 18.0,
            },
            ToolbarActions::default(),
        );
        assert!(json.contains(r#""x":0,"y":0,"w":0,"h":18"#));
        assert!(!json.contains("NaN"));
        assert!(!json.contains("inf"));
    }

    // --- Platform-view commands JSON --------------

    fn rect(x: f64, y: f64, w: f64, h: f64) -> PvRect {
        PvRect { x, y, w, h }
    }

    #[test]
    fn no_change_returns_none() {
        assert_eq!(platform_view_commands_json(3, 3, &[]), None);
    }

    #[test]
    fn create_then_update_golden() {
        let commands = vec![
            PlatformViewCommand::Create {
                slot_id: 3,
                view_type: "dev.frust.XFactory".to_string(),
                params_json: "{}".to_string(),
                interactive: false,
            },
            PlatformViewCommand::Update {
                slot_id: 3,
                rect: rect(10.0, 20.0, 100.0, 50.0),
                clip: None,
                visible: true,
                shields: Vec::new(),
            },
        ];
        let json = platform_view_commands_json(1, 0, &commands).unwrap();
        assert_eq!(
            json,
            r#"{"generation":1,"commands":[{"op":"create","slot":3,"viewType":"dev.frust.XFactory","params":"{}","interactive":false},{"op":"update","slot":3,"rect":[10,20,100,50],"clip":null,"visible":true,"shields":[]}]}"#
        );
    }

    #[test]
    fn update_with_clip_golden() {
        let commands = vec![PlatformViewCommand::Update {
            slot_id: 5,
            rect: rect(0.0, 0.0, 40.0, 40.0),
            clip: Some(rect(0.0, 0.0, 20.0, 40.0)),
            visible: false,
            shields: Vec::new(),
        }];
        let json = platform_view_commands_json(2, 1, &commands).unwrap();
        assert_eq!(
            json,
            r#"{"generation":2,"commands":[{"op":"update","slot":5,"rect":[0,0,40,40],"clip":[0,0,20,40],"visible":false,"shields":[]}]}"#
        );
    }

    #[test]
    fn update_params_golden() {
        let commands = vec![PlatformViewCommand::UpdateParams {
            slot_id: 7,
            params_json: "{\"a\":1}".to_string(),
        }];
        let json = platform_view_commands_json(3, 2, &commands).unwrap();
        assert_eq!(
            json,
            r#"{"generation":3,"commands":[{"op":"updateParams","slot":7,"params":"{\"a\":1}"}]}"#
        );
    }

    #[test]
    fn dispose_golden() {
        let commands = vec![PlatformViewCommand::Dispose { slot_id: 9 }];
        let json = platform_view_commands_json(4, 3, &commands).unwrap();
        assert_eq!(
            json,
            r#"{"generation":4,"commands":[{"op":"dispose","slot":9}]}"#
        );
    }

    #[test]
    fn view_type_and_params_are_escaped() {
        let commands = vec![PlatformViewCommand::Create {
            slot_id: 1,
            view_type: "dev.frust.\"Weird\"".to_string(),
            params_json: "line1\nline2".to_string(),
            interactive: false,
        }];
        let json = platform_view_commands_json(1, 0, &commands).unwrap();
        assert!(json.contains(r#""viewType":"dev.frust.\"Weird\"""#));
        assert!(json.contains(r#""params":"line1\nline2""#));
    }

    #[test]
    fn platform_view_rect_json_sanitizes_non_finite_components() {
        // Android parity: a `NaN`/`inf` rect component must
        // never emit an unparseable JSON token — the Swift host parses the
        // whole batch, so one bad number would drop every command in it.
        let commands = vec![PlatformViewCommand::Update {
            slot_id: 1,
            rect: PvRect {
                x: f64::NAN,
                y: f64::INFINITY,
                w: f64::NEG_INFINITY,
                h: 4.0,
            },
            clip: Some(PvRect {
                x: 0.0,
                y: f64::NAN,
                w: 2.0,
                h: 3.0,
            }),
            visible: true,
            shields: Vec::new(),
        }];
        let json = platform_view_commands_json(1, 0, &commands).unwrap();
        assert!(json.contains(r#""rect":[0,0,0,4]"#), "{json}");
        assert!(json.contains(r#""clip":[0,0,2,3]"#), "{json}");
        assert!(!json.contains("NaN") && !json.contains("inf"), "{json}");
    }

    #[test]
    fn resolved_translucency_handoff_survives_a_fake_install_sequence() {
        // The Arc<AtomicBool> handoff the render-thread split runs on, driven
        // with FAKE installs so it is host-testable (the real
        // `install_surface` needs a GPU and a `CAMetalLayer`).
        //
        // Seeded from the REQUEST: the app asked for translucency, so frame 1
        // renders Mode B optimistically rather than flashing opaque while the
        // render thread is still creating the surface.
        let flag = std::sync::atomic::AtomicBool::new(true);
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

        // A capable re-install / self-healed recreate restores Mode B.
        publish_resolved_translucency(&flag, Some(true));
        assert_eq!(
            base_clear_color(read_resolved_translucency(&flag), "TRANSPARENT", "surface"),
            "TRANSPARENT"
        );

        // A FAILED recreate clears it rather than leaving the previous
        // surface's `true` standing.
        publish_resolved_translucency(&flag, None);
        assert!(!read_resolved_translucency(&flag));
    }

    #[test]
    fn empty_command_batch_still_serializes() {
        // generation advances (e.g. a batch that only touched already-acked
        // entries via compaction) but nothing new is in the slice: an
        // ack_generation strictly behind generation still yields a valid
        // (if commands-empty) JSON object, not the None fast path.
        let json = platform_view_commands_json(2, 1, &[]).unwrap();
        assert_eq!(json, r#"{"generation":2,"commands":[]}"#);
    }

    // --- Overlay ops (JSON), byte-identical to the Android shell's --------

    fn overlay(
        overlay_id: u64,
        above_slot_id: u64,
        rect: PvRect,
        range: std::ops::Range<u64>,
    ) -> PlatformViewCommand {
        PlatformViewCommand::Overlay {
            overlay_id,
            above_slot_id,
            rect,
            range_start: range.start,
            range_end: range.end,
        }
    }

    #[test]
    fn platform_view_json_overlay_golden() {
        let json = platform_view_commands_json(
            8,
            0,
            &[overlay(2, 7, rect(0.0, 0.0, 1080.0, 2340.0), 41..96)],
        )
        .unwrap();
        assert_eq!(
            json,
            "{\"generation\":8,\"commands\":[{\"op\":\"overlay\",\"overlay\":2,\"aboveSlot\":7,\"rect\":[0,0,1080,2340],\"rangeStart\":41,\"rangeEnd\":96}]}"
        );
    }

    #[test]
    fn platform_view_json_dispose_overlay_golden() {
        let json = platform_view_commands_json(
            9,
            0,
            &[PlatformViewCommand::DisposeOverlay { overlay_id: 2 }],
        )
        .unwrap();
        assert_eq!(
            json,
            "{\"generation\":9,\"commands\":[{\"op\":\"disposeOverlay\",\"overlay\":2}]}"
        );
    }

    #[test]
    fn platform_view_json_overlay_ops_serialise_to_exact_strings() {
        let cases = [
            (
                overlay(1, 3, rect(0.0, 0.0, 720.5, 1600.0), 12..12),
                "{\"op\":\"overlay\",\"overlay\":1,\"aboveSlot\":3,\"rect\":[0,0,720.5,1600],\"rangeStart\":12,\"rangeEnd\":12}",
            ),
            (
                overlay(
                    u64::from(u32::MAX) + 5,
                    42,
                    rect(10.0, 20.0, 30.0, 40.0),
                    0..9000,
                ),
                "{\"op\":\"overlay\",\"overlay\":4294967300,\"aboveSlot\":42,\"rect\":[10,20,30,40],\"rangeStart\":0,\"rangeEnd\":9000}",
            ),
            (
                PlatformViewCommand::DisposeOverlay { overlay_id: 17 },
                "{\"op\":\"disposeOverlay\",\"overlay\":17}",
            ),
        ];
        for (op, expected) in cases {
            let json = platform_view_commands_json(1, 0, std::slice::from_ref(&op)).unwrap();
            assert_eq!(
                json,
                format!("{{\"generation\":1,\"commands\":[{expected}]}}")
            );
        }
    }

    /// The overlay ops are additive: a batch carrying them serialises the
    /// four frozen ops exactly as before, in order, around them — the same
    /// golden string the Android shell pins.
    #[test]
    fn platform_view_json_overlay_ops_leave_the_frozen_ops_unchanged() {
        let frozen = [
            PlatformViewCommand::Create {
                slot_id: 3,
                view_type: "dev.frust.XFactory".to_string(),
                params_json: String::new(),
                interactive: true,
            },
            PlatformViewCommand::Update {
                slot_id: 3,
                rect: rect(0.0, 0.0, 10.0, 10.0),
                clip: None,
                visible: true,
                shields: vec![rect(1.0, 2.0, 3.0, 4.0)],
            },
            PlatformViewCommand::UpdateParams {
                slot_id: 3,
                params_json: "{}".to_string(),
            },
            PlatformViewCommand::Dispose { slot_id: 3 },
        ];
        let alone = platform_view_commands_json(4, 0, &frozen).unwrap();
        let mut mixed = frozen.to_vec();
        mixed.insert(2, overlay(1, 3, rect(0.0, 0.0, 10.0, 10.0), 5..8));
        mixed.push(PlatformViewCommand::DisposeOverlay { overlay_id: 1 });
        let with_overlays = platform_view_commands_json(4, 0, &mixed).unwrap();
        let overlay_op = "{\"op\":\"overlay\",\"overlay\":1,\"aboveSlot\":3,\"rect\":[0,0,10,10],\"rangeStart\":5,\"rangeEnd\":8},";
        let dispose_op = ",{\"op\":\"disposeOverlay\",\"overlay\":1}";
        assert_eq!(
            with_overlays
                .replacen(overlay_op, "", 1)
                .replacen(dispose_op, "", 1),
            alone
        );
        assert_eq!(
            alone,
            "{\"generation\":4,\"commands\":[{\"op\":\"create\",\"slot\":3,\"viewType\":\"dev.frust.XFactory\",\"params\":\"\",\"interactive\":true},{\"op\":\"update\",\"slot\":3,\"rect\":[0,0,10,10],\"clip\":null,\"visible\":true,\"shields\":[[1,2,3,4]]},{\"op\":\"updateParams\",\"slot\":3,\"params\":\"{}\"},{\"op\":\"dispose\",\"slot\":3}]}"
        );
    }

    #[test]
    fn overlay_rect_json_sanitizes_non_finite_components() {
        let json = platform_view_commands_json(
            1,
            0,
            &[overlay(1, 2, rect(f64::NAN, 0.0, f64::INFINITY, 4.0), 0..1)],
        )
        .unwrap();
        assert!(json.contains("\"rect\":[0,0,0,4]"), "{json}");
    }

    // --- Overlay host: layers handed back for release -------------------

    type Log = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    /// A surface double that logs its own drop, so the drop-before-release
    /// order is observable.
    struct Tracked {
        name: String,
        log: Log,
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.log.borrow_mut().push(format!("drop {}", self.name));
        }
    }

    type Host = OverlayHost<usize, Tracked>;

    fn take(host: &mut Host, log: &Log) -> Vec<(u64, Tracked)> {
        let (flying, refusals) = host.take_for_frame(
            |layer, _| -> Result<Tracked, String> {
                Ok(Tracked {
                    name: format!("surface@{layer:#x}"),
                    log: log.clone(),
                })
            },
            |_, _| {},
        );
        assert!(refusals.is_empty());
        flying
    }

    /// Drain the release queue the way `frust_overlay_take_released` does: a
    /// caller buffer, refilled until it comes back short.
    fn drain(host: &mut Host, capacity: usize) -> Vec<usize> {
        let mut drained = Vec::new();
        loop {
            let mut buffer = vec![0usize; capacity];
            let count = host.take_released(&mut buffer);
            drained.extend_from_slice(&buffer[..count]);
            if count < capacity {
                return drained;
            }
        }
    }

    #[test]
    fn overlay_host_releases_a_landed_layer_at_once() {
        let log = Log::default();
        let mut host = Host::new();
        assert!(host.attach(1, 0x10, (8, 6)), "a new overlay owes a frame");
        assert!(!host.attach(1, 0x10, (8, 6)), "an unchanged one does not");
        let flying = take(&mut host, &log);
        assert_eq!(host.land(flying), 0);

        assert_eq!(host.detach(1), Withdraw::Released(0x10));
        assert_eq!(log.borrow().as_slice(), ["drop surface@0x10"]);
        assert_eq!(drain(&mut host, 4), [0x10]);
        assert!(host.is_empty());
        assert_eq!(host.detach(1), Withdraw::Unknown);
        assert!(drain(&mut host, 4).is_empty());
    }

    #[test]
    fn overlay_host_defers_a_flying_layer_to_its_land() {
        let log = Log::default();
        let mut host = Host::new();
        host.attach(1, 0x10, (8, 6));
        host.attach(2, 0x20, (8, 6));
        let flying = take(&mut host, &log);

        // Mid-frame: the withdraw returns at once and hands nothing back yet.
        assert_eq!(host.detach(1), Withdraw::Deferred);
        assert!(drain(&mut host, 4).is_empty());
        assert!(log.borrow().is_empty(), "the flying surface is untouched");
        // The render side drops overlay 1's parked present before landing.
        assert_eq!(host.retiring_among(&flying), [1]);

        host.land(flying);
        assert_eq!(log.borrow().as_slice(), ["drop surface@0x10"]);
        assert_eq!(drain(&mut host, 4), [0x10], "released only after the drop");
        // Overlay 2 kept its surface and is not retiring.
        let flying = take(&mut host, &log);
        assert!(host.retiring_among(&flying).is_empty());
        host.land(flying);
        assert!(!host.is_empty());
    }

    #[test]
    fn overlay_host_hands_back_replaced_layers_after_their_surface() {
        let log = Log::default();
        let mut host = Host::new();
        host.attach(1, 0x10, (8, 6));
        let flying = take(&mut host, &log);

        // Replaced twice mid-frame: the first layer comes back with the land
        // that drops its surface, the second never had one.
        assert!(host.attach(1, 0x20, (8, 6)));
        assert!(host.attach(1, 0x30, (8, 6)));
        assert_eq!(drain(&mut host, 4), [0x20]);
        assert_eq!(host.retiring_among(&flying), [1]);
        host.land(flying);
        assert_eq!(log.borrow().as_slice(), ["drop surface@0x10"]);
        assert_eq!(drain(&mut host, 4), [0x10]);

        // Replaced at rest: the surface goes, then the layer, at once.
        let flying = take(&mut host, &log);
        host.land(flying);
        assert!(host.attach(1, 0x40, (8, 6)));
        assert_eq!(
            log.borrow().last().map(String::as_str),
            Some("drop surface@0x30")
        );
        assert_eq!(drain(&mut host, 4), [0x30]);
    }

    #[test]
    fn overlay_host_release_queue_drains_oldest_first_in_buffers() {
        let log = Log::default();
        let mut host = Host::new();
        for id in 1..=5 {
            host.attach(id, 0x100 * id as usize, (1, 1));
        }
        let flying = take(&mut host, &log);
        host.land(flying);
        for id in 1..=5 {
            host.detach(id);
        }
        let mut two = [0usize; 2];
        assert_eq!(host.take_released(&mut two), 2);
        assert_eq!(two, [0x100, 0x200]);
        assert_eq!(drain(&mut host, 2), [0x300, 0x400, 0x500]);
        assert_eq!(host.take_released(&mut []), 0);
    }

    #[test]
    fn overlay_host_clear_forgets_everything_surfaces_first() {
        let log = Log::default();
        let mut host = Host::new();
        host.attach(1, 0x10, (1, 1));
        let flying = take(&mut host, &log);
        host.land(flying);
        host.attach(2, 0x20, (1, 1));
        host.detach(2);
        host.clear();
        assert_eq!(log.borrow().as_slice(), ["drop surface@0x10"]);
        assert!(host.is_empty());
        assert!(drain(&mut host, 4).is_empty());
    }

    #[test]
    fn overlay_host_latches_logging_and_follows_the_main_surface() {
        let log = Log::default();
        let mut host = Host::new();
        host.attach(1, 0x10, (1, 1));
        let flying = take(&mut host, &log);
        host.land(flying);
        assert_eq!(host.drop_surfaces(), 1);
        assert_eq!(host.note_error(1, 1), Some(1));
        assert_eq!(host.note_error(1, 1), None);
        assert!(host.note_unservable(1));
        assert!(!host.note_unservable(1));
    }

    // --- Range fold ----------------------------------------------------

    #[test]
    fn overlay_ranges_fold_only_what_is_new_and_restart_for_a_new_differ() {
        let mut ranges = OverlayRanges::new();
        assert_eq!(ranges.begin_fold(0), None);
        assert_eq!(ranges.begin_fold(3), Some(0));
        ranges.set(2, 40..50);
        ranges.set(1, 10..20);
        ranges.finish_fold(3);
        assert_eq!(ranges.cuts(), vec![(1, 10..20), (2, 40..50)]);
        assert_eq!(ranges.begin_fold(3), None);
        assert_eq!(ranges.begin_fold(5), Some(3));
        ranges.dispose(1);
        ranges.finish_fold(5);
        assert_eq!(ranges.cuts(), vec![(2, 40..50)]);
        // An older generation is a fresh differ: start over.
        assert_eq!(ranges.begin_fold(1), Some(0));
        assert!(ranges.cuts().is_empty());
    }

    // --- Present parking slot ------------------------------------------

    #[test]
    fn present_slot_presents_under_the_slot_and_drains_for_the_render_side() {
        let slot: PresentSlot<u64> = PresentSlot::new();
        assert_eq!(slot.wait_drained(std::time::Duration::ZERO), None);
        assert_eq!(slot.store(7), None);
        assert!(slot.is_parked());
        assert_eq!(slot.take_with(|frame| frame * 2), Some(14));
        assert!(!slot.is_parked());
        assert_eq!(slot.take_with(|frame| frame), None);
    }

    #[test]
    fn present_slot_waits_for_the_ui_thread_before_an_acquire() {
        let slot = std::sync::Arc::new(PresentSlot::<u64>::new());
        slot.store(1);
        let ui = {
            let slot = std::sync::Arc::clone(&slot);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(20));
                slot.take_with(|frame| frame)
            })
        };
        // The render side waits for the present instead of acquiring over a
        // parked frame: the UI thread presented it, nothing is left to drop.
        assert_eq!(slot.wait_drained(std::time::Duration::from_secs(5)), None);
        assert_eq!(ui.join().unwrap(), Some(1));
    }

    #[test]
    fn present_slot_drops_a_frame_the_ui_thread_never_took() {
        let slot: PresentSlot<u64> = PresentSlot::new();
        slot.store(3);
        // Bounded: past the wait, the leftover comes out for the caller to
        // drop, so the render side never acquires with it parked.
        assert_eq!(
            slot.wait_drained(std::time::Duration::from_millis(5)),
            Some(3)
        );
        assert!(!slot.is_parked());
        slot.store(4);
        assert_eq!(slot.store(5), Some(4));
        slot.with_parked(|frame| *frame += 1);
        assert_eq!(slot.clear(), Some(6));
        assert_eq!(slot.clear(), None);
    }
}
