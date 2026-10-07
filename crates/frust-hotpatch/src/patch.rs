// Ported from subsecond 0.7.10 (DioxusLabs, MIT OR Apache-2.0),
// https://github.com/DioxusLabs/dioxus/tree/main/packages/subsecond.
//
//! The load side: install a patch library, rebase its jump table onto this process, publish it to
//! every [`HotFn`](crate::HotFn), and notify registered handlers.

use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(any(unix, windows))]
use std::path::Path;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::JumpTable;
use crate::hot_fn::reset_fall_throughs;

// The installed table, a leaked `Box`, published with a Release store and read with an Acquire
// load (one `ldar` on aarch64, a plain load on x86). Patches are never freed, so a reader can never
// observe a dangling pointer; a reader racing a swap sees the previous table for one more call.
static APP_JUMP_TABLE: AtomicPtr<JumpTable> = AtomicPtr::new(std::ptr::null_mut());

type Handler = Arc<dyn Fn() + Send + Sync + 'static>;
static HOTRELOAD_HANDLERS: Mutex<Vec<Handler>> = Mutex::new(Vec::new());

// Serialises `apply_patch` end to end: load, rebase, publish, handlers. Nothing is published before
// a step that can panic, so a lock poisoned by a panicking loader guards no torn state and is
// recovered.
#[cfg(any(unix, windows))]
static APPLY_LOCK: Mutex<()> = Mutex::new(());

/// Why a patch could not be applied. On every error nothing is installed and no handler runs.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum PatchError {
    /// The patch library failed to load, or lacks the `main` anchor symbol. A `String` rather than
    /// the loader's error type keeps `libloading` out of the public API.
    #[error("Failed to load library: {0}")]
    Dlopen(String),

    /// The Android memfd loader failed, most likely on a permissions issue.
    #[error("Failed to load library on Android: {0}")]
    AndroidMemfd(String),

    /// This executable's `main` anchor could not be resolved ([`aslr_reference`] returned 0), so
    /// the table's keys cannot be rebased onto this process. Checked before the library is loaded.
    #[error("cannot resolve this executable's `main` anchor; the jump table cannot be rebased")]
    AnchorUnresolved,
}

/// Register `handler` to run right after each patch library is loaded and its jump table installed.
///
/// It runs on the thread that called [`apply_patch`], while `apply_patch` still holds its lock: a
/// handler must not apply a patch itself. A panicking handler is caught and logged; the patch stays
/// installed and the remaining handlers still run.
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
    let ptr = APP_JUMP_TABLE.load(Ordering::Acquire);
    // SAFETY: a non-null pointer was produced by `Box::into_raw` in `commit_patch` and is never
    // freed, so it is valid for the rest of the process. This Acquire load pairs with that
    // Release store, so every write that built the table (the `HashMap` allocation and entries)
    // happens-before this read, and the table is never written after publication.
    unsafe { ptr.as_ref() }
}

/// Publish `table`, reset the fall-through diagnostics, and run every handler.
fn commit_patch(table: JumpTable) {
    // Release: pairs with `get_jump_table`'s Acquire load (see the SAFETY note there).
    APP_JUMP_TABLE.store(Box::into_raw(Box::new(table)), Ordering::Release);
    reset_fall_throughs();
    run_handlers();
}

