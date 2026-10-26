//! Sanctioned unsafe zone (decision D-6): FFI to DirectWrite (windows-sys and windows-link, no `windows` crate).
//! Every `unsafe` block carries a SAFETY note, and the FFI stays behind safe
//! wrappers that never leak past the `Backend` trait. No other oracle module
//! may contain `unsafe`; the guard tests in `oracle/mod.rs` enforce it.
//!
//! The DirectWrite oracle backend (Windows).
//!
//! Compiled only for `windows` (the cfg gate is on this module's `mod` line in
//! `oracle/mod.rs`). `IDWriteFontFallback::MapCharacters` answers which system
//! font draws a cluster; the mapped font's local file loader gives the file
//! path and `IDWriteFontFace::GetIndex` the face index. windows-sys 0.61 ships
//! no DirectWrite module, so every interface is a hand-declared vtable: one
//! marker type per interface, a named constant per vtable slot (the slot test
//! exercises each one against a known string, the guard against the
//! wrong-slot bug class), and a [`ComPtr`] guard that releases on drop.
//!
//! [`preferred_ui_languages`] reads the user's UI language list, from which `oracle/mod.rs`
//! picks the Han language of a Han question that carries no locale (none: `en-US`, as before).
//!
//! The scale factor `MapCharacters` reports travels on `FaceSource::scale`. The
//! PostScript name stays `None`: reading it needs `GetInformationalStrings`, a
//! vtable slot this module does not declare, so it is not cheaply available.

#![allow(unsafe_code)]

use std::ffi::{OsString, c_void};
use std::marker::PhantomData;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr::{self, NonNull};
use std::sync::OnceLock;

use windows_sys::core::{GUID, HRESULT};

use super::{Backend, FaceSource, FallbackRequest};
use crate::style::{FontStyle, GenericSlot};

windows_link::link!("dwrite.dll" "system" fn DWriteCreateFactory(factorytype: i32, iid: *const GUID, factory: *mut *mut c_void) -> HRESULT);

/// `RtlGetVersion` fills an `OSVERSIONINFOW`; declared here so no extra windows-sys feature is needed.
#[repr(C)]
struct OsVersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    csd: [u16; 128],
}
windows_link::link!("ntdll.dll" "system" fn RtlGetVersion(info: *mut OsVersionInfo) -> i32);

// `GetUserPreferredUILanguages` (kernel32), declared here like `RtlGetVersion` so no extra
// windows-sys feature is needed. Writes a double-NUL-terminated list of language names.
windows_link::link!("kernel32.dll" "system" fn GetUserPreferredUILanguages(flags: u32, count: *mut u32, buffer: *mut u16, len: *mut u32) -> i32);

/// `MUI_LANGUAGE_NAME`: list language names (`ja-JP`), not LANGIDs.
const MUI_LANGUAGE_NAME: u32 = 0x8;

/// The user's preferred UI languages, most preferred first (`GetUserPreferredUILanguages`).
pub(super) fn preferred_ui_languages() -> Vec<String> {
    let (mut count, mut len) = (0u32, 0u32);
    // SAFETY: a null buffer with a zero length is the documented size query; both out-pointers
    // are live locals.
    let ok = unsafe {
        GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, ptr::null_mut(), &mut len)
    };
    if ok == 0 || len == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; len as usize];
    // SAFETY: `buffer` is writable for `len` UTF-16 units, the size the query reported, and
    // both out-pointers are live locals; the call writes at most `len` units.
    let ok = unsafe {
        GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buffer.as_mut_ptr(), &mut len)
    };
    if ok == 0 {
        return Vec::new();
    }
    buffer.truncate(len as usize);
    multi_sz(&buffer)
}

