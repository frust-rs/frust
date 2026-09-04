//! Cylindrical 3D: the drum the catalog's rotating surfaces seat their
//! *faces* on, built entirely on [`quad3d`](super::quad3d).
//!
//! One shape, two axes. A **wheel** turns about a horizontal axis, so its
//! faces travel up and down the screen and each one pitches into the wall; a
//! **carousel** turns about a vertical one, so its faces travel left and right
//! and each one yaws. Both are the same three numbers — a displacement along
//! the travel axis, a displacement into depth, and a rotation matching the
//! wall's own tangent — so they live in one [`Cylinder3d`] rather than in two
//! modules that would each restate the trigonometry.
//!
//! # What a component gets from this module, and what it still owns
//!
//! Everything here is pure geometry, testable without a device. The handle
//! lifecycle is [`Card3d`](super::card3d::Card3d) — the substrate has one
//! handle type, and a drum claims it under its own label rather than
//! introducing a second one — and the overscanned target is
//! [`card3d::target_for`](super::card3d::target_for), for the reasons that
//! module's docs give.
//!
//! What a component still owns is **the angle**. This module renders a drum
//! position; it never computes one. The component's existing momentum, snap
//! and wrap arithmetic stays the single source of truth for where the drum is,
//! and the same call that places its flat rows places these faces — which is
//! what keeps the 2D and 3D paths from drifting apart by a fraction of a row.
//! It likewise owns **the pick**: a pointer lands on the component, and the
//! component's own hit logic resolves it. [`frontmost`] is the only mapping
//! offered here, and it answers a question about the *drum* (which seated item
//! is facing the viewer), not about the pointer.
//!
//! # The sign of every turn
//!
//! A face sits on the wall, so its rotation is the wall's tangent at its own
//! angle, and the four combinations follow from that one rule plus the two
//! substrate conventions ([`Quad3d::yaw`] positive brings the *left* edge
//! toward the camera, [`Quad3d::pitch`] positive brings the *top* edge toward
//! it — the pitch half read back off hardware by `card3d`'s
//! `a_pitched_face_lifts_its_top_edge`):
//!
//! | axis | side | face at `+angle` | rotation |
//! |---|---|---|---|
//! | horizontal | convex (outside) | above centre, receding | `−angle` (pitch) |
//! | horizontal | concave (inside) | above centre, approaching | `+angle` (pitch) |
//! | vertical | convex (outside) | right of centre, receding | `+angle` (yaw) |
//! | vertical | concave (inside) | right of centre, approaching | `−angle` (yaw) |
//!
//! The pitch column looks inverted against the yaw column and is not: target
//! space runs `y` **down** and world space runs it up, so a face travelling to
//! a positive angle moves *up* the screen while a face at a positive angle on
//! the other axis moves *right*.
//!
//! # Depth is the whole point, and it needs the nearest face first
//!
//! [`drum_scene`] turns the depth attachment **on** and sorts the faces
//! **nearest first**. A drum is exactly the case painter order cannot serve:
//! a face on the far side of the wall overlaps a near one on screen, and which
//! one wins has to be decided by distance, not by which the component happened
//! to emit last.
//!
//! The sort is the half that is easy to get wrong, and hardware said so before
//! this module claimed otherwise. Depth writes only *reject* a fragment that
//! is behind one already written: with the far face drawn first, the near one
//! still passes and **blends over it**, so a translucent drum darkens along
//! every seam where two rows overlap — and only on the side of the centre
//! whose item order happens to run far-to-near, which reads as a band across
//! half the drum. Drawing the nearest face first makes the farther one drop
//! out outright, so every texel shows exactly one face and the wall reads as
//! one surface. With opaque faces the sort costs nothing and changes nothing.
//! Item order carries no meaning on a drum, unlike a fan's back-to-front pile
//! ([`fan`](super::fan)), which is why this is the module's business rather
//! than each component's.
//!
//! # What this cannot carry
//!
//! - **A face is a colour, a ramp or a caller-owned texture — never a widget
//!   subtree.** The substrate-wide boundary. A wheel's row text and a
//!   carousel's item content keep painting flat, above the composited drum,
//!   exactly as a wallet row's content does over its plate.
//! - **A face has square corners**, for the reason `card3d` gives: a rounded
//!   clip is a rectangle in scene space and a seated face is not one.
//! - **The camera is the substrate's, not the component's.** A component whose
//!   2D path spends a perspective divide of its own
//!   ([`PERSPECTIVE`](crate::components::wheel_picker::PERSPECTIVE) is an
//!   absolute CSS distance) seats its faces through
//!   [`Cylinder3d::face`] and finds them a few percent smaller than the flat
//!   content riding on them at the far end of the arc, where the two
//!   projections disagree most. A component that cannot afford that
//!   disagreement seats them through [`Cylinder3d::scaled_face`] instead,
//!   which takes the depth from the scale the component itself computed and
//!   lands the face on precisely the box it drew. Either way the faces are a
//!   *surface*: the content is the component's, placed by the component's own
//!   arithmetic.