/// Run every registered handler once, each under `catch_unwind`; returns how many panicked.
fn run_handlers() -> usize {
    // Snapshot first: a handler may itself register another handler.
    let handlers = HOTRELOAD_HANDLERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let mut panicked = 0;
    for handler in &handlers {
        if catch_unwind(AssertUnwindSafe(|| handler())).is_err() {
            panicked += 1;
            eprintln!(
                "frust-hotpatch: a patch handler panicked; the patch stays installed and the \
                 remaining handlers still run"
            );
        }
    }
    panicked
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
/// Calls are serialised end to end: a concurrent call waits for the running one to finish,
/// handlers included. Fails closed with [`PatchError::AnchorUnresolved`], before loading anything,
/// when this executable's `main` anchor cannot be resolved. On any error nothing is installed and
/// no handler runs.
///
/// # Safety
///
/// This detours live functions to code from another binary. A malformed or mismatched table (one
/// not built against this exact executable) makes calls jump to arbitrary addresses with the wrong
/// signatures. Only apply tables produced by the patch builder for this running build.
///
/// A well-formed table built against this executable is not enough on its own. Entries are matched
/// by symbol name, and a name does not encode layout, so the caller must also ensure that every
/// mapped function's argument, return and capture types keep their layout between the running
/// image and the patch. Otherwise the patched function reads and writes live values through the
/// new layout: the measured case is a component's `build` taking `(&C, &mut C::State)` after a
/// field grew `C::State` from 4 to 8 bytes (the hot-patch spike's RESULTS.md, row D2). Closures
/// dispatched through [`HotFn::current`](crate::HotFn::current) carry their captures as an
/// argument, so a changed capture is the same hazard. Checking this is the patch builder's job
/// (the spike's PORT.md, section 2(c)); this function cannot see layouts.
///
/// It loads a library and allocates, so it must not run where the process is stopped (e.g. in a
/// signal handler), nor from a patch handler (it would wait on its own lock).
#[cfg(any(unix, windows))]
pub unsafe fn apply_patch(table: JumpTable) -> Result<(), PatchError> {
    // SAFETY: forwarded verbatim; the caller upholds this function's contract.
    unsafe { apply_patch_with_anchor(table, aslr_reference) }
}

/// [`apply_patch`] with the executable's anchor lookup injected, so a test can force it unresolved.
///
/// # Safety
///
/// As [`apply_patch`].
#[cfg(any(unix, windows))]
unsafe fn apply_patch_with_anchor(
    mut table: JumpTable,
    anchor: fn() -> usize,
) -> Result<(), PatchError> {
    let _serial = APPLY_LOCK.lock().unwrap_or_else(PoisonError::into_inner);

    // `main` anchors both images: the running binary's slide and the patch's load address. The
    // patch builder exports `main` from every patch library. Rebasing against an unresolved (0)
    // anchor would key every entry on a wrong address, so refuse before loading anything.
    let anchor = anchor();
    if anchor == 0 {
        return Err(PatchError::AnchorUnresolved);
    }
    let old_offset = anchor.wrapping_sub(table.aslr_reference as usize);

    // SAFETY: the caller guarantees `table.lib` is the patch library built for this process.
    let lib = unsafe { load_patch_library(&table.lib)? };
    // Never unload a patch: its code stays reachable through the table, and dropping a handle has
    // been seen to run teardown code that crashes the process.
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));

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

