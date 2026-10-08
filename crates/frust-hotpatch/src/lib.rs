// Ported from subsecond 0.7.10 (DioxusLabs, MIT OR Apache-2.0),
// https://github.com/DioxusLabs/dioxus/tree/main/packages/subsecond.
//
//! Frust's native-only hot-patch runtime: the in-app half of hot patching.
//!
//! Wrap a patch point in [`HotFn`] (or the free [`call`]). In a debug build every call does one
//! lookup in the installed [`JumpTable`], keyed on the address of the function's
//! [`HotFunction::call_it`] monomorphisation (or, for [`HotFn::from_fn_ptr`], the pointer's own
//! value), and jumps to the patched version when one is mapped, else runs the original. There is
//! no stale-call detection and no retry. A patch builder (today `dx` 0.7.10, whose wire format
//! [`JumpTable`] matches) compiles the changed code into a library and sends the table;
//! [`apply_patch`] loads that library, rebases the table onto this process, publishes it, and runs
//! every [`register_handler`] handler. Patches are serialised, and a patch is refused before
//! anything is loaded in a release build, while no anchor is set ([`set_anchor`]; the app defines
//! the [`ANCHOR_SYMBOL`] function), and, in every apply entry ([`apply_patch`] and
//! [`apply_from_devtools`]), while a layout-mismatch record is unreported
//! ([`report_layout_mismatch`]; checked under the apply lock, [`PatchError::LayoutMismatchPending`]).
//!
//! The layout precondition: every type whose values cross images (a mapped function's arguments,
//! return and closure captures, a component's state) must keep its layout between the running
//! image and a patch. Symbol names do not encode layout, so nothing here can check it; the patch
//! builder's L3 gate does, and the runtime reports a disagreement it meets anyway.
//!
//! The patch boundary: nothing rewrites process memory, so the table only redirects calls that
//! enter through a `HotFn`. Patch code is also reached without one, though: a trait object, stored
//! closure or fn pointer created while patched code ran points straight into the patch library,
//! and calling it later runs that code with no `HotFn` in between (including after a newer patch).
//! Likewise such values created before a patch keep running the old code until they are rebuilt.
//!
//! Lookups happen only under `cfg!(debug_assertions)`. In a release build every `HotFn` call is a
//! direct call and the table is never read. See the crate README for scope and differences.

#[cfg(target_arch = "wasm32")]
compile_error!("frust-hotpatch is native-only: wasm32 patch loading is not ported");

mod anchor;
#[cfg(target_os = "android")]
mod android;
mod hot_fn;
mod jump_table;
mod patch;

#[doc(hidden)]
pub use anchor::aslr_reference;
pub use anchor::{ANCHOR_SYMBOL, UNKNOWN_IMAGE, set_anchor};
pub use hot_fn::{
    FnPointer, HotFn, HotFnPtr, HotFunction, MissedKey, call, fall_through_count,
    last_call_fell_through, missed_keys, seam_hits,
};
pub use jump_table::{AddressHasher, AddressMap, BuildAddressHasher, JumpTable};
#[cfg(any(unix, windows))]
pub use patch::{ApplyReport, apply_from_devtools, apply_patch, load_patch_library};
pub use patch::{
    LayoutMismatch, PatchError, get_jump_table, mark_layout_mismatches_reported,
    pending_layout_mismatches, register_handler, report_layout_mismatch,
};
