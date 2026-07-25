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
// "Build Rust staticlib" run-script phase (frust-shell-ios, spec Phase 3
// task 33). Signatures are fixed by contract; keep them in sync with the
// `extern "C"` exports on the Rust side.
//
// Lifetime contract: `metal_layer` (the app's CAMetalLayer) must outlive the
// handle returned by frust_init — Swift owns the layer and must call
// frust_destroy before releasing the view.
void *frust_init(void *metal_layer, uint32_t width, uint32_t height, float scale);
// Accessibility (spec §9, phase 6d): attach the AccessKit adapter to the app's
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
// Touch delivery (spec §9). `phase` is a fixed numeric ABI shared with the Rust
// `frust_dispatch_touch` glue — DO NOT renumber without changing both sides:
//   0 = began, 1 = moved, 2 = ended, 3 = cancelled.
// `x`/`y` are logical points (`touch.location(in:)`) — already density-scaled,
// so the Rust side passes them through without dividing by the display scale.
void  frust_dispatch_touch(void *handle, uint32_t phase, float x, float y);
void  frust_pause(void *handle);
void  frust_resume(void *handle);
void  frust_destroy(void *handle);

// Text input (spec §9, Phase 4B). The Swift `FrustView` adopts the full
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
void  frust_string_free(char *s);

// Appearance (spec §17, task 08): flip the app's theme brightness between
// light and dark. `dark` is 0/1 (no existing bool-ish precedent to match in
// this header, so plain uint8_t) — see FrustViewController's
// traitCollectionDidChange, which also seeds the initial value right after
// frust_init returns a handle.
void  frust_set_appearance(void *handle, uint8_t dark);

// Deep links (task 07): deliver a platform URL — cold-start, from
// SceneDelegate's connectionOptions.urlContexts, or running, from
// scene(_:openURLContexts:) — into the process-wide deep-link source. `url`
// is the URL's absoluteString as a UTF-8 C string; malformed/null input
// decodes lossily. A missing handle is a no-op (Swift queues the link until
// frust_init has returned a handle — see FrustViewController).
void  frust_on_deep_link(void *handle, const char *url);

// Insets (device-parity task 06/08 — RESEARCH.md "Insets / SafeArea"). The
// eight floats are `view_padding` (vp_*: from the view's safeAreaInsets)
// then `view_insets` (vi_*: the keyboard frame), each left/top/right/bottom,
// in logical points (UIKit's coordinate space — no scale multiplication, the
// same asymmetry frust_dispatch_touch uses). See FrustViewController's
// viewSafeAreaInsetsDidChange/keyboard-notification handlers.
void  frust_set_insets(
    void *handle,
    float vp_l, float vp_t, float vp_r, float vp_b,
    float vi_l, float vi_t, float vi_r, float vi_b
);

// System UI / SystemChrome (task 03/09/10/14 — RESEARCH.md "Insets /
// SafeArea / SystemChrome"): peek the process-wide `frust::set_system_ui_mode`
// override slot, returning `frust_shell_common::system_ui::encoded_state()`'s
// packed `(generation, mode)` u64 verbatim (see that fn's doc comment for the
// exact bit layout). `handle` is taken and ignored (the slot is
// process-global) — see FrustViewController's `pollSystemUiState`.
uint64_t frust_system_ui_state(void *handle);

// Platform views (platform-views task 06/09 — RESEARCH.md "Platform views").
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

#endif /* FRUST_FFI_H */
