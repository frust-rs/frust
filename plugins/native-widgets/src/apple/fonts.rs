//! Theme ladder L3's Apple half: mirrors
//! `crate::android::fonts`'s "resolve the embedded Glyph bytes to a real,
//! process-cached platform font object, once" shape, but via CoreText rather
//! than a cache-file write + `Typeface.createFromFile` — see *Registration,
//! the Apple way* below for why this arm needs neither a cache file nor any
//! system-wide font-manager registration call at all.
//!
//! # `crate::api::theme::publish_font_bytes` is the only caller of
//! [`set_glyph_bytes`]
//!
//! Same api→runtime seam as the Android half (`crate::android::fonts`'s own
//! doc): this module never names `frust-theme` directly. This crate's
//! `Cargo.toml` allows a `frust-theme` dependency only behind the
//! `frust-api` feature, and only `crate::api::theme` ever names it; the
//! plain `&'static [u8]` slices cross into this module the same way they
//! cross into the Android one.
//!
//! # Registration, the Apple way: a descriptor, not a registered font
//!
//! Android's `Typeface.createFromFile` needs an actual file path — there is
//! no create-from-bytes overload at this crate's `minSdk` floor
//! (`crate::android::fonts`'s own doc) — so that half writes the embedded
//! bytes to a content-hash-keyed cache file once. CoreText carries no such
//! restriction: `CTFontManagerCreateFontDescriptorFromData` builds a real
//! `CTFontDescriptor` straight from an in-memory `CFData`, and a descriptor
//! needs no *system-wide* registration
//! (`CTFontManagerRegisterFontDescriptors`/the deprecated
//! `CTFontManagerRegisterGraphicsFont`) to become usable —
//! `CTFont::with_font_descriptor` resolves a real, sized `CTFont` straight
//! from it, and a `CTFont` **is** a `UIFont` (the same underlying object,
//! toll-free bridged — `objc2-ui-kit`'s `AsRef<UIFont> for CTFont`, gated on
//! this crate's `objc2-core-text` feature on `objc2-ui-kit`, `Cargo.toml`) —
//! not a cast, a bridge. So this arm needs no cache file, no font-manager
//! registration call, and — unlike the modern
//! `CTFontManagerRegisterFontDescriptors` API, whose `registrationHandler`
//! is a `block2` completion block this crate would otherwise have to
//! synchronize against — no asynchrony to reason about at all: resolution is
//! synchronous data-in, `CTFontDescriptor`-out, exactly like every other
//! call this crate makes into UIKit/CoreText.
//!
//! The descriptor (family-level; no point size baked in, since
//! `CTFontManagerCreateFontDescriptorFromData` returns one independent of
//! size — matching `CTFontDescriptor`'s own documented shape) is cached once
//! per face here, mirroring Android's cached `Typeface` object. Its caller
//! ([`crate::controls::platform::resolve_font`]) builds a freshly **sized**
//! `CTFont` from it on every apply — the same "cheap per-size construction
//! over a cached face" shape `Typeface` + `setTextSize` already has on
//! Android, forced here by `UIFont`/`CTFont`'s own immutability (family and
//! point size bake into one object, unlike Android's two independent
//! setters) — see `crate::controls::platform`'s `FontState` doc for the
//! consequence that has for this arm's setter application.
//!
//! # One-time resolution, never cached on failure
//!
//! [`descriptor_for`] resolves (and thread-locally caches, in [`MONO`]/
//! [`PLEX`] — see those statics' own doc for why this arm's cache is
//! thread-local rather than the process-wide `static` Android's equivalent
//! cache uses) each face's descriptor the first time it is asked for. A
//! failed resolution is **not** cached — mirroring `crate::android::fonts`'s
//! identical rule — so a later call retries fresh instead of latching a
//! permanent degrade.
//!
//! # Degrade path
//!
//! A resolution failure (invalid font data, or
//! `CTFontManagerCreateFontDescriptorFromData` returning `None`) degrades to
//! [`Typeface::System`]'s own meaning — [`descriptor_for`] returns `None`,
//! and its caller falls back to the system font — and logs **one** warning
//! for the whole process lifetime ([`WARNED`]), mirroring
//! `crate::android::fonts`'s identical contract: never a crash, never a
//! panic across FFI (`docs/CODE_STANDARDS.md`'s no-unwind rule).
//!
//! # OFL-1.1 redistribution note
//!
//! Same provenance as the Android half: [`Typeface::GlyphMono`]/
//! [`Typeface::GlyphPlex`] resolve to Space Mono / IBM Plex Mono, the exact
//! bytes `frust-theme`'s `glyph-fonts` feature bundles (Google Fonts / IBM,
//! SIL Open Font License 1.1 — see `crates/frust-theme/src/glyph/fonts.rs`
//! and `crates/frust-theme/fonts/README.md`). This module never persists
//! them anywhere a user or another app can reach — a `CTFontDescriptor` sits
//! in this process's own memory for this process's own lifetime, no file, no
//! `Persistent`/`Session` scope registration.

