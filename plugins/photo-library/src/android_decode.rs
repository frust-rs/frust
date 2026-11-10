//! Android full-resolution decode, the `decode` feature's half of the Rust-Kotlin contract in
//! [`super`]. Declared from `android.rs`; every host call, result field and code here is pinned
//! by `tests/kotlin_contract.rs`. The Kotlin side is `ImageDecodeHost`, reached through
//! `FrustPhotoLibraryHost`'s statics, so no other class is looked up from here.
//!
//! | Method | Signature (Java) | Returns |
//! |---|---|---|
//! | `decodeHeader` | `(long requestId, String path, int maxDimension) -> DecodeHeader` | the oriented source size and the size capped to `maxDimension`, or a failure code |
//! | `decodeInto` | `(long requestId, String path, int width, int height, boolean srgb, ByteBuffer pixels) -> DecodeResult` | the decoded colour space and EXIF after filling `pixels`, or a failure code |
//! | `cancelDecode` | `(long requestId) -> void` | nothing; an unknown or finished id is ignored |
//!
//! # One buffer, never a Java array
//!
//! A 48 MP RGBA8 image is about 192 MB, too large to cross JNI as a `byte[]`. `decodeHeader`
//! reads only the header and reports the capped size; this module allocates exactly
//! `width * height * 4` bytes, wraps them in a direct `ByteBuffer` (`NewDirectByteBuffer`, the
//! module's one `unsafe` block) and passes it to `decodeInto`, where `ImageDecoder` decodes with
//! the software allocator at that size and `copyPixelsToBuffer` writes premultiplied
//! `ARGB_8888` (bytes R, G, B, A). The buffer is used only inside that call. The straight-alpha
//! conversion happens here, with [`pixels::unpremultiply_rgba8_in_place`].
//!
//! # Errors
//!
//! A format with no installed decoder (`ImageDecoder.isMimeTypeSupported`, and an HEVC decoder
//! for HEIF) is [`PhotoError::Unsupported`]; a `DecodeException` or other read failure is
//! [`PhotoError::Platform`] naming the decode; a missing file is [`PhotoError::Io`], checked
//! here before any JNI call. `android.rs`'s `decode_file` arm refuses the UI thread with
//! [`PhotoError::UiThread`] before calling in.
//!
//! # Cancel
//!
//! The token's hook calls `cancelDecode`. The token is also checked here before the header,
//! between the header and the decode, and after the copy (a cancel that raced the host's answer
//! still wins); the host checks between the decode and the copy. Abandoning a request after its
//! header always tells the host, which otherwise drops it when `decodeInto` returns.
//! `ImageDecoder` applies the EXIF orientation itself, so nothing here rotates.

use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;

use jni::objects::{JClass, JObject, JString, JValue};
use jni::refs::Global;
use jni::sys::{jint, jlong};
use jni::{Env, jni_sig, jni_str};

use super::{CANCELLED, NEXT_REQUEST_ID, NOT_INITIALIZED, error_for, run_jni, with_host};
use crate::cancel::CancelToken;
use crate::decode::{DecodeOptions, DecodedImage, ImageColorSpace};
use crate::pixels;
use crate::types::{ExifData, PhotoError};

/// The call succeeded.
const OK: jint = 7;
/// No access to the file or content URI.
const PERMISSION_DENIED: jint = 8;
/// The file or content URI no longer exists.
const NOT_FOUND: jint = 9;
/// No installed decoder handles the image's format.
const NO_DECODER: jint = 14;
/// The image could not be decoded.
const DECODE_FAILED: jint = 15;

/// `DecodeResult.colorSpace`: sRGB.
const COLOR_SRGB: jint = 0;
/// `DecodeResult.colorSpace`: Display P3.
const COLOR_DISPLAY_P3: jint = 1;
/// `DecodeResult.colorSpace`: any other space.
const COLOR_OTHER: jint = 2;

/// The largest pixel buffer a `java.nio.ByteBuffer` can address: its capacity is an `int`.
const MAX_BUFFER_BYTES: usize = i32::MAX as usize;

/// `DecodeHeader` as the host reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Header {
    code: jint,
    source_width: jint,
    source_height: jint,
    width: jint,
    height: jint,
}

/// A header that passed [`checked_size`]: the capped size and the buffer length it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Size {
    width: u32,
    height: u32,
    source_width: u32,
    source_height: u32,
    bytes: usize,
}

