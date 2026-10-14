//! Opacity layers, snapshot brackets, and the bracket stack they share with
//! clips.
//!
//! Three of the display list's commands open a *group* — a bracket whose
//! matching pop has to undo exactly what it did, and inside which a hoisted
//! clear is confined (see [`clear`](super::clear)). [`GroupStack`] is that one
//! stack. It is deliberately one stack and not three: `frust_scene` records
//! `PopClip`, `PopLayer` and `PopRetained` as distinct commands, but a widget
//! tree can emit them unbalanced or interleaved, and three stacks would let one
//! kind of pop lift a bracket another kind still relies on. One stack means the
//! innermost open bracket is always what closes, whichever pop closes it.
//!
//! ## Opacity layers
//!
//! A `PushLayer` at `alpha >= 1.0` composites source-over at full opacity,
//! which is precisely what drawing its contents into the parent does — so it
//! lowers to nothing but its rectangle's clip, exactly as `PushClip` does
//! (`frust_scene` documents the two as the same command with `alpha` fixed).
//! Below full opacity the layer has to be rendered in isolation and composited
//! as a whole, which is a page and a pass in the scheduler
//! ([`schedule`](crate::schedule)) — so it lowers to a recorded layer *plus*
//! that same rectangular clip.
//!
//! The rectangle goes through the clip stack rather than through
//! [`LayerProps::clip_path`]: frust lowers every clip to a scissor or a
//! coverage mask and never to an intermediate texture, and the scheduler
//! refuses a recorded layer carrying a clip path for that exact reason. Setting
//! both would clip the layer twice.
//!
//! ## Snapshot brackets
//!
//! `frust_scene` states what a renderer that does not retain must do with a
//! `PushRetained` bracket: paint the body inline, wrapped exactly as if the
//! recorder had emitted `push_transform(scale_about(scale, rect.center()))`
//! when `scale != 1.0` and `push_layer(rect, alpha)` when `alpha < 1.0`.
//! [`RetainedStack`] is that emulation, and it is also the whole of what a
//! compiler with retention off does (see `SceneCompiler::set_retained_layers`).
//!
//! A compiler with retention on lowers the outermost bracket the same way and
//! additionally records it as a [`RetainedBracket`]: the body becomes a
//! recorded layer of its own *whatever* its alpha — at full opacity one with no
//! clip of its own ([`GroupStack::push_retained`]), so the inline lowering's
//! geometry is unchanged — which the renderer's retained-page store
//! (`crate::cache::retained`) either admits, rendering the layer into a page it
//! keeps across frames, or refuses, in which case the scheduler serves it as
//! the ordinary layer the inline emulation describes. A `DrawRetained` becomes
//! an empty recorded layer standing at its painter-order position, which the
//! store resolves to a composite of the resident page. The page holds the body
//! *with* the presentation scale baked in, exactly as the inline lowering
//! rasterizes it, and *without* the alpha, so a page is reusable under any
//! alpha and any whole-pixel move of the same transform ([`RetainedPlacement`]).
//!
//! The scale is applied as a *correction* composed ahead of every subsequent
//! command's own transform rather than by rewriting the body's commands,
//! because the body was recorded in the ordinary composed transform space: the
//! bracket's presentation parameters are deliberately not baked into it. The
//! correction that reproduces `push_transform` around a body already carrying
//! `transform` is `transform * scale_about(..) * transform.inverse()` — see
//! [`snapshot_correction`].
//!
//! Only the outermost bracket is honoured, which the display list explicitly
//! permits: an inner bracket's own `alpha` and `scale` are ignored, and its
//! depth is tracked only so the matching pop can be identified.

use kurbo::{Affine, Rect};
use peniko::BlendMode;
use vello_common::geometry::RectU16;
use vello_common::record::LayerProps;

/// One open bracket: what its matching pop has to undo, and the device-space
/// bounds a clear hoisted past it is confined to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Group {
    clip: bool,
    layer: bool,
    bounds: Rect,
}

