//! Android EXIF: the `DecodeResult` read shared by the decoder and
//! [`PhotoLibrary::asset_metadata`](crate::PhotoLibrary::asset_metadata), and that call's own host
//! request. Declared from `android.rs` whenever `decode` or `library` is on; the host call and its
//! codes are pinned by `tests/kotlin_contract.rs`. The Kotlin side is `AssetMetadataHost`, reached
//! through `FrustPhotoLibraryHost`'s statics, so no other class is looked up from here.
//!
//! | Method | Signature (Java) | Returns |
//! |---|---|---|
//! | `assetMetadata` | `(String id) -> DecodeResult` | the asset's EXIF, or a failure code; `colorSpace` is always `COLOR_OTHER` |
//!
//! # One result type, one EXIF read
//!
//! The host answers `assetMetadata` with the decoder's own `DecodeResult`, filled by the same
//! `ImageDecodeHost.withExif` tag reads, so both calls cross JNI through [`read_result`] and become
//! [`ExifData`] through [`exif_from`]: a zero, negative or non-finite measurement is absent on
//! both, as on the Apple arm.
//!
//! # The metadata call
//!
//! The host opens the asset's content URI as `exportOriginal` does: unredacted
//! (`MediaStore.setRequireOriginal`) only when `ACCESS_MEDIA_LOCATION` is granted, the plain URI
//! otherwise, and then reports no GPS. It reads the stream with `android.media.ExifInterface` and
//! closes it. A video answers an empty result. `MediaStore` makes no network distinction, so
//! [`MetadataOptions::allow_network`](crate::MetadataOptions::allow_network) has no effect. The
//! read is short and local, so the token is checked before it (by the crate root) and after it,
//! never during. `android.rs`'s `asset_metadata` arm refuses the UI thread before calling in.

use jni::objects::{JObject, JString};
use jni::sys::jint;
use jni::{Env, jni_sig, jni_str};

use crate::decode;
use crate::types::ExifData;

#[cfg(feature = "library")]
use jni::objects::{JClass, JValue};
#[cfg(feature = "library")]
use jni::refs::Global;

#[cfg(feature = "library")]
use super::{CANCELLED, NOT_INITIALIZED, error_for, run_jni, with_host};
#[cfg(feature = "library")]
use crate::{AssetId, MetadataOptions, PhotoError};

/// The call succeeded.
#[cfg(feature = "library")]
const OK: jint = 7;
/// No access to the asset.
#[cfg(feature = "library")]
const PERMISSION_DENIED: jint = 8;
/// The asset no longer exists, or the id is not a `MediaStore` image or video item.
#[cfg(feature = "library")]
const NOT_FOUND: jint = 9;
/// The asset's metadata could not be read.
#[cfg(feature = "library")]
const DECODE_FAILED: jint = 15;

/// `DecodeResult` as the host reported it; absent EXIF values are `None`, 0 or NaN. The host
/// reads `captureTime` from EXIF `DateTimeOriginal` only.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Filled {
    pub(super) code: jint,
    // Read by the decoder only; the metadata call always gets `COLOR_OTHER`.
    #[cfg_attr(not(feature = "decode"), allow(dead_code))]
    pub(super) color_space: jint,
    pub(super) capture_time: Option<String>,
    pub(super) camera_make: Option<String>,
    pub(super) camera_model: Option<String>,
    pub(super) lens: Option<String>,
    pub(super) iso: jint,
    pub(super) exposure_seconds: f64,
    pub(super) aperture: f64,
    pub(super) focal_length_mm: f64,
    pub(super) gps_latitude: f64,
    pub(super) gps_longitude: f64,
}

/// The EXIF fields the host found, or `None` when it found none.
pub(super) fn exif_from(filled: Filled) -> Option<ExifData> {
    let text = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    let number = |v: f64| Some(v).filter(|v| v.is_finite());
    // Zero, negative and non-finite measurements are absent, as on the Apple arm.
    let exif = ExifData {
        capture_time: text(filled.capture_time),
        camera_make: text(filled.camera_make),
        camera_model: text(filled.camera_model),
        lens: text(filled.lens),
        iso: u32::try_from(filled.iso).ok().filter(|iso| *iso > 0),
        exposure_seconds: decode::positive_measure(filled.exposure_seconds),
        aperture: decode::positive_measure(filled.aperture),
        focal_length_mm: decode::positive_measure(filled.focal_length_mm),
        gps_latitude: number(filled.gps_latitude),
        gps_longitude: number(filled.gps_longitude),
    };
    (exif != ExifData::default()).then_some(exif)
}

/// Every field of a `DecodeResult`, read once for both calls.
pub(super) fn read_result(
    env: &mut Env<'_>,
    result: &JObject<'_>,
) -> Result<Filled, jni::errors::Error> {
    Ok(Filled {
        code: int_field(env, result, jni_str!("code"))?,
        color_space: int_field(env, result, jni_str!("colorSpace"))?,
        capture_time: string_field(env, result, jni_str!("captureTime"))?,
        camera_make: string_field(env, result, jni_str!("cameraMake"))?,
        camera_model: string_field(env, result, jni_str!("cameraModel"))?,
        lens: string_field(env, result, jni_str!("lens"))?,
        iso: int_field(env, result, jni_str!("iso"))?,
        exposure_seconds: double_field(env, result, jni_str!("exposureSeconds"))?,
        aperture: double_field(env, result, jni_str!("aperture"))?,
        focal_length_mm: double_field(env, result, jni_str!("focalLengthMm"))?,
        gps_latitude: double_field(env, result, jni_str!("gpsLatitude"))?,
        gps_longitude: double_field(env, result, jni_str!("gpsLongitude"))?,
    })
}

