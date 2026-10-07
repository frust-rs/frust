//! The app-owned ASLR anchor and the registry of loaded images.
//!
//! The anchor is a function the app defines under the fixed symbol [`ANCHOR_SYMBOL`] and registers
//! with [`set_anchor`]. The patch builder sends that symbol's link-time address as
//! [`JumpTable::aslr_reference`](crate::JumpTable::aslr_reference) and exports the same symbol from
//! every patch library, so the running binary's slide and each patch's load address are recovered
//! without `main` (an Android cdylib defines none) and without `dlsym`.
//!
//! The registry maps an address back to the image it belongs to: image 0 is the base executable
//! and image `n` the n-th patch library this crate loaded. It lets the missed-key diagnostics name
//! the calling image (see [`MissedKey`](crate::MissedKey)).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

/// The symbol every patch library exports and the app defines: `#[unsafe(no_mangle)] pub extern
/// "C" fn __frust_hotpatch_anchor() {}`.
pub const ANCHOR_SYMBOL: &str = "__frust_hotpatch_anchor";

/// The image index reported for an address that lies in no image this crate knows.
pub const UNKNOWN_IMAGE: u32 = u32::MAX;

static ANCHOR: AtomicUsize = AtomicUsize::new(0);

/// Register the running executable's anchor: the address of its `__frust_hotpatch_anchor`
/// function, taken as a function pointer (`__frust_hotpatch_anchor as usize`).
///
/// Call it once at startup, from the main executable's own code. Until it is called with a
/// non-zero address, [`apply_patch`](crate::apply_patch) refuses with
/// [`PatchError::AnchorUnresolved`](crate::PatchError::AnchorUnresolved) before loading anything.
/// A later call replaces the registered address.
pub fn set_anchor(address: usize) {
    ANCHOR.store(address, Ordering::Release);
}

/// The registered anchor address, or 0 while none is set. Nothing is cached beyond the stored
/// value: an unset anchor reads as 0 on every call and is retried by the next [`apply_patch`](
/// crate::apply_patch), so a late [`set_anchor`] takes effect.
#[doc(hidden)]
pub fn aslr_reference() -> usize {
    ANCHOR.load(Ordering::Acquire)
}

/// One loaded image's address range and slide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Image {
    pub(crate) index: u32,
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// Runtime address minus link-time address.
    pub(crate) slide: usize,
}

static IMAGES: Mutex<Vec<Image>> = Mutex::new(Vec::new());

fn images() -> std::sync::MutexGuard<'static, Vec<Image>> {
    IMAGES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Record the base executable (image 0), replacing any earlier record.
pub(crate) fn register_base(start: usize, end: usize, slide: usize) {
    let mut images = images();
    images.retain(|image| image.index != 0);
    images.insert(
        0,
        Image {
            index: 0,
            start,
            end,
            slide,
        },
    );
}

/// Record a newly loaded patch image and return its index (1 for the first patch).
pub(crate) fn register_patch(start: usize, end: usize, slide: usize) -> u32 {
    let mut images = images();
    let index = images.iter().map(|image| image.index).max().unwrap_or(0) + 1;
    images.push(Image {
        index,
        start,
        end,
        slide,
    });
    index
}

/// The image `address` lies in, as `(index, link_address)` where `link_address` is the address
/// minus that image's slide; an address in no known image yields `(UNKNOWN_IMAGE, address)`.
pub(crate) fn classify(address: u64) -> (u32, u64) {
    let found = images()
        .iter()
        .copied()
        .find(|image| usize::try_from(address).is_ok_and(|a| image.start <= a && a < image.end));
    match found {
        Some(image) => (image.index, address.wrapping_sub(image.slide as u64)),
        None => (UNKNOWN_IMAGE, address),
    }
}

/// The `[start, end)` extent of the loaded image containing `address`, or `None` when the platform
/// cannot say.
pub(crate) fn image_range_containing(address: usize) -> Option<(usize, usize)> {
    platform::image_range_containing(address)
}

#[cfg(test)]
pub(crate) fn reset_for_test() {
    ANCHOR.store(0, Ordering::Release);
    images().clear();
}

