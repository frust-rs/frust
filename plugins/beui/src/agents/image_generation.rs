//! Ports beUI's `image-generation` agent-interface part.
//!
//! **Source:** `components/agents/image-generation.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `image-generation`: *"A stable generated-image surface that moves from
//! queued work through progressive refinement to a completed result without
//! layout shift."*
//!
//! The card an agent shows while it paints: a fixed-aspect media slot that
//! reserves its space up front, a **dither field** — a grid of dots displaced
//! around a drifting attractor — covering it while the work runs, and the media
//! itself fading through as the run refines and completes. A resolution badge
//! sits in the corner, a status line reads underneath, and a failed run offers
//! a retry.
//!
//! # A correction to the brief this port was written from
//!
//! The porting card describes a *shimmer/progress* placeholder. Upstream has
//! neither: there is no progress value anywhere in the component, and the
//! placeholder is the canvas dither field described above. This port follows
//! upstream — the dither field, upstream's five statuses, and its per-status
//! media table — rather than the card's description.
//!
//! # Upstream's exports, and where each landed
//!
//! | upstream | here |
//! |---|---|
//! | `ImageGeneration` | [`image_generation`] |
//! | `ImageGenerationStatus` | [`ImageGenerationStatus`] |
//! | `STATUS_TEXT` | [`ImageGenerationStatus::status_text`] |
//! | `MEDIA_STATE` | [`ImageGenerationStatus::media_opacity`]/[`media_scale`](ImageGenerationStatus::media_scale) |
//! | `OVERLAY_OPACITY` | [`ImageGenerationStatus::overlay_opacity`] |
//! | `DitherField` (canvas) | the painted dot grid — [`DITHER_GAP`] and friends |
//! | `DitherMark` | the status line's leading glyph |
//! | `children` (the media) | the optional media slot [`ImageGenerationView::media`] takes |
//! | `onRetry` | [`ImageGenerationView::on_retry`] |
//!
//! # Motion
//!
//! * **The media** crossfades and scales between the per-status pair over
//!   [`IMAGE_MEDIA_MS`] of [`EASE_OUT`], on two lanes.
//! * **The overlay** fades to its own per-status opacity on the same ramp.
//! * **The dither field** is a perpetual decorative loop, so it asks for frames
//!   through `request_frame_paced` (`TickClass::CosmeticLoop`) rather than the
//!   unpaced transition class, exactly as `docs/WIDGETS_CODE_STANDARDS.md`
//!   requires. Its attractor drifts on upstream's own two periods
//!   ([`DITHER_DRIFT_X_MS`]/[`DITHER_DRIFT_Y_MS`]) until a pointer enters, and
//!   then follows the pointer with upstream's own two easing factors.
//! * **The status glyph** turns once every [`IMAGE_MARK_SPIN_MS`] while the run
//!   is active — also a cosmetic loop.
//! * **The status text** rises and fades whenever it changes
//!   ([`IMAGE_STATUS_SWAP_MS`]).
//!
//! Under `reduce_motion` the drift, the spin and the media ramps all collapse:
//! the field is painted from its resting attractor, the media sits at its
//! status' resting opacity, and **no frames are requested at all** — which is
//! upstream's own behaviour (`if (!reduce) frame = requestAnimationFrame(draw)`).
//!
//! # Degradations against the web original
//!
//! - **No blur, so the media reveal is opacity and scale only.** Upstream's
//!   `MEDIA_STATE` is a `filter: blur(Npx) saturate(N)` triple per status.
//!   `PaintScene` has no blur or saturation filter — nothing in the framework
//!   does — so the two channels that survive are kept and the filter is
//!   dropped. A queued/generating slot is invisible either way (opacity `0`);
//!   what is lost is the *soft* edge of a refining one.
//! - **The dither dots are painted every frame, not into a cached canvas.**
//!   Upstream draws into a `<canvas>` at device resolution; there is no
//!   retained drawing surface here, so the grid is emitted as scene primitives.
//!   [`DITHER_GAP`] keeps the count bounded (a 208x208 card is ~500 dots).
//! - **The media slot takes a view, not an `<img>`.** Image *delivery* is the
//!   app's business — upstream says as much in its own prop docs — so the slot
//!   is any view, laid out to fill the reserved box.
//! - **No `interactive` pointer capability test.** Upstream gates the
//!   pointer-follow on a `(hover: hover)` media query so a finger never drags
//!   the attractor. There is no pointer-capability seam in `frust::authoring`,
//!   so [`ImageGenerationView::interactive`] gates it on the caller's own flag
//!   alone; a touch-first host passes `false`.
//! - **The status text swaps in place rather than through a popLayout pair.**
//!   Upstream cross-fades the outgoing and incoming strings over each other.
//!   Here the outgoing string fades up and out while the incoming rises into
//!   the same slot, which is the same read with one run less.

