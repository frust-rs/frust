// Ported from subsecond 0.7.10 (DioxusLabs, MIT OR Apache-2.0),
// https://github.com/DioxusLabs/dioxus/tree/main/packages/subsecond.
//
//! Android patch loader. The linker namespace of a non-rooted app refuses to `dlopen` a library
//! outside the app's native-library directory, and a patch arrives elsewhere (e.g. a temp dir). So
//! the library's bytes are copied into an anonymous memfd, mapped executable, and opened by file
//! descriptor through `android_dlopen_ext(ANDROID_DLEXT_USE_LIBRARY_FD)`.
//!
//! References: <https://developer.android.com/ndk/reference/group/libdl> and
//! <https://developer.android.com/ndk/reference/structandroid/dlextinfo>.

use std::ffi::{CStr, c_void};
use std::os::fd::{AsRawFd, IntoRawFd};
use std::path::Path;
use std::ptr;

use crate::PatchError;

/// `android_dlextinfo` from `<android/dlext.h>`.
#[repr(C)]
struct ExtInfo {
    flags: u64,
    reserved_addr: *const c_void,
    reserved_size: libc::size_t,
    relro_fd: libc::c_int,
    library_fd: libc::c_int,
    library_fd_offset: libc::off64_t,
    library_namespace: *const c_void,
}

/// `ANDROID_DLEXT_USE_LIBRARY_FD`: load from `library_fd` instead of opening `filename`.
const ANDROID_DLEXT_USE_LIBRARY_FD: u64 = 0x10;

unsafe extern "C" {
    fn android_dlopen_ext(
        filename: *const libc::c_char,
        flags: libc::c_int,
        ext_info: *const ExtInfo,
    ) -> *const c_void;
}

/// Load the library at `file` through a memfd copy.
///
/// # Safety
///
/// As [`load_patch_library`](crate::load_patch_library): loading runs the library's initialisers,
/// so `file` must be a library built to be loaded into this process.
pub(crate) unsafe fn memfd_dlopen(file: &Path) -> Result<libloading::Library, PatchError> {
    let memfd_err =
        |what: &str, e: &dyn std::fmt::Display| PatchError::AndroidMemfd(format!("{what}: {e}"));

    let contents = std::fs::read(file).map_err(|e| memfd_err("Failed to read file", &e))?;
    let mfd = memfd::MemfdOptions::default()
        .create("frust-hotpatch")
        .map_err(|e| memfd_err("Failed to create memfd", &e))?;
    mfd.as_file()
        .set_len(contents.len() as u64)
        .map_err(|e| memfd_err("Failed to set memfd length", &e))?;

    // `mfd` owns the descriptor until `android_dlopen_ext` succeeds: every error return below
    // drops it, closing the fd, so a failed patch leaks nothing.

    // SAFETY: `mfd` is a fresh memfd sized to `contents` that nothing else maps or resizes.
    let mut map = unsafe { memmap2::MmapMut::map_mut(mfd.as_file()) }
        .map_err(|e| memfd_err("Failed to map memfd", &e))?;
    map.copy_from_slice(&contents);
    // Held until the library is loaded, then unmapped; the loader maps the fd itself.
    let _exec_map = map
        .make_exec()
        .map_err(|e| memfd_err("Failed to make memfd executable", &e))?;

    let info = ExtInfo {
        flags: ANDROID_DLEXT_USE_LIBRARY_FD,
        reserved_addr: ptr::null(),
        reserved_size: 0,
        relro_fd: 0,
        library_fd: mfd.as_raw_fd(),
        library_fd_offset: 0,
        library_namespace: ptr::null(),
    };
    let flags = libloading::os::unix::RTLD_LAZY | libloading::os::unix::RTLD_LOCAL;

    let handle = libloading::os::unix::with_dlerror(
        || {
            // SAFETY: the name is NUL-terminated, `info` outlives the call, and `info.library_fd`
            // is `mfd`'s descriptor, still open because `mfd` is alive; the caller vouches for
            // the library's initialisers.
            let handle = unsafe { android_dlopen_ext(c"/frust-hotpatch".as_ptr(), flags, &info) };
            (!handle.is_null()).then_some(handle)
        },
        |err: &CStr| err.to_string_lossy().into_owned(),
    )
    .map_err(|e| {
        PatchError::AndroidMemfd(format!(
            "android_dlopen_ext failed: {}",
            e.unwrap_or_default()
        ))
    })?;

    // Loaded: the descriptor is now deliberately leaked, as the library stays mapped for the
    // process life.
    let _ = mfd.into_raw_fd();

    // SAFETY: `handle` is a live handle `android_dlopen_ext` just returned; ownership moves into
    // the `Library`.
    let lib = unsafe { libloading::os::unix::Library::from_raw(handle.cast_mut()) };
    Ok(lib.into())
}