impl Group {
    /// Whether closing this bracket pops the clip stack.
    #[must_use]
    pub fn closes_clip(&self) -> bool {
        self.clip
    }

    /// Whether closing this bracket pops a recorded layer.
    #[must_use]
    pub fn closes_layer(&self) -> bool {
        self.layer
    }

    /// The bracket's device-space bounds.
    ///
    /// Exact for the axis-aligned transforms frust records (translate and
    /// scale); a rotated or skewed bracket bounds conservatively, which widens
    /// a hoisted clear's confinement rather than narrowing it.
    #[must_use]
    pub fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// The compiler's stack of open clip, layer and snapshot brackets.
///
/// Retained across frames alongside the clip stack it mirrors —
/// [`reset`](Self::reset) is what keeps it from carrying state between frames.
///
/// Every entry here has a counterpart on the clip stack, and the two are
/// pushed and popped together: a bracket is opened by pushing both, and closed
/// by popping both. That is the invariant that lets an unbalanced pop be
/// ignored safely rather than underflowing either one.
#[derive(Debug, Default)]
pub struct GroupStack {
    entries: Vec<Group>,
}

impl GroupStack {
    /// An empty stack, with no bracket open.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop every open bracket, keeping the buffer.
    pub fn reset(&mut self) {
        self.entries.clear();
    }

    /// Open a clip bracket covering `bounds`.
    pub fn push_clip(&mut self, bounds: Rect) {
        self.entries.push(Group {
            clip: true,
            layer: false,
            bounds,
        });
    }

    /// Open a layer bracket covering `bounds`.
    ///
    /// `isolated` says whether the layer was recorded as a layer of its own —
    /// a full-opacity one lowered to its clip alone, and has nothing in the
    /// recording for its pop to close.
    pub fn push_layer(&mut self, bounds: Rect, isolated: bool) {
        self.entries.push(Group {
            clip: true,
            layer: isolated,
            bounds,
        });
    }

    /// Open the bracket a retained body at full opacity records: a layer of
    /// its own with no clip behind it.
    ///
    /// The inline lowering of such a bracket clips nothing, so neither does
    /// this — the body draws exactly the pixels it would have drawn unwrapped,
    /// and the layer only gathers them for the retained page. Its bounds are
    /// unbounded for the same reason: nothing hoisted past it is confined by
    /// it, and nothing drawn inside it is windowed by it.
    pub fn push_retained(&mut self) {
        self.entries.push(Group {
            clip: false,
            layer: true,
            bounds: Rect::new(
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::INFINITY,
            ),
        });
    }

    /// Close the innermost open bracket, or `None` when none is open.
    pub fn pop(&mut self) -> Option<Group> {
        self.entries.pop()
    }

    /// Whether no bracket is open — a command here is at the frame root.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many brackets are open.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.entries.len()
    }

    /// The device-space bounds of every open bracket, outermost first.
    pub fn bounds(&self) -> impl Iterator<Item = Rect> + '_ {
        self.entries.iter().map(Group::bounds)
    }
}

/// How a `PushLayer` bracket lowers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerLowering {
    /// Its rectangle's clip and nothing else.
    Clip,
    /// A recorded layer of its own, plus that same clip.
    Isolated,
}

/// How a layer at `alpha` lowers.
///
/// A non-finite `alpha` answers [`LayerLowering::Isolated`], the same as any
/// alpha below one. It never reaches here from the frame path — the compiler's
/// up-front walk refuses such a layer outright — and the scheduler escalates a
/// recorded layer whose opacity is not finite, so the conservative answer is
/// the isolating one either way.
#[must_use]
pub fn lower_layer(alpha: f32) -> LayerLowering {
    if alpha >= 1.0 {
        LayerLowering::Clip
    } else {
        LayerLowering::Isolated
    }
}

