//! Glyph runs in an `hvgl`-only face: Apple's PingFang, whose outlines live
//! in an `hvgl` table `glifo`'s `skrifa` cannot read.
//!
//! On Apple targets the engine fills such a run's glyphs from CoreText's own
//! outlines; everywhere else, and for a face CoreText cannot open faithfully,
//! every glyph is counted as skipped and nothing is drawn. What is pinned:
//!
//! - **Off CoreText, or refused, an `hvgl` run is skipped whole.** A synthetic
//!   `hvgl`-only face (a `head`, a `name` and an `hvgl` table, no outline
//!   table) draws nothing and counts every glyph in `skipped_glyphs` — on
//!   every target: a target without CoreText refuses every face, and on an
//!   Apple target a hidden (dot-prefixed) name or a name CoreText resolves to
//!   a different face is refused rather than drawn in the substitute.
//! - **On macOS the system PingFang draws.** A run of 中文爱 in
//!   `PingFangUI.ttc`'s `PingFangSC-Medium` face, glyph ids taken from
//!   CoreText's own character map, produces one draw per glyph with real
//!   coverage and skips nothing; the same glyphs in a dot-named face of the
//!   same collection are skipped. The outline cache stays at its cap however
//!   many distinct glyphs are drawn. These cases print why and pass when the
//!   collection is not installed.
//! - **Through the GPU** (ignored by default, needs Metal): the same run read
//!   back from a headless target carries ink.

use std::sync::Arc;

use frust_engine::cache::images::ImageResidency;
use frust_engine::compile::CompiledFrame;
use frust_engine::{AtlasBudget, SceneCompiler};
use frust_scene::{FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use kurbo::Affine;
use peniko::{Blob, Brush, Color, FontData};

/// Viewport every case compiles against.
const VIEWPORT: (u16, u16) = (128, 64);

/// Font size every case draws at.
const FONT_SIZE: f32 = 24.0;

/// A single-face sfnt holding `tables` (tag, bytes), table data 4-aligned.
fn sfnt(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let count = u16::try_from(tables.len()).expect("a handful of tables");
    let mut out = vec![0x00, 0x01, 0x00, 0x00];
    out.extend(count.to_be_bytes());
    out.extend([0; 6]);
    let mut offset = 12 + 16 * tables.len();
    let mut data = Vec::new();
    for (tag, bytes) in tables {
        out.extend(tag);
        out.extend([0; 4]);
        out.extend(u32::try_from(offset).expect("small").to_be_bytes());
        out.extend(u32::try_from(bytes.len()).expect("small").to_be_bytes());
        let padded = bytes.len().div_ceil(4) * 4;
        data.extend(bytes);
        data.resize(data.len() + padded - bytes.len(), 0);
        offset += padded;
    }
    out.extend(data);
    out
}

/// A 54-byte `head` table carrying `units_per_em`.
fn head(units_per_em: u16) -> Vec<u8> {
    let mut head = vec![0; 54];
    head[18..20].copy_from_slice(&units_per_em.to_be_bytes());
    head
}

/// A `name` table whose one record is a Windows-Unicode PostScript name.
fn name_table(postscript_name: &str) -> Vec<u8> {
    let bytes: Vec<u8> = postscript_name
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    let mut out = Vec::new();
    for field in [0_u16, 1, 18, 3, 1, 0x409, 6] {
        out.extend(field.to_be_bytes());
    }
    out.extend(u16::try_from(bytes.len()).expect("short").to_be_bytes());
    out.extend(0_u16.to_be_bytes());
    out.extend(bytes);
    out
}

/// An `hvgl`-only face named `postscript_name`, with no outline table at all.
fn synthetic_hvgl_face(postscript_name: &str) -> FontHandle {
    let bytes = sfnt(&[
        (*b"head", head(1000)),
        (*b"hvgl", vec![0; 16]),
        (*b"name", name_table(postscript_name)),
    ]);
    FontHandle::new(FontData::new(Blob::new(Arc::new(bytes)), 0))
}

/// A run of `ids` on one baseline, inside the viewport.
fn run(font: FontHandle, ids: &[u32]) -> GlyphRun {
    GlyphRun {
        font,
        font_size: FONT_SIZE,
        brush: Brush::Solid(Color::BLACK),
        transform: Affine::translate((8.0, 44.0)),
        glyphs: ids
            .iter()
            .enumerate()
            .map(|(index, id)| Glyph {
                id: *id,
                x: index as f32 * FONT_SIZE,
                y: 0.0,
            })
            .collect(),
    }
}

fn scene_of(runs: Vec<GlyphRun>) -> Scene {
    let mut scene = Scene::new();
    let mut builder = SceneBuilder::new(&mut scene);
    for glyph_run in runs {
        builder.draw_glyph_run(glyph_run);
    }
    scene
}

/// A compiler with the atlas left on: an `hvgl` run must take the outline
/// route on its own, not because the atlas happened to be off.
fn compiler() -> SceneCompiler {
    SceneCompiler::new(VIEWPORT.0, VIEWPORT.1)
}

fn compile(compiler: &mut SceneCompiler, runs: Vec<GlyphRun>) -> CompiledFrame {
    compiler
        .compile(&scene_of(runs), Affine::IDENTITY, VIEWPORT)
        .expect("an in-range scene compiles")
}

#[test]
fn the_compiler_stays_send_and_sync_with_the_hvgl_provider_inside() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SceneCompiler>();
}

