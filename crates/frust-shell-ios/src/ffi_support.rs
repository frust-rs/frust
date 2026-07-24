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

use accesskit::{Node, NodeId, Tree, TreeId, TreeUpdate};
use frust_render::SurfacePhase;

/// Assemble an `accesskit::TreeUpdate` from the flat pieces of a
/// `frust_core::SemanticsUpdate` (phase-6d task 05, D3-ios).
///
/// This is the pure, host-testable core of the iOS accesskit push: the iOS
/// adapter ([`crate::accessibility`]) calls it inside `Adapter::update_if_active`
/// to turn a semantics pass into the incremental update accesskit consumes. It
/// lives here (not in the target-gated adapter) so its correctness is covered by
/// `cargo test --workspace` on the host, even though the `accesskit_ios` adapter
/// that consumes the result is only ever compiled for iOS.
///
/// v1 pushes the **whole tree every update** (allowed because our node ids are
/// stable across frames — phase-6d D1), so:
/// - `tree` is always `Some(Tree::new(root))` (the whole-tree metadata accesskit
///   requires on every full update);
/// - `tree_id` is [`TreeId::ROOT`] — Frust exposes a single root tree, no
///   subtrees;
/// - `focus` is the caller's already-resolved focus id
///   (`SemanticsUpdate::focus_id()` — the focused node, or the root when nothing
///   is focused, since accesskit's `focus` is non-optional).
///
/// `nodes` is moved through unchanged — our semantics pass already produces the
/// exact `Vec<(NodeId, Node)>` shape `TreeUpdate` wants.
pub(crate) fn build_tree_update(
    nodes: Vec<(NodeId, Node)>,
    root: NodeId,
    focus: NodeId,
) -> TreeUpdate {
    TreeUpdate {
        nodes,
        tree: Some(Tree::new(root)),
        tree_id: TreeId::ROOT,
        focus,
    }
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
/// appearance state-machine transition task 08 wires up.
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
/// SurfaceReady" (spec §8.1).
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

/// `frust_render_frame`'s "keep driving frames" return code (phase-11 fix F2).
pub(crate) const FRAME_ALIVE: u8 = 1;

/// `frust_render_frame`'s "fatal — stop the CADisplayLink" return code
/// (phase-11 fix F2).
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
///  "caretX":f,"caretY":f,"caretW":f,"caretH":f}
/// ```
///
/// Selection/composing indices are UTF-16 code units (the platform-native unit,
/// passed opaquely through the `EditingState` shell seam — see
/// `frust_core::event::EditingState`); `-1` denotes "none". The caret is
/// logical-pixel; an absent (or non-finite) caret serializes the four caret
/// fields as `null`. JSON is hand-rolled (no `serde` in the shell) with a tiny
/// escaper — see [`json_escape_into`].
pub(crate) fn ime_state_json(
    active: bool,
    text: &str,
    sel_base: i32,
    sel_ext: i32,
    comp_base: i32,
    comp_ext: i32,
    caret: Option<CaretRect>,
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

/// A logical-or-physical rect, decoupled from `kurbo::Rect` so this module
/// stays host-testable (`kurbo` is an iOS-gated dependency in this crate —
/// see the crate's `Cargo.toml`, mirroring [`CaretRect`] above). The caller
/// ([`crate::app::IosAppHandle`]) has already converted logical px into
/// **physical** px (`* scale`) before building one of these — the
/// physical-at-FFI/logical-inside rule, applied outbound (matches
/// `frust-shell-android`'s `nativePlatformViewCommands` JSON, task 05).
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
    },
    /// Place/resize/clip/show-or-hide an already-created slot.
    Update {
        slot_id: u64,
        rect: PvRect,
        clip: Option<PvRect>,
        visible: bool,
    },
    /// `params_json` changed with no necessary rect/clip/visible change.
    UpdateParams { slot_id: u64, params_json: String },
    /// Tear down a slot's native view entirely.
    Dispose { slot_id: u64 },
}

