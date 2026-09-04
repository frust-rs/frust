//! The perspective sheet-fan: the effect a stack of cards or sheets spreads
//! through when it opens, built entirely on [`quad3d`](super::quad3d).
//!
//! A fan is a small pile of same-sized sheets that pivot apart in their own
//! plane as the pile opens — a hand of cards, the previews inside a folder.
//! Its 2D form is a rotation about each sheet's centre plus a paint order, and
//! that form is exact right up to the moment two sheets need to sit at
//! *different distances*: painter order can only say which was drawn last, and
//! an `Affine` rotation keeps every edge parallel, so the sheet that has swung
//! furthest out never grows the near edge a real pivot would.
//!
//! This module adds those two: a per-slot displacement into depth, and a swing
//! about the vertical axis that grows with the fan's own progress.
//!
//! # What a component gets from this module, and what it still owns
//!
//! Everything here is pure geometry, testable without a device. The handle
//! lifecycle is [`Card3d`](super::card3d::Card3d) and the overscanned target is
//! [`card3d::target_for`](super::card3d::target_for) — the substrate has one of
//! each, and a fan uses them rather than restating them.
//!
//! What a component still owns is **the placement**: where each sheet sits,
//! how big it is drawn, how far it has turned in its own plane, and in what
//! order the pile stacks. Those come from the component's existing fan
//! arithmetic — the same call that places its flat sheets places these faces —
//! and this module renders that placement rather than recomputing it.
//!
//! # Depth, and why the sheets are still submitted back-to-front
//!
//! [`fan_scene`] turns the depth attachment **on** and keeps the caller's own
//! order, which for a fan means *back-to-front*. That looks redundant and is
//! not: fan sheets are translucent, and a translucent face writes depth like
//! an opaque one ([`quad3d`](super::quad3d)'s *Depth*), so back-to-front
//! submission is what keeps a nearer sheet blending **over** a further one
//! instead of rejecting it outright — the see-through pile the 2D path draws.
//! The attachment is then the guard for the case painter order cannot express:
//! a sheet that has swung far enough to genuinely cross another is resolved by
//! distance, whatever order it arrived in.
//!
//! # What this cannot carry
//!
//! - **A sheet is a colour, a ramp or a caller-owned texture — never a widget
//!   subtree.** The substrate-wide boundary. A sheet's caption and any content
//!   on it keep painting flat, above the composited fan.
//! - **A sheet has square corners**, so a component with a rounded sheet keeps
//!   that rounding in its flat path and accepts square corners on the
//!   projected surface — there is no projective clip to round a turned face.
//! - **The swing is a yaw, not a hinge.** Upstream fans pivot about a point
//!   below the pile; this swings each sheet about its own centre, because
//!   [`Quad3d`] rotates about its destination rectangle's centre and an
//!   off-centre pivot would have to be baked into the destination the caller
//!   already computed.

use frust::authoring::Rect;

use super::quad3d::{Quad3d, Quad3dScene, QuadFace};

/// How far the frontmost sheet sits toward the viewer, in world units (one
/// unit is the target's height).
///
/// A lift of `l` magnifies a face by `CAMERA_DISTANCE / (CAMERA_DISTANCE − l)`,
/// so `0.06` grows the front sheet by a little over three percent — enough for
/// the pile to read as *stacked* rather than as one flat card, and little
/// enough that an opening fan does not appear to zoom.
pub const SHEET_LIFT: f32 = 0.06;

/// How far each slot back from the front sheet recedes, in world units.
///
/// Deliberately smaller than [`SHEET_LIFT`]: a folder shows a handful of
/// sheets, and a step large enough to be read as depth on two of them would
/// have pushed the last one visibly out of size with the rest.
pub const SHEET_STEP: f32 = 0.02;

/// How far the outermost sheet swings into depth at a fully open fan, in
/// degrees.
///
/// Matched to the in-plane rotation an opened fan already carries at its ends,
/// so the swing reads as the same pivot continuing into depth rather than as a
/// second, unrelated turn.
pub const SHEET_SWING_DEGREES: f64 = 12.0;