/// A face named for nothing installed: off Apple there is no CoreText at all,
/// and on Apple CoreText substitutes another face for an unknown name, which
/// the provider refuses because its PostScript name differs.
#[test]
fn an_hvgl_face_the_provider_cannot_open_is_skipped_whole_and_draws_nothing() {
    let mut compiler = compiler();
    for frame_index in 0..2 {
        let frame = compile(
            &mut compiler,
            vec![run(
                synthetic_hvgl_face("FrustNoSuchFace-Regular"),
                &[1, 2, 3],
            )],
        );
        assert_eq!(frame.skipped_glyphs, 3, "frame {frame_index}");
        assert_eq!(frame.glyph_draws, 0, "frame {frame_index}");
        assert!(frame.draws().is_empty(), "frame {frame_index}");
        assert_eq!(frame.atlas_glyph_draws, 0, "frame {frame_index}");
    }
    assert_eq!(compiler.hvgl_cached_outlines(), 0);
}

/// A dot-prefixed name is the shape of Apple's hidden system faces, which
/// `CTFontCreateWithName` resolves to Times rather than the face.
#[test]
fn a_dot_named_hvgl_face_is_refused_and_counted_as_skipped() {
    let mut compiler = compiler();
    let frame = compile(
        &mut compiler,
        vec![run(
            synthetic_hvgl_face(".PingFangUITextSC-Default"),
            &[10, 11],
        )],
    );
    assert_eq!(frame.skipped_glyphs, 2);
    assert_eq!(frame.glyph_draws, 0);
    assert!(frame.draws().is_empty());
}

/// The cap is a fixed constant, not zero: a compiler that kept nothing would
/// pass the eviction case trivially.
#[test]
fn the_hvgl_outline_cache_has_a_real_cap() {
    let compiler = compiler();
    assert!(compiler.hvgl_outline_capacity() >= 256);
    assert_eq!(compiler.hvgl_cached_outlines(), 0);
}

