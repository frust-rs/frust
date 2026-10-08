// Ported from subsecond 0.7.10 (DioxusLabs, MIT OR Apache-2.0),
// https://github.com/DioxusLabs/dioxus/tree/main/packages/subsecond.
//
//! The load side: install a patch library, rebase its jump table onto this process, publish it to
//! every [`HotFn`](crate::HotFn), and notify registered handlers.

use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(any(unix, windows))]
use std::path::Path;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::JumpTable;
#[cfg(any(unix, windows))]
use crate::anchor::{
    ANCHOR_SYMBOL, aslr_reference, image_range_containing, image_slide_containing, register_base,
    register_patch,
};
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
    /// The patch library failed to load, or lacks the `__frust_hotpatch_anchor` symbol. A `String` rather than
    /// the loader's error type keeps `libloading` out of the public API.
    #[error("Failed to load library: {0}")]
    Dlopen(String),

    /// The Android memfd loader failed, most likely on a permissions issue.
    #[error("Failed to load library on Android: {0}")]
    AndroidMemfd(String),

    /// No anchor is set ([`set_anchor`](crate::set_anchor) was never called with a non-zero
    /// address), so the table's keys cannot be rebased onto this process. Checked before the
    /// library is loaded.
    #[error("no ASLR anchor is set; the jump table cannot be rebased")]
    AnchorUnresolved,

    /// This is a release-profile build: patches are applied only under `debug_assertions`.
    #[error("hot patching is available only in builds with debug assertions")]
    ReleaseBuild,

    /// The platform could not say which loaded image holds an anchor, so missed keys could not be
    /// attributed to an image. Nothing is installed.
    #[error("cannot determine the address range of a loaded image")]
    ImageRangeUnresolved,

    /// Layout-mismatch records are unreported (see [`report_layout_mismatch`]): no patch is
    /// applied over a known mismatch. Carries the unreported records; checked under the apply lock
    /// before anything is loaded, and cleared by [`mark_layout_mismatches_reported`].
    #[error("{} unreported layout mismatch(es); patch refused", .0.len())]
    LayoutMismatchPending(Vec<LayoutMismatch>),

    /// The table's anchor address for `image` (`"base"` or `"patch"`) is not that image's
    /// `__frust_hotpatch_anchor` link-time address: the offset it implies (`got`) differs from
    /// the image's real slide (`expected`). A table anchored on another symbol (dx 0.7.10 anchors
    /// on `main`) is refused rather than rebased by a wrong constant. The `"base"` check runs
    /// before the library is loaded; for `"patch"` the library is already mapped and leaked
    /// (never unloaded), as with [`PatchError::ImageRangeUnresolved`] after the load. Nothing is
    /// registered or installed.
    #[error("{image} anchor mismatch: table implies offset {got:#x}, image slide is {expected:#x}")]
    AnchorMismatch {
        /// Which image's anchor disagrees: `"base"` or `"patch"`.
        image: &'static str,
        /// The image's real slide, from the platform loader.
        expected: u64,
        /// The offset the table's anchor address implies.
        got: u64,
    },
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
/// handlers included. Fails closed, before loading anything: a release-profile build
/// ([`PatchError::ReleaseBuild`]), no anchor set with [`set_anchor`](crate::set_anchor)
/// ([`PatchError::AnchorUnresolved`]), an unlocatable base image
/// ([`PatchError::ImageRangeUnresolved`]), or any unreported layout-mismatch record
/// ([`PatchError::LayoutMismatchPending`], checked under the lock, so every apply entry refuses
/// the same way until the host calls [`mark_layout_mismatches_reported`]). The patch library must export
/// [`ANCHOR_SYMBOL`](crate::ANCHOR_SYMBOL), else [`PatchError::Dlopen`]. On any error nothing is
/// installed and no handler runs.
///
/// A table whose anchor addresses are not those of [`ANCHOR_SYMBOL`](crate::ANCHOR_SYMBOL) is
/// refused with [`PatchError::AnchorMismatch`]: the offset implied by `aslr_reference` must equal
/// the running image's slide, and the one implied by `new_base_address` the patch image's slide.
/// The base check runs before the library is loaded; a patch-side refusal leaves the library
/// mapped and leaked.
///
/// # Safety
///
/// This detours live functions to code from another binary. A malformed or mismatched table (one
/// not built against this exact executable) makes calls jump to arbitrary addresses with the wrong
/// signatures. Only apply tables produced by the patch builder for this running build.
///
/// Two anchor preconditions hold the rebase honest. Both are checked at run time
/// ([`PatchError::AnchorMismatch`]) but stated here because a table is trusted input:
/// `table.aslr_reference` must be the link-time address of `__frust_hotpatch_anchor` in the
/// running base image, and `table.new_base_address` its link-time address in the patch library.
/// A table anchored on any other symbol (for example `main`, as dx 0.7.10 builds it) is not valid.
///
/// A well-formed table built against this executable is not enough on its own. Entries are matched
/// by symbol name, and a name does not encode layout, so the caller must also ensure that every
/// mapped function's argument, return and capture types keep their layout between the running
/// image and the patch: every type whose values cross images keeps its layout. Otherwise the
/// patched function reads and writes live values through the new layout: the measured case is a
/// component's `build` taking `(&C, &mut C::State)` after a field grew `C::State` from 4 to 8
/// bytes (the hot-patch spike's RESULTS.md, row D2). Closures dispatched through
/// [`HotFn::current`](crate::HotFn::current) carry their captures as an argument, so a changed
/// capture is the same hazard. Checking this is the patch builder's job (its L3 gate, the spike's
/// PORT.md section 2(c)); this function cannot see layouts.
///
/// Refuses with [`PatchError::LayoutMismatchPending`] while any layout-mismatch record is
/// unreported ([`report_layout_mismatch`]); the caller must report the records to the host and call
/// [`mark_layout_mismatches_reported`] before a patch can apply.
///
/// It loads a library and allocates, so it must not run where the process is stopped (e.g. in a
/// signal handler), nor from a patch handler (it would wait on its own lock).
#[cfg(any(unix, windows))]
pub unsafe fn apply_patch(table: JumpTable) -> Result<(), PatchError> {
    if !cfg!(debug_assertions) {
        return Err(PatchError::ReleaseBuild);
    }
    // SAFETY: forwarded verbatim; the caller upholds this function's contract.
    unsafe { apply_patch_with_anchor(table, aslr_reference) }
}

