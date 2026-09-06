//! Ports beUI's `dynamic-island` composed block —
//! `components/motion/dynamic-island.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `dynamic-island`: *"iOS-style island pill that morphs between live
//! activity views with bouncy shell resize and blur crossfades."*
//!
//! | upstream | here |
//! |---|---|
//! | `PILL_WIDTH = 126`, `PILL_HEIGHT = 37` | [`DYNAMIC_ISLAND_PILL_WIDTH`], [`DYNAMIC_ISLAND_PILL_HEIGHT`] |
//! | `RADIUS = 32`, constant, browser-clamped to half the height | [`DYNAMIC_ISLAND_RADIUS`] through [`style::resolve_radius`] |
//! | shell `bg-foreground text-background shadow-2xl overflow-hidden` | the resolved ink surface, [`DYNAMIC_ISLAND_SHADOW_STD_DEV`] |
//! | shell `inline-flex items-start justify-center` | content pinned to the top edge, centred across |
//! | `SHELL_SPRING {duration: 0.8, bounce: 0.2}` | [`DYNAMIC_ISLAND_SHELL_SPRING`] (converted) |
//! | `CONTENT_SPRING {duration: 0.8, bounce: 0.35}` | [`DYNAMIC_ISLAND_CONTENT_SPRING`] (converted) |
//! | compact slot `min-h-[37px] min-w-[126px] gap-2 px-4 py-1.5` | [`DYNAMIC_ISLAND_COMPACT_PADDING_X`], [`DYNAMIC_ISLAND_COMPACT_PADDING_Y`] |
//! | view slot `px-6 py-4` | [`DYNAMIC_ISLAND_VIEW_PADDING_X`], [`DYNAMIC_ISLAND_VIEW_PADDING_Y`] |
//! | slot `initial {opacity:0, scale:0.9, y:-8, blur 5}` | [`DYNAMIC_ISLAND_SLOT_SCALE`], [`DYNAMIC_ISLAND_SLOT_ENTER_LIFT`] |
//! | slot `exit {opacity:0, scale:0.9, y:-6, 0.08s EASE_OUT}` | [`DYNAMIC_ISLAND_SLOT_EXIT_LIFT`], [`DYNAMIC_ISLAND_SLOT_EXIT`] |
//! | `transformOrigin: "top center"` | the per-slot pivot |
//! | `role="status" aria-live="polite"` | [`Role::Status`] |
//!
//! # Premise correction: the radius is **not** animated
//!
//! The porting card asks for "width/height/radius springs". `dynamic-island.tsx`
//! explicitly does the opposite, and says so in a comment: *"Constant radius —
//! never animated. The browser clamps it to half the shell height, so the
//! pill-to-rounded-rect morph falls out of the resize for free with zero chance
//! of corner glitches."* So there is no radius lane here. The port instead
//! passes the constant [`DYNAMIC_ISLAND_RADIUS`] through
//! [`style::resolve_radius`] on every paint, which clamps to half the shorter
//! edge — the same clamp `border-radius` performs, so the morph falls out of the
//! resize here for exactly the same reason. A test pins both halves (pill height
//! → clamped, tall view → the literal 32).
//!
//! # Premise correction: the state set belongs to the caller
//!
//! The card asks for the state machine to be "tested across all upstream
//! states". The component has no state set of its own: `view` is an opaque
//! `string | null` and each state is a `DynamicIslandView` child the *caller*
//! supplies. The `call`/`timer`/`music` trio is
//! `components/previews/blocks/dynamic-island.preview.tsx`'s demo data, not the
//! component's vocabulary. What is testable — and is tested — is the machine
//! itself: compact → any slot, slot → slot, slot → compact, and an unknown id
//! (which upstream renders as an empty expanded shell, since no
//! `DynamicIslandView` matches and the compact slot is gated on `view === null`;
//! that arm is ported literally, oddity included).
//!
//! # Motion's `{ duration, bounce }` springs, converted
//!
//! Both springs are authored in Motion's perceptual form, and are converted by
//! the identity [`crate::components::bouncy_accordion`] establishes:
//!
//! ```text
//! ζ  = 1 − bounce
//! ω₀ = −ln(ε) / (ζ · duration)      with ε = 1e-3, this crate's settle epsilon
//! mass = 1, stiffness = ω₀², damping = 2 · ζ · ω₀
//! ```
//!
//! The relationship upstream cares about survives: the content spring
//! (`bounce: 0.35`) is looser than the shell's (`bounce: 0.2`), which is the
//! comment's *"content gets a touch more life than the shell"*.
//!
//! # A layout animation, timed at paint and applied at layout
//!
//! The shell animates **real width and height**, not a transform — upstream's
//! own stated reason is that a scaled shell would distort its slots. `layout`
//! carries no clock, so this port takes the split
//! [`crate::blocks::expandable_action_bar`] documents: `layout` measures the
//! slots and retargets the two `Lane`s, `paint` advances them and calls
//! `PaintCtx::request_layout` while either is moving.
//!
//! # Desktop framing
//!
//! There is no desktop analogue of the OS-level notch this shape imitates, and
//! nothing in this crate can put a widget over one: frust's shells own a window,
//! not a status bar, and no platform seam publishes a notch cut-out. So this is
//! a **floating status surface** — an ordinary widget an app mounts at the top
//! of a `Stack` (the preview's own `h-32 items-start pt-2` framing, which exists
//! for the same reason on the web: the browser cannot reach the notch either).
//! Everything the port loses by that framing is framing, not behaviour: the
//! morph, the crossfade and the state machine are all the component's own.
//!
//! # Passive by construction
//!
//! Upstream binds no handler at all — the island is a `role="status"` readout
//! and every control in the preview lives *inside* a `DynamicIslandView`, as the
//! caller's own child. This port keeps that exactly: it handles no pointer or
//! key event of its own, and routes input to the visible slot so a caller's
//! buttons work. `view` is a prop with no callback, because there is nothing
//! here to report.
//!
//! # Degradations against the web original
//!
//! - **No blur.** Both slot stages animate `filter: blur(5px) → 0`.
//!   `PaintScene` publishes no blur filter, so the fade, the lift and the scale
//!   carry the crossfade. The registry blurb's "blur crossfades" is therefore a
//!   fade crossfade here.
//! - **No `ResizeObserver`.** Upstream re-measures its content whenever it
//!   changes size on its own (a ticking clock widening). Here a slot re-measures
//!   on the rebuild that changed it, which is every frame a reactive caller
//!   updates it, so the case that matters is covered without an observer.
//! - **Every slot is measured, only the visible ones are painted.** A slot is a
//!   `ChildPod` from the first build and stays laid out through its whole exit
//!   ramp, but only a visible one is painted, routed to or published — the
//!   input-parity carve-out [`crate::components::tabs`] documents.