use std::f64::consts::TAU;
use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::{
    Action, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, ErasedCallback,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect,
    Role, SemanticsCtx, Shape, Size, View, Widget, any, build_child, erase_callback, rebuild_child,
    route_event_single, teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use kurbo::{Arc, Vec2};

use crate::motion::{PointerTracker, Ramp};
use crate::press::{Lane, presses};
use crate::style::{self, scale_alpha, with_alpha};
use crate::text::{Label, LabelRun};
use crate::tokens::motion::{EASE_IN_OUT, EASE_OUT, SPRING_PRESS};
use crate::tokens::{BEUI_LIGHT, BeuiPalette, mono_family, sans_family};

/// The widest a `compact` card gets, in logical px (`max-w-52`).
pub const IMAGE_COMPACT_WIDTH: f64 = 208.0;

/// The media slot's corner radius (`rounded-xl`).
pub const IMAGE_RADIUS: f64 = style::RADIUS_XL;

/// The gap between the media slot and the status line (`mt-3`).
pub const IMAGE_STATUS_GAP: f64 = 12.0;

/// The status line's smallest height (`min-h-5`).
pub const IMAGE_STATUS_HEIGHT: f64 = 20.0;

/// The gap between the status glyph and its text (`gap-2`).
pub const IMAGE_STATUS_INNER_GAP: f64 = style::GAP_MD;

/// The status glyph's box (`size-3.5`).
pub const IMAGE_MARK_SIZE: f64 = 14.0;

/// The gap between the status line and the prompt caption (`mt-0.5`).
pub const IMAGE_PROMPT_GAP: f64 = 2.0;

/// The retry button's height (`min-h-10`), and its `mt-3` lead-in shares
/// [`IMAGE_STATUS_GAP`].
pub const IMAGE_RETRY_HEIGHT: f64 = style::HEIGHT_MD;

/// Horizontal padding inside the retry button (`px-3`).
pub const IMAGE_RETRY_PADDING_X: f64 = style::PADDING_X_SM;

/// The resolution badge's inset from the media slot's corner (`top-2 right-2`).
pub const IMAGE_BADGE_INSET: f64 = 8.0;

/// Horizontal padding inside the resolution badge (`px-2`).
pub const IMAGE_BADGE_PADDING_X: f64 = 8.0;

/// Vertical padding inside the resolution badge (`py-0.5`).
pub const IMAGE_BADGE_PADDING_Y: f64 = 2.0;

/// The resolution badge's type size, in logical px (`text-[10px]`).
pub const IMAGE_BADGE_TEXT: f64 = 10.0;

/// How long the media and overlay crossfades take, in ms (`duration: 0.4`).
pub const IMAGE_MEDIA_MS: u64 = 400;

/// How long a status-text swap takes, in ms (`duration: 0.15`).
pub const IMAGE_STATUS_SWAP_MS: u64 = 150;

/// How far the status text travels through its swap, in logical px (`y: 4`).
pub const IMAGE_STATUS_SWAP_SHIFT: f64 = 4.0;

/// How long the active status glyph takes to turn once, in ms
/// (`duration: 2.4`).
pub const IMAGE_MARK_SPIN_MS: u64 = 2400;

/// The gap between dither dots, in logical px (`DOT_GAP = 10`).
pub const DITHER_GAP: f64 = 10.0;

/// The attractor's radius of influence, as a fraction of the field's shorter
/// edge (`Math.min(width, height) * 0.38`).
pub const DITHER_RADIUS_FRACTION: f64 = 0.38;

/// How far a fully-influenced dot is pushed away from the attractor, in logical
/// px (`influence * influence * 9`).
pub const DITHER_DISPLACEMENT: f64 = 9.0;

/// The idle attractor drift's horizontal period, in ms (`Math.sin(time / 1700)`).
pub const DITHER_DRIFT_X_MS: f64 = 1700.0;

/// The idle attractor drift's vertical period, in ms (`Math.cos(time / 2100)`).
pub const DITHER_DRIFT_Y_MS: f64 = 2100.0;

/// How far the idle attractor wanders horizontally, as a fraction of the field
/// (`width * 0.12`).
const DITHER_DRIFT_X: f64 = 0.12;

/// How far it wanders vertically (`height * 0.1`).
const DITHER_DRIFT_Y: f64 = 0.1;

/// How fast the attractor closes on its idle target each frame (`0.045`).
const DITHER_FOLLOW_IDLE: f64 = 0.045;

/// How fast it closes on a live pointer each frame (`0.16`).
const DITHER_FOLLOW_POINTER: f64 = 0.16;

/// A dot's radius at zero influence (`0.65`), and how much influence adds
/// (`+ influence * 0.85`).
const DITHER_DOT_RADIUS: f64 = 0.65;
const DITHER_DOT_GROWTH: f64 = 0.85;

/// A dot's alpha at zero influence (`0.17`), and how much influence adds
/// (`+ influence * 0.72`).
const DITHER_ALPHA: f32 = 0.17;
const DITHER_ALPHA_GROWTH: f32 = 0.72;

/// The default resolution caption (`resolution = "1024 x 1024"`, whose
/// separator upstream writes as a multiplication sign).
pub const IMAGE_RESOLUTION: &str = "1024 \u{00d7} 1024";

/// The default reserved aspect ratio (`aspectRatio = "1 / 1"`).
pub const IMAGE_ASPECT_RATIO: f64 = 1.0;

/// The retry button's label (`"Try again"`).
pub const IMAGE_RETRY_LABEL: &str = "Try again";

/// Alpha of the resolution badge's backing (`bg-background/75`).
const BADGE_BACKING_ALPHA: f32 = 0.75;

/// Unthemed fallback palette — see [`super::message_bubble`]'s own note.
const FALLBACK: BeuiPalette = BEUI_LIGHT;

/// How far a generation has got — upstream's `ImageGenerationStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageGenerationStatus {
    /// Accepted, not started.
    Queued,
    /// Running: the field covers the slot completely.
    #[default]
    Generating,
    /// Nearly there: the media shows through the thinning field.
    Refining,
    /// Done: the field is gone and the media is at full strength.
    Complete,
    /// Failed: the media is a ghost and a retry is offered.
    Error,
}

impl ImageGenerationStatus {
    /// Every status, in upstream's own union order.
    pub const ALL: [ImageGenerationStatus; 5] = [
        ImageGenerationStatus::Queued,
        ImageGenerationStatus::Generating,
        ImageGenerationStatus::Refining,
        ImageGenerationStatus::Complete,
        ImageGenerationStatus::Error,
    ];

    /// Whether work is still in flight (`aria-busy`), which is also what keeps
    /// the dither field mounted.
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            ImageGenerationStatus::Queued
                | ImageGenerationStatus::Generating
                | ImageGenerationStatus::Refining
        )
    }

    /// The status line upstream's `STATUS_TEXT` table gives this status.
    pub const fn status_text(self) -> &'static str {
        match self {
            ImageGenerationStatus::Queued => "Waiting to generate",
            ImageGenerationStatus::Generating => "Generating image",
            ImageGenerationStatus::Refining => "Refining details",
            ImageGenerationStatus::Complete => "Image ready",
            ImageGenerationStatus::Error => "Generation failed",
        }
    }

    /// The media's opacity in this status (`MEDIA_STATE[...].opacity`).
    pub const fn media_opacity(self) -> f64 {
        match self {
            ImageGenerationStatus::Queued | ImageGenerationStatus::Generating => 0.0,
            ImageGenerationStatus::Refining => 0.62,
            ImageGenerationStatus::Complete => 1.0,
            ImageGenerationStatus::Error => 0.28,
        }
    }

    /// The media's scale in this status (`MEDIA_STATE[...].scale`).
    pub const fn media_scale(self) -> f64 {
        match self {
            ImageGenerationStatus::Queued => 1.02,
            ImageGenerationStatus::Generating => 1.015,
            ImageGenerationStatus::Refining => 1.005,
            ImageGenerationStatus::Complete | ImageGenerationStatus::Error => 1.0,
        }
    }

    /// The dither field's opacity in this status (`OVERLAY_OPACITY`).
    pub const fn overlay_opacity(self) -> f64 {
        match self {
            ImageGenerationStatus::Queued | ImageGenerationStatus::Generating => 1.0,
            ImageGenerationStatus::Refining => 0.48,
            ImageGenerationStatus::Complete | ImageGenerationStatus::Error => 0.0,
        }
    }
}

/// How wide the card lets itself get — upstream's `size` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageGenerationSize {
    /// `mx-auto max-w-52`: a thumbnail centred in whatever room it is given.
    #[default]
    Compact,
    /// `w-full`: the card fills its row.
    Fluid,
}

/// A view-held retry callback (erased on build).
type OnRetry<State> = Rc<dyn Fn(&mut State)>;

/// A declarative beUI generated-image card. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::agents::image_generation::{
///     ImageGenerationStatus, image_generation,
/// };
///
/// // While the run is live the slot is a dither field...
/// let working = image_generation::<()>().prompt("a cat riding a bicycle");
/// // ...and the finished media is handed in as the slot's child.
/// let done = image_generation::<()>()
///     .media(text("[the image]"))
///     .status(ImageGenerationStatus::Complete);
/// ```
pub struct ImageGenerationView<State: 'static> {
    media: Option<AnyView<State>>,
    status: ImageGenerationStatus,
    label: Option<String>,
    prompt: Option<String>,
    resolution: Option<String>,
    aspect_ratio: f64,
    size: ImageGenerationSize,
    interactive: bool,
    status_text: Option<String>,
    show_status: bool,
    on_retry: Option<OnRetry<State>>,
}

/// Create a [`Generating`](ImageGenerationStatus::Generating) card with no
/// media, a square reserved slot and the default resolution caption.
pub fn image_generation<State: 'static>() -> ImageGenerationView<State> {
    ImageGenerationView {
        media: None,
        status: ImageGenerationStatus::default(),
        label: None,
        prompt: None,
        resolution: Some(IMAGE_RESOLUTION.to_owned()),
        aspect_ratio: IMAGE_ASPECT_RATIO,
        size: ImageGenerationSize::default(),
        interactive: true,
        status_text: None,
        show_status: true,
        on_retry: None,
    }
}

