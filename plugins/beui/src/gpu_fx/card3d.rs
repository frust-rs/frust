//! Card-shaped 3D: the effect the catalog's tilting and stacking surfaces
//! render their *faces* through, built entirely on [`quad3d`](super::quad3d).
//!
//! Two shapes, one substrate. A **tilt** is one face under pointer-driven
//! yaw/pitch with an optional glare riding on it; a **fan** is a stack of
//! faces separated in depth, the selected one lifted toward the viewer and the
//! rest receding and leaning away. Both are the same three ingredients — a
//! destination rectangle in an overscanned target, a rotation, and a
//! [`QuadFace`] — so they live together rather than in two modules that would
//! each restate the camera contract.
//!
//! # What a component gets from this module, and what it still owns
//!
//! [`Card3d`] is the handle's whole life: acquire on a paint, submit a scene
//! every paint with content, [`Card3d::clear`] when the component drops to its
//! 2D path mid-life, [`Card3d::release`] from `View::teardown`. Everything
//! else here is pure geometry, testable without a device, and that is
//! deliberate — the mapping from a pointer offset to a rotation is the part a
//! port gets wrong, and it should not need a GPU to check.
//!
//! What a component still owns is the 2D path. Acquisition answers `None`
//! whenever the GPU is unreachable, and [`Card3d::acquire`] passes that
//! through unchanged: **the 3D face is an enhancement over a component that
//! already renders correctly without it.**
//!
//! # The overscanned target, and why a face is still pixel-exact at rest
//!
//! [`quad3d`](super::quad3d)'s camera is chosen so an unrotated quad covering
//! the target projects onto exactly the target. Tilt that quad and its near
//! edge grows *past* the target's own bounds, where the pass's viewport
//! scissors it off — a tilted card with a clipped near edge.
//!
//! So a card never fills its target. [`target_for`] allocates a target
//! [`OVERSCAN`] times the card's box, positioned so the composited
//! destination stays centred on the card, and [`in_target`] places the card's
//! own rectangle inside it. One target texel is one logical pixel either way,
//! so the face at zero tilt lands exactly on the card's laid-out box and a
//! tilt has room to grow into. The cost is the margin's texels, which is the
//! price of an unclipped near edge.
//!
//! # Rotation signs
//!
//! Upstream's transforms are CSS `rotateX(rx) rotateY(ry)`. Both map straight
//! onto [`Quad3d`]'s own axes: CSS `rotateY` takes `+X` toward `-Z` and so
//! does [`Quad3d::yaw`] (positive brings the *left* edge toward the camera),
//! and CSS `rotateX` takes `+Y` toward `+Z` and so does [`Quad3d::pitch`]
//! (positive brings the *top* edge toward the camera). [`rotation`] is
//! therefore a degrees-to-radians conversion and nothing more — and the pitch
//! half of that claim is read back off real hardware by
//! `a_pitched_face_lifts_its_top_edge` rather than trusted.
//!
//! # What this cannot carry
//!
//! - **A face is a colour, a ramp or a caller-owned texture — never a widget
//!   subtree.** The substrate-wide boundary, restated here because it is the
//!   one every component built on this module has to explain to its own
//!   caller: a child subtree keeps compositing under an `Affine`.
//! - **A face has square corners.** A rounded clip is a 2D rectangle in scene
//!   space; a tilted face is not, and there is no projective clip to give it
//!   one.
//! - **The glare is a linear ramp, not a radial one.** [`QuadFace`] offers no
//!   radial gradient, so [`glare_quad`] aims a linear ramp *at* the pointer
//!   instead of centring a disc on it. The bright end tracks the pointer; the
//!   circular falloff does not survive.
//! - **The camera is the substrate's, not the page's.** CSS
//!   `perspective(1000px)` is an absolute distance; this camera sits
//!   [`CAMERA_DISTANCE`](super::quad3d::CAMERA_DISTANCE) *target heights*
//!   away, so the projection is somewhat stronger than upstream's on a card a
//!   few hundred pixels tall. That is the substrate's tuning, not a knob a
//!   component adjusts.

use std::sync::Arc;

use frust::authoring::{Point, Rect, Size, Vec2};
use peniko::Color;

use crate::style;

use super::context::{GpuFx, GpuFxHandle};
use super::pool::MAX_TARGET_SIDE;
use super::quad3d::{Quad3d, Quad3dScene, QuadFace};
use super::schedule::FxCadence;

/// How much larger than its card a 3D target is allocated, so a tilted face's
/// near edge has room to grow rather than being scissored at the target's
/// edge. See the [module docs](self).
///
/// `1.4` gives a fifth of the card's extent as margin on each side. A face
/// rotated by `θ` magnifies its near edge by
/// `CAMERA_DISTANCE / (CAMERA_DISTANCE − sin θ · half-extent)`, so this margin
/// clears a tilt of roughly 30 degrees — several times the catalog's own
/// default travel, and past that the near edge is scissored at the target's
/// edge rather than rendering wrong.
pub const OVERSCAN: f64 = 1.4;