use std::time::Duration;

use frust::authoring::{
    Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Rect, Role, SemanticsCtx, Size, View,
    Widget, any, build_child, rebuild_children, route_event_single, teardown_child, visit_children,
};
use frust::{ChildKey, FrameTime, SpringDescription, Theme};

use crate::motion::{Presence, PresencePhase, Ramp};
use crate::press::Lane;
use crate::style;
use crate::tokens::motion::EASE_OUT;

// ---- Metrics ---------------------------------------------------------------

/// The compact pill's width floor, in logical px (`PILL_WIDTH = 126`) — the
/// iPhone pill's own proportions, and the shell's pre-measure target.
pub const DYNAMIC_ISLAND_PILL_WIDTH: f64 = 126.0;

/// The compact pill's height floor, in logical px (`PILL_HEIGHT = 37`).
pub const DYNAMIC_ISLAND_PILL_HEIGHT: f64 = 37.0;

/// The shell's corner radius, in logical px (`RADIUS = 32`) — **constant**, and
/// resolved through [`style::resolve_radius`] so it clamps to half the shorter
/// edge exactly as `border-radius` does. See the [module docs](self).
pub const DYNAMIC_ISLAND_RADIUS: f64 = 32.0;

/// The compact slot's horizontal padding, in logical px (`px-4`).
pub const DYNAMIC_ISLAND_COMPACT_PADDING_X: f64 = 16.0;

/// The compact slot's vertical padding, in logical px (`py-1.5`).
pub const DYNAMIC_ISLAND_COMPACT_PADDING_Y: f64 = 6.0;

/// A view slot's horizontal padding, in logical px (`px-6`).
pub const DYNAMIC_ISLAND_VIEW_PADDING_X: f64 = 24.0;

/// A view slot's vertical padding, in logical px (`py-4`).
pub const DYNAMIC_ISLAND_VIEW_PADDING_Y: f64 = 16.0;

/// Gaussian sigma of the shell's `shadow-2xl` (`0 25px 50px -12px`), halved to
/// the standard deviation this scene's shadow primitive takes.
pub const DYNAMIC_ISLAND_SHADOW_STD_DEV: f64 = 12.5;

/// Alpha of the shell's `shadow-2xl` (`rgb(0 0 0 / 0.25)`).
pub const DYNAMIC_ISLAND_SHADOW_ALPHA: f32 = 0.25;

/// Vertical offset of that shadow, in logical px (`0 25px …`).
pub const DYNAMIC_ISLAND_SHADOW_Y_OFFSET: f64 = 25.0;

// ---- Motion ----------------------------------------------------------------

/// `SHELL_SPRING = { type: "spring", duration: 0.8, bounce: 0.2 }`, converted
/// (see the [module docs](self)). Drives the shell's real width and height.
pub const DYNAMIC_ISLAND_SHELL_SPRING: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 116.50,
    damping: 17.27,
};

/// `CONTENT_SPRING = { type: "spring", duration: 0.8, bounce: 0.35 }`,
/// converted — looser than the shell's, upstream's *"content gets a touch more
/// life"*.
pub const DYNAMIC_ISLAND_CONTENT_SPRING: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 176.47,
    damping: 17.27,
};

