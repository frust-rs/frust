//! The segmented frame (render-model Phase 4, decision D10): one scene cut
//! into the main surface's segment — every command outside the overlays'
//! ranges — and one range per overlay, all recorded into one encoder and
//! submitted once.
//!
//! The device-free cases pin the segmentation itself
//! ([`main_segment_ranges`], [`project_main_segment`]): with 0, 1 and 2 cuts
//! every drawing command of the frame lands in exactly one segment, every
//! segment is bracket-balanced, and a cut starting at a slot's own punch keeps
//! the punch in the overlay's segment (the differ's ranges start AT the
//! slot's index). They also pin the surface-free answers of
//! `SurfaceRenderer::encode_segmented`/`submit_segmented`, and the cut
//! servability rule ([`cut_is_servable`]): a cut holding retained content
//! (a page composite or recording, or a retained bracket crossing its
//! boundary), an external texture or a backdrop blur is kept on the main
//! surface — the main segment draws it, the overlay's range is empty — while
//! a plain cut is still served. They run everywhere.
//!
//! The `#[ignore]`d cases need a GPU: the main segment and each overlay's
//! served range recorded into ONE encoder against offscreen targets — the
//! shape `SurfaceRenderer::submit_segmented` records against swapchains —
//! land each command on its own target pixel-correctly, a clip spanning a cut
//! still clips on both sides, and the targets composited bottom to top
//! reproduce the uncut frame within 1/255. A cut kept on main draws its
//! retained page (recorded, then composited on a later frame while the ledger
//! keeps naming it), its texture-bus frame and its backdrop blur on the main
//! target and nothing on its overlay, and no served cut's overlay drops a
//! retained composite.
//!
//! ```text
//! WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400 FRUST_GOLDEN_EXPECT_ADAPTER=T400 \
//!   cargo test -p frust-render --test segmented -- --ignored --test-threads=1
//! ```

#![cfg(not(target_arch = "wasm32"))]
#![expect(
    clippy::single_range_in_vec_init,
    reason = "a list of one command range is a frame with one overlay cut, not a range to collect"
)]

use std::ops::Range;
use std::sync::Arc;

use frust_engine::renderer::{RangeCut, RangeOptions, project_command_range};
use frust_engine::{EngineRenderer, EngineTarget, OutputAlpha};
use frust_gpu::{HeadlessTarget, TierCaps};
use frust_render::overlay::{
    CutServiceability, OverlayRenderer, UnservableReason, cut_is_servable, main_segment_ranges,
    project_main_segment,
};
use frust_render::{
    EncodeOutcome, FrameOutcome, GOLDEN_EXPECT_ADAPTER_ENV_VAR, RenderContext, SurfaceRenderer,
};
use frust_scene::{Command, FontHandle, Glyph, GlyphRun, Scene, SceneBuilder};
use kurbo::{Affine, Rect};
use peniko::color::palette::css::{BLUE, GREEN, RED, WHITE};
use peniko::{Blob, Brush, FontData};

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Noto Sans, subsetted — the bundled face the engine's own text tests and
/// the golden corpus draw with.
const LATIN_FONT: &[u8] = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");

/// `H`, `e`, `l`, `o` in [`LATIN_FONT`]'s character map.
const HELLO_GLYPHS: [u32; 5] = [5, 6, 7, 7, 8];

fn keep() -> RangeOptions {
    RangeOptions {
        keep_draw_retained: true,
    }
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    Rect::new(x0, y0, x1, y1)
}

/// `Hello` in blue, 24 px, on a baseline at y = 44 starting at x = 4: its ink
/// runs from about x = 4 past the right edge of the 64-px frame, so a clip at
/// x = 40/48 cuts it.
fn hello() -> GlyphRun {
    GlyphRun {
        font: FontHandle::new(FontData::new(Blob::new(Arc::new(LATIN_FONT)), 0)),
        font_size: 24.0,
        brush: Brush::Solid(BLUE),
        transform: Affine::translate((4.0, 44.0)),
        glyphs: HELLO_GLYPHS
            .iter()
            .enumerate()
            .map(|(index, id)| Glyph {
                id: *id,
                x: index as f32 * 14.0,
                y: 0.0,
            })
            .collect(),
    }
}

/// One slot: `[fill red band, push_clip(x < 40), clear(slot marker), text,
/// pop_clip]` — a rect under the slot, the text above it, and a clip the cut
/// falls inside. The overlay's cut is `2..5`.
fn one_slot_scene() -> (Scene, Vec<Range<usize>>) {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 64.0, 16.0), Brush::Solid(RED));
    b.push_clip(rect(0.0, 0.0, 40.0, 64.0));
    b.clear_rect(Rect::ZERO);
    b.draw_glyph_run(hello());
    b.pop_clip();
    (scene, vec![2..5])
}