#[cfg(test)]
pub(crate) fn register_for_test(index: u32, start: usize, end: usize, slide: usize) {
    let mut images = images();
    images.retain(|image| image.index != index);
    images.push(Image {
        index,
        start,
        end,
        slide,
    });
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod platform {
    use std::ffi::c_void;

    struct Probe {
        address: usize,
        found: Option<(usize, usize)>,
    }

    unsafe extern "C" fn visit(
        info: *mut libc::dl_phdr_info,
        _size: libc::size_t,
        data: *mut c_void,
    ) -> libc::c_int {
        // SAFETY: `dl_iterate_phdr` passes a valid `dl_phdr_info` and the `data` pointer given to
        // it below, a live `Probe`.
        let (info, probe) = unsafe { (&*info, &mut *data.cast::<Probe>()) };
        if info.dlpi_phdr.is_null() {
            return 0;
        }
        // SAFETY: the loader guarantees `dlpi_phdr` points at `dlpi_phnum` program headers.
        let headers =
            unsafe { std::slice::from_raw_parts(info.dlpi_phdr, usize::from(info.dlpi_phnum)) };
        let base = info.dlpi_addr as usize;
        let (mut lo, mut hi) = (usize::MAX, 0usize);
        for header in headers.iter().filter(|h| h.p_type == libc::PT_LOAD) {
            let start = base.wrapping_add(header.p_vaddr as usize);
            lo = lo.min(start);
            hi = hi.max(start.wrapping_add(header.p_memsz as usize));
        }
        if lo <= probe.address && probe.address < hi {
            probe.found = Some((lo, hi));
            return 1;
        }
        0
    }

    pub(super) fn image_range_containing(address: usize) -> Option<(usize, usize)> {
        // aarch64 Android tags the top byte of heap pointers; code addresses are untagged but the
        // mask keeps the comparison honest for either.
        #[cfg(target_pointer_width = "64")]
        let address = address & 0x00FF_FFFF_FFFF_FFFF;
        let mut probe = Probe {
            address,
            found: None,
        };
        // SAFETY: `visit` matches the callback type and `probe` outlives the call.
        unsafe {
            libc::dl_iterate_phdr(Some(visit), std::ptr::from_mut(&mut probe).cast::<c_void>());
        }
        probe.found
    }
}

#[cfg(all(target_vendor = "apple", target_pointer_width = "64"))]
mod platform {
    // Declared here rather than taken from `libc`, which deprecates its dyld and Mach-O items in
    // favour of the `mach2` crate; two functions and three small structs are not worth a dependency.
    unsafe extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_header(index: u32) -> *const MachHeader64;
        fn _dyld_get_image_vmaddr_slide(index: u32) -> isize;
    }

    #[repr(C)]
    struct MachHeader64 {
        magic: u32,
        cpu_type: i32,
        cpu_subtype: i32,
        file_type: u32,
        command_count: u32,
        commands_size: u32,
        flags: u32,
        reserved: u32,
    }

    #[repr(C)]
    struct LoadCommand {
        cmd: u32,
        size: u32,
    }

    #[repr(C)]
    struct Segment64 {
        cmd: u32,
        size: u32,
        name: [u8; 16],
        vm_address: u64,
        vm_size: u64,
    }

    const LC_SEGMENT_64: u32 = 0x19;

    pub(super) fn image_range_containing(address: usize) -> Option<(usize, usize)> {
        // SAFETY: the dyld image-list functions may be called from any thread; an index below the
        // count yields a valid header (or null) that stays mapped, since this crate never unloads
        // an image and system images are immortal.
        let count = unsafe { _dyld_image_count() };
        for index in 0..count {
            // SAFETY: as above.
            let (header, slide) = unsafe {
                (
                    _dyld_get_image_header(index),
                    _dyld_get_image_vmaddr_slide(index) as usize,
                )
            };
            if header.is_null() {
                continue;
            }
            // SAFETY: a non-null header from dyld is a mapped 64-bit Mach-O header.
            let range = unsafe { segments_extent(header, slide) };
            if let Some((lo, hi)) = range
                && lo <= address
                && address < hi
            {
                return Some((lo, hi));
            }
        }
        None
    }

    /// The extent of the image's mapped segments, `__PAGEZERO` (a guard region, not code) excluded.
    ///
    /// # Safety
    ///
    /// `header` must point at a mapped 64-bit Mach-O header.
    unsafe fn segments_extent(header: *const MachHeader64, slide: usize) -> Option<(usize, usize)> {
        // SAFETY: forwarded from this function's contract; the load commands follow the header,
        // `command_count` of them, each `size` bytes long.
        let (count, mut cursor) = unsafe { ((*header).command_count, header.add(1).cast::<u8>()) };
        let (mut lo, mut hi) = (usize::MAX, 0usize);
        for _ in 0..count {
            // SAFETY: `cursor` walks the load-command list by each command's own size.
            let command = unsafe { &*cursor.cast::<LoadCommand>() };
            if command.cmd == LC_SEGMENT_64 {
                // SAFETY: an `LC_SEGMENT_64` command starts with the `Segment64` fields.
                let segment = unsafe { &*cursor.cast::<Segment64>() };
                if !segment.name.starts_with(b"__PAGEZERO") && segment.vm_size != 0 {
                    let start = (segment.vm_address as usize).wrapping_add(slide);
                    lo = lo.min(start);
                    hi = hi.max(start.wrapping_add(segment.vm_size as usize));
                }
            }
            if command.size == 0 {
                break;
            }
            // SAFETY: the next command begins `size` bytes on, within the header's command area.
            cursor = unsafe { cursor.add(command.size as usize) };
        }
        (lo < hi).then_some((lo, hi))
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;

    unsafe extern "system" {
        fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut *mut c_void) -> i32;
    }

    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;

    pub(super) fn image_range_containing(address: usize) -> Option<(usize, usize)> {
        let mut module: *mut c_void = std::ptr::null_mut();
        // SAFETY: FROM_ADDRESS treats `address` as a pointer to look up and never dereferences it;
        // UNCHANGED_REFCOUNT leaves the module's reference count alone.
        let ok = unsafe {
            GetModuleHandleExW(
                FROM_ADDRESS | UNCHANGED_REFCOUNT,
                address as *const c_void,
                &mut module,
            )
        };
        if ok == 0 || module.is_null() {
            return None;
        }
        let base = module as usize;
        // SAFETY: an HMODULE is the image base, which starts with a DOS header whose `e_lfanew`
        // (offset 0x3C) locates the NT headers; `SizeOfImage` is 56 bytes into the optional
        // header, itself 24 bytes (signature + file header) into the NT headers.
        let size = unsafe {
            let e_lfanew = std::ptr::read_unaligned((base + 0x3C) as *const u32) as usize;
            std::ptr::read_unaligned((base + e_lfanew + 24 + 56) as *const u32) as usize
        };
        Some((base, base + size))
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    all(target_vendor = "apple", target_pointer_width = "64"),
    windows
)))]
mod platform {
    pub(super) fn image_range_containing(_address: usize) -> Option<(usize, usize)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here() {}