/// What [`apply_from_devtools`] did: whether the patch was applied, and why not when it was not.
#[cfg(any(unix, windows))]
#[derive(Debug, PartialEq)]
pub struct ApplyReport {
    /// The patch is installed and its handlers have run.
    pub applied: bool,
    /// The unreported layout-mismatch records that made the call refuse (empty otherwise). The
    /// caller carries them in its answer and then marks them with
    /// [`mark_layout_mismatches_reported`].
    pub layout_mismatches: Vec<LayoutMismatch>,
    /// Why applying failed, when it did (and no mismatch record was the reason).
    pub error: Option<PatchError>,
}

/// The safe devtools entry: apply the patch library at `bytes_path` with `table`.
///
/// The one `unsafe` call of the devtools path lives here so the shell crates stay `unsafe`-free.
/// `bytes_path` must be a file the app itself wrote from bytes received on the authenticated
/// devtools connection, never a path taken from the wire: this function overwrites `table.lib`
/// with it and trusts it. That, and an authenticated peer that is the patch builder for this very
/// build, are the preconditions the devtools layer owes [`apply_patch`]'s safety contract.
///
/// Refuses, loading nothing and answering `applied: false` with the records, while any
/// layout-mismatch record is unreported (see [`report_layout_mismatch`]): [`apply_patch`] itself
/// refuses with [`PatchError::LayoutMismatchPending`] under its lock, and this entry maps that to
/// the report. A
/// release-profile build refuses the same way, with [`PatchError::ReleaseBuild`].
#[cfg(any(unix, windows))]
pub fn apply_from_devtools(bytes_path: &Path, mut table: JumpTable) -> ApplyReport {
    if !cfg!(debug_assertions) {
        return ApplyReport {
            applied: false,
            layout_mismatches: Vec::new(),
            error: Some(PatchError::ReleaseBuild),
        };
    }
    table.lib = bytes_path.to_path_buf();
    // SAFETY: this entry's documented contract: `bytes_path` was written by the app from bytes
    // received on the authenticated devtools connection, and the table comes from the same
    // authenticated builder for this running build, which is what `apply_patch` requires. The
    // builder's L3 gate owns the layout precondition.
    match unsafe { apply_patch(table) } {
        Ok(()) => ApplyReport {
            applied: true,
            layout_mismatches: Vec::new(),
            error: None,
        },
        Err(PatchError::LayoutMismatchPending(records)) => ApplyReport {
            applied: false,
            layout_mismatches: records,
            error: None,
        },
        Err(error) => ApplyReport {
            applied: false,
            layout_mismatches: Vec::new(),
            error: Some(error),
        },
    }
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

    // Under the lock and before any load: a mismatch reported while this call waited is seen, and
    // none can slip between this check and the install.
    let pending = pending_layout_mismatches();
    if !pending.is_empty() {
        return Err(PatchError::LayoutMismatchPending(pending));
    }

    // The app's anchor symbol anchors both images: the running binary's slide and the patch's load
    // address. The patch builder exports it from every patch library. Rebasing against an
    // unresolved (0) anchor would key every entry on a wrong address, so refuse before loading
    // anything.
    let anchor = anchor();
    if anchor == 0 {
        return Err(PatchError::AnchorUnresolved);
    }
    let old_offset = anchor.wrapping_sub(table.aslr_reference as usize);
    let (base_start, base_end) =
        image_range_containing(anchor).ok_or(PatchError::ImageRangeUnresolved)?;
    // The table's anchor must be this image's anchor symbol, so the offset it implies is exactly
    // the image's slide. Checked before the load: a wrong table costs nothing.
    let base_slide = image_slide_containing(anchor).ok_or(PatchError::ImageRangeUnresolved)?;
    if old_offset != base_slide {
        return Err(PatchError::AnchorMismatch {
            image: "base",
            expected: base_slide as u64,
            got: old_offset as u64,
        });
    }

    // SAFETY: the caller guarantees `table.lib` is the patch library built for this process.
    let lib = unsafe { load_patch_library(&table.lib)? };
    // Never unload a patch: its code stays reachable through the table, and dropping a handle has
    // been seen to run teardown code that crashes the process.
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));

    // SAFETY: the anchor is only read as an address, never called through this symbol.
    let symbol = unsafe { lib.get::<*const ()>(ANCHOR_SYMBOL.as_bytes()) }.map_err(|err| {
        PatchError::Dlopen(format!(
            "patch library has no `{ANCHOR_SYMBOL}` anchor: {err}"
        ))
    })?;
    // SAFETY: the symbol came from `lib`, which is leaked and so outlives the raw pointer; the
    // pointer is only used for address arithmetic.
    let patch_anchor = unsafe { symbol.try_as_raw_ptr() }.ok_or_else(|| {
        PatchError::Dlopen(format!("patch library's `{ANCHOR_SYMBOL}` anchor is null"))
    })?;
    let new_offset = patch_anchor.wrapping_byte_sub(table.new_base_address as usize) as usize;
    let (patch_start, patch_end) =
        image_range_containing(patch_anchor as usize).ok_or(PatchError::ImageRangeUnresolved)?;
    // The library is already mapped and leaked here; nothing is registered or installed.
    let patch_slide =
        image_slide_containing(patch_anchor as usize).ok_or(PatchError::ImageRangeUnresolved)?;
    if new_offset != patch_slide {
        return Err(PatchError::AnchorMismatch {
            image: "patch",
            expected: patch_slide as u64,
            got: new_offset as u64,
        });
    }

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

    // Registered before the table is published, so a miss right after publication resolves.
    register_base(base_start, base_end, old_offset);
    register_patch(patch_start, patch_end, new_offset);
    commit_patch(table);
    Ok(())
}

