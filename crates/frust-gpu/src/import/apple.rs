//! Apple zero-copy import: CVPixelBuffer -> CVMetalTextureCache -> MTLTexture ->
//! `wgpu_hal::metal::Device::texture_from_raw` -> `wgpu::Device::create_texture_from_hal`.
//!
//! [`MetalImporter`] is the whole surface: built once per `wgpu::Device` (it
//! owns a `CVMetalTextureCache` created against that device's `MTLDevice`), it
//! turns a `CVPixelBufferRef` into a sampleable `wgpu::Texture` aliasing the
//! buffer's IOSurface, with no copy. On a device that is not a Metal device it
//! answers [`MetalImportError::Unsupported`].
//!
//! # What one import does
//!
//! 1. Checks the buffer: a `CVPixelBuffer` (by CoreFoundation type id) in the
//!    `kCVPixelFormatType_32BGRA` layout, non-empty and within the device's 2D
//!    texture limit. Any other layout — NV12 (`420v`/`420f`) included — is
//!    refused as `Unsupported`, so a producer keeps that format on its CPU path.
//! 2. Flushes the texture cache (see the invariants), then
//!    `CVMetalTextureCacheCreateTextureFromImage` with `MTLPixelFormatBGRA8Unorm`
//!    over plane 0 — a `CVMetalTexture` bound live to the buffer's IOSurface —
//!    and `CVMetalTextureGetTexture` for its `MTLTexture`.
//! 3. `texture_from_raw` (2D, one layer, one mip, the buffer's extent) with a
//!    drop callback owning the `CVMetalTexture` and the import's own retain on
//!    the pixel buffer, then `create_texture_from_hal` as `Bgra8Unorm` with
//!    `TEXTURE_BINDING` usage and `TextureUses::RESOURCE` (Metal tracks no
//!    layouts, and the producer finished writing the buffer before handing it
//!    over).
//!
//! Nothing is submitted: the import touches neither the queue nor any command
//! buffer, so — unlike the Android import — it carries no queue invariant.
//!
//! # Invariants
//!
//! - **The pixel buffer is IOSurface-backed.** Only a buffer created with
//!   `kCVPixelBufferIOSurfacePropertiesKey` (or `kCVPixelBufferMetalCompatibilityKey`)
//!   can alias a Metal texture — what AVFoundation's video outputs and camera
//!   data outputs hand out when asked for Metal compatibility. CoreVideo
//!   refuses any other buffer, and the import answers `Failed` with its code.
//! - **The buffer and its texture outlive every GPU use.** The import takes
//!   its own `CFRetain` on the pixel buffer and keeps the `CVMetalTexture` (the
//!   cache's signal that the binding is in use and must not be recycled); both
//!   belong to the texture's drop callback, which `wgpu` runs once the texture
//!   is destroyed — after every submission using it finished. The caller may
//!   drop its own reference as soon as the call returns.
//! - **The cache is flushed as frames are dropped.** A `CVMetalTexture` given
//!   back by a drop callback stays in the cache until a
//!   `CVMetalTextureCacheFlush`; every import flushes first (and dropping the
//!   importer flushes once more), so the cache holds at most what was dropped
//!   since the previous import — bindings still referenced are never evicted,
//!   and a buffer recycled from a producer's ring is found again (a cache hit).
//! - **The texture does not outlive the device.** Every `wgpu` resource keeps
//!   its device alive, and the cache retains the `MTLDevice` it was created
//!   on; an importer only imports on the `wgpu::Device` it was built for.
//! - **One importing thread at a time.** The importer is `Send` but not
//!   `Sync`: CoreVideo documents no concurrent use of one texture cache, so
//!   the type system serialises every import on it (a shared importer lives
//!   behind a `Mutex`).
//! - **The texture is read-only.** It carries `TEXTURE_BINDING` only; the
//!   producer must not write the buffer while a texture over it is alive
//!   (the frame bus holds the frame's release until then).

use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;