/// Three slots, two of them with overlays: `[0 fill red band, 1 push_clip(x <
/// 48), 2 clear(slot A), 3 text, 4 clear(slot B), 5 fill green chip, 6
/// clear(slot C), 7 fill blue band, 8 pop_clip, 9 fill blue square]`.
///
/// The cuts are A's `2..4` and C's `6..10`; slot B's `4..6` is a gap the main
/// surface keeps drawing (its punch and the green chip, which overlaps
/// nothing an overlay draws). The clip spans both cuts.
fn two_slot_scene() -> (Scene, Vec<Range<usize>>) {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 64.0, 16.0), Brush::Solid(RED));
    b.push_clip(rect(0.0, 0.0, 48.0, 64.0));
    b.clear_rect(Rect::ZERO);
    b.draw_glyph_run(hello());
    b.clear_rect(Rect::ZERO);
    b.fill_rect(rect(40.0, 0.0, 48.0, 8.0), Brush::Solid(GREEN));
    b.clear_rect(Rect::ZERO);
    b.fill_rect(rect(0.0, 52.0, 64.0, 60.0), Brush::Solid(BLUE));
    b.pop_clip();
    b.fill_rect(rect(52.0, 52.0, 60.0, 60.0), Brush::Solid(BLUE));
    (scene, vec![2..4, 6..10])
}

/// The scene's commands as comparable text (`Command` has no `PartialEq`).
fn listing<'a>(commands: impl IntoIterator<Item = &'a Command>) -> Vec<String> {
    let mut out: Vec<String> = commands.into_iter().map(|c| format!("{c:?}")).collect();
    out.sort();
    out
}

/// Each cut's classification, in order — what `encode_segmented` decides.
fn service(scene: &Scene, cuts: &[Range<usize>]) -> Vec<CutServiceability> {
    cuts.iter()
        .map(|cut| cut_is_servable(scene.commands(), cut.clone()))
        .collect()
}

/// The range each overlay records, and the main segment excludes: a cut
/// itself when servable, empty when kept on main.
fn served(scene: &Scene, cuts: &[Range<usize>]) -> Vec<Range<usize>> {
    cuts.iter()
        .zip(service(scene, cuts))
        .map(|(cut, service)| service.served_range(cut))
        .collect()
}

/// Every segment of `scene` under `cuts`, cut the way `encode_segmented` and
/// `submit_segmented` cut them: the main segment first (the complement of the
/// served ranges), then each overlay's served range — the main segment
/// projected for a retaining main renderer, each overlay's range for a
/// retention-off one, with what that projection cut.
fn segments_with_cuts(scene: &Scene, cuts: &[Range<usize>]) -> (Vec<Scene>, Vec<RangeCut>) {
    let served = served(scene, cuts);
    let mut main = Scene::new();
    let main_cut = project_main_segment(scene, &served, keep(), &mut main);
    let mut all = vec![main];
    let mut range_cuts = vec![main_cut];
    for range in served {
        let mut overlay = Scene::new();
        range_cuts.push(project_command_range(
            scene,
            range,
            RangeOptions {
                keep_draw_retained: false,
            },
            &mut overlay,
        ));
        all.push(overlay);
    }
    (all, range_cuts)
}

/// [`segments_with_cuts`]'s segments alone.
fn segments(scene: &Scene, cuts: &[Range<usize>]) -> Vec<Scene> {
    segments_with_cuts(scene, cuts).0
}

/// The retained page's key and body rectangle in the retained scenes.
const KEY: u64 = 0x5e9;
const BODY: Rect = Rect::new(8.0, 24.0, 56.0, 40.0);

/// A clean frame's widget recorded above a slot: `[0 fill red band, 1
/// clear(slot), 2 push_retained(KEY), 3 fill green body, 4 pop_retained]`,
/// its cut `1..5`.
fn retained_record_scene() -> (Scene, Vec<Range<usize>>) {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 64.0, 16.0), Brush::Solid(RED));
    b.clear_rect(Rect::ZERO);
    b.push_retained(KEY, BODY, 1.0, 1.0);
    b.fill_rect(BODY, Brush::Solid(GREEN));
    b.pop_retained();
    (scene, vec![1..5])
}

/// The same widget on a later, clean frame, drawn from its page: `[0 fill red
/// band, 1 clear(slot), 2 draw_retained(KEY)]`, its cut `1..3`.
fn retained_draw_scene() -> (Scene, Vec<Range<usize>>) {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 64.0, 16.0), Brush::Solid(RED));
    b.clear_rect(Rect::ZERO);
    b.draw_retained(KEY, BODY, 1.0, 1.0);
    (scene, vec![1..3])
}

/// `[0 fill red band, 1 clear(slot), 2 backdrop_blur, 3 fill blue square]`,
/// its cut `1..4`: a frosted panel above the slot.
fn backdrop_scene() -> (Scene, Vec<Range<usize>>) {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 64.0, 16.0), Brush::Solid(RED));
    b.clear_rect(Rect::ZERO);
    b.backdrop_blur(rect(0.0, 0.0, 64.0, 32.0), 4.0);
    b.fill_rect(rect(52.0, 52.0, 60.0, 60.0), Brush::Solid(BLUE));
    (scene, vec![1..4])
}

/// `[0 fill red band, 1 clear(slot A), 2 text, 3 clear(slot B), 4
/// draw_retained(KEY), 5 fill blue square]`, cut at A's `1..3` (plain) and
/// B's `3..6` (retained content).
fn mixed_scene() -> (Scene, Vec<Range<usize>>) {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 64.0, 16.0), Brush::Solid(RED));
    b.clear_rect(Rect::ZERO);
    b.draw_glyph_run(hello());
    b.clear_rect(Rect::ZERO);
    b.draw_retained(KEY, BODY, 1.0, 1.0);
    b.fill_rect(rect(52.0, 52.0, 60.0, 60.0), Brush::Solid(BLUE));
    (scene, vec![1..3, 3..6])
}

