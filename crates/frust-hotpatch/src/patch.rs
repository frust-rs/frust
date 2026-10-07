// Ported from subsecond 0.7.10 (DioxusLabs, MIT OR Apache-2.0),
// https://github.com/DioxusLabs/dioxus/tree/main/packages/subsecond.
//
//! The load side: install a patch library, rebase its jump table onto this process, publish it to
//! every [`HotFn`](crate::HotFn), and notify registered handlers.

use std::path::Path;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::JumpTable;
use crate::hot_fn::reset_fall_throughs;

// The installed table, a leaked `Box`. Reading it is one relaxed atomic load: the hot path pays
// nothing for it. Patches are applied rarely and never freed, so a reader can never observe a
// dangling pointer; a reader racing a swap simply sees the previous table for one more call.
static APP_JUMP_TABLE: AtomicPtr<JumpTable> = AtomicPtr::new(std::ptr::null_mut());

type Handler = Arc<dyn Fn() + Send + Sync + 'static>;
static HOTRELOAD_HANDLERS: Mutex<Vec<Handler>> = Mutex::new(Vec::new());

/// Why a patch could not be applied.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum PatchError {
    /// The patch library failed to load, or lacks the `main` anchor symbol. A `String` rather than
    /// the loader's error type keeps `libloading` out of the public API.
    #[error("Failed to load library: {0}")]
    Dlopen(String),

    /// The Android memfd loader failed, most likely on a permissions issue.
    #[error("Failed to load library on Android: {0}")]
    AndroidMemfd(String),
}

/// Register `handler` to run right after each patch library is loaded and its jump table installed.
///
/// It runs on the thread that called [`apply_patch`].
pub fn register_handler(handler: Arc<dyn Fn() + Send + Sync + 'static>) {
    HOTRELOAD_HANDLERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(handler);
}

/// The installed jump table, or `None` before the first patch.
///
/// # Safety
///
/// The reference points into a leaked allocation and is valid today for `'static`, but a future
/// version may reclaim tables between patches: use it within one call, never across a patch.
pub unsafe fn get_jump_table() -> Option<&'static JumpTable> {
    let ptr = APP_JUMP_TABLE.load(Ordering::Relaxed);
    // SAFETY: a non-null pointer was produced by `Box::into_raw` in `commit_patch` and is never
    // freed, so it is valid and immutable for the rest of the process.
    unsafe { ptr.as_ref() }
}

fn commit_patch(table: JumpTable) {
    APP_JUMP_TABLE.store(Box::into_raw(Box::new(table)), Ordering::Relaxed);
    reset_fall_throughs();
    // Snapshot first: a handler may itself register another handler.
    let handlers = HOTRELOAD_HANDLERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for handler in &handlers {
        handler();
    }
}

/// Load `path` with the platform loader [`apply_patch`] uses: `dlopen`/`LoadLibrary` through
/// `libloading` on desktop, the memfd + `android_dlopen_ext` path on Android (which lets a library
/// outside the app's native-library directory load on a non-rooted device).
///
/// # Safety
///
/// Loading a library runs its initialisers, which can do anything: `path` must be a library built
/// to be loaded into this process. The returned handle must never be dropped while code from it
/// may still run.
#[cfg(any(unix, windows))]
pub unsafe fn load_patch_library(path: &Path) -> Result<libloading::Library, PatchError> {
    #[cfg(target_os = "android")]
    {
        // SAFETY: forwarded verbatim; the caller upholds this function's contract.
        unsafe { crate::android::memfd_dlopen(path) }
    }

    #[cfg(not(target_os = "android"))]
    {
        // SAFETY: forwarded verbatim; the caller upholds this function's contract.
        unsafe { libloading::Library::new(path) }.map_err(|err| PatchError::Dlopen(err.to_string()))
    }
}