/// `DecodeResult` as the host reported it; absent EXIF values are `None`, 0 or NaN.
#[derive(Debug, Clone, Default, PartialEq)]
struct Filled {
    code: jint,
    color_space: jint,
    capture_time: Option<String>,
    camera_make: Option<String>,
    camera_model: Option<String>,
    lens: Option<String>,
    iso: jint,
    exposure_seconds: f64,
    aperture: f64,
    focal_length_mm: f64,
    gps_latitude: f64,
    gps_longitude: f64,
}

/// `android.rs`'s `decode_file` arm after its UI-thread check. Blocks for the whole decode; see
/// the module doc.
pub(super) fn decode(path: &Path, options: &DecodeOptions) -> Result<DecodedImage, PhotoError> {
    let path = path
        .to_str()
        .ok_or_else(|| PhotoError::Platform("the file path is not valid UTF-8".to_string()))?;
    if !is_content_uri(path) {
        fs::metadata(path)?;
    }
    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    if let Some(token) = &options.cancel {
        token.set_hook(move || cancel_decode(request_id));
    }
    let result = run_decode(path, options, request_id);
    if let Some(token) = &options.cancel {
        token.clear_hook();
    }
    result
}

/// [`decode`] after the file check: the header, the buffer, the decode, the conversion.
fn run_decode(
    path: &str,
    options: &DecodeOptions,
    request_id: u64,
) -> Result<DecodedImage, PhotoError> {
    if is_cancelled(options) {
        return Err(PhotoError::Cancelled);
    }
    let max_dimension = jint::try_from(options.max_dimension).unwrap_or(jint::MAX);
    let header =
        with_host(|env, class| decode_header(env, class, request_id, path, max_dimension))?;
    if header.code != OK {
        return Err(decode_error(header.code));
    }

    // The host now holds `request_id` until `decodeInto` returns or `cancelDecode` runs.
    let prepared = checked_size(&header, options.max_dimension).and_then(|size| {
        let rgba = allocate(size.bytes)?;
        if is_cancelled(options) {
            return Err(PhotoError::Cancelled);
        }
        Ok((size, rgba))
    });
    let (size, mut rgba) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => {
            cancel_decode(request_id);
            return Err(err);
        }
    };

    let filled = with_host(|env, class| {
        decode_into(env, class, request_id, path, &size, options.srgb, &mut rgba)
    })?;
    // A cancel that raced the host's answer still wins.
    if is_cancelled(options) {
        return Err(PhotoError::Cancelled);
    }
    finish(size, rgba, filled)
}

fn is_cancelled(options: &DecodeOptions) -> bool {
    options
        .cancel
        .as_ref()
        .is_some_and(CancelToken::is_cancelled)
}

fn is_content_uri(path: &str) -> bool {
    path.starts_with("content://")
}

/// Validates the host's header against the cap: both sides positive, within `max_dimension`
/// and the source, and a buffer a `ByteBuffer` can address.
fn checked_size(header: &Header, max_dimension: u32) -> Result<Size, PhotoError> {
    let invalid = || PhotoError::Platform("the image decode host returned a bad size".to_string());
    let positive = |v: jint| u32::try_from(v).ok().filter(|v| *v > 0).ok_or_else(invalid);
    let width = positive(header.width)?;
    let height = positive(header.height)?;
    let source_width = positive(header.source_width)?;
    let source_height = positive(header.source_height)?;
    if width.max(height) > max_dimension || width > source_width || height > source_height {
        return Err(invalid());
    }
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|n| *n <= MAX_BUFFER_BYTES)
        .ok_or(PhotoError::InvalidRequest {
            reason: "the capped image is too large for one pixel buffer; lower max_dimension",
        })?;
    Ok(Size {
        width,
        height,
        source_width,
        source_height,
        bytes,
    })
}

/// A zeroed buffer of `len` bytes, or an error rather than an abort when memory is short.
fn allocate(len: usize) -> Result<Vec<u8>, PhotoError> {
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(len)
        .map_err(|_| PhotoError::Platform("not enough memory for the decoded image".to_string()))?;
    rgba.resize(len, 0);
    Ok(rgba)
}

