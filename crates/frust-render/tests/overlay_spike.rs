//! The overlay spike (render-model Phase 4, decision D10): a frame cut at a
//! command index into the part the main surface draws and the part an overlay
//! above a native view draws.
//!
//! The device-free cases pin the range projection and its bracket rule
//! (`frust_engine::renderer::project_command_range`, documented there): a clip
//! or layer a cut falls inside is closed at the cut and reopened across it, a
//! retained bracket a cut falls inside is painted inline on both sides, and the
//! two halves together carry every drawing command of the frame exactly once.
//! They run everywhere.
//!
//! The `#[ignore]`d cases need a GPU: an [`OverlayRenderer`] drawing a range
//! into an offscreen target must leave everything outside the range's paint
//! transparent (the overlay's clear), and the main half composited under the
//! overlay half must reproduce the uncut frame.
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-render --test overlay_spike -- --ignored --test-threads=1
//! ```

#![cfg(not(target_arch = "wasm32"))]

use frust_engine::renderer::{RangeCut, RangeOptions, project_command_range};
use frust_engine::{EngineTarget, OutputAlpha};
use frust_gpu::{HeadlessTarget, TierCaps};
use frust_render::overlay::{OverlayRenderer, spike_cut};
use frust_render::{
    GOLDEN_EXPECT_ADAPTER_ENV_VAR, HeadlessOptions, HeadlessRenderer, HeadlessSpec,
};
use frust_scene::{Command, Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::Brush;
use peniko::color::palette::css::{BLUE, RED, WHITE};

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn keep() -> RangeOptions {
    RangeOptions {
        keep_draw_retained: true,
    }
}

fn project(
    scene: &Scene,
    range: std::ops::Range<usize>,
    options: RangeOptions,
) -> (Scene, RangeCut) {
    let mut out = Scene::new();
    let cut = project_command_range(scene, range, options, &mut out);
    (out, cut)
}

/// The commands of `scene` as comparable text (`Command` has no `PartialEq`).
fn listing(scene: &Scene) -> Vec<String> {
    scene.commands().iter().map(|c| format!("{c:?}")).collect()
}

/// The command kinds of `scene`, in order — the bracket structure at a glance.
fn kinds(scene: &Scene) -> Vec<&'static str> {
    scene
        .commands()
        .iter()
        .map(|command| match command {
            Command::FillRect { .. } => "fill",
            Command::PushClip { .. } => "push_clip",
            Command::PopClip => "pop_clip",
            Command::PushLayer { .. } => "push_layer",
            Command::PopLayer => "pop_layer",
            Command::PushRetained { .. } => "push_retained",
            Command::PopRetained => "pop_retained",
            Command::DrawRetained { .. } => "draw_retained",
            Command::ClearRect { .. } => "clear",
            _ => "other",
        })
        .collect()
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    Rect::new(x0, y0, x1, y1)
}

/// `[fill red (whole frame), push_clip(left half), fill white, clear(marker),
/// fill blue (whole frame), pop_clip, fill blue square]` — a clip the cut falls
/// inside, and drawing on both sides of it.
fn clipped_scene() -> Scene {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 64.0, 64.0), Brush::Solid(RED));
    b.push_clip(rect(0.0, 0.0, 32.0, 64.0));
    b.fill_rect(rect(0.0, 0.0, 64.0, 16.0), Brush::Solid(WHITE));
    b.clear_rect(Rect::ZERO);
    b.fill_rect(rect(0.0, 32.0, 64.0, 48.0), Brush::Solid(BLUE));
    b.pop_clip();
    b.push_transform(Affine::translate((40.0, 40.0)));
    b.fill_rect(rect(0.0, 0.0, 8.0, 8.0), Brush::Solid(BLUE));
    b.pop_transform();
    scene
}

#[test]
fn the_spike_cut_falls_just_past_the_first_clear() {
    assert_eq!(spike_cut(&clipped_scene()), Some(4));
    assert_eq!(spike_cut(&Scene::new()), None);
}