/// How far in front of its face the glare quad sits, in world units.
///
/// Small enough to be invisible as a displacement and large enough that a
/// depth-tested scene puts the glare in front of the face it belongs to
/// rather than leaving the pair to a `LessEqual` tie.
pub const GLARE_LIFT: f32 = 0.001;

/// Below this pointer offset the glare has no direction to aim along, and
/// falls back to [`GLARE_RESTING_ANGLE`].
pub const GLARE_DEADZONE: f64 = 0.02;

/// The glare's ramp direction with the pointer at the card's centre: bright
/// end at the top, which reads as an ordinary overhead light rather than as an
/// arbitrary diagonal.
pub const GLARE_RESTING_ANGLE: f32 = -std::f32::consts::FRAC_PI_2;

/// How far each step away from the selected card leans, in degrees.
pub const FAN_PITCH: f64 = 9.0;

/// How far the selected card lifts toward the viewer, in world units.
///
/// A lift of `l` magnifies a card by `CAMERA_DISTANCE / (CAMERA_DISTANCE − l)`,
/// so `0.08` grows the selected card by about four and a half percent — enough
/// that adjacent cards in a list genuinely overlap and the depth attachment
/// has something to resolve, and little enough that the lifted card does not
/// burst out of the box it was laid out in.
pub const FAN_LIFT: f32 = 0.08;

/// How far each step away from the selected card recedes, in world units.
///
/// Deliberately equal to [`FAN_LIFT`]: one step back from the selected card is
/// exactly the lift undone, so the cards immediately either side of it sit at
/// `z = 0` and render at precisely the size they were laid out, and only the
/// ones beyond them recede. That makes the effect read as *one card lifting*
/// out of a flat run rather than as a whole list changing size.
pub const FAN_STEP: f32 = FAN_LIFT;

/// How much wider an elevation shadow spreads at full tilt once the face is
/// genuinely projected — the depth cue a flat card has to fake with alpha.
pub const SHADOW_SOFTENING: f64 = 1.75;

/// One component's 3D card path: the handle, and the label it is diagnosed
/// under.
///
/// Deliberately not `Default`: a handle is claimed under a name, and an
/// unnamed pass is unattributable in a validation log.
#[derive(Debug)]
pub struct Card3d {
    label: &'static str,
    handle: Option<GpuFxHandle>,
}

impl Card3d {
    /// A path that has not claimed anything yet.
    ///
    /// `label` names this component in adapter and validation diagnostics — a
    /// short static string like `"tilt-card"`, never per-instance text.
    #[must_use]
    pub const fn new(label: &'static str) -> Self {
        Self {
            label,
            handle: None,
        }
    }

    /// Claims the 3D path if it is available and not already held, answering
    /// whether it is live.
    ///
    /// Safe to call every paint: a held handle is kept (re-acquiring would
    /// mint a second id and leak a second pass), and a refusal is retried,
    /// which is how a component picks the path up once a shell's first
    /// surface exists.
    pub fn acquire(&mut self) -> bool {
        if self.handle.is_none() {
            self.handle = GpuFx::try_acquire(self.label);
        }
        self.handle.is_some()
    }

    /// Whether the 3D path is currently held.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.handle.is_some()
    }

    /// The id a display list names to composite this component's output, or
    /// `None` while the path is not held.
    #[must_use]
    pub fn scene_texture_id(&self) -> Option<u64> {
        self.handle.as_ref().map(GpuFxHandle::scene_texture_id)
    }

    /// Hands `scene` to the pass, answering whether this was the pass's
    /// *first* content.
    ///
    /// That answer is the one frame a component owes: a texture is bound
    /// during the frame after the paint that submitted it, so the paint that
    /// first submits composites an unbound id and draws nothing. Asking for a
    /// frame on a `true` is what makes the face appear rather than waiting
    /// for the next unrelated repaint.
    pub fn submit(&self, extent: (u32, u32), scene: Quad3dScene) -> bool {
        let Some(handle) = self.handle.as_ref() else {
            return false;
        };
        let first = !handle.has_content();
        handle.submit(extent, scene);
        first
    }

    /// Withdraws the content without giving up the claim — what a component
    /// calls when it drops to its 2D path mid-life.
    pub fn clear(&self) {
        if let Some(handle) = self.handle.as_ref() {
            handle.clear();
        }
    }

    /// Withdraws the registration. Call from `View::teardown`; a handle
    /// dropped without it leaves its pass called every frame forever.
    pub fn release(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.release();
        }
    }
}

