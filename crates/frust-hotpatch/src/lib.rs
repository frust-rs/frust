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
//! every [`register_handler`] handler. Patches are serialised, and a patch whose `main` anchor
//! cannot be resolved is refused before anything is loaded.
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

#[cfg(target_os = "android")]
mod android;
mod hot_fn;
mod jump_table;
mod patch;

pub use hot_fn::{
    FnPointer, HotFn, HotFnPtr, HotFunction, call, fall_through_count, last_call_fell_through,
};
pub use jump_table::{AddressHasher, AddressMap, BuildAddressHasher, JumpTable};
#[doc(hidden)]
pub use patch::aslr_reference;
pub use patch::{PatchError, get_jump_table, register_handler};
#[cfg(any(unix, windows))]
pub use patch::{apply_patch, load_patch_library};