#[test]
fn a_whole_range_reproduces_the_scene_command_for_command() {
    let scene = clipped_scene();
    let (whole, cut) = project(&scene, 0..scene.commands().len(), keep());
    assert_eq!(listing(&whole), listing(&scene));
    assert_eq!(
        cut,
        RangeCut {
            start: 0,
            end: scene.commands().len(),
            ..RangeCut::default()
        }
    );
}

#[test]
fn a_clip_spanning_the_cut_is_closed_at_it_and_reopened_across_it() {
    let scene = clipped_scene();
    let at = spike_cut(&scene).expect("the scene records a marker");

    let (main, main_cut) = project(&scene, 0..at, keep());
    assert_eq!(
        kinds(&main),
        ["fill", "push_clip", "fill", "clear", "pop_clip"],
        "the main half closes the clip it was cut inside"
    );
    assert_eq!((main_cut.reopened, main_cut.closed), (0, 1));

    let (overlay, overlay_cut) = project(&scene, at..scene.commands().len(), keep());
    assert_eq!(
        kinds(&overlay),
        ["push_clip", "fill", "pop_clip", "fill"],
        "the overlay half reopens the clip ahead of its first command"
    );
    assert_eq!((overlay_cut.reopened, overlay_cut.closed), (1, 0));
    // The reopened clip is the original, geometry and transform verbatim.
    assert_eq!(listing(&overlay)[0], listing(&scene)[1]);
}

#[test]
fn the_two_halves_carry_every_drawing_command_exactly_once() {
    let scene = clipped_scene();
    let at = spike_cut(&scene).expect("the scene records a marker");
    let (main, _) = project(&scene, 0..at, keep());
    let (overlay, _) = project(&scene, at..scene.commands().len(), keep());
    let draws = |scene: &Scene| -> Vec<String> {
        scene
            .commands()
            .iter()
            .filter(|c| matches!(c, Command::FillRect { .. } | Command::ClearRect { .. }))
            .map(|c| format!("{c:?}"))
            .collect()
    };
    let mut halves = draws(&main);
    halves.extend(draws(&overlay));
    assert_eq!(halves, draws(&scene));
}

#[test]
fn layers_split_like_clips_and_reopen_in_push_order() {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.push_layer(rect(0.0, 0.0, 10.0, 10.0), 0.5);
    b.push_clip(rect(0.0, 0.0, 5.0, 5.0));
    b.fill_rect(rect(0.0, 0.0, 10.0, 10.0), Brush::Solid(RED));
    b.fill_rect(rect(0.0, 0.0, 10.0, 10.0), Brush::Solid(BLUE));
    b.pop_clip();
    b.pop_layer();

    let (overlay, cut) = project(&scene, 3..6, keep());
    assert_eq!(
        kinds(&overlay),
        ["push_layer", "push_clip", "fill", "pop_clip", "pop_layer"]
    );
    assert_eq!((cut.reopened, cut.closed), (2, 0));

    let (main, cut) = project(&scene, 0..3, keep());
    assert_eq!(
        kinds(&main),
        ["push_layer", "push_clip", "fill", "pop_clip", "pop_layer"],
        "closed innermost first"
    );
    assert_eq!((cut.reopened, cut.closed), (0, 2));
}

#[test]
fn a_retained_bracket_spanning_the_cut_is_painted_inline_on_both_sides() {
    let body = rect(0.0, 0.0, 20.0, 20.0);
    let transform = Affine::translate((10.0, 10.0));
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.push_transform(transform);
    b.push_retained(7, body, 0.5, 2.0);
    b.fill_rect(rect(0.0, 0.0, 4.0, 4.0), Brush::Solid(RED));
    b.push_retained(8, body, 0.25, 3.0); // nested: presentation ignored
    b.fill_rect(rect(4.0, 4.0, 8.0, 8.0), Brush::Solid(BLUE));
    b.pop_retained();
    b.pop_retained();
    b.pop_transform();
    assert_eq!(scene.commands().len(), 6);

    let correction = transform * Affine::scale_about(2.0, body.center()) * transform.inverse();

    for (range, side) in [(0..3, "main"), (3..6, "overlay")] {
        let (half, cut) = project(&scene, range, keep());
        assert_eq!(
            cut.demoted_retained, 1,
            "{side}: the spanning bracket is demoted"
        );
        assert!(
            !kinds(&half).contains(&"push_retained") && !kinds(&half).contains(&"pop_retained"),
            "{side}: no page is recorded from half a body: {:?}",
            kinds(&half)
        );
        // Its alpha opens a plain layer under the corrected transform, and
        // every body command carries the presentation scale.
        match &half.commands()[0] {
            Command::PushLayer {
                rect,
                alpha,
                transform: layer,
            } => {
                assert_eq!((*rect, *alpha), (body, 0.5));
                assert_eq!(*layer, correction * transform, "{side}");
            }
            other => panic!("{side}: expected the demoted bracket's layer, got {other:?}"),
        }
        for command in half.commands() {
            if let Command::FillRect { transform: t, .. } = command {
                assert_eq!(
                    *t,
                    correction * transform,
                    "{side}: body under the correction"
                );
            }
        }
        assert_eq!(
            *kinds(&half).last().expect("non-empty"),
            "pop_layer",
            "{side}"
        );
    }
}