use std::f64::consts::FRAC_PI_2;

use frust::authoring::{Point, Rect, Size, Vec2};

use super::quad3d::{CAMERA_DISTANCE, Quad3d, Quad3dScene, QuadFace};

/// How far around the drum a face is still worth submitting, in radians.
///
/// At exactly a quarter turn a face is edge-on: it projects onto a line, it
/// covers no texel, and it is the same face the component's own 2D path culls
/// at its horizon. Past it the face has turned onto the hidden side of the
/// wall, where only the depth attachment would keep it out of view — and
/// depth-testing a face that cannot be seen is work spent to draw nothing.
pub const VISIBLE_ARC: f64 = FRAC_PI_2;

/// Which way a drum's axis runs, and therefore which way its faces travel and
/// which rotation seats them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CylinderAxis {
    /// The axis runs horizontally: faces travel **up and down** the screen and
    /// each pitches into the wall. The shape a wheel picker's drum is.
    Horizontal,
    /// The axis runs vertically: faces travel **left and right** and each yaws
    /// into the wall. The shape a carousel's cylinder is.
    Vertical,
}

/// Where one face lands on the drum, in the component's own units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CylinderSeat {
    /// Displacement from the drum's centre, in logical px, already carrying
    /// the screen's `y`-down convention — a caller adds it to its centre point
    /// and is done.
    pub travel: Vec2,
    /// Displacement toward the viewer, in logical px: negative for a face that
    /// has receded behind the drum's front plane, positive for one that has
    /// come forward of it.
    pub depth: f64,
    /// The face's own rotation into the wall, in radians — a pitch on a
    /// horizontal axis and a yaw on a vertical one, signed as the [module
    /// docs](self) table sets out.
    pub rotation: f64,
}

/// The drum a component seats its faces on: how big it is, which way it turns,
/// and which side of its wall the faces line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cylinder3d {
    /// The drum's radius, in logical px — the component's own, never derived
    /// here, since it is the same radius its 2D path places rows with.
    pub radius: f64,
    /// Which way the axis runs.
    pub axis: CylinderAxis,
    /// Whether the faces line the **outside** of the wall — the centre face
    /// nearest and the rest receding, which is what a wheel drum looks like —
    /// or the inside, where the centre face is furthest and the arc comes
    /// forward toward the viewer at both ends.
    pub convex: bool,
}

impl Cylinder3d {
    /// A drum of `radius` logical px turning about `axis`, its faces lining
    /// the outside (`convex`) or the inside of the wall.
    #[must_use]
    pub const fn new(radius: f64, axis: CylinderAxis, convex: bool) -> Self {
        Self {
            radius,
            axis,
            convex,
        }
    }

    /// Where a face `angle` radians around the drum from the front sits.
    ///
    /// A negative radius is treated as zero — a drum with no radius is a flat
    /// stack of coincident faces rather than one turned inside out.
    #[must_use]
    pub fn seat(self, angle: f64) -> CylinderSeat {
        let radius = self.radius.max(0.0);
        let (sin, cos) = angle.sin_cos();
        let travel = match self.axis {
            // `y` runs down in target space and up in world space, so a face at
            // a positive angle sits *above* the drum's centre.
            CylinderAxis::Horizontal => Vec2::new(0.0, -radius * sin),
            CylinderAxis::Vertical => Vec2::new(radius * sin, 0.0),
        };
        let depth = if self.convex {
            radius * (cos - 1.0)
        } else {
            radius * (1.0 - cos)
        };
        let rotation = match (self.axis, self.convex) {
            (CylinderAxis::Horizontal, true) => -angle,
            (CylinderAxis::Horizontal, false) => angle,
            (CylinderAxis::Vertical, true) => angle,
            (CylinderAxis::Vertical, false) => -angle,
        };
        CylinderSeat {
            travel,
            depth,
            rotation,
        }
    }