/// The recorded properties an isolated opacity layer carries.
///
/// Everything but the opacity is left at its default deliberately: a blend
/// mode, a mask or a clip path on a recorded layer each need a route the
/// scheduler does not serve, and frust records none of them — the layer's own
/// rectangle is lowered through the clip stack instead (see the module
/// header).
#[must_use]
pub fn layer_props(opacity: f32) -> LayerProps {
    LayerProps {
        blend_mode: BlendMode::default(),
        opacity,
        mask: None,
        clip_path: None,
    }
}

/// The correction that reproduces a snapshot bracket's presentation scale.
///
/// The body was recorded under `transform` without the scale baked in, so
/// scaling it about `rect`'s centre means conjugating the scale by that
/// transform: move into the body's own space, scale about the centre there,
/// and move back. Composed ahead of the frame root, the result applies to
/// every command inside the bracket without any of them being rewritten.
///
/// Answers the identity in the two cases where there is nothing to correct: a
/// scale of exactly one, and a `transform` whose inverse is not finite. A
/// singular transform (a zero scale, a collapsed axis) has no usable inverse,
/// and the conjugation would carry infinities into every subsequent command's
/// transform; dropping the presentation scale draws the body unscaled, where
/// propagating them would refuse a frame the rest of which is perfectly
/// drawable.
#[must_use]
pub fn snapshot_correction(rect: Rect, scale: f64, transform: Affine) -> Affine {
    if scale == 1.0 {
        return Affine::IDENTITY;
    }

    let correction = transform * Affine::scale_about(scale, rect.center()) * transform.inverse();
    if correction.as_coeffs().iter().all(|c| c.is_finite()) {
        correction
    } else {
        Affine::IDENTITY
    }
}

/// Where a retained body lands on the device grid: the composed transform
/// its local coordinates are mapped through, split into the linear part and
/// the translation.
///
/// A page holds the body rasterized under one placement. Another placement
/// can be served from it only when the linear parts agree — the same scale,
/// rotation and skew, so the page's pixels are the pixels the body would draw
/// there — and the translations differ by a move a 1:1 composite can make
/// ([`Self::offset_to`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetainedPlacement {
    /// The transform's linear part, `[a, b, c, d]` in `kurbo`'s coefficient
    /// order.
    pub linear: [f64; 4],
    /// The transform's translation, in device pixels.
    pub translation: (f64, f64),
}

impl RetainedPlacement {
    /// How far apart two linear parts may be and still be read as one: well
    /// below anything a scale animation or a device-scale change produces, and
    /// well above the rounding a transform recomposed from the same factors
    /// accumulates.
    pub const LINEAR_EPSILON: f64 = 1e-6;

    /// The placement `transform` describes.
    #[must_use]
    pub fn of(transform: Affine) -> Self {
        let [a, b, c, d, e, f] = transform.as_coeffs();
        Self {
            linear: [a, b, c, d],
            translation: (e, f),
        }
    }

    /// The whole-pixel move from `self` to `other`, or `None` when the two
    /// differ in anything but their translation.
    ///
    /// A fractional move is rounded to the nearest whole pixel: a composite is
    /// a 1:1 copy of the page's texels and cannot land between them, so a body
    /// sliding by a fraction of a pixel per frame is drawn at most half a pixel
    /// from where an inline lowering would rasterize it. A whole-pixel move
    /// (the common case of a scroll or a pixel-snapped transition) is exact.
    #[must_use]
    pub fn offset_to(&self, other: &Self) -> Option<(i32, i32)> {
        let same_linear = self
            .linear
            .iter()
            .zip(other.linear.iter())
            .all(|(a, b)| (a - b).abs() <= Self::LINEAR_EPSILON);
        if !same_linear {
            return None;
        }
        let dx = (other.translation.0 - self.translation.0).round();
        let dy = (other.translation.1 - self.translation.1).round();
        let representable = |value: f64| value.is_finite() && value.abs() <= f64::from(u16::MAX);
        (representable(dx) && representable(dy)).then_some((dx as i32, dy as i32))
    }

