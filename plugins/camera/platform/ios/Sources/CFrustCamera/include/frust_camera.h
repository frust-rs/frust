// The `frust-camera` plugin's C FFI surface, consumed from Swift as
// `import CFrustCamera` — the plugin-package mirror of
// `crates/frust-shell-ios/platform/ios/FrustEmbedding/Sources/CFrustFFI/include/frust_ffi.h`.
//
// The symbol below is DECLARED here and DEFINED nowhere in this package: it
// is a `#[no_mangle] extern "C"` export of `plugins/camera/src/apple.rs`,
// linked in from the SAME Rust staticlib the consuming app's
// "Build Rust staticlib" run-script phase already produces for
// `CFrustFFI`'s exports — a second C target resolving symbols from one
// staticlib. This compiles on this host; whether the symbol actually
// survives dependency-crate stripping through to the app's final Xcode
// link (as opposed to falling back to the exports macro below) is
// unverified pending a physical-device build. An undefined symbol in this
// target's object file is by design
// (Xcode links a package target to a relocatable `.o`, so it resolves at
// the app's final link, same as `CFrustFFI`).
//
// Signature is fixed by contract — keep it in sync with
// `plugins/camera/src/apple.rs`'s `frust_camera_session_handle` export.
// ⚠️ Symbol-survival caveat: a
// dependency-crate `#[no_mangle]` export may be stripped under
// `lto = "fat"`/`strip = "symbols"`; the fallback is the
// `frust_camera::ios_exports!()` macro the app crate can invoke instead —
// which one survives is unverified without a physical-device build.

#ifndef FRUST_CAMERA_H
#define FRUST_CAMERA_H

#include <stdint.h>

// Returns a RETAINED (+1) `AVCaptureSession *` for the open `frust-camera`
// session identified by `session` — an opaque handle id
// `plugins/camera/src/lib.rs`'s `CameraSession` never exposes directly to
// Swift; `CameraPreviewFactory` recovers it only from the `{"session": N}`
// params JSON its `createView`/`updateParams` receive.
//
// OWNERSHIP IS TRANSFERRED (the `CFBridgingRetain` idiom): the callee takes
// the retain, the caller owns it and must release it exactly once — from
// Swift, `Unmanaged<AVCaptureSession>.fromOpaque(ptr).takeRetainedValue()`
// (ARC then owns the object); from C/ObjC, `CFRelease`/`release`. Dropping
// the pointer without releasing leaks the session object; taking it
// *unretained* leaks the callee's retain instead.
//
// It is +1 rather than a +0 borrow because the Rust side can close a session
// from ANY thread, and this caller cannot observe that close: a +0 pointer
// could be freed between this call returning and the caller retaining it
// (use-after-free). The returned object therefore stays valid until released,
// even if its session is closed meanwhile — a closed session simply stops
// running, and the caller re-asks on its next `createView`/`updateParams`.
// A session id with no matching open session returns NULL (which owns
// nothing and must not be released).
void *frust_camera_session_handle(int32_t session);

#endif /* FRUST_CAMERA_H */