    /// Whether a face at `angle` is on the visible half of the wall and worth
    /// submitting — see [`VISIBLE_ARC`]. A non-finite angle is never visible,
    /// which is what keeps a `NaN` scroll position from reaching the GPU as a
    /// transform.
    #[must_use]
    pub fn visible(self, angle: f64) -> bool {
        angle.is_finite() && angle.abs() < VISIBLE_ARC
    }

    /// The box a face of `item` takes when it is seated: `item`-sized, centred
    /// on `centre` displaced by the seat's own travel.
    ///
    /// `centre` is the drum's centre in the target's texel space, so the
    /// result is a destination rectangle ready for [`Self::face`].
    #[must_use]
    pub fn seated_rect(self, centre: Point, item: Size, seat: CylinderSeat) -> Rect {
        Rect::from_center_size(centre + seat.travel, item)
    }

    /// The face at `seat`, filling `dest`, seated in depth by the caller's own
    /// projected `scale` rather than by the drum's radius.
    ///
    /// The seam for a component whose 2D path already spends a perspective
    /// divide of its own: seating such a face by the drum's radius would apply
    /// a *second* projection, and the plate would come out a different size
    /// from the flat content riding on it. [`depth_for_scale`] is the exact
    /// inverse of the substrate camera, so a plate placed this way projects
    /// onto precisely the box the component drew — and the drum still supplies
    /// what only it knows, the turn into the wall.
    #[must_use]
    pub fn scaled_face(self, seat: CylinderSeat, dest: Rect, face: QuadFace, scale: f64) -> Quad3d {
        let quad = Quad3d::new(dest, face).depth_offset(depth_for_scale(scale));
        match self.axis {
            CylinderAxis::Horizontal => quad.pitch(seat.rotation as f32),
            CylinderAxis::Vertical => quad.yaw(seat.rotation as f32),
        }
    }

    /// The face at `seat`, filling `dest`.
    ///
    /// `target_height` is the target's own height in texels, which is what one
    /// world unit measures ([`quad3d`](super::quad3d)'s camera) — so a depth in
    /// logical px divided by it is a depth in world units, and one texel stays
    /// one logical pixel across the whole seat.
    #[must_use]
    pub fn face(
        self,
        seat: CylinderSeat,
        dest: Rect,
        face: QuadFace,
        target_height: f64,
    ) -> Quad3d {
        let depth = if target_height > 0.0 {
            (seat.depth / target_height) as f32
        } else {
            0.0
        };
        let quad = Quad3d::new(dest, face).depth_offset(depth);
        match self.axis {
            CylinderAxis::Horizontal => quad.pitch(seat.rotation as f32),
            CylinderAxis::Vertical => quad.yaw(seat.rotation as f32),
        }
    }
}

/// The displacement along `+Z`, in world units, at which the substrate's
/// camera projects a face at `scale` — the inverse of the perspective divide
/// [`CAMERA_DISTANCE`](super::quad3d::CAMERA_DISTANCE) defines.
///
/// `z = D · (s − 1) / s`, so a scale of `1` is the reference plane, a larger
/// one is in front of it and a smaller one behind. A scale at or below zero
/// has no plane at all — it is behind the camera — and answers the reference
/// plane rather than an infinity.
#[must_use]
pub fn depth_for_scale(scale: f64) -> f32 {
    if !scale.is_finite() || scale <= 0.0 {
        return 0.0;
    }
    (f64::from(CAMERA_DISTANCE) * (scale - 1.0) / scale) as f32
}

/// A drum's whole scene: every seated face, **depth-tested**, sorted
/// **nearest first**.
///
/// The sort is what makes a translucent drum read as one surface instead of
/// darkening along every seam — see the [module docs](self). It is stable, so
/// faces at equal depth keep the component's own order, and a non-finite
/// depth sorts to the back rather than poisoning the comparison.
#[must_use]
pub fn drum_scene(faces: impl IntoIterator<Item = Quad3d>) -> Quad3dScene {
    let mut scene = Quad3dScene::new().with_depth(true);
    scene.quads.extend(faces);
    scene
        .quads
        .sort_by(|a, b| b.depth_offset.total_cmp(&a.depth_offset));
    scene
}