/// A slot's exit: `{ duration: 0.08, ease: EASE_OUT }` — *"sucked up into the
/// pill, fast, before the shrinking shell can clip it"*.
pub const DYNAMIC_ISLAND_SLOT_EXIT: Ramp = Ramp::eased(Duration::from_millis(80), EASE_OUT);

/// How far above its resting place an entering slot starts, in logical px
/// (`initial: { y: -8 }`).
pub const DYNAMIC_ISLAND_SLOT_ENTER_LIFT: f64 = 8.0;

/// How far above its resting place a leaving slot ends, in logical px
/// (`exit: { y: -6 }`).
pub const DYNAMIC_ISLAND_SLOT_EXIT_LIFT: f64 = 6.0;

/// The scale an entering/leaving slot is staged from (`scale: 0.9`).
pub const DYNAMIC_ISLAND_SLOT_SCALE: f64 = 0.9;

// ---- The view --------------------------------------------------------------

/// One island state: the id `view` matches, and the content shown while it does.
///
/// Upstream calls this component `DynamicIslandView`; the name is taken here by
/// the crate-wide `<Name>View<State>` convention for the *declarative view* half
/// of a widget pair, so upstream's own internal name for the same element —
/// `Slot` — is used instead.
pub struct DynamicIslandSlot<State: 'static> {
    id: String,
    content: AnyView<State>,
}

/// A slot shown while the island's `view` equals `id`.
pub fn dynamic_island_slot<State: 'static, V: View<State>>(
    id: impl Into<String>,
    content: V,
) -> DynamicIslandSlot<State> {
    DynamicIslandSlot {
        id: id.into(),
        content: any(content),
    }
}

impl<State: 'static> DynamicIslandSlot<State> {
    /// The id this slot answers to.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// A declarative beUI dynamic island. See the [module docs](self).
pub struct DynamicIslandView<State: 'static> {
    view: Option<String>,
    label: String,
    compact: AnyView<State>,
    slots: Vec<DynamicIslandSlot<State>>,
}

/// Create an island showing the slot whose id equals `view`, or `compact` while
/// `view` is `None`.
///
/// There is no callback: upstream's island is a `role="status"` readout that
/// reports nothing, and every control the preview puts on it lives inside a
/// slot as the caller's own child.
pub fn dynamic_island<State: 'static, C: View<State>>(
    view: Option<String>,
    compact: C,
    slots: Vec<DynamicIslandSlot<State>>,
) -> DynamicIslandView<State> {
    DynamicIslandView {
        view,
        label: String::new(),
        compact: any(compact),
        slots,
    }
}

impl<State: 'static> DynamicIslandView<State> {
    /// Give the live region an accessible name.
    ///
    /// An addition: upstream's `aria-live="polite"` region is named by whatever
    /// text its slot happens to render, which a painted frust child cannot
    /// supply. Empty by default, so an unnamed island publishes the same bare
    /// `Role::Status` container upstream does.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

/// The resolved island palette.
struct DynamicIslandColors {
    /// The shell's fill (`bg-foreground` — the island is *inverted* chrome).
    surface: Color,
}

/// Resolve the palette, falling back to the vendored **light** table with no
/// theme threaded — the unthemed posture every component in this catalog takes.
fn resolve_colors(theme: Option<&Theme>) -> DynamicIslandColors {
    match theme {
        Some(theme) => DynamicIslandColors {
            surface: theme.scheme().on_surface,
        },
        None => DynamicIslandColors {
            surface: crate::BEUI_LIGHT.foreground,
        },
    }
}

/// A slot's closed presence: the content spring in, the short tween out.
fn fresh_presence() -> Presence {
    Presence::new(
        Ramp::spring(DYNAMIC_ISLAND_CONTENT_SPRING),
        DYNAMIC_ISLAND_SLOT_EXIT,
    )
}