/// An image residency switched off must not change an `hvgl` run's answer:
/// it never asked the atlas for anything.
#[test]
fn an_hvgl_run_is_skipped_identically_with_the_atlas_off() {
    let mut compiler = compiler();
    compiler.set_image_residency(ImageResidency::disabled(AtlasBudget::MOBILE));
    let frame = compile(
        &mut compiler,
        vec![run(synthetic_hvgl_face("FrustNoSuchFace-Bold"), &[4, 5])],
    );
    assert_eq!(frame.skipped_glyphs, 2);
    assert_eq!(frame.glyph_draws, 0);
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
    use frust_gpu::{HeadlessTarget, TierCaps};
    use frust_scene::FontHandle;
    use kurbo::Affine;
    use objc2_core_foundation::CFString;
    use objc2_core_text::CTFont;
    use peniko::{Blob, Color, FontData};

    use super::{VIEWPORT, compile, compiler, run, scene_of};

    /// Where macOS keeps the PingFang collection. It moved into a private
    /// framework's reserved fonts; older releases kept it in `Fonts`.
    const PINGFANG_PATHS: [&str; 2] = [
        "/System/Library/PrivateFrameworks/FontServices.framework/Versions/A/Resources/Reserved/PingFangUI.ttc",
        "/System/Library/Fonts/PingFangUI.ttc",
    ];

    /// The collection face drawn: a public (non-hidden) PostScript name.
    const FACE: &str = "PingFangSC-Medium";

    /// The text drawn — three Han characters PingFang covers.
    const TEXT: &str = "中文爱";

    /// The system collection's bytes, or `None` (with the reason printed)
    /// when it is not installed.
    fn pingfang() -> Option<Arc<Vec<u8>>> {
        let found = PINGFANG_PATHS
            .iter()
            .find_map(|path| std::fs::read(path).ok());
        if found.is_none() {
            println!("skipped: PingFangUI.ttc is not installed at any of {PINGFANG_PATHS:?}");
        }
        found.map(Arc::new)
    }

    fn be_u16(data: &[u8], at: usize) -> u16 {
        u16::from_be_bytes([data[at], data[at + 1]])
    }

    fn be_u32(data: &[u8], at: usize) -> u32 {
        u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
    }

    /// Every face of the collection `data`, as `(index, PostScript name)`.
    fn postscript_names(data: &[u8]) -> Vec<(u32, String)> {
        assert_eq!(&data[0..4], b"ttcf", "PingFangUI.ttc is a collection");
        (0..be_u32(data, 8))
            .filter_map(|index| {
                let directory = be_u32(data, 12 + 4 * index as usize) as usize;
                let tables = be_u16(data, directory + 4) as usize;
                let name = (0..tables)
                    .map(|table| directory + 12 + 16 * table)
                    .find(|record| &data[*record..*record + 4] == b"name")?;
                let table = be_u32(data, name + 8) as usize;
                let strings = table + be_u16(data, table + 4) as usize;
                (0..be_u16(data, table + 2) as usize)
                    .map(|record| table + 6 + 12 * record)
                    .find(|record| be_u16(data, *record) == 3 && be_u16(data, record + 6) == 6)
                    .map(|record| {
                        let start = strings + be_u16(data, record + 10) as usize;
                        let len = be_u16(data, record + 8) as usize;
                        let units: Vec<u16> = (0..len / 2)
                            .map(|unit| be_u16(data, start + 2 * unit))
                            .collect();
                        (index, String::from_utf16_lossy(&units))
                    })
            })
            .collect()
    }

    /// The face of `data` named `name`.
    fn face(data: &Arc<Vec<u8>>, name: &str) -> FontHandle {
        let (index, _) = postscript_names(data)
            .into_iter()
            .find(|(_, face)| face == name)
            .unwrap_or_else(|| panic!("PingFangUI.ttc has no face named {name}"));
        FontHandle::new(FontData::new(Blob::new(data.clone()), index))
    }

    /// The first hidden (dot-named) face of `data`.
    fn hidden_face(data: &Arc<Vec<u8>>) -> FontHandle {
        let (index, _) = postscript_names(data)
            .into_iter()
            .find(|(_, face)| face.starts_with('.'))
            .expect("PingFangUI.ttc carries hidden system faces");
        FontHandle::new(FontData::new(Blob::new(data.clone()), index))
    }

    /// CoreText's own glyph ids for `text` in the installed face `name`.
    fn coretext_glyphs(name: &str, text: &str) -> Vec<u32> {
        let mut characters: Vec<u16> = text.encode_utf16().collect();
        let mut glyphs = vec![0_u16; characters.len()];
        // SAFETY: a null matrix is the documented identity argument.
        let font = unsafe { CTFont::with_name(&CFString::from_str(name), 24.0, std::ptr::null()) };
        // SAFETY: both buffers hold `characters.len()` elements.
        let mapped = unsafe {
            font.glyphs_for_characters(
                NonNull::new(characters.as_mut_ptr()).expect("non-empty"),
                NonNull::new(glyphs.as_mut_ptr()).expect("non-empty"),
                characters.len() as isize,
            )
        };
        assert!(
            mapped,
            "CoreText maps every character of {text:?} in {name}"
        );
        glyphs.into_iter().map(u32::from).collect()
    }

    #[test]
    fn pingfang_han_glyphs_draw_from_coretext_outlines() {
        let Some(data) = pingfang() else { return };
        let glyphs = coretext_glyphs(FACE, TEXT);
        assert!(glyphs.iter().all(|glyph| *glyph != 0), "{glyphs:?}");
        // One handle for every frame: the cache is keyed on the blob's
        // identity, as a shaped run's font is from frame to frame.
        let font = face(&data, FACE);
        let mut compiler = compiler();
        for frame_index in 0..2 {
            let frame = compile(&mut compiler, vec![run(font.clone(), &glyphs)]);
            assert_eq!(frame.skipped_glyphs, 0, "frame {frame_index}");
            assert_eq!(frame.glyph_draws, 3, "frame {frame_index}");
            assert_eq!(
                frame.atlas_glyph_draws, 0,
                "an hvgl run never samples the atlas"
            );
            assert!(
                frame.alphas().iter().any(|alpha| *alpha != 0),
                "frame {frame_index}: the glyphs carry coverage"
            );
            assert!(!frame.strip_buf().is_empty(), "frame {frame_index}");
        }
        assert_eq!(compiler.hvgl_cached_outlines(), 3, "cached once, reused");
    }

    /// CoreText returns no path for a space; its empty bounding box marks it
    /// as a glyph with no ink rather than one CoreText could not supply.
    #[test]
    fn a_pingfang_space_is_neither_drawn_nor_skipped() {
        let Some(data) = pingfang() else { return };
        let glyphs = coretext_glyphs(FACE, "中 文");
        let mut compiler = compiler();
        let frame = compile(&mut compiler, vec![run(face(&data, FACE), &glyphs)]);
        assert_eq!(frame.skipped_glyphs, 0);
        assert_eq!(frame.glyph_draws, 2);
    }

    #[test]
    fn a_dot_named_pingfang_face_is_skipped_rather_than_drawn_in_a_substitute() {
        let Some(data) = pingfang() else { return };
        let glyphs = coretext_glyphs(FACE, TEXT);
        let mut compiler = compiler();
        let frame = compile(&mut compiler, vec![run(hidden_face(&data), &glyphs)]);
        assert_eq!(frame.skipped_glyphs, 3);
        assert_eq!(frame.glyph_draws, 0);
        assert!(frame.draws().is_empty());
        assert_eq!(compiler.hvgl_cached_outlines(), 0);
    }

    #[test]
    fn the_hvgl_outline_cache_evicts_at_its_cap() {
        let Some(data) = pingfang() else { return };
        let mut compiler = compiler();
        let capacity = compiler.hvgl_outline_capacity();
        let font = face(&data, FACE);
        // Glyph ids 1.. are all inside PingFang (tens of thousands of glyphs),
        // drawn 32 distinct ones a frame until the cap is passed.
        let ids: Vec<u32> = (1..=u32::try_from(capacity + 64).expect("small")).collect();
        for (chunk_index, chunk) in ids.chunks(32).enumerate() {
            compile(&mut compiler, vec![run(font.clone(), chunk)]);
            let drawn = (chunk_index + 1) * 32;
            assert_eq!(
                compiler.hvgl_cached_outlines(),
                drawn.min(capacity),
                "after {drawn} distinct glyphs"
            );
        }
        assert_eq!(compiler.hvgl_cached_outlines(), capacity);
        // Drawing the newest glyphs again fetches nothing new and keeps the
        // population at the cap.
        compile(&mut compiler, vec![run(font, &ids[ids.len() - 32..])]);
        assert_eq!(compiler.hvgl_cached_outlines(), capacity);
    }

    static RENDER_LOCK: Mutex<()> = Mutex::new(());

    fn render_lock() -> MutexGuard<'static, ()> {
        RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Waker};

        let mut cx = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn gpu() -> (wgpu::Device, wgpu::Queue, TierCaps) {
        block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
                .await
                .expect("no compatible GPU adapter");
            let caps = TierCaps::probe(&adapter);
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-engine hvgl device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                    ..Default::default()
                })
                .await
                .expect("failed to create the device");
            (device, queue, caps)
        })
    }

    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test hvgl_provider -- --ignored`"]
    fn pingfang_han_glyphs_render_ink_through_the_gpu() {
        let Some(data) = pingfang() else { return };
        let _guard = render_lock();
        let (device, queue, caps) = gpu();
        const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
        let glyphs = coretext_glyphs(FACE, TEXT);
        let scene = scene_of(vec![run(face(&data, FACE), &glyphs)]);
        let mut renderer = EngineRenderer::with_image_sampler(&device, &caps, FORMAT, None, true)
            .expect("the engine builds on this device");
        renderer.finish_warm_up(&device);
        let (width, height) = (u32::from(VIEWPORT.0), u32::from(VIEWPORT.1));
        let target = HeadlessTarget::new(&device, width, height, FORMAT);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine hvgl frame"),
        });
        renderer
            .encode(
                &device,
                &queue,
                &mut encoder,
                &scene,
                EngineTarget {
                    view: target.view(),
                    format: FORMAT,
                    width,
                    height,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                Color::TRANSPARENT,
                Affine::IDENTITY,
            )
            .expect("a well-formed frame encodes");
        queue.submit([encoder.finish()]);
        renderer.end_frame(&queue);
        let pixels = target.read_back(&device, &queue);
        let inked = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] != 0)
            .count();
        // Three 24 px Han glyphs cover well over a hundred pixels between them.
        assert!(inked > 100, "only {inked} pixels carry ink");
    }
}