/// The overscanned target a card of `size` at `origin` renders through: the
/// destination rectangle to composite in the scene's own space, and the
/// target's extent in texels.
///
/// `None` for a card with no area, a non-finite box, or one whose overscanned
/// extent would exceed [`MAX_TARGET_SIDE`] — all three are the component's
/// signal to paint 2D, since a clamped target would map the destination onto
/// the wrong texels rather than merely rendering something smaller.
#[must_use]
pub fn target_for(origin: Point, size: Size) -> Option<(Rect, (u32, u32))> {
    if !(size.width.is_finite() && size.height.is_finite())
        || !(origin.x.is_finite() && origin.y.is_finite())
        || size.width <= 0.0
        || size.height <= 0.0
    {
        return None;
    }
    let width = (size.width * OVERSCAN).ceil();
    let height = (size.height * OVERSCAN).ceil();
    if width > f64::from(MAX_TARGET_SIDE) || height > f64::from(MAX_TARGET_SIDE) {
        return None;
    }
    let centre = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    Some((
        Rect::from_center_size(centre, Size::new(width, height)),
        (width as u32, height as u32),
    ))
}

/// Maps `rect`, given in the card's own space (origin at the card's top-left),
/// onto the overscanned target's texel space.
///
/// One texel per logical pixel, so a rectangle that was pixel-exact in the
/// card's space stays pixel-exact in the target's.
#[must_use]
pub fn in_target(extent: (u32, u32), size: Size, rect: Rect) -> Rect {
    rect + Vec2::new(
        (f64::from(extent.0) - size.width) / 2.0,
        (f64::from(extent.1) - size.height) / 2.0,
    )
}

/// The card's whole box inside its own overscanned target — [`in_target`] of
/// the card itself, the destination a single-face tilt uses.
#[must_use]
pub fn face_rect(extent: (u32, u32), size: Size) -> Rect {
    in_target(extent, size, Rect::from_origin_size(Point::ORIGIN, size))
}

/// `(yaw, pitch)` in radians for upstream's `rotateX(rx) rotateY(ry)` in
/// degrees. See the [module docs](self) for why neither sign flips.
#[must_use]
pub fn rotation(rx_degrees: f64, ry_degrees: f64) -> (f32, f32) {
    (
        ry_degrees.to_radians() as f32,
        rx_degrees.to_radians() as f32,
    )
}

/// One card face at `dest`, rotated by upstream's `rx`/`ry` in degrees.
#[must_use]
pub fn face_quad(dest: Rect, face: QuadFace, rx_degrees: f64, ry_degrees: f64) -> Quad3d {
    let (yaw, pitch) = rotation(rx_degrees, ry_degrees);
    Quad3d::new(dest, face).yaw(yaw).pitch(pitch)
}

/// The glare that rides on a tilted face: a linear ramp from transparent to
/// `ink` at `opacity`, aimed along `offset`.
///
/// `offset` is the pointer's position as a `-1..1` fraction of the card from
/// its centre — exactly what `PointerTracker::offset` produces — with `+y`
/// downward. The bright end of the ramp sits on the pointer's side; with the
/// pointer at the centre there is no direction to aim along and the ramp
/// falls back to [`GLARE_RESTING_ANGLE`].
#[must_use]
pub fn glare_quad(
    dest: Rect,
    rx_degrees: f64,
    ry_degrees: f64,
    offset: Vec2,
    ink: Color,
    opacity: f32,
) -> Quad3d {
    let angle = if offset.hypot() > GLARE_DEADZONE {
        offset.y.atan2(offset.x) as f32
    } else {
        GLARE_RESTING_ANGLE
    };
    let (yaw, pitch) = rotation(rx_degrees, ry_degrees);
    Quad3d::new(
        dest,
        QuadFace::Gradient {
            from: style::with_alpha(ink, 0.0),
            to: style::with_alpha(ink, opacity.clamp(0.0, 1.0)),
            angle_radians: angle,
        },
    )
    .yaw(yaw)
    .pitch(pitch)
    .depth_offset(GLARE_LIFT)
}

/// A tilting card's whole scene: its face, and its glare when it has one.
///
/// Depth stays off — one face and a coplanar highlight have nothing to
/// occlude, and the glare is translucent, which is exactly the case the
/// substrate's depth notes warn against.
#[must_use]
pub fn tilt_scene(face: Quad3d, glare: Option<Quad3d>) -> Quad3dScene {
    let mut scene = Quad3dScene::new().with(face);
    if let Some(glare) = glare {
        scene.push(glare);
    }
    scene
}

/// One card in a depth-fanned stack.
#[derive(Clone, Debug)]
pub struct FanCard {
    /// Where the card sits, in the target's texel space, before it is fanned.
    pub dest: Rect,
    /// What fills it.
    pub face: QuadFace,
    /// Where it sits relative to the selected card: `0` is the selected one,
    /// negative is ahead of it in the list and positive behind it.
    pub slot: i32,
}