#[test]
fn a_retained_bracket_wholly_inside_the_range_is_carried_as_recorded() {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 1.0, 1.0), Brush::Solid(RED));
    b.push_retained(1, rect(0.0, 0.0, 4.0, 4.0), 1.0, 1.0);
    b.fill_rect(rect(0.0, 0.0, 4.0, 4.0), Brush::Solid(BLUE));
    b.pop_retained();

    let (half, cut) = project(&scene, 1..4, keep());
    assert_eq!(kinds(&half), ["push_retained", "fill", "pop_retained"]);
    assert_eq!(cut.demoted_retained, 0);
    assert_eq!(half.retained_depth(), 0, "the projection stays balanced");
}

#[test]
fn draw_retained_is_dropped_only_when_asked() {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.draw_retained(3, rect(0.0, 0.0, 4.0, 4.0), 1.0, 1.0);
    b.fill_rect(rect(0.0, 0.0, 1.0, 1.0), Brush::Solid(RED));

    let (kept, cut) = project(&scene, 0..2, keep());
    assert_eq!(kinds(&kept), ["draw_retained", "fill"]);
    assert_eq!(cut.dropped_draw_retained, 0);

    let (dropped, cut) = project(
        &scene,
        0..2,
        RangeOptions {
            keep_draw_retained: false,
        },
    );
    assert_eq!(kinds(&dropped), ["fill"]);
    assert_eq!(cut.dropped_draw_retained, 1);
}

#[test]
fn an_unmatched_pop_is_dropped_and_a_range_is_clamped_to_the_scene() {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.pop_clip();
    b.fill_rect(rect(0.0, 0.0, 1.0, 1.0), Brush::Solid(RED));
    b.pop_layer();

    let (half, cut) = project(&scene, 0..99, keep());
    assert_eq!(kinds(&half), ["fill"]);
    assert_eq!(cut.dropped_pops, 2);
    assert_eq!((cut.start, cut.end), (0, 3));

    // A start past the end (and both past the scene) projects nothing.
    let reversed = std::ops::Range { start: 7, end: 5 };
    let (empty, cut) = project(&scene, reversed, keep());
    assert!(empty.commands().is_empty());
    assert_eq!((cut.start, cut.end), (3, 3));
}

// --------------------------------------------------------------------- GPU --

/// Blocks on `future` by polling it — wgpu's native requests resolve without
/// an executor.
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

/// Pops a validation error scope, pumping the device until it resolves.
fn drain_error_scope(device: &wgpu::Device, scope: wgpu::ErrorScopeGuard) -> Option<wgpu::Error> {
    use std::task::{Context, Poll, Waker};

    let mut cx = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(scope.pop());
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(error) => return error,
            Poll::Pending => {
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
            }
        }
    }
}

