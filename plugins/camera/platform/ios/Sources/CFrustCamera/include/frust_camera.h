// The `frust-camera` plugin's C FFI surface, consumed from Swift as
// `import CFrustCamera` — the plugin-package mirror of
// `platform/ios/FrustEmbedding/Sources/CFrustFFI/include/frust_ffi.h`.
//
// The symbol below is DECLARED here and DEFINED nowhere in this package: it
// is a `#[no_mangle] extern "C"` export of `plugins/camera/src/apple.rs`
// (task 07), linked in from the SAME Rust staticlib the consuming app's
// "Build Rust staticlib" run-script phase already produces for
// `CFrustFFI`'s exports — a second C target resolving symbols from one
// staticlib (task 08's two-package spike; see that task's completion
// summary for what's proven on this host vs owed to the mac device gate,
// task 14). An undefined symbol in this target's object file is by design
// (Xcode links a package target to a relocatable `.o`, so it resolves at
// the app's final link, same as `CFrustFFI`).
//
// Signature is fixed by contract — keep it in sync with
// `plugins/camera/src/apple.rs`'s `frust_camera_session_handle` export.
// ⚠️ Symbol-survival caveat (task 07's doc, restated here): a
// dependency-crate `#[no_mangle]` export may be stripped under
// `lto = "fat"`/`strip = "symbols"`; the fallback is the
// `frust_camera::ios_exports!()` macro the app crate can invoke instead —
// which one survives is settled by the device gate (task 14).

#ifndef FRUST_CAMERA_H
#define FRUST_CAMERA_H

#include <stdint.h>

// Returns an `AVCaptureSession *` (bridged, not retained beyond the caller's
// own use) for the open `frust-camera` session identified by `session` — an
// opaque handle id `plugins/camera/src/lib.rs`'s `CameraSession` never
// exposes directly to Swift; `CameraPreviewFactory` recovers it only from
// the `{"session": N}` params JSON its `createView`/`updateParams` receive.
// A caller wraps the returned pointer as an `AVCaptureSession` via
// `Unmanaged<AVCaptureSession>.fromOpaque(ptr).takeUnretainedValue()` —
// ownership stays with the Rust-side session; the returned pointer is valid
// only while that session is open (`CameraSession::close`d elsewhere
// invalidates it — never cache it past a single `createView`/`updateParams`
// call). A session id with no matching open session returns NULL.
void *frust_camera_session_handle(int32_t session);

#endif /* FRUST_CAMERA_H */
