//! The glifo pin-move tripwire: PingFang drawn through the hvgl atlas route
//! against the same run drawn as outlines.
//!
//! `src/text/hvgl_atlas.rs` restates a few of `glifo`'s private raster
//! helpers, which is safe only under the exact `=0.3.0` pin. This test is the
//! check that pin row in `docs/RENDER_DEVELOPMENT.md` names: when `glifo`
//! moves, run it, and a restated helper that drifted shows up as a diff the
//! outline route does not have. It renders a Han paragraph in PingFang SC
//! through Metal with `set_hvgl_atlas(true)` and `(false)`, reads both frames
//! back, and compares them:
//!
//! - **Whole-pixel and subpixel-bucket origins** (`x` = 0, 0.25, 0.5, 0.75):
//!   the ink counts match and no pixel moves by more than a few 1/255 steps
//!   (the atlas rasterises into its own bucketed bitmap, the outline route
//!   fills strips, so antialiasing differs in the last bit).
//! - **Shaped fractional advances** (PingFang's advance is not a whole
//!   pixel): subpixel bucketing makes the atlas differ from the outline by
//!   more than the integer case, so the summed difference is compared with a
//!   Hiragino Sans control (CFF, which `glifo` itself rasterises) rendered
//!   both ways at the same positions, and must stay inside its band.
//!
//! The measured numbers ride in every assertion message. Ignored by default
//! (needs Metal and macOS's PingFang and Hiragino Sans); run with
//! `cargo test -p frust-engine --test hvgl_atlas_parity -- --ignored --test-threads=1`.