pub(super) fn int_field(
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

/// `android.rs`'s `asset_metadata` arm after the crate root's cancel check and the arm's
/// UI-thread check. Blocks for the read; see the module doc.
#[cfg(feature = "library")]
pub(super) fn asset_metadata(
    id: &AssetId,
    options: &MetadataOptions,
) -> Result<ExifData, PhotoError> {
    let filled = with_host(|env, class| request_metadata(env, class, id.as_str()))?;
    // A cancel that raced the host's answer still wins.
    if options.is_cancelled() {
        return Err(PhotoError::Cancelled);
    }
    metadata_result(filled)
}

/// The EXIF for an [`OK`] answer (empty when the asset has none), the typed error otherwise.
#[cfg(feature = "library")]
fn metadata_result(filled: Filled) -> Result<ExifData, PhotoError> {
    if filled.code != OK {
        return Err(metadata_error(filled.code));
    }
    Ok(exif_from(filled).unwrap_or_default())
}

/// The typed error for a non-[`OK`] metadata code.
#[cfg(feature = "library")]
fn metadata_error(code: jint) -> PhotoError {
    match code {
        NOT_FOUND => PhotoError::NotFound,
        PERMISSION_DENIED => PhotoError::PermissionDenied,
        NOT_INITIALIZED => PhotoError::PlatformNotInitialized,
        DECODE_FAILED => PhotoError::Platform("the asset's metadata could not be read".to_string()),
        CANCELLED => PhotoError::Cancelled,
        other => error_for(other),
    }
}

#[cfg(feature = "library")]
fn request_metadata(
    env: &mut Env<'_>,
    class: &Global<JClass<'static>>,
    id: &str,
) -> Result<Filled, PhotoError> {
    run_jni(env, "FrustPhotoLibraryHost.assetMetadata", |env| {
        let id = env.new_string(id)?;
        let result = env
            .call_static_method(
                class,
                jni_str!("assetMetadata"),
                jni_sig!("(Ljava/lang/String;)Ldev/frust/photolibrary/FrustPhotoLibraryHost$DecodeResult;"),
                &[JValue::Object(&id)],
            )?
            .l()?;
        read_result(env, &result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(code: jint) -> Filled {
        Filled {
            code,
            color_space: 2,
            exposure_seconds: f64::NAN,
            aperture: f64::NAN,
            focal_length_mm: f64::NAN,
            gps_latitude: f64::NAN,
            gps_longitude: f64::NAN,
            ..Filled::default()
        }
    }

    #[test]
    fn exif_is_none_when_empty_and_drops_placeholders() {
        assert_eq!(exif_from(filled(7)), None);
        let exif = exif_from(Filled {
            camera_make: Some("Make".to_string()),
            lens: Some("  ".to_string()),
            iso: 200,
            aperture: 1.8,
            gps_latitude: -33.5,
            ..filled(7)
        })
        .unwrap();
        assert_eq!(exif.camera_make.as_deref(), Some("Make"));
        assert_eq!(exif.lens, None);
        assert_eq!(exif.iso, Some(200));
        assert_eq!(exif.aperture, Some(1.8));
        assert_eq!(exif.gps_latitude, Some(-33.5));
        assert_eq!(exif.gps_longitude, None);
        assert_eq!(exif.exposure_seconds, None);

        // Zero, negative and non-finite measurements are absent.
        for bad in [0.0, -1.0, f64::INFINITY] {
            let exif = exif_from(Filled {
                camera_make: Some("Make".to_string()),
                iso: -5,
                exposure_seconds: bad,
                aperture: bad,
                focal_length_mm: bad,
                ..filled(7)
            })
            .unwrap();
            assert_eq!(exif.iso, None);
            assert_eq!(exif.exposure_seconds, None);
            assert_eq!(exif.aperture, None);
            assert_eq!(exif.focal_length_mm, None);
        }
    }

    #[cfg(feature = "library")]
    #[test]
    fn metadata_codes_map_to_typed_errors_and_ok_is_never_an_error() {
        assert_eq!(metadata_error(NOT_FOUND), PhotoError::NotFound);
        assert_eq!(
            metadata_error(PERMISSION_DENIED),
            PhotoError::PermissionDenied
        );
        assert_eq!(
            metadata_error(NOT_INITIALIZED),
            PhotoError::PlatformNotInitialized
        );
        assert!(matches!(
            metadata_error(DECODE_FAILED),
            PhotoError::Platform(_)
        ));
        assert_eq!(metadata_error(CANCELLED), PhotoError::Cancelled);
        assert!(matches!(metadata_error(99), PhotoError::Platform(_)));

        assert_eq!(metadata_result(filled(OK)), Ok(ExifData::default()));
        assert_eq!(
            metadata_result(Filled {
                camera_model: Some("Model".to_string()),
                ..filled(OK)
            })
            .map(|e| e.camera_model),
            Ok(Some("Model".to_string()))
        );
        assert_eq!(
            metadata_result(filled(NOT_FOUND)),
            Err(PhotoError::NotFound)
        );
    }
}