    #[test]
    fn anchor_reads_zero_until_set_and_is_not_cached_while_unset() {
        let _serial = crate::patch::test_support::serial();
        reset_for_test();
        assert_eq!(aslr_reference(), 0);
        assert_eq!(aslr_reference(), 0);
        set_anchor(0x1234);
        assert_eq!(aslr_reference(), 0x1234);
        reset_for_test();
    }

    #[test]
    fn classify_names_the_image_and_subtracts_its_slide() {
        let _serial = crate::patch::test_support::serial();
        reset_for_test();
        register_for_test(0, 0x1000, 0x2000, 0x1000);
        register_for_test(1, 0x8000, 0x9000, 0x7000);
        assert_eq!(classify(0x1800), (0, 0x800));
        assert_eq!(classify(0x8100), (1, 0x1100));
        assert_eq!(classify(0x9000), (UNKNOWN_IMAGE, 0x9000));
        reset_for_test();
    }

    #[test]
    fn patch_images_are_numbered_in_load_order() {
        let _serial = crate::patch::test_support::serial();
        reset_for_test();
        register_base(0x1000, 0x2000, 0);
        assert_eq!(register_patch(0x4000, 0x5000, 0), 1);
        assert_eq!(register_patch(0x6000, 0x7000, 0), 2);
        assert_eq!(classify(0x6100).0, 2);
        reset_for_test();
    }

    #[test]
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        all(target_vendor = "apple", target_pointer_width = "64"),
        windows
    ))]
    fn the_running_executable_has_a_range_containing_its_own_code() {
        let address = here as *const () as usize;
        let (lo, hi) = image_range_containing(address).expect("a range for this test binary");
        assert!(lo <= address && address < hi);
    }
}
