// DO NOT DELETE — this file is not dead code, it is a build requirement.
//
// `CFrustFFI` is a header-only C target (it declares the `frust_*` FFI surface
// and defines nothing), but Xcode's SwiftPM integration unconditionally expects
// a `<CTarget>.o` product for every package target. With `Sources/CFrustFFI/`
// containing only `include/`, `swift build` is happy but `xcodebuild` — the
// tool that actually builds a generated frust app — hard-fails the consuming
// app's link step with:
//
//     error: Build input file cannot be found: '…/CFrustFFI.o'.
//
// One compilable translation unit is enough to fix it, so this file exists
// purely to be that translation unit. It emits no `frust_*` definitions
// (`nm CFrustFFI.o | grep -c frust` → 0), so it cannot shadow the symbols the
// Rust staticlib provides at the app's final link.
#include "include/frust_ffi.h"