/// Asserts `scene`'s brackets are balanced: no pop without its push, nothing
/// left open.
fn assert_balanced(scene: &Scene, what: &str) {
    let mut stack = Vec::new();
    for command in scene.commands() {
        match command {
            Command::PushClip { .. }
            | Command::PushClipRounded { .. }
            | Command::PushClipPath { .. } => stack.push("clip"),
            Command::PushLayer { .. } => stack.push("layer"),
            Command::PushRetained { .. } => stack.push("retained"),
            Command::PopClip => assert_eq!(stack.pop(), Some("clip"), "{what}"),
            Command::PopLayer => assert_eq!(stack.pop(), Some("layer"), "{what}"),
            Command::PopRetained => assert_eq!(stack.pop(), Some("retained"), "{what}"),
            _ => {}
        }
    }
    assert!(stack.is_empty(), "{what} leaves {stack:?} open");
}

#[test]
fn the_main_segment_is_the_complement_of_the_cuts() {
    // No cut: the whole frame.
    assert_eq!(main_segment_ranges(10, &[]), [0..10]);
    // One cut tiling the tail: the head alone.
    assert_eq!(main_segment_ranges(10, &[4..10]), [0..4]);
    // One cut short of the end: the head and the tail.
    assert_eq!(main_segment_ranges(10, &[4..6]), [0..4, 6..10]);
    // Two cuts with a gap: head, gap, nothing after.
    assert_eq!(main_segment_ranges(10, &[2..4, 6..10]), [0..2, 4..6]);
    // Two abutting cuts tiling the tail.
    assert_eq!(main_segment_ranges(10, &[2..6, 6..10]), [0..2]);
    // A cut at the very start leaves no empty head.
    assert_eq!(main_segment_ranges(10, &[0..3]), [3..10]);
    // An empty cut removes nothing.
    assert_eq!(main_segment_ranges(10, &[5..5]), [0..10]);
    // Clamped to the scene, and a cut starting inside the previous one starts
    // where it ended: nothing drawn twice, nothing backwards.
    assert_eq!(main_segment_ranges(10, &[2..6, 4..8]), [0..2, 8..10]);
    assert_eq!(main_segment_ranges(10, &[8..99]), [0..8]);
    assert_eq!(main_segment_ranges(0, &[0..4]), Vec::<Range<usize>>::new());
}

#[test]
fn every_drawing_command_lands_in_exactly_one_segment() {
    let (one, one_cuts) = one_slot_scene();
    let (two, two_cuts) = two_slot_scene();
    let (mixed, mixed_cuts) = mixed_scene();
    let (record, record_cuts) = retained_record_scene();
    let (backdrop, backdrop_cuts) = backdrop_scene();
    for (scene, cuts) in [
        (&one, Vec::new()),
        (&one, one_cuts),
        (&two, Vec::new()),
        (&two, vec![two_cuts[0].clone()]),
        (&two, two_cuts),
        (&mixed, mixed_cuts),
        (&record, record_cuts),
        (&backdrop, backdrop_cuts),
    ] {
        let parts = segments(scene, &cuts);
        assert_eq!(parts.len(), cuts.len() + 1);
        let drawn = listing(
            parts
                .iter()
                .flat_map(|part| part.commands().iter())
                .filter(|c| c.is_drawing()),
        );
        let expected = listing(scene.commands().iter().filter(|c| c.is_drawing()));
        assert_eq!(drawn, expected, "cuts {cuts:?}");
        for (index, part) in parts.iter().enumerate() {
            assert_balanced(part, &format!("segment {index} of cuts {cuts:?}"));
        }
    }
}

#[test]
fn a_cut_at_a_slot_keeps_its_punch_on_the_overlay() {
    let (scene, cuts) = two_slot_scene();
    let parts = segments(&scene, &cuts);
    // Each overlay opens with the reopened clip, then the slot's own punch.
    for overlay in &parts[1..] {
        let commands = overlay.commands();
        assert!(matches!(commands[0], Command::PushClip { .. }));
        assert!(matches!(commands[1], Command::ClearRect { .. }));
    }
    // The main segment keeps slot B's punch (in the gap) and no other.
    let punches = parts[0]
        .commands()
        .iter()
        .filter(|c| matches!(c, Command::ClearRect { .. }))
        .count();
    assert_eq!(punches, 1);
    // And the clip spanning both cuts is closed and reopened on the main
    // surface around each of them: two pushes, two pops.
    let clips = parts[0]
        .commands()
        .iter()
        .filter(|c| matches!(c, Command::PushClip { .. }))
        .count();
    assert_eq!(clips, 2);
}

#[test]
fn the_main_segment_with_no_cut_is_the_scene_itself() {
    let (scene, _) = two_slot_scene();
    let mut main = Scene::new();
    project_main_segment(&scene, &[], keep(), &mut main);
    let unsorted =
        |s: &Scene| -> Vec<String> { s.commands().iter().map(|c| format!("{c:?}")).collect() };
    assert_eq!(unsorted(&main), unsorted(&scene));
}