/// The strings of a double-NUL-terminated UTF-16 list.
fn multi_sz(units: &[u16]) -> Vec<String> {
    units
        .split(|&unit| unit == 0)
        .filter(|name| !name.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

const IID_IDWRITEFACTORY2: GUID = GUID::from_u128(0x0439fc60_ca44_4994_8dee_3a9af7b732ec);
const IID_IDWRITELOCALFONTFILELOADER: GUID =
    GUID::from_u128(0xb2d9f3ec_c9fe_4a11_a2ec_d86208f7c0a2);
const IID_IDWRITETEXTANALYSISSOURCE: GUID = GUID::from_u128(0x688e1a58_5094_47c8_adc8_fbcea60ae92b);
const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);

const DWRITE_FACTORY_TYPE_SHARED: i32 = 0;
const E_NOINTERFACE: HRESULT = 0x8000_4002_u32 as i32;

/// Vtable slot indices, counted from the interface's first method (IUnknown
/// is 0..=2 on every interface, then base interfaces, then own methods).
mod slots {
    pub(super) const QUERY_INTERFACE: usize = 0;
    pub(super) const ADD_REF: usize = 1;
    pub(super) const RELEASE: usize = 2;

    // IDWriteFactory (3..=23), IDWriteFactory1 (24, 25), IDWriteFactory2 (26).
    pub(super) const FACTORY_GET_SYSTEM_FONT_COLLECTION: usize = 3;
    pub(super) const FACTORY2_GET_SYSTEM_FONT_FALLBACK: usize = 26;

    // IDWriteFontCollection.
    pub(super) const COLLECTION_GET_FONT_FAMILY_COUNT: usize = 3;
    pub(super) const COLLECTION_GET_FONT_FAMILY: usize = 4;
    pub(super) const COLLECTION_FIND_FAMILY_NAME: usize = 5;

    // IDWriteFontFamily : IDWriteFontList (GetFontCollection 3, GetFontCount 4, GetFont 5).
    pub(super) const FAMILY_GET_FAMILY_NAMES: usize = 6;
    pub(super) const FAMILY_GET_FIRST_MATCHING_FONT: usize = 7;

    // IDWriteFont.
    pub(super) const FONT_GET_FONT_FAMILY: usize = 3;
    pub(super) const FONT_GET_WEIGHT: usize = 4;
    pub(super) const FONT_GET_STRETCH: usize = 5;
    pub(super) const FONT_GET_STYLE: usize = 6;
    pub(super) const FONT_HAS_CHARACTER: usize = 12;
    pub(super) const FONT_CREATE_FONT_FACE: usize = 13;

    // IDWriteFontFace.
    pub(super) const FACE_GET_FILES: usize = 4;
    pub(super) const FACE_GET_INDEX: usize = 5;

    // IDWriteFontFile.
    pub(super) const FILE_GET_REFERENCE_KEY: usize = 3;
    pub(super) const FILE_GET_LOADER: usize = 4;

    // IDWriteFontFileLoader (CreateStreamFromKey 3) < IDWriteLocalFontFileLoader.
    pub(super) const LOCAL_LOADER_GET_FILE_PATH_LENGTH: usize = 4;
    pub(super) const LOCAL_LOADER_GET_FILE_PATH: usize = 5;

    // IDWriteLocalizedStrings.
    pub(super) const STRINGS_GET_COUNT: usize = 3;
    pub(super) const STRINGS_FIND_LOCALE_NAME: usize = 4;
    pub(super) const STRINGS_GET_LOCALE_NAME_LENGTH: usize = 5;
    pub(super) const STRINGS_GET_LOCALE_NAME: usize = 6;
    pub(super) const STRINGS_GET_STRING_LENGTH: usize = 7;
    pub(super) const STRINGS_GET_STRING: usize = 8;

    // IDWriteFontFallback.
    pub(super) const FALLBACK_MAP_CHARACTERS: usize = 3;
}

/// Calls vtable slot `$slot` of COM object `$obj` as an `extern "system"` method.
/// Must be used inside an `unsafe` block whose SAFETY note names the slot.
macro_rules! vcall {
    ($obj:expr, $slot:expr, fn($($ty:ty),* $(,)?) -> $ret:ty, $($arg:expr),* $(,)?) => {{
        let obj: *mut c_void = $obj;
        let f: unsafe extern "system" fn(*mut c_void $(, $ty)*) -> $ret = slot_fn(obj, $slot);
        f(obj $(, $arg)*)
    }};
}

/// Reads the function pointer in vtable slot `index` of `obj`.
///
/// # Safety
/// `obj` must point at a live COM object whose vtable has at least `index + 1`
/// entries, and `F` must be that entry's exact `extern "system"` signature.
unsafe fn slot_fn<F: Copy>(obj: *mut c_void, index: usize) -> F {
    debug_assert_eq!(size_of::<F>(), size_of::<usize>());
    // SAFETY: a COM object's first word is its vtable pointer (caller contract);
    // `index` is within the vtable and `F` is a pointer-sized fn pointer.
    unsafe {
        let vtbl = *(obj as *const *const usize);
        std::mem::transmute_copy(&*vtbl.add(index))
    }
}

fn succeeded(hr: HRESULT) -> bool {
    hr >= 0
}

fn guid_eq(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

/// Counts live [`ComPtr`]s so tests can prove every acquired interface was released.
#[cfg(test)]
static LIVE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

// Interface markers: they type a `ComPtr` so a pointer cannot be used through the wrong wrapper.
struct Factory;
struct Collection;
struct Family;
struct Font;
struct Face;
struct File;
struct Loader;
struct LocalLoader;
struct Strings;
struct Fallback;

/// An owned COM interface pointer: Release on drop, AddRef on clone.
struct ComPtr<T>(NonNull<c_void>, PhantomData<T>);

impl<T> ComPtr<T> {
    /// Takes ownership of one reference (an out-parameter that DirectWrite filled).
    fn from_raw(raw: *mut c_void) -> Option<Self> {
        let ptr = NonNull::new(raw)?;
        #[cfg(test)]
        LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(Self(ptr, PhantomData))
    }

    fn raw(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    /// `IUnknown::AddRef` then `Release`, returning the count (a leak probe for tests).
    #[cfg(test)]
    fn ref_count(&self) -> u32 {
        // SAFETY: slots 1 (AddRef) and 2 (Release) exist on every COM object; `self`
        // owns a reference, so the object is alive for both calls.
        unsafe {
            vcall!(self.raw(), slots::ADD_REF, fn() -> u32,);
            vcall!(self.raw(), slots::RELEASE, fn() -> u32,)
        }
    }

    /// `IUnknown::QueryInterface` for `iid`, as a new owned pointer.
    fn query<U>(&self, iid: &GUID) -> Option<ComPtr<U>> {
        let mut out: *mut c_void = ptr::null_mut();
        // SAFETY: slot 0 (QueryInterface) of a live object `self` owns; `iid` and
        // `out` are valid for the call. On S_OK `out` holds one new reference,
        // which `from_raw` takes ownership of.
        let hr = unsafe {
            vcall!(
                self.raw(),
                slots::QUERY_INTERFACE,
                fn(*const GUID, *mut *mut c_void) -> HRESULT,
                iid,
                &mut out
            )
        };
        if succeeded(hr) {
            ComPtr::from_raw(out)
        } else {
            None
        }
    }
}

impl<T> Clone for ComPtr<T> {
    fn clone(&self) -> Self {
        // SAFETY: slot 1 (AddRef) of a live object; the new reference is owned by the clone.
        unsafe { vcall!(self.raw(), slots::ADD_REF, fn() -> u32,) };
        #[cfg(test)]
        LIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(self.0, PhantomData)
    }
}

impl<T> Drop for ComPtr<T> {
    fn drop(&mut self) {
        // SAFETY: slot 2 (Release) drops the one reference this guard owns; the
        // pointer is never used again after drop.
        unsafe { vcall!(self.raw(), slots::RELEASE, fn() -> u32,) };
        #[cfg(test)]
        LIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Runs an out-pointer COM call and wraps the interface it produced.
macro_rules! out_ptr {
    ($call:expr) => {{
        let mut out: *mut c_void = ptr::null_mut();
        let out_ref = &mut out;
        let hr = $call(out_ref as *mut *mut c_void);
        if succeeded(hr) {
            ComPtr::from_raw(out)
        } else {
            None
        }
    }};
}

fn utf16z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl ComPtr<Factory> {
    /// A shared `IDWriteFactory2`.
    fn create() -> Option<Self> {
        let mut out: *mut c_void = ptr::null_mut();
        // SAFETY: `DWriteCreateFactory` (dwrite.dll) with the SHARED type and the
        // IDWriteFactory2 IID; `out` is a valid out-pointer and on success holds
        // one reference that `from_raw` owns.
        let hr = unsafe {
            DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED, &IID_IDWRITEFACTORY2, &mut out)
        };
        if succeeded(hr) {
            Self::from_raw(out)
        } else {
            None
        }
    }

    fn system_collection(&self) -> Option<ComPtr<Collection>> {
        out_ptr!(|p| {
            // SAFETY: IDWriteFactory slot 3 GetSystemFontCollection(out collection,
            // BOOL checkForUpdates) on a live factory; `p` is a valid out-pointer.
            unsafe {
                vcall!(
                    self.raw(),
                    slots::FACTORY_GET_SYSTEM_FONT_COLLECTION,
                    fn(*mut *mut c_void, i32) -> HRESULT,
                    p,
                    0
                )
            }
        })
    }

    fn system_fallback(&self) -> Option<ComPtr<Fallback>> {
        out_ptr!(|p| {
            // SAFETY: IDWriteFactory2 slot 26 GetSystemFontFallback(out fallback) on a
            // live factory; `p` is a valid out-pointer.
            unsafe {
                vcall!(
                    self.raw(),
                    slots::FACTORY2_GET_SYSTEM_FONT_FALLBACK,
                    fn(*mut *mut c_void) -> HRESULT,
                    p
                )
            }
        })
    }
}

impl ComPtr<Collection> {
    fn family_count(&self) -> u32 {
        // SAFETY: IDWriteFontCollection slot 3 GetFontFamilyCount() on a live collection.
        unsafe {
            vcall!(
                self.raw(),
                slots::COLLECTION_GET_FONT_FAMILY_COUNT,
                fn() -> u32,
            )
        }
    }

    fn family(&self, index: u32) -> Option<ComPtr<Family>> {
        out_ptr!(|p| {
            // SAFETY: IDWriteFontCollection slot 4 GetFontFamily(index, out family) on a
            // live collection; `p` is a valid out-pointer.
            unsafe {
                vcall!(
                    self.raw(),
                    slots::COLLECTION_GET_FONT_FAMILY,
                    fn(u32, *mut *mut c_void) -> HRESULT,
                    index,
                    p
                )
            }
        })
    }

    /// Index of the family named `name` (case-insensitive), if the collection has it.
    fn find_family(&self, name: &str) -> Option<u32> {
        let wide = utf16z(name);
        let (mut index, mut exists) = (0u32, 0i32);
        // SAFETY: IDWriteFontCollection slot 5 FindFamilyName(const WCHAR*, UINT32*,
        // BOOL*) on a live collection; `wide` is NUL-terminated and outlives the
        // call, the out-pointers are valid locals.
        let hr = unsafe {
            vcall!(
                self.raw(),
                slots::COLLECTION_FIND_FAMILY_NAME,
                fn(*const u16, *mut u32, *mut i32) -> HRESULT,
                wide.as_ptr(),
                &mut index,
                &mut exists
            )
        };
        (succeeded(hr) && exists != 0).then_some(index)
    }
}

impl ComPtr<Family> {
    fn names(&self) -> Option<ComPtr<Strings>> {
        out_ptr!(|p| {
            // SAFETY: IDWriteFontFamily slot 6 GetFamilyNames(out strings) on a live
            // family; `p` is a valid out-pointer.
            unsafe {
                vcall!(
                    self.raw(),
                    slots::FAMILY_GET_FAMILY_NAMES,
                    fn(*mut *mut c_void) -> HRESULT,
                    p
                )
            }
        })
    }

    fn first_matching_font(&self, weight: i32, stretch: i32, style: i32) -> Option<ComPtr<Font>> {
        out_ptr!(|p| {
            // SAFETY: IDWriteFontFamily slot 7 GetFirstMatchingFont(weight, stretch,
            // style, out font) on a live family; the three enums are plain i32s and
            // `p` is a valid out-pointer.
            unsafe {
                vcall!(
                    self.raw(),
                    slots::FAMILY_GET_FIRST_MATCHING_FONT,
                    fn(i32, i32, i32, *mut *mut c_void) -> HRESULT,
                    weight,
                    stretch,
                    style,
                    p
                )
            }
        })
    }
}

impl ComPtr<Strings> {
    fn count(&self) -> u32 {
        // SAFETY: IDWriteLocalizedStrings slot 3 GetCount() on a live object.
        unsafe { vcall!(self.raw(), slots::STRINGS_GET_COUNT, fn() -> u32,) }
    }

    fn find_locale(&self, locale: &str) -> Option<u32> {
        let wide = utf16z(locale);
        let (mut index, mut exists) = (0u32, 0i32);
        // SAFETY: IDWriteLocalizedStrings slot 4 FindLocaleName(const WCHAR*, UINT32*,
        // BOOL*): `wide` is NUL-terminated and outlives the call; out-pointers are locals.
        let hr = unsafe {
            vcall!(
                self.raw(),
                slots::STRINGS_FIND_LOCALE_NAME,
                fn(*const u16, *mut u32, *mut i32) -> HRESULT,
                wide.as_ptr(),
                &mut index,
                &mut exists
            )
        };
        (succeeded(hr) && exists != 0).then_some(index)
    }

    /// The locale name at `index` (slots 5 and 6, NOT the string slots 7 and 8).
    fn locale_name(&self, index: u32) -> Option<String> {
        let mut len = 0u32;
        // SAFETY: slot 5 GetLocaleNameLength(index, UINT32*) on a live object; `len` is a local.
        let hr = unsafe {
            vcall!(
                self.raw(),
                slots::STRINGS_GET_LOCALE_NAME_LENGTH,
                fn(u32, *mut u32) -> HRESULT,
                index,
                &mut len
            )
        };
        if !succeeded(hr) {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        // SAFETY: slot 6 GetLocaleName(index, WCHAR*, size) writes at most `size`
        // units; `buf` holds `len + 1` (length plus NUL).
        let hr = unsafe {
            vcall!(
                self.raw(),
                slots::STRINGS_GET_LOCALE_NAME,
                fn(u32, *mut u16, u32) -> HRESULT,
                index,
                buf.as_mut_ptr(),
                len + 1
            )
        };
        succeeded(hr).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }

    /// The string at `index` (slots 7 and 8).
    fn string(&self, index: u32) -> Option<String> {
        let mut len = 0u32;
        // SAFETY: slot 7 GetStringLength(index, UINT32*) on a live object; `len` is a local.
        let hr = unsafe {
            vcall!(
                self.raw(),
                slots::STRINGS_GET_STRING_LENGTH,
                fn(u32, *mut u32) -> HRESULT,
                index,
                &mut len
            )
        };
        if !succeeded(hr) {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        // SAFETY: slot 8 GetString(index, WCHAR*, size) writes at most `size` units;
        // `buf` holds `len + 1` (length plus NUL).
        let hr = unsafe {
            vcall!(
                self.raw(),
                slots::STRINGS_GET_STRING,
                fn(u32, *mut u16, u32) -> HRESULT,
                index,
                buf.as_mut_ptr(),
                len + 1
            )
        };
        succeeded(hr).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }

    /// The en-US string, else the first one.
    fn preferred_string(&self) -> Option<String> {
        let index = self.find_locale("en-us").unwrap_or(0);
        self.string(index)
    }
}

impl ComPtr<Font> {
    fn family(&self) -> Option<ComPtr<Family>> {
        out_ptr!(|p| {
            // SAFETY: IDWriteFont slot 3 GetFontFamily(out family) on a live font; `p` is a valid out-pointer.
            unsafe {
                vcall!(
                    self.raw(),
                    slots::FONT_GET_FONT_FAMILY,
                    fn(*mut *mut c_void) -> HRESULT,
                    p
                )
            }
        })
    }

    fn weight(&self) -> i32 {
        // SAFETY: IDWriteFont slot 4 GetWeight() on a live font.
        unsafe { vcall!(self.raw(), slots::FONT_GET_WEIGHT, fn() -> i32,) }
    }

    fn stretch(&self) -> i32 {
        // SAFETY: IDWriteFont slot 5 GetStretch() on a live font.
        unsafe { vcall!(self.raw(), slots::FONT_GET_STRETCH, fn() -> i32,) }
    }

    fn style(&self) -> i32 {
        // SAFETY: IDWriteFont slot 6 GetStyle() on a live font.
        unsafe { vcall!(self.raw(), slots::FONT_GET_STYLE, fn() -> i32,) }
    }

    fn has_character(&self, c: char) -> bool {
        let mut has = 0i32;
        // SAFETY: IDWriteFont slot 12 HasCharacter(UINT32 codepoint, BOOL*) on a live font; `has` is a local.
        let hr = unsafe {
            vcall!(
                self.raw(),
                slots::FONT_HAS_CHARACTER,
                fn(u32, *mut i32) -> HRESULT,
                c as u32,
                &mut has
            )
        };
        succeeded(hr) && has != 0
    }

    fn create_face(&self) -> Option<ComPtr<Face>> {
        out_ptr!(|p| {
            // SAFETY: IDWriteFont slot 13 CreateFontFace(out face) on a live font; `p` is a valid out-pointer.
            unsafe {
                vcall!(
                    self.raw(),
                    slots::FONT_CREATE_FONT_FACE,
                    fn(*mut *mut c_void) -> HRESULT,
                    p
                )
            }
        })
    }

    /// The font's local file path and face index.
    fn location(&self) -> Option<(PathBuf, u32)> {
        self.create_face()?.location()
    }
}

impl ComPtr<Face> {
    /// The face's first file (a face may span several; local system fonts have one).
    fn first_file(&self) -> Option<ComPtr<File>> {
        let mut count = 1u32;
        let mut file: *mut c_void = ptr::null_mut();
        // SAFETY: IDWriteFontFace slot 4 GetFiles(UINT32* count, IDWriteFontFile** out):
        // `count` is 1 on input so at most one pointer is written into `file`, a
        // valid one-element out-slot; the reference it receives is owned by `from_raw`.
        let hr = unsafe {
            vcall!(
                self.raw(),
                slots::FACE_GET_FILES,
                fn(*mut u32, *mut *mut c_void) -> HRESULT,
                &mut count,
                &mut file
            )
        };
        if succeeded(hr) {
            ComPtr::from_raw(file)
        } else {
            None
        }
    }

    fn index(&self) -> u32 {
        // SAFETY: IDWriteFontFace slot 5 GetIndex() on a live face.
        unsafe { vcall!(self.raw(), slots::FACE_GET_INDEX, fn() -> u32,) }
    }

    fn location(&self) -> Option<(PathBuf, u32)> {
        let file = self.first_file()?;
        let path = file.local_path()?;
        Some((path, self.index()))
    }
}

impl ComPtr<File> {
    /// The path of a file backed by the local file loader, `None` for any other loader.
    fn local_path(&self) -> Option<PathBuf> {
        let mut key: *const c_void = ptr::null();
        let mut key_size = 0u32;
        // SAFETY: IDWriteFontFile slot 3 GetReferenceKey(const void** key, UINT32*
        // size) on a live file; the key points into DirectWrite-owned storage that
        // stays valid while `self` (and so the file) is alive.
        let hr = unsafe {
            vcall!(
                self.raw(),
                slots::FILE_GET_REFERENCE_KEY,
                fn(*mut *const c_void, *mut u32) -> HRESULT,
                &mut key,
                &mut key_size
            )
        };
        if !succeeded(hr) {
            return None;
        }
        let loader: ComPtr<Loader> = out_ptr!(|p| {
            // SAFETY: IDWriteFontFile slot 4 GetLoader(out loader) on a live file; `p` is a valid out-pointer.
            unsafe {
                vcall!(
                    self.raw(),
                    slots::FILE_GET_LOADER,
                    fn(*mut *mut c_void) -> HRESULT,
                    p
                )
            }
        })?;
        let local: ComPtr<LocalLoader> = loader.query(&IID_IDWRITELOCALFONTFILELOADER)?;
        let mut len = 0u32;
        // SAFETY: IDWriteLocalFontFileLoader slot 4 GetFilePathLengthFromKey(key,
        // size, UINT32* len); `key`/`key_size` came from the file above, which is
        // still alive.
        let hr = unsafe {
            vcall!(
                local.raw(),
                slots::LOCAL_LOADER_GET_FILE_PATH_LENGTH,
                fn(*const c_void, u32, *mut u32) -> HRESULT,
                key,
                key_size,
                &mut len
            )
        };
        if !succeeded(hr) {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        // SAFETY: slot 5 GetFilePathFromKey(key, size, WCHAR* path, UINT32 cap)
        // writes at most `cap` units; `buf` holds `len + 1` (length plus NUL).
        let hr = unsafe {
            vcall!(
                local.raw(),
                slots::LOCAL_LOADER_GET_FILE_PATH,
                fn(*const c_void, u32, *mut u16, u32) -> HRESULT,
                key,
                key_size,
                buf.as_mut_ptr(),
                len + 1
            )
        };
        succeeded(hr).then(|| PathBuf::from(OsString::from_wide(&buf[..len as usize])))
    }
}

/// A Rust-implemented `IDWriteTextAnalysisSource` over one UTF-16 string.
/// It lives on the caller's stack for one `MapCharacters` call, so its
/// reference counting is a constant and nothing frees it.
#[repr(C)]
struct TextSource {
    /// Must stay the first field: COM reads the vtable through the object pointer.
    vtbl: *const SourceVtbl,
    text: Vec<u16>,
    /// NUL-terminated BCP 47 locale.
    locale: Vec<u16>,
}

#[repr(C)]
struct SourceVtbl {
    query_interface:
        unsafe extern "system" fn(*mut TextSource, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut TextSource) -> u32,
    release: unsafe extern "system" fn(*mut TextSource) -> u32,
    get_text_at_position:
        unsafe extern "system" fn(*mut TextSource, u32, *mut *const u16, *mut u32) -> HRESULT,
    get_text_before_position:
        unsafe extern "system" fn(*mut TextSource, u32, *mut *const u16, *mut u32) -> HRESULT,
    get_paragraph_reading_direction: unsafe extern "system" fn(*mut TextSource) -> i32,
    get_locale_name:
        unsafe extern "system" fn(*mut TextSource, u32, *mut u32, *mut *const u16) -> HRESULT,
    get_number_substitution:
        unsafe extern "system" fn(*mut TextSource, u32, *mut u32, *mut *mut c_void) -> HRESULT,
}

static SOURCE_VTBL: SourceVtbl = SourceVtbl {
    query_interface: source_query_interface,
    add_ref: source_ref,
    release: source_ref,
    get_text_at_position: source_text_at,
    get_text_before_position: source_text_before,
    get_paragraph_reading_direction: source_direction,
    get_locale_name: source_locale,
    get_number_substitution: source_number_substitution,
};

// SAFETY (all `source_*` callbacks): DirectWrite calls them only during the
// `MapCharacters` call that was handed `this`, which borrows a live `TextSource`
// for that whole call; the out-pointers are DirectWrite's valid locals.
unsafe extern "system" fn source_query_interface(
    this: *mut TextSource,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    // SAFETY: see the callback note above.
    unsafe {
        let iid = &*iid;
        if guid_eq(iid, &IID_IUNKNOWN) || guid_eq(iid, &IID_IDWRITETEXTANALYSISSOURCE) {
            *out = this.cast();
            0
        } else {
            *out = ptr::null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn source_ref(_: *mut TextSource) -> u32 {
    1 // stack-owned for one call, never freed through COM
}

unsafe extern "system" fn source_text_at(
    this: *mut TextSource,
    pos: u32,
    text: *mut *const u16,
    len: *mut u32,
) -> HRESULT {
    // SAFETY: see the callback note above; `pos` is clamped, so the pointer stays in `text`.
    unsafe {
        let source = &*this;
        let pos = (pos as usize).min(source.text.len());
        let rest = source.text.len() - pos;
        *text = if rest == 0 {
            ptr::null()
        } else {
            source.text.as_ptr().add(pos)
        };
        *len = rest as u32;
    }
    0
}

unsafe extern "system" fn source_text_before(
    this: *mut TextSource,
    pos: u32,
    text: *mut *const u16,
    len: *mut u32,
) -> HRESULT {
    // SAFETY: see the callback note above.
    unsafe {
        let source = &*this;
        let pos = (pos as usize).min(source.text.len());
        *text = if pos == 0 {
            ptr::null()
        } else {
            source.text.as_ptr()
        };
        *len = pos as u32;
    }
    0
}

unsafe extern "system" fn source_direction(_: *mut TextSource) -> i32 {
    0 // DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
}

unsafe extern "system" fn source_locale(
    this: *mut TextSource,
    pos: u32,
    len: *mut u32,
    name: *mut *const u16,
) -> HRESULT {
    // SAFETY: see the callback note above.
    unsafe {
        let source = &*this;
        *len = (source.text.len() as u32).saturating_sub(pos);
        *name = source.locale.as_ptr();
    }
    0
}

unsafe extern "system" fn source_number_substitution(
    this: *mut TextSource,
    pos: u32,
    len: *mut u32,
    sub: *mut *mut c_void,
) -> HRESULT {
    // SAFETY: see the callback note above.
    unsafe {
        let source = &*this;
        *len = (source.text.len() as u32).saturating_sub(pos);
        *sub = ptr::null_mut();
    }
    0
}

impl TextSource {
    fn new(text: &str, locale: &str) -> Self {
        Self {
            vtbl: &SOURCE_VTBL,
            text: text.encode_utf16().collect(),
            locale: utf16z(locale),
        }
    }

    fn len(&self) -> u32 {
        self.text.len() as u32
    }
}

/// One `MapCharacters` answer.
struct Mapped {
    /// UTF-16 units covered from the segment's start.
    length: u32,
    /// DirectWrite's scale hint (the mapped font should be drawn at this ratio).
    scale: f32,
    /// `None` when no system font covers the segment.
    font: Option<ComPtr<Font>>,
}

impl ComPtr<Fallback> {
    /// Maps the segment `[start, start + len)` of `source`. `base_family` is a NUL-terminated
    /// family name or null; `attrs` is (weight, stretch, style), reordered into the call's weight, style, stretch.
    fn map_characters(
        &self,
        source: &mut TextSource,
        start: u32,
        len: u32,
        collection: &ComPtr<Collection>,
        base_family: *const u16,
        attrs: (i32, i32, i32),
    ) -> Option<Mapped> {
        let (mut mapped_len, mut scale) = (0u32, 0f32);
        let mut font: *mut c_void = ptr::null_mut();
        // SAFETY: IDWriteFontFallback slot 3 MapCharacters(source, start, len,
        // baseCollection, baseFamily, weight, style, stretch (that order), UINT32* mappedLen,
        // IDWriteFont** out, FLOAT* scale). `source` is a live, correctly laid out
        // IDWriteTextAnalysisSource (first field = vtable) that DirectWrite does not
        // retain past the call; `collection` is live; `base_family` is null or
        // NUL-terminated and outlives the call; the out-pointers are locals. The
        // font reference written to `font` is owned by `from_raw` below.
        let hr = unsafe {
            vcall!(
                self.raw(),
                slots::FALLBACK_MAP_CHARACTERS,
                fn(
                    *mut c_void,
                    u32,
                    u32,
                    *mut c_void,
                    *const u16,
                    i32,
                    i32,
                    i32,
                    *mut u32,
                    *mut *mut c_void,
                    *mut f32,
                ) -> HRESULT,
                (source as *mut TextSource).cast(),
                start,
                len,
                collection.raw(),
                base_family,
                attrs.0,
                attrs.2,
                attrs.1,
                &mut mapped_len,
                &mut font,
                &mut scale
            )
        };
        succeeded(hr).then(|| Mapped {
            length: mapped_len,
            scale,
            font: ComPtr::from_raw(font),
        })
    }
}

/// The process-wide factory, system collection and system fallback.
struct System {
    factory: ComPtr<Factory>,
    collection: ComPtr<Collection>,
    fallback: ComPtr<Fallback>,
}

// SAFETY: the shared IDWriteFactory and the system collection and fallback it
// produced are free-threaded (DirectWrite's documented contract for shared
// factories) and are never released, only used through `&self`.
unsafe impl Send for System {}
unsafe impl Sync for System {}

/// The lazily built system, `None` when DirectWrite is unavailable. Building it
/// (collection and fallback acquisition) takes about 140 ms once.
fn system() -> Option<&'static System> {
    static SYSTEM: OnceLock<Option<System>> = OnceLock::new();
    SYSTEM
        .get_or_init(|| {
            let factory = ComPtr::<Factory>::create()?;
            let collection = factory.system_collection()?;
            let fallback = factory.system_fallback()?;
            Some(System {
                factory,
                collection,
                fallback,
            })
        })
        .as_ref()
}

/// DirectWrite weight (`DWRITE_FONT_WEIGHT`, 1 to 999).
fn dwrite_weight(weight: f32) -> i32 {
    (weight.round() as i32).clamp(1, 999)
}

/// `DWRITE_FONT_STRETCH`: 1 (ultra-condensed) to 9 (ultra-expanded), 5 normal.
fn dwrite_stretch(width: f32) -> i32 {
    const RATIOS: [f32; 9] = [0.5, 0.625, 0.75, 0.875, 1.0, 1.125, 1.25, 1.5, 2.0];
    let mut best = 4;
    for (i, ratio) in RATIOS.iter().enumerate() {
        if (ratio - width).abs() < (RATIOS[best] - width).abs() {
            best = i;
        }
    }
    best as i32 + 1
}

/// `DWRITE_FONT_STYLE`: 0 normal, 1 oblique, 2 italic.
fn dwrite_style(style: FontStyle) -> i32 {
    match style {
        FontStyle::Normal => 0,
        FontStyle::Italic => 2,
        FontStyle::Oblique(_) => 1,
    }
}

/// A mapped face plus what the tests need to check it.
struct Resolved {
    source: FaceSource,
    #[cfg(test)]
    family: String,
}

impl System {
    fn resolve(&self, req: &FallbackRequest) -> Option<Resolved> {
        let locale = super::language_for(req.script, req.locale.as_deref()).unwrap_or("en-US");
        let base: Option<Vec<u16>> = req.families.iter().find_map(|f| match f {
            crate::style::FamilyName::Named(name) => Some(utf16z(name)),
            crate::style::FamilyName::Generic(_) => None,
        });
        let base_ptr = base.as_ref().map_or(ptr::null(), |b| b.as_ptr());
        let attrs = (
            dwrite_weight(req.weight.value()),
            dwrite_stretch(req.width),
            dwrite_style(req.slant),
        );
        let mut source = TextSource::new(&req.chars, locale);
        let total = source.len();
        let mut pos = 0;
        // `mappedLength` counts UTF-16 units; walk segments until one has a font.
        while pos < total {
            let mapped = self.fallback.map_characters(
                &mut source,
                pos,
                total - pos,
                &self.collection,
                base_ptr,
                attrs,
            )?;
            if let Some(font) = &mapped.font {
                let (path, index) = font.location()?;
                let mut source = FaceSource::path(path, index);
                source.scale = Some(mapped.scale);
                return Some(Resolved {
                    source,
                    #[cfg(test)]
                    family: font.family()?.names()?.preferred_string()?,
                });
            }
            pos += mapped.length.max(1);
        }
        None
    }

    fn default_family(&self, generic: GenericSlot) -> Option<FaceSource> {
        let name = match generic {
            GenericSlot::SansSerif | GenericSlot::SystemUi => "Segoe UI",
            GenericSlot::Monospace => "Consolas",
            GenericSlot::Serif => "Times New Roman",
            GenericSlot::Emoji => "Segoe UI Emoji",
        };
        let index = self.collection.find_family(name)?;
        let font = self
            .collection
            .family(index)?
            .first_matching_font(400, 5, 0)?;
        let (path, face) = font.location()?;
        Some(FaceSource::path(path, face))
    }
}

/// Answers fallback questions through DirectWrite.
#[derive(Debug, Default)]
pub(crate) struct DirectWriteBackend {}

impl DirectWriteBackend {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

impl Backend for DirectWriteBackend {
    fn resolve(&mut self, req: &FallbackRequest) -> Option<FaceSource> {
        system()?.resolve(req).map(|r| r.source)
    }

    fn default_family(&self, generic: GenericSlot) -> Option<FaceSource> {
        system()?.default_family(generic)
    }

    fn os_build_id(&self) -> String {
        let mut info = OsVersionInfo {
            size: size_of::<OsVersionInfo>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform: 0,
            csd: [0; 128],
        };
        // SAFETY: ntdll's RtlGetVersion fills the OSVERSIONINFOW at `info`, whose
        // `size` field is set to its exact byte size; it cannot fail for that struct.
        let status = unsafe { RtlGetVersion(&mut info) };
        if status < 0 {
            return String::from("windows-unknown");
        }
        format!("windows-{}.{}.{}", info.major, info.minor, info.build)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::oracle::{RenderCapabilities, ScriptTag};
    use crate::style::FontFamily;

    #[test]
    fn the_ui_language_list_parses_as_a_multi_string() {
        let units: Vec<u16> = "ja-JP\0en-US\0\0".encode_utf16().collect();
        assert_eq!(multi_sz(&units), ["ja-JP", "en-US"]);
        assert!(multi_sz(&[0, 0]).is_empty());
        let languages = preferred_ui_languages();
        assert!(
            !languages.is_empty(),
            "Windows lists at least one UI language"
        );
        println!("preferred UI languages {languages:?}");
    }

    /// Serialises tests: the live-pointer counter is process-global.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn request(chars: &str, locale: &str) -> FallbackRequest {
        let mut req = FallbackRequest::new(
            chars,
            ScriptTag::COMMON,
            &FontFamily::SystemUi,
            RenderCapabilities::default(),
        );
        req.locale = Some(locale.to_owned());
        req
    }

    fn file_name(source: &FaceSource) -> String {
        match &source.location {
            crate::oracle::FaceLocation::Path(p) => {
                p.file_name().unwrap().to_string_lossy().to_lowercase()
            }
            crate::oracle::FaceLocation::Blob(_) => panic!("expected a path"),
        }
    }

    #[test]
    fn every_declared_slot_answers_a_known_string() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let sys = system().expect("DirectWrite is available");
        // Factory slots 3 and 26 produced the collection and fallback; the shared
        // factory is IDWriteFactory2, so QueryInterface (0) round-trips.
        let again: ComPtr<Factory> = sys
            .factory
            .query(&IID_IDWRITEFACTORY2)
            .expect("QI factory2");
        assert!(again.ref_count() >= 2);
        drop(again);
        assert!(sys.fallback.ref_count() >= 1);

        // Collection slots 3, 4, 5.
        assert!(sys.collection.family_count() > 10);
        let index = sys
            .collection
            .find_family("segoe ui")
            .expect("Segoe UI is installed");
        assert!(sys.collection.find_family("No Such Family 7f3a").is_none());
        let family = sys.collection.family(index).expect("family");

        // Family slots 6, 7; strings slots 3, 4, 5, 6, 7, 8.
        let names = family.names().expect("names");
        assert!(names.count() >= 1);
        let en = names.find_locale("en-us").expect("en-us name");
        assert_eq!(names.locale_name(en).as_deref(), Some("en-us"));
        assert_eq!(
            names.string(en).as_deref(),
            Some("Segoe UI"),
            "family name, not the locale name"
        );
        assert_eq!(names.preferred_string().as_deref(), Some("Segoe UI"));

        // Font slots 3, 4, 5, 6, 12, 13; face 4, 5; file 3, 4; loader QI + 4, 5.
        let font = family.first_matching_font(400, 5, 0).expect("font");
        assert_eq!(font.weight(), 400);
        assert_eq!(font.stretch(), 5);
        assert_eq!(font.style(), 0);
        assert!(font.has_character('A'));
        assert!(!font.has_character('\u{10FFFD}'));
        let owner = font
            .family()
            .expect("font family")
            .names()
            .expect("names")
            .preferred_string();
        assert_eq!(owner.as_deref(), Some("Segoe UI"));
        let face = font.create_face().expect("face");
        let path = face
            .first_file()
            .expect("file")
            .local_path()
            .expect("local path");
        assert_eq!(
            path.file_name().unwrap().to_string_lossy().to_lowercase(),
            "segoeui.ttf"
        );
        assert_eq!(face.index(), 0);
        assert_eq!(font.location(), Some((path, 0)));
    }

    #[test]
    fn scripts_map_to_their_system_faces() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let sys = system().expect("DirectWrite is available");
        let latin = sys
            .resolve(&request("Hello, world", "en-US"))
            .expect("latin");
        assert_eq!(latin.family, "Segoe UI");
        assert_eq!(file_name(&latin.source), "segoeui.ttf");
        assert!(latin.source.scale.is_some_and(|s| s > 0.0));

        let han = sys
            .resolve(&request("\u{4e2d}\u{6587}", "zh-CN"))
            .expect("han");
        assert!(
            han.family.contains("YaHei")
                || han.family.contains("Sim")
                || han.family.contains("Hei"),
            "han family: {}",
            han.family
        );

        let thai = sys
            .resolve(&request("\u{0e44}\u{0e17}\u{0e22}", "th-TH"))
            .expect("thai");
        assert_eq!(thai.family, "Leelawadee UI");

        let arabic = sys
            .resolve(&request(
                "\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064a}\u{0629}",
                "ar",
            ))
            .expect("arabic");
        assert!(
            arabic.family == "Segoe UI" || arabic.family.contains("Arab"),
            "arabic family: {}",
            arabic.family
        );

        let emoji = sys.resolve(&request("\u{1f600}", "en-US")).expect("emoji");
        assert_eq!(emoji.family, "Segoe UI Emoji");
        assert_eq!(file_name(&emoji.source), "seguiemj.ttf");
    }

    #[test]
    fn the_backend_trait_answers_and_defaults_resolve() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut backend = DirectWriteBackend::new();
        let face = backend
            .resolve(&request("\u{e9}", "en-US"))
            .expect("resolve");
        assert!(matches!(
            face.location,
            crate::oracle::FaceLocation::Path(_)
        ));
        for (slot, file) in [
            (GenericSlot::SystemUi, "segoeui.ttf"),
            (GenericSlot::SansSerif, "segoeui.ttf"),
            (GenericSlot::Monospace, "consola.ttf"),
            (GenericSlot::Serif, "times.ttf"),
            (GenericSlot::Emoji, "seguiemj.ttf"),
        ] {
            let source = backend
                .default_family(slot)
                .unwrap_or_else(|| panic!("{slot:?}"));
            assert_eq!(file_name(&source), file, "{slot:?}");
        }
        let build = backend.os_build_id();
        assert!(
            build.starts_with("windows-10.") || build.starts_with("windows-6."),
            "{build}"
        );
    }

    #[test]
    fn a_cluster_nothing_covers_is_a_miss() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let sys = system().expect("DirectWrite is available");
        // A noncharacter: no installed font maps it.
        assert!(sys.resolve(&request("\u{fdd0}", "en-US")).is_none());
    }

    #[test]
    fn ten_thousand_resolves_leak_no_interfaces() {
        let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let sys = system().expect("DirectWrite is available");
        let samples = ["Hello", "\u{4e2d}\u{6587}", "\u{1f600}", "\u{fdd0}"];
        // Warm up so lazy one-time acquisitions are not counted.
        for text in samples {
            let _ = sys.resolve(&request(text, "en-US"));
        }
        // `system()` is process-wide and `SERIAL` orders only this module's
        // tests, so another test thread resolving through the Windows backend
        // holds a few transient guards and collection references at the
        // instant a count is read (observed: one in flight at either sample).
        // A real leak adds thousands of references over the loop, so each
        // counter may drift by a handful between the samples, and the
        // measurement is retried when it drifts by more.
        const TOLERANCE: i64 = 8;
        let counts = || -> [i64; 4] {
            [
                LIVE.load(Ordering::SeqCst) as i64,
                i64::from(sys.collection.ref_count()),
                i64::from(sys.fallback.ref_count()),
                i64::from(sys.factory.ref_count()),
            ]
        };
        let mut last = None;
        for _ in 0..5 {
            let before = counts();
            for i in 0..10_000 {
                let _ = sys.resolve(&request(samples[i % samples.len()], "en-US"));
            }
            let after = counts();
            if before
                .iter()
                .zip(&after)
                .all(|(b, a)| (a - b).abs() <= TOLERANCE)
            {
                return;
            }
            last = Some((before, after));
        }
        panic!(
            "interfaces leaked across 10 000 resolves (live, collection, fallback, factory) before/after: {last:?}"
        );
    }
}
