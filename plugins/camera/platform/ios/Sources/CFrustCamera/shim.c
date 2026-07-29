// DO NOT DELETE — this file is not dead code, it is a build requirement.
// Mirrors `platform/ios/FrustEmbedding/Sources/CFrustFFI/shim.c` exactly,
// for the same reason: `CFrustCamera` is a header-only C target (it declares
// the `frust_camera_*` FFI surface and defines nothing), but Xcode's SwiftPM
// integration unconditionally expects a `<CTarget>.o` product for every
// package target. With `Sources/CFrustCamera/` containing only `include/`,
// `swift package dump-package` and `swift build` are happy but `xcodebuild`
// — the tool that actually builds a generated frust app — hard-fails the
// consuming app's link step with:
//
//     error: Build input file cannot be found: '…/CFrustCamera.o'.
//
// One compilable translation unit is enough to fix it, so this file exists
// purely to be that translation unit. It emits no `frust_camera_*`
// definitions (`nm CFrustCamera.o | grep -c frust_camera` -> 0), so it
// cannot shadow the symbol the Rust staticlib provides at the app's final
// link.
#include "include/frust_camera.h"
