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

const PROGRESSIVE_444: &[u8] = include_bytes!("fixtures/progressive-444.jpg");
const PROGRESSIVE_420: &[u8] = include_bytes!("fixtures/progressive-420.jpg");
const BASELINE_444: &[u8] = include_bytes!("fixtures/baseline-444.jpg");

/// 64x48 px fixtures; 3 B/px native; no target, so the RGBA copy is 4 B/px.
const FIXTURE_PX: u64 = 64 * 48;
const RGBA_COPY: u64 = 4 * FIXTURE_PX;
/// 4:4:4 store: 2 B x 3 components x 1x1 = 6 B/px (64x48 is whole 8x8 MCUs).
const STORE_444: u64 = 6 * FIXTURE_PX;
const ESTIMATE_444: u64 = 3 * FIXTURE_PX + STORE_444;
/// The 4:2:0 store (2 B x 6 blocks per 16x16 MCU = 3 B/px) is below the 4 B/px RGBA copy, which then dominates.
const ESTIMATE_420: u64 = 3 * FIXTURE_PX + RGBA_COPY;

/// Component sampling factors `(h, v)` of the first SOF in `jpeg`, with its marker.
fn sof(jpeg: &[u8]) -> (u8, Vec<(u8, u8)>) {
    let at = jpeg
        .windows(2)
        .position(|w| w[0] == 0xFF && matches!(w[1], 0xC0 | 0xC2))
        .expect("SOF marker");
    let count = usize::from(jpeg[at + 9]);
    let factors = (0..count)
        .map(|i| {
            let hv = jpeg[at + 11 + i * 3];
            (hv >> 4, hv & 0x0F)
        })
        .collect();
    (jpeg[at + 1], factors)
}

fn over_budget_estimate(body: &[u8], budget: u64) -> u64 {
    let limits = DecodeLimits {
        max_alloc_bytes: budget,
        ..DecodeLimits::default()
    };
    match run(loader(body.to_vec(), limits).load(request())) {
        Err(LoadError::Decode(DecodeError::OverBudget {
            estimated_peak_bytes,
            ..
        })) => estimated_peak_bytes,
        other => panic!("expected OverBudget, got {:?}", other.map(|_| ())),
    }
}

fn decodes_under(body: &[u8], budget: u64) -> bool {
    let limits = DecodeLimits {
        max_alloc_bytes: budget,
        ..DecodeLimits::default()
    };
    run(loader(body.to_vec(), limits).load(request())).is_ok()
}

#[test]
fn fixtures_carry_the_expected_frame_headers() {
    assert_eq!(sof(PROGRESSIVE_444), (0xC2, vec![(1, 1); 3]));
    assert_eq!(sof(PROGRESSIVE_420), (0xC2, vec![(2, 2), (1, 1), (1, 1)]));
    assert_eq!(sof(BASELINE_444).0, 0xC0);
}

#[test]
fn a_progressive_jpeg_decodes_under_the_default_limits() {
    let src = run(decode_source(PROGRESSIVE_444.to_vec(), None)).expect("decode");
    let size = src.natural_size();
    assert_eq!((size.width, size.height), (64.0, 48.0));
}

#[test]
fn a_progressive_jpeg_counts_its_coefficient_store() {
    assert_eq!(
        over_budget_estimate(PROGRESSIVE_444, ESTIMATE_444 - 1),
        ESTIMATE_444
    );
    assert!(decodes_under(PROGRESSIVE_444, ESTIMATE_444));
}

#[test]
fn a_420_progressive_store_is_smaller_than_a_444_one() {
    assert_eq!(
        over_budget_estimate(PROGRESSIVE_420, ESTIMATE_420 - 1),
        ESTIMATE_420
    );
    assert_eq!(ESTIMATE_444 - ESTIMATE_420, STORE_444 - RGBA_COPY);
}

#[test]
fn a_baseline_jpeg_is_not_charged_the_store() {
    // Rejects the progressive twin, but baseline only pays native + RGBA copy.
    assert!(!decodes_under(PROGRESSIVE_444, ESTIMATE_444 - 1));
    assert!(decodes_under(BASELINE_444, ESTIMATE_444 - 1));
    assert_eq!(
        over_budget_estimate(BASELINE_444, 3 * FIXTURE_PX + RGBA_COPY - 1),
        3 * FIXTURE_PX + RGBA_COPY
    );
}