use objc2_core_foundation::{CFGetTypeID, CFRetained};
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetTypeID,
    CVPixelBufferGetWidth, kCVPixelFormatType_32BGRA, kCVReturnSuccess,
};
use objc2_metal::{MTLPixelFormat, MTLTextureType};
use wgpu::hal::api::Metal;

/// The one texture format an import produces: the `32BGRA` layout, sampled
/// as `Bgra8Unorm` (a shader reads it as RGBA; the hardware swizzles).
const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

/// Why a [`MetalImporter`] could not be built or could not import a buffer.
/// Mirrors the frame bus's own import error, which a shell's importer maps it
/// onto.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetalImportError {
    /// The device or the buffer's layout cannot be imported at all: not a
    /// Metal device, or a pixel format other than `32BGRA` (NV12 included).
    Unsupported(String),
    /// This buffer could not be imported: a null or mistyped pointer, an
    /// extent outside the device's limit, a buffer that is not IOSurface-backed,
    /// or a CoreVideo error.
    Failed(String),
}

impl fmt::Display for MetalImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(reason) => write!(f, "CVPixelBuffer import unsupported: {reason}"),
            Self::Failed(reason) => write!(f, "CVPixelBuffer import failed: {reason}"),
        }
    }
}

impl std::error::Error for MetalImportError {}

/// Imports `CVPixelBuffer`s as `wgpu::Texture`s on one Metal device, through a
/// `CVMetalTextureCache` of its own (see the module docs).
pub struct MetalImporter {
    /// The device every import creates its texture on.
    device: wgpu::Device,
    /// The texture cache, created against `device`'s `MTLDevice`.
    cache: CFRetained<CVMetalTextureCache>,
}

// SAFETY: CoreFoundation objects are not bound to the thread that created
// them, and their retain/release is atomic, so moving the cache (and the
// device, already `Send`) to another thread is sound. The importer is not
// `Sync`, so the cache is never used from two threads at once.
unsafe impl Send for MetalImporter {}

impl fmt::Debug for MetalImporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetalImporter")
            .field("device", &self.device)
            .finish_non_exhaustive()
    }
}

impl MetalImporter {
    /// An importer for `device`, with a texture cache created against its
    /// `MTLDevice`.
    ///
    /// # Errors
    ///
    /// [`MetalImportError::Unsupported`] when `device` is not a Metal device;
    /// [`MetalImportError::Failed`] when CoreVideo cannot create the cache.
    pub fn new(device: &wgpu::Device) -> Result<Self, MetalImportError> {
        // SAFETY: the guard only borrows the device's hal object for the
        // cache creation below; nothing here destroys or mutates it, and the
        // cache takes its own retain on the `MTLDevice`.
        let hal_device = unsafe { device.as_hal::<Metal>() }.ok_or_else(|| {
            MetalImportError::Unsupported("the device is not a Metal device".to_string())
        })?;
        let mut cache: *mut CVMetalTextureCache = std::ptr::null_mut();
        // SAFETY: no attribute dictionaries (both optional), a live
        // `MTLDevice`, and a valid out-pointer.
        let status = unsafe {
            CVMetalTextureCache::create(
                None,
                None,
                hal_device.raw_device(),
                None,
                NonNull::from(&mut cache),
            )
        };
        drop(hal_device);
        let cache = NonNull::new(cache)
            .filter(|_| status == kCVReturnSuccess)
            .ok_or_else(|| {
                MetalImportError::Failed(format!("CVMetalTextureCacheCreate returned {status}"))
            })?;
        // SAFETY: a `Create` function's result, owned (+1) by the caller.
        let cache = unsafe { CFRetained::from_raw(cache) };
        Ok(Self {
            device: device.clone(),
            cache,
        })
    }