/// A layout disagreement found at run time: a value built by one image was reached by another
/// whose own layout for `type_name` differs.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LayoutMismatch {
    /// The mismatched type's name.
    pub type_name: String,
    /// The layout recorded by the creating image, as text (e.g. its size).
    pub stored: String,
    /// The layout the reporting image computed for itself.
    pub own: String,
}

impl std::fmt::Display for LayoutMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: stored {}, own {}",
            self.type_name, self.stored, self.own
        )
    }
}

static LAYOUT_MISMATCHES: Mutex<Vec<LayoutMismatch>> = Mutex::new(Vec::new());

/// Record that `type_name` has layout `stored` in the image that created a value and `own` in the
/// image now handling it. The record is kept until it is reported to the host: it is returned by
/// [`pending_layout_mismatches`] and cleared by [`mark_layout_mismatches_reported`]. Identical
/// records collapse into one. While any record is unreported, every apply entry
/// ([`apply_patch`], [`apply_from_devtools`]) refuses.
///
/// Callable from any thread, including from a `Drop`; it never panics.
pub fn report_layout_mismatch(
    type_name: &str,
    stored: impl std::fmt::Display,
    own: impl std::fmt::Display,
) {
    let record = LayoutMismatch {
        type_name: type_name.to_owned(),
        stored: stored.to_string(),
        own: own.to_string(),
    };
    let mut records = LAYOUT_MISMATCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if !records.contains(&record) {
        records.push(record);
    }
}

