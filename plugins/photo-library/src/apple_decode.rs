//! iOS and macOS: the ImageIO property reader, and full-resolution decode through ImageIO for
//! [`PhotoLibrary::decode_file`](crate::PhotoLibrary::decode_file).
//!
//! The property reader ([`first_image_properties`] and the EXIF read) compiles with either the
//! `decode` or the `library` feature; the pixel decode needs `decode`. Two callers share it: the
//! decoder below, and [`exif_from_data`], which `asset_metadata` (`library`, iOS) uses on the
//! in-memory bytes of a library asset's original. On macOS [`exif_from_data`] has no caller
//! outside this module's tests, because desktop has no photo library.
//!
//! 1. The file URL becomes an image source with `kCGImageSourceShouldCache` off, so ImageIO keeps
//!    no full-size copy of the pixels around.
//! 2. The first image's properties give the source size (upright, after its EXIF orientation)
//!    and the EXIF, TIFF and GPS dictionaries that fill [`ExifData`]. When the size is known,
//!    a capped image over the shared byte ceiling ([`decode::MAX_BUFFER_BYTES`]) is refused
//!    here, before ImageIO decodes anything.
//! 3. `CGImageSourceCreateThumbnailAtIndex` builds the image from the full data (never an
//!    embedded preview), applies the orientation and scales the long edge down to
//!    [`DecodeOptions::max_dimension`]. A smaller image keeps its size.
//! 4. The decoded image is checked again before any buffer exists: a long edge over
//!    `max_dimension` is an error, and so is a buffer over the ceiling. The buffer is then
//!    allocated fallibly ([`decode::allocate_rgba`]), so an allocation the allocator refuses is an
//!    error. The system can still end the process if memory runs out while the buffer is filled.
//! 5. A premultiplied RGBA8 bitmap context in an explicitly chosen space draws it: Display P3
//!    when the image is wide-gamut and [`DecodeOptions::srgb`] is off, sRGB otherwise.
//!    [`DecodedImage::color_space`] names the space drawn into. The pixels are then converted
//!    to straight alpha.
//!
//! # EXIF
//!
//! `capture_time` comes only from EXIF `DateTimeOriginal`, never the TIFF `DateTime` (the
//! last-modified time). An ISO, exposure time, aperture or focal length that is zero, negative
//! or non-finite is absent, as on Android ([`decode::positive_measure`]).
//!
//! # Cancel
//!
//! The token is checked before the source is created, before the decode and before the draw.
//! The ImageIO decode itself cannot be interrupted: a cancel that lands during it takes effect
//! when it returns.
//!
//! # Errors
//!
//! A missing file is [`PhotoError::NotFound`], as on every arm ([`decode::require_file`]). A
//! capped image over the byte ceiling is [`PhotoError::InvalidRequest`]. A file ImageIO cannot
//! read, one cut short inside its headers, or one without an image is a [`PhotoError::Platform`]
//! decode error, never a panic. Messages never echo the path.
//!
//! ImageIO is lenient with a file whose pixel data stops early: JPEG, PNG and HEIC alike decode,
//! with the missing part left blank, and ImageIO reports the source as complete. Such a file is
//! returned as an image, not an error; telling it apart would need a per-format container check.

use std::ffi::c_void;
#[cfg(feature = "decode")]
use std::path::Path;
use std::ptr::NonNull;

use objc2::rc::autoreleasepool;
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
#[cfg(feature = "decode")]
use objc2_core_foundation::{CFURL, CGFloat, CGPoint, CGRect, CGSize};
#[cfg(feature = "decode")]
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapInfo, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceDisplayP3, kCGColorSpaceSRGB,
};

use crate::decode;
#[cfg(feature = "decode")]
use crate::decode::{DecodeOptions, DecodedImage, ImageColorSpace};
#[cfg(feature = "decode")]
use crate::pixels;
use crate::types::{ExifData, PhotoError};

/// A property dictionary as ImageIO returns it: string keys, values of any CF type.
type Properties = CFDictionary<CFString, CFType>;

/// `CGImageSourceStatus.kCGImageStatusComplete`.
const STATUS_COMPLETE: i32 = 0;

// Hand-declared: the crate enables no `CFData` binding, and the in-memory reader needs only a
// data object over bytes it does not copy and the retain count that proves nothing kept it. A
// `CFDataRef` is a CF object; it is handled as a plain `CFType`, and a `CFAllocatorRef` as an
// opaque pointer.
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreateWithBytesNoCopy(
        allocator: *const c_void,
        bytes: *const u8,
        length: isize,
        bytes_deallocator: *const c_void,
    ) -> Option<NonNull<CFType>>;
    fn CFGetRetainCount(cf: &CFType) -> isize;

    /// The allocator that does nothing: as a bytes deallocator, CF never frees the bytes.
    static kCFAllocatorNull: *const c_void;
}

// Hand-declared: the workspace has no ImageIO bindings, and the reader and decoder need only
// these.
#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    // A `CGImageSourceRef` is a CF object; it is handled as a plain `CFType`.
    #[cfg(feature = "decode")]
    fn CGImageSourceCreateWithURL(
        url: &CFURL,
        options: Option<&CFDictionary>,
    ) -> Option<NonNull<CFType>>;
    fn CGImageSourceCreateWithData(
        data: &CFType,
        options: Option<&CFDictionary>,
    ) -> Option<NonNull<CFType>>;
    fn CGImageSourceGetCount(isrc: &CFType) -> usize;
    fn CGImageSourceGetStatusAtIndex(isrc: &CFType, index: usize) -> i32;
    fn CGImageSourceCopyPropertiesAtIndex(
        isrc: &CFType,
        index: usize,
        options: Option<&CFDictionary>,
    ) -> Option<NonNull<CFDictionary>>;
    #[cfg(feature = "decode")]
    fn CGImageSourceCreateThumbnailAtIndex(
        isrc: &CFType,
        index: usize,
        options: Option<&CFDictionary>,
    ) -> Option<NonNull<CGImage>>;

    static kCGImageSourceShouldCache: &'static CFString;
    #[cfg(feature = "decode")]
    static kCGImageSourceCreateThumbnailFromImageAlways: &'static CFString;
    #[cfg(feature = "decode")]
    static kCGImageSourceCreateThumbnailWithTransform: &'static CFString;
    #[cfg(feature = "decode")]
    static kCGImageSourceThumbnailMaxPixelSize: &'static CFString;

    #[cfg(feature = "decode")]
    static kCGImagePropertyPixelWidth: &'static CFString;
    #[cfg(feature = "decode")]
    static kCGImagePropertyPixelHeight: &'static CFString;
    // The tests write it into their fixtures.
    #[cfg(any(feature = "decode", test))]
    static kCGImagePropertyOrientation: &'static CFString;
    static kCGImagePropertyExifDictionary: &'static CFString;
    static kCGImagePropertyTIFFDictionary: &'static CFString;
    static kCGImagePropertyGPSDictionary: &'static CFString;

    static kCGImagePropertyExifDateTimeOriginal: &'static CFString;
    static kCGImagePropertyExifISOSpeedRatings: &'static CFString;
    static kCGImagePropertyExifExposureTime: &'static CFString;
    static kCGImagePropertyExifFNumber: &'static CFString;
    static kCGImagePropertyExifFocalLength: &'static CFString;
    static kCGImagePropertyExifLensModel: &'static CFString;
    static kCGImagePropertyTIFFMake: &'static CFString;
    static kCGImagePropertyTIFFModel: &'static CFString;
    static kCGImagePropertyGPSLatitude: &'static CFString;
    static kCGImagePropertyGPSLatitudeRef: &'static CFString;
    static kCGImagePropertyGPSLongitude: &'static CFString;
    static kCGImagePropertyGPSLongitudeRef: &'static CFString;
}