/// The decoded image from the host's answer: straight alpha, the reported colour space, EXIF.
fn finish(size: Size, mut rgba: Vec<u8>, filled: Filled) -> Result<DecodedImage, PhotoError> {
    if filled.code != OK {
        return Err(decode_error(filled.code));
    }
    pixels::unpremultiply_rgba8_in_place(&mut rgba);
    Ok(DecodedImage {
        width: size.width,
        height: size.height,
        source_width: size.source_width,
        source_height: size.source_height,
        rgba,
        color_space: color_space(filled.color_space),
        exif: exif_from(filled),
    })
}

/// The typed error for a non-[`OK`] header or decode code.
fn decode_error(code: jint) -> PhotoError {
    match code {
        NO_DECODER => PhotoError::Unsupported,
        DECODE_FAILED => PhotoError::Platform("the image could not be decoded".to_string()),
        CANCELLED => PhotoError::Cancelled,
        NOT_FOUND => PhotoError::NotFound,
        PERMISSION_DENIED => PhotoError::PermissionDenied,
        NOT_INITIALIZED => PhotoError::PlatformNotInitialized,
        other => error_for(other),
    }
}

fn color_space(code: jint) -> ImageColorSpace {
    match code {
        COLOR_SRGB => ImageColorSpace::Srgb,
        COLOR_DISPLAY_P3 => ImageColorSpace::DisplayP3,
        COLOR_OTHER => ImageColorSpace::Other,
        // The host writes only the three codes above.
        _ => ImageColorSpace::Other,
    }
}

/// The EXIF fields the host found, or `None` when it found none.
fn exif_from(filled: Filled) -> Option<ExifData> {
    let text = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    let number = |v: f64| Some(v).filter(|v| v.is_finite());
    let exif = ExifData {
        capture_time: text(filled.capture_time),
        camera_make: text(filled.camera_make),
        camera_model: text(filled.camera_model),
        lens: text(filled.lens),
        iso: u32::try_from(filled.iso).ok().filter(|iso| *iso > 0),
        exposure_seconds: number(filled.exposure_seconds).filter(|v| *v > 0.0),
        aperture: number(filled.aperture).filter(|v| *v > 0.0),
        focal_length_mm: number(filled.focal_length_mm).filter(|v| *v > 0.0),
        gps_latitude: number(filled.gps_latitude),
        gps_longitude: number(filled.gps_longitude),
    };
    (exif != ExifData::default()).then_some(exif)
}

/// The cancel hook for `request_id`, and the cleanup for a request abandoned after its header.
/// Runs on whichever thread called [`CancelToken::cancel`].
fn cancel_decode(request_id: u64) {
    let sent = with_host(|env, class| {
        run_jni(env, "FrustPhotoLibraryHost.cancelDecode", |env| {
            env.call_static_method(
                class,
                jni_str!("cancelDecode"),
                jni_sig!("(J)V"),
                &[JValue::Long(request_id as jlong)],
            )?
            .v()
        })
    });
    if let Err(err) = sent {
        log::debug!("frust-photo-library: cancelling the decode on the host failed: {err}");
    }
}

// --- JNI calls --------------------------------------------------------------

fn decode_header(
    env: &mut Env<'_>,
    class: &Global<JClass<'static>>,
    request_id: u64,
    path: &str,
    max_dimension: jint,
) -> Result<Header, PhotoError> {
    run_jni(env, "FrustPhotoLibraryHost.decodeHeader", |env| {
        let path = env.new_string(path)?;
        let result = env
            .call_static_method(
                class,
                jni_str!("decodeHeader"),
                jni_sig!(
                    "(JLjava/lang/String;I)Ldev/frust/photolibrary/FrustPhotoLibraryHost$DecodeHeader;"
                ),
                // `u64` -> `jlong` reinterprets the same 64 bits; the host treats it as opaque.
                &[
                    JValue::Long(request_id as jlong),
                    JValue::Object(&path),
                    JValue::Int(max_dimension),
                ],
            )?
            .l()?;
        Ok(Header {
            code: int_field(env, &result, jni_str!("code"))?,
            source_width: int_field(env, &result, jni_str!("sourceWidth"))?,
            source_height: int_field(env, &result, jni_str!("sourceHeight"))?,
            width: int_field(env, &result, jni_str!("width"))?,
            height: int_field(env, &result, jni_str!("height"))?,
        })
    })
}