/// The layout-mismatch records not yet reported, oldest first (for `hotpatch_info`).
pub fn pending_layout_mismatches() -> Vec<LayoutMismatch> {
    LAYOUT_MISMATCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Mark `reported` as carried by a `PatchOutcome` or `hotpatch_info` answer. Only the records
/// given are removed, so one reported after the caller read the list stays pending.
pub fn mark_layout_mismatches_reported(reported: &[LayoutMismatch]) {
    LAYOUT_MISMATCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|record| !reported.contains(record));
}

/// Test-only control over the process-global table and handlers.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use super::{APP_JUMP_TABLE, HOTRELOAD_HANDLERS, LAYOUT_MISMATCHES, Ordering, commit_patch};
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
        crate::anchor::reset_for_test();
        LAYOUT_MISMATCHES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// Publish `table` verbatim (no library load, no rebase) and run the handlers.
    pub(crate) fn install(table: JumpTable) {
        commit_patch(table);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;

    use crate::anchor::UNKNOWN_IMAGE;

    use super::*;

    fn table(entries: &[(u64, u64)]) -> JumpTable {
        JumpTable {
            lib: "/nonexistent/frust-hotpatch-test.so".into(),
            map: entries.iter().copied().collect(),
            // The test anchor's link-time address in this test binary, as a correct table has it.
            aslr_reference: (test_anchor() - slide_of_test_anchor()) as u64,
            new_base_address: 0x2000,
            ifunc_count: 0,
        }
    }

    /// A real code address in this test binary, standing in for the app's registered anchor.
    fn test_anchor_fn() {}

    fn test_anchor() -> usize {
        test_anchor_fn as *const () as usize
    }

    fn slide_of_test_anchor() -> usize {
        crate::anchor::image_slide_containing(test_anchor()).unwrap_or(0)
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
        let err = unsafe { apply_patch_with_anchor(table(&[(1, 2)]), test_anchor) };
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

    #[cfg(any(unix, windows))]
    #[test]
    #[cfg_attr(
        not(debug_assertions),
        ignore = "the release profile refuses before the anchor; see release_profile_refuses"
    )]
    fn apply_refuses_until_an_anchor_is_set_then_proceeds() {
        let _serial = test_support::serial();
        test_support::reset();

        // SAFETY: the library does not exist, so nothing is loaded or installed.
        let err = unsafe { apply_patch(table(&[(0x1100, 0x2100)])) };
        assert_eq!(err, Err(PatchError::AnchorUnresolved), "no anchor yet");

        crate::set_anchor(test_anchor());
        // SAFETY: as above.
        let err = unsafe { apply_patch(table(&[(0x1100, 0x2100)])) };
        assert!(
            matches!(
                err,
                Err(PatchError::Dlopen(_) | PatchError::AndroidMemfd(_))
            ),
            "with an anchor the call reaches the loader: {err:?}"
        );
        test_support::reset();
    }

    #[cfg(any(unix, windows))]
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "covers the release profile; run `cargo test --release -p frust-hotpatch`"
    )]
    fn release_profile_refuses() {
        let _serial = test_support::serial();
        test_support::reset();
        crate::set_anchor(test_anchor());
        // SAFETY: a release build refuses before touching the table.
        let err = unsafe { apply_patch(table(&[])) };
        assert_eq!(err, Err(PatchError::ReleaseBuild));
        let report = apply_from_devtools(Path::new("/nonexistent/x.so"), table(&[]));
        assert!(!report.applied);
        assert_eq!(report.error, Some(PatchError::ReleaseBuild));
        test_support::reset();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn debug_profile_does_not_refuse_with_release_build() {
        if !cfg!(debug_assertions) {
            return;
        }
        let _serial = test_support::serial();
        test_support::reset();
        crate::set_anchor(test_anchor());
        // SAFETY: the library does not exist, so nothing is loaded.
        let err = unsafe { apply_patch(table(&[])) };
        assert_ne!(err, Err(PatchError::ReleaseBuild));
        test_support::reset();
    }

    /// A system library that certainly loads and certainly lacks the anchor symbol.
    #[cfg(any(unix, windows))]
    fn anchorless_library() -> &'static str {
        if cfg!(target_vendor = "apple") {
            "/usr/lib/libSystem.B.dylib"
        } else if cfg!(windows) {
            "kernel32.dll"
        } else {
            "libc.so.6"
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "apply_patch refuses in release")]
    fn library_without_the_anchor_symbol_is_refused() {
        let _serial = test_support::serial();
        test_support::reset();
        crate::set_anchor(test_anchor());
        let mut t = table(&[(0x1100, 0x2100)]);
        t.lib = anchorless_library().into();
        // SAFETY: a system library is loaded; its initialisers are the platform's own.
        let err = unsafe { apply_patch(t) };
        match err {
            Err(PatchError::Dlopen(msg)) => assert!(msg.contains(ANCHOR_SYMBOL), "{msg}"),
            other => panic!("expected Dlopen naming the anchor, got {other:?}"),
        }
        // SAFETY: only checked for presence.
        assert!(unsafe { get_jump_table() }.is_none());
        test_support::reset();
    }

    /// Build a real cdylib exporting the anchor symbol, so the first end-to-end apply with a
    /// non-`main` anchor runs here. Skipped (loudly) only when `rustc` cannot be spawned.
    #[cfg(any(unix, windows))]
    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "apply_patch refuses in release")]
    fn real_patch_library_applies_through_the_anchor_symbol() {
        let _serial = test_support::serial();
        test_support::reset();

        let dir = std::env::temp_dir().join(format!("frust-hotpatch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let src = dir.join("patch.rs");
        std::fs::write(
            &src,
            "#[unsafe(no_mangle)] pub extern \"C\" fn __frust_hotpatch_anchor() {}\n\
             #[unsafe(no_mangle)] pub extern \"C\" fn patched() -> u32 { 7 }\n",
        )
        .expect("write patch source");
        let ext = if cfg!(windows) {
            "dll"
        } else if cfg!(target_vendor = "apple") {
            "dylib"
        } else {
            "so"
        };
        let lib = dir.join(format!("patch.{ext}"));
        let status = std::process::Command::new("rustc")
            .args([
                "--edition=2024",
                "--crate-type=cdylib",
                "--crate-name=patch",
                "-o",
            ])
            .arg(&lib)
            .arg(&src)
            .status();
        let Ok(status) = status else {
            eprintln!("rustc is not spawnable; real-library apply test skipped");
            return;
        };
        assert!(status.success(), "building the patch library failed");

        crate::set_anchor(test_anchor());
        // The link-time anchor addresses a correct table carries: runtime address minus slide,
        // the patch's read by mapping the library (the loader returns the same handle later).
        let base_slide = crate::anchor::image_slide_containing(test_anchor()).expect("base slide");
        let good_old = test_anchor().wrapping_sub(base_slide) as u64;
        // SAFETY: the library was just built by this test and has no initialisers; the handle is
        // leaked, as `apply_patch` leaks its own.
        let probe: &'static libloading::Library = Box::leak(Box::new(
            unsafe { load_patch_library(&lib) }.expect("map patch"),
        ));
        // SAFETY: the symbol is only read as an address.
        let symbol = unsafe { probe.get::<*const ()>(ANCHOR_SYMBOL.as_bytes()) }.expect("anchor");
        // SAFETY: the library is leaked, so the pointer is only used for address arithmetic.
        let patch_anchor = unsafe { symbol.try_as_raw_ptr() }.expect("non-null anchor") as usize;
        let patch_slide = crate::anchor::image_slide_containing(patch_anchor).expect("patch slide");
        let good_new = patch_anchor.wrapping_sub(patch_slide) as u64;
        let build = |old: u64, new: u64| {
            let mut t = table(&[(0x1100, 0x2100)]);
            t.lib = lib.clone();
            t.aslr_reference = old;
            t.new_base_address = new;
            t
        };

        // A table anchored elsewhere (dx anchors on `main`) is refused before anything installs.
        // SAFETY: the library was just built by this test and has no initialisers.
        let wrong_base = unsafe { apply_patch(build(good_old + 0x10, good_new)) };
        assert_eq!(
            wrong_base,
            Err(PatchError::AnchorMismatch {
                image: "base",
                expected: base_slide as u64,
                got: base_slide as u64 - 0x10,
            })
        );
        // SAFETY: as above.
        let wrong_patch = unsafe { apply_patch(build(good_old, good_new + 0x10)) };
        assert_eq!(
            wrong_patch,
            Err(PatchError::AnchorMismatch {
                image: "patch",
                expected: patch_slide as u64,
                got: patch_slide as u64 - 0x10,
            })
        );
        // SAFETY: only checked for presence.
        assert!(unsafe { get_jump_table() }.is_none(), "nothing installed");
        assert_eq!(
            crate::anchor::classify(test_anchor() as u64).0,
            UNKNOWN_IMAGE
        );

        // SAFETY: as above.
        unsafe { apply_patch(build(good_old, good_new)) }.expect("apply through the anchor symbol");

        let old_offset = base_slide;
        // SAFETY: used within this test under the serial lock.
        let installed = unsafe { get_jump_table() }.expect("table installed");
        assert_eq!(installed.map.len(), 1);
        let (key, value) = installed.map.iter().next().expect("one entry");
        assert_eq!(*key, (0x1100usize.wrapping_add(old_offset)) as u64);
        assert_ne!(
            *value, 0x2100,
            "the target was rebased onto the loaded image"
        );
        // The patch is image 1, the executable image 0.
        assert_eq!(crate::anchor::classify(test_anchor() as u64).0, 0);
        assert_eq!(crate::anchor::classify(*value).0, 1);

        test_support::reset();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(any(unix, windows))]
    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "apply_patch refuses in release")]
    fn raw_apply_patch_refuses_while_a_mismatch_is_unreported_then_proceeds() {
        let _serial = test_support::serial();
        test_support::reset();
        crate::set_anchor(test_anchor());

        report_layout_mismatch("RawState", 4, 8);
        let expected = pending_layout_mismatches();
        // SAFETY: the library path does not exist, so nothing can be loaded.
        let refused = unsafe { apply_patch(table(&[(1, 2)])) };
        assert_eq!(
            refused,
            Err(PatchError::LayoutMismatchPending(expected.clone()))
        );
        // SAFETY: only checked for presence.
        assert!(unsafe { get_jump_table() }.is_none());

        mark_layout_mismatches_reported(&expected);
        // SAFETY: as above; the loader is reached and fails on the missing file.
        let proceeded = unsafe { apply_patch(table(&[(1, 2)])) };
        assert!(
            matches!(
                proceeded,
                Err(PatchError::Dlopen(_) | PatchError::AndroidMemfd(_))
            ),
            "once reported the raw apply reaches the loader: {proceeded:?}"
        );
        test_support::reset();
    }

    #[cfg(any(unix, windows))]
    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "apply_patch refuses in release")]
    fn devtools_apply_refuses_while_a_mismatch_is_unreported() {
        let _serial = test_support::serial();
        test_support::reset();
        crate::set_anchor(test_anchor());

        report_layout_mismatch("HomeState", 4, 8);
        report_layout_mismatch("HomeState", 4, 8);
        let expected = LayoutMismatch {
            type_name: "HomeState".into(),
            stored: "4".into(),
            own: "8".into(),
        };
        assert_eq!(
            pending_layout_mismatches(),
            vec![expected.clone()],
            "deduplicated"
        );

        // The library path does not exist: `Dlopen` would mean the load was attempted.
        let report = apply_from_devtools(Path::new("/nonexistent/x.so"), table(&[(1, 2)]));
        assert!(!report.applied);
        assert_eq!(report.layout_mismatches, vec![expected.clone()]);
        assert_eq!(report.error, None, "refused for the record, not an error");
        // SAFETY: only checked for presence.
        assert!(unsafe { get_jump_table() }.is_none());

        // A record reported after the list was read stays pending.
        report_layout_mismatch("Other", 1, 2);
        mark_layout_mismatches_reported(&report.layout_mismatches);
        assert_eq!(pending_layout_mismatches().len(), 1);
        mark_layout_mismatches_reported(&pending_layout_mismatches());
        assert!(pending_layout_mismatches().is_empty());

        let report = apply_from_devtools(Path::new("/nonexistent/x.so"), table(&[(1, 2)]));
        assert!(report.layout_mismatches.is_empty());
        assert!(
            matches!(
                report.error,
                Some(PatchError::Dlopen(_) | PatchError::AndroidMemfd(_))
            ),
            "once reported the apply proceeds to the loader: {report:?}"
        );
        assert_eq!(
            LayoutMismatch {
                type_name: "T".into(),
                stored: "1".into(),
                own: "2".into()
            }
            .to_string(),
            "T: stored 1, own 2"
        );
        test_support::reset();
    }
}