/// A stack of cards fanned in depth: the selected one lifted toward the
/// viewer, its neighbours receding by [`FAN_STEP`] a step and leaning away by
/// [`FAN_PITCH`] a step, so the run reads as a curve bending away at both
/// ends.
///
/// Depth is **on**, and the cards are submitted in the caller's own order
/// rather than sorted back-to-front: that is the whole point of the mode —
/// a card behind another is hidden by distance, not by when it was drawn.
#[must_use]
pub fn fan_scene(cards: impl IntoIterator<Item = FanCard>) -> Quad3dScene {
    let mut scene = Quad3dScene::new().with_depth(true);
    for card in cards {
        let steps = f64::from(card.slot);
        let depth = FAN_LIFT - card.slot.unsigned_abs() as f32 * FAN_STEP;
        scene.push(face_quad(card.dest, card.face, steps * FAN_PITCH, 0.0).depth_offset(depth));
    }
    scene
}

/// The blur an elevation shadow takes under a genuinely projected face at
/// `lean` (`0..1`): a card whose far edge has rotated into depth throws a
/// softer shadow than the flat one a 2D tilt casts.
#[must_use]
pub fn shadow_blur(base: f64, lean: f64) -> f64 {
    base * (1.0 + lean.clamp(0.0, 1.0) * (SHADOW_SOFTENING - 1.0))
}

/// What the frame loop is asked for after a 3D paint.
///
/// A settled card asks for nothing: the pass keeps its last scene, so the
/// composite survives an idle surface rather than blinking out.
#[must_use]
pub const fn cadence(animating: bool) -> FxCadence {
    if animating {
        FxCadence::Animating
    } else {
        FxCadence::Still
    }
}