    /// The device extent `rect` covers under this placement, per axis, rounded
    /// to whole pixels — the extent the resident-key ledger reports.
    #[must_use]
    pub fn extent(&self, rect: Rect) -> (u32, u32) {
        let [a, b, c, d] = self.linear;
        let x_scale = a.hypot(b);
        let y_scale = c.hypot(d);
        let pixels = |value: f64| {
            if value.is_finite() {
                value.round().clamp(0.0, f64::from(u32::MAX)) as u32
            } else {
                0
            }
        };
        (
            pixels(rect.width() * x_scale),
            pixels(rect.height() * y_scale),
        )
    }

    /// The uniform scale this placement rasterizes at — the square root of the
    /// linear part's determinant, which is the device scale for an
    /// axis-aligned body and the presentation scale folded into it.
    #[must_use]
    pub fn scale(&self) -> f32 {
        let [a, b, c, d] = self.linear;
        let scale = (a * d - b * c).abs().sqrt();
        if scale.is_finite() { scale as f32 } else { 0.0 }
    }
}

/// One retained bracket a compiler with retention on recorded this frame,
/// for the renderer's store (`crate::cache::retained`) to resolve.
#[derive(Debug, Clone, PartialEq)]
pub struct RetainedBracket {
    /// The key the bracket carried.
    pub key: u64,
    /// The recorded layer standing for the bracket, indexed into
    /// [`CommandRecorder::layers`](vello_common::record::CommandRecorder::layers).
    pub layer: u32,
    /// The bracket's alpha — what the page is composited at. Never baked into
    /// the page itself.
    pub opacity: f32,
    /// Where the body's local space lands on the device grid, presentation
    /// scale included.
    pub placement: RetainedPlacement,
    /// The device extent the bracket's `rect` covers under
    /// [`placement`](Self::placement) — the ledger's `extent_px`.
    pub extent_px: (u32, u32),
    /// Which of the two commands this is, with what each one needs.
    pub kind: RetainedKind,
}

/// The two retained commands, as the store needs to see them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedKind {
    /// A `PushRetained` body: rendered into a page when the store admits it.
    Record {
        /// The bracket's `rect` on the device grid, rounded out to whole tiles
        /// and clamped to the viewport — the most a page of this body may
        /// hold. A body whose own drawn bounds leave it painted outside its
        /// rectangle, which a page holding `rect` cannot carry anywhere else.
        rect: RectU16,
        /// Whether the body can be retained at all: it lay wholly inside the
        /// viewport and every scissor around it under no coverage mask (so
        /// nothing of it was clipped away on the way in), it drew no hole
        /// punch and no externally bound texture (whose pixels change with no
        /// command changing), and its bracket closed where it was opened.
        retainable: bool,
    },
    /// A `DrawRetained`: a composite of a resident page.
    Draw {
        /// The painter-order index the composite takes — the slot the command
        /// was recorded at.
        depth: u32,
        /// The device rectangle the composite may write: the viewport
        /// intersected with every scissor open around the command.
        window: RectU16,
        /// Whether a coverage-mask clip is open around the command, which a
        /// 1:1 composite cannot honour.
        masked: bool,
    },
}

impl RetainedBracket {
    /// Whether this is a `DrawRetained`.
    #[must_use]
    pub fn is_draw(&self) -> bool {
        matches!(self.kind, RetainedKind::Draw { .. })
    }
}

/// The snapshot bracket's inline emulation: how deep the walk is inside one,
/// and what the outermost bracket installed.
#[derive(Debug)]
pub struct RetainedStack {
    depth: usize,
    correction: Affine,
    layer_bracket: Option<usize>,
}

impl Default for RetainedStack {
    fn default() -> Self {
        Self::new()
    }
}

impl RetainedStack {
    /// A stack with no bracket open and nothing corrected.
    #[must_use]
    pub fn new() -> Self {
        Self {
            depth: 0,
            correction: Affine::IDENTITY,
            layer_bracket: None,
        }
    }

    /// Drop every open bracket and its correction.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The affine to compose ahead of the frame root for every command inside
    /// the open bracket — the identity when none is open, or when the
    /// outermost one asked for no scale.
    #[must_use]
    pub fn correction(&self) -> Affine {
        self.correction
    }