impl<State: 'static> ImageGenerationView<State> {
    /// Put `media` in the reserved slot — upstream's `children`. Delivery is the
    /// app's business; the card only reveals what it is handed.
    pub fn media<V: View<State>>(mut self, media: V) -> Self {
        self.media = Some(any(media));
        self
    }

    /// Show `status` instead of
    /// [`Generating`](ImageGenerationStatus::Generating).
    pub fn status(mut self, status: ImageGenerationStatus) -> Self {
        self.status = status;
        self
    }

    /// Override the accessible description (upstream's `label`, which otherwise
    /// derives from the status text and the prompt).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Caption the card with the prompt it is generating from.
    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = Some(prompt.into());
        self
    }

    /// Replace the resolution badge's text (default [`IMAGE_RESOLUTION`]).
    pub fn resolution(mut self, resolution: impl Into<String>) -> Self {
        self.resolution = Some(resolution.into());
        self
    }

    /// Drop the resolution badge entirely (upstream's `resolution={undefined}`).
    pub fn no_resolution(mut self) -> Self {
        self.resolution = None;
        self
    }

    /// Reserve a `width / height` aspect ratio for the slot (default `1.0`).
    pub fn aspect_ratio(mut self, ratio: f64) -> Self {
        self.aspect_ratio = if ratio > 0.0 {
            ratio
        } else {
            IMAGE_ASPECT_RATIO
        };
        self
    }

    /// Let the card fill its row instead of capping at
    /// [`IMAGE_COMPACT_WIDTH`].
    pub fn size(mut self, size: ImageGenerationSize) -> Self {
        self.size = size;
        self
    }

    /// Whether the dither field's attractor follows the pointer (`interactive`).
    pub fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// Override the status line's text.
    pub fn status_text(mut self, text: impl Into<String>) -> Self {
        self.status_text = Some(text.into());
        self
    }

    /// Whether the status line is shown at all (`showStatus`).
    pub fn show_status(mut self, show: bool) -> Self {
        self.show_status = show;
        self
    }

    /// Offer a retry after a failed run (`onRetry`). The button appears only in
    /// [`Error`](ImageGenerationStatus::Error), exactly as upstream gates it.
    pub fn on_retry<F: Fn(&mut State) + 'static>(mut self, callback: F) -> Self {
        self.on_retry = Some(Rc::new(callback));
        self
    }

    /// The status line this card resolves to — the override, else the status'
    /// own text.
    pub fn resolved_status_text(&self) -> String {
        self.status_text
            .clone()
            .unwrap_or_else(|| self.status.status_text().to_owned())
    }

    /// The accessible name this card resolves to, upstream's own derivation.
    pub fn resolved_label(&self) -> String {
        if let Some(label) = &self.label {
            return label.clone();
        }
        let status = self.resolved_status_text();
        match &self.prompt {
            Some(prompt) => format!("{status}: {prompt}"),
            None => status,
        }
    }
}

/// The resolved card palette.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ImageColors {
    /// The slot's backing (`bg-muted`), which the dither field sits on.
    surface: Color,
    /// The dots' and the media's ink (`text-foreground`).
    ink: Color,
    /// Secondary ink (`text-muted-foreground`).
    muted: Color,
    /// The badge's backing (`bg-background/75`).
    backing: Color,
    /// The failed status line's ink (`text-destructive`).
    danger: Color,
}

impl ImageColors {
    /// Resolve against `theme`, falling back to beUI light.
    fn resolve(theme: Option<&Theme>) -> Self {
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                ImageColors {
                    surface: scheme.surface_container_highest,
                    ink: scheme.on_surface,
                    muted: scheme.on_surface_variant,
                    backing: scheme.surface,
                    danger: scheme.error,
                }
            }
            None => ImageColors {
                surface: FALLBACK.muted,
                ink: FALLBACK.foreground,
                muted: FALLBACK.muted_foreground,
                backing: FALLBACK.background,
                danger: FALLBACK.danger,
            },
        }
    }
}

/// The status line's style: `text-sm font-medium`.
fn status_style() -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: style::TEXT_SM as f32,
        weight: FontWeight::MEDIUM,
        ..TextStyle::default()
    }
}

/// The prompt caption's style: `text-xs text-muted-foreground`.
fn prompt_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: style::TEXT_XS as f32,
        color,
        ..TextStyle::default()
    }
}

/// The badge's style: `font-mono text-[10px] tabular-nums`.
fn badge_style(color: Color) -> TextStyle {
    TextStyle {
        family: mono_family(),
        size: IMAGE_BADGE_TEXT as f32,
        color,
        ..TextStyle::default()
    }
}

/// The smoothstep-weighted influence a dot at `distance` feels from an
/// attractor of `radius` — upstream's `proximity * proximity * (3 - 2 *
/// proximity)`.
fn dither_influence(distance: f64, radius: f64) -> f64 {
    if radius <= 0.0 {
        return 0.0;
    }
    let proximity = (1.0 - distance / radius).max(0.0);
    proximity * proximity * (3.0 - 2.0 * proximity)
}

/// The retained widget for an [`ImageGenerationView`].
pub struct ImageGenerationWidget {
    media: Option<ChildPod>,
    status: ImageGenerationStatus,
    /// The status line, and the one it is swapping away from.
    status_run: LabelRun,
    previous_status_run: LabelRun,
    prompt: Option<Label>,
    resolution: Option<Label>,
    retry: LabelRun,
    label: String,
    aspect_ratio: f64,
    size: ImageGenerationSize,
    interactive: bool,
    show_status: bool,
    /// The media's opacity and scale, each retargeted on a status change.
    media_alpha: Lane,
    media_scale: Lane,
    /// The dither field's own opacity.
    overlay: Lane,
    /// The status-line swap, `0` on the outgoing text .. `1` on the incoming.
    swap: Lane,
    /// Where the field's attractor is, in slot-local px.
    attractor: Point,
    /// Whether the attractor has been placed for the current slot size.
    placed: bool,
    /// The pointer, when one is over the slot.
    pointer: PointerTracker,
    /// The retry button's press scale.
    press: Lane,
    pressed: bool,
    captured: bool,
    /// The media slot's box, resolved by layout.
    slot: Rect,
    /// The retry button's box, resolved by layout; empty when none is offered.
    retry_box: Rect,
    on_retry: Option<ErasedCallback>,
}

impl ImageGenerationWidget {
    /// The status this card is showing.
    pub fn status(&self) -> ImageGenerationStatus {
        self.status
    }

    /// The media slot's box, as of the last layout — the space reserved before
    /// any media exists, which is what keeps the card from shifting.
    pub fn slot_box(&self) -> Rect {
        self.slot
    }

    /// The retry button's box, as of the last layout. Empty unless the run
    /// failed and a retry was offered.
    pub fn retry_box(&self) -> Rect {
        self.retry_box
    }

    /// How strongly the media is showing through, `0` hidden .. `1` revealed.
    pub fn media_progress(&self) -> f64 {
        self.media_alpha.value()
    }

    /// How strongly the dither field is covering the slot.
    pub fn overlay_progress(&self) -> f64 {
        self.overlay.value()
    }

    /// The dither attractor's current position, in slot-local px.
    pub fn attractor(&self) -> Point {
        self.attractor
    }

    /// Whether a pointer is currently over the slot, so the attractor is
    /// following it rather than drifting.
    pub fn is_tracking_pointer(&self) -> bool {
        self.interactive && self.pointer.hovered()
    }