/// Which of `angles` is facing the viewer — the smallest turn from the front,
/// or `None` when nothing on the drum is finite or the drum is empty.
///
/// The drum's half of a pick, and the only half this module owns: a component
/// resolves a pointer against its own boxes and its own hit rules, then asks
/// the drum which item that leaves at the front. Ties go to the earlier index,
/// so a drum parked exactly between two items reports the one nearer the start
/// of the list rather than an arbitrary one.
#[must_use]
pub fn frontmost(angles: impl IntoIterator<Item = f64>) -> Option<usize> {
    angles
        .into_iter()
        .enumerate()
        .filter(|(_, angle)| angle.is_finite())
        .min_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::{Cylinder3d, CylinderAxis, VISIBLE_ARC, drum_scene, frontmost};
    use crate::gpu_fx::quad3d::QuadFace;
    use frust::authoring::{Point, Rect, Size};
    use peniko::Color;

    const STEP: f64 = std::f64::consts::FRAC_PI_6;

    fn wheel() -> Cylinder3d {
        Cylinder3d::new(60.0, CylinderAxis::Horizontal, true)
    }

    fn carousel() -> Cylinder3d {
        Cylinder3d::new(200.0, CylinderAxis::Vertical, false)
    }

    fn solid() -> QuadFace {
        QuadFace::Solid(Color::WHITE)
    }

    /// The property the whole drum hangs on: the item at angle zero is dead
    /// front — no travel, no depth, no rotation — and its two neighbours are
    /// mirror images of each other.
    #[test]
    fn the_item_at_the_front_is_unmoved_and_its_neighbours_are_symmetric() {
        let drum = wheel();
        let front = drum.seat(0.0);
        assert!(front.travel.hypot() < 1e-9);
        assert!(front.depth.abs() < 1e-9);
        assert!(front.rotation.abs() < 1e-9);

        let above = drum.seat(STEP);
        let below = drum.seat(-STEP);
        assert!(
            above.travel.y < 0.0,
            "a positive angle sits above the centre"
        );
        assert!((above.travel.y + below.travel.y).abs() < 1e-9);
        assert!(
            (above.depth - below.depth).abs() < 1e-9,
            "equal turns, equal depth"
        );
        assert!((above.rotation + below.rotation).abs() < 1e-9);
        assert!(
            above.travel.x.abs() < 1e-9,
            "a horizontal axis travels in y alone"
        );

        // ...and the travel is the drum's own chord, not an approximation of it.
        assert!((above.travel.y + 60.0 * STEP.sin()).abs() < 1e-9);
        assert!((above.depth - 60.0 * (STEP.cos() - 1.0)).abs() < 1e-9);
    }

    /// The two walls are the same drum seen from opposite sides: an outside
    /// face recedes as it turns, an inside one comes forward, and each leans
    /// the other way.
    #[test]
    fn an_outside_wall_recedes_where_an_inside_one_comes_forward() {
        let outside = Cylinder3d::new(60.0, CylinderAxis::Horizontal, true).seat(STEP);
        let inside = Cylinder3d::new(60.0, CylinderAxis::Horizontal, false).seat(STEP);
        assert!(outside.depth < 0.0 && inside.depth > 0.0);
        assert!((outside.depth + inside.depth).abs() < 1e-9);
        assert!((outside.rotation + inside.rotation).abs() < 1e-9);
        // The seat on the wall is the same place either way; only the side the
        // face is glued to differs.
        assert!((outside.travel.y - inside.travel.y).abs() < 1e-9);
    }

    /// A horizontal axis pitches its faces and a vertical one yaws them, and
    /// the signs are the ones the module's table sets out — the mapping the
    /// whole port hangs on.
    #[test]
    fn the_axis_decides_whether_a_face_pitches_or_yaws() {
        let extent_height = 240.0;
        let dest = Rect::new(0.0, 0.0, 40.0, 20.0);

        let drum = wheel();
        let seat = drum.seat(STEP);
        let face = drum.face(seat, dest, solid(), extent_height);
        assert!(face.yaw.abs() < 1e-9);
        assert!(
            face.pitch < 0.0,
            "an outside face above the centre leans its top away"
        );
        assert!((f64::from(face.pitch) + STEP).abs() < 1e-6);
        assert!(face.depth_offset < 0.0, "it has receded");
        assert!((f64::from(face.depth_offset) - seat.depth / extent_height).abs() < 1e-6);
        assert!(face.is_renderable());

        let rail = carousel();
        let seat = rail.seat(STEP);
        let face = rail.face(seat, dest, solid(), extent_height);
        assert!(face.pitch.abs() < 1e-9);
        assert!(
            face.yaw < 0.0,
            "an inside face right of centre turns back toward the axis"
        );
        assert!(
            face.depth_offset > 0.0,
            "an inside wall comes forward at its ends"
        );
        assert!(face.is_renderable());
    }

    /// A target with no height cannot carry a depth in world units, and the
    /// face flattens rather than producing an infinity.
    #[test]
    fn a_target_without_height_seats_a_flat_face() {
        let drum = wheel();
        let face = drum.face(
            drum.seat(STEP),
            Rect::new(0.0, 0.0, 40.0, 20.0),
            solid(),
            0.0,
        );
        assert!(face.depth_offset.abs() < 1e-9);
        assert!(face.is_renderable());
    }

    /// The horizon: a face turned edge-on covers nothing, and a drum whose
    /// position has gone non-finite submits nothing at all.
    #[test]
    fn the_horizon_culls_an_edge_on_face() {
        let drum = wheel();
        assert!(drum.visible(0.0));
        assert!(drum.visible(-VISIBLE_ARC + 0.01));
        assert!(!drum.visible(VISIBLE_ARC));
        assert!(!drum.visible(VISIBLE_ARC + 0.01));
        assert!(!drum.visible(f64::NAN));
        assert!(!drum.visible(f64::INFINITY));
    }

    /// A seated box keeps the item's own size and lands centred on the seat —
    /// which is what makes the flat content a component paints over it land in
    /// register.
    #[test]
    fn a_seated_box_keeps_its_size_and_centres_on_the_seat() {
        let drum = wheel();
        let seat = drum.seat(STEP);
        let centre = Point::new(140.0, 120.0);
        let item = Size::new(200.0, 36.0);
        let rect = drum.seated_rect(centre, item, seat);
        assert!((rect.width() - item.width).abs() < 1e-9);
        assert!((rect.height() - item.height).abs() < 1e-9);
        assert!((rect.center().x - centre.x).abs() < 1e-9);
        assert!((rect.center().y - centre.y - seat.travel.y).abs() < 1e-9);
    }

    /// The camera inverse: a face seated by a scale projects at exactly that
    /// scale, which is what keeps a plate the same size as the flat content a
    /// component draws over it.
    #[test]
    fn a_scale_seats_a_face_where_the_camera_projects_it_at_that_scale() {
        let camera = f64::from(crate::gpu_fx::quad3d::CAMERA_DISTANCE);
        for scale in [0.55, 0.9, 1.0, 1.25] {
            let depth = f64::from(super::depth_for_scale(scale));
            // The projection this depth produces is the scale it came from.
            assert!((camera / (camera - depth) - scale).abs() < 1e-6, "{scale}");
        }
        assert!(super::depth_for_scale(1.0).abs() < 1e-9);
        assert!(super::depth_for_scale(0.5) < 0.0);
        assert!(super::depth_for_scale(2.0) > 0.0);
        // Nonsense never reaches the GPU as a transform.
        assert!(super::depth_for_scale(0.0).abs() < 1e-9);
        assert!(super::depth_for_scale(-1.0).abs() < 1e-9);
        assert!(super::depth_for_scale(f64::NAN).abs() < 1e-9);

        // ...and a scaled face still turns into the wall the drum names.
        let rail = carousel();
        let seat = rail.seat(STEP);
        let face = rail.scaled_face(seat, Rect::new(0.0, 0.0, 40.0, 40.0), solid(), 0.8);
        assert!(face.pitch.abs() < 1e-9);
        assert!((f64::from(face.yaw) + STEP).abs() < 1e-6);
        assert!(face.depth_offset < 0.0);
        assert!(face.is_renderable());
    }

    /// A negative radius is a degenerate drum, not an inverted one.
    #[test]
    fn a_drum_without_a_radius_stacks_its_faces_at_the_front() {
        let seat = Cylinder3d::new(-10.0, CylinderAxis::Vertical, true).seat(STEP);
        assert!(seat.travel.hypot() < 1e-9);
        assert!(seat.depth.abs() < 1e-9);
        assert!((seat.rotation - STEP).abs() < 1e-9, "the face still turns");
    }

    /// A drum occludes by distance, so its scene carries the depth attachment
    /// and arrives nearest first — which is what stops a translucent wall
    /// blending twice along its seams.
    #[test]
    fn a_drum_scene_is_depth_tested_nearest_first() {
        let drum = wheel();
        let dest = |y: f64| Rect::new(0.0, y, 40.0, y + 20.0);
        // Submitted furthest first, which is the order a component's own item
        // list gives on one side of the centre.
        let scene = drum_scene((0..3).rev().map(|i| {
            drum.face(
                drum.seat(f64::from(i) * STEP),
                dest(f64::from(i) * 20.0),
                solid(),
                240.0,
            )
        }));
        assert!(scene.depth, "a drum occludes by distance");
        assert_eq!(scene.len(), 3);
        for pair in scene.quads.windows(2) {
            assert!(
                pair[0].depth_offset >= pair[1].depth_offset,
                "the nearest face is submitted first"
            );
        }
        assert!(
            (scene.quads[0].dest.y0 - 0.0).abs() < 1e-9,
            "the front face"
        );

        // Equal depths keep the component's own order.
        let flat = drum_scene(
            (0..3).map(|i| drum.face(drum.seat(0.0), dest(f64::from(i) * 20.0), solid(), 240.0)),
        );
        let tops: Vec<f64> = flat.quads.iter().map(|quad| quad.dest.y0).collect();
        assert_eq!(tops, vec![0.0, 20.0, 40.0]);
    }

    /// The drum's half of a pick: the smallest turn from the front wins, ties
    /// go to the earlier index, and a drum of nothing answers nothing.
    #[test]
    fn the_frontmost_item_is_the_one_with_the_smallest_turn() {
        assert_eq!(frontmost([2.0 * STEP, -0.2, STEP]), Some(1));
        assert_eq!(
            frontmost([-STEP, STEP]),
            Some(0),
            "a tie goes to the earlier index"
        );
        assert_eq!(frontmost([f64::NAN, 0.4]), Some(1));
        assert_eq!(frontmost([f64::NAN]), None);
        assert_eq!(frontmost([]), None);
    }
}