#[test]
fn a_plain_cut_is_served_and_the_main_segment_excludes_it() {
    let (one, one_cuts) = one_slot_scene();
    let (two, two_cuts) = two_slot_scene();
    for (scene, cuts) in [(&one, one_cuts), (&two, two_cuts)] {
        assert!(
            service(scene, &cuts)
                .iter()
                .all(|service| service.is_servable()),
            "fills, text, punches and clips are servable: {cuts:?}"
        );
        assert_eq!(served(scene, &cuts), cuts, "a served cut is drawn whole");
    }
    // An empty cut is servable: there is nothing to draw on either surface.
    let (scene, _) = retained_draw_scene();
    assert_eq!(
        cut_is_servable(scene.commands(), 2..2),
        CutServiceability::Servable
    );
}

#[test]
fn a_cut_holding_a_retained_composite_is_kept_on_main_and_its_overlay_is_empty() {
    let (scene, cuts) = retained_draw_scene();
    assert_eq!(
        service(&scene, &cuts),
        [CutServiceability::Unservable {
            reason: UnservableReason::RetainedComposite,
            at: 2,
        }]
    );
    assert_eq!(served(&scene, &cuts), [0..0]);
    let (parts, range_cuts) = segments_with_cuts(&scene, &cuts);
    // The main segment is the uncut frame, the page's composite included.
    let unsorted =
        |s: &Scene| -> Vec<String> { s.commands().iter().map(|c| format!("{c:?}")).collect() };
    assert_eq!(unsorted(&parts[0]), unsorted(&scene));
    // The overlay's range is empty: nothing reopened, nothing dropped.
    assert!(parts[1].commands().is_empty());
    assert_eq!(range_cuts[1], RangeCut::default());
}

#[test]
fn a_cut_holding_a_retained_recording_is_kept_on_main() {
    let (scene, cuts) = retained_record_scene();
    assert_eq!(
        service(&scene, &cuts),
        [CutServiceability::Unservable {
            reason: UnservableReason::RetainedRecording,
            at: 2,
        }]
    );
    let (parts, range_cuts) = segments_with_cuts(&scene, &cuts);
    // The main surface records the page itself, bracket intact.
    assert!(
        parts[0]
            .commands()
            .iter()
            .any(|c| matches!(c, Command::PushRetained { key: KEY, .. }))
    );
    assert_eq!(range_cuts[0].demoted_retained, 0);
    assert!(parts[1].commands().is_empty());
}

#[test]
fn a_cut_holding_an_external_texture_or_a_backdrop_blur_is_kept_on_main() {
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 64.0, 16.0), Brush::Solid(RED));
    b.clear_rect(Rect::ZERO);
    b.scene_texture(7, rect(0.0, 0.0, 32.0, 32.0));
    assert_eq!(
        cut_is_servable(scene.commands(), 1..3),
        CutServiceability::Unservable {
            reason: UnservableReason::SceneTexture,
            at: 2,
        }
    );
    // The command before the texture is still servable on its own.
    assert!(cut_is_servable(scene.commands(), 1..2).is_servable());

    let (scene, cuts) = backdrop_scene();
    assert_eq!(
        service(&scene, &cuts),
        [CutServiceability::Unservable {
            reason: UnservableReason::BackdropBlur,
            at: 2,
        }]
    );
    let parts = segments(&scene, &cuts);
    assert!(
        parts[0]
            .commands()
            .iter()
            .any(|c| matches!(c, Command::BackdropBlur { .. }))
    );
    assert!(parts[1].commands().is_empty());
}

#[test]
fn a_retained_bracket_crossing_a_cut_boundary_makes_the_cut_unservable() {
    // `[0 fill, 1 clear(A), 2 push_retained, 3 fill, 4 clear(B), 5 fill, 6
    // pop_retained, 7 fill]`.
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 64.0, 16.0), Brush::Solid(RED));
    b.clear_rect(Rect::ZERO);
    b.push_retained(KEY, BODY, 0.5, 1.0);
    b.fill_rect(BODY, Brush::Solid(GREEN));
    b.clear_rect(Rect::ZERO);
    b.fill_rect(BODY, Brush::Solid(BLUE));
    b.pop_retained();
    b.fill_rect(rect(52.0, 52.0, 60.0, 60.0), Brush::Solid(BLUE));
    let commands = scene.commands();
    let crossing = CutServiceability::Unservable {
        reason: UnservableReason::RetainedBracketCrossing,
        at: 2,
    };
    // Opened inside the cut and still open past its end.
    assert_eq!(cut_is_servable(commands, 1..4), crossing);
    // Opened before the cut and still open at its start.
    assert_eq!(cut_is_servable(commands, 4..8), crossing);
    assert_eq!(cut_is_servable(commands, 3..5), crossing);
    // Wholly inside: a recording, still kept on main.
    assert_eq!(
        cut_is_servable(commands, 1..7),
        CutServiceability::Unservable {
            reason: UnservableReason::RetainedRecording,
            at: 2,
        }
    );
    // Closed before the cut: servable.
    assert!(cut_is_servable(commands, 7..8).is_servable());

    // Kept on main, the bracket is no longer split: the main segment records
    // it whole.
    let (parts, range_cuts) = segments_with_cuts(&scene, &[4..8]);
    assert_eq!(range_cuts[0].demoted_retained, 0);
    assert!(parts[1].commands().is_empty());
    // Served, the same bracket would have been demoted on both surfaces.
    let mut main = Scene::new();
    let demoted = project_main_segment(&scene, &[4..8], keep(), &mut main);
    assert_eq!(demoted.demoted_retained, 1);
}