/// One sheet of a fan, as the component has already placed it.
#[derive(Clone, Debug)]
pub struct FanSheet {
    /// Where the sheet sits, in the target's texel space, at the size the
    /// component drew it — its own scale included.
    pub dest: Rect,
    /// What fills it.
    pub face: QuadFace,
    /// The sheet's rotation in its own plane, in degrees, clockwise on screen —
    /// the component's existing fan rotation, unchanged.
    pub roll_degrees: f64,
    /// Where the sheet sits in the pile: `0` is the sheet nearest the viewer
    /// and each step back recedes by [`SHEET_STEP`]. A negative slot is
    /// treated as the front one.
    pub slot: i32,
    /// Where the sheet sits along the fan, as a signed fraction in `-1..=1`:
    /// `-1` at the left end, `0` on the centre line, `1` at the right end.
    /// This is what the swing is scaled by, so the ends turn and the middle
    /// stays square to the viewer.
    pub swing: f64,
}

/// The face for one sheet of a fan `open` of the way spread (`0` closed, `1`
/// fully fanned).
///
/// The roll and the depth are the sheet's own and do not depend on `open` —
/// the component has already animated them — while the swing does: a closed
/// pile is square to the viewer, and the turn into depth grows with the
/// spread, so switching the effect on changes nothing about a closed fan.
///
/// Every angle is sanitised here rather than downstream: this runs inside an
/// `ExternalPass`, and a non-finite spread arriving from a stalled animation
/// would otherwise reach the GPU as a `NaN` transform.
#[must_use]
pub fn sheet_quad(sheet: &FanSheet, open: f64) -> Quad3d {
    let open = if open.is_finite() {
        open.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let swing = if sheet.swing.is_finite() {
        sheet.swing.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let roll = if sheet.roll_degrees.is_finite() {
        sheet.roll_degrees
    } else {
        0.0
    };
    let depth = SHEET_LIFT - sheet.slot.max(0) as f32 * SHEET_STEP;
    Quad3d::new(sheet.dest, sheet.face.clone())
        .roll(roll.to_radians() as f32)
        .yaw((swing * SHEET_SWING_DEGREES * open).to_radians() as f32)
        .depth_offset(depth)
}

/// A whole fan `open` of the way spread: every sheet, **depth-tested**, in the
/// caller's own order — which for a fan is back-to-front. See the [module
/// docs](self) for why both halves of that are needed.
#[must_use]
pub fn fan_scene<'a>(sheets: impl IntoIterator<Item = &'a FanSheet>, open: f64) -> Quad3dScene {
    let mut scene = Quad3dScene::new().with_depth(true);
    for sheet in sheets {
        scene.push(sheet_quad(sheet, open));
    }
    scene
}

/// The signed position along a fan of `count` sheets that sheet `index` sits
/// at, in `-1..=1` — the value [`FanSheet::swing`] takes.
///
/// The fan is symmetric about its middle, so an odd count puts one sheet dead
/// centre at `0` and an even one straddles. A fan of one sheet is all centre.
#[must_use]
pub fn swing_of(index: usize, count: usize) -> f64 {
    if count <= 1 {
        return 0.0;
    }
    let half = (count as f64 - 1.0) / 2.0;
    (index as f64 - half) / half
}

#[cfg(test)]
mod tests {
    use super::{
        FanSheet, SHEET_LIFT, SHEET_STEP, SHEET_SWING_DEGREES, fan_scene, sheet_quad, swing_of,
    };
    use crate::gpu_fx::quad3d::QuadFace;
    use frust::authoring::Rect;
    use peniko::Color;

    fn sheet(slot: i32, swing: f64) -> FanSheet {
        FanSheet {
            dest: Rect::new(0.0, 0.0, 96.0, 160.0),
            face: QuadFace::Solid(Color::WHITE),
            roll_degrees: swing * 6.0,
            slot,
            swing,
        }
    }

    /// A closed fan is square to the viewer: the swing is the one property the
    /// spread drives, so switching the 3D path on changes nothing about a pile
    /// at rest beyond the depth that stacks it.
    #[test]
    fn a_closed_fan_is_square_to_the_viewer() {
        let quad = sheet_quad(&sheet(0, 1.0), 0.0);
        assert!(quad.yaw.abs() < 1e-9);
        assert!(quad.pitch.abs() < 1e-9);
        assert!((f64::from(quad.roll) - 6.0_f64.to_radians()).abs() < 1e-6);
        assert!((quad.depth_offset - SHEET_LIFT).abs() < 1e-6);
        assert!(quad.is_renderable());
    }

    /// Opening the fan turns its ends into depth, in opposite directions, and
    /// leaves its middle facing the viewer.
    #[test]
    fn opening_the_fan_swings_its_ends_into_depth() {
        let left = sheet_quad(&sheet(1, -1.0), 1.0);
        let middle = sheet_quad(&sheet(0, 0.0), 1.0);
        let right = sheet_quad(&sheet(1, 1.0), 1.0);
        assert!(middle.yaw.abs() < 1e-9);
        assert!(left.yaw < 0.0 && right.yaw > 0.0);
        assert!((left.yaw + right.yaw).abs() < 1e-6, "the fan is symmetric");
        assert!(
            (f64::from(right.yaw) - SHEET_SWING_DEGREES.to_radians()).abs() < 1e-6,
            "the end sheet reaches the full swing"
        );

        // Half open is half the turn, and the roll is untouched by it.
        let half = sheet_quad(&sheet(1, 1.0), 0.5);
        assert!((f64::from(half.yaw) - f64::from(right.yaw) / 2.0).abs() < 1e-6);
        assert!((half.roll - right.roll).abs() < 1e-9);
    }

    /// The pile is ordered by distance: the front sheet is lifted, each slot
    /// back recedes by one step, and a nonsense slot lands at the front rather
    /// than in front of it.
    #[test]
    fn each_slot_back_recedes_by_one_step() {
        let depths: Vec<f32> = (0..3)
            .map(|slot| sheet_quad(&sheet(slot, 0.0), 1.0).depth_offset)
            .collect();
        assert!((depths[0] - SHEET_LIFT).abs() < 1e-6);
        assert!((depths[0] - depths[1] - SHEET_STEP).abs() < 1e-6);
        assert!((depths[1] - depths[2] - SHEET_STEP).abs() < 1e-6);
        assert!((sheet_quad(&sheet(-4, 0.0), 1.0).depth_offset - SHEET_LIFT).abs() < 1e-6);
    }

    /// A non-finite spread or swing never reaches the GPU as a transform.
    #[test]
    fn a_nonsense_spread_flattens_rather_than_propagating() {
        let quad = sheet_quad(&sheet(0, 1.0), f64::NAN);
        assert!(quad.yaw.abs() < 1e-9);
        assert!(quad.is_renderable());

        let mut wild = sheet(0, f64::INFINITY);
        wild.roll_degrees = f64::NAN;
        let quad = sheet_quad(&wild, 1.0);
        assert!(quad.yaw.abs() < 1e-9);
        assert!(quad.roll.abs() < 1e-9);
        assert!(quad.is_renderable());

        // A swing past the ends is clamped to them, not extrapolated.
        let quad = sheet_quad(&sheet(0, 4.0), 1.0);
        assert!((f64::from(quad.yaw) - SHEET_SWING_DEGREES.to_radians()).abs() < 1e-6);
    }

    /// The scene is depth-tested and keeps the caller's back-to-front order,
    /// which is what preserves a translucent pile's see-through stacking.
    #[test]
    fn a_fan_scene_is_depth_tested_in_the_callers_order() {
        let sheets = [sheet(2, -1.0), sheet(1, 0.0), sheet(0, 1.0)];
        let scene = fan_scene(&sheets, 1.0);
        assert!(scene.depth);
        assert_eq!(scene.len(), 3);
        assert!(
            scene.quads[0].depth_offset < scene.quads[2].depth_offset,
            "the furthest sheet is submitted first"
        );
        assert!(scene.quads.iter().all(super::Quad3d::is_renderable));
    }

    /// The fan's own symmetry, from the index side.
    #[test]
    fn the_swing_runs_from_end_to_end_about_the_middle() {
        assert!((swing_of(0, 5) + 1.0).abs() < 1e-9);
        assert!(swing_of(2, 5).abs() < 1e-9);
        assert!((swing_of(4, 5) - 1.0).abs() < 1e-9);
        // An even fan straddles the centre line.
        assert!((swing_of(1, 4) + 1.0 / 3.0).abs() < 1e-9);
        assert!((swing_of(2, 4) - 1.0 / 3.0).abs() < 1e-9);
        // Degenerate fans are all centre.
        assert!(swing_of(0, 1).abs() < 1e-9);
        assert!(swing_of(0, 0).abs() < 1e-9);
    }
}

/// The fan against real hardware. Ignored by default; see
/// [`crate::gpu_fx::test_gpu`] for the invocation and why a read-back is the
/// only honest check here.
#[cfg(test)]
mod gpu_tests {
    use frust::authoring::{Point, Rect, Size, Vec2};
    use peniko::Color;

    use super::{FanSheet, fan_scene};
    use crate::gpu_fx::card3d::target_for;
    use crate::gpu_fx::card3d::test_render::{pixel, render};
    use crate::gpu_fx::quad3d::{Quad3dRenderer, QuadFace};
    use crate::gpu_fx::test_gpu::with_device;

    /// The card every case renders through: 288x224 logical px, the folder's
    /// own box, which overscans to 404x314 texels.
    const CARD: Size = Size::new(288.0, 224.0);

    /// One sheet's box.
    const SHEET: Size = Size::new(96.0, 160.0);

    /// Every sheet arrives: three overlapping sheets are all present in the
    /// render, the frontmost one wins where all three cover, and the pile is
    /// stacked by distance rather than by nothing at all.
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn a_fan_renders_every_sheet_and_stacks_them_by_distance() {
        with_device(|device, queue| {
            let mut renderer = Quad3dRenderer::new(device, "fan-stacking");
            let (_, extent) = target_for(Point::ORIGIN, CARD).expect("a card with area");
            let centre = Point::new(f64::from(extent.0) / 2.0, f64::from(extent.1) / 2.0);
            let colors = [
                Color::from_rgba8(0, 0, 255, 255),
                Color::from_rgba8(0, 255, 0, 255),
                Color::from_rgba8(255, 0, 0, 255),
            ];
            // Back to front, spread far enough that the three genuinely
            // overlap but not so far that any leaves the target.
            let sheets: Vec<FanSheet> = (0..3)
                .map(|i| {
                    let swing = f64::from(i) - 1.0;
                    FanSheet {
                        dest: Rect::from_center_size(centre + Vec2::new(swing * 30.0, 0.0), SHEET),
                        face: QuadFace::Solid(colors[i as usize]),
                        roll_degrees: swing * 6.0,
                        slot: 2 - i,
                        swing,
                    }
                })
                .collect();

            let frame = render(
                device,
                queue,
                &mut renderer,
                extent,
                true,
                &fan_scene(&sheets, 1.0),
            );
            let present = |color: [u8; 4]| {
                (0..extent.0 * extent.1)
                    .map(|i| (i % extent.0, i / extent.0))
                    .any(|(x, y)| pixel(&frame, extent.0, x, y) == color)
            };
            for (index, color) in colors.iter().enumerate() {
                let rgba = [
                    (color.components[0] * 255.0) as u8,
                    (color.components[1] * 255.0) as u8,
                    (color.components[2] * 255.0) as u8,
                    255,
                ];
                assert!(
                    present(rgba),
                    "sheet {index} ({rgba:?}) never reached the target"
                );
            }

            let hit = pixel(&frame, extent.0, centre.x as u32, centre.y as u32);
            println!("fan stacking: centre {hit:?}");
            assert_eq!(
                hit,
                [255, 0, 0, 255],
                "the front sheet should own the overlap"
            );
        });
    }

    /// The depth attachment is a real guard, not decoration: a sheet from the
    /// back of the pile submitted *after* the front one still loses, which is
    /// the case painter order gets wrong.
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn a_back_sheet_submitted_last_still_loses() {
        with_device(|device, queue| {
            let mut renderer = Quad3dRenderer::new(device, "fan-depth-guard");
            let (_, extent) = target_for(Point::ORIGIN, CARD).expect("a card with area");
            let centre = Point::new(f64::from(extent.0) / 2.0, f64::from(extent.1) / 2.0);
            let plate = |slot: i32, color: Color| FanSheet {
                dest: Rect::from_center_size(centre, SHEET),
                face: QuadFace::Solid(color),
                roll_degrees: 0.0,
                slot,
                swing: 0.0,
            };
            let front = plate(0, Color::from_rgba8(255, 0, 0, 255));
            let back = plate(3, Color::from_rgba8(0, 0, 255, 255));

            let sheets = [front, back];
            let occluded = render(
                device,
                queue,
                &mut renderer,
                extent,
                true,
                &fan_scene(&sheets, 1.0),
            );
            let (x, y) = (centre.x as u32, centre.y as u32);
            assert_eq!(
                pixel(&occluded, extent.0, x, y),
                [255, 0, 0, 255],
                "the front sheet survives a back one drawn after it"
            );

            let painted = render(
                device,
                queue,
                &mut renderer,
                extent,
                false,
                &fan_scene(&sheets, 1.0).with_depth(false),
            );
            assert_eq!(
                pixel(&painted, extent.0, x, y),
                [0, 0, 255, 255],
                "without depth the same order is plain painter order"
            );
        });
    }
}