/// The drum against real hardware. Ignored by default; see
/// [`crate::gpu_fx::test_gpu`] for the invocation and why a read-back is the
/// only honest check here.
#[cfg(test)]
mod gpu_tests {
    use frust::authoring::{Point, Size};
    use peniko::Color;

    use super::{Cylinder3d, CylinderAxis, drum_scene};
    use crate::gpu_fx::card3d::target_for;
    use crate::gpu_fx::card3d::test_render::{pixel, render};
    use crate::gpu_fx::quad3d::{Quad3dRenderer, QuadFace};
    use crate::gpu_fx::test_gpu::with_device;

    /// The window every case renders through: 200x120 logical px, which
    /// overscans to 280x168 texels.
    const WINDOW: Size = Size::new(200.0, 120.0);

    /// One row of the test drum, tall enough that two adjacent seats overlap
    /// on screen once the drum has turned — which is the only arrangement in
    /// which depth has anything to decide.
    const ROW: Size = Size::new(160.0, 44.0);

    /// The drum's radius: small enough against [`ROW`] that a step of turn
    /// moves a row by less than its own height.
    const RADIUS: f64 = 40.0;

    /// One step of the drum, in radians.
    const STEP: f64 = std::f64::consts::FRAC_PI_6;

    /// The claim the whole 3D drum is worth having for, read off hardware: a
    /// row that has turned onto the far side of the wall is hidden by the row
    /// in front of it **through a whole step of rotation**, even though it is
    /// submitted last — which painter order, the only ordering a 2D path has,
    /// gets backwards at every one of those positions.
    ///
    /// The sample is found by rendering each row alone and intersecting what
    /// they covered, rather than by predicting where the projection puts them:
    /// a hand-computed sample point would be checking this case's arithmetic
    /// instead of the depth attachment.
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn a_rear_row_passes_behind_the_front_one_through_a_whole_step() {
        with_device(|device, queue| {
            let mut renderer = Quad3dRenderer::new(device, "cylinder-occlusion");
            let (_, extent) = target_for(Point::ORIGIN, WINDOW).expect("a window with area");
            let centre = Point::new(f64::from(extent.0) / 2.0, f64::from(extent.1) / 2.0);
            let drum = Cylinder3d::new(RADIUS, CylinderAxis::Horizontal, true);
            let front = Color::from_rgba8(255, 0, 0, 255);
            let rear = Color::from_rgba8(0, 0, 255, 255);

            // Sampled across one step, so the assertion is about the drum
            // turning rather than about one lucky position.
            for tick in 0..=4 {
                let scroll = STEP * f64::from(tick) / 4.0;
                // Row order, not depth order: the row *behind* is submitted
                // last, so only depth can keep it behind.
                let faces = [(front, scroll), (rear, scroll + STEP)].map(|(color, angle)| {
                    let seat = drum.seat(angle);
                    drum.face(
                        seat,
                        drum.seated_rect(centre, ROW, seat),
                        QuadFace::Solid(color),
                        f64::from(extent.1),
                    )
                });
                assert!(
                    faces[1].depth_offset < faces[0].depth_offset,
                    "the second row should be the further one"
                );

                let alone = |renderer: &mut Quad3dRenderer, index: usize| {
                    render(
                        device,
                        queue,
                        renderer,
                        extent,
                        true,
                        &drum_scene([faces[index].clone()]),
                    )
                };
                let near_only = alone(&mut renderer, 0);
                let far_only = alone(&mut renderer, 1);
                let sample = (0..extent.0 * extent.1)
                    .map(|i| (i % extent.0, i / extent.0))
                    .find(|(x, y)| {
                        pixel(&near_only, extent.0, *x, *y)[3] == 255
                            && pixel(&far_only, extent.0, *x, *y)[3] == 255
                    })
                    .expect("two adjacent rows on a drum overlap on screen");

                let occluded = render(
                    device,
                    queue,
                    &mut renderer,
                    extent,
                    true,
                    &drum_scene(faces.clone()),
                );
                let (x, y) = sample;
                let hit = pixel(&occluded, extent.0, x, y);
                println!("cylinder occlusion: tick {tick}, sample ({x}, {y}) -> {hit:?}");
                assert_eq!(
                    hit,
                    [255, 0, 0, 255],
                    "the near row should survive the far one drawn after it"
                );

                let painted = render(
                    device,
                    queue,
                    &mut renderer,
                    extent,
                    false,
                    &drum_scene(faces).with_depth(false),
                );
                assert_eq!(
                    pixel(&painted, extent.0, x, y),
                    [0, 0, 255, 255],
                    "without depth the same order is plain painter order"
                );
            }
        });
    }