    /// The device this importer creates its textures on.
    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Wraps `pixel_buffer` — an IOSurface-backed `CVPixelBufferRef` in the
    /// `32BGRA` layout — as a sampleable `Bgra8Unorm` `wgpu::Texture` on
    /// [`Self::device`], without a copy, returning it with its extent.
    ///
    /// The texture has `TEXTURE_BINDING` usage only, one mip level and the
    /// buffer's extent; its texels are the buffer's bytes as the producer left
    /// them (the frame bus samples them as premultiplied RGBA). The texture
    /// keeps its own reference to the buffer; the caller keeps ownership of the
    /// reference it passed in.
    ///
    /// # Errors
    ///
    /// [`MetalImportError::Unsupported`] for a pixel format other than
    /// `32BGRA`; [`MetalImportError::Failed`] for a null pointer, an object that
    /// is not a `CVPixelBuffer`, an empty or oversized extent, or a CoreVideo
    /// error (a buffer that is not IOSurface-backed among them).
    ///
    /// # Safety
    ///
    /// - `pixel_buffer` is null or a valid CoreFoundation object (a
    ///   `CVPixelBufferRef`, which the import checks) that the caller holds a
    ///   reference to for the duration of the call.
    /// - The buffer's contents are complete, and the producer does not write
    ///   it again while the returned texture is alive.
    pub unsafe fn import(
        &self,
        pixel_buffer: *mut c_void,
    ) -> Result<(wgpu::Texture, (u32, u32)), MetalImportError> {
        let buffer = NonNull::new(pixel_buffer.cast::<CVPixelBuffer>())
            .ok_or_else(|| MetalImportError::Failed("a null CVPixelBuffer".to_string()))?;
        // SAFETY: a live CoreFoundation object the caller holds (this fn's
        // contract); this is the texture's own reference, released by the
        // drop callback below (or on an early return).
        let buffer = unsafe { CFRetained::retain(buffer) };
        if CFGetTypeID(Some(&buffer)) != CVPixelBufferGetTypeID() {
            return Err(MetalImportError::Failed(
                "the object is not a CVPixelBuffer".to_string(),
            ));
        }
        let format = CVPixelBufferGetPixelFormatType(&buffer);
        if format != kCVPixelFormatType_32BGRA {
            return Err(MetalImportError::Unsupported(format!(
                "pixel format {} is not 32BGRA",
                four_char_code(format)
            )));
        }
        let max_dimension = self.device.limits().max_texture_dimension_2d;
        let (width, height) = check_extent(
            CVPixelBufferGetWidth(&buffer),
            CVPixelBufferGetHeight(&buffer),
            max_dimension,
        )?;

        // Give back what frames dropped since the last import left behind.
        self.cache.flush(0);
        let mut cv_texture: *mut CVMetalTexture = std::ptr::null_mut();
        // SAFETY: a live cache and pixel buffer, no attribute dictionary, a
        // non-planar BGRA buffer mapped as BGRA8 at its own extent over plane
        // 0 (ignored for a non-planar buffer), and a valid out-pointer.
        let status = unsafe {
            CVMetalTextureCache::create_texture_from_image(
                None,
                &self.cache,
                &buffer,
                None,
                MTLPixelFormat::BGRA8Unorm,
                width as usize,
                height as usize,
                0,
                NonNull::from(&mut cv_texture),
            )
        };
        let cv_texture = NonNull::new(cv_texture)
            .filter(|_| status == kCVReturnSuccess)
            .ok_or_else(|| {
                MetalImportError::Failed(format!(
                    "CVMetalTextureCacheCreateTextureFromImage returned {status} (is the \
                     buffer IOSurface-backed?)"
                ))
            })?;
        // SAFETY: a `Create` function's result, owned (+1) by the caller.
        let cv_texture = unsafe { CFRetained::from_raw(cv_texture) };
        let raw = CVMetalTextureGetTexture(&cv_texture).ok_or_else(|| {
            MetalImportError::Failed("CVMetalTextureGetTexture returned no texture".to_string())
        })?;

        let held = HeldFrame {
            _texture: cv_texture,
            _buffer: buffer,
        };
        let on_drop: wgpu::hal::DropCallback = Box::new(move || drop(held));
        // SAFETY: `raw` is a 2D, single-layer, single-mip BGRA8Unorm texture
        // of exactly `width x height` (what step 2 asked CoreVideo for) on
        // this device's `MTLDevice` (the cache's); `on_drop` keeps its
        // backing alive until `wgpu` destroys the texture.
        let hal_texture = unsafe {
            wgpu::hal::metal::Device::texture_from_raw(
                raw,
                TEXTURE_FORMAT,
                MTLTextureType::Type2D,
                1,
                1,
                wgpu::hal::CopyExtent {
                    width,
                    height,
                    depth: 1,
                },
                Some(on_drop),
            )
        };
        let descriptor = wgpu::TextureDescriptor {
            label: Some("frust-gpu CVPixelBuffer"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        // SAFETY: the hal texture came from this device's `MTLDevice` and
        // matches `descriptor` (extent, format, one mip, one sample, 2D); its
        // contents are the producer's finished frame (initialised), and Metal
        // has no layout for `RESOURCE` to disagree with.
        let texture = unsafe {
            self.device.create_texture_from_hal::<Metal>(
                hal_texture,
                &descriptor,
                wgpu::TextureUses::RESOURCE,
            )
        };
        Ok((texture, (width, height)))
    }
}

impl Drop for MetalImporter {
    fn drop(&mut self) {
        self.cache.flush(0);
    }
}

/// What an imported texture keeps alive until `wgpu` destroys it: the
/// `CVMetalTexture` (the cache binding) and the import's retain on the pixel
/// buffer. Dropping it releases both.
struct HeldFrame {
    _texture: CFRetained<CVMetalTexture>,
    _buffer: CFRetained<CVPixelBuffer>,
}

// SAFETY: two CoreFoundation references, only ever released (when the drop
// callback drops this) — CoreFoundation's release is atomic and callable from
// any thread, and nothing reads through them.
unsafe impl Send for HeldFrame {}
// SAFETY: as `Send` — the wrapper exposes nothing through a shared reference.
unsafe impl Sync for HeldFrame {}

/// The extent of a buffer an import accepts, or why it is refused: non-empty
/// and within `max_dimension` on both axes.
fn check_extent(
    width: usize,
    height: usize,
    max_dimension: u32,
) -> Result<(u32, u32), MetalImportError> {
    let fits = |edge: usize| {
        u32::try_from(edge)
            .ok()
            .filter(|&edge| (1..=max_dimension).contains(&edge))
    };
    match (fits(width), fits(height)) {
        (Some(width), Some(height)) => Ok((width, height)),
        _ => Err(MetalImportError::Failed(format!(
            "a {width}x{height} buffer is outside 1..={max_dimension}"
        ))),
    }
}

/// A CoreVideo pixel-format code as its four characters (`BGRA`, `420v`), or
/// as hex when they are not printable.
fn four_char_code(code: u32) -> String {
    let bytes = code.to_be_bytes();
    if bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        bytes.iter().map(|&b| char::from(b)).collect()
    } else {
        format!("{code:#010x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extent_within_the_limit_is_accepted() {
        assert_eq!(check_extent(64, 32, 4096), Ok((64, 32)));
        assert_eq!(check_extent(4096, 4096, 4096), Ok((4096, 4096)));
    }

    #[test]
    fn empty_and_oversized_extents_are_refused() {
        assert!(check_extent(0, 32, 4096).is_err());
        assert!(check_extent(32, 0, 4096).is_err());
        assert!(check_extent(4097, 32, 4096).is_err());
        assert!(check_extent(usize::MAX, 32, 4096).is_err());
    }

    #[test]
    fn pixel_formats_print_as_their_four_characters() {
        assert_eq!(four_char_code(kCVPixelFormatType_32BGRA), "BGRA");
        assert_eq!(four_char_code(0x3432_3076), "420v");
        assert_eq!(four_char_code(0x0000_0020), "0x00000020");
    }
}