impl<State: 'static> View<State> for DynamicIslandView<State> {
    type Element = DynamicIslandWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> DynamicIslandWidget {
        let mut widget = DynamicIslandWidget {
            ids: self.slots.iter().map(|slot| slot.id.clone()).collect(),
            compact: build_child(&self.compact, ctx),
            slots: self
                .slots
                .iter()
                .map(|slot| build_child(&slot.content, ctx))
                .collect(),
            compact_presence: fresh_presence(),
            presences: self.slots.iter().map(|_| fresh_presence()).collect(),
            compact_box: Size::new(DYNAMIC_ISLAND_PILL_WIDTH, DYNAMIC_ISLAND_PILL_HEIGHT),
            slot_boxes: self.slots.iter().map(|_| Size::ZERO).collect(),
            view: self.view.clone(),
            label: self.label.clone(),
            shell_w: Lane::at_rest(
                Ramp::spring(DYNAMIC_ISLAND_SHELL_SPRING),
                DYNAMIC_ISLAND_PILL_WIDTH,
            ),
            shell_h: Lane::at_rest(
                Ramp::spring(DYNAMIC_ISLAND_SHELL_SPRING),
                DYNAMIC_ISLAND_PILL_HEIGHT,
            ),
            sized: false,
        };
        widget.sync_presences();
        widget.settle_initial();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DynamicIslandWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        if prev.slots.len() != self.slots.len() {
            element.presences = self.slots.iter().map(|_| fresh_presence()).collect();
            element.slot_boxes = self.slots.iter().map(|_| Size::ZERO).collect();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let ids: Vec<String> = self.slots.iter().map(|slot| slot.id.clone()).collect();
        if element.ids != ids {
            element.ids = ids;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.label != self.label {
            element.label = self.label.clone();
            flags |= ChangeFlags::PAINT;
        }
        if prev.view != self.view {
            element.view = self.view.clone();
            element.sync_presences();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags |= frust::authoring::rebuild_child(
            &self.compact,
            &prev.compact,
            &mut element.compact,
            ctx,
        );
        flags |= rebuild_children(
            &prev.slots,
            &self.slots,
            &mut element.slots,
            ctx,
            |slot: &DynamicIslandSlot<State>| &slot.content,
            |_| None::<ChildKey>,
        );
        flags
    }

    fn teardown(&self, element: &mut DynamicIslandWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.compact, &mut element.compact, ctx);
        for (slot, pod) in self.slots.iter().zip(element.slots.iter_mut()) {
            teardown_child(&slot.content, pod, ctx);
        }
    }
}

/// The retained widget for a [`DynamicIslandView`].
pub struct DynamicIslandWidget {
    /// The slot ids, in declaration order — the state set.
    ids: Vec<String>,
    /// The compact pill's content.
    compact: ChildPod,
    /// One pod per slot.
    slots: Vec<ChildPod>,
    /// The compact pill's presence, open exactly while `view` is `None`.
    compact_presence: Presence,
    /// One presence per slot, open exactly for the matching `view`.
    presences: Vec<Presence>,
    /// The compact pill's own box, from the last layout.
    compact_box: Size,
    /// Each slot's own box (content plus `px-6 py-4`), from the last layout.
    slot_boxes: Vec<Size>,
    /// The app-confirmed view (`None` is the compact pill).
    view: Option<String>,
    /// The live region's accessible name.
    label: String,
    /// The shell's real width, springing between boxes.
    shell_w: Lane,
    /// The shell's real height, springing between boxes.
    shell_h: Lane,
    /// Whether the shell lanes have ever been placed.
    sized: bool,
}

impl DynamicIslandWidget {
    /// The active slot's index, if any slot's id matches the view.
    fn active(&self) -> Option<usize> {
        let view = self.view.as_deref()?;
        self.ids.iter().position(|id| id == view)
    }

    /// Whether the compact pill shows: `view === null`, exactly upstream's
    /// gate — an *unmatched* id expands the shell onto nothing rather than
    /// falling back to the pill.
    fn compact_open(&self) -> bool {
        self.view.is_none()
    }

    /// Apply the confirmed view to every presence.
    fn sync_presences(&mut self) {
        let active = self.active();
        self.compact_presence.set_open(self.compact_open());
        for (index, presence) in self.presences.iter_mut().enumerate() {
            presence.set_open(active == Some(index));
        }
    }

    /// Drive whichever presence is open straight to `Present` —
    /// `<AnimatePresence initial={false}>`: an island that mounts showing a view
    /// is simply showing it, it does not play an entrance nobody asked for.
    fn settle_initial(&mut self) {
        let settled = FrameTime::from_nanos(
            Ramp::spring(DYNAMIC_ISLAND_CONTENT_SPRING)
                .settle()
                .as_nanos() as u64
                + 1,
        );
        self.compact_presence.advance(FrameTime::ZERO);
        self.compact_presence.advance(settled);
        for presence in &mut self.presences {
            presence.advance(FrameTime::ZERO);
            presence.advance(settled);
        }
    }

    /// The box the shell is springing toward: the active slot's, the compact
    /// pill's, or (for an unmatched id) an empty expanded shell.
    fn target_box(&self) -> Size {
        match self.active() {
            Some(index) => self.slot_boxes[index],
            None if self.compact_open() => self.compact_box,
            None => Size::ZERO,
        }
    }

    /// Whether the compact pill is still visible — true through its exit.
    fn compact_visible(&self) -> bool {
        self.compact_presence.is_visible()
    }

    /// Whether slot `index` is still visible — true through its exit.
    fn slot_visible(&self, index: usize) -> bool {
        self.presences.get(index).is_some_and(Presence::is_visible)
    }

    /// Advance both lanes and every presence to `now`, returning whether
    /// anything is still moving.
    fn advance(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        if reduce_motion {
            self.shell_w.snap();
            self.shell_h.snap();
            self.compact_presence = self.compact_presence.collapsed();
            for presence in &mut self.presences {
                *presence = presence.collapsed();
            }
        }
        let mut moving = self.shell_w.advance(now);
        moving |= self.shell_h.advance(now);
        self.compact_presence.advance(now);
        moving |= self.compact_presence.is_animating();
        for presence in &mut self.presences {
            presence.advance(now);
            moving |= presence.is_animating();
        }
        moving
    }

    /// The stage a presence is drawn at: `(alpha, lift, scale)`.
    ///
    /// The entrance and the exit are staged from different lifts (`initial:
    /// y -8`, `exit: y -6`), so the phase picks which.
    fn stage(presence: &Presence, now: FrameTime) -> (f64, f64, f64) {
        let p = presence.presence(now).clamp(0.0, 1.0);
        let lift = match presence.phase() {
            PresencePhase::Exiting => DYNAMIC_ISLAND_SLOT_EXIT_LIFT,
            _ => DYNAMIC_ISLAND_SLOT_ENTER_LIFT,
        };
        (
            p,
            -lift * (1.0 - p),
            DYNAMIC_ISLAND_SLOT_SCALE + (1.0 - DYNAMIC_ISLAND_SLOT_SCALE) * p,
        )
    }

    /// A slot's own box inside the shell: its natural size, pinned to the top
    /// edge and centred across (`items-start justify-center`).
    fn slot_rect(&self, box_size: Size) -> Rect {
        let shell_w = self.shell_w.value().max(0.0);
        Rect::from_origin_size(Point::new((shell_w - box_size.width) / 2.0, 0.0), box_size)
    }
}

/// Paint one presence's child at `rect`, staged by its own crossfade.
///
/// A free function rather than a method: the caller holds a `&mut ChildPod`
/// borrowed out of the widget, so a `&self` method could not be reached.
fn paint_slot(
    ctx: &mut PaintCtx,
    scene: &mut dyn PaintScene,
    pod: &mut ChildPod,
    rect: Rect,
    stage: (f64, f64, f64),
) {
    let (alpha, lift, scale) = stage;
    let origin = ctx.origin();
    // `transformOrigin: top center` — content unfurls downward out of the pill
    // line and is sucked back up into it.
    let pivot = Point::new(origin.x + rect.x0 + rect.width() / 2.0, origin.y + rect.y0);
    let size = ctx.size();
    scene.push_layer(origin, size, alpha as f32);
    scene.push_transform(
        Affine::translate((0.0, lift))
            * Affine::translate(pivot.to_vec2())
            * Affine::scale(scale)
            * Affine::translate(-pivot.to_vec2()),
    );
    pod.paint_child(ctx, scene);
    scene.pop_transform();
    scene.pop_layer();
}

impl Widget for DynamicIslandWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let loose = BoxConstraints::new(Size::ZERO, bc.max());

        // The compact pill: its content plus `px-4 py-1.5`, floored at the
        // iPhone pill's own 126x37.
        let compact_content = self.compact.layout_child(ctx, &loose);
        self.compact_box = Size::new(
            DYNAMIC_ISLAND_PILL_WIDTH
                .max(compact_content.width + DYNAMIC_ISLAND_COMPACT_PADDING_X * 2.0),
            DYNAMIC_ISLAND_PILL_HEIGHT
                .max(compact_content.height + DYNAMIC_ISLAND_COMPACT_PADDING_Y * 2.0),
        );

        // Each view slot: its content plus `px-6 py-4`.
        for index in 0..self.slots.len() {
            let content = self.slots[index].layout_child(ctx, &loose);
            self.slot_boxes[index] = Size::new(
                content.width + DYNAMIC_ISLAND_VIEW_PADDING_X * 2.0,
                content.height + DYNAMIC_ISLAND_VIEW_PADDING_Y * 2.0,
            );
        }

        // Retarget the shell — `layout` knows the boxes, `paint` has the clock.
        let target = self.target_box();
        if self.sized {
            self.shell_w.retarget(target.width);
            self.shell_h.retarget(target.height);
        } else {
            self.shell_w = Lane::at_rest(Ramp::spring(DYNAMIC_ISLAND_SHELL_SPRING), target.width);
            self.shell_h = Lane::at_rest(Ramp::spring(DYNAMIC_ISLAND_SHELL_SPRING), target.height);
            self.sized = true;
        }

        // Place the children inside their own boxes, top-pinned and centred.
        let compact_rect = self.slot_rect(self.compact_box);
        self.compact.set_origin(Point::new(
            compact_rect.x0 + (self.compact_box.width - compact_content.width) / 2.0,
            compact_rect.y0 + (self.compact_box.height - compact_content.height) / 2.0,
        ));
        for index in 0..self.slots.len() {
            let rect = self.slot_rect(self.slot_boxes[index]);
            self.slots[index].set_origin(Point::new(
                rect.x0 + DYNAMIC_ISLAND_VIEW_PADDING_X,
                rect.y0 + DYNAMIC_ISLAND_VIEW_PADDING_Y,
            ));
        }

        bc.constrain(Size::new(
            self.shell_w.value().max(0.0),
            self.shell_h.value().max(0.0),
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let moving = self.advance(now, reduce_motion);
        let origin = ctx.origin();
        let shell = ctx.size();
        if shell.width <= 0.0 || shell.height <= 0.0 {
            return;
        }

        // The constant radius, clamped to half the shorter edge — the whole
        // pill-to-rounded-rect morph, for free (see the module docs).
        let radius = style::resolve_radius(DYNAMIC_ISLAND_RADIUS, shell.width, shell.height);
        scene.draw_shadow(
            Point::new(origin.x, origin.y + DYNAMIC_ISLAND_SHADOW_Y_OFFSET),
            shell,
            radius,
            DYNAMIC_ISLAND_SHADOW_STD_DEV,
            style::with_alpha(Color::BLACK, DYNAMIC_ISLAND_SHADOW_ALPHA),
        );
        scene.fill_rounded_rect(origin, shell, radius, colors.surface);

        // `overflow-hidden`: a slot wider than the springing shell is clipped
        // to it rather than spilling past the pill.
        scene.push_clip_rounded(origin, shell, radius);
        if self.compact_visible() {
            let stage = Self::stage(&self.compact_presence, now);
            let rect = self.slot_rect(self.compact_box);
            paint_slot(ctx, scene, &mut self.compact, rect, stage);
        }
        for index in 0..self.slots.len() {
            if !self.slot_visible(index) {
                continue;
            }
            let stage = Self::stage(&self.presences[index], now);
            let rect = self.slot_rect(self.slot_boxes[index]);
            paint_slot(ctx, scene, &mut self.slots[index], rect, stage);
        }
        scene.pop_clip();

        // The shell animates real width and height, so a moving island asks for
        // a relayout rather than a bare repaint.
        if moving {
            ctx.request_layout();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.compact.event_child(ctx, event);
            for pod in &mut self.slots {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        // The island itself handles nothing — every control on it is the
        // caller's own child inside the visible slot.
        if let Some(index) = self.active()
            && let Some(pod) = self.slots.get_mut(index)
        {
            return route_event_single(pod, ctx, event);
        }
        if self.compact_open() {
            return route_event_single(&mut self.compact, ctx, event);
        }
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.label.clone();
        ctx.push_container(
            Role::Status,
            |node| {
                if !label.is_empty() {
                    node.set_label(label.as_str());
                }
            },
            |ctx| {
                // Only what input can reach is published.
                if let Some(pod) = self.active().and_then(|index| self.slots.get(index)) {
                    pod.semantics_child(ctx);
                } else if self.compact_open() {
                    self.compact.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(compact, slots);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BezPath, Brush, SemanticsUpdate};
    use frust::{SizedBox, text};
    use std::any::Any;

    /// Records what the widget paints.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size, f64)>,
        rounded_clips: Vec<(Point, Size, f64)>,
        layers: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, r: f64, _d: f64, _c: Color) {
            self.shadows.push((o, s, r));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_clip_rounded(&mut self, o: Point, s: Size, r: f64) {
            self.rounded_clips.push((o, s, r));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, _t: Affine) {}
    }

    #[derive(Default)]
    struct App;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// The preview's own three states, plus the compact pill — the trio is demo
    /// data, not the component's vocabulary (see the module docs).
    fn slots() -> Vec<DynamicIslandSlot<App>> {
        vec![
            // Tall enough that the constant radius is *not* the binding
            // constraint, which is half of what the radius test pins.
            dynamic_island_slot("call", SizedBox::<App>(Some(200.0), Some(90.0))),
            dynamic_island_slot("timer", text("2:34")),
            dynamic_island_slot(
                "music",
                text("Midnight City by M83, playing on the living room speaker"),
            ),
        ]
    }

    fn view(v: Option<&str>) -> DynamicIslandView<App> {
        dynamic_island::<App, _>(v.map(str::to_string), text("9:41"), slots())
            .label("Live activity")
    }

    fn build(v: Option<&str>) -> DynamicIslandWidget {
        let mut counter = 0u64;
        View::<App>::build(&view(v), &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut DynamicIslandWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(600.0, 400.0)),
        )
    }

    fn laid_out(v: Option<&str>) -> (DynamicIslandWidget, Size) {
        let mut w = build(v);
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(w: &mut DynamicIslandWidget, size: Size, ms: f64) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn retarget(w: &mut DynamicIslandWidget, from: Option<&str>, to: Option<&str>) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<App>::rebuild(&view(to), &view(from), w, &mut ctx);
    }

    /// Paint/relayout far past every ramp so the layout animation lands.
    fn settle(w: &mut DynamicIslandWidget, size: Size, from_ms: f64) -> Size {
        let mut size = size;
        for step in 0..40 {
            paint_at(w, size, from_ms + step as f64 * 500.0);
            size = layout(w);
        }
        size
    }

    /// At rest with no view the island is the iPhone pill, and it owes no frame.
    #[test]
    fn the_compact_island_is_the_iphone_pill() {
        let (mut w, size) = laid_out(None);
        assert_eq!(size.width, DYNAMIC_ISLAND_PILL_WIDTH);
        assert_eq!(size.height, DYNAMIC_ISLAND_PILL_HEIGHT);
        let (_, needs_frame) = paint_at(&mut w, size, 0.0);
        assert!(!needs_frame, "a settled pill animates nothing");
    }

    /// The full state machine: compact → each state → compact, and state →
    /// state directly. Every hop springs the shell onto that state's own box.
    #[test]
    fn the_state_machine_morphs_between_every_state_and_back() {
        let (mut w, size) = laid_out(None);
        let mut size = settle(&mut w, size, 0.0);
        let pill = size;

        let mut boxes = Vec::new();
        for (index, state) in ["call", "timer", "music"].into_iter().enumerate() {
            retarget(&mut w, None, Some(state));
            let staged = layout(&mut w);
            size = settle(&mut w, staged, 100_000.0);
            assert!(
                (size.width - w.slot_boxes[index].width).abs() < 0.01
                    && (size.height - w.slot_boxes[index].height).abs() < 0.01,
                "{state}: {size:?} vs {:?}",
                w.slot_boxes[index]
            );
            assert_eq!(w.active(), Some(index));
            boxes.push(size);

            retarget(&mut w, Some(state), None);
            let staged = layout(&mut w);
            size = settle(&mut w, staged, 200_000.0);
            assert!(
                (size.width - pill.width).abs() < 0.01 && (size.height - pill.height).abs() < 0.01,
                "{state} → compact: {size:?} vs {pill:?}"
            );
        }

        // ...and state → state, without passing through the pill.
        retarget(&mut w, None, Some("timer"));
        let staged = layout(&mut w);
        settle(&mut w, staged, 300_000.0);
        retarget(&mut w, Some("timer"), Some("music"));
        let staged = layout(&mut w);
        let size = settle(&mut w, staged, 400_000.0);
        assert!(
            (size.width - boxes[2].width).abs() < 0.01,
            "timer → music lands on music's box: {size:?} vs {:?}",
            boxes[2]
        );
        assert!(
            boxes[2].width > boxes[1].width,
            "the three states really do have different boxes"
        );
    }

    /// The shell is strictly between the two boxes mid-morph and owes frames —
    /// the retarget property.
    #[test]
    fn the_shell_springs_between_boxes_rather_than_snapping() {
        let (mut w, size) = laid_out(None);
        let pill = settle(&mut w, size, 0.0);
        retarget(&mut w, None, Some("music"));
        let size = layout(&mut w);
        assert_eq!(size, pill, "the first frame is still the pill");

        let (_, needs_frame) = paint_at(&mut w, size, 100_000.0);
        assert!(needs_frame, "a morphing island owes frames");
        let mut size = layout(&mut w);
        for step in 1..5 {
            paint_at(&mut w, size, 100_000.0 + step as f64 * 60.0);
            size = layout(&mut w);
        }
        let target = w.slot_boxes[2];
        assert!(
            size.width > pill.width && size.width < target.width,
            "mid-morph {size:?} between {pill:?} and {target:?}"
        );
    }

    /// The radius is constant and *clamped*, never animated: a pill-height
    /// shell paints half its height, a taller one paints the literal 32.
    #[test]
    fn the_radius_is_the_constant_clamped_to_half_the_height() {
        let (mut w, size) = laid_out(None);
        let size = settle(&mut w, size, 0.0);
        let (rec, _) = paint_at(&mut w, size, 100_000.0);
        let (_, _, radius, _) = rec.rrects[0];
        assert_eq!(
            radius,
            DYNAMIC_ISLAND_PILL_HEIGHT / 2.0,
            "a 37px pill clamps below the 32px constant"
        );

        let (mut tall, size) = laid_out(Some("call"));
        let size = settle(&mut tall, size, 0.0);
        assert!(
            size.height > DYNAMIC_ISLAND_RADIUS * 2.0,
            "the call view is taller than twice the radius"
        );
        let (rec, _) = paint_at(&mut tall, size, 100_000.0);
        assert_eq!(rec.rrects[0].2, DYNAMIC_ISLAND_RADIUS);
        assert_eq!(
            rec.rounded_clips[0].2, DYNAMIC_ISLAND_RADIUS,
            "the `overflow-hidden` clip takes the same radius"
        );
        assert!(!rec.shadows.is_empty(), "the shell casts its `shadow-2xl`");
    }

    /// A morph keeps the outgoing slot alive through its exit while the
    /// incoming one enters — `AnimatePresence mode="popLayout"`, ported.
    #[test]
    fn a_morph_crossfades_the_outgoing_slot_against_the_incoming_one() {
        let (mut w, size) = laid_out(Some("call"));
        let size = settle(&mut w, size, 0.0);
        assert!(w.slot_visible(0) && !w.slot_visible(1) && !w.compact_visible());

        retarget(&mut w, Some("call"), Some("timer"));
        layout(&mut w);
        let (rec, _) = paint_at(&mut w, size, 100_000.0);
        assert!(
            w.slot_visible(0) && w.slot_visible(1),
            "both slots are alive mid-morph"
        );
        assert_eq!(w.presences[0].phase(), PresencePhase::Exiting);
        assert_eq!(w.presences[1].phase(), PresencePhase::Entering);
        assert_eq!(rec.layers.len(), 2, "one staged layer per live slot");

        settle(&mut w, size, 100_100.0);
        assert!(!w.slot_visible(0), "the exit has finished");
        assert!(w.slot_visible(1));
    }

    /// An entering slot is lifted, faded and scaled from 0.9; a leaving one is
    /// staged from the *exit* lift instead. Both land square.
    #[test]
    fn slots_enter_lifted_and_leave_on_the_shorter_exit_lift() {
        let (mut w, size) = laid_out(None);
        paint_at(&mut w, size, 0.0);
        retarget(&mut w, None, Some("timer"));
        layout(&mut w);
        paint_at(&mut w, size, 100_000.0);
        layout(&mut w);
        paint_at(&mut w, size, 100_060.0);

        let (alpha, lift, scale) = DynamicIslandWidget::stage(&w.presences[1], ft_ms(100_060.0));
        assert!(alpha > 0.0 && alpha < 1.0, "mid-entrance alpha {alpha}");
        assert!((-DYNAMIC_ISLAND_SLOT_ENTER_LIFT..0.0).contains(&lift));
        assert!(scale > DYNAMIC_ISLAND_SLOT_SCALE && scale < 1.0);

        let (alpha, lift, scale) =
            DynamicIslandWidget::stage(&w.compact_presence, ft_ms(100_060.0));
        assert_eq!(w.compact_presence.phase(), PresencePhase::Exiting);
        assert!(alpha < 1.0);
        assert!(
            (-DYNAMIC_ISLAND_SLOT_EXIT_LIFT..=0.0).contains(&lift),
            "the exit lift bounds the travel: {lift}"
        );
        assert!(scale >= DYNAMIC_ISLAND_SLOT_SCALE);

        settle(&mut w, size, 100_100.0);
        assert_eq!(
            DynamicIslandWidget::stage(&w.presences[1], ft_ms(200_000.0)),
            (1.0, 0.0, 1.0)
        );
    }

    /// An id that matches no slot expands onto an empty shell rather than
    /// falling back to the pill — upstream's `view === null` gate, ported
    /// literally.
    #[test]
    fn an_unmatched_id_shows_neither_the_pill_nor_a_slot() {
        let (mut w, size) = laid_out(Some("nope"));
        assert_eq!(w.active(), None);
        assert!(!w.compact_open());
        assert_eq!(w.target_box(), Size::ZERO);
        let settled = settle(&mut w, size, 0.0);
        assert_eq!(settled, Size::ZERO);
        let (rec, _) = paint_at(&mut w, settled, 100_000.0);
        assert!(
            rec.rrects.is_empty(),
            "a zero-box shell paints nothing at all"
        );
    }

    /// `reduce_motion` collapses every ramp: the shell lands on its target box
    /// on the first frame and owes no frame.
    #[test]
    fn reduced_motion_lands_the_morph_at_once() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(None);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);

        retarget(&mut w, None, Some("music"));
        let size = layout(&mut w);
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(16.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        assert!(!ctx.needs_frame(), "a collapsed ramp owes no frame");
        assert_eq!(layout(&mut w), w.slot_boxes[2]);
    }

    /// The shell fills with the *inverted* chrome role — `bg-foreground`, not a
    /// surface — which is what makes the island read as a dark pill in light
    /// mode.
    #[test]
    fn the_shell_fills_with_the_inverted_ink_role() {
        let (mut w, size) = laid_out(None);
        let size = settle(&mut w, size, 0.0);
        let (rec, _) = paint_at(&mut w, size, 100_000.0);
        assert_eq!(rec.rrects[0].3, crate::BEUI_LIGHT.foreground);
        let themed = crate::theme();
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(100_000.0)).with_theme(&themed);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(rec.rrects[0].3, themed.scheme().on_surface);
    }

    /// The converted springs settle at the durations upstream states, and the
    /// content spring is the looser of the two.
    #[test]
    fn the_converted_springs_settle_at_the_upstream_durations() {
        for spring in [DYNAMIC_ISLAND_SHELL_SPRING, DYNAMIC_ISLAND_CONTENT_SPRING] {
            let settle = Ramp::spring(spring).settle().as_secs_f64() * 1000.0;
            assert!(
                (settle - 800.0).abs() < 5.0,
                "{spring:?} settles at {settle}ms, expected 800ms"
            );
        }
        let damping_ratio =
            |s: SpringDescription| s.damping / (2.0 * (s.mass * s.stiffness).sqrt());
        assert!(
            damping_ratio(DYNAMIC_ISLAND_CONTENT_SPRING)
                < damping_ratio(DYNAMIC_ISLAND_SHELL_SPRING),
            "the content spring has more life than the shell's"
        );
        assert_eq!(DYNAMIC_ISLAND_SLOT_EXIT.settle(), Duration::from_millis(80));
    }

    /// The island publishes a named live-region status container.
    #[test]
    fn semantics_publish_a_named_status_region() {
        let mut root = frust_core::RenderRoot::new();
        let mut state = App;
        let mut tcx = TextContext::new();
        let mut logic = move |_s: &mut App| view(Some("call"));
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(600.0, 400.0), &mut tcx as &mut dyn Any);

        let update: SemanticsUpdate = root.semantics();
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Status && n.label() == Some("Live activity")),
            "the island is a named status region"
        );
    }
}