#[cfg(target_os = "macos")]
mod macos {
    use frust_engine::cache::images::ImageResidency;
    use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha, SceneCompiler};
    use frust_gpu::{HeadlessTarget, TierCaps};
    use frust_scene::{GlyphRun, Scene, SceneBuilder};
    use frust_text::{FontFamily, TextContext, TextStyle};
    use kurbo::{Affine, Point};
    use peniko::Color;

    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 64;
    const FONT_SIZE: f32 = 24.0;
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// A Han paragraph both families cover.
    const TEXT: &str = "中文爱国人民大学生活";

    /// Exact-bucket case: largest per-channel step allowed between the routes,
    /// in 1/255 (measured 0 on the M4; ha-01's scratch harness saw 1).
    const TIGHT_MAX_STEP: u8 = 1;
    /// Exact-bucket case: summed per-channel difference allowed (measured 0;
    /// ha-01 saw about 50 pixels at a step of 1, under 250 here).
    const TIGHT_MAX_SUM: u64 = 1000;
    /// Exact-bucket case: coverage (summed alpha) agreement as a fraction
    /// (0.001%; measured exact).
    const TIGHT_COVERAGE_GAP: f64 = 0.001 / 100.0;
    /// Shaped-advance case: how far above the Hiragino control's summed
    /// difference PingFang's may sit (measured 0.85x to 0.86x).
    const BAND_FACTOR: f64 = 1.25;
    /// Shaped-advance case: how far above the control's largest step PingFang's
    /// may sit, in 1/255 (measured equal).
    const BAND_MAX_STEP_SLACK: u8 = 16;
    /// Shaped-advance case: coverage gap allowed, as a fraction, whatever the
    /// control's (measured 0.05% PingFang, 0.04% control at worst).
    const BAND_COVERAGE_GAP: f64 = 0.5 / 100.0;

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
                    label: Some("frust-engine hvgl atlas parity device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                    ..Default::default()
                })
                .await
                .expect("failed to create the device");
            (device, queue, caps)
        })
    }

    /// `TEXT` laid out in `family` through the system font oracle. `None`
    /// when the family does not resolve to a face with glyphs for it.
    fn laid_out(family: &str) -> Vec<GlyphRun> {
        let mut ctx = TextContext::new();
        let mut style = TextStyle::new(FONT_SIZE, Color::BLACK);
        style.family = FontFamily::named(family);
        ctx.layout(TEXT, &style, None)
            .to_scene_runs(Point::new(0.0, 0.0))
    }

    /// `runs` with every glyph moved to `index * FONT_SIZE + frac`: the
    /// positions a subpixel bucket holds exactly.
    fn on_grid(runs: &[GlyphRun], frac: f32) -> Vec<GlyphRun> {
        let mut index = 0.0;
        runs.iter()
            .cloned()
            .map(|mut run| {
                for glyph in &mut run.glyphs {
                    glyph.x = index * FONT_SIZE + frac;
                    glyph.y = 0.0;
                    index += 1.0;
                }
                run
            })
            .collect()
    }

    /// `runs` with its glyphs moved to the `x` of `donor`'s, glyph for glyph:
    /// the control face at the positions PingFang was shaped to.
    fn at_positions_of(runs: &[GlyphRun], donor: &[GlyphRun]) -> Vec<GlyphRun> {
        let xs: Vec<f32> = donor
            .iter()
            .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.x))
            .collect();
        let mut index = 0;
        runs.iter()
            .cloned()
            .map(|mut run| {
                for glyph in &mut run.glyphs {
                    glyph.x = xs[index];
                    glyph.y = 0.0;
                    index += 1;
                }
                run
            })
            .collect()
    }

    /// `runs` drawn on a fresh renderer with the hvgl atlas `atlas` and read
    /// back as RGBA bytes.
    fn render(runs: &[GlyphRun], origin_x: f64, atlas: bool) -> Vec<u8> {
        let (device, queue, caps) = gpu();
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            for run in runs {
                let mut run = run.clone();
                run.transform = Affine::translate((origin_x, 40.0)) * run.transform;
                builder.draw_glyph_run(run);
            }
        }
        let mut renderer = EngineRenderer::with_image_sampler(&device, &caps, FORMAT, None, true)
            .expect("the engine builds on this device");
        renderer.set_hvgl_atlas(atlas);
        if !atlas {
            // The control arm: a `glifo`-native face leaves the atlas for outline
            // strips only when residency is off; PingFang already has with the
            // switch above.
            renderer.set_image_residency(ImageResidency::disabled(renderer.atlas_budget()));
        }
        renderer.finish_warm_up(&device);
        let target = HeadlessTarget::new(&device, WIDTH, HEIGHT, FORMAT);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine hvgl atlas parity frame"),
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
                    width: WIDTH,
                    height: HEIGHT,
                    depth: None,
                    output: OutputAlpha::Premultiplied,
                },
                Color::TRANSPARENT,
                Affine::IDENTITY,
            )
            .expect("a well-formed frame encodes");
        queue.submit([encoder.finish()]);
        renderer.end_frame(&queue);
        target.read_back(&device, &queue)
    }

    /// What differs between two frames.
    #[derive(Debug)]
    struct Diff {
        ink_a: usize,
        ink_b: usize,
        coverage_a: u64,
        coverage_b: u64,
        /// Largest per-channel step, in 1/255.
        max: u8,
        /// Pixels whose largest channel step exceeds 1/255.
        over_one: usize,
        /// Sum of every channel's absolute step.
        sum: u64,
    }

    impl Diff {
        fn coverage_gap(&self) -> f64 {
            let (a, b) = (self.coverage_a as f64, self.coverage_b as f64);
            (a - b).abs() / a.max(b).max(1.0)
        }
    }

    fn diff(a: &[u8], b: &[u8]) -> Diff {
        assert_eq!(a.len(), b.len());
        let mut out = Diff {
            ink_a: 0,
            ink_b: 0,
            coverage_a: 0,
            coverage_b: 0,
            max: 0,
            over_one: 0,
            sum: 0,
        };
        for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0.iter()) {
            out.ink_a += usize::from(pa[3] != 0);
            out.ink_b += usize::from(pb[3] != 0);
            out.coverage_a += u64::from(pa[3]);
            out.coverage_b += u64::from(pb[3]);
            let step = pa
                .iter()
                .zip(pb)
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap_or(0);
            out.max = out.max.max(step);
            out.over_one += usize::from(step > 1);
            out.sum += pa
                .iter()
                .zip(pb)
                .map(|(x, y)| u64::from(x.abs_diff(*y)))
                .sum::<u64>();
        }
        out
    }

    /// `(atlas draws, outline fallbacks)` of `hvgl` glyphs the compiler records
    /// for `runs` with the route `atlas`: proof the two renders took the two
    /// routes they are named for.
    fn routes(runs: &[GlyphRun], atlas: bool) -> (u32, u32) {
        let mut scene = Scene::new();
        {
            let mut builder = SceneBuilder::new(&mut scene);
            for run in runs {
                let mut run = run.clone();
                run.transform = Affine::translate((8.0, 40.0)) * run.transform;
                builder.draw_glyph_run(run);
            }
        }
        let mut compiler = SceneCompiler::new(WIDTH as u16, HEIGHT as u16);
        compiler.set_hvgl_atlas(atlas);
        let frame = compiler
            .compile(&scene, Affine::IDENTITY, (WIDTH as u16, HEIGHT as u16))
            .expect("an in-range scene compiles");
        (frame.hvgl_atlas_draws, frame.hvgl_outline_fallbacks)
    }

    /// The atlas-versus-outline diff of `runs` at `origin_x`.
    fn parity(runs: &[GlyphRun], origin_x: f64) -> Diff {
        let atlas = render(runs, origin_x, true);
        let outline = render(runs, origin_x, false);
        diff(&atlas, &outline)
    }

    fn has_family(family: &str) -> Option<Vec<GlyphRun>> {
        let runs = laid_out(family);
        let drawable = runs.iter().any(|run| !run.glyphs.is_empty());
        if !drawable {
            println!("skipped: {family} does not resolve here");
        }
        drawable.then_some(runs)
    }

    /// Whole-pixel origins and exact subpixel-bucket positions: the restated
    /// helpers reproduce the outline route to within antialiasing noise.
    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test hvgl_atlas_parity -- --ignored --test-threads=1`"]
    fn pingfang_atlas_matches_outline_on_exact_buckets() {
        let Some(shaped) = has_family("PingFang SC") else {
            return;
        };
        let (on_draws, on_fallbacks) = routes(&shaped, true);
        let (off_draws, off_fallbacks) = routes(&shaped, false);
        println!("routes: on {on_draws}/{on_fallbacks}, off {off_draws}/{off_fallbacks}");
        assert!(
            on_draws > 0 && off_draws == 0 && off_fallbacks > 0,
            "the two renders must take the atlas and the outline route: on (atlas draws \
             {on_draws}, outline fallbacks {on_fallbacks}), off ({off_draws}, {off_fallbacks})"
        );
        for frac in [0.0_f32, 0.25, 0.5, 0.75] {
            let runs = on_grid(&shaped, frac);
            let d = parity(&runs, 8.0);
            println!("exact bucket x+{frac}: {d:?}");
            assert!(d.ink_a > 1000, "x+{frac}: PingFang drew no ink: {d:?}");
            assert_eq!(d.ink_a, d.ink_b, "x+{frac}: ink counts differ: {d:?}");
            assert!(
                d.max <= TIGHT_MAX_STEP,
                "x+{frac}: a pixel moved by {} / 255 (limit {TIGHT_MAX_STEP}): {d:?}",
                d.max
            );
            assert!(
                d.sum <= TIGHT_MAX_SUM,
                "x+{frac}: summed difference {} (limit {TIGHT_MAX_SUM}): {d:?}",
                d.sum
            );
            assert!(
                d.coverage_gap() <= TIGHT_COVERAGE_GAP,
                "x+{frac}: coverage differs by {:.5}%: {d:?}",
                d.coverage_gap() * 100.0
            );
        }
    }

    /// Shaped fractional advances: bucketing moves PingFang's atlas away from
    /// its outline route, and no further than glifo's own CFF route moves for
    /// Hiragino Sans at the same positions.
    #[test]
    #[ignore = "requires a GPU (Metal); run with `cargo test -p frust-engine --test hvgl_atlas_parity -- --ignored --test-threads=1`"]
    fn pingfang_atlas_stays_inside_the_hiragino_control_band_at_shaped_advances() {
        let (Some(pingfang), Some(hiragino)) =
            (has_family("PingFang SC"), has_family("Hiragino Sans"))
        else {
            return;
        };
        let hiragino = at_positions_of(&hiragino, &pingfang);
        for origin in [8.0_f64, 8.37] {
            let ping = parity(&pingfang, origin);
            let control = parity(&hiragino, origin);
            println!("shaped advances at {origin}: pingfang {ping:?} control {control:?}");
            let limit = (control.sum as f64 * BAND_FACTOR) as u64;
            assert!(
                ping.sum <= limit,
                "origin {origin}: PingFang atlas-vs-outline diff {} exceeds {BAND_FACTOR}x the Hiragino control's {} (limit {limit}): pingfang {ping:?} control {control:?}",
                ping.sum,
                control.sum
            );
            assert!(
                ping.max <= control.max.saturating_add(BAND_MAX_STEP_SLACK),
                "origin {origin}: PingFang max step {} vs control {} (+{BAND_MAX_STEP_SLACK}): pingfang {ping:?} control {control:?}",
                ping.max,
                control.max
            );
            assert!(
                ping.coverage_gap() <= BAND_COVERAGE_GAP,
                "origin {origin}: coverage differs by {:.5}%: pingfang {ping:?} control {control:?}",
                ping.coverage_gap() * 100.0
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
#[ignore = "needs macOS Metal, PingFang and Hiragino Sans"]
fn pingfang_atlas_parity_needs_macos() {
    println!("skipped: needs macOS");
}