/// The EXIF of the image held in `bytes`, read through the same [`first_image_properties`] and
/// EXIF read as the decoder; no pixel is decoded. An image without EXIF is an empty
/// [`ExifData`]; bytes ImageIO cannot read, or that hold no image, are the decoder's
/// [`PhotoError::Platform`] error. Only the header matters: ImageIO reports the properties of a
/// header it can parse even when the rest of the data is missing.
///
/// ImageIO reads `bytes` in place: no second copy of the data is made, so the peak memory is
/// `bytes` itself. Every object made from them is released before this returns; should one
/// still be alive then (a retain this crate cannot see), `bytes` is leaked rather than freed
/// under it.
#[cfg_attr(not(all(target_os = "ios", feature = "library")), allow(dead_code))]
pub(crate) fn exif_from_data(bytes: Vec<u8>) -> Result<ExifData, PhotoError> {
    // SAFETY: `data` is dropped below, before `bytes` (or `bytes` is leaked); see `borrowed_data`.
    let data = unsafe { borrowed_data(&bytes) }?;
    let exif = autoreleasepool(|_| exif_of(&data));
    // The pool has drained, so every image source and dictionary made from `data` is released;
    // only this reference should remain.
    // SAFETY: `data` is a live CF object.
    if unsafe { CFGetRetainCount(&data) } != 1 {
        log::warn!(
            "photo metadata: ImageIO kept the asset's data; leaking it instead of freeing it"
        );
        std::mem::forget(bytes);
    }
    drop(data);
    exif
}

/// A `CFData` over `bytes` that neither copies nor frees them.
///
/// # Safety
///
/// `bytes` must stay alive and unmodified while the result, or any object made from it, is
/// alive: drop the result first and confirm its retain count was one, or leak `bytes`.
unsafe fn borrowed_data(bytes: &[u8]) -> Result<CFRetained<CFType>, PhotoError> {
    let length = isize::try_from(bytes.len())
        .map_err(|_| PhotoError::Platform("the image data is too large to read".to_owned()))?;
    // SAFETY: `bytes` is valid for reads of `length` bytes, and the caller keeps it alive and
    // unmodified for as long as the data object and everything made from it lives. A null
    // allocator is the default one, for the object itself; `kCFAllocatorNull` as the bytes
    // deallocator means CF never frees `bytes`. The Create-rule result is owned (+1) and
    // released by `CFRetained`.
    unsafe {
        CFDataCreateWithBytesNoCopy(std::ptr::null(), bytes.as_ptr(), length, kCFAllocatorNull)
    }
    // SAFETY: a non-null Create-rule result is owned by the caller.
    .map(|ptr| unsafe { CFRetained::from_raw(ptr) })
    .ok_or_else(|| PhotoError::Platform("not enough memory for the image data".to_owned()))
}

/// The EXIF of the image in `data`. Call it inside an autorelease pool, so nothing made from
/// `data` outlives the pool.
fn exif_of(data: &CFType) -> Result<ExifData, PhotoError> {
    let no_cache = no_cache_options();
    let no_cache_off = no_cache.as_opaque();
    // SAFETY: `data` and the options dictionary are live CF objects for the call. The result
    // follows the Create rule, so it is owned (+1) and `CFRetained` releases it on every path.
    let source = unsafe { CGImageSourceCreateWithData(data, Some(no_cache_off)) }
        // SAFETY: a non-null Create-rule result is owned by the caller.
        .map(|ptr| unsafe { CFRetained::from_raw(ptr) })
        .ok_or_else(undecodable)?;
    let properties = first_image_properties(&source, no_cache_off)?;
    Ok(read_exif(&properties).unwrap_or_default())
}

/// The property dictionary of `source`'s first image: the reader both [`exif_from_data`] and the
/// decoder go through. A source with no image, or one ImageIO could not parse completely, is
/// [`undecodable`].
fn first_image_properties(
    source: &CFType,
    options: &CFDictionary,
) -> Result<CFRetained<Properties>, PhotoError> {
    // SAFETY: `source` is a live image source.
    if unsafe { CGImageSourceGetCount(source) } == 0 {
        return Err(undecodable());
    }
    // SAFETY: as above; index 0 exists, checked just now. A source ImageIO could not parse
    // reports a status other than complete.
    if unsafe { CGImageSourceGetStatusAtIndex(source, 0) } != STATUS_COMPLETE {
        return Err(undecodable());
    }
    // SAFETY: as above, and `options` is a live dictionary. The Copy-rule result is owned (+1)
    // and released by `CFRetained`.
    unsafe { CGImageSourceCopyPropertiesAtIndex(source, 0, Some(options)) }
        // SAFETY: a non-null Copy-rule result is owned by the caller, and ImageIO documents it
        // as a dictionary keyed by `CFString`; values stay untyped `CFType`.
        .map(|ptr| unsafe { CFRetained::<Properties>::from_raw(ptr.cast()) })
        .ok_or_else(undecodable)
}

/// `{ kCGImageSourceShouldCache: false }`, so ImageIO keeps no decoded copy of the image.
fn no_cache_options() -> CFRetained<CFDictionary<CFString, CFBoolean>> {
    flag_options(&[
        // SAFETY: reading an ImageIO constant key, valid for the life of the process.
        unsafe { kCGImageSourceShouldCache },
    ])
}

