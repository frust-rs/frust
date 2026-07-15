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
void  forgekit_pause(void *handle);
void  forgekit_resume(void *handle);
void  forgekit_destroy(void *handle);

#endif /* Runner_Bridging_Header_h */
