// Frust's C FFI surface, consumed from Swift as `import CFrustFFI`. This is
// the former `Runner/Runner-Bridging-Header.h` of a generated app, moved into
// the embedding package verbatim (declarations byte-identical; only the include
// guard was renamed) — a scaffolded app no longer carries a bridging header at
// all, and `SWIFT_OBJC_BRIDGING_HEADER` is set nowhere.
//
// Every symbol below is DECLARED here and DEFINED nowhere in this package: they
// are `extern "C"` exports of `frust-shell-ios`, linked in from the Rust
// staticlib the consuming app target's "Build Rust staticlib" phase produces.
// Undefined symbols in this target are by design (Xcode links a package target
// to a relocatable `.o`, so they resolve at the app's final link).
//
// Signatures are fixed by contract — keep them in sync with the `extern "C"`
// exports on the Rust side.

#ifndef FRUST_FFI_H
#define FRUST_FFI_H

#include <stdint.h>

// Frust native entry points, exported by the Rust staticlib built by the
// "Build Rust staticlib" run-script phase (frust-shell-ios). Signatures are
// fixed by contract; keep them in sync with the `extern "C"` exports on the
// Rust side.
//
// Lifetime contract: `metal_layer` (the app's CAMetalLayer) must outlive the
// handle returned by frust_init — Swift owns the layer and must call
// frust_destroy before releasing the view.
void *frust_init(void *metal_layer, uint32_t width, uint32_t height, float scale);
// Accessibility: attach the AccessKit adapter to the app's
// FrustView. Separate from frust_init because the adapter subclasses the
// *UIView*, whereas frust_init only gets the CAMetalLayer. Call this once,
// on the first layout, right after frust_init returns a handle and before
// the view is shown — pass the FrustView pointer as `view`. A null handle or
// null view is a no-op.
void  frust_init_accessibility(void *handle, void *view);
void  frust_resize(void *handle, uint32_t width, uint32_t height, float scale);
// `timestamp_ns` is the CADisplayLink tick's `timestamp` (CFTimeInterval
// seconds), converted to nanoseconds by the caller: UInt64(link.timestamp * 1_000_000_000).
// Returns 0 = FATAL, unrecoverable render-thread failure (the first surface
// install could not succeed — an incapable GPU/driver); 1 = keep driving
// frames. On 0 the Swift side latches `initFailed` and invalidates its
// CADisplayLink (see FrustViewController.renderFrame). uint8_t, not C _Bool,
// to match frust_set_appearance's no-<stdbool.h> convention.
uint8_t frust_render_frame(void *handle, uint64_t timestamp_ns);
// Touch delivery. `phase` is a fixed numeric ABI shared with the Rust
// `frust_dispatch_touch` glue — DO NOT renumber without changing both sides:
//   0 = began, 1 = moved, 2 = ended, 3 = cancelled.
// `x`/`y` are logical points (`touch.location(in:)`) — already density-scaled,
// so the Rust side passes them through without dividing by the display scale.
void  frust_dispatch_touch(void *handle, uint32_t phase, float x, float y);
void  frust_pause(void *handle);
void  frust_resume(void *handle);
void  frust_destroy(void *handle);

// Text input. The Swift `FrustView` adopts the full
// UITextInput protocol over a UTF-16 NSMutableString mirror and syncs it here.
//
// frust_ime_apply pushes the whole editing value: `text` is UTF-8;
// `sel_*`/`comp_*` are UTF-16 code-unit indices (-1 = none for the composing
// region). frust_ime_state_json returns the reconciled editing state as a
// heap-allocated JSON string the CALLER MUST FREE with frust_string_free:
//   {"active":bool,"text":"...","selBase":n,"selExt":n,"compBase":n,
//    "compExt":n,"caretX":f,"caretY":f,"caretW":f,"caretH":f}
// (absent caret => the four caret fields are null). A null return is the
// "no editing state" sentinel. frust_string_free is a no-op on null.
void  frust_ime_apply(void *handle, const char *text, int32_t sel_base, int32_t sel_ext, int32_t comp_base, int32_t comp_ext);
char *frust_ime_state_json(void *handle);
// Also frees the strings frust_take_clipboard_write,
// frust_selection_toolbar_json and frust_platform_view_commands_json return —
// one ownership contract for every heap-allocated string that crosses this
// boundary. A no-op on null.
void  frust_string_free(char *s);