/// Decodes `path` per `options`. The caller has run the shared [`decode::precheck`] (so
/// `max_dimension` is non-zero) and then, on iOS only, refused the main thread; macOS decodes on
/// the calling thread.
#[cfg(feature = "decode")]
pub(crate) fn decode_file(
    path: &Path,
    options: &DecodeOptions,
) -> Result<DecodedImage, PhotoError> {
    autoreleasepool(|_| decode_in_pool(path, options))
}

#[cfg(feature = "decode")]
fn decode_in_pool(path: &Path, options: &DecodeOptions) -> Result<DecodedImage, PhotoError> {
    if options.is_cancelled() {
        return Err(PhotoError::Cancelled);
    }
    decode::require_file(path)?;
    let url = CFURL::from_file_path(path)
        .ok_or_else(|| PhotoError::Platform("the path is not a valid file URL".to_owned()))?;
    let no_cache = no_cache_options();
    let no_cache_off = no_cache.as_opaque();
    // SAFETY: `url` and the options dictionary are live CF objects for the call. The result
    // follows the Create rule, so it is owned (+1) and `CFRetained` releases it on every path.
    let source = unsafe { CGImageSourceCreateWithURL(&url, Some(no_cache_off)) }
        // SAFETY: a non-null Create-rule result is owned by the caller.
        .map(|ptr| unsafe { CFRetained::from_raw(ptr) })
        .ok_or_else(undecodable)?;
    let properties = first_image_properties(&source, no_cache_off)?;
    let info = SourceInfo::read(&properties);
    info.check_capped_len(options.max_dimension)?;

    if options.is_cancelled() {
        return Err(PhotoError::Cancelled);
    }
    let image = thumbnail(&source, &info, options.max_dimension)?;

    if options.is_cancelled() {
        return Err(PhotoError::Cancelled);
    }
    let drawn = draw(&image, options.srgb, options.max_dimension)?;
    let (source_width, source_height) = info.upright_size().unwrap_or((drawn.width, drawn.height));
    Ok(DecodedImage {
        width: drawn.width,
        height: drawn.height,
        source_width,
        source_height,
        rgba: drawn.rgba,
        color_space: drawn.color_space,
        exif: info.exif,
    })
}

fn undecodable() -> PhotoError {
    PhotoError::Platform("the file has no decodable image".to_owned())
}

/// `{ key: false }` for each key.
fn flag_options(keys: &[&CFString]) -> CFRetained<CFDictionary<CFString, CFBoolean>> {
    let values = vec![CFBoolean::new(false); keys.len()];
    CFDictionary::from_slices(keys, &values)
}

/// The full-data, upright, capped image.
#[cfg(feature = "decode")]
fn thumbnail(
    source: &CFType,
    info: &SourceInfo,
    max_dimension: u32,
) -> Result<CFRetained<CGImage>, PhotoError> {
    // Never ask for more than the source has: the cap only ever shrinks. Both sides are at
    // least 1 (`SourceInfo::read`) and the precheck refused a zero cap.
    let long_edge = info
        .upright_size()
        .map_or(max_dimension, |(w, h)| w.max(h).min(max_dimension));
    let size = CFNumber::new_i64(i64::from(long_edge));
    let yes = CFBoolean::new(true);
    let no = CFBoolean::new(false);
    // SAFETY: reading ImageIO constant keys, valid for the life of the process.
    let keys: [&CFString; 4] = unsafe {
        [
            kCGImageSourceCreateThumbnailFromImageAlways,
            kCGImageSourceCreateThumbnailWithTransform,
            kCGImageSourceShouldCache,
            kCGImageSourceThumbnailMaxPixelSize,
        ]
    };
    let values: [&CFType; 4] = [yes, yes, no, &size];
    let options = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    let options = options.as_opaque();
    // SAFETY: `source` and `options` are live CF objects for the call, and index 0 exists
    // (checked by the caller). The Create-rule result is owned and released by `CFRetained`.
    unsafe { CGImageSourceCreateThumbnailAtIndex(source, 0, Some(options)) }
        // SAFETY: a non-null Create-rule result is owned by the caller.
        .map(|ptr| unsafe { CFRetained::from_raw(ptr) })
        .ok_or_else(undecodable)
}

#[cfg(feature = "decode")]
struct Drawn {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    color_space: ImageColorSpace,
}

/// The checked output size of a decoded `width x height` image: both sides non-zero, the long
/// edge within `max_dimension`, and the buffer within the shared byte ceiling. Runs before any
/// buffer is allocated.
#[cfg(feature = "decode")]
fn checked_output(
    width: usize,
    height: usize,
    max_dimension: u32,
) -> Result<(u32, u32, usize), PhotoError> {
    if width == 0 || height == 0 {
        return Err(undecodable());
    }
    let within_cap = |side: usize| u32::try_from(side).ok().filter(|v| *v <= max_dimension);
    let (Some(out_width), Some(out_height)) = (within_cap(width), within_cap(height)) else {
        return Err(PhotoError::Platform(
            "the decoded image is larger than max_dimension".to_owned(),
        ));
    };
    let len = decode::rgba_len(out_width, out_height)?;
    Ok((out_width, out_height, len))
}

/// Draw `image` 1:1 into a fresh RGBA8 context and return straight-alpha pixels.
#[cfg(feature = "decode")]
fn draw(image: &CGImage, srgb: bool, max_dimension: u32) -> Result<Drawn, PhotoError> {
    let (width, height) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    let (out_width, out_height, len) = checked_output(width, height, max_dimension)?;
    // Cannot overflow: `len` is `stride * height` and fits in `usize`.
    let stride = width * 4;
    let wide = !srgb && CGImage::color_space(Some(image)).is_some_and(|s| s.is_wide_gamut_rgb());
    // SAFETY: reading Core Graphics' constant colour-space names.
    let (p3_name, srgb_name) = unsafe { (kCGColorSpaceDisplayP3, kCGColorSpaceSRGB) };
    let (name, color_space) = if wide {
        (p3_name, ImageColorSpace::DisplayP3)
    } else {
        (srgb_name, ImageColorSpace::Srgb)
    };
    let space = CGColorSpace::with_name(Some(name))
        .ok_or_else(|| PhotoError::Platform("the output colour space is unavailable".to_owned()))?;
    let mut rgba = decode::allocate_rgba(len)?;
    let info =
        CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0);
    // SAFETY: `rgba` holds exactly `stride * height` writable bytes and outlives the context,
    // which is dropped below before the buffer is read or moved. The parameters describe an
    // 8-bit premultiplied RGBA layout Core Graphics supports.
    let context = unsafe {
        CGBitmapContextCreate(
            rgba.as_mut_ptr().cast(),
            width,
            height,
            8,
            stride,
            Some(&space),
            info.0,
        )
    }
    .ok_or_else(|| PhotoError::Platform("cannot create a bitmap context".to_owned()))?;
    let rect = CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(width as CGFloat, height as CGFloat),
    );
    CGContext::draw_image(Some(&context), rect, Some(image));
    drop(context);
    pixels::unpremultiply_rgba8_in_place(&mut rgba);
    Ok(Drawn {
        width: out_width,
        height: out_height,
        rgba,
        color_space,
    })
}

