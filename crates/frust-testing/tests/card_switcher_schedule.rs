//! A card switcher closing over translucent pages, frame by frame: every frame
//! of its close tail is scheduled, and the spill pool is really exercised.
//!
//! The freeze shape. A recents-style switcher shows the live page as one card
//! and each peer as a further card, dimmed by recording it inside a layer at a
//! fractional opacity and a rounded clip. Each card's face is itself a page
//! that carries translucent content: one translucent element beside a
//! translucent group that holds two translucent children. Nested inside a
//! dimmed card, that page is a depth-two layer whose parent round has to keep
//! a group for the translucent element while the fan beneath it takes two more
//! at once. A surface has no fallback renderer, so a frame the scheduler
//! refuses is a skipped frame, and a refusal that holds for the whole of a
//! gesture reads on a device as a frozen screen rather than as a wrong pixel.
//!
//! The shape is recorded through the real widget, not by hand: the widget tree
//! is built from `frust-widgets`' public API, driven through its close ramp on
//! one retained root exactly as a shell drives it, and each captured frame is
//! compiled and scheduled the way the engine's own scheduler tests do. The
//! assertions are that every frame schedules, and that at least one of them
//! really takes a page from the spill pool, so the test cannot pass vacuously
//! if the widgets stop recording the shape.
//!
//! The scheduler is host-only (no device is opened), so this runs in the
//! ordinary gate.

use frust_core::{FrameTime, RenderRoot, View};
use frust_engine::SceneCompiler;
use frust_engine::schedule::{PageConfig, RoundOp, RoundTarget, Schedule};
use frust_gpu::{DownlevelProfile, TierCaps};
use frust_scene::Scene;
use frust_testing::frame::{FrameSpec, frame, test_text_context};
use frust_theme::Theme;
use frust_widgets::motion::{animated_opacity, card_switcher, peer_card};
use frust_widgets::{Column, EdgeInsets, Padding, colored_box, container};
use kurbo::{Affine, Size};
use peniko::Color;

/// The translucency every layered element of a page is drawn at.
const HALF: f64 = 0.5;

/// Logical viewport; small, since only the layer structure matters.
const VIEWPORT: Size = Size::new(402.0, 874.0);

/// Milliseconds after the close begins that frames are captured at.
///
/// The first is the instant the close starts, the rest sweep the tail. The
/// card alphas, geometry and clip radius all move over this window.
const CLOSE_TAIL_MS: &[u64] = &[0, 16, 48, 96, 160, 224, 288, 320];

/// A translucent solid block.
fn block(color: Color) -> impl View<bool> {
    animated_opacity(HALF, colored_box().fill(color).size(120.0, 40.0))
}

/// One card's face: a translucent element beside a translucent group holding
/// two translucent children, over a rounded backing.
fn page() -> impl View<bool> {
    let group = animated_opacity(
        HALF,
        Column((
            block(Color::from_rgb8(200, 60, 60)),
            block(Color::from_rgb8(60, 200, 60)),
        )),
    );
    container(Padding(
        EdgeInsets::all(16.0),
        Column((block(Color::from_rgb8(60, 60, 200)), group)),
    ))
    .fill(Color::from_rgb8(240, 240, 240))
    .radius(24.0)
    .expand()
}

/// The switcher: the live page centred, a dimmed peer on each side.
fn switcher(open: bool) -> impl View<bool> {
    card_switcher(open, page())
        .peers([peer_card(page()), peer_card(page())])
        .live_index(1)
}

fn ms(ms: u64) -> FrameTime {
    FrameTime::from_nanos(ms * 1_000_000)
}

#[test]
fn card_switcher_close_tail_schedules_every_frame_and_uses_the_spill_pool() {
    let (mut tcx, _family) = test_text_context();
    let mut root: RenderRoot<bool, _> = RenderRoot::new();
    root.set_theme(Box::new(Theme::neutral()));
    let mut open = true;
    let mut logic = |open: &mut bool| switcher(*open);
    let base = FrameSpec::phone();
    let spec_at = |time| FrameSpec {
        size: VIEWPORT,
        time,
        ..base
    };

    // Mount already open, then settle so the close is a real transition.
    let mut scene = Scene::new();
    frame(
        &mut root,
        &mut logic,
        &mut open,
        &mut tcx,
        &spec_at(ms(0)),
        &mut scene,
    );
    frame(
        &mut root,
        &mut logic,
        &mut open,
        &mut tcx,
        &spec_at(ms(1000)),
        &mut scene,
    );

    // Flip to closed: the first frame after the flip seeds the ramp's clock.
    open = false;
    let started = 2000;
    frame(
        &mut root,
        &mut logic,
        &mut open,
        &mut tcx,
        &spec_at(ms(started)),
        &mut scene,
    );

    let size = (
        u16::try_from(spec_at(ms(0)).physical_width()).expect("inside the device grid"),
        u16::try_from(spec_at(ms(0)).physical_height()).expect("inside the device grid"),
    );
    let mut spill_slots = Vec::new();
    for &offset in CLOSE_TAIL_MS {
        let mut scene = Scene::new();
        frame(
            &mut root,
            &mut logic,
            &mut open,
            &mut tcx,
            &spec_at(ms(started + offset)),
            &mut scene,
        );
        let compiled = SceneCompiler::new(size.0, size.1)
            .compile(&scene, Affine::IDENTITY, size)
            .expect("an in-range scene compiles");
        let rounds = Schedule::build(
            &compiled.recorder,
            &TierCaps::fake(DownlevelProfile::Full),
            &PageConfig::default(),
        )
        .unwrap_or_else(|error| panic!("close tail +{offset} ms was refused: {error:?}"));
        for round in &rounds {
            if let RoundTarget::Page(page) = &round.target
                && let Some(slot) = page.parity.spill_slot()
            {
                spill_slots.push((offset, slot));
            }
            for op in &round.ops {
                if let RoundOp::Composite(composite) = op
                    && composite.retained.is_none()
                    && let Some(slot) = composite.parity.spill_slot()
                {
                    spill_slots.push((offset, slot));
                }
            }
        }
    }
    assert!(
        !spill_slots.is_empty(),
        "no frame of the close tail took a spill page, so the shape this test guards is no longer produced"
    );
    // Slot 0 is the single spill page the scheduler used to have; a slot past
    // it is a frame that earlier scheduler could not have served.
    assert!(
        spill_slots.iter().any(|&(_, slot)| slot >= 1),
        "no frame needed a second spill page: {spill_slots:?}"
    );
}