/// Serialize the platform-view command backlog into the wire JSON
/// `frust_platform_view_commands_json` returns (platform-views task 06) —
/// byte-identical schema to `frust-shell-android`'s `nativePlatformViewCommands`
/// (task 05), so a shared Kotlin/Swift-side parser (a future template task)
/// need not branch on platform:
///
/// ```text
/// {"generation":7,"commands":[
///  {"op":"create","slot":3,"viewType":"dev.frust.XFactory","params":"{...}"},
///  {"op":"update","slot":3,"rect":[x,y,w,h],"clip":[x,y,w,h]|null,"visible":true},
///  {"op":"updateParams","slot":3,"params":"{...}"},
///  {"op":"dispose","slot":3}]}
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
        } => {
            out.push_str("{\"op\":\"create\",\"slot\":");
            let _ = write!(out, "{slot_id}");
            out.push_str(",\"viewType\":\"");
            json_escape_into(view_type, out);
            out.push_str("\",\"params\":\"");
            json_escape_into(params_json, out);
            out.push_str("\"}");
        }
        PlatformViewCommand::Update {
            slot_id,
            rect,
            clip,
            visible,
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
            out.push('}');
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
    }
}

fn push_rect_json(out: &mut String, rect: PvRect) {
    out.push('[');
    let _ = write!(out, "{}", rect.x);
    out.push(',');
    let _ = write!(out, "{}", rect.y);
    out.push(',');
    let _ = write!(out, "{}", rect.w);
    out.push(',');
    let _ = write!(out, "{}", rect.h);
    out.push(']');
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
        );
        assert_eq!(
            json,
            r#"{"active":true,"text":"hi","selBase":0,"selExt":2,"compBase":-1,"compExt":-1,"caretX":4,"caretY":8,"caretW":2,"caretH":16}"#
        );
    }

    #[test]
    fn ime_json_without_caret_emits_nulls() {
        let json = ime_state_json(false, "", -1, -1, -1, -1, None);
        assert_eq!(
            json,
            r#"{"active":false,"text":"","selBase":-1,"selExt":-1,"compBase":-1,"compExt":-1,"caretX":null,"caretY":null,"caretW":null,"caretH":null}"#
        );
    }

    #[test]
    fn ime_json_escapes_text_and_keeps_utf16_indices() {
        // Text with a quote is escaped; composing indices survive verbatim.
        let json = ime_state_json(true, "a\"b", 1, 1, 0, 3, None);
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
        let update = build_tree_update(nodes, root, button);
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
        let update = build_tree_update(nodes, root, root);
        assert_eq!(update.focus, root);
    }

    #[test]
    fn tree_update_allows_an_empty_node_list() {
        // A bare (unbuilt) tree still assembles a valid update — the adapter push
        // must never panic on an empty semantics pass.
        let root = NodeId(1);
        let update = build_tree_update(Vec::new(), root, root);
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
        );
        // Non-finite components degrade to null rather than emitting `NaN`/`inf`
        // (which are not valid JSON); finite ones still serialize.
        assert!(json.contains(r#""caretX":null,"caretY":null,"caretW":2,"caretH":10"#));
    }

    // --- Platform-view commands JSON (platform-views task 06) --------------

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
            },
            PlatformViewCommand::Update {
                slot_id: 3,
                rect: rect(10.0, 20.0, 100.0, 50.0),
                clip: None,
                visible: true,
            },
        ];
        let json = platform_view_commands_json(1, 0, &commands).unwrap();
        assert_eq!(
            json,
            r#"{"generation":1,"commands":[{"op":"create","slot":3,"viewType":"dev.frust.XFactory","params":"{}"},{"op":"update","slot":3,"rect":[10,20,100,50],"clip":null,"visible":true}]}"#
        );
    }

    #[test]
    fn update_with_clip_golden() {
        let commands = vec![PlatformViewCommand::Update {
            slot_id: 5,
            rect: rect(0.0, 0.0, 40.0, 40.0),
            clip: Some(rect(0.0, 0.0, 20.0, 40.0)),
            visible: false,
        }];
        let json = platform_view_commands_json(2, 1, &commands).unwrap();
        assert_eq!(
            json,
            r#"{"generation":2,"commands":[{"op":"update","slot":5,"rect":[0,0,40,40],"clip":[0,0,20,40],"visible":false}]}"#
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
        }];
        let json = platform_view_commands_json(1, 0, &commands).unwrap();
        assert!(json.contains(r#""viewType":"dev.frust.\"Weird\"""#));
        assert!(json.contains(r#""params":"line1\nline2""#));
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
}