fn device() -> (wgpu::Device, wgpu::Queue, TierCaps) {
    block_on(async {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
            .await
            .expect("no compatible GPU adapter");
        let info = adapter.get_info();
        println!("frust-render overlay_spike adapter: {info:?}");
        if let Ok(expected) = std::env::var(GOLDEN_EXPECT_ADAPTER_ENV_VAR) {
            let expected = expected.trim();
            assert!(
                expected.is_empty() || info.name.to_lowercase().contains(&expected.to_lowercase()),
                "{GOLDEN_EXPECT_ADAPTER_ENV_VAR}={expected:?} but the run resolved {:?}",
                info.name
            );
        }
        let caps = TierCaps::probe(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("frust-render overlay spike device"),
                required_features: wgpu::Features::empty(),
                required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

fn pixel(frame: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE + x) * 4) as usize;
    frame[at..at + 4].try_into().expect("four bytes")
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test overlay_spike -- --ignored`"]
fn an_overlay_range_draws_over_a_transparent_clear() {
    let (device, queue, caps) = device();
    let target = HeadlessTarget::new(&device, SIZE, SIZE, FORMAT);
    let mut overlay =
        OverlayRenderer::new(&device, &queue, &caps, FORMAT, None).expect("the overlay builds");
    let scene = clipped_scene();
    let at = spike_cut(&scene).expect("the scene records a marker");

    // The target starts out opaque white, so a missing clear would show.
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("overlay spike pre-fill"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target.view(),
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    queue.submit([encoder.finish()]);

    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let cut = overlay
        .record(
            &device,
            &queue,
            &mut encoder,
            &scene,
            at..scene.commands().len(),
            EngineTarget {
                view: target.view(),
                format: FORMAT,
                width: SIZE,
                height: SIZE,
                depth: None,
                output: OutputAlpha::Premultiplied,
            },
        )
        .expect("the overlay range encodes");
    queue.submit([encoder.finish()]);
    overlay.end_frame(&queue, true);
    let error = drain_error_scope(&device, scope);
    assert!(error.is_none(), "the overlay frame raised {error:?}");
    assert_eq!(cut.reopened, 1);

    let frame = target.read_back(&device, &queue);
    let transparent = [0, 0, 0, 0];
    let blue = [0, 0, 255, 255];
    // Under the reopened clip: the band painted after the cut.
    assert_eq!(pixel(&frame, 8, 40), blue, "the range's own paint");
    // The same band past the clip's edge: the reopened clip still clips.
    assert_eq!(
        pixel(&frame, 40, 36),
        transparent,
        "clipped by the reopened clip"
    );
    // The square after the clip closed.
    assert_eq!(pixel(&frame, 44, 44), blue);
    // Everything the main half paints is absent: the clear is transparent.
    assert_eq!(
        pixel(&frame, 2, 2),
        transparent,
        "the main half's white band"
    );
    assert_eq!(
        pixel(&frame, 60, 4),
        transparent,
        "the main half's red fill"
    );
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test overlay_spike -- --ignored`"]
fn the_main_half_under_the_overlay_half_reproduces_the_uncut_frame() {
    let mut headless = block_on(HeadlessRenderer::new(HeadlessOptions::default()))
        .expect("failed to create the headless renderer");
    println!("frust-render overlay_spike headless: {}", headless.meta());
    let scene = clipped_scene();
    let at = spike_cut(&scene).expect("the scene records a marker");
    let (main, _) = project(&scene, 0..at, keep());
    let (overlay, _) = project(&scene, at..scene.commands().len(), keep());

    let spec = HeadlessSpec::new(SIZE, SIZE);
    let whole = block_on(headless.render(&scene, &spec)).expect("the whole frame renders");
    let under = block_on(headless.render(&main, &spec)).expect("the main half renders");
    let over = block_on(headless.render(
        &overlay,
        &HeadlessSpec {
            base_color: peniko::Color::TRANSPARENT,
            ..spec
        },
    ))
    .expect("the overlay half renders");

    // Premultiplied source-over, the way a compositor stacks the overlay's
    // swapchain on the main one.
    let mut worst = 0_u8;
    for at in (0..whole.rgba8.len()).step_by(4) {
        let alpha = u16::from(over.rgba8[at + 3]);
        for channel in 0..4 {
            let o = u16::from(over.rgba8[at + channel]);
            let m = u16::from(under.rgba8[at + channel]);
            let composed = (o + (m * (255 - alpha) + 127) / 255).min(255) as u8;
            worst = worst.max(composed.abs_diff(whole.rgba8[at + channel]));
        }
    }
    assert!(
        worst <= 1,
        "the composed halves differ from the uncut frame by {worst}"
    );
}