/// What the image's properties say before any pixel is decoded.
#[cfg(feature = "decode")]
#[derive(Debug, Default)]
struct SourceInfo {
    /// Stored (un-oriented) pixel size.
    width: Option<u32>,
    height: Option<u32>,
    /// EXIF orientation, `1..=8`; `1` when absent.
    orientation: u32,
    exif: Option<ExifData>,
}

#[cfg(feature = "decode")]
impl SourceInfo {
    fn read(properties: &Properties) -> Self {
        // SAFETY (all `unsafe` reads below): ImageIO constant keys, valid for the life of the
        // process.
        let (width_key, height_key, orientation_key) = unsafe {
            (
                kCGImagePropertyPixelWidth,
                kCGImagePropertyPixelHeight,
                kCGImagePropertyOrientation,
            )
        };
        let dimension = |key| {
            number(properties, key)
                .filter(|v| *v >= 1.0 && *v <= f64::from(u32::MAX))
                .map(|v| v as u32)
        };
        let orientation = number(properties, orientation_key)
            .filter(|v| (1.0..=8.0).contains(v))
            .map_or(1, |v| v as u32);
        Self {
            width: dimension(width_key),
            height: dimension(height_key),
            orientation,
            exif: read_exif(properties),
        }
    }

    /// Refuses a request whose capped output would exceed the shared byte ceiling, from the
    /// source size alone, so an oversized request fails before ImageIO decodes anything. The
    /// short side is rounded down, so a request at the edge passes here and is caught exactly
    /// after the decode ([`checked_output`]). An unknown size passes.
    fn check_capped_len(&self, max_dimension: u32) -> Result<(), PhotoError> {
        let Some((w, h)) = self.upright_size() else {
            return Ok(());
        };
        let (long, short) = (w.max(h), w.min(h));
        let capped_long = long.min(max_dimension);
        let capped_short = u64::from(short) * u64::from(capped_long) / u64::from(long);
        let capped_short = u32::try_from(capped_short).unwrap_or(u32::MAX).max(1);
        decode::rgba_len(capped_long, capped_short).map(|_| ())
    }

    /// The source size as displayed: orientations 5 to 8 turn the image a quarter.
    fn upright_size(&self) -> Option<(u32, u32)> {
        let (w, h) = (self.width?, self.height?);
        Some(if (5..=8).contains(&self.orientation) {
            (h, w)
        } else {
            (w, h)
        })
    }
}

fn read_exif(properties: &Properties) -> Option<ExifData> {
    // SAFETY (all `unsafe` reads in this function): ImageIO constant keys, valid for the life
    // of the process.
    let exif = sub_dictionary(properties, unsafe { kCGImagePropertyExifDictionary });
    let tiff = sub_dictionary(properties, unsafe { kCGImagePropertyTIFFDictionary });
    let gps = sub_dictionary(properties, unsafe { kCGImagePropertyGPSDictionary });
    let exif = exif.as_deref();
    let tiff = tiff.as_deref();
    let gps = gps.as_deref();

    let exif_string = |key| exif.and_then(|d| string(d, key));
    let exif_number = |key| exif.and_then(|d| number(d, key));
    let tiff_string = |key| tiff.and_then(|d| string(d, key));
    let measure = |key| exif_number(key).and_then(decode::positive_measure);

    let iso = exif.and_then(|d| {
        let key = unsafe { kCGImagePropertyExifISOSpeedRatings };
        first_number(d, key).or_else(|| number(d, key))
    });
    let coordinate = |value_key, ref_key, negative: &str| {
        let d = gps?;
        let value = number(d, value_key)?;
        let negate = string(d, ref_key).is_some_and(|r| r.eq_ignore_ascii_case(negative));
        Some(if negate { -value.abs() } else { value })
    };

    let data = ExifData {
        // The original capture only; TIFF `DateTime` is the last-modified time.
        capture_time: exif_string(unsafe { kCGImagePropertyExifDateTimeOriginal }),
        camera_make: tiff_string(unsafe { kCGImagePropertyTIFFMake }),
        camera_model: tiff_string(unsafe { kCGImagePropertyTIFFModel }),
        lens: exif_string(unsafe { kCGImagePropertyExifLensModel }),
        iso: iso
            .and_then(decode::positive_measure)
            .filter(|v| *v <= f64::from(u32::MAX))
            .map(|v| v.round() as u32)
            .filter(|v| *v > 0),
        exposure_seconds: measure(unsafe { kCGImagePropertyExifExposureTime }),
        aperture: measure(unsafe { kCGImagePropertyExifFNumber }),
        focal_length_mm: measure(unsafe { kCGImagePropertyExifFocalLength }),
        gps_latitude: coordinate(
            unsafe { kCGImagePropertyGPSLatitude },
            unsafe { kCGImagePropertyGPSLatitudeRef },
            "S",
        ),
        gps_longitude: coordinate(
            unsafe { kCGImagePropertyGPSLongitude },
            unsafe { kCGImagePropertyGPSLongitudeRef },
            "W",
        ),
    };
    (data != ExifData::default()).then_some(data)
}

/// A finite number stored under `key`.
fn number(dict: &Properties, key: &CFString) -> Option<f64> {
    let value = dict.get(key)?;
    value
        .downcast_ref::<CFNumber>()?
        .as_f64()
        .filter(|v| v.is_finite())
}

/// A non-empty string stored under `key`, trimmed.
fn string(dict: &Properties, key: &CFString) -> Option<String> {
    let value = dict.get(key)?;
    let s = value.downcast_ref::<CFString>()?.to_string();
    let s = s.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    (!s.is_empty()).then(|| s.to_owned())
}

/// The first element of an array of numbers stored under `key`.
fn first_number(dict: &Properties, key: &CFString) -> Option<f64> {
    let value = dict.get(key)?;
    let array = value.downcast_ref::<CFArray>()?;
    // SAFETY: the array's elements are CF objects (ImageIO stores only property-list values),
    // so viewing them as `CFType` is sound.
    let array: &CFArray<CFType> = unsafe { array.cast_unchecked() };
    let first = array.get(0)?;
    first
        .downcast_ref::<CFNumber>()?
        .as_f64()
        .filter(|v| v.is_finite())
}

