//! WebP decode behind the optional `webp` feature.

use std::io::Cursor;
use std::sync::Arc;

use frust_network_image::decode_source;

fn run<F: std::future::Future>(fut: F) -> F::Output {
    frust_reactive::ReactiveRuntime::init(Arc::new(|| {}));
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(fut)
}

/// A WebP body encoded at test time by the image crate round-trips.
#[cfg(feature = "webp")]
#[test]
fn webp_round_trips() {
    let img = image::RgbaImage::from_fn(16, 12, |x, y| {
        image::Rgba([x as u8 * 10, y as u8 * 10, 7, 255])
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::WebP)
        .expect("encode webp");
    let src = run(decode_source(out.into_inner(), None)).expect("decode webp");
    let size = src.natural_size();
    assert_eq!((size.width, size.height), (16.0, 12.0));
}

/// Without the feature, WebP bytes answer `UnsupportedFormat` naming the format.
#[cfg(not(feature = "webp"))]
#[test]
fn webp_without_feature_is_unsupported() {
    let _ = Cursor::new(Vec::<u8>::new());
    let mut bytes = b"RIFF\x1a\0\0\0WEBPVP8L".to_vec();
    bytes.extend_from_slice(&[0; 16]);
    let err = run(decode_source(bytes, None)).expect_err("must not decode");
    assert!(
        matches!(err, frust_network_image::DecodeError::UnsupportedFormat(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("WebP"), "{err}");
}
