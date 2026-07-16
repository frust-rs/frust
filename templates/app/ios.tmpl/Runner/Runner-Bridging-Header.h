#ifndef Runner_Bridging_Header_h
#define Runner_Bridging_Header_h

#include <stdint.h>

// ForgeKit native entry points, exported by the Rust staticlib built by the
// "Build Rust staticlib" run-script phase (forgekit-shell-ios, spec Phase 3
// task 33). Signatures are fixed by contract; keep them in sync with the
// `extern "C"` exports on the Rust side.
//
// Lifetime contract: `metal_layer` (the app's CAMetalLayer) must outlive the
// handle returned by forgekit_init — Swift owns the layer and must call
// forgekit_destroy before releasing the view.
void *forgekit_init(void *metal_layer, uint32_t width, uint32_t height, float scale);
void  forgekit_resize(void *handle, uint32_t width, uint32_t height, float scale);
void  forgekit_render_frame(void *handle);
// Touch delivery (spec §9). `phase` is a fixed numeric ABI shared with the Rust
// `forgekit_dispatch_touch` glue — DO NOT renumber without changing both sides:
//   0 = began, 1 = moved, 2 = ended, 3 = cancelled.
// `x`/`y` are logical points (`touch.location(in:)`) — already density-scaled,
// so the Rust side passes them through without dividing by the display scale.
void  forgekit_dispatch_touch(void *handle, uint32_t phase, float x, float y);
void  forgekit_pause(void *handle);
void  forgekit_resume(void *handle);
void  forgekit_destroy(void *handle);

// Text input (spec §9, Phase 4B). The Swift `ForgeKitView` adopts the full
// UITextInput protocol over a UTF-16 NSMutableString mirror and syncs it here.
//
// forgekit_ime_apply pushes the whole editing value: `text` is UTF-8;
// `sel_*`/`comp_*` are UTF-16 code-unit indices (-1 = none for the composing
// region). forgekit_ime_state_json returns the reconciled editing state as a
// heap-allocated JSON string the CALLER MUST FREE with forgekit_string_free:
//   {"active":bool,"text":"...","selBase":n,"selExt":n,"compBase":n,
//    "compExt":n,"caretX":f,"caretY":f,"caretW":f,"caretH":f}
// (absent caret => the four caret fields are null). A null return is the
// "no editing state" sentinel. forgekit_string_free is a no-op on null.
void  forgekit_ime_apply(void *handle, const char *text, int32_t sel_base, int32_t sel_ext, int32_t comp_base, int32_t comp_ext);
char *forgekit_ime_state_json(void *handle);
void  forgekit_string_free(char *s);

#endif /* Runner_Bridging_Header_h */
