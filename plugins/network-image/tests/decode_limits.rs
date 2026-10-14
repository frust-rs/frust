//! Decode limits: header pre-check and `image` allocation caps.

use std::io::Cursor;
use std::sync::Arc;

use frust_network_image::{
    DecodeError, DecodeLimits, DiskCacheConfig, FetchFuture, FetchRequest, FetchedBytes,
    ImageFetcher, ImageLoader, LoadError, LoadRequest, LoaderConfig, TargetPx, decode_source,
};

fn run<F: std::future::Future>(fut: F) -> F::Output {
    frust_reactive::ReactiveRuntime::init(Arc::new(|| {}));
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(fut)
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([x as u8, y as u8, 7]));
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A small PNG whose IHDR declares `w` x `h`; the body is not a valid image.
fn bomb(w: u32, h: u32) -> Vec<u8> {
    let mut bytes = png(4, 4);
    // Signature (8) + length (4) + "IHDR" (4), then width, height, ..., CRC.
    bytes[16..20].copy_from_slice(&w.to_be_bytes());
    bytes[20..24].copy_from_slice(&h.to_be_bytes());
    let crc = crc32(&bytes[12..29]);
    bytes[29..33].copy_from_slice(&crc.to_be_bytes());
    bytes
}

struct Serve(Vec<u8>);

impl ImageFetcher for Serve {
    fn fetch(&self, _request: FetchRequest) -> FetchFuture {
        let bytes = self.0.clone();
        Box::pin(async move {
            Ok(FetchedBytes {
                bytes,
                content_type: None,
            })
        })
    }
}

fn loader(body: Vec<u8>, limits: DecodeLimits) -> ImageLoader {
    ImageLoader::new(
        Arc::new(Serve(body)),
        LoaderConfig {
            decode_limits: limits,
            disk: DiskCacheConfig {
                enabled: false,
                ..DiskCacheConfig::default()
            },
            ..LoaderConfig::default()
        },
    )
}

fn request() -> LoadRequest {
    LoadRequest {
        url: "https://x/i.png".into(),
        headers: Vec::new(),
        target: None,
    }
}

#[test]
fn declared_20000_square_is_rejected_from_the_header() {
    let started = std::time::Instant::now();
    let err = run(decode_source(bomb(20000, 20000), None)).expect_err("bomb");
    assert!(
        matches!(
            err,
            DecodeError::TooLarge {
                width: 20000,
                height: 20000,
                max_width: 8192,
                max_height: 8192
            }
        ),
        "{err:?}"
    );
    assert!(started.elapsed().as_secs() < 5, "rejected quickly");
}

#[test]
fn alloc_cap_rejects_a_valid_image() {
    let l = loader(
        png(2000, 2000),
        DecodeLimits {
            max_alloc_bytes: 1024 * 1024,
            ..DecodeLimits::default()
        },
    );
    let err = run(l.load(request())).expect_err("over the alloc cap");
    assert!(
        matches!(err, LoadError::Decode(DecodeError::OverBudget { .. })),
        "{err:?}"
    );
}

#[test]
fn resize_intermediate_counts_against_the_budget() {
    let limits = DecodeLimits {
        max_alloc_bytes: 12 * 1024 * 1024,
        ..DecodeLimits::default()
    };
    let mut req = request();
    req.target = Some(TargetPx {
        width: 300,
        height: 300,
    });
    let err = run(loader(png(1200, 1200), limits).load(req)).expect_err("resize set too big");
    assert!(
        matches!(err, LoadError::Decode(DecodeError::OverBudget { .. })),
        "{err:?}"
    );
    let src = run(loader(png(1200, 1200), limits).load(request())).expect("fits unscaled");
    let size = src.natural_size();
    assert_eq!((size.width, size.height), (1200.0, 1200.0));
}

#[test]
fn sixteen_bit_sources_are_estimated_at_their_depth() {
    let img = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_fn(500, 500, |x, y| {
        image::Rgb([x as u16, y as u16, 7])
    });
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb16(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode png");
    let limits = DecodeLimits {
        max_alloc_bytes: 2 * 1024 * 1024,
        ..DecodeLimits::default()
    };
    let err = run(loader(out.into_inner(), limits).load(request())).expect_err("16-bit over");
    assert!(
        matches!(err, LoadError::Decode(DecodeError::OverBudget { .. })),
        "{err:?}"
    );
    run(loader(png(500, 500), limits).load(request())).expect("8-bit fits");
}

#[test]
fn an_image_under_the_limits_decodes() {
    let src = run(decode_source(png(1000, 1000), None)).expect("decode");
    let size = src.natural_size();
    assert_eq!((size.width, size.height), (1000.0, 1000.0));
}

#[test]
fn loader_honours_dimension_limits() {
    let l = loader(
        png(200, 200),
        DecodeLimits {
            max_height: 100,
            ..DecodeLimits::default()
        },
    );
    let err = run(l.load(request())).expect_err("too tall");
    assert!(
        matches!(
            err,
            LoadError::Decode(DecodeError::TooLarge { height: 200, .. })
        ),
        "{err:?}"
    );
}