use std::cell::RefCell;
use std::sync::{Once, OnceLock};

use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_text::{CTFontDescriptor, CTFontManagerCreateFontDescriptorFromData};

use crate::NativeWidgetError;
use crate::controls::typeface::{Typeface, degrade_on_failure};

/// This process's Glyph font bytes, published once by [`crate::api::theme`]
/// — mirrors `crate::android::fonts::GlyphBytes` exactly (same seam, same
/// idempotent-publish contract). Plain `&'static [u8]` slices are `Sync`, so
/// this half of the cache stays a process-wide [`OnceLock`] like Android's —
/// only the *descriptor* cache below needs a different shape.
struct GlyphBytes {
    /// Space Mono (Regular face) — [`Typeface::GlyphMono`].
    mono: &'static [u8],
    /// IBM Plex Mono (Regular face) — [`Typeface::GlyphPlex`].
    plex: &'static [u8],
}

static BYTES: OnceLock<GlyphBytes> = OnceLock::new();

/// Publish this process's Glyph font bytes — idempotent (the first call
/// wins; a later call, with the same or different bytes, is a silent
/// no-op), mirroring `crate::android::fonts::set_glyph_bytes`'s identical
/// contract. Never called at all when the `frust-api` feature is off, or on
/// a process that never resolves a `Theme`, in which case [`descriptor_for`]
/// simply never finds published bytes and every control quietly stays on
/// [`Typeface::System`] — no warning, since nothing was ever asked to
/// resolve.
///
/// First-call-wins is why a *live* face swap does not re-register here: the
/// caller's own guard already skips an unchanged pair and re-publishes a
/// changed one, but this arm latches the first (see `crate::api::theme`'s
/// module doc — widening it is a platform-half change owing its own device
/// gate).
pub(crate) fn set_glyph_bytes(mono: &'static [u8], plex: &'static [u8]) {
    let _ = BYTES.set(GlyphBytes { mono, plex });
}

thread_local! {
    /// The two faces' cached descriptors — populated only on a *successful*
    /// [`register`] (module doc: a failure is never cached, so a later call
    /// retries fresh).
    ///
    /// **Thread-local, not a process-wide `static` like Android's
    /// `Global<JObject>` cache.** A JNI global reference is a JVM-tracked
    /// handle the JVM itself keeps valid from any thread — `Global<T>` is a
    /// genuine `Sync` type. `CTFontDescriptor` carries no such contract:
    /// objc2-core-text 0.3.2 marks it neither `Send` nor `Sync` (a plain
    /// CoreFoundation object, reference-counted but not
    /// cross-thread-synchronized), so a `static OnceLock<CFRetained<..>>`
    /// does not compile — and `unsafe impl Sync` would be a false promise
    /// about a handle this crate has no grounds to call thread-safe. Every
    /// caller of [`descriptor_for`] already runs on the platform main thread
    /// (`crate::apple`'s whole arm is UIKit-confined there, and this crate
    /// never spins up a second one), the same main-thread-confinement
    /// `crate::runtime`'s own `thread_local!` `RUNTIME` documents — so one
    /// cache per thread is, in practice, one cache, period.
    static MONO: RefCell<Option<CFRetained<CTFontDescriptor>>> = const { RefCell::new(None) };
    static PLEX: RefCell<Option<CFRetained<CTFontDescriptor>>> = const { RefCell::new(None) };
}