    /// Whether a bracket is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.depth > 0
    }

    /// Enter a bracket, returning whether it is the outermost one.
    ///
    /// The outermost bracket installs its presentation scale as the
    /// correction; an inner one installs nothing, which is the display list's
    /// own rule that only the outermost bracket needs honouring.
    pub fn enter(&mut self, rect: Rect, scale: f64, transform: Affine) -> bool {
        let outermost = self.depth == 0;
        if outermost {
            self.correction = snapshot_correction(rect, scale, transform);
        }
        self.depth = self.depth.saturating_add(1);
        outermost
    }

    /// Record that the outermost bracket opened a layer group, so the matching
    /// pop closes it.
    ///
    /// Separate from [`enter`](Self::enter) because opening the layer is the
    /// compiler's to do and can be declined — a bracket whose corrected
    /// transform does not survive composition opens none, and its pop must not
    /// then close a bracket it never opened. `bracket` is the group stack's
    /// depth just after the layer's own push — the handle [`leave`](Self::leave)
    /// uses to tell whether that group is still the one it would close.
    pub fn record_layer(&mut self, bracket: usize) {
        self.layer_bracket = Some(bracket);
    }

    /// Leave a bracket, returning whether the outermost one just closed
    /// *having opened a layer group* — that is, whether the caller now has a
    /// group to close.
    ///
    /// A pop with no bracket open is ignored, the policy the display list
    /// states for an unbalanced `PopRetained`. `open_brackets` is the group
    /// stack's current depth: when a stray pop inside the bracket's body has
    /// already closed the group the snapshot opened, the recorded depth is no
    /// longer reachable and no group is reported — closing one anyway would
    /// lift an ancestor bracket the display list still holds open.
    pub fn leave(&mut self, open_brackets: usize) -> bool {
        if self.depth == 0 {
            return false;
        }
        self.depth -= 1;
        if self.depth > 0 {
            return false;
        }

        self.correction = Affine::IDENTITY;
        self.layer_bracket
            .take()
            .is_some_and(|bracket| bracket <= open_brackets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_opacity_layer_lowers_to_its_clip_alone() {
        assert_eq!(lower_layer(1.0), LayerLowering::Clip);
        assert_eq!(lower_layer(2.0), LayerLowering::Clip);
        assert_eq!(lower_layer(0.5), LayerLowering::Isolated);
        // Zero contributes nothing, but that is the scheduler's call to make
        // from the recorded opacity, not the compiler's to make by omission.
        assert_eq!(lower_layer(0.0), LayerLowering::Isolated);
    }

    #[test]
    fn a_recorded_layer_carries_only_its_opacity() {
        let props = layer_props(0.25);
        assert_eq!(props.opacity, 0.25);
        assert_eq!(props.blend_mode, BlendMode::default());
        assert!(props.mask.is_none());
        assert!(props.clip_path.is_none());
    }

    #[test]
    fn the_innermost_bracket_is_what_closes_whichever_pop_closes_it() {
        let mut stack = GroupStack::new();
        stack.push_clip(Rect::new(0.0, 0.0, 10.0, 10.0));
        stack.push_layer(Rect::new(2.0, 2.0, 8.0, 8.0), true);
        assert_eq!(stack.depth(), 2);

        let inner = stack.pop().expect("two brackets are open");
        assert!(inner.closes_clip() && inner.closes_layer());
        let outer = stack.pop().expect("one bracket is still open");
        assert!(outer.closes_clip() && !outer.closes_layer());

        assert!(stack.is_empty());
        assert!(stack.pop().is_none());
    }

    #[test]
    fn a_full_opacity_layer_bracket_has_no_recorded_layer_to_close() {
        let mut stack = GroupStack::new();
        stack.push_layer(Rect::new(0.0, 0.0, 4.0, 4.0), false);
        let group = stack.pop().expect("a bracket is open");
        assert!(group.closes_clip());
        assert!(!group.closes_layer());
    }

    #[test]
    fn open_bracket_bounds_are_listed_outermost_first() {
        let mut stack = GroupStack::new();
        stack.push_clip(Rect::new(0.0, 0.0, 40.0, 40.0));
        stack.push_layer(Rect::new(10.0, 10.0, 20.0, 20.0), true);

        let bounds: Vec<Rect> = stack.bounds().collect();
        assert_eq!(
            bounds,
            vec![
                Rect::new(0.0, 0.0, 40.0, 40.0),
                Rect::new(10.0, 10.0, 20.0, 20.0),
            ]
        );
    }

    #[test]
    fn the_correction_scales_the_body_about_the_rect_centre() {
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let correction = snapshot_correction(rect, 0.5, Affine::IDENTITY);

        // The centre is the scale's fixed point; a corner moves halfway to it.
        assert_eq!(correction * rect.center(), rect.center());
        assert_eq!(correction * rect.origin(), (5.0, 5.0).into());
    }

    #[test]
    fn the_correction_conjugates_by_the_body_transform() {
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let transform = Affine::translate((100.0, 0.0));
        let correction = snapshot_correction(rect, 0.5, transform);

        // The centre in the body's own space is what stays put, so under the
        // composed transform the body's centre is still where it was.
        let composed = correction * transform;
        assert_eq!(composed * rect.center(), transform * rect.center());
    }

    #[test]
    fn a_scale_of_one_and_a_singular_transform_both_correct_nothing() {
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        assert_eq!(
            snapshot_correction(rect, 1.0, Affine::translate((3.0, 4.0))),
            Affine::IDENTITY
        );
        assert_eq!(
            snapshot_correction(rect, 0.5, Affine::scale(0.0)),
            Affine::IDENTITY
        );
    }

    #[test]
    fn only_the_outermost_bracket_installs_its_presentation() {
        let outer = Rect::new(0.0, 0.0, 20.0, 20.0);
        let inner = Rect::new(0.0, 0.0, 4.0, 4.0);
        let mut stack = RetainedStack::new();

        assert!(stack.enter(outer, 0.5, Affine::IDENTITY));
        let installed = stack.correction();
        assert_ne!(installed, Affine::IDENTITY);

        assert!(!stack.enter(inner, 4.0, Affine::IDENTITY));
        assert_eq!(stack.correction(), installed);

        // The inner pop restores nothing; the outer one restores everything.
        assert!(!stack.leave(0));
        assert_eq!(stack.correction(), installed);
        assert!(!stack.leave(0));
        assert_eq!(stack.correction(), Affine::IDENTITY);
        assert!(!stack.is_open());
    }

    #[test]
    fn a_bracket_that_opened_a_layer_reports_it_once_at_its_own_pop() {
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let mut stack = RetainedStack::new();

        stack.enter(rect, 1.0, Affine::IDENTITY);
        stack.record_layer(1);
        stack.enter(rect, 1.0, Affine::IDENTITY);

        assert!(!stack.leave(1), "the inner pop closes no group");
        assert!(stack.leave(1), "the outer pop closes the group it opened");
        assert!(!stack.leave(1), "an unbalanced pop closes nothing");
    }

    #[test]
    fn an_unbalanced_pop_retained_is_ignored() {
        let mut stack = RetainedStack::new();
        assert!(!stack.leave(0));
        assert!(!stack.leave(0));
        assert!(!stack.is_open());
        assert_eq!(stack.correction(), Affine::IDENTITY);
    }

    #[test]
    fn a_stray_pop_inside_a_snapshot_body_does_not_hand_its_group_to_pop_retained() {
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let mut stack = RetainedStack::new();

        stack.enter(rect, 1.0, Affine::IDENTITY);
        stack.record_layer(2);

        // A stray pop inside the body already closed the snapshot's own group:
        // the group stack is back below the recorded depth, so the snapshot's
        // pop must not report a group — closing one would lift an ancestor.
        assert!(!stack.leave(1), "a lost bracket reports no group to close");
        assert!(!stack.leave(1), "an unbalanced pop still closes nothing");
    }
}