/// Apply a patch: load `table.lib`, rebase `table`'s addresses onto this process and the loaded
/// library, install it, then run every [`register_handler`] handler.
///
/// # Safety
///
/// This detours live functions to code from another binary. A malformed or mismatched table (one
/// not built against this exact executable) makes calls jump to arbitrary addresses with the wrong
/// signatures. Only apply tables produced by the patch builder for this running build.
///
/// It loads a library and allocates, so it must not run where the process is stopped (e.g. in a
/// signal handler).
#[cfg(any(unix, windows))]
pub unsafe fn apply_patch(mut table: JumpTable) -> Result<(), PatchError> {
    // SAFETY: the caller guarantees `table.lib` is the patch library built for this process.
    let lib = unsafe { load_patch_library(&table.lib)? };
    // Never unload a patch: its code stays reachable through the table, and dropping a handle has
    // been seen to run teardown code that crashes the process.
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));

    // `main` anchors both images: the running binary's slide and the patch's load address. The
    // patch builder exports `main` from every patch library.
    let old_offset = aslr_reference().wrapping_sub(table.aslr_reference as usize);
    // SAFETY: `main` is only read as an address, never called through this symbol.
    let main = unsafe { lib.get::<*const ()>(b"main") }
        .map_err(|err| PatchError::Dlopen(format!("patch library has no `main` anchor: {err}")))?;
    // SAFETY: the symbol came from `lib`, which is leaked and so outlives the raw pointer; the
    // pointer is only used for address arithmetic.
    let main = unsafe { main.try_as_raw_ptr() }
        .ok_or_else(|| PatchError::Dlopen("patch library's `main` anchor is null".to_owned()))?;
    let new_offset = main.wrapping_byte_sub(table.new_base_address as usize) as usize;

    table.map = table
        .map
        .iter()
        .map(|(old, new)| {
            (
                (*old as usize).wrapping_add(old_offset) as u64,
                (*new as usize).wrapping_add(new_offset) as u64,
            )
        })
        .collect();

    commit_patch(table);
    Ok(())
}

/// The runtime address of this executable's `main`, the anchor for its ASLR slide. The patch
/// builder sends `main`'s link-time address as [`JumpTable::aslr_reference`].
///
/// Resolved on first use and cached; call it from the main executable first, never first from a
/// shared library.
#[doc(hidden)]
pub fn aslr_reference() -> usize {
    static MAIN_PTR: AtomicUsize = AtomicUsize::new(0);

    let cached = MAIN_PTR.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    let main = main_address();
    MAIN_PTR.store(main, Ordering::Relaxed);
    main
}

#[cfg(unix)]
fn main_address() -> usize {
    // SAFETY: `dlsym` with `RTLD_DEFAULT` and a NUL-terminated name only reads; it returns null
    // when the symbol is absent.
    unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"main".as_ptr()) as usize }
}

#[cfg(windows)]
fn main_address() -> usize {
    unsafe extern "system" {
        fn GetModuleHandleA(module_name: *const i8) -> *mut std::ffi::c_void;
        fn GetProcAddress(
            module: *mut std::ffi::c_void,
            proc_name: *const i8,
        ) -> *mut std::ffi::c_void;
    }
    // SAFETY: a null module name yields the running executable's handle, and `GetProcAddress`
    // with a NUL-terminated name only reads; it returns null when the symbol is absent.
    unsafe { GetProcAddress(GetModuleHandleA(std::ptr::null()), c"main".as_ptr()) as usize }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_library_is_a_load_error() {
        // SAFETY: the path does not exist, so nothing is loaded.
        let err = unsafe { load_patch_library(Path::new("/nonexistent/frust-hotpatch-test.so")) };
        assert!(matches!(
            err,
            Err(PatchError::Dlopen(_) | PatchError::AndroidMemfd(_))
        ));
    }

    #[test]
    fn aslr_reference_is_stable() {
        assert_eq!(aslr_reference(), aslr_reference());
    }
}