// Clipboard + the SYSTEM edit menu. iOS exempts a paste from its "pasted from"
// banner (14+) and its per-app paste-permission alert (16+) ONLY when the paste
// is system-initiated — a tap in UIKit's own edit menu, or a hardware Cmd+V
// through the responder chain. So on this platform the framework does NOT draw
// the selection toolbar: the Rust side declares the Native selection-toolbar
// policy at frust_init, a focused field then publishes its request and floats
// nothing, and FrustView/FrustViewController present UIKit's own menu at the
// published anchor. The pasteboard and the menu are Swift-side throughout; the
// Rust half never links UIKit.
//
// frust_edit_command delivers one verb from that menu (or from a hardware
// Cmd+C/X/V/A UIKit resolved). `cmd` is a fixed numeric ABI shared with the
// Rust glue — DO NOT renumber without changing both sides:
//   0 = copy, 1 = cut, 2 = paste, 3 = select all.
// An unrecognised code is dropped rather than guessed at (unlike
// frust_dispatch_touch's phase, every verb here mutates or discloses the
// document, so there is no safe default). `text` is the pasted UTF-8 payload
// and is read for cmd == 2 ONLY; pass NULL for the other three. Copy and cut
// carry nothing because the widget owns the selection and answers by filling
// the slot frust_take_clipboard_write drains.
void  frust_edit_command(void *handle, uint8_t cmd, const char *text);
// frust_take_clipboard_write takes (and clears) the text a widget asked to put
// on the pasteboard — the answer to a copy/cut. Returns a heap-allocated
// string the CALLER MUST FREE with frust_string_free, or NULL when nothing was
// copied. DESTRUCTIVE: the slot is an edge, so a caller that drains and drops
// the result loses that write. FrustViewController drains it once per
// CADisplayLink tick into UIPasteboard.general.string.
char *frust_take_clipboard_write(void *handle);
// frust_take_paste_request takes (and clears) whether a widget asked the shell
// to read the pasteboard back to it. 1 = asked, 0 = not (plain uint8_t, the
// same no-<stdbool.h> convention as frust_set_appearance's `dark`).
// DESTRUCTIVE, like the drain above.
//
// This is the one paste route iOS does NOT exempt: answering it means reading
// UIPasteboard.general.string from a display-link tick, which is not a
// system-initiated action and so raises the notice/alert. Only a
// FRAMEWORK-drawn selection toolbar's Paste button reaches it, and iOS never
// draws the framework toolbar (see the Native policy above) — so in practice
// it stays unused. Wired anyway so the shell implements the whole clipboard
// channel rather than half of it.
uint8_t frust_take_paste_request(void *handle);
// frust_selection_toolbar_json reports the focused field's clipboard verbs,
// where a system edit menu should be anchored, and whether one should be
// presented right now — as a heap-allocated JSON string the CALLER MUST FREE
// with frust_string_free, or NULL when NO FIELD IS FOCUSED (the "dismiss
// whatever stands and forget the generation" sentinel):
//   {"generation":n,"presentMenu":bool,"x":f,"y":f,"w":f,"h":f,
//    "copy":bool,"cut":bool,"paste":bool,"selectAll":bool}
// x/y/w/h are LOGICAL POINTS, the same space as the caret rect the IME JSON
// above reports and as frust_dispatch_touch's coordinates — no scale
// conversion on either side. The rect is absolute in the window, and FrustView
// fills the window, so it is already in the view's own coordinates.
//
// A FOCUSED FIELD ANSWERS EVERY TICK, selection or not. The verbs are a LEVEL,
// not an edge: read them on every poll and keep the responder's answer to
// canPerformAction: up to date from them, because a hardware Cmd+C/X/V/A is
// asked for with no menu on screen at all. With no selection the anchor is the
// caret rect, which is the right place for a paste-only menu.
//
// `presentMenu` is the EDGE half: true means the field wants a menu presented
// now (the user long-pressed or tapped inside a selection), false means it
// does not. Present when the generation moves and presentMenu is true; dismiss
// when the generation moves and it is false.
//
// `generation` moves only when presentMenu or the verb set actually changes —
// NOT when the anchor alone moves. That exclusion is deliberate: the anchor is
// recomputed every painted frame and tracks a dragging selection, so moving
// the generation with it would ask the host to re-present its menu on every
// touch sample. Re-read the anchor as a level for the menu's target rect; do
// not treat it as a reason to present.
char *frust_selection_toolbar_json(void *handle);

// Appearance: flip the app's theme brightness between
// light and dark. `dark` is 0/1 (no existing bool-ish precedent to match in
// this header, so plain uint8_t) — see FrustViewController's
// traitCollectionDidChange, which also seeds the initial value right after
// frust_init returns a handle.
void  frust_set_appearance(void *handle, uint8_t dark);

// Reduced motion: apply the platform's reduce-motion accessibility
// preference to the app's motion tokens. `reduce` is 0/1 (same convention as
// `dark` above). The source is UIAccessibility.isReduceMotionEnabled — NOT a
// UITraitCollection trait, so FrustViewController observes
// UIAccessibility.reduceMotionStatusDidChangeNotification for live changes and
// seeds the initial value right after frust_init returns a handle. The Rust
// side ORs this over the active theme's own reduce_motion token (a floor, not
// a replacement), so it also reaches a theme forced with frust::set_app_theme.
void  frust_set_reduce_motion(void *handle, uint8_t reduce);