/// The dictionary stored under `key`, when it is one.
fn sub_dictionary(dict: &Properties, key: &CFString) -> Option<CFRetained<Properties>> {
    let value = dict.get(key)?;
    value.downcast_ref::<CFDictionary>()?;
    // SAFETY: just checked that the value is a dictionary; ImageIO's metadata dictionaries are
    // keyed by `CFString`, and values stay untyped `CFType`.
    Some(unsafe { CFRetained::cast_unchecked(value) })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "decode")]
    use crate::cancel::CancelToken;
    use objc2_core_foundation::CFURL;
    #[cfg(feature = "decode")]
    use objc2_core_graphics::kCGColorSpaceDisplayP3;
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGBitmapInfo, CGColorSpace, CGImage,
        CGImageAlphaInfo, CGImageByteOrderInfo, kCGColorSpaceSRGB,
    };
    use std::path::{Path, PathBuf};

    // Encoding is test-only: the fixtures are written by ImageIO itself, so no binary file
    // lives in the repository.
    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        fn CGImageDestinationCreateWithURL(
            url: &CFURL,
            kind: &CFString,
            count: usize,
            options: Option<&CFDictionary>,
        ) -> Option<NonNull<CFType>>;
        fn CGImageDestinationAddImage(
            idst: &CFType,
            image: &CGImage,
            properties: Option<&CFDictionary>,
        );
        fn CGImageDestinationFinalize(idst: &CFType) -> bool;

        static kCGImagePropertyTIFFDateTime: &'static CFString;
    }

    /// ImageIO loads its private codecs by leaf name through `DYLD_LIBRARY_PATH`, so a directory
    /// there holding `libpng`/`libjpeg` (Homebrew's `lib`, for one) replaces them and the first
    /// decode dies with `SIGBUS`. When the variable is set, run test `name` again in a child
    /// process without it and report its outcome; `true` means the child ran it.
    fn rerun_without_dyld_library_path(name: &str) -> bool {
        if std::env::var_os("DYLD_LIBRARY_PATH").is_none_or(|v| v.is_empty()) {
            return false;
        }
        let module = module_path!();
        let module = module.split_once("::").map_or(module, |(_, rest)| rest);
        let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                &format!("{module}::{name}"),
                "--exact",
                "--quiet",
                "--test-threads",
                "1",
            ])
            .env_remove("DYLD_LIBRARY_PATH")
            .status()
            .expect("rerun the test");
        assert!(status.success(), "{name} failed in the child process");
        true
    }

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(tag: &str, ext: &str) -> Self {
            Self(std::env::temp_dir().join(format!(
                "frust-photo-apple-decode-{tag}-{}.{ext}",
                std::process::id()
            )))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A `width x height` image whose premultiplied RGBA8 pixels come from `pixel(x, y)`, in
    /// the named colour space.
    fn image(
        width: usize,
        height: usize,
        space: &CFString,
        pixel: impl Fn(usize, usize) -> [u8; 4],
    ) -> CFRetained<CGImage> {
        let mut buf = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let i = (y * width + x) * 4;
                buf[i..i + 4].copy_from_slice(&pixel(x, y));
            }
        }
        let space = CGColorSpace::with_name(Some(space)).expect("colour space");
        let info = CGBitmapInfo(
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
        );
        // SAFETY: `buf` holds `width * 4 * height` bytes and outlives the context, which is
        // dropped at the end of this function after the image has copied the pixels.
        let context = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                Some(&space),
                info.0,
            )
        }
        .expect("bitmap context");
        CGBitmapContextCreateImage(Some(&context)).expect("image")
    }

    /// Writes `image` as `uti` with `properties`; `false` when this system cannot encode it.
    fn write(path: &Path, uti: &str, image: &CGImage, properties: Option<&CFDictionary>) -> bool {
        let url = CFURL::from_file_path(path).expect("url");
        let uti = CFString::from_str(uti);
        // SAFETY: live CF arguments; the Create-rule result is owned by `CFRetained`.
        let dest = unsafe { CGImageDestinationCreateWithURL(&url, &uti, 1, None) };
        let Some(dest) = dest else {
            return false;
        };
        // SAFETY: a non-null Create-rule result is owned by the caller.
        let dest = unsafe { CFRetained::from_raw(dest) };
        // SAFETY: `dest`, `image` and `properties` are live CF objects for both calls.
        unsafe {
            CGImageDestinationAddImage(&dest, image, properties);
            CGImageDestinationFinalize(&dest)
        }
    }

    fn srgb_name() -> &'static CFString {
        // SAFETY: reading Core Graphics' constant colour-space name.
        unsafe { kCGColorSpaceSRGB }
    }

    #[cfg(feature = "decode")]
    fn pixel(img: &DecodedImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.rgba[i..i + 4].try_into().unwrap()
    }

    #[cfg(feature = "decode")]
    fn near(a: [u8; 4], b: [u8; 4], tolerance: u8) -> bool {
        a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tolerance)
    }

    /// 400 x 200 opaque: red left half, blue right half.
    fn halves() -> CFRetained<CGImage> {
        image(400, 200, srgb_name(), |x, _| {
            if x < 200 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            }
        })
    }

    fn jpeg_properties() -> CFRetained<CFDictionary<CFString, CFType>> {
        // SAFETY (every `unsafe` read here): ImageIO constant keys.
        let exif_keys: [&CFString; 6] = unsafe {
            [
                kCGImagePropertyExifDateTimeOriginal,
                kCGImagePropertyExifISOSpeedRatings,
                kCGImagePropertyExifExposureTime,
                kCGImagePropertyExifFNumber,
                kCGImagePropertyExifFocalLength,
                kCGImagePropertyExifLensModel,
            ]
        };
        let time = CFString::from_str("2024:05:06 07:08:09");
        let iso_value = CFNumber::new_i32(400);
        let iso = CFArray::<CFNumber>::from_objects(&[&iso_value]);
        let exposure = CFNumber::new_f64(0.01);
        let f_number = CFNumber::new_f64(1.8);
        let focal = CFNumber::new_f64(4.2);
        let lens = CFString::from_str("Fixture lens");
        let exif_values: [&CFType; 6] = [&time, &iso, &exposure, &f_number, &focal, &lens];
        let exif = CFDictionary::<CFString, CFType>::from_slices(&exif_keys, &exif_values);

        let tiff_keys: [&CFString; 2] =
            unsafe { [kCGImagePropertyTIFFMake, kCGImagePropertyTIFFModel] };
        let make = CFString::from_str("Frust");
        let model = CFString::from_str("Fixture");
        let tiff_values: [&CFType; 2] = [&make, &model];
        let tiff = CFDictionary::<CFString, CFType>::from_slices(&tiff_keys, &tiff_values);

        let gps_keys: [&CFString; 4] = unsafe {
            [
                kCGImagePropertyGPSLatitude,
                kCGImagePropertyGPSLatitudeRef,
                kCGImagePropertyGPSLongitude,
                kCGImagePropertyGPSLongitudeRef,
            ]
        };
        let lat = CFNumber::new_f64(12.5);
        let lat_ref = CFString::from_str("S");
        let lon = CFNumber::new_f64(45.25);
        let lon_ref = CFString::from_str("W");
        let gps_values: [&CFType; 4] = [&lat, &lat_ref, &lon, &lon_ref];
        let gps = CFDictionary::<CFString, CFType>::from_slices(&gps_keys, &gps_values);

        let keys: [&CFString; 4] = unsafe {
            [
                kCGImagePropertyOrientation,
                kCGImagePropertyExifDictionary,
                kCGImagePropertyTIFFDictionary,
                kCGImagePropertyGPSDictionary,
            ]
        };
        // 6: the stored image is shown turned a quarter clockwise.
        let orientation = CFNumber::new_i32(6);
        let values: [&CFType; 4] = [&orientation, &exif, &tiff, &gps];
        CFDictionary::<CFString, CFType>::from_slices(&keys, &values)
    }

    #[cfg(feature = "decode")]
    #[test]
    fn jpeg_is_capped_oriented_and_carries_exif() {
        if rerun_without_dyld_library_path("jpeg_is_capped_oriented_and_carries_exif") {
            return;
        }
        let file = TempFile::new("orient", "jpg");
        let properties = jpeg_properties();
        assert!(write(
            &file.0,
            "public.jpeg",
            &halves(),
            Some(properties.as_opaque())
        ));

        let img = decode_file(&file.0, &DecodeOptions::new(100)).expect("decode");
        // Turned upright: 200 x 400 before the cap, 50 x 100 after.
        assert_eq!((img.width, img.height), (50, 100));
        assert_eq!((img.source_width, img.source_height), (200, 400));
        assert_eq!(img.rgba.len(), 50 * 100 * 4);
        assert_eq!(img.color_space, ImageColorSpace::Srgb);
        // The stored left (red) half is now on top.
        assert!(near(pixel(&img, 25, 20), [255, 0, 0, 255], 24));
        assert!(near(pixel(&img, 25, 80), [0, 0, 255, 255], 24));

        let exif = img.exif.expect("exif");
        assert_eq!(exif.capture_time.as_deref(), Some("2024:05:06 07:08:09"));
        assert_eq!(exif.camera_make.as_deref(), Some("Frust"));
        assert_eq!(exif.camera_model.as_deref(), Some("Fixture"));
        assert_eq!(exif.lens.as_deref(), Some("Fixture lens"));
        assert_eq!(exif.iso, Some(400));
        let close = |v: Option<f64>, want: f64| v.is_some_and(|v| (v - want).abs() < 1e-6);
        assert!(close(exif.exposure_seconds, 0.01), "{exif:?}");
        assert!(close(exif.aperture, 1.8), "{exif:?}");
        assert!(close(exif.focal_length_mm, 4.2), "{exif:?}");
        assert!(close(exif.gps_latitude, -12.5), "{exif:?}");
        assert!(close(exif.gps_longitude, -45.25), "{exif:?}");
    }

    #[cfg(feature = "decode")]
    #[test]
    fn jpeg_without_orientation_keeps_its_shape_and_never_upscales() {
        if rerun_without_dyld_library_path(
            "jpeg_without_orientation_keeps_its_shape_and_never_upscales",
        ) {
            return;
        }
        let file = TempFile::new("plain", "jpg");
        assert!(write(&file.0, "public.jpeg", &halves(), None));

        let img = decode_file(&file.0, &DecodeOptions::new(160)).expect("decode");
        assert_eq!((img.width, img.height), (160, 80));
        assert_eq!((img.source_width, img.source_height), (400, 200));
        assert!(near(pixel(&img, 20, 40), [255, 0, 0, 255], 24));
        assert!(near(pixel(&img, 140, 40), [0, 0, 255, 255], 24));

        // A cap above the source size returns the source size.
        let img = decode_file(&file.0, &DecodeOptions::new(4096)).expect("decode");
        assert_eq!((img.width, img.height), (400, 200));
    }

    #[cfg(feature = "decode")]
    #[test]
    fn png_keeps_straight_alpha() {
        if rerun_without_dyld_library_path("png_keeps_straight_alpha") {
            return;
        }
        let file = TempFile::new("alpha", "png");
        // Straight (255, 128, 0, 128), stored premultiplied.
        let src = image(64, 32, srgb_name(), |_, _| [128, 64, 0, 128]);
        assert!(write(&file.0, "public.png", &src, None));

        let img = decode_file(&file.0, &DecodeOptions::new(32)).expect("decode");
        assert_eq!((img.width, img.height), (32, 16));
        assert_eq!(img.color_space, ImageColorSpace::Srgb);
        assert!(img.exif.is_none());
        let px = pixel(&img, 16, 8);
        assert!(near(px, [255, 128, 0, 128], 2), "{px:?}");
    }

    #[cfg(feature = "decode")]
    #[test]
    fn wide_gamut_png_stays_in_display_p3_unless_srgb_is_asked() {
        if rerun_without_dyld_library_path(
            "wide_gamut_png_stays_in_display_p3_unless_srgb_is_asked",
        ) {
            return;
        }
        let file = TempFile::new("p3", "png");
        // SAFETY: reading Core Graphics' constant colour-space name.
        let p3 = unsafe { kCGColorSpaceDisplayP3 };
        let src = image(8, 8, p3, |_, _| [0, 255, 0, 255]);
        assert!(write(&file.0, "public.png", &src, None));

        let img = decode_file(&file.0, &DecodeOptions::new(8)).expect("decode");
        assert_eq!(img.color_space, ImageColorSpace::DisplayP3);
        assert!(near(pixel(&img, 4, 4), [0, 255, 0, 255], 2));

        let img = decode_file(&file.0, &DecodeOptions::new(8).srgb()).expect("decode");
        assert_eq!(img.color_space, ImageColorSpace::Srgb);
        // P3 green lies outside sRGB: the red channel goes negative and clips, green stays high.
        let px = pixel(&img, 4, 4);
        assert!(px[1] > 200 && px[3] == 255, "{px:?}");
    }

    #[cfg(feature = "decode")]
    #[test]
    fn heic_decodes_when_the_system_can_encode_it() {
        if rerun_without_dyld_library_path("heic_decodes_when_the_system_can_encode_it") {
            return;
        }
        let file = TempFile::new("heic", "heic");
        if !write(&file.0, "public.heic", &halves(), None) {
            eprintln!("this system has no HEIC encoder; HEIC decode is left to the device gate");
            return;
        }
        let img = decode_file(&file.0, &DecodeOptions::new(200)).expect("decode");
        assert_eq!((img.width, img.height), (200, 100));
        assert!(near(pixel(&img, 20, 50), [255, 0, 0, 255], 32));
    }

    #[cfg(feature = "decode")]
    #[test]
    fn truncated_and_foreign_files_are_decode_errors() {
        if rerun_without_dyld_library_path("truncated_and_foreign_files_are_decode_errors") {
            return;
        }
        // Each cut keeps at most a fifth of the file, so it ends inside the headers.
        let jpeg = TempFile::new("whole", "jpg");
        assert!(write(&jpeg.0, "public.jpeg", &halves(), None));
        let png = TempFile::new("whole", "png");
        let src = image(64, 32, srgb_name(), |x, y| {
            [(x * 4) as u8, (y * 8) as u8, 7, 255]
        });
        assert!(write(&png.0, "public.png", &src, None));
        for whole in [&jpeg, &png] {
            let bytes = std::fs::read(&whole.0).unwrap();
            for keep in [bytes.len() / 5, 30, 2] {
                let cut = TempFile::new(&format!("cut{keep}"), "img");
                std::fs::write(&cut.0, &bytes[..keep]).unwrap();
                assert_eq!(
                    decode_file(&cut.0, &DecodeOptions::new(64)).map(|_| ()),
                    Err(undecodable()),
                    "{keep} of {} bytes",
                    bytes.len()
                );
            }
        }

        let text = TempFile::new("text", "jpg");
        std::fs::write(&text.0, b"not an image at all").unwrap();
        assert_eq!(
            decode_file(&text.0, &DecodeOptions::new(64)).map(|_| ()),
            Err(undecodable())
        );

        let empty = TempFile::new("empty", "png");
        std::fs::write(&empty.0, b"").unwrap();
        assert_eq!(
            decode_file(&empty.0, &DecodeOptions::new(64)).map(|_| ()),
            Err(undecodable())
        );
    }

    #[cfg(feature = "decode")]
    #[test]
    fn missing_file_and_cancel() {
        let missing = TempFile::new("missing", "jpg");
        assert_eq!(
            decode_file(&missing.0, &DecodeOptions::new(8)).map(|_| ()),
            Err(PhotoError::NotFound)
        );
        let token = CancelToken::new();
        token.cancel();
        assert_eq!(
            decode_file(&missing.0, &DecodeOptions::new(8).cancel(token)).map(|_| ()),
            Err(PhotoError::Cancelled)
        );
    }

    /// The in-memory entry point reads the same fields the decoder does, from the bytes alone.
    #[test]
    fn in_memory_bytes_carry_the_same_exif_as_the_file() {
        if rerun_without_dyld_library_path("in_memory_bytes_carry_the_same_exif_as_the_file") {
            return;
        }
        let file = TempFile::new("bytes-exif", "jpg");
        let properties = jpeg_properties();
        assert!(write(
            &file.0,
            "public.jpeg",
            &halves(),
            Some(properties.as_opaque())
        ));
        let bytes = std::fs::read(&file.0).unwrap();

        // ImageIO lets go of the borrowed bytes once the read returns: the leak guard never
        // fires for an ordinary image.
        {
            // SAFETY: `data` is dropped at the end of this block, before `bytes`.
            let data = unsafe { borrowed_data(&bytes) }.expect("data");
            let read = autoreleasepool(|_| exif_of(&data));
            assert!(read.is_ok(), "{read:?}");
            // SAFETY: `data` is a live CF object.
            assert_eq!(unsafe { CFGetRetainCount(&data) }, 1);
        }

        let exif = exif_from_data(bytes).expect("exif");
        assert_eq!(exif.capture_time.as_deref(), Some("2024:05:06 07:08:09"));
        assert_eq!(exif.camera_make.as_deref(), Some("Frust"));
        assert_eq!(exif.camera_model.as_deref(), Some("Fixture"));
        assert_eq!(exif.lens.as_deref(), Some("Fixture lens"));
        assert_eq!(exif.iso, Some(400));
        let close = |v: Option<f64>, want: f64| v.is_some_and(|v| (v - want).abs() < 1e-6);
        assert!(close(exif.exposure_seconds, 0.01), "{exif:?}");
        assert!(close(exif.aperture, 1.8), "{exif:?}");
        assert!(close(exif.focal_length_mm, 4.2), "{exif:?}");
        assert!(close(exif.gps_latitude, -12.5), "{exif:?}");
        assert!(close(exif.gps_longitude, -45.25), "{exif:?}");
    }

    #[test]
    fn in_memory_bytes_without_exif_are_empty_and_foreign_bytes_are_errors() {
        if rerun_without_dyld_library_path(
            "in_memory_bytes_without_exif_are_empty_and_foreign_bytes_are_errors",
        ) {
            return;
        }
        let file = TempFile::new("bytes-plain", "png");
        let src = image(16, 8, srgb_name(), |_, _| [0, 0, 255, 255]);
        assert!(write(&file.0, "public.png", &src, None));
        let bytes = std::fs::read(&file.0).unwrap();
        assert_eq!(exif_from_data(bytes.clone()), Ok(ExifData::default()));

        // Two bytes of the signature, not an image at all, and empty: the decoder's error. A cut
        // later in the header is not tried: ImageIO reports the properties of an in-memory PNG
        // whose header it can parse, even when the rest of the file is missing.
        for foreign in [&bytes[..2], b"not an image at all", b""] {
            assert_eq!(
                exif_from_data(foreign.to_vec()),
                Err(undecodable()),
                "{foreign:?}"
            );
        }
    }

    /// Properties holding an EXIF and a TIFF dictionary with the given entries.
    fn exif_properties(
        exif: &[(&CFString, &CFType)],
        tiff: &[(&CFString, &CFType)],
    ) -> CFRetained<Properties> {
        let dict = |entries: &[(&CFString, &CFType)]| {
            let keys: Vec<&CFString> = entries.iter().map(|(k, _)| *k).collect();
            let values: Vec<&CFType> = entries.iter().map(|(_, v)| *v).collect();
            CFDictionary::<CFString, CFType>::from_slices(&keys, &values)
        };
        let (exif, tiff) = (dict(exif), dict(tiff));
        // SAFETY: reading ImageIO constant keys.
        let keys: [&CFString; 2] = unsafe {
            [
                kCGImagePropertyExifDictionary,
                kCGImagePropertyTIFFDictionary,
            ]
        };
        let values: [&CFType; 2] = [&exif, &tiff];
        CFDictionary::<CFString, CFType>::from_slices(&keys, &values)
    }

    #[test]
    fn capture_time_comes_only_from_the_original_capture_tag() {
        let modified = CFString::from_str("2025:01:02 03:04:05");
        let original = CFString::from_str("2024:05:06 07:08:09");
        let make = CFString::from_str("Frust");
        // SAFETY (every `unsafe` read here): ImageIO constant keys.
        let (date_time, date_time_original, make_key) = unsafe {
            (
                kCGImagePropertyTIFFDateTime,
                kCGImagePropertyExifDateTimeOriginal,
                kCGImagePropertyTIFFMake,
            )
        };

        // Only the TIFF (last-modified) time: no capture time.
        let props = exif_properties(&[], &[(date_time, &modified), (make_key, &make)]);
        let exif = read_exif(&props).expect("the make is still read");
        assert_eq!(exif.capture_time, None);
        assert_eq!(exif.camera_make.as_deref(), Some("Frust"));
        // Nothing but the TIFF time: no EXIF at all.
        assert_eq!(
            read_exif(&exif_properties(&[], &[(date_time, &modified)])),
            None
        );

        // Both: the original wins.
        let props = exif_properties(
            &[(date_time_original, &original)],
            &[(date_time, &modified)],
        );
        assert_eq!(
            read_exif(&props).and_then(|e| e.capture_time).as_deref(),
            Some("2024:05:06 07:08:09")
        );
    }

    #[test]
    fn zero_negative_and_non_finite_measures_are_absent() {
        // SAFETY (every `unsafe` read here): ImageIO constant keys.
        let (iso_key, exposure_key, f_key, focal_key, lens_key) = unsafe {
            (
                kCGImagePropertyExifISOSpeedRatings,
                kCGImagePropertyExifExposureTime,
                kCGImagePropertyExifFNumber,
                kCGImagePropertyExifFocalLength,
                kCGImagePropertyExifLensModel,
            )
        };
        let lens = CFString::from_str("Fixture lens");
        for bad in [0.0, -2.0, f64::INFINITY, 0.2] {
            let iso_value = CFNumber::new_f64(bad);
            let iso = CFArray::<CFNumber>::from_objects(&[&iso_value]);
            let value = CFNumber::new_f64(if bad == 0.2 { 0.0 } else { bad });
            let props = exif_properties(
                &[
                    (iso_key, &iso),
                    (exposure_key, &value),
                    (f_key, &value),
                    (focal_key, &value),
                    (lens_key, &lens),
                ],
                &[],
            );
            let exif = read_exif(&props).expect("the lens is still read");
            // 0.2 rounds to an ISO of 0, which is absent too.
            assert_eq!(exif.iso, None, "{bad}");
            assert_eq!(exif.exposure_seconds, None, "{bad}");
            assert_eq!(exif.aperture, None, "{bad}");
            assert_eq!(exif.focal_length_mm, None, "{bad}");
            assert_eq!(exif.lens.as_deref(), Some("Fixture lens"));
        }

        // A scalar (not array) ISO and positive measures are kept.
        let iso = CFNumber::new_i32(100);
        let exposure = CFNumber::new_f64(0.5);
        let props = exif_properties(&[(iso_key, &iso), (exposure_key, &exposure)], &[]);
        let exif = read_exif(&props).expect("exif");
        assert_eq!(exif.iso, Some(100));
        assert_eq!(exif.exposure_seconds, Some(0.5));
    }

    // A real file cannot reach the ceiling in a host test: ImageIO itself refuses a JPEG header
    // of 16384 x 16384 or more as undecodable. This test and the next drive the arm's own checks
    // with the sizes a decoder would report.
    #[cfg(feature = "decode")]
    #[test]
    fn an_output_over_the_cap_or_the_ceiling_is_refused_before_allocating() {
        assert_eq!(checked_output(400, 300, 400), Ok((400, 300, 400 * 300 * 4)));
        assert_eq!(checked_output(0, 300, 400), Err(undecodable()));
        assert!(matches!(
            checked_output(401, 300, 400),
            Err(PhotoError::Platform(_))
        ));
        assert!(matches!(
            checked_output(usize::MAX, 1, u32::MAX),
            Err(PhotoError::Platform(_))
        ));
        // Within the cap but over the byte ceiling: refused, never allocated.
        assert!(matches!(
            checked_output(40_000, 40_000, 40_000),
            Err(PhotoError::InvalidRequest { .. })
        ));
    }

    #[cfg(feature = "decode")]
    #[test]
    fn an_oversized_request_is_refused_before_the_decode() {
        let info = |w, h| SourceInfo {
            width: Some(w),
            height: Some(h),
            orientation: 1,
            exif: None,
        };
        // 60000 x 60000 capped at 60000 needs 14.4 GB: refused from the header alone.
        assert!(matches!(
            info(60_000, 60_000).check_capped_len(60_000),
            Err(PhotoError::InvalidRequest { .. })
        ));
        // The same source capped near the display size is fine.
        assert_eq!(info(60_000, 60_000).check_capped_len(2048), Ok(()));
        // A long, thin source: the short side scales with the long one.
        assert_eq!(info(100_000, 10).check_capped_len(60_000), Ok(()));
        // An unknown size is left to the post-decode check.
        let unknown = SourceInfo {
            orientation: 1,
            ..SourceInfo::default()
        };
        assert_eq!(unknown.check_capped_len(60_000), Ok(()));
    }

    #[cfg(feature = "decode")]
    #[test]
    fn upright_size_turns_for_quarter_orientations() {
        let mut info = SourceInfo {
            width: Some(4),
            height: Some(3),
            orientation: 1,
            exif: None,
        };
        assert_eq!(info.upright_size(), Some((4, 3)));
        for o in 5..=8 {
            info.orientation = o;
            assert_eq!(info.upright_size(), Some((3, 4)));
        }
        info.height = None;
        assert_eq!(info.upright_size(), None);
    }
}