    /// A seated face is genuinely turned into the wall, not merely moved: the
    /// edge that rotated toward the camera covers more of its row than the one
    /// that rotated away, which is the foreshortening an affine cannot make.
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn a_seated_face_is_foreshortened_across_the_wall() {
        with_device(|device, queue| {
            let mut renderer = Quad3dRenderer::new(device, "cylinder-foreshortening");
            let (_, extent) = target_for(Point::ORIGIN, WINDOW).expect("a window with area");
            let centre = Point::new(f64::from(extent.0) / 2.0, f64::from(extent.1) / 2.0);
            let rail = Cylinder3d::new(90.0, CylinderAxis::Vertical, true);
            let seat = rail.seat(STEP);
            let face = rail.face(
                seat,
                rail.seated_rect(centre, Size::new(90.0, 90.0), seat),
                QuadFace::Solid(Color::WHITE),
                f64::from(extent.1),
            );
            let frame = render(
                device,
                queue,
                &mut renderer,
                extent,
                true,
                &drum_scene([face]),
            );

            let column = |x: u32| {
                (0..extent.1)
                    .filter(|y| pixel(&frame, extent.0, x, *y)[3] > 0)
                    .count() as u32
            };
            let covered: Vec<u32> = (0..extent.0).filter(|x| column(*x) > 0).collect();
            let (&left, &right) = covered
                .first()
                .zip(covered.last())
                .expect("the seated face rendered nothing");
            let near = (left..(left + 8).min(extent.0))
                .map(column)
                .max()
                .unwrap_or(0);
            let far = (right.saturating_sub(7)..=right)
                .map(column)
                .max()
                .unwrap_or(0);
            println!("cylinder seat: span {left}..={right}, near {near}, far {far}");
            assert!(
                near > far + 4,
                "the edge turned toward the camera ({near}) should cover more than the one turned away ({far})"
            );
        });
    }
}