    /// Whether a retry is on offer right now.
    pub fn offers_retry(&self) -> bool {
        self.status == ImageGenerationStatus::Error && self.on_retry.is_some()
    }

    /// Aim every status-driven lane at `status`' resting values.
    fn aim(&mut self, reduce_motion: bool) {
        let ramp = Ramp::eased(Duration::from_millis(IMAGE_MEDIA_MS), EASE_OUT);
        self.media_alpha
            .retarget_with(ramp, self.status.media_opacity());
        self.media_scale
            .retarget_with(ramp, self.status.media_scale());
        self.overlay
            .retarget_with(ramp, self.status.overlay_opacity());
        if reduce_motion {
            self.media_alpha.snap();
            self.media_scale.snap();
            self.overlay.snap();
            self.swap.snap();
        }
    }

    /// Step the attractor one frame toward wherever it is being pulled —
    /// upstream's own per-frame lerp, pointer-led when a pointer is inside and
    /// drifting on the two sine periods when not.
    fn step_attractor(&mut self, elapsed: Duration, reduce_motion: bool) {
        let (width, height) = (self.slot.width(), self.slot.height());
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let centre = Point::new(width / 2.0, height / 2.0);
        if !self.placed {
            self.attractor = centre;
            self.placed = true;
        }
        let inside_pointer = self.interactive && self.pointer.hovered() && !reduce_motion;
        let target = if inside_pointer {
            self.pointer.position()
        } else if reduce_motion {
            centre
        } else {
            let ms = elapsed.as_secs_f64() * 1000.0;
            Point::new(
                centre.x + (ms / DITHER_DRIFT_X_MS).sin() * width * DITHER_DRIFT_X,
                centre.y + (ms / DITHER_DRIFT_Y_MS).cos() * height * DITHER_DRIFT_Y,
            )
        };
        let follow = if reduce_motion {
            1.0
        } else if inside_pointer {
            DITHER_FOLLOW_POINTER
        } else {
            DITHER_FOLLOW_IDLE
        };
        self.attractor = Point::new(
            self.attractor.x + (target.x - self.attractor.x) * follow,
            self.attractor.y + (target.y - self.attractor.y) * follow,
        );
    }

    /// Paint the dot grid over the slot at `alpha`.
    fn paint_dither(&self, scene: &mut dyn PaintScene, origin: Point, ink: Color, alpha: f64) {
        let (width, height) = (self.slot.width(), self.slot.height());
        if width <= 0.0 || height <= 0.0 || alpha <= 0.0 {
            return;
        }
        let radius = width.min(height) * DITHER_RADIUS_FRACTION;
        let columns = (width / DITHER_GAP).ceil() as usize + 1;
        let rows = (height / DITHER_GAP).ceil() as usize + 1;
        let offset_x = (width - (columns as f64 - 1.0) * DITHER_GAP) / 2.0;
        let offset_y = (height - (rows as f64 - 1.0) * DITHER_GAP) / 2.0;

        for row in 0..rows {
            for column in 0..columns {
                let anchor_x = offset_x + column as f64 * DITHER_GAP;
                let anchor_y = offset_y + row as f64 * DITHER_GAP;
                let dx = anchor_x - self.attractor.x;
                let dy = anchor_y - self.attractor.y;
                let distance = (dx * dx + dy * dy).sqrt();
                let influence = dither_influence(distance, radius);
                let displacement = influence * influence * DITHER_DISPLACEMENT;
                let (dir_x, dir_y) = if distance > 0.0 {
                    (dx / distance, dy / distance)
                } else {
                    (0.0, 0.0)
                };
                let dot = DITHER_DOT_RADIUS + influence * DITHER_DOT_GROWTH;
                let dot_alpha =
                    (DITHER_ALPHA + influence as f32 * DITHER_ALPHA_GROWTH) * alpha as f32;
                scene.fill_rounded_rect(
                    Point::new(
                        origin.x + anchor_x + dir_x * displacement - dot,
                        origin.y + anchor_y + dir_y * displacement - dot,
                    ),
                    Size::new(dot * 2.0, dot * 2.0),
                    dot,
                    scale_alpha(ink, dot_alpha),
                );
            }
        }
    }

    /// Paint the status line's leading glyph: a check when complete, an alert
    /// when failed, and the turning four-dot square while work is in flight.
    fn paint_mark(&self, scene: &mut dyn PaintScene, origin: Point, ink: Color, turn: f64) {
        let centre = Point::new(
            origin.x + IMAGE_MARK_SIZE / 2.0,
            origin.y + IMAGE_MARK_SIZE / 2.0,
        );
        match self.status {
            ImageGenerationStatus::Complete => {
                let points = [
                    (origin.x + 2.5, origin.y + 7.5),
                    (origin.x + 5.5, origin.y + 10.5),
                    (origin.x + 11.5, origin.y + 4.0),
                ];
                let mut path = kurbo::BezPath::new();
                path.move_to(Point::new(points[0].0, points[0].1));
                path.line_to(Point::new(points[1].0, points[1].1));
                path.line_to(Point::new(points[2].0, points[2].1));
                scene.stroke_path(Point::ORIGIN, &path, 1.6, &Brush::Solid(ink));
            }
            ImageGenerationStatus::Error => {
                let ring = Arc::new(centre, Vec2::new(6.0, 6.0), 0.0, TAU, 0.0);
                scene.stroke_path(
                    Point::ORIGIN,
                    &ring.to_path(style::PATH_TOLERANCE),
                    1.4,
                    &Brush::Solid(ink),
                );
                scene.stroke_line(
                    Point::new(centre.x, centre.y - 3.2),
                    Point::new(centre.x, centre.y + 1.0),
                    1.6,
                    ink,
                );
                scene.fill_rounded_rect(
                    Point::new(centre.x - 0.8, centre.y + 2.4),
                    Size::new(1.6, 1.6),
                    0.8,
                    ink,
                );
            }
            _ => {
                // Four squares on a 2x2 grid, the pair on one diagonal dimmer,
                // the whole cluster turning once per `IMAGE_MARK_SPIN_MS`.
                let angle = turn * TAU;
                let (sin, cos) = angle.sin_cos();
                let arm = 2.5;
                let dot = 4.0;
                for (index, (ox, oy)) in [(-arm, -arm), (arm, -arm), (-arm, arm), (arm, arm)]
                    .into_iter()
                    .enumerate()
                {
                    let x = centre.x + ox * cos - oy * sin;
                    let y = centre.y + ox * sin + oy * cos;
                    let faint = index == 1 || index == 2;
                    let color = if faint { scale_alpha(ink, 0.55) } else { ink };
                    scene.fill_rounded_rect(
                        Point::new(x - dot / 2.0, y - dot / 2.0),
                        Size::new(dot, dot),
                        1.0,
                        color,
                    );
                }
            }
        }
    }

    /// The width the card lets itself occupy inside `available`.
    fn card_width(&self, available: f64) -> f64 {
        match self.size {
            ImageGenerationSize::Compact => available.min(IMAGE_COMPACT_WIDTH),
            ImageGenerationSize::Fluid => available,
        }
    }
}