/// The one warning this whole module ever logs (module doc's degrade path).
static WARNED: Once = Once::new();

/// Resolve `typeface` to its thread-locally cached [`CTFontDescriptor`] —
/// `None` for [`Typeface::System`] (the caller's own system-font fallback) or
/// on any resolution failure (module doc's degrade path).
///
/// Returns an owned, cloned [`CFRetained`] rather than a `'static` reference
/// (`CFRetained::clone` is a cheap `CFRetain` — an ARC-style refcount bump,
/// not a copy) — the thread-local cache below has no `'static` storage to
/// borrow from, unlike Android's process-wide `OnceLock`.
pub(crate) fn descriptor_for(typeface: Typeface) -> Option<CFRetained<CTFontDescriptor>> {
    match typeface {
        Typeface::System => None,
        Typeface::GlyphMono => resolve(&MONO, "glyphMono", |bytes| bytes.mono),
        Typeface::GlyphPlex => resolve(&PLEX, "glyphPlex", |bytes| bytes.plex),
    }
}

/// Resolve-and-cache one face — see [`descriptor_for`].
fn resolve(
    cache: &'static std::thread::LocalKey<RefCell<Option<CFRetained<CTFontDescriptor>>>>,
    face_id: &str,
    pick: impl FnOnce(&GlyphBytes) -> &'static [u8],
) -> Option<CFRetained<CTFontDescriptor>> {
    if let Some(cached) = cache.with(|cell| cell.borrow().clone()) {
        return Some(cached);
    }
    // No bytes ever published: this feature/theme combination was never
    // asked for a Glyph face (module doc's `set_glyph_bytes` note) — quietly
    // degrade, no warning (mirrors `crate::android::fonts::resolve`).
    let bytes = pick(BYTES.get()?);
    let descriptor = degrade_on_failure(register(bytes), |e| warn_once(face_id, e))?;
    cache.with(|cell| *cell.borrow_mut() = Some(descriptor.clone()));
    Some(descriptor)
}

/// `CTFontManagerCreateFontDescriptorFromData` — the whole registration this
/// arm needs (module doc's *Registration, the Apple way*): no cache file, no
/// font-manager call, no `block2` completion handler.
///
/// # Errors
/// [`NativeWidgetError::Platform`] when the bytes are not a font CoreText
/// recognizes (or contain a font collection with none — the same "only the
/// first font" caveat the single-descriptor CoreText call documents).
fn register(bytes: &'static [u8]) -> Result<CFRetained<CTFontDescriptor>, NativeWidgetError> {
    let data = CFData::from_static_bytes(bytes);
    // SAFETY: `data` is a live `CFData` for the duration of this call, which
    // is all `CTFontManagerCreateFontDescriptorFromData` requires — it reads
    // the bytes and returns a new, independently retained descriptor (or
    // `None`), never retaining `data` itself past the call.
    let descriptor = unsafe { CTFontManagerCreateFontDescriptorFromData(&data) };
    descriptor.ok_or_else(|| {
        NativeWidgetError::Platform(
            "CTFontManagerCreateFontDescriptorFromData returned null (not a valid font)".into(),
        )
    })
}

/// See [`WARNED`].
fn warn_once(face_id: &str, error: &NativeWidgetError) {
    WARNED.call_once(|| {
        log::warn!(
            "frust-native-widgets: could not resolve the Glyph {face_id} typeface ({error}) — \
             degrading to the platform's system font (see docs/ARCHITECTURE.md's \
             `frust-native-widgets` row for the theme ladder)"
        );
    });
}