/// The runtime address of this executable's `main`, the anchor for its ASLR slide, or 0 when it
/// cannot be resolved. The patch builder sends `main`'s link-time address as
/// [`JumpTable::aslr_reference`].
///
/// Resolved on first success and cached (an unresolved 0 is retried); call it from the main
/// executable first, never first from a shared library.
#[doc(hidden)]
pub fn aslr_reference() -> usize {
    static MAIN_PTR: AtomicUsize = AtomicUsize::new(0);

    let cached = MAIN_PTR.load(Ordering::Acquire);
    if cached != 0 {
        return cached;
    }
    let main = main_address();
    if main != 0 {
        MAIN_PTR.store(main, Ordering::Release);
    }
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

#[cfg(not(any(unix, windows)))]
fn main_address() -> usize {
    0
}

/// Test-only control over the process-global table and handlers.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use super::{APP_JUMP_TABLE, HOTRELOAD_HANDLERS, Ordering, commit_patch};
    use crate::JumpTable;
    use crate::hot_fn::reset_fall_throughs;

    static SERIAL: Mutex<()> = Mutex::new(());

    /// Hold for a test's whole body: the table, handlers and miss count are process-global.
    pub(crate) fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Uninstall the table (leaking it, as patches always are), drop every handler, zero misses.
    pub(crate) fn reset() {
        APP_JUMP_TABLE.store(std::ptr::null_mut(), Ordering::Release);
        HOTRELOAD_HANDLERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        reset_fall_throughs();
    }

    /// Publish `table` verbatim (no library load, no rebase) and run the handlers.
    pub(crate) fn install(table: JumpTable) {
        commit_patch(table);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;

    use super::*;

    fn table(entries: &[(u64, u64)]) -> JumpTable {
        JumpTable {
            lib: "/nonexistent/frust-hotpatch-test.so".into(),
            map: entries.iter().copied().collect(),
            aslr_reference: 0x1000,
            new_base_address: 0x2000,
            ifunc_count: 0,
        }
    }

    fn counting_handler() -> (Arc<AtomicU32>, Handler) {
        let count = Arc::new(AtomicU32::new(0));
        let seen = Arc::clone(&count);
        (
            count,
            Arc::new(move || {
                seen.fetch_add(1, Ordering::SeqCst);
            }),
        )
    }

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

    #[test]
    fn handler_runs_once_per_patch() {
        let _serial = test_support::serial();
        test_support::reset();
        let (count, handler) = counting_handler();
        register_handler(handler);

        test_support::install(table(&[]));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        test_support::install(table(&[]));
        assert_eq!(count.load(Ordering::SeqCst), 2);

        test_support::reset();
    }

    #[test]
    fn panicking_handler_does_not_abort_the_patch() {
        let _serial = test_support::serial();
        test_support::reset();
        register_handler(Arc::new(|| panic!("handler failure under test")));
        let (count, handler) = counting_handler();
        register_handler(handler);

        commit_patch(table(&[(1, 2)]));
        assert_eq!(count.load(Ordering::SeqCst), 1, "later handlers still run");
        // SAFETY: the reference is used only within this test, under the serial lock.
        let installed = unsafe { get_jump_table() }.map(|t| t.map.get(&1).copied());
        assert_eq!(installed, Some(Some(2)), "the patch stays installed");

        // `run_handlers` reports the panic it caught.
        assert_eq!(run_handlers(), 1);
        test_support::reset();
    }

    #[test]
    fn poisoned_handler_lock_is_recovered() {
        let _serial = test_support::serial();
        test_support::reset();
        let _ = std::thread::spawn(|| {
            let _held = HOTRELOAD_HANDLERS.lock();
            panic!("poisoning the handler lock under test");
        })
        .join();
        assert!(HOTRELOAD_HANDLERS.is_poisoned());

        let (count, handler) = counting_handler();
        register_handler(handler);
        test_support::install(table(&[]));
        assert_eq!(count.load(Ordering::SeqCst), 1);

        HOTRELOAD_HANDLERS.clear_poison();
        test_support::reset();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn poisoned_apply_lock_is_recovered() {
        let _serial = test_support::serial();
        test_support::reset();
        let _ = std::thread::spawn(|| {
            let _held = APPLY_LOCK.lock();
            panic!("poisoning the apply lock under test");
        })
        .join();
        assert!(APPLY_LOCK.is_poisoned());

        // The poisoned lock is taken, not panicked on: the call reaches the loader and reports it.
        // SAFETY: the library does not exist, so nothing is loaded or installed.
        let err = unsafe { apply_patch_with_anchor(table(&[(1, 2)]), || 0x1000) };
        assert!(matches!(
            err,
            Err(PatchError::Dlopen(_) | PatchError::AndroidMemfd(_))
        ));
        // SAFETY: only checked for presence.
        assert!(unsafe { get_jump_table() }.is_none());

        APPLY_LOCK.clear_poison();
        test_support::reset();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn unresolved_anchor_fails_closed() {
        let _serial = test_support::serial();
        test_support::reset();
        let (count, handler) = counting_handler();
        register_handler(handler);

        // The library path does not exist: an `AnchorUnresolved` (not a load error) proves the
        // anchor is checked before anything is loaded.
        // SAFETY: nothing is loaded or installed when the anchor is unresolved.
        let err = unsafe { apply_patch_with_anchor(table(&[(0x1100, 0x2100)]), || 0) };
        assert_eq!(err, Err(PatchError::AnchorUnresolved));
        // SAFETY: only checked for presence.
        assert!(unsafe { get_jump_table() }.is_none(), "nothing installed");
        assert_eq!(count.load(Ordering::SeqCst), 0, "no handler fired");

        test_support::reset();
    }
}