impl<State: 'static> View<State> for ImageGenerationView<State> {
    type Element = ImageGenerationWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ImageGenerationWidget {
        let status_text = self.resolved_status_text();
        ImageGenerationWidget {
            media: self.media.as_ref().map(|view| build_child(view, ctx)),
            status: self.status,
            status_run: LabelRun::new(status_text.clone()),
            previous_status_run: LabelRun::new(status_text),
            prompt: self.prompt.clone().map(Label::new),
            resolution: self.resolution.clone().map(Label::new),
            retry: LabelRun::new(IMAGE_RETRY_LABEL),
            label: self.resolved_label(),
            aspect_ratio: self.aspect_ratio,
            size: self.size,
            interactive: self.interactive,
            show_status: self.show_status,
            media_alpha: Lane::at_rest(
                Ramp::eased(Duration::from_millis(IMAGE_MEDIA_MS), EASE_OUT),
                self.status.media_opacity(),
            ),
            media_scale: Lane::at_rest(
                Ramp::eased(Duration::from_millis(IMAGE_MEDIA_MS), EASE_OUT),
                self.status.media_scale(),
            ),
            overlay: Lane::at_rest(
                Ramp::eased(Duration::from_millis(IMAGE_MEDIA_MS), EASE_OUT),
                self.status.overlay_opacity(),
            ),
            swap: Lane::at_rest(
                Ramp::eased(Duration::from_millis(IMAGE_STATUS_SWAP_MS), EASE_OUT),
                1.0,
            ),
            attractor: Point::ORIGIN,
            placed: false,
            pointer: PointerTracker::new(),
            press: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            pressed: false,
            captured: false,
            slot: Rect::ZERO,
            retry_box: Rect::ZERO,
            on_retry: self.on_retry.as_ref().map(erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ImageGenerationWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        match (&prev.media, &self.media, &mut element.media) {
            (Some(old), Some(new), Some(pod)) => flags |= rebuild_child(old, new, pod, ctx),
            (None, Some(new), slot @ None) => {
                *slot = Some(build_child(new, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(old), None, slot) => {
                if let Some(pod) = slot {
                    teardown_child(old, pod, ctx);
                }
                *slot = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            _ => {}
        }

        if element.status != self.status {
            element.status = self.status;
            flags |= ChangeFlags::PAINT;
            // The retry button appears and disappears with the error status, so
            // the card's own height changes with it.
            flags |= ChangeFlags::LAYOUT;
        }

        let status_text = self.resolved_status_text();
        if element.status_run.content() != status_text {
            element
                .previous_status_run
                .set_content(element.status_run.content().to_owned());
            element.status_run.set_content(status_text);
            element.swap = Lane::at_rest(
                Ramp::eased(Duration::from_millis(IMAGE_STATUS_SWAP_MS), EASE_OUT),
                0.0,
            );
            element.swap.retarget(1.0);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.prompt != self.prompt {
            element.prompt = self.prompt.clone().map(Label::new);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.resolution != self.resolution {
            element.resolution = self.resolution.clone().map(Label::new);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.aspect_ratio != self.aspect_ratio
            || element.size != self.size
            || element.show_status != self.show_status
        {
            element.aspect_ratio = self.aspect_ratio;
            element.size = self.size;
            element.show_status = self.show_status;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.interactive = self.interactive;
        element.label = self.resolved_label();
        element.on_retry = self.on_retry.as_ref().map(erase_callback);
        flags
    }

    fn teardown(&self, element: &mut ImageGenerationWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(view), Some(pod)) = (&self.media, &mut element.media) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for ImageGenerationWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let available = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let width = self.card_width(available);
        let slot_height = width / self.aspect_ratio;
        let left = (available - width) / 2.0;
        self.slot = Rect::from_origin_size(Point::new(left, 0.0), Size::new(width, slot_height));

        if let Some(media) = &mut self.media {
            media.layout_child(ctx, &BoxConstraints::tight(self.slot.size()));
            media.set_origin(self.slot.origin());
        }

        let mut height = slot_height;
        let status_style = status_style();
        self.status_run.layout(ctx, &status_style);
        self.previous_status_run.layout(ctx, &status_style);
        if self.show_status {
            height += IMAGE_STATUS_GAP + self.status_run.size().height.max(IMAGE_STATUS_HEIGHT);
        }
        if let Some(prompt) = &mut self.prompt {
            let measured = prompt.layout(ctx, &prompt_style(Color::BLACK));
            height += if self.show_status {
                IMAGE_PROMPT_GAP
            } else {
                IMAGE_STATUS_GAP
            } + measured.height;
        }
        if let Some(resolution) = &mut self.resolution {
            resolution.layout(ctx, &badge_style(Color::BLACK));
        }

        self.retry.layout(ctx, &status_style);
        self.retry_box = if self.offers_retry() {
            let button_width = self.retry.size().width
                + IMAGE_RETRY_PADDING_X * 2.0
                + style::ICON_SIZE
                + style::GAP_MD;
            let box_top = height + IMAGE_STATUS_GAP;
            height = box_top + IMAGE_RETRY_HEIGHT;
            Rect::from_origin_size(
                Point::new(left, box_top),
                Size::new(button_width, IMAGE_RETRY_HEIGHT),
            )
        } else {
            Rect::ZERO
        };

        bc.constrain(Size::new(available, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (reduce_motion, colors) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                ImageColors::resolve(theme),
            )
        };
        let now = ctx.frame_time();
        let origin = ctx.origin();
        self.aim(reduce_motion);
        self.pointer.sync_hovered(ctx.is_hovered());

        let mut owes_frame = self.media_alpha.advance(now);
        owes_frame |= self.media_scale.advance(now);
        owes_frame |= self.overlay.advance(now);
        owes_frame |= self.swap.advance(now);
        owes_frame |= self.press.advance(now);

        let slot_origin = Point::new(origin.x + self.slot.x0, origin.y + self.slot.y0);
        let radius = style::resolve_radius(IMAGE_RADIUS, self.slot.width(), self.slot.height());
        scene.fill_rounded_rect(slot_origin, self.slot.size(), radius, colors.surface);

        // Everything inside the slot is clipped to it — upstream's
        // `overflow-hidden`, and what keeps a displaced dot from spilling.
        scene.push_clip_rounded(slot_origin, self.slot.size(), radius);
        if let Some(media) = &mut self.media {
            let alpha = self.media_alpha.value().clamp(0.0, 1.0);
            if alpha > 0.0 {
                let scale = self.media_scale.value();
                let centre = Point::new(
                    slot_origin.x + self.slot.width() / 2.0,
                    slot_origin.y + self.slot.height() / 2.0,
                );
                scene.push_transform(
                    kurbo::Affine::translate((centre.x, centre.y))
                        * kurbo::Affine::scale(scale)
                        * kurbo::Affine::translate((-centre.x, -centre.y)),
                );
                scene.push_layer(slot_origin, self.slot.size(), alpha as f32);
                media.paint_child(ctx, scene);
                scene.pop_layer();
                scene.pop_transform();
            }
        }

        let overlay = self.overlay.value().clamp(0.0, 1.0);
        if overlay > 0.0 {
            let elapsed = Duration::from_nanos(now.as_nanos());
            self.step_attractor(elapsed, reduce_motion);
            self.paint_dither(scene, slot_origin, colors.ink, overlay);
            if !reduce_motion {
                // A perpetual decorative loop, never the unpaced transition
                // class.
                ctx.request_frame_paced();
            }
        }
        scene.pop_clip();

        // The resolution badge, above everything in the slot.
        if let Some(resolution) = &self.resolution {
            let size = resolution.size();
            let badge = Size::new(
                size.width + IMAGE_BADGE_PADDING_X * 2.0,
                size.height + IMAGE_BADGE_PADDING_Y * 2.0,
            );
            let at = Point::new(
                slot_origin.x + self.slot.width() - IMAGE_BADGE_INSET - badge.width,
                slot_origin.y + IMAGE_BADGE_INSET,
            );
            scene.fill_rounded_rect(
                at,
                badge,
                badge.height / 2.0,
                with_alpha(colors.backing, BADGE_BACKING_ALPHA),
            );
            resolution.paint(
                Point::new(at.x + IMAGE_BADGE_PADDING_X, at.y + IMAGE_BADGE_PADDING_Y),
                scene,
            );
        }

        let mut y = origin.y + self.slot.height();
        if self.show_status {
            y += IMAGE_STATUS_GAP;
            let ink = if self.status == ImageGenerationStatus::Error {
                colors.danger
            } else {
                colors.ink
            };
            let turn = if reduce_motion || !self.status.is_active() {
                0.0
            } else {
                let ms = now.as_nanos() as f64 / 1_000_000.0;
                let cycle = ms / IMAGE_MARK_SPIN_MS as f64;
                EASE_IN_OUT.transform(cycle - cycle.floor())
            };
            let mark_origin = Point::new(
                origin.x + self.slot.x0,
                y + (IMAGE_STATUS_HEIGHT - IMAGE_MARK_SIZE) / 2.0,
            );
            self.paint_mark(scene, mark_origin, ink, turn);
            if self.status.is_active() && !reduce_motion {
                ctx.request_frame_paced();
            }

            let text_x = origin.x + self.slot.x0 + IMAGE_MARK_SIZE + IMAGE_STATUS_INNER_GAP;
            let swap = self.swap.value().clamp(0.0, 1.0);
            let travel = IMAGE_STATUS_SWAP_SHIFT;
            if swap < 1.0 {
                self.previous_status_run.paint(
                    Point::new(text_x, y - travel * swap),
                    scale_alpha(ink, (1.0 - swap) as f32),
                    scene,
                );
            }
            self.status_run.paint(
                Point::new(text_x, y + travel * (1.0 - swap)),
                scale_alpha(ink, swap as f32),
                scene,
            );
            y += self.status_run.size().height.max(IMAGE_STATUS_HEIGHT);
        }

        if let Some(prompt) = &self.prompt {
            y += if self.show_status {
                IMAGE_PROMPT_GAP
            } else {
                IMAGE_STATUS_GAP
            };
            prompt.paint(Point::new(origin.x + self.slot.x0, y), scene);
        }

        if self.offers_retry() {
            let at = Point::new(origin.x + self.retry_box.x0, origin.y + self.retry_box.y0);
            let size = self.retry_box.size();
            let scale = crate::press::press_scale(style::PRESS_SCALE, self.press.value());
            let centre = Point::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
            scene.push_transform(
                kurbo::Affine::translate((centre.x, centre.y))
                    * kurbo::Affine::scale(scale)
                    * kurbo::Affine::translate((-centre.x, -centre.y)),
            );
            if self.pressed {
                scene.fill_rounded_rect(
                    at,
                    size,
                    style::resolve_radius(style::RADIUS_CONTROL, size.width, size.height),
                    colors.surface,
                );
            }
            // The rewind glyph: three quarters of a ring with a tick head.
            let glyph = Point::new(
                at.x + IMAGE_RETRY_PADDING_X + style::ICON_SIZE / 2.0,
                at.y + size.height / 2.0,
            );
            let ring = Arc::new(glyph, Vec2::new(6.0, 6.0), 0.6, TAU * 0.8, 0.0);
            scene.stroke_path(
                Point::ORIGIN,
                &ring.to_path(style::PATH_TOLERANCE),
                1.6,
                &Brush::Solid(colors.ink),
            );
            self.retry.paint(
                Point::new(
                    at.x + IMAGE_RETRY_PADDING_X + style::ICON_SIZE + style::GAP_MD,
                    at.y + (size.height - self.retry.size().height) / 2.0,
                ),
                colors.ink,
                scene,
            );
            scene.pop_transform();
        }

        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            if let Some(media) = &mut self.media {
                media.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        // The slot's own content may be interactive; the card only owns the
        // retry button and the dither field's attractor.
        if !self.captured
            && let Some(media) = &mut self.media
            && route_event_single(media, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        // The attractor tracks the pointer in slot-local coordinates.
        let local = frust::authoring::PointerEvent {
            phase: p.phase,
            position: Point::new(p.position.x - self.slot.x0, p.position.y - self.slot.y0),
            button: p.button,
        };
        if self.pointer.on_pointer(&local, self.slot.size()) {
            ctx.request_redraw();
        }

        let hits_retry = self.offers_retry() && self.retry_box.contains(p.position);
        match p.phase {
            PointerPhase::Down if hits_retry && presses(p) => {
                self.pressed = true;
                self.captured = true;
                self.press.retarget(1.0);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if hits_retry {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                // The field's attractor follows a live pointer, so the card
                // claims hover for the whole slot, not just the button — the
                // claim is what makes `PaintCtx::is_hovered` (the paint-time
                // authority the tracker re-syncs from) answer true.
                if hits_retry || self.slot.contains(p.position) {
                    ctx.claim_hover();
                }
                if self.captured && self.pressed != hits_retry {
                    self.pressed = hits_retry;
                    self.press.retarget(if hits_retry { 1.0 } else { 0.0 });
                    ctx.request_redraw();
                }
                if self.captured {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            PointerPhase::Up if self.captured => {
                let fired = self.pressed && hits_retry;
                self.pressed = false;
                self.captured = false;
                self.press.retarget(0.0);
                if fired && let Some(callback) = &mut self.on_retry {
                    callback(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel if self.captured => {
                self.pressed = false;
                self.captured = false;
                self.press.retarget(0.0);
                ctx.request_redraw();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Image,
            |node| {
                node.set_label(self.label.clone());
                if self.status.is_active() {
                    // The busy half of upstream's `aria-busy`; the label already
                    // carries which phase the run is in.
                    node.set_busy();
                }
            },
            |ctx| {
                if let Some(media) = &self.media {
                    media.semantics_child(ctx);
                }
                if self.offers_retry() {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(IMAGE_RETRY_LABEL);
                        node.add_action(Action::Click);
                    });
                }
            },
        );
    }

    visit_children!(media);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Affine, BezPath, PointerButton, PointerEvent};
    use frust::{FrameTime, text};
    use std::any::Any;

    /// The row every card test lays itself into.
    const ROW: Size = Size::new(320.0, 500.0);

    /// Records the primitives the card paints.
    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        strokes: usize,
        lines: usize,
        layers: Vec<f32>,
        clips: usize,
        transforms: Vec<Affine>,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
            self.rounded.push((origin, size, color));
        }
        fn stroke_line(&mut self, _p0: Point, _p1: Point, _width: f64, _color: Color) {
            self.lines += 1;
        }
        fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, _brush: &Brush) {
            self.strokes += 1;
        }
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_clip(&mut self, _origin: Point, _size: Size) {
            self.clips += 1;
        }
        fn push_clip_rounded(&mut self, _origin: Point, _size: Size, _radius: f64) {
            self.clips += 1;
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    /// What a retry press reports.
    #[derive(Default)]
    struct Retried(u32);

    fn at(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    fn build(view: &ImageGenerationView<Retried>) -> ImageGenerationWidget {
        let mut next_id = 0u64;
        View::<Retried>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    fn layout(widget: &mut ImageGenerationWidget) -> Size {
        let mut text_ctx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut ctx, &BoxConstraints::loose(ROW))
    }

    fn laid_out(view: &ImageGenerationView<Retried>) -> (ImageGenerationWidget, Size) {
        let mut widget = build(view);
        let size = layout(&mut widget);
        (widget, size)
    }

    /// Paint at `ms`, returning what was drawn plus both frame requests.
    fn painted(
        widget: &mut ImageGenerationWidget,
        size: Size,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, at(ms));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame(), ctx.needs_frame_paced_only())
    }

    fn rebuild(
        widget: &mut ImageGenerationWidget,
        from: &ImageGenerationView<Retried>,
        to: &ImageGenerationView<Retried>,
    ) {
        let mut next_id = 0u64;
        View::<Retried>::rebuild(to, from, widget, &mut BuildCtx::new(&mut next_id));
    }

    fn dispatch(
        widget: &mut ImageGenerationWidget,
        size: Size,
        phase: PointerPhase,
        position: Point,
        state: &mut Retried,
    ) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ORIGIN, size);
        widget.event(
            &mut ctx,
            &InputEvent::Pointer(PointerEvent {
                phase,
                position,
                button: PointerButton::Primary,
            }),
        );
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// Upstream's three per-status tables, transcribed value for value: a
    /// mis-typed row here is the one defect no visual test would catch.
    #[test]
    fn every_status_carries_upstreams_own_media_overlay_and_text_tables() {
        assert_eq!(ImageGenerationStatus::ALL.len(), 5);
        let table = [
            (
                ImageGenerationStatus::Queued,
                0.0,
                1.02,
                1.0,
                "Waiting to generate",
            ),
            (
                ImageGenerationStatus::Generating,
                0.0,
                1.015,
                1.0,
                "Generating image",
            ),
            (
                ImageGenerationStatus::Refining,
                0.62,
                1.005,
                0.48,
                "Refining details",
            ),
            (
                ImageGenerationStatus::Complete,
                1.0,
                1.0,
                0.0,
                "Image ready",
            ),
            (
                ImageGenerationStatus::Error,
                0.28,
                1.0,
                0.0,
                "Generation failed",
            ),
        ];
        for (status, opacity, scale, overlay, text) in table {
            assert_eq!(status.media_opacity(), opacity, "{status:?} opacity");
            assert_eq!(status.media_scale(), scale, "{status:?} scale");
            assert_eq!(status.overlay_opacity(), overlay, "{status:?} overlay");
            assert_eq!(status.status_text(), text, "{status:?} text");
        }
        // Exactly the three phases upstream calls `active` keep the field up.
        let active: Vec<_> = ImageGenerationStatus::ALL
            .into_iter()
            .filter(|s| s.is_active())
            .collect();
        assert_eq!(
            active,
            vec![
                ImageGenerationStatus::Queued,
                ImageGenerationStatus::Generating,
                ImageGenerationStatus::Refining
            ]
        );
    }

    /// The whole point of the component: the slot reserves its box before any
    /// media exists, so delivering the image shifts nothing.
    #[test]
    fn the_slot_reserves_its_aspect_before_any_media_arrives() {
        let empty = image_generation::<Retried>();
        let (widget, without) = laid_out(&empty);
        assert_eq!(
            widget.slot_box().width(),
            IMAGE_COMPACT_WIDTH,
            "`compact` caps at max-w-52"
        );
        assert_eq!(widget.slot_box().height(), IMAGE_COMPACT_WIDTH, "1 / 1");

        let filled = image_generation::<Retried>().media(text("image"));
        let (_, with) = laid_out(&filled);
        assert_eq!(with, without, "delivering the media shifts nothing");

        // A different ratio reserves a different box, and `fluid` fills the row.
        let wide = image_generation::<Retried>().aspect_ratio(16.0 / 9.0);
        let (widget, _) = laid_out(&wide);
        assert!(widget.slot_box().height() < widget.slot_box().width());
        let fluid = image_generation::<Retried>().size(ImageGenerationSize::Fluid);
        let (widget, _) = laid_out(&fluid);
        assert_eq!(widget.slot_box().width(), ROW.width);
    }

    /// The field covers a running slot, clears on completion, and asks for its
    /// frames on the **paced** cosmetic-loop class rather than the unpaced one.
    #[test]
    fn the_dither_field_covers_a_running_slot_and_clears_when_complete() {
        let running = image_generation::<Retried>();
        let (mut widget, size) = laid_out(&running);
        let (rec, needs_frame, paced_only) = painted(&mut widget, size, 0, None);
        // One rounded rect is the slot backing; the rest are dots.
        assert!(
            rec.rounded.len() > 100,
            "the grid is painted: {}",
            rec.rounded.len()
        );
        assert!(needs_frame && paced_only, "a decorative loop is paced");
        assert_eq!(widget.overlay_progress(), 1.0);
        assert!(rec.clips > 0, "the field is clipped to the slot");

        let done = image_generation::<Retried>().status(ImageGenerationStatus::Complete);
        rebuild(&mut widget, &running, &done);
        painted(&mut widget, size, 10, None);
        painted(&mut widget, size, 10 + IMAGE_MEDIA_MS, None);
        assert_eq!(widget.overlay_progress(), 0.0, "the field has cleared");
        let (rec, _, _) = painted(&mut widget, size, 10 + IMAGE_MEDIA_MS + 1, None);
        assert!(
            rec.rounded.len() < 8,
            "a complete card paints no dots: {}",
            rec.rounded.len()
        );
    }

    /// The attractor drifts on its own while nothing is hovering, and closes on
    /// a pointer that enters the slot.
    #[test]
    fn the_attractor_drifts_while_idle_and_follows_a_pointer_inside() {
        let view = image_generation::<Retried>();
        let (mut widget, size) = laid_out(&view);
        painted(&mut widget, size, 0, None);
        let centre = widget.attractor();
        painted(&mut widget, size, 400, None);
        painted(&mut widget, size, 800, None);
        let drifted = widget.attractor();
        assert!(
            (drifted.x - centre.x).abs() > 1e-6 || (drifted.y - centre.y).abs() > 1e-6,
            "an idle field drifts: {centre:?} -> {drifted:?}"
        );

        // A pointer inside latches onto the tracker...
        let mut state = Retried::default();
        let corner = Point::new(widget.slot_box().x0 + 8.0, widget.slot_box().y0 + 8.0);
        dispatch(&mut widget, size, PointerPhase::Move, corner, &mut state);
        assert!(widget.is_tracking_pointer());

        // ...and the attractor then closes on it. Stepped directly, because a
        // test `PaintCtx` reports `is_hovered() == false` and paint re-syncs the
        // latch from that authority (the same seam `tilt_card` documents), so a
        // paint here would drop the pointer before the step.
        for _ in 0..12 {
            widget.step_attractor(Duration::from_millis(900), false);
        }
        let pulled = widget.attractor();
        assert!(
            pulled.x < drifted.x && pulled.y < drifted.y,
            "the attractor closed on the pointer: {pulled:?}"
        );
        assert!(pulled.x > 0.0 && pulled.y > 0.0, "and not past it");

        // An uninteractive card ignores the pointer entirely.
        let inert = image_generation::<Retried>().interactive(false);
        let (mut widget, size) = laid_out(&inert);
        painted(&mut widget, size, 0, None);
        dispatch(&mut widget, size, PointerPhase::Move, corner, &mut state);
        assert!(
            !widget.is_tracking_pointer(),
            "no pointer pull without `interactive`"
        );
        let idle = widget.attractor();
        widget.step_attractor(Duration::ZERO, false);
        assert!(
            (widget.attractor().x - idle.x).abs() < IMAGE_COMPACT_WIDTH,
            "it keeps drifting from the centre instead"
        );
    }

    /// The media crossfades in over a status change and lands on the status'
    /// own resting opacity.
    #[test]
    fn the_media_reveals_across_a_status_change() {
        let refining = image_generation::<Retried>()
            .media(text("image"))
            .status(ImageGenerationStatus::Refining);
        let (mut widget, size) = laid_out(&refining);
        painted(&mut widget, size, 0, None);
        assert_eq!(
            widget.media_progress(),
            ImageGenerationStatus::Refining.media_opacity()
        );

        let complete = image_generation::<Retried>()
            .media(text("image"))
            .status(ImageGenerationStatus::Complete);
        rebuild(&mut widget, &refining, &complete);
        painted(&mut widget, size, 0, None);
        let (rec, needs_frame, _) = painted(&mut widget, size, IMAGE_MEDIA_MS / 2, None);
        assert!(needs_frame, "the reveal owes frames");
        let mid = widget.media_progress();
        assert!(mid > 0.62 && mid < 1.0, "mid-reveal: {mid}");
        assert!(
            rec.layers.iter().any(|alpha| *alpha < 1.0),
            "the media composites through a partial layer"
        );
        assert!(
            !rec.transforms.is_empty(),
            "and scales about the slot's centre"
        );

        painted(&mut widget, size, IMAGE_MEDIA_MS, None);
        assert_eq!(widget.media_progress(), 1.0);
    }

    /// `reduce_motion`: the field is painted from its resting attractor, the
    /// media sits at rest, and **nothing** is requested — upstream stops its own
    /// animation frame under the same flag.
    #[test]
    fn reduce_motion_paints_the_field_from_rest_and_asks_for_nothing() {
        let theme = reduced();
        let running = image_generation::<Retried>();
        let (mut widget, size) = laid_out(&running);
        let (rec, needs_frame, _) = painted(&mut widget, size, 0, Some(&theme));
        assert!(!needs_frame, "a reduced field is static");
        assert!(rec.rounded.len() > 100, "but it is still painted");
        let resting = widget.attractor();
        painted(&mut widget, size, 5_000, Some(&theme));
        assert_eq!(widget.attractor(), resting, "and it does not drift");

        let done = image_generation::<Retried>().status(ImageGenerationStatus::Complete);
        rebuild(&mut widget, &running, &done);
        let (_, needs_frame, _) = painted(&mut widget, size, 5_001, Some(&theme));
        assert_eq!(widget.overlay_progress(), 0.0, "the swap lands at once");
        assert!(!needs_frame);
    }

    /// A failed run offers a retry, and the retry fires on up-inside only.
    #[test]
    fn a_failed_run_offers_a_retry_that_fires_on_up_inside() {
        let failed = image_generation::<Retried>()
            .status(ImageGenerationStatus::Error)
            .on_retry(|state: &mut Retried| state.0 += 1);
        let (mut widget, size) = laid_out(&failed);
        assert!(widget.offers_retry());
        let button = widget.retry_box();
        assert!(button.width() > 0.0 && button.height() == IMAGE_RETRY_HEIGHT);

        let mut state = Retried::default();
        let inside_button = Point::new(button.x0 + 4.0, button.y0 + button.height() / 2.0);
        dispatch(
            &mut widget,
            size,
            PointerPhase::Down,
            inside_button,
            &mut state,
        );
        dispatch(
            &mut widget,
            size,
            PointerPhase::Up,
            inside_button,
            &mut state,
        );
        assert_eq!(state.0, 1, "an up-inside retries");

        // A release outside the button does not.
        dispatch(
            &mut widget,
            size,
            PointerPhase::Down,
            inside_button,
            &mut state,
        );
        dispatch(
            &mut widget,
            size,
            PointerPhase::Up,
            Point::new(button.x0 + 4.0, button.y1 + 40.0),
            &mut state,
        );
        assert_eq!(state.0, 1, "an up-outside does not");

        // No handler, no button — upstream gates on `status === "error" &&
        // onRetry`.
        let silent = image_generation::<Retried>().status(ImageGenerationStatus::Error);
        let (widget, _) = laid_out(&silent);
        assert!(!widget.offers_retry());
        assert_eq!(widget.retry_box(), Rect::ZERO);
    }

    /// The status line swaps when the status does, and the swap settles.
    #[test]
    fn the_status_line_swaps_when_the_status_changes() {
        let generating = image_generation::<Retried>();
        let (mut widget, size) = laid_out(&generating);
        painted(&mut widget, size, 0, None);

        let refining = image_generation::<Retried>().status(ImageGenerationStatus::Refining);
        rebuild(&mut widget, &generating, &refining);
        assert_eq!(widget.status(), ImageGenerationStatus::Refining);
        painted(&mut widget, size, 0, None);
        let (_, needs_frame, _) = painted(&mut widget, size, IMAGE_STATUS_SWAP_MS / 2, None);
        assert!(needs_frame);
        painted(&mut widget, size, IMAGE_STATUS_SWAP_MS, None);

        // A caller-supplied line wins over the table, as upstream's
        // `statusText ?? STATUS_TEXT[status]` does.
        let custom = image_generation::<Retried>().status_text("Almost there");
        assert_eq!(custom.resolved_status_text(), "Almost there");
        assert_eq!(
            image_generation::<Retried>()
                .prompt("a cat")
                .resolved_label(),
            "Generating image: a cat",
            "the accessible name derives from the status and the prompt"
        );
        assert_eq!(
            image_generation::<Retried>()
                .label("custom")
                .resolved_label(),
            "custom"
        );
    }

    /// The falloff is a smoothstep over the attractor's radius: full at the
    /// centre, zero at the edge, and never negative outside it.
    #[test]
    fn dither_influence_is_a_smoothstep_falloff() {
        assert_eq!(dither_influence(0.0, 50.0), 1.0);
        assert_eq!(dither_influence(50.0, 50.0), 0.0);
        assert_eq!(dither_influence(500.0, 50.0), 0.0, "clamped outside");
        let half = dither_influence(25.0, 50.0);
        assert!((half - 0.5).abs() < 1e-9, "smoothstep is symmetric: {half}");
        // A degenerate field influences nothing rather than dividing by zero.
        assert_eq!(dither_influence(1.0, 0.0), 0.0);
    }

    /// The card publishes its media pod, so an inspector sees the slot's
    /// content.
    #[test]
    fn the_card_publishes_its_media_pod() {
        let (widget, _) = laid_out(&image_generation::<Retried>().media(text("image")));
        let mut seen = 0;
        Widget::visit_children(&widget, &mut |_| seen += 1);
        assert_eq!(seen, 1);

        let (widget, _) = laid_out(&image_generation::<Retried>());
        let mut seen = 0;
        Widget::visit_children(&widget, &mut |_| seen += 1);
        assert_eq!(seen, 0, "an empty slot publishes nothing");
    }

    /// The palette resolves through the scheme when themed and falls back to
    /// beUI light when not.
    #[test]
    fn the_palette_resolves_through_the_theme_or_falls_back_to_beui_light() {
        let bare = ImageColors::resolve(None);
        assert_eq!(bare.surface, FALLBACK.muted);
        assert_eq!(bare.danger, FALLBACK.danger);

        let theme = crate::theme();
        let themed = ImageColors::resolve(Some(&theme));
        assert_eq!(themed.ink, theme.scheme().on_surface);
        assert_eq!(themed.danger, theme.scheme().error);
    }
}