#[test]
fn served_cuts_never_drop_a_retained_composite_and_kept_cuts_stay_on_main() {
    let (scene, cuts) = mixed_scene();
    assert_eq!(
        service(&scene, &cuts),
        [
            CutServiceability::Servable,
            CutServiceability::Unservable {
                reason: UnservableReason::RetainedComposite,
                at: 4,
            },
        ]
    );
    let (parts, range_cuts) = segments_with_cuts(&scene, &cuts);
    for (index, cut) in range_cuts.iter().enumerate().skip(1) {
        assert_eq!(cut.dropped_draw_retained, 0, "overlay {index}");
    }
    // Overlay A draws the text; overlay B nothing; the main segment the band,
    // the composite and the square.
    assert!(matches!(parts[1].commands()[1], Command::GlyphRun(_)));
    assert!(parts[2].commands().is_empty());
    assert!(
        parts[0]
            .commands()
            .iter()
            .any(|c| matches!(c, Command::DrawRetained { key: KEY, .. }))
    );
    assert!(
        parts[0]
            .commands()
            .iter()
            .all(|c| !matches!(c, Command::GlyphRun(_)))
    );
    assert_eq!(
        format!("{}", service(&scene, &cuts)[1]),
        "kept on main: retained content (a page composite) at command 4"
    );
}

#[test]
fn a_surfaceless_renderer_skips_a_segmented_frame_without_submitting() {
    let ctx = RenderContext::new();
    let mut renderer = SurfaceRenderer::new();
    let (scene, cuts) = two_slot_scene();
    assert_eq!(
        renderer
            .encode_segmented(&ctx, &scene, WHITE, &cuts)
            .expect("encode never fails without a surface"),
        EncodeOutcome::Skipped
    );
    let outcome = renderer
        .submit_segmented(&ctx, &mut [])
        .expect("submit never fails without a surface");
    assert_eq!(outcome.frame, FrameOutcome::Skipped);
    assert!(outcome.overlays.is_empty());
    assert_eq!((outcome.encoders, outcome.submits), (0, 0));
    let (outcome, presents) = renderer
        .submit_segmented_deferred(&ctx, &mut [])
        .expect("submit never fails without a surface");
    assert_eq!(outcome.frame, FrameOutcome::Skipped);
    assert!(presents.is_empty());
    assert!(presents.into_vec().is_empty());
}

// --------------------------------------------------------------------- GPU --

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
        println!("frust-render segmented adapter: {info:?}");
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
                label: Some("frust-render segmented device"),
                required_features: wgpu::Features::empty(),
                required_limits: frust_gpu::test_device_limits(&adapter, &caps),
                ..Default::default()
            })
            .await
            .expect("failed to create the device");
        (device, queue, caps)
    })
}

/// The main surface's engine and one overlay renderer per cut, on one
/// device, each drawing into an offscreen target of its own.
struct Rig {
    device: wgpu::Device,
    queue: wgpu::Queue,
    main: EngineRenderer,
    main_target: HeadlessTarget,
    overlays: Vec<(OverlayRenderer, HeadlessTarget)>,
    main_scene: Scene,
    /// Queue submits the rig's frames issued, counted at the one call site.
    submits: u32,
    /// Command encoders the rig's frames created.
    encoders: u32,
}

/// What one segmented frame left on each target.
struct Frame {
    main: Vec<u8>,
    overlays: Vec<Vec<u8>>,
    /// Each cut's classification, in order.
    service: Vec<CutServiceability>,
}

impl Rig {
    fn new(overlays: usize) -> Self {
        let (device, queue, caps) = device();
        let mut main =
            EngineRenderer::new(&device, &caps, FORMAT, None).expect("the main engine builds");
        main.finish_warm_up(&device);
        let overlays = (0..overlays)
            .map(|_| {
                (
                    OverlayRenderer::new(&device, &queue, &caps, FORMAT, None)
                        .expect("the overlay builds"),
                    HeadlessTarget::new(&device, SIZE, SIZE, FORMAT),
                )
            })
            .collect();
        Self {
            main_target: HeadlessTarget::new(&device, SIZE, SIZE, FORMAT),
            device,
            queue,
            main,
            overlays,
            main_scene: Scene::new(),
            submits: 0,
            encoders: 0,
        }
    }