fn decode_into(
    env: &mut Env<'_>,
    class: &Global<JClass<'static>>,
    request_id: u64,
    path: &str,
    size: &Size,
    srgb: bool,
    rgba: &mut [u8],
) -> Result<Filled, PhotoError> {
    let (width, height) = (size.width as jint, size.height as jint);
    run_jni(env, "FrustPhotoLibraryHost.decodeInto", |env| {
        let path = env.new_string(path)?;
        // SAFETY: `NewDirectByteBuffer` wraps `rgba`'s memory without copying or owning it, so
        // that memory must stay alive and unmoved for as long as the buffer is used. `rgba` is a
        // unique borrow of the caller's `Vec` held for this whole function, so the `Vec` cannot
        // be dropped, reallocated, moved or read until this function returns. The buffer's only
        // use is the `decodeInto` call below: the host writes through it only before that call
        // returns and never keeps it (`ImageDecodeHost`'s contract, pinned by
        // `tests/kotlin_contract.rs`), and its local reference is deleted right after the call,
        // before the borrow ends. The pointer and length are the slice's own, so every byte the
        // buffer addresses is inside the allocation, and `checked_size` kept the length within a
        // `ByteBuffer`'s `int` capacity.
        let buffer = unsafe { env.new_direct_byte_buffer(rgba.as_mut_ptr(), rgba.len()) }?;
        let result = env.call_static_method(
            class,
            jni_str!("decodeInto"),
            jni_sig!(
                "(JLjava/lang/String;IIZLjava/nio/ByteBuffer;)Ldev/frust/photolibrary/FrustPhotoLibraryHost$DecodeResult;"
            ),
            &[
                JValue::Long(request_id as jlong),
                JValue::Object(&path),
                JValue::Int(width),
                JValue::Int(height),
                JValue::Bool(srgb),
                JValue::Object(&buffer),
            ],
        );
        env.delete_local_ref(buffer);
        let result = result?.l()?;
        Ok(Filled {
            code: int_field(env, &result, jni_str!("code"))?,
            color_space: int_field(env, &result, jni_str!("colorSpace"))?,
            capture_time: string_field(env, &result, jni_str!("captureTime"))?,
            camera_make: string_field(env, &result, jni_str!("cameraMake"))?,
            camera_model: string_field(env, &result, jni_str!("cameraModel"))?,
            lens: string_field(env, &result, jni_str!("lens"))?,
            iso: int_field(env, &result, jni_str!("iso"))?,
            exposure_seconds: double_field(env, &result, jni_str!("exposureSeconds"))?,
            aperture: double_field(env, &result, jni_str!("aperture"))?,
            focal_length_mm: double_field(env, &result, jni_str!("focalLengthMm"))?,
            gps_latitude: double_field(env, &result, jni_str!("gpsLatitude"))?,
            gps_longitude: double_field(env, &result, jni_str!("gpsLongitude"))?,
        })
    })
}

fn int_field(
    env: &mut Env<'_>,
    object: &JObject<'_>,
    name: &'static jni::strings::JNIStr,
) -> Result<jint, jni::errors::Error> {
    env.get_field(object, name, jni_sig!("I"))?.i()
}

fn double_field(
    env: &mut Env<'_>,
    object: &JObject<'_>,
    name: &'static jni::strings::JNIStr,
) -> Result<f64, jni::errors::Error> {
    env.get_field(object, name, jni_sig!("D"))?.d()
}