/// Whether two faces would render identically — the equality [`QuadFace`]
/// cannot derive, because a texture view is an `Arc` with no meaningful
/// value equality.
///
/// A component's `rebuild` needs this to tell a re-declared identical face
/// from a genuinely changed one; textures compare by identity, which is the
/// only answer available and the right one for a caller that keeps its view.
#[must_use]
pub fn same_face(a: &QuadFace, b: &QuadFace) -> bool {
    match (a, b) {
        (QuadFace::Solid(a), QuadFace::Solid(b)) => a == b,
        (
            QuadFace::Gradient {
                from: a_from,
                to: a_to,
                angle_radians: a_angle,
            },
            QuadFace::Gradient {
                from: b_from,
                to: b_to,
                angle_radians: b_angle,
            },
        ) => a_from == b_from && a_to == b_to && a_angle.to_bits() == b_angle.to_bits(),
        (
            QuadFace::Texture {
                view: a_view,
                opacity: a_opacity,
            },
            QuadFace::Texture {
                view: b_view,
                opacity: b_opacity,
            },
        ) => Arc::ptr_eq(a_view, b_view) && a_opacity.to_bits() == b_opacity.to_bits(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Card3d, FAN_LIFT, FAN_PITCH, FAN_STEP, GLARE_DEADZONE, GLARE_RESTING_ANGLE, OVERSCAN,
        SHADOW_SOFTENING, face_quad, face_rect, fan_scene, glare_quad, in_target, rotation,
        same_face, shadow_blur, target_for, tilt_scene,
    };
    use crate::gpu_fx::quad3d::QuadFace;
    use crate::gpu_fx::schedule::FxCadence;
    use frust::authoring::{Point, Rect, Size, Vec2};
    use peniko::Color;

    const CARD: Size = Size::new(200.0, 100.0);

    fn solid() -> QuadFace {
        QuadFace::Solid(Color::WHITE)
    }

    /// The property the whole overscan exists for: the target is bigger than
    /// the card, the composited destination stays centred on the card, and one
    /// target texel is one logical pixel — so the face at rest lands exactly on
    /// the card's own box.
    #[test]
    fn a_target_is_overscanned_about_the_cards_own_box() {
        let origin = Point::new(30.0, 12.0);
        let (dest, extent) = target_for(origin, CARD).expect("a card with area has a target");

        assert_eq!(extent, (280, 140));
        assert!((dest.width() - f64::from(extent.0)).abs() < 1e-9);
        assert!((dest.height() - f64::from(extent.1)).abs() < 1e-9);
        assert!(dest.width() > CARD.width * (OVERSCAN - 1e-9));

        let card_centre = Point::new(origin.x + CARD.width / 2.0, origin.y + CARD.height / 2.0);
        assert!((dest.center() - card_centre).hypot() < 1e-9);

        // ...and the card's own rectangle inside that target is card-sized and
        // centred, which is what makes the zero-tilt face pixel-exact.
        let face = face_rect(extent, CARD);
        assert!((face.width() - CARD.width).abs() < 1e-9);
        assert!((face.height() - CARD.height).abs() < 1e-9);
        assert!((face.x0 - 40.0).abs() < 1e-9);
        assert!((face.y0 - 20.0).abs() < 1e-9);
    }

    /// A rectangle in the card's space keeps its size and its offset from the
    /// card's own top-left when it lands in the target.
    #[test]
    fn a_card_space_rectangle_keeps_its_offset_in_the_target() {
        let (_, extent) = target_for(Point::ORIGIN, CARD).expect("a target");
        let row = Rect::new(10.0, 20.0, 60.0, 44.0);
        let mapped = in_target(extent, CARD, row);
        let face = face_rect(extent, CARD);
        assert!((mapped.x0 - face.x0 - 10.0).abs() < 1e-9);
        assert!((mapped.y0 - face.y0 - 20.0).abs() < 1e-9);
        assert!((mapped.width() - row.width()).abs() < 1e-9);
    }

    /// Every degenerate box answers `None`, which is the component's own
    /// signal to paint 2D rather than a case it has to special-case.
    #[test]
    fn a_card_without_a_usable_box_has_no_target() {
        assert!(target_for(Point::ORIGIN, Size::ZERO).is_none());
        assert!(target_for(Point::ORIGIN, Size::new(-5.0, 10.0)).is_none());
        assert!(target_for(Point::ORIGIN, Size::new(f64::NAN, 10.0)).is_none());
        assert!(target_for(Point::new(f64::INFINITY, 0.0), CARD).is_none());
        // Larger than the pool will ever allocate: a clamped target would map
        // the destination onto the wrong texels, so the 3D path declines.
        assert!(target_for(Point::ORIGIN, Size::new(8_000.0, 100.0)).is_none());
    }

    /// Upstream's `rotateX`/`rotateY` degrees pass onto the substrate's own
    /// axes with no sign flip and no reordering — the mapping the whole port
    /// hangs on.
    #[test]
    fn the_rotation_mapping_carries_upstreams_own_signs() {
        let (yaw, pitch) = rotation(12.0, -6.0);
        assert!((f64::from(pitch) - 12.0_f64.to_radians()).abs() < 1e-6);
        assert!((f64::from(yaw) + 6.0_f64.to_radians()).abs() < 1e-6);

        // A pointer to the right of the centre asks for a positive `ry`, which
        // brings the card's left edge forward.
        let quad = face_quad(Rect::new(0.0, 0.0, 10.0, 10.0), solid(), 0.0, 12.0);
        assert!(quad.yaw > 0.0);
        assert!(quad.pitch.abs() < 1e-9);
        assert!(quad.is_renderable());
    }

    /// The glare aims at the pointer, and recentres onto a plain overhead ramp
    /// when there is no direction to aim along.
    #[test]
    fn the_glare_ramps_toward_the_pointer() {
        let dest = Rect::new(0.0, 0.0, 100.0, 50.0);
        let ink = Color::WHITE;

        let quad = glare_quad(dest, 0.0, 0.0, Vec2::new(1.0, 0.0), ink, 0.15);
        let QuadFace::Gradient {
            from,
            to,
            angle_radians,
        } = &quad.face
        else {
            panic!("the glare is a ramp");
        };
        assert!(angle_radians.abs() < 1e-6, "a right-hand pointer runs +X");
        assert!(from.components[3] < 1e-6, "the ramp starts clear");
        assert!((to.components[3] - 0.15).abs() < 1e-6);

        let down = glare_quad(dest, 0.0, 0.0, Vec2::new(0.0, 1.0), ink, 0.15);
        let QuadFace::Gradient { angle_radians, .. } = &down.face else {
            panic!("the glare is a ramp");
        };
        assert!((angle_radians - std::f32::consts::FRAC_PI_2).abs() < 1e-6);

        let resting = glare_quad(
            dest,
            0.0,
            0.0,
            Vec2::new(0.0, GLARE_DEADZONE / 2.0),
            ink,
            0.15,
        );
        let QuadFace::Gradient { angle_radians, .. } = &resting.face else {
            panic!("the glare is a ramp");
        };
        assert!((angle_radians - GLARE_RESTING_ANGLE).abs() < 1e-6);

        // It sits in front of the face it belongs to, and a tilt carries it.
        let tilted = glare_quad(dest, 8.0, 4.0, Vec2::new(0.5, 0.5), ink, 0.15);
        assert!(tilted.depth_offset > 0.0);
        assert!(tilted.pitch > 0.0 && tilted.yaw > 0.0);
    }

    /// A tilt scene is depth-free, and the glare is optional and painted after
    /// the face.
    #[test]
    fn a_tilt_scene_is_a_face_and_an_optional_glare() {
        let dest = Rect::new(0.0, 0.0, 40.0, 20.0);
        let bare = tilt_scene(face_quad(dest, solid(), 0.0, 0.0), None);
        assert_eq!(bare.len(), 1);
        assert!(!bare.depth, "one face has nothing to occlude");

        let lit = tilt_scene(
            face_quad(dest, solid(), 0.0, 0.0),
            Some(glare_quad(
                dest,
                0.0,
                0.0,
                Vec2::new(1.0, 0.0),
                Color::WHITE,
                0.15,
            )),
        );
        assert_eq!(lit.len(), 2);
        assert!(matches!(lit.quads[0].face, QuadFace::Solid(_)));
        assert!(matches!(lit.quads[1].face, QuadFace::Gradient { .. }));
    }

    /// The selected card is nearest, its neighbours recede a step at a time,
    /// and the run leans away at both ends — a curve, not a shear.
    #[test]
    fn a_fan_lifts_the_selected_card_and_leans_its_neighbours_away() {
        let dest = Rect::new(0.0, 0.0, 60.0, 24.0);
        let scene = fan_scene((-2..=2).map(|slot| super::FanCard {
            dest,
            face: solid(),
            slot,
        }));

        assert!(scene.depth, "a stack occludes by distance");
        assert_eq!(scene.len(), 5);

        let depths: Vec<f32> = scene.quads.iter().map(|quad| quad.depth_offset).collect();
        assert!(
            (depths[2] - FAN_LIFT).abs() < 1e-6,
            "the selected card lifts"
        );
        assert!(depths[1] < depths[2] && depths[3] < depths[2]);
        assert!((depths[1] - depths[0] - FAN_STEP).abs() < 1e-6);
        assert!((depths[1] - depths[3]).abs() < 1e-6, "the fan is symmetric");

        // A card ahead of the selected one leans its top away; one behind it
        // leans its bottom away. Positive pitch brings the top forward, so the
        // signs mirror about the selected card.
        assert!(scene.quads[2].pitch.abs() < 1e-9);
        assert!(scene.quads[1].pitch < 0.0);
        assert!(scene.quads[3].pitch > 0.0);
        assert!(
            (f64::from(scene.quads[3].pitch) - FAN_PITCH.to_radians()).abs() < 1e-6,
            "one step is one FAN_PITCH"
        );
        assert!(scene.quads.iter().all(super::Quad3d::is_renderable));
    }

    #[test]
    fn a_shadow_softens_with_the_lean() {
        assert!((shadow_blur(24.0, 0.0) - 24.0).abs() < 1e-9);
        assert!((shadow_blur(24.0, 1.0) - 24.0 * SHADOW_SOFTENING).abs() < 1e-9);
        assert!(shadow_blur(24.0, 0.5) > shadow_blur(24.0, 0.25));
        // A lean outside its own range never produces a negative blur.
        assert!((shadow_blur(24.0, -3.0) - 24.0).abs() < 1e-9);
        assert!((shadow_blur(24.0, 9.0) - 24.0 * SHADOW_SOFTENING).abs() < 1e-9);
    }

    /// The fallback contract, from the component's side: no shell installs a
    /// device in a test process, so acquisition refuses, every other call is a
    /// no-op, and nothing panics. This is the path every 3D component in this
    /// catalog actually renders through in its own unit tests.
    #[test]
    fn a_path_without_a_device_stays_inert() {
        let mut card = Card3d::new("test");
        assert!(!card.acquire(), "a test process has no shell device");
        assert!(!card.is_active());
        assert!(card.scene_texture_id().is_none());
        assert!(
            !card.submit(
                (64, 64),
                tilt_scene(
                    face_quad(Rect::new(0.0, 0.0, 8.0, 8.0), solid(), 0.0, 0.0),
                    None
                )
            ),
            "an unheld path owes no frame"
        );
        card.clear();
        card.release();
        assert!(!card.is_active());
    }

    /// A settled card asks for nothing, which is what lets a surface holding a
    /// composited face go idle.
    #[test]
    fn only_a_moving_card_asks_for_a_frame() {
        assert_eq!(super::cadence(true), FxCadence::Animating);
        assert_eq!(super::cadence(false), FxCadence::Still);
    }

    /// Face equality is what a `rebuild` tells a re-declared identical face
    /// from a changed one with; textures compare by identity because nothing
    /// else is available.
    #[test]
    fn faces_compare_by_value_and_textures_by_identity() {
        assert!(same_face(&solid(), &QuadFace::Solid(Color::WHITE)));
        assert!(!same_face(&solid(), &QuadFace::Solid(Color::BLACK)));

        let ramp = |angle: f32| QuadFace::Gradient {
            from: Color::WHITE,
            to: Color::BLACK,
            angle_radians: angle,
        };
        assert!(same_face(&ramp(0.5), &ramp(0.5)));
        assert!(!same_face(&ramp(0.5), &ramp(0.6)));
        assert!(!same_face(&solid(), &ramp(0.5)));
    }
}

/// The offscreen render every GPU case built on this module runs — the card
/// cases below, and the components that fan and tilt through it.
///
/// Shared rather than restated per component: a case that allocated its target
/// differently from the pass would be checking its own harness rather than the
/// geometry.
#[cfg(test)]
pub(crate) mod test_render {
    use frust::gpu::wgpu;

    use crate::gpu_fx::quad3d::{
        COLOR_FORMAT, DEPTH_FORMAT, Quad3dRenderer, Quad3dScene, Quad3dTarget,
    };
    use crate::gpu_fx::test_gpu::read_back;

    /// Renders `scene` into a fresh target of `extent` and hands back its
    /// texels, tightly packed RGBA8.
    pub(crate) fn render(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut Quad3dRenderer,
        extent: (u32, u32),
        depth: bool,
        scene: &Quad3dScene,
    ) -> Vec<u8> {
        let (width, height) = extent;
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frust-beui card3d case colour"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth.then(|| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("frust-beui card3d case depth"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: DEPTH_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-beui card3d case"),
        });
        renderer.record(
            device,
            queue,
            &mut encoder,
            Quad3dTarget {
                color: &view,
                depth: depth_view.as_ref(),
                requested: extent,
            },
            scene,
        );
        queue.submit([encoder.finish()]);
        read_back(device, queue, &texture, width, height)
    }

    /// The RGBA at `(x, y)` of a `width`-wide frame.
    pub(crate) fn pixel(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * width + x) * 4) as usize;
        frame[start..start + 4]
            .try_into()
            .expect("a read-back pixel is four bytes")
    }
}