// Deep links: deliver a platform URL — cold-start, from
// SceneDelegate's connectionOptions.urlContexts, or running, from
// scene(_:openURLContexts:) — into the process-wide deep-link source. `url`
// is the URL's absoluteString as a UTF-8 C string; malformed/null input
// decodes lossily. A missing handle is a no-op (Swift queues the link until
// frust_init has returned a handle — see FrustViewController).
void  frust_on_deep_link(void *handle, const char *url);

// Insets. The eight floats are `view_padding` (vp_*: system-bar/notch/cutout
// occlusion, from the view's safeAreaInsets) then `view_insets` (vi_*: the
// area the on-screen keyboard obscures, not part of safeAreaInsets and
// tracked separately), each left/top/right/bottom, in logical points
// (UIKit's coordinate space — no scale multiplication, the same asymmetry
// frust_dispatch_touch uses). `padding = max(0, view_padding - view_insets)`
// per edge is derived on the Rust side, never transported. See
// FrustViewController's viewSafeAreaInsetsDidChange/keyboard-notification
// handlers.
void  frust_set_insets(
    void *handle,
    float vp_l, float vp_t, float vp_r, float vp_b,
    float vi_l, float vi_t, float vi_r, float vi_b
);

// System UI / SystemChrome: peek the process-wide `frust::set_system_ui_mode`
// override slot, returning `frust_shell_common::system_ui::encoded_state()`'s
// packed `(generation, mode)` u64 verbatim (see that fn's doc comment for the
// exact bit layout). `handle` is taken and ignored (the slot is
// process-global) — see FrustViewController's `pollSystemUiState`.
uint64_t frust_system_ui_state(void *handle);

// Platform views: hosting native UIKit sibling views alongside the Frust
// render surface.
//
// frust_set_surface_mode requests a translucent (non-opaque) render surface.
// Callable BEFORE frust_init (no handle arg — it writes a process-global,
// one-way latch), which is what FrustViewController does from viewDidLoad
// when its `translucentSurface` const is set. `translucent` is 0/1 (plain
// uint8_t, matching frust_set_appearance's no-<stdbool.h> convention). A
// translucent surface lets native sibling views hosted BELOW it (Mode B —
// see FrustViewHost) composite through.
void  frust_set_surface_mode(uint8_t translucent);
// frust_platform_view_commands_json returns the platform-view compositor
// command backlog accumulated since `ack_generation` as a heap-allocated
// JSON string the CALLER MUST FREE with frust_string_free — or NULL when
// nothing changed (generation == ack_generation), the cheap per-frame
// no-change fast path. Rects are PHYSICAL px ([x,y,w,h] arrays);
// byte-identical schema to Android's nativePlatformViewCommands:
//   {"generation":N,"commands":[
//     {"op":"create","slot":n,"viewType":"...","params":"..."},
//     {"op":"update","slot":n,"rect":[x,y,w,h],"clip":[x,y,w,h]|null,"visible":bool},
//     {"op":"updateParams","slot":n,"params":"..."},
//     {"op":"dispose","slot":n}]}
// FrustViewHost parses and applies each batch (see FrustViewController's
// renderFrame, which polls it after frust_render_frame).
char *frust_platform_view_commands_json(void *handle, uint64_t ack_generation);

// Present-sync. iOS's platform-view desync has the OPPOSITE sign to
// Android's: the native sibling LAGS frust's content, so the fix holds
// frust's *present* back to meet the geometry rather than delaying the view.
//
// frust_set_present_sync declares that this host will present the surface
// itself, inside the CATransaction that commits platform-view geometry.
// Callable BEFORE frust_init (no handle arg — a process-global latch, like
// frust_set_surface_mode), which is what FrustViewController does from
// viewDidLoad when its `synchronizesPresentWithPlatformViews` const is set —
// in the SAME branch that sets CAMetalLayer.presentsWithTransaction = true.
// The two halves must move together: the layer flag alone stops presentation
// entirely under the render-thread split (the drawable is handed over on a
// thread that commits no transaction), and this call alone only defers each
// present by a tick. `enabled` is 0/1 (plain uint8_t, matching
// frust_set_appearance's no-<stdbool.h> convention). The render-thread split
// stays ON — this is not a kill switch.
void  frust_set_present_sync(uint8_t enabled);
// frust_present_frame presents the frame the render thread parked for the UI
// thread, and records which frust frame that was. Call it once per
// CADisplayLink tick, inside a CATransaction and BEFORE FrustViewHost.poll:
// the poll then releases exactly the geometry batch that frame painted, so
// pixels and native-sibling geometry land in one visual frame (see
// FrustViewController.renderFrame — the order is load-bearing). A no-op when
// present-sync was never declared, on the inline render path
// (FRUST_NO_RENDER_THREAD), on a frame-gate-skipped tick, or with no live
// handle — safe to call unconditionally.
void  frust_present_frame(void *handle);

#endif /* FRUST_FFI_H */