    fn target(view: &wgpu::TextureView) -> EngineTarget<'_> {
        EngineTarget {
            view,
            format: FORMAT,
            width: SIZE,
            height: SIZE,
            depth: None,
            output: OutputAlpha::Premultiplied,
        }
    }

    /// One segmented frame, cut the way `encode_segmented` cuts it (the main
    /// segment excludes only the served ranges) and recorded the way
    /// `submit_segmented` records it: the external passes, the main segment,
    /// then every overlay's served range, into ONE encoder, then ONE submit.
    fn segmented(&mut self, scene: &Scene, cuts: &[Range<usize>]) -> Frame {
        assert_eq!(cuts.len(), self.overlays.len());
        let service = service(scene, cuts);
        let served = served(scene, cuts);
        project_main_segment(
            scene,
            &served,
            RangeOptions {
                keep_draw_retained: self.main.retained_layers(),
            },
            &mut self.main_scene,
        );
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.encoders += 1;
        frust_render::external_pass::run_external_passes(
            &self.device,
            &self.queue,
            &mut encoder,
            &mut self.main,
        );
        self.main
            .encode(
                &self.device,
                &self.queue,
                &mut encoder,
                &self.main_scene,
                Self::target(self.main_target.view()),
                WHITE,
                Affine::IDENTITY,
            )
            .expect("the main segment encodes");
        for ((overlay, target), range) in self.overlays.iter_mut().zip(served) {
            let cut = overlay
                .record(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    scene,
                    range,
                    Self::target(target.view()),
                )
                .expect("the overlay range encodes");
            assert_eq!(
                cut.dropped_draw_retained, 0,
                "no overlay drops a retained composite"
            );
        }
        self.queue.submit([encoder.finish()]);
        self.submits += 1;
        self.main.end_frame(&self.queue);
        for (overlay, _) in &mut self.overlays {
            overlay.end_frame(&self.queue, true);
        }
        let error = drain_error_scope(&self.device, scope);
        assert!(error.is_none(), "the segmented frame raised {error:?}");
        Frame {
            main: self.main_target.read_back(&self.device, &self.queue),
            overlays: self
                .overlays
                .iter()
                .map(|(_, target)| target.read_back(&self.device, &self.queue))
                .collect(),
            service,
        }
    }

    /// The keys the main engine's retained store holds, sorted.
    fn ledger(&self) -> Vec<u64> {
        let mut keys: Vec<u64> = self.main.retained_ledger().map(|entry| entry.key).collect();
        keys.sort_unstable();
        keys
    }

    /// The uncut frame on the main engine, for the composite comparison.
    fn uncut(&mut self, scene: &Scene) -> Vec<u8> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.main
            .encode(
                &self.device,
                &self.queue,
                &mut encoder,
                scene,
                Self::target(self.main_target.view()),
                WHITE,
                Affine::IDENTITY,
            )
            .expect("the uncut frame encodes");
        self.queue.submit([encoder.finish()]);
        self.main.end_frame(&self.queue);
        self.main_target.read_back(&self.device, &self.queue)
    }
}

/// Whether every pixel of `frame` is transparent.
fn all_clear(frame: &[u8]) -> bool {
    frame.chunks(4).all(|px| px == CLEAR_PX)
}

fn pixel(frame: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE + x) * 4) as usize;
    frame[at..at + 4].try_into().expect("four bytes")
}

/// Pixels in the rectangle `x0..x1` x `y0..y1` whose alpha is at least half.
fn inked(frame: &[u8], (x0, x1): (u32, u32), (y0, y1): (u32, u32)) -> usize {
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(frame, x, y)[3] >= 128)
        .count()
}

/// Pixels in the rectangle that are not exactly `color`.
fn differing(frame: &[u8], (x0, x1): (u32, u32), (y0, y1): (u32, u32), color: [u8; 4]) -> usize {
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(frame, x, y) != color)
        .count()
}

/// The targets stacked bottom to top with premultiplied source-over — the way
/// a compositor stacks the overlays' swapchains on the main one — against the
/// uncut frame: the worst channel difference.
fn composite_error(frame: &Frame, uncut: &[u8]) -> u8 {
    let mut worst = 0_u8;
    for at in (0..uncut.len()).step_by(4) {
        let mut composed = [0_u16; 4];
        for (channel, slot) in composed.iter_mut().enumerate() {
            *slot = u16::from(frame.main[at + channel]);
        }
        for overlay in &frame.overlays {
            let alpha = u16::from(overlay[at + 3]);
            for (channel, slot) in composed.iter_mut().enumerate() {
                let o = u16::from(overlay[at + channel]);
                *slot = (o + (*slot * (255 - alpha) + 127) / 255).min(255);
            }
        }
        for (channel, value) in composed.iter().enumerate() {
            worst = worst.max((*value as u8).abs_diff(uncut[at + channel]));
        }
    }
    worst
}

const RED_PX: [u8; 4] = [255, 0, 0, 255];
const BLUE_PX: [u8; 4] = [0, 0, 255, 255];
const GREEN_PX: [u8; 4] = [0, 128, 0, 255];
const WHITE_PX: [u8; 4] = [255, 255, 255, 255];
const CLEAR_PX: [u8; 4] = [0, 0, 0, 0];