/// The card modes against real hardware. Ignored by default; see
/// [`crate::gpu_fx::test_gpu`] for the invocation and why a read-back is the
/// only honest check here.
#[cfg(test)]
mod gpu_tests {
    use frust::authoring::{Point, Rect, Size, Vec2};
    use peniko::Color;

    use super::test_render::{pixel, render};
    use super::{FanCard, face_quad, face_rect, fan_scene, glare_quad, target_for, tilt_scene};
    use crate::gpu_fx::quad3d::{Quad3dRenderer, QuadFace};
    use crate::gpu_fx::test_gpu::with_device;

    /// The card every case renders: 160x80 logical px, which overscans to
    /// 224x112 texels.
    const CARD: Size = Size::new(160.0, 80.0);

    /// How many texels of column `x` the render covered at all.
    fn column_coverage(frame: &[u8], extent: (u32, u32), x: u32) -> u32 {
        (0..extent.1)
            .filter(|y| pixel(frame, extent.0, x, *y)[3] > 0)
            .count() as u32
    }

    /// How many texels of row `y` the render covered at all.
    fn row_coverage(frame: &[u8], extent: (u32, u32), y: u32) -> u32 {
        (0..extent.0)
            .filter(|x| pixel(frame, extent.0, *x, y)[3] > 0)
            .count() as u32
    }