/// A nullable `String` field; its local reference is released once read.
fn string_field(
    env: &mut Env<'_>,
    object: &JObject<'_>,
    name: &'static jni::strings::JNIStr,
) -> Result<Option<String>, jni::errors::Error> {
    let value = env
        .get_field(object, name, jni_sig!("Ljava/lang/String;"))?
        .l()?;
    if value.is_null() {
        return Ok(None);
    }
    let text = env.cast_local::<JString>(value)?;
    let out = text.try_to_string(env)?;
    env.delete_local_ref(text);
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(source: (jint, jint), capped: (jint, jint)) -> Header {
        Header {
            code: OK,
            source_width: source.0,
            source_height: source.1,
            width: capped.0,
            height: capped.1,
        }
    }

    fn filled(code: jint) -> Filled {
        Filled {
            code,
            color_space: COLOR_SRGB,
            exposure_seconds: f64::NAN,
            aperture: f64::NAN,
            focal_length_mm: f64::NAN,
            gps_latitude: f64::NAN,
            gps_longitude: f64::NAN,
            ..Filled::default()
        }
    }

    #[test]
    fn host_codes_map_to_typed_errors() {
        assert_eq!(decode_error(NO_DECODER), PhotoError::Unsupported);
        assert_eq!(
            decode_error(DECODE_FAILED),
            PhotoError::Platform("the image could not be decoded".to_string())
        );
        assert_eq!(decode_error(CANCELLED), PhotoError::Cancelled);
        assert_eq!(decode_error(NOT_FOUND), PhotoError::NotFound);
        assert_eq!(
            decode_error(PERMISSION_DENIED),
            PhotoError::PermissionDenied
        );
        assert_eq!(
            decode_error(NOT_INITIALIZED),
            PhotoError::PlatformNotInitialized
        );
        assert!(matches!(decode_error(99), PhotoError::Platform(_)));
    }

    #[test]
    fn a_failed_decode_is_returned_without_pixels() {
        let size = checked_size(&header((2, 1), (2, 1)), 8).unwrap();
        for (code, want) in [
            (NO_DECODER, PhotoError::Unsupported),
            (CANCELLED, PhotoError::Cancelled),
        ] {
            assert_eq!(finish(size, vec![0; 8], filled(code)), Err(want));
        }
        assert!(matches!(
            finish(size, vec![0; 8], filled(DECODE_FAILED)),
            Err(PhotoError::Platform(_))
        ));
    }

    #[test]
    fn premultiplied_pixels_become_straight_alpha() {
        let size = checked_size(&header((2, 1), (2, 1)), 8).unwrap();
        // Half-transparent red, premultiplied, then fully transparent with stray colour.
        let image = finish(size, vec![128, 0, 0, 128, 9, 9, 9, 0], filled(OK)).unwrap();
        assert_eq!(image.rgba, vec![255, 0, 0, 128, 0, 0, 0, 0]);
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.color_space, ImageColorSpace::Srgb);
        assert_eq!(image.exif, None);
    }

    #[test]
    fn colour_space_codes() {
        assert_eq!(color_space(COLOR_SRGB), ImageColorSpace::Srgb);
        assert_eq!(color_space(COLOR_DISPLAY_P3), ImageColorSpace::DisplayP3);
        assert_eq!(color_space(COLOR_OTHER), ImageColorSpace::Other);
        assert_eq!(color_space(7), ImageColorSpace::Other);
    }

    #[test]
    fn header_sizes_are_checked_against_the_cap() {
        let size = checked_size(&header((4000, 3000), (400, 300)), 400).unwrap();
        assert_eq!(
            size,
            Size {
                width: 400,
                height: 300,
                source_width: 4000,
                source_height: 3000,
                bytes: 400 * 300 * 4,
            }
        );
        for bad in [
            header((4000, 3000), (401, 300)),
            header((4000, 3000), (0, 300)),
            header((4000, 3000), (-1, 300)),
            header((100, 100), (200, 200)),
        ] {
            assert!(matches!(
                checked_size(&bad, 400),
                Err(PhotoError::Platform(_))
            ));
        }
        assert!(matches!(
            checked_size(&header((40_000, 40_000), (40_000, 40_000)), 40_000),
            Err(PhotoError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn exif_is_none_when_empty_and_drops_placeholders() {
        assert_eq!(exif_from(filled(OK)), None);
        let exif = exif_from(Filled {
            camera_make: Some("Make".to_string()),
            lens: Some("  ".to_string()),
            iso: 200,
            aperture: 1.8,
            gps_latitude: -33.5,
            ..filled(OK)
        })
        .unwrap();
        assert_eq!(exif.camera_make.as_deref(), Some("Make"));
        assert_eq!(exif.lens, None);
        assert_eq!(exif.iso, Some(200));
        assert_eq!(exif.aperture, Some(1.8));
        assert_eq!(exif.gps_latitude, Some(-33.5));
        assert_eq!(exif.gps_longitude, None);
        assert_eq!(exif.exposure_seconds, None);
    }

    #[test]
    fn content_uris_skip_the_file_check() {
        assert!(is_content_uri("content://media/external/images/media/1"));
        assert!(!is_content_uri("/data/user/0/app/cache/a.heic"));
    }
}