/// The text's band: rows its ink can reach (24 px on a baseline at 44).
const TEXT_ROWS: (u32, u32) = (20, 50);

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test segmented -- --ignored`"]
fn one_cut_puts_the_rect_on_the_main_target_and_the_text_on_the_overlay() {
    let mut rig = Rig::new(1);
    let (scene, cuts) = one_slot_scene();
    let frame = rig.segmented(&scene, &cuts);
    assert_eq!(
        (rig.encoders, rig.submits),
        (1, 1),
        "one encoder, one submit"
    );

    // The main target: the red band over the white base, and no text at all.
    assert_eq!(pixel(&frame.main, 8, 8), RED_PX);
    assert_eq!(
        differing(&frame.main, (0, SIZE), TEXT_ROWS, WHITE_PX),
        0,
        "the text never reaches the main target"
    );

    // The overlay: the text inside the reopened clip, nothing past it, and
    // nothing of the main segment.
    let overlay = &frame.overlays[0];
    assert!(
        inked(overlay, (0, 40), TEXT_ROWS) > 20,
        "the text's ink lands on the overlay"
    );
    assert_eq!(
        differing(overlay, (40, SIZE), (0, SIZE), CLEAR_PX),
        0,
        "the reopened clip still clips the text at x = 40"
    );
    assert_eq!(
        differing(overlay, (0, SIZE), (0, 16), CLEAR_PX),
        0,
        "the red band is the main target's"
    );

    let uncut = rig.uncut(&scene);
    let worst = composite_error(&frame, &uncut);
    assert!(
        worst <= 1,
        "the composed segments differ from the uncut frame by {worst}"
    );
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test segmented -- --ignored`"]
fn two_cuts_land_each_range_on_its_own_target() {
    let mut rig = Rig::new(2);
    let (scene, cuts) = two_slot_scene();
    let frame = rig.segmented(&scene, &cuts);
    assert_eq!(
        (rig.encoders, rig.submits),
        (1, 1),
        "one encoder, one submit"
    );

    // The main target: the red band, the gap's green chip, white elsewhere.
    assert_eq!(pixel(&frame.main, 8, 8), RED_PX);
    assert_eq!(pixel(&frame.main, 44, 4), GREEN_PX);
    assert_eq!(differing(&frame.main, (0, SIZE), TEXT_ROWS, WHITE_PX), 0);
    assert_eq!(differing(&frame.main, (0, SIZE), (52, 60), WHITE_PX), 0);

    // Overlay A: the text alone, clipped at x = 48.
    let a = &frame.overlays[0];
    assert!(inked(a, (0, 48), TEXT_ROWS) > 20);
    assert_eq!(differing(a, (48, SIZE), (0, SIZE), CLEAR_PX), 0);
    assert_eq!(differing(a, (0, SIZE), (0, 16), CLEAR_PX), 0);
    assert_eq!(differing(a, (0, SIZE), (50, SIZE), CLEAR_PX), 0);

    // Overlay C: the clipped blue band and the unclipped square, nothing else.
    let c = &frame.overlays[1];
    assert_eq!(pixel(c, 8, 56), BLUE_PX);
    assert_eq!(pixel(c, 50, 56), CLEAR_PX, "the band is clipped at x = 48");
    assert_eq!(pixel(c, 56, 56), BLUE_PX, "the square is past the pop");
    assert_eq!(differing(c, (0, SIZE), (0, 50), CLEAR_PX), 0);

    let uncut = rig.uncut(&scene);
    let worst = composite_error(&frame, &uncut);
    assert!(
        worst <= 1,
        "the composed segments differ from the uncut frame by {worst}"
    );

    // A second frame of the same shape: still one encoder and one submit per
    // frame, and the same pixels.
    let again = rig.segmented(&scene, &cuts);
    assert_eq!((rig.encoders, rig.submits), (2, 2));
    assert_eq!(again.main, frame.main);
    assert_eq!(again.overlays, frame.overlays);
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test segmented -- --ignored`"]
fn a_retained_widget_above_a_slot_composites_on_the_main_target_across_frames() {
    let mut rig = Rig::new(1);
    assert!(rig.main.retained_layers(), "the main engine retains");

    // A dirty frame records the page: kept on main, so the main store records
    // it and the ledger names it.
    let (record, record_cuts) = retained_record_scene();
    let recorded = rig.segmented(&record, &record_cuts);
    assert!(!recorded.service[0].is_servable());
    assert_eq!(rig.ledger(), [KEY], "the main store recorded the page");
    assert_eq!(pixel(&recorded.main, 8, 8), RED_PX);
    assert_eq!(pixel(&recorded.main, 32, 32), GREEN_PX, "the body on main");
    assert!(
        all_clear(&recorded.overlays[0]),
        "the overlay draws nothing"
    );

    // Two clean frames composite it from the page, still on main, and the
    // ledger keeps naming it.
    let (draw, draw_cuts) = retained_draw_scene();
    for frame in 0..2 {
        let drawn = rig.segmented(&draw, &draw_cuts);
        assert_eq!(
            drawn.service[0],
            CutServiceability::Unservable {
                reason: UnservableReason::RetainedComposite,
                at: 2,
            }
        );
        assert_eq!(rig.ledger(), [KEY], "frame {frame}");
        assert_eq!(
            drawn.main, recorded.main,
            "frame {frame}: the composite draws exactly the recorded body"
        );
        assert!(all_clear(&drawn.overlays[0]), "frame {frame}");
    }
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test segmented -- --ignored`"]
fn a_texture_bus_frame_above_a_slot_draws_on_the_main_target() {
    const TEXTURE_PX: [u8; 4] = [0, 255, 0, 255];
    let pixels: Vec<u8> = TEXTURE_PX
        .iter()
        .copied()
        .cycle()
        .take((SIZE * SIZE * 4) as usize)
        .collect();
    let pixels: Arc<[u8]> = Arc::from(pixels);
    let registration = frust_render::texture_bus::register_texture_source(move || {
        Some(frust_render::texture_bus::SourceFrame::rgba8(
            SIZE,
            SIZE,
            Arc::clone(&pixels),
        ))
    });
    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    b.fill_rect(rect(0.0, 0.0, 64.0, 16.0), Brush::Solid(RED));
    b.clear_rect(Rect::ZERO);
    b.scene_texture(
        registration.scene_texture_id(),
        rect(16.0, 24.0, 48.0, 56.0),
    );
    let cuts = vec![1..3];

    let mut rig = Rig::new(1);
    let frame = rig.segmented(&scene, &cuts);
    assert_eq!(
        frame.service[0],
        CutServiceability::Unservable {
            reason: UnservableReason::SceneTexture,
            at: 2,
        }
    );
    assert_eq!(pixel(&frame.main, 32, 40), TEXTURE_PX, "the frame on main");
    assert_eq!(pixel(&frame.main, 8, 8), RED_PX);
    assert_eq!(pixel(&frame.main, 4, 40), WHITE_PX, "nothing past the dest");
    assert!(all_clear(&frame.overlays[0]), "the overlay draws nothing");

    // Unregistered, and the queued unbind drained, as the bus's own tests
    // leave the process-wide registry.
    drop(registration);
    let _ = rig.segmented(&Scene::new(), &[0..0]);
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test segmented -- --ignored`"]
fn a_backdrop_blur_above_a_slot_stays_on_the_main_target() {
    let mut rig = Rig::new(1);
    let (scene, cuts) = backdrop_scene();
    let frame = rig.segmented(&scene, &cuts);
    assert!(!frame.service[0].is_servable());
    assert!(all_clear(&frame.overlays[0]), "the overlay draws nothing");
    let uncut = rig.uncut(&scene);
    assert_eq!(
        frame.main, uncut,
        "the main target draws the uncut frame, blur included"
    );
    // The blur did run: the red band's edge is softened below it.
    assert_ne!(pixel(&uncut, 32, 18), WHITE_PX);
}

/// A busier frame — a grid of rects and rows of text with two cuts — timed
/// on the CPU: the main segment's encode and each overlay's range record (its
/// projection, its own compile and its recording), against the uncut frame's
/// encode. Prints the medians; asserts only that every frame encodes.
#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-render --test segmented -- --ignored --nocapture`"]
fn segmented_compile_cost_per_overlay() {
    use std::time::{Duration, Instant};

    let mut scene = Scene::new();
    let mut b = SceneBuilder::new(&mut scene);
    let grid = |b: &mut SceneBuilder<'_>, colour| {
        for row in 0..16 {
            for column in 0..16 {
                let (x, y) = (f64::from(column) * 4.0, f64::from(row) * 4.0);
                b.fill_rect(rect(x, y, x + 3.0, y + 3.0), Brush::Solid(colour));
            }
        }
    };
    grid(&mut b, RED);
    for _ in 0..4 {
        b.draw_glyph_run(hello());
    }
    let first = b.command_count();
    b.clear_rect(Rect::ZERO);
    grid(&mut b, GREEN);
    for _ in 0..4 {
        b.draw_glyph_run(hello());
    }
    let second = b.command_count();
    b.clear_rect(Rect::ZERO);
    grid(&mut b, BLUE);
    let len = scene.commands().len();
    let cuts = vec![first..second, second..len];

    let mut rig = Rig::new(2);
    const WARM: usize = 10;
    const FRAMES: usize = 40;
    let median = |mut samples: Vec<Duration>| {
        samples.sort();
        samples[samples.len() / 2]
    };
    let (mut main, mut overlays, mut whole) = (Vec::new(), [Vec::new(), Vec::new()], Vec::new());
    for frame in 0..WARM + FRAMES {
        project_main_segment(&scene, &cuts, keep(), &mut rig.main_scene);
        let mut encoder = rig
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let started = Instant::now();
        rig.main
            .encode(
                &rig.device,
                &rig.queue,
                &mut encoder,
                &rig.main_scene,
                Rig::target(rig.main_target.view()),
                WHITE,
                Affine::IDENTITY,
            )
            .expect("the main segment encodes");
        let main_took = started.elapsed();
        let mut overlay_took = [Duration::ZERO; 2];
        for (index, ((overlay, target), cut)) in rig.overlays.iter_mut().zip(&cuts).enumerate() {
            let started = Instant::now();
            overlay
                .record(
                    &rig.device,
                    &rig.queue,
                    &mut encoder,
                    &scene,
                    cut.clone(),
                    Rig::target(target.view()),
                )
                .expect("the overlay range encodes");
            overlay_took[index] = started.elapsed();
        }
        rig.queue.submit([encoder.finish()]);
        rig.main.end_frame(&rig.queue);
        for (overlay, _) in &mut rig.overlays {
            overlay.end_frame(&rig.queue, true);
        }
        let mut encoder = rig
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let started = Instant::now();
        rig.main
            .encode(
                &rig.device,
                &rig.queue,
                &mut encoder,
                &scene,
                Rig::target(rig.main_target.view()),
                WHITE,
                Affine::IDENTITY,
            )
            .expect("the uncut frame encodes");
        let whole_took = started.elapsed();
        rig.queue.submit([encoder.finish()]);
        rig.main.end_frame(&rig.queue);
        let _ = rig.device.poll(wgpu::PollType::wait_indefinitely());
        if frame >= WARM {
            main.push(main_took);
            overlays[0].push(overlay_took[0]);
            overlays[1].push(overlay_took[1]);
            whole.push(whole_took);
        }
    }
    let [a, c] = overlays;
    println!(
        "frust-render segmented cost: commands={len} cuts={cuts:?} main_segment_us={} \
         overlay_a_us={} overlay_c_us={} uncut_us={}",
        median(main).as_micros(),
        median(a).as_micros(),
        median(c).as_micros(),
        median(whole).as_micros(),
    );
}