    /// The claim the whole 3D path is worth having for: a yawed card face is
    /// *projected*, so its near edge covers markedly more of its column than
    /// its far edge — which an `Affine` tilt, whatever its shear, cannot do.
    ///
    /// The zero-tilt control in the same case is the other half: an untilted
    /// face lands exactly on the card's own box inside the overscanned target,
    /// so switching the 3D path on at rest changes nothing.
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn a_tilted_face_is_foreshortened_where_a_flat_one_is_exact() {
        with_device(|device, queue| {
            let mut renderer = Quad3dRenderer::new(device, "card3d-tilt");
            let (_, extent) = target_for(Point::ORIGIN, CARD).expect("a card with area");
            let dest = face_rect(extent, CARD);
            let face = || QuadFace::Solid(Color::WHITE);

            let flat = render(
                device,
                queue,
                &mut renderer,
                extent,
                false,
                &tilt_scene(face_quad(dest, face(), 0.0, 0.0), None),
            );
            let inside = dest.x0 as u32 + 4;
            assert_eq!(
                column_coverage(&flat, extent, inside),
                CARD.height as u32,
                "an untilted face covers exactly the card's own height"
            );
            assert_eq!(
                column_coverage(&flat, extent, 1),
                0,
                "the overscan margin stays empty at rest"
            );

            let yawed = render(
                device,
                queue,
                &mut renderer,
                extent,
                false,
                &tilt_scene(face_quad(dest, face(), 0.0, 20.0), None),
            );
            // Sampled against the *projected* span, not the resting rectangle:
            // a foreshortened far edge has moved inward, so a column measured
            // where the untilted face used to end is simply empty.
            let covered: Vec<u32> = (0..extent.0)
                .filter(|x| column_coverage(&yawed, extent, *x) > 0)
                .collect();
            let (&left, &right) = covered
                .first()
                .zip(covered.last())
                .expect("the tilted face rendered nothing");
            let near = (left..left + 10)
                .map(|x| column_coverage(&yawed, extent, x))
                .max()
                .unwrap_or(0);
            let far = (right - 9..=right)
                .map(|x| column_coverage(&yawed, extent, x))
                .max()
                .unwrap_or(0);
            println!("card3d tilt: span {left}..={right}, near column {near}, far column {far}");
            assert!(
                near > far + 4,
                "the near edge ({near}) should cover markedly more than the far edge ({far})"
            );
            // The near edge grew *past* the card's resting box, which is the
            // overscan earning its texels and the one thing an affine shear
            // never does.
            assert!(
                f64::from(left) < dest.x0,
                "the near edge ({left}) should reach past the resting box ({})",
                dest.x0
            );
            assert!(
                f64::from(right) < dest.x1,
                "the far edge ({right}) should have pulled inside the resting box ({})",
                dest.x1
            );
        });
    }

    /// Pins the pitch sign the whole fan geometry is built on: a positive
    /// pitch brings the *top* edge toward the camera, so the top of the face
    /// covers more of its row than the bottom does.
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn a_pitched_face_lifts_its_top_edge() {
        with_device(|device, queue| {
            let mut renderer = Quad3dRenderer::new(device, "card3d-pitch");
            let (_, extent) = target_for(Point::ORIGIN, CARD).expect("a card with area");
            let dest = face_rect(extent, CARD);

            let pitched = render(
                device,
                queue,
                &mut renderer,
                extent,
                false,
                &tilt_scene(
                    face_quad(dest, QuadFace::Solid(Color::WHITE), 30.0, 0.0),
                    None,
                ),
            );
            let top = (dest.y0 as u32 + 2..dest.y0 as u32 + 10)
                .map(|y| row_coverage(&pitched, extent, y))
                .max()
                .unwrap_or(0);
            let bottom = (dest.y1 as u32 - 10..dest.y1 as u32 - 2)
                .map(|y| row_coverage(&pitched, extent, y))
                .max()
                .unwrap_or(0);
            println!("card3d pitch: top row {top}, bottom row {bottom}");
            assert!(top > 0 && bottom > 0, "the pitched face rendered nothing");
            assert!(
                top > bottom + 4,
                "a positive pitch should bring the top edge ({top}) nearer than the bottom ({bottom})"
            );
        });
    }

    /// The wallet fan's own claim: the selected card is in front, and stays in
    /// front of a neighbour drawn *after* it. The depth-free control in the
    /// same case is the negative half — without the attachment the same order
    /// is plain painter order and the later card wins.
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn a_fan_occludes_back_to_front_not_in_paint_order() {
        with_device(|device, queue| {
            let mut renderer = Quad3dRenderer::new(device, "card3d-fan");
            let (_, extent) = target_for(Point::ORIGIN, CARD).expect("a card with area");
            let row = Rect::from_origin_size(Point::new(20.0, 20.0), Size::new(120.0, 40.0));
            let dest = super::in_target(extent, CARD, row);
            let ahead = Color::from_rgba8(0, 255, 0, 255);
            let selected = Color::from_rgba8(255, 0, 0, 255);
            let behind = Color::from_rgba8(0, 0, 255, 255);

            // Row order, not depth order: the card *behind* the selected one is
            // submitted last, so only depth can keep it behind.
            let stack = || {
                [(ahead, -1), (selected, 0), (behind, 1)].map(|(color, slot)| FanCard {
                    dest,
                    face: QuadFace::Solid(color),
                    slot,
                })
            };

            let centre = (dest.center().x as u32, dest.center().y as u32);
            let occluded = render(
                device,
                queue,
                &mut renderer,
                extent,
                true,
                &fan_scene(stack()),
            );
            assert_eq!(
                pixel(&occluded, extent.0, centre.0, centre.1),
                [255, 0, 0, 255],
                "the selected card survives the one behind it drawn after it"
            );

            let painted = render(
                device,
                queue,
                &mut renderer,
                extent,
                false,
                &fan_scene(stack()).with_depth(false),
            );
            assert_eq!(
                pixel(&painted, extent.0, centre.0, centre.1),
                [0, 0, 255, 255],
                "without depth the same order is plain painter order"
            );
        });
    }

    /// The glare is a real ramp on the face: the pointer's side is brighter
    /// than the far side, and it travels with the tilt rather than staying
    /// square to the screen.
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn a_glare_brightens_the_pointers_side_of_the_face() {
        with_device(|device, queue| {
            let mut renderer = Quad3dRenderer::new(device, "card3d-glare");
            let (_, extent) = target_for(Point::ORIGIN, CARD).expect("a card with area");
            let dest = face_rect(extent, CARD);
            let scene = tilt_scene(
                face_quad(dest, QuadFace::Solid(Color::BLACK), 0.0, 0.0),
                Some(glare_quad(
                    dest,
                    0.0,
                    0.0,
                    Vec2::new(1.0, 0.0),
                    Color::WHITE,
                    0.5,
                )),
            );
            let frame = render(device, queue, &mut renderer, extent, false, &scene);
            let y = dest.center().y as u32;
            let left = pixel(&frame, extent.0, dest.x0 as u32 + 4, y);
            let right = pixel(&frame, extent.0, dest.x1 as u32 - 4, y);
            println!("card3d glare: left {left:?}, right {right:?}");
            assert!(
                right[0] > left[0] + 8,
                "the pointer's side ({right:?}) should be brighter than the far side ({left:?})"
            );
        });
    }
}
