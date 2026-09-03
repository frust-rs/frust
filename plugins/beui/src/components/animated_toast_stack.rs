//! Ports beUI's `animated-toast-stack` component.
//!
//! **Source:** `components/motion/animated-toast-stack.tsx` of the beUI
//! monorepo, rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved
//! 2026-09-01, with the collapsed-stack geometry taken from the catalog's other
//! stacking surface, `components/motion/notification-stack.tsx` (same rev) — see
//! *Premise correction* below.
//!
//! | upstream | here |
//! |---|---|
//! | `STACK_SPRING = {stiffness: 420, damping: 34, mass: 0.75}` | [`AnimatedToastStackSpring`](TOAST_STACK_SPRING) |
//! | item `initial={{opacity:0, y:22, scale:0.96}}` | [`TOAST_STACK_ENTER_LIFT`], [`TOAST_STACK_ENTER_SCALE`] |
//! | item `exit={{opacity:0, x:32, scale:0.96, 0.18s EASE_OUT}}` | [`TOAST_STACK_EXIT_SLIDE`], [`TOAST_STACK_EXIT_SCALE`], [`TOAST_STACK_EXIT`] |
//! | `drag="x"`, `dragElastic={0.18}`, dismiss past `offset.x > 72` | [`TOAST_STACK_DRAG_ELASTIC`], [`TOAST_STACK_SWIPE_DISMISS`] |
//! | `maxVisible = 4`, `toasts.slice(-maxVisible)` | [`TOAST_STACK_MAX_VISIBLE`] |
//! | surface `rounded-2xl border border-border bg-card/95 p-3 shadow-2xl` | [`TOAST_STACK_RADIUS`], [`TOAST_STACK_PADDING`], [`TOAST_STACK_SURFACE_ALPHA`], the `shadow-2xl` recipe |
//! | `w-[calc(100vw-2rem)] max-w-sm gap-2` | [`TOAST_STACK_MAX_WIDTH`], [`TOAST_STACK_GAP`], [`TOAST_STACK_MARGIN`] |
//! | icon wrap `h-7 w-7 rounded-full`, row `gap-3` | [`TOAST_STACK_ICON_BOX`], [`TOAST_STACK_ICON_GAP`] |
//! | title `text-sm font-medium`, description `mt-0.5 text-xs` | [`crate::style::TEXT_SM`] / [`crate::style::TEXT_XS`], [`TOAST_STACK_TITLE_GAP`] |
//! | action `mt-2 h-7 rounded-full bg-primary/[0.06] px-3 text-xs` | [`TOAST_STACK_ACTION_GAP`], [`TOAST_STACK_ACTION_HEIGHT`], [`TOAST_STACK_ACTION_WASH`] |
//! | `POSITION_CLASS` (`left-4 top-4` … `bottom-6 right-4`) | [`AnimatedToastStackPosition`], [`TOAST_STACK_MARGIN`], [`TOAST_STACK_MARGIN_BOTTOM`] |
//! | `STATUS_CLASS` (`text-primary bg-primary/10`, …) | [`AnimatedToastStackStatus`]'s resolved inks and washes |
//! | collapsed peek `y: index * 8`, inset `index * 12` (`notification-stack.tsx`) | [`TOAST_STACK_PEEK`], [`TOAST_STACK_INSET`] |
//!
//! # Premise correction: upstream's own file is a gap-separated list
//!
//! The porting card describes this slug as a real depth stack — "toasts layer
//! with depth offsets, a new toast pushes existing ones back, hover expands the
//! stack into a list". `animated-toast-stack.tsx` itself is **not** that: it is
//! an `<ol>` of `motion.li`s in a `flex-col`/`flex-col-reverse` with `gap-2`,
//! every card at full width, with no depth offset, no hover expansion and no
//! collapsed state at all. The only stacking behaviour in the upstream catalog
//! lives in `notification-stack.tsx`, which collapses its cards onto one grid
//! cell with `y: index * STACK_PEEK` and `clipPath: inset(0 index * STACK_INSET)`
//! and expands them into a column when opened.
//!
//! This port builds the card's stack and takes the two numbers from the source
//! that authors them, so nothing here is invented: **collapsed** is
//! `notification-stack.tsx`'s peek/inset geometry, and **expanded** is exactly
//! `animated-toast-stack.tsx`'s own `gap-2` column — the hover expansion is
//! therefore a move *between two shapes upstream ships*, not a third one. Every
//! other value (surface, padding, springs, entrance, exit, swipe, statuses)
//! comes from `animated-toast-stack.tsx` unchanged.
//!
//! One divergence is deliberate: upstream paints `zIndex: 20 - index` over a
//! queue ordered oldest-first, so its *oldest* card is on top. The card's stack
//! requires the opposite — a new toast pushes the others back — so **depth 0 is
//! the newest toast**, nearest the anchored edge, and older ones recede.
//!
//! # Where it mounts
//!
//! Neither [`crate::overlay`] host: like `frust_glyph`'s toast host and the
//! `huddle` sample's app toast, this widget **is** the top layer. It fills the
//! area it is given and positions its own cards against
//! [`AnimatedToastStackPosition`]'s corner, widened by the window's own safe-area
//! padding, so an app mounts it as the top child of a full-area [`frust::Stack`]
//! and needs no `Align` around it. It stays event-transparent everywhere its
//! cards are not (upstream's `pointer-events-none` root over `pointer-events-auto`
//! items), so the screen underneath keeps receiving presses.
//!
//! # Controlled, and kept mounted for the exit
//!
//! The queue belongs to the owner: this widget never removes a toast, it reports
//! a dismissal through `on_dismiss` and the owner drops the entry. A slot whose
//! id disappears from the view is *not* dropped on that frame — it is put into
//! [`crate::motion::Presence`]'s exit and stays laid out until the ramp settles,
//! which is what makes the exit visible at all (`crate::motion::presence`'s
//! kept-mounted contract). While a card exits it keeps its depth, and the cards
//! around it spring to their new places on [`TOAST_STACK_SPRING`] — the
//! "neighbours re-spring" half of the choreography.
//!
//! # Degradations against the web original
//!
//! - **No timer.** Upstream's auto-dismiss lives in its `useAnimatedToastStack`
//!   *hook*, not in the component, and it stays with the owner here for the same
//!   reason plus a harder one: a countdown expiring during `paint` has no route
//!   to `&mut State` — the facade publishes no housekeeping-flush seam — so a
//!   widget-side timer could only fire on the user's next input event, which is
//!   exactly when a toast must *not* need one. `duration` is therefore not
//!   carried as a field; an owner drives its own timeout and drops the entry,
//!   and the exit plays from there.
//! - **Text, not nodes.** Upstream's `title`/`description`/`icon`/`action.label`
//!   are `ReactNode`, with a `renderToast` escape hatch replacing the whole card.
//!   Here they are strings and the status mark is painted, the same narrowing the
//!   sibling catalog's table records for its head cells
//!   (`docs/LIMITATIONS.md`'s `shadcn-otp-table-button-api-gaps`). Titles are one
//!   line, clipped to the content column, rather than `truncate`d with an
//!   ellipsis and `line-clamp-2`'d — the catalog's shaped runs are single-line.
//! - **No blur in the ramps.** Every upstream transition here also animates
//!   `filter: blur(10px) → 0` (entrance), `blur(8px)` (exit) and `blur(6px)` (the
//!   status/content morph). `PaintScene` publishes no blur filter, so the fade,
//!   the lift and the scale carry the motion.
//! - **No content morph.** Upstream re-mounts the icon and the title block
//!   through a nested `AnimatePresence` keyed on the status and the title, so
//!   *updating* a live toast crossfades its parts. Here an updated entry
//!   re-shapes its runs in place; the card's own entrance and exit are the two
//!   staged transitions.
//! - **Swipe is distance-only.** Upstream dismisses on
//!   `|offset.x| > 72 || |velocity.x| > 520`. A `PointerEvent` carries no
//!   velocity, so only the distance arm is ported; a fast flick shorter than the
//!   threshold springs back instead of dismissing.
//! - **No portal knobs.** `portal`/`portalRoot`/`placement`/`fixed` all pick
//!   *where in the document* the stack renders. frust has no portal — the mount
//!   is the app's `Stack` — so the position enum is all that survives of that
//!   group.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, SemanticsCtx, Size, TickClass, View, Widget, erase_callback_arg,
    text::{FontWeight, TextStyle},
};
use frust::{FrameTime, SpringDescription, Theme};

use crate::motion::{Presence, PresencePhase, Ramp};
use crate::press::{Lane, presses};
use crate::style::{self, with_alpha};
use crate::text::LabelRun;
use crate::tokens::BeuiTokens;
use crate::tokens::motion::{EASE_OUT, SPRING_GLIDE};

// ---- Motion ----------------------------------------------------------------

/// The stack's own layout spring: `STACK_SPRING = { type: "spring", stiffness:
/// 420, damping: 34, mass: 0.75 }`.
///
/// Authored in the component rather than in `lib/ease.ts`, so it is a
/// component-local constant here rather than one of
/// [`crate::tokens::motion`]'s six. It is close to `SPRING_PANEL` and
/// deliberately not folded onto it: the mass differs (0.75 against 0.5), which
/// is what gives a re-stacking card its heavier settle.
pub const TOAST_STACK_SPRING: SpringDescription = SpringDescription {
    mass: 0.75,
    stiffness: 420.0,
    damping: 34.0,
};

/// The stacking ramp — [`TOAST_STACK_SPRING`] as a [`Ramp`]. Drives the
/// entrance, every card's re-stacking slide, and the hover expansion.
pub const TOAST_STACK_RAMP: Ramp = Ramp::spring(TOAST_STACK_SPRING);

/// The exit ramp: `transition: { duration: 0.18, ease: EASE_OUT }` on the item's
/// `exit` variant — shorter than the entrance, the catalog's usual asymmetry.
pub const TOAST_STACK_EXIT: Ramp = Ramp::eased(Duration::from_millis(180), EASE_OUT);

/// How far below its resting place a card starts, in logical px
/// (`initial={{ y: 22 }}`).
pub const TOAST_STACK_ENTER_LIFT: f64 = 22.0;

/// The scale a card enters from (`initial={{ scale: 0.96 }}`).
pub const TOAST_STACK_ENTER_SCALE: f64 = 0.96;

/// How far to the trailing side a dismissed card slides, in logical px
/// (`exit={{ x: 32 }}`).
pub const TOAST_STACK_EXIT_SLIDE: f64 = 32.0;

/// The scale a card exits to (`exit={{ scale: 0.96 }}`).
pub const TOAST_STACK_EXIT_SCALE: f64 = 0.96;

/// The fraction of a horizontal drag a card actually travels
/// (`dragElastic={0.18}` against `dragConstraints={{left: 0, right: 0}}` — the
/// constraints pin the rest position at zero, so the elasticity *is* the
/// displacement).
pub const TOAST_STACK_DRAG_ELASTIC: f64 = 0.18;

/// How far a drag must travel before release dismisses the card, in logical px
/// (`Math.abs(info.offset.x) > 72`). Measured on the raw pointer travel, not on
/// the elastically-reduced displacement the card shows.
pub const TOAST_STACK_SWIPE_DISMISS: f64 = 72.0;

/// The ramp a released, under-threshold drag springs back on — the catalog's
/// dragged-handle spring, which is what
/// [`SPRING_GLIDE`](crate::tokens::motion::SPRING_GLIDE) exists for.
pub const TOAST_STACK_SETTLE: Ramp = Ramp::spring(SPRING_GLIDE);

// ---- Stacking geometry -----------------------------------------------------

/// How far each depth step peeks past the one in front, in logical px
/// (`STACK_PEEK = 8` in `notification-stack.tsx`).
pub const TOAST_STACK_PEEK: f64 = 8.0;

/// How far each depth step is inset horizontally, per side, in logical px
/// (`STACK_INSET = 12`, applied as `clipPath: inset(0px index*12)`).
pub const TOAST_STACK_INSET: f64 = 12.0;

/// How many toasts the stack shows (`maxVisible = 4`); the queue keeps the rest
/// and reveals them as the visible ones leave.
pub const TOAST_STACK_MAX_VISIBLE: usize = 4;

/// The gap between two expanded cards, in logical px (`gap-2`).
pub const TOAST_STACK_GAP: f64 = 8.0;

/// The stack's widest card, in logical px (`max-w-sm`).
pub const TOAST_STACK_MAX_WIDTH: f64 = 384.0;

/// The margin from the viewport edges, in logical px (`left-4` / `right-4` /
/// `top-4`, and `w-[calc(100vw-2rem)]`'s own `1rem` per side).
pub const TOAST_STACK_MARGIN: f64 = 16.0;

/// The margin from the *bottom* edge, in logical px — upstream anchors bottom
/// positions a rung lower (`bottom-6`) than it does top ones (`top-4`).
pub const TOAST_STACK_MARGIN_BOTTOM: f64 = 24.0;

// ---- Card chrome -----------------------------------------------------------

/// The card's padding, in logical px (`p-3`).
pub const TOAST_STACK_PADDING: f64 = 12.0;

/// The card's corner radius, in logical px (`rounded-2xl`).
pub const TOAST_STACK_RADIUS: f64 = style::RADIUS_2XL;

/// The card fill's alpha (`bg-card/95`).
pub const TOAST_STACK_SURFACE_ALPHA: f32 = 0.95;

/// Gaussian sigma of the card's `shadow-2xl` (`0 25px 50px -12px`), halved to
/// the standard deviation this scene's shadow primitive takes.
pub const TOAST_STACK_SHADOW_STD_DEV: f64 = 12.5;

/// Alpha of the card's `shadow-2xl` (`rgb(0 0 0 / 0.25)`).
pub const TOAST_STACK_SHADOW_ALPHA: f32 = 0.25;

/// The status mark's round box, in logical px (`h-7 w-7`).
pub const TOAST_STACK_ICON_BOX: f64 = 28.0;

/// The gap between the status mark and the text column, in logical px (`gap-3`).
pub const TOAST_STACK_ICON_GAP: f64 = 12.0;

/// The mark drawn inside the status box, in logical px (`h-3.5 w-3.5`).
pub const TOAST_STACK_MARK_SIZE: f64 = 14.0;

/// The gap between title and description, in logical px (`mt-0.5`).
pub const TOAST_STACK_TITLE_GAP: f64 = 2.0;

/// The gap above the action button, in logical px (`mt-2`).
pub const TOAST_STACK_ACTION_GAP: f64 = 8.0;

/// The action button's height, in logical px (`h-7`).
pub const TOAST_STACK_ACTION_HEIGHT: f64 = 28.0;

/// The action button's horizontal padding, in logical px (`px-3`).
pub const TOAST_STACK_ACTION_PADDING_X: f64 = 12.0;

/// The close button's box, in logical px (`h-7 w-7`).
pub const TOAST_STACK_CLOSE_BOX: f64 = 28.0;

/// The action button's resting wash (`bg-primary/[0.06]`).
pub const TOAST_STACK_ACTION_WASH: f32 = 0.06;

/// The action and close buttons' hover wash (`hover:bg-primary/[0.1]`).
pub const TOAST_STACK_HOVER_WASH: f32 = 0.10;

/// The neutral status mark's wash (`bg-primary/[0.05]`).
pub const TOAST_STACK_NEUTRAL_WASH: f32 = 0.05;

/// Every other status mark's wash (`bg-primary/10`, `bg-emerald-500/10`,
/// `bg-destructive/10`).
pub const TOAST_STACK_STATUS_WASH: f32 = 0.10;

/// One full turn of the loading mark, in milliseconds — Tailwind's
/// `animate-spin` (`animation: spin 1s linear infinite`).
pub const TOAST_STACK_SPIN_MS: f64 = 1000.0;

/// Stroke width of a painted status mark, in logical px.
const MARK_STROKE: f64 = 1.75;

/// Unthemed fallback card fill — the light table's `--card`.
const FALLBACK_CARD: Color = crate::BEUI_LIGHT.card;
/// Unthemed fallback hairline — the light table's `--border`.
const FALLBACK_BORDER: Color = crate::BEUI_LIGHT.border;
/// Unthemed fallback ink — the light table's `--foreground`.
const FALLBACK_INK: Color = crate::BEUI_LIGHT.foreground;
/// Unthemed fallback dimmed ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED: Color = crate::BEUI_LIGHT.muted_foreground;
/// Unthemed fallback accent — the light table's `--primary`.
const FALLBACK_PRIMARY: Color = crate::BEUI_LIGHT.primary;
/// Unthemed fallback destructive hue — the light table's `--danger`.
const FALLBACK_DANGER: Color = crate::BEUI_LIGHT.danger;

// ---- Public vocabulary -----------------------------------------------------

/// A toast's tone — upstream's `ToastStatus`, and the row of `STATUS_ICON` /
/// `STATUS_CLASS` it selects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnimatedToastStackStatus {
    /// `"neutral"`: a bell on the faintest primary wash. The default.
    #[default]
    Neutral,
    /// `"info"`: an info mark in the accent ink.
    Info,
    /// `"loading"`: a spinning ring in the accent ink.
    Loading,
    /// `"success"`: a check in beUI's own `--success` (upstream's
    /// `text-emerald-600 dark:text-emerald-400` reads a Tailwind hue this
    /// catalog has a token for — the same substitution
    /// [`crate::components::animated_badge`] records).
    Success,
    /// `"error"`: an alert mark in the destructive hue.
    Error,
}

impl AnimatedToastStackStatus {
    /// Every tone, in upstream's own `STATUS_ICON` order.
    pub const ALL: [AnimatedToastStackStatus; 5] = [
        AnimatedToastStackStatus::Neutral,
        AnimatedToastStackStatus::Info,
        AnimatedToastStackStatus::Loading,
        AnimatedToastStackStatus::Success,
        AnimatedToastStackStatus::Error,
    ];

    /// Whether this tone's mark spins (`animate-spin` on the loading icon).
    pub const fn spins(self) -> bool {
        matches!(self, AnimatedToastStackStatus::Loading)
    }
}

/// Which corner the stack anchors to — upstream's `ToastPosition` and its
/// `POSITION_CLASS` table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnimatedToastStackPosition {
    /// `"top-left"`: `left-4 top-4`.
    TopLeft,
    /// `"top-center"`: `left-1/2 top-4`.
    TopCenter,
    /// `"top-right"`: `right-4 top-4`.
    TopRight,
    /// `"bottom-left"`: `bottom-6 left-4`.
    BottomLeft,
    /// `"bottom-center"`: `bottom-6 left-1/2`.
    BottomCenter,
    /// `"bottom-right"`: `bottom-6 right-4`. Upstream's default.
    #[default]
    BottomRight,
}

impl AnimatedToastStackPosition {
    /// Every position, in `POSITION_CLASS`'s own order.
    pub const ALL: [AnimatedToastStackPosition; 6] = [
        AnimatedToastStackPosition::TopLeft,
        AnimatedToastStackPosition::TopCenter,
        AnimatedToastStackPosition::TopRight,
        AnimatedToastStackPosition::BottomLeft,
        AnimatedToastStackPosition::BottomCenter,
        AnimatedToastStackPosition::BottomRight,
    ];

    /// Whether the stack grows upward from the bottom edge
    /// (`position.startsWith("bottom")`).
    pub const fn is_bottom(self) -> bool {
        matches!(
            self,
            AnimatedToastStackPosition::BottomLeft
                | AnimatedToastStackPosition::BottomCenter
                | AnimatedToastStackPosition::BottomRight
        )
    }

    /// The margin between the anchored horizontal edge and the card, in logical
    /// px, or `None` for the centred positions.
    const fn edge_margin(self) -> Option<(bool, f64)> {
        match self {
            AnimatedToastStackPosition::TopLeft | AnimatedToastStackPosition::BottomLeft => {
                Some((true, TOAST_STACK_MARGIN))
            }
            AnimatedToastStackPosition::TopRight | AnimatedToastStackPosition::BottomRight => {
                Some((false, TOAST_STACK_MARGIN))
            }
            AnimatedToastStackPosition::TopCenter | AnimatedToastStackPosition::BottomCenter => {
                None
            }
        }
    }
}

/// One queued toast — upstream's `AnimatedToast`, minus the parts that have no
/// frust analogue (see the [module docs](self)' degradations).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimatedToastStackEntry {
    id: u64,
    title: String,
    description: Option<String>,
    action: Option<String>,
    status: AnimatedToastStackStatus,
    dismissible: bool,
}

/// Queue a toast titled `title` under the stable `id` the owner dismisses it by.
pub fn animated_toast(id: u64, title: impl Into<String>) -> AnimatedToastStackEntry {
    AnimatedToastStackEntry {
        id,
        title: title.into(),
        description: None,
        action: None,
        status: AnimatedToastStackStatus::default(),
        // `dismissible: true` is upstream's own default (`createToast`).
        dismissible: true,
    }
}

impl AnimatedToastStackEntry {
    /// Add the second line (`toast.description`).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add the action button and its label (`toast.action`). The press is
    /// reported through [`AnimatedToastStackView::on_action`].
    pub fn action(mut self, label: impl Into<String>) -> Self {
        self.action = Some(label.into());
        self
    }

    /// Set the tone (`toast.status`).
    pub fn status(mut self, status: AnimatedToastStackStatus) -> Self {
        self.status = status;
        self
    }

    /// Whether the card shows its close button (`toast.dismissible`).
    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    /// This toast's stable id.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// This toast's title.
    pub fn title(&self) -> &str {
        &self.title
    }
}

/// The depth-`depth` card's peek past the one in front, in logical px.
///
/// The collapsed stack's whole along-axis geometry: card `d` sits
/// `d * `[`TOAST_STACK_PEEK`] further from the anchored edge than the newest
/// one, which is `notification-stack.tsx`'s `y: index * STACK_PEEK`.
pub fn toast_stack_peek(depth: usize) -> f64 {
    depth as f64 * TOAST_STACK_PEEK
}

/// The depth-`depth` card's horizontal inset per side, in logical px —
/// `clipPath: inset(0px index * STACK_INSET)`, clamped so a deep card never
/// inverts.
pub fn toast_stack_inset(depth: usize, width: f64) -> f64 {
    (depth as f64 * TOAST_STACK_INSET).min((width / 2.0).max(0.0))
}

/// The depth-`depth` card's width as a fraction of the front card's, derived
/// from [`toast_stack_inset`] — the scale a two-sided clip inset reads as.
///
/// `1.0` at depth 0 and at a degenerate width; never negative.
pub fn toast_stack_scale(depth: usize, width: f64) -> f64 {
    if width <= 0.0 {
        return 1.0;
    }
    ((width - 2.0 * toast_stack_inset(depth, width)) / width).clamp(0.0, 1.0)
}

/// A view-held, typed toast callback (erased on build).
type OnToast<State> = Rc<dyn Fn(&mut State, u64)>;

/// A declarative beUI toast stack. See the [module docs](self).
pub struct AnimatedToastStackView<State: 'static> {
    toasts: Vec<AnimatedToastStackEntry>,
    position: AnimatedToastStackPosition,
    max_visible: usize,
    on_dismiss: OnToast<State>,
    on_action: Option<OnToast<State>>,
}

/// Mount a toast stack over `toasts` — oldest first, newest last — reporting a
/// dismissal (close button or swipe) through `on_dismiss`.
///
/// The queue is the owner's: `on_dismiss` hands back the toast's id and the
/// owner drops it, which is what starts the exit.
pub fn animated_toast_stack<State: 'static, F: Fn(&mut State, u64) + 'static>(
    toasts: Vec<AnimatedToastStackEntry>,
    on_dismiss: F,
) -> AnimatedToastStackView<State> {
    AnimatedToastStackView {
        toasts,
        position: AnimatedToastStackPosition::default(),
        max_visible: TOAST_STACK_MAX_VISIBLE,
        on_dismiss: Rc::new(on_dismiss),
        on_action: None,
    }
}

impl<State: 'static> AnimatedToastStackView<State> {
    /// Anchor the stack to a different corner (upstream's `position`).
    pub fn position(mut self, position: AnimatedToastStackPosition) -> Self {
        self.position = position;
        self
    }

    /// Show at most `max_visible` cards (upstream's `maxVisible`). Clamped to at
    /// least one — a stack showing nothing has no reason to be mounted.
    pub fn max_visible(mut self, max_visible: usize) -> Self {
        self.max_visible = max_visible.max(1);
        self
    }

    /// Report presses on a card's action button, by toast id.
    pub fn on_action<F: Fn(&mut State, u64) + 'static>(mut self, on_action: F) -> Self {
        self.on_action = Some(Rc::new(on_action));
        self
    }

    /// The ids currently queued, in order.
    fn ids(&self) -> Vec<u64> {
        self.toasts.iter().map(|toast| toast.id).collect()
    }
}

/// The inks and washes one status paints with.
struct ToastColors {
    /// The card fill (`bg-card/95`).
    surface: Color,
    /// The card hairline (`border-border`).
    border: Color,
    /// The title (`text-foreground`).
    ink: Color,
    /// The description and the close button (`text-muted-foreground`).
    muted: Color,
    /// The accent the action button's wash is mixed from (`--primary`).
    primary: Color,
    /// The status mark's ink.
    mark: Color,
    /// The status mark's round wash.
    mark_wash: Color,
}

/// Resolve the palette for `status`, falling back to the vendored **light**
/// table unthemed.
fn resolve_colors(status: AnimatedToastStackStatus, theme: Option<&Theme>) -> ToastColors {
    let tokens = BeuiTokens::resolve(theme);
    let scheme = theme.map(Theme::scheme);
    let pick =
        |role: fn(&frust::ColorScheme) -> Color, fallback: Color| scheme.map_or(fallback, role);
    let primary = pick(|s| s.primary, FALLBACK_PRIMARY);
    let muted = pick(|s| s.on_surface_variant, FALLBACK_MUTED);
    let (mark, wash_alpha) = match status {
        AnimatedToastStackStatus::Neutral => (muted, TOAST_STACK_NEUTRAL_WASH),
        AnimatedToastStackStatus::Info | AnimatedToastStackStatus::Loading => {
            (primary, TOAST_STACK_STATUS_WASH)
        }
        AnimatedToastStackStatus::Success => (tokens.success, TOAST_STACK_STATUS_WASH),
        AnimatedToastStackStatus::Error => {
            (pick(|s| s.error, FALLBACK_DANGER), TOAST_STACK_STATUS_WASH)
        }
    };
    // The neutral wash is mixed from the accent, not from the mark's own ink —
    // `bg-primary/[0.05]` sits under a `text-muted-foreground` bell.
    let wash_hue = match status {
        AnimatedToastStackStatus::Neutral => primary,
        _ => mark,
    };
    ToastColors {
        surface: with_alpha(
            pick(|s| s.surface_container, FALLBACK_CARD),
            TOAST_STACK_SURFACE_ALPHA,
        ),
        border: pick(|s| s.outline_variant, FALLBACK_BORDER),
        ink: pick(|s| s.on_surface, FALLBACK_INK),
        muted,
        primary,
        mark,
        mark_wash: with_alpha(wash_hue, wash_alpha),
    }
}

/// The title style (`text-sm font-medium`).
fn title_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    });
    TextStyle {
        family,
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_SM as f32, crate::text::SHAPING_INK)
    }
}

/// The description style (`text-xs`, regular weight).
fn body_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.body_small.family.clone()
    });
    TextStyle {
        family,
        ..TextStyle::new(style::TEXT_XS as f32, crate::text::SHAPING_INK)
    }
}

/// The action label style (`text-xs font-medium`).
fn action_style(theme: Option<&Theme>) -> TextStyle {
    TextStyle {
        weight: FontWeight::MEDIUM,
        ..body_style(theme)
    }
}

/// One retained toast.
struct Slot {
    id: u64,
    status: AnimatedToastStackStatus,
    dismissible: bool,
    title: LabelRun,
    description: Option<LabelRun>,
    action: Option<LabelRun>,
    /// The enter/exit driver — the reason a dismissed toast is still here.
    presence: Presence,
    /// Distance from the anchored edge to this card's near edge, springing to
    /// its target as the stack re-stacks or expands.
    along: Lane,
    /// Raw horizontal drag travel while a swipe is live.
    drag: f64,
    /// The released drag's spring back to rest.
    settle: Lane,
    /// True once the owner has dropped the entry: the card is exiting and this
    /// widget owns its removal.
    closing: bool,
    /// The card's own natural size, from the last layout.
    size: Size,
    /// Whether this slot has ever had a target assigned — a fresh card lands on
    /// its place rather than sliding to it.
    placed: bool,
}

impl Slot {
    /// Build a slot for `entry`, staged to play its entrance.
    fn new(entry: &AnimatedToastStackEntry) -> Self {
        let mut presence = Presence::new(TOAST_STACK_RAMP, TOAST_STACK_EXIT);
        presence.set_open(true);
        Slot {
            id: entry.id,
            status: entry.status,
            dismissible: entry.dismissible,
            title: LabelRun::new(entry.title.clone()),
            description: entry.description.as_ref().map(LabelRun::new),
            action: entry.action.as_ref().map(LabelRun::new),
            presence,
            along: Lane::at_rest(TOAST_STACK_RAMP, 0.0),
            drag: 0.0,
            settle: Lane::at_rest(TOAST_STACK_SETTLE, 0.0),
            closing: false,
            size: Size::ZERO,
            placed: false,
        }
    }

    /// Apply an updated entry in place — upstream's `updateToast`, which patches
    /// a live toast rather than replacing it.
    ///
    /// The runs re-shape only where the text actually differs, so a rebuild
    /// feeding the same queue back costs nothing.
    fn sync(&mut self, entry: &AnimatedToastStackEntry) {
        self.title.set_content(entry.title.clone());
        sync_run(&mut self.description, entry.description.as_ref());
        sync_run(&mut self.action, entry.action.as_ref());
        self.dismissible = entry.dismissible;
        self.status = entry.status;
    }

    /// The card's horizontal displacement: the live drag's elastic share plus
    /// whatever the release spring still owes.
    fn slide(&self) -> f64 {
        self.drag * TOAST_STACK_DRAG_ELASTIC + self.settle.value()
    }
}

/// Apply an optional label to an optional cached run, re-shaping only on a real
/// change.
fn sync_run(run: &mut Option<LabelRun>, text: Option<&String>) {
    match (run.as_mut(), text) {
        (Some(existing), Some(text)) => {
            existing.set_content(text.clone());
        }
        (None, Some(text)) => *run = Some(LabelRun::new(text.clone())),
        (Some(_), None) => *run = None,
        (None, None) => {}
    }
}

/// What a `Down` armed on a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToastTarget {
    /// The card body — a swipe candidate.
    Surface,
    /// The close button.
    Close,
    /// The action button.
    Action,
}

/// The pointer press in flight.
struct Armed {
    /// The toast's id, not its index: the queue can move under a live press.
    id: u64,
    target: ToastTarget,
    /// Where the press started, in widget-local coordinates.
    start: Point,
}

impl<State: 'static> View<State> for AnimatedToastStackView<State> {
    type Element = AnimatedToastStackWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> AnimatedToastStackWidget {
        AnimatedToastStackWidget {
            slots: self.toasts.iter().map(Slot::new).collect(),
            position: self.position,
            max_visible: self.max_visible,
            area: Size::ZERO,
            width: 0.0,
            expand: Lane::at_rest(TOAST_STACK_RAMP, 0.0),
            expanded: false,
            armed: None,
            hovered_button: None,
            on_dismiss: erase_callback_arg(&self.on_dismiss),
            on_action: self.on_action.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnimatedToastStackWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_dismiss = erase_callback_arg(&self.on_dismiss);
        element.on_action = self.on_action.as_ref().map(erase_callback_arg);
        let mut flags = ChangeFlags::NONE;

        if prev.position != self.position || prev.max_visible != self.max_visible {
            element.position = self.position;
            element.max_visible = self.max_visible;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.ids() != self.ids() || prev.toasts != self.toasts {
            element.reconcile(&self.toasts);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained widget for an [`AnimatedToastStackView`].
pub struct AnimatedToastStackWidget {
    slots: Vec<Slot>,
    position: AnimatedToastStackPosition,
    max_visible: usize,
    /// The area this host fills, from the last layout.
    area: Size,
    /// Every card's shared width, from the last layout.
    width: f64,
    /// `0` collapsed, `1` expanded — the hover-driven spread.
    expand: Lane,
    /// Whether a pointer is on the stack.
    expanded: bool,
    /// The press in flight.
    armed: Option<Armed>,
    /// The `(id, target)` under the pointer, for the buttons' hover wash.
    hovered_button: Option<(u64, ToastTarget)>,
    on_dismiss: ErasedArgCallback<u64>,
    on_action: Option<ErasedArgCallback<u64>>,
}

impl AnimatedToastStackWidget {
    /// The queued toasts' ids, newest last — the order the stack's depths are
    /// assigned from, exiting cards included.
    pub fn slot_ids(&self) -> Vec<u64> {
        self.slots.iter().map(|slot| slot.id).collect()
    }

    /// Which toasts are currently exiting.
    pub fn closing_ids(&self) -> Vec<u64> {
        self.slots
            .iter()
            .filter(|slot| slot.closing)
            .map(|slot| slot.id)
            .collect()
    }

    /// Whether the stack is spread into a list (pointer on it).
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// Apply the owner's queue: keep every id it still holds, start the exit on
    /// every id it dropped, and append the ones it added.
    ///
    /// Appending is the only insertion this follows — upstream's queue is
    /// push-and-filter (`[...current, toast]` / `filter`), never a reorder — so
    /// a toast that reappears after being dropped comes back as the newest.
    fn reconcile(&mut self, toasts: &[AnimatedToastStackEntry]) {
        for slot in &mut self.slots {
            match toasts.iter().find(|entry| entry.id == slot.id) {
                Some(entry) => {
                    slot.sync(entry);
                    if slot.closing {
                        // Re-queued mid-exit: reverse the ramp rather than
                        // leaving a card that is on screen playing its exit.
                        slot.closing = false;
                        slot.presence.set_open(true);
                    }
                }
                None => {
                    if !slot.closing {
                        slot.closing = true;
                        slot.presence.set_open(false);
                    }
                }
            }
        }
        for entry in toasts {
            if !self.slots.iter().any(|slot| slot.id == entry.id) {
                self.slots.push(Slot::new(entry));
            }
        }
    }

    /// The slot indices the stack shows, deepest first — the last
    /// `max_visible` of the queue, exactly upstream's `toasts.slice(-maxVisible)`.
    fn visible(&self) -> Vec<usize> {
        let start = self.slots.len().saturating_sub(self.max_visible);
        (start..self.slots.len()).collect()
    }

    /// Slot `index`'s depth: `0` is the newest card, at the front.
    fn depth(&self, index: usize) -> Option<usize> {
        let start = self.slots.len().saturating_sub(self.max_visible);
        (index >= start && index < self.slots.len()).then(|| self.slots.len() - 1 - index)
    }

    /// The slot holding `id`.
    fn slot_of(&self, id: u64) -> Option<usize> {
        self.slots.iter().position(|slot| slot.id == id)
    }

    /// Re-aim every visible card's along-axis lane at the place its depth and
    /// the current spread put it. Called from `layout`, where the heights the
    /// expanded targets are summed from are known.
    fn retarget_places(&mut self) {
        let visible = self.visible();
        let expanded = self.expanded;
        // Walk front to back so each card's expanded offset is the running sum
        // of the cards in front of it.
        let mut running = 0.0;
        for index in visible.into_iter().rev() {
            let Some(depth) = self.depth(index) else {
                continue;
            };
            let target = if expanded {
                running
            } else {
                toast_stack_peek(depth)
            };
            let slot = &mut self.slots[index];
            if slot.placed {
                slot.along.retarget(target);
            } else {
                slot.along = Lane::at_rest(TOAST_STACK_RAMP, target);
                slot.placed = true;
            }
            running += slot.size.height + TOAST_STACK_GAP;
        }
    }

    /// The height a card at `depth` displays at: the front card's box while
    /// collapsed, its own once spread.
    fn displayed_height(&self, index: usize) -> f64 {
        let own = self.slots[index].size.height;
        let front = self.slots.last().map_or(own, |slot| slot.size.height);
        let spread = self.expand.value().clamp(0.0, 1.0);
        front + (own - front) * spread
    }

    /// Slot `index`'s box in widget-local coordinates, at the currently
    /// displayed spread, stacking offset and swipe.
    fn card_rect(&self, index: usize) -> Option<Rect> {
        let depth = self.depth(index)?;
        let slot = &self.slots[index];
        let spread = self.expand.value().clamp(0.0, 1.0);
        let inset = toast_stack_inset(depth, self.width) * (1.0 - spread);
        let width = (self.width - 2.0 * inset).max(0.0);
        let height = self.displayed_height(index);
        let along = slot.along.value();

        let x = match self.position.edge_margin() {
            Some((true, margin)) => margin + inset,
            Some((false, margin)) => self.area.width - margin - self.width + inset,
            None => (self.area.width - self.width) / 2.0 + inset,
        } + slot.slide();

        let y = if self.position.is_bottom() {
            self.area.height - TOAST_STACK_MARGIN_BOTTOM - height - along
        } else {
            TOAST_STACK_MARGIN + along
        };
        Some(Rect::from_origin_size(
            Point::new(x, y),
            Size::new(width, height),
        ))
    }

    /// Whether slot `index`'s content is drawn and its buttons are live: the
    /// front card always, every other only once the stack has spread.
    fn content_alpha(&self, index: usize) -> f64 {
        match self.depth(index) {
            Some(0) => 1.0,
            Some(_) => self.expand.value().clamp(0.0, 1.0),
            None => 0.0,
        }
    }

    /// The close button's box inside `card`.
    fn close_rect(&self, card: Rect) -> Rect {
        Rect::from_origin_size(
            Point::new(
                card.x1 - TOAST_STACK_PADDING - TOAST_STACK_CLOSE_BOX,
                card.y0 + TOAST_STACK_PADDING,
            ),
            Size::new(TOAST_STACK_CLOSE_BOX, TOAST_STACK_CLOSE_BOX),
        )
    }

    /// The action button's box inside `card`, given its measured label.
    fn action_rect(&self, index: usize, card: Rect) -> Option<Rect> {
        let slot = &self.slots[index];
        let label = slot.action.as_ref()?;
        let text_x = card.x0 + TOAST_STACK_PADDING + TOAST_STACK_ICON_BOX + TOAST_STACK_ICON_GAP;
        let width = label.size().width + TOAST_STACK_ACTION_PADDING_X * 2.0;
        Some(Rect::from_origin_size(
            Point::new(
                text_x,
                card.y1 - TOAST_STACK_PADDING - TOAST_STACK_ACTION_HEIGHT,
            ),
            Size::new(width, TOAST_STACK_ACTION_HEIGHT),
        ))
    }

    /// What a widget-local `pos` lands on, front card first.
    fn hit(&self, pos: Point) -> Option<(usize, ToastTarget)> {
        for index in self.visible().into_iter().rev() {
            if !self.slots[index].presence.is_visible() {
                continue;
            }
            let Some(card) = self.card_rect(index) else {
                continue;
            };
            if !card.contains(pos) {
                continue;
            }
            let live = self.content_alpha(index) > 0.5;
            if live && self.slots[index].dismissible && self.close_rect(card).contains(pos) {
                return Some((index, ToastTarget::Close));
            }
            if live
                && self
                    .action_rect(index, card)
                    .is_some_and(|rect| rect.contains(pos))
            {
                return Some((index, ToastTarget::Action));
            }
            return Some((index, ToastTarget::Surface));
        }
        None
    }

    /// Set the spread target, reporting whether it changed.
    fn set_expanded(&mut self, expanded: bool) -> bool {
        if self.expanded == expanded {
            return false;
        }
        self.expanded = expanded;
        self.expand.retarget(if expanded { 1.0 } else { 0.0 });
        true
    }

    /// Advance every lane and presence to `now`, dropping the cards whose exit
    /// has finished, and report whether anything is still moving.
    ///
    /// Dropping happens here rather than from the event pass
    /// `Presence::take_exited` documents: the owner has *already* removed the
    /// entry — that removal is what started the exit — so there is no callback
    /// left to defer, only this widget's own bookkeeping.
    fn advance(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        if reduce_motion {
            self.expand.snap();
            for slot in &mut self.slots {
                slot.presence = slot.presence.collapsed();
                slot.along.snap();
                slot.settle.snap();
            }
        }
        let mut moving = self.expand.advance(now);
        for slot in &mut self.slots {
            slot.presence.advance(now);
            moving |= slot.presence.is_animating();
            moving |= slot.along.advance(now);
            moving |= slot.settle.advance(now);
        }
        self.slots
            .retain(|slot| !slot.closing || slot.presence.is_visible());
        moving
    }
}

/// Paint the mark for `status`, centred on `centre`, rotated by `angle`.
fn draw_status_mark(
    scene: &mut dyn PaintScene,
    centre: Point,
    status: AnimatedToastStackStatus,
    angle: f64,
    color: Color,
) {
    let arm = TOAST_STACK_MARK_SIZE / 2.0;
    let brush = Brush::Solid(color);
    scene.push_transform(Affine::translate(centre.to_vec2()) * Affine::rotate(angle));
    match status {
        // A bell reads as a dot at 14px; the wash behind it carries the tone.
        AnimatedToastStackStatus::Neutral => {
            scene.fill_rounded_rect(
                Point::new(-arm * 0.3, -arm * 0.3),
                Size::new(arm * 0.6, arm * 0.6),
                arm * 0.3,
                color,
            );
        }
        // `i`: a dot over a stem.
        AnimatedToastStackStatus::Info => {
            scene.fill_rounded_rect(
                Point::new(-arm * 0.15, -arm * 0.85),
                Size::new(arm * 0.3, arm * 0.3),
                arm * 0.15,
                color,
            );
            let mut stem = BezPath::new();
            stem.move_to(Point::new(0.0, -arm * 0.25));
            stem.line_to(Point::new(0.0, arm * 0.8));
            scene.stroke_path(Point::ZERO, &stem, MARK_STROKE, &brush);
        }
        // A three-quarter ring, polylined: the spinner's own rotation is what
        // reads, not the arc's smoothness at 14px.
        AnimatedToastStackStatus::Loading => {
            let mut ring = BezPath::new();
            let steps = 16;
            for step in 0..=steps {
                let t = std::f64::consts::TAU * 0.75 * (step as f64 / steps as f64);
                let point = Point::new(arm * 0.8 * t.cos(), arm * 0.8 * t.sin());
                if step == 0 {
                    ring.move_to(point);
                } else {
                    ring.line_to(point);
                }
            }
            scene.stroke_path(Point::ZERO, &ring, MARK_STROKE, &brush);
        }
        // A check.
        AnimatedToastStackStatus::Success => {
            let mut check = BezPath::new();
            check.move_to(Point::new(-arm * 0.7, 0.0));
            check.line_to(Point::new(-arm * 0.15, arm * 0.55));
            check.line_to(Point::new(arm * 0.7, -arm * 0.55));
            scene.stroke_path(Point::ZERO, &check, MARK_STROKE, &brush);
        }
        // `!`: a stem over a dot.
        AnimatedToastStackStatus::Error => {
            let mut stem = BezPath::new();
            stem.move_to(Point::new(0.0, -arm * 0.8));
            stem.line_to(Point::new(0.0, arm * 0.25));
            scene.stroke_path(Point::ZERO, &stem, MARK_STROKE, &brush);
            scene.fill_rounded_rect(
                Point::new(-arm * 0.15, arm * 0.55),
                Size::new(arm * 0.3, arm * 0.3),
                arm * 0.15,
                color,
            );
        }
    }
    scene.pop_transform();
}

/// Paint the close button's cross, centred on `centre`.
fn draw_close_mark(scene: &mut dyn PaintScene, centre: Point, color: Color) {
    let arm = TOAST_STACK_MARK_SIZE / 2.0 * 0.7;
    let mut cross = BezPath::new();
    cross.move_to(Point::new(centre.x - arm, centre.y - arm));
    cross.line_to(Point::new(centre.x + arm, centre.y + arm));
    cross.move_to(Point::new(centre.x + arm, centre.y - arm));
    cross.line_to(Point::new(centre.x - arm, centre.y + arm));
    scene.stroke_path(Point::ZERO, &cross, MARK_STROKE, &Brush::Solid(color));
}

impl Widget for AnimatedToastStackWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let title = title_style(theme);
        let body = body_style(theme);
        let action = action_style(theme);

        self.area = Size::new(
            crate::overlay::finite_or_zero(bc.max().width),
            crate::overlay::finite_or_zero(bc.max().height),
        );
        let insets = ctx.window_insets().padding();
        let horizontal = insets.left + insets.right;
        self.width = TOAST_STACK_MAX_WIDTH
            .min((self.area.width - 2.0 * TOAST_STACK_MARGIN - horizontal).max(0.0));

        for slot in &mut self.slots {
            let title_size = slot.title.layout(ctx, &title);
            let description = slot
                .description
                .as_mut()
                .map(|run| run.layout(ctx, &body).height);
            let has_action = slot.action.is_some();
            if let Some(run) = slot.action.as_mut() {
                run.layout(ctx, &action);
            }
            let content = title_size.height
                + description.map_or(0.0, |height| TOAST_STACK_TITLE_GAP + height)
                + if has_action {
                    TOAST_STACK_ACTION_GAP + TOAST_STACK_ACTION_HEIGHT
                } else {
                    0.0
                };
            slot.size = Size::new(
                self.width,
                TOAST_STACK_PADDING * 2.0 + content.max(TOAST_STACK_ICON_BOX),
            );
        }
        self.retarget_places();
        self.area
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() && self.expanded {
            self.set_expanded(false);
        }
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let moving = self.advance(now, reduce_motion);
        let origin = ctx.origin();

        let mut spinning = false;
        // Back to front, so the newest card lands on top of its neighbours.
        for index in self.visible() {
            let slot = &self.slots[index];
            if !slot.presence.is_visible() {
                continue;
            }
            let Some(card) = self.card_rect(index) else {
                continue;
            };
            let colors = resolve_colors(slot.status, theme);
            let presence = slot.presence.presence(now).clamp(0.0, 1.0);
            let exiting = matches!(
                slot.presence.phase(),
                PresencePhase::Exiting | PresencePhase::Absent
            );
            let (dx, dy, scale) = if exiting {
                (
                    (1.0 - presence) * TOAST_STACK_EXIT_SLIDE,
                    0.0,
                    TOAST_STACK_EXIT_SCALE + (1.0 - TOAST_STACK_EXIT_SCALE) * presence,
                )
            } else {
                (
                    0.0,
                    (1.0 - presence) * TOAST_STACK_ENTER_LIFT,
                    TOAST_STACK_ENTER_SCALE + (1.0 - TOAST_STACK_ENTER_SCALE) * presence,
                )
            };
            let card_origin = Point::new(origin.x + card.x0, origin.y + card.y0);
            let card_size = Size::new(card.width(), card.height());
            let pivot = Point::new(
                card_origin.x + card_size.width / 2.0,
                card_origin.y + card_size.height / 2.0,
            );

            let layered = presence < 1.0;
            if layered {
                scene.push_layer(card_origin, card_size, presence as f32);
            }
            scene.push_transform(
                Affine::translate((dx, dy))
                    * Affine::translate(pivot.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-pivot.to_vec2()),
            );

            scene.draw_shadow(
                card_origin,
                card_size,
                TOAST_STACK_RADIUS,
                TOAST_STACK_SHADOW_STD_DEV,
                with_alpha(Color::BLACK, TOAST_STACK_SHADOW_ALPHA),
            );
            scene.fill_rounded_rect(card_origin, card_size, TOAST_STACK_RADIUS, colors.surface);
            crate::press::stroke_outline(
                scene,
                card_origin,
                card_size,
                TOAST_STACK_RADIUS,
                colors.border,
            );

            let content = self.content_alpha(index);
            if content > 0.0 {
                if content < 1.0 {
                    scene.push_layer(card_origin, card_size, content as f32);
                }
                self.paint_card_content(index, card_origin, card_size, &colors, now, scene);
                if content < 1.0 {
                    scene.pop_layer();
                }
            }
            scene.pop_transform();
            if layered {
                scene.pop_layer();
            }
            spinning |= slot.status.spins() && content > 0.0 && !reduce_motion;
        }

        if moving {
            ctx.request_frame();
        } else if spinning {
            // A perpetual decorative loop, not a transition with an endpoint.
            ctx.request_frame_class(TickClass::CosmeticLoop);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some((index, target)) = self.hit(p.position) else {
                    // Every pixel the cards do not cover belongs to whatever
                    // this host overlays (`pointer-events-none`).
                    return EventResult::Ignored;
                };
                self.armed = Some(Armed {
                    id: self.slots[index].id,
                    target,
                    start: p.position,
                });
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if let Some(armed) = &self.armed {
                    if armed.target == ToastTarget::Surface
                        && let Some(index) = self.slot_of(armed.id)
                    {
                        self.slots[index].drag = p.position.x - armed.start.x;
                        ctx.request_redraw();
                    }
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                    return EventResult::Handled;
                }
                let hit = self.hit(p.position);
                let button = hit.and_then(|(index, target)| match target {
                    ToastTarget::Surface => None,
                    other => Some((self.slots[index].id, other)),
                });
                if hit.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                let mut changed = self.set_expanded(hit.is_some());
                if self.hovered_button != button {
                    self.hovered_button = button;
                    changed = true;
                }
                if changed {
                    ctx.request_redraw();
                }
                // Never consumed: a pointer resting on a toast must not stop the
                // screen underneath from tracking it.
                EventResult::Ignored
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                let Some(index) = self.slot_of(armed.id) else {
                    return EventResult::Handled;
                };
                let inside_card = self
                    .card_rect(index)
                    .is_some_and(|card| card.contains(p.position));
                match armed.target {
                    ToastTarget::Close if inside_card => (self.on_dismiss)(ctx, armed.id),
                    ToastTarget::Action if inside_card => {
                        if let Some(on_action) = self.on_action.as_mut() {
                            on_action(ctx, armed.id);
                        }
                    }
                    ToastTarget::Surface => {
                        let travel = self.slots[index].drag;
                        let displaced = self.slots[index].slide();
                        self.slots[index].drag = 0.0;
                        self.slots[index].settle = Lane::at_rest(TOAST_STACK_SETTLE, displaced);
                        self.slots[index].settle.retarget(0.0);
                        if self.slots[index].dismissible && travel.abs() > TOAST_STACK_SWIPE_DISMISS
                        {
                            (self.on_dismiss)(ctx, armed.id);
                        }
                    }
                    _ => {}
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                if let Some(index) = self.slot_of(armed.id) {
                    let displaced = self.slots[index].slide();
                    self.slots[index].drag = 0.0;
                    self.slots[index].settle = Lane::at_rest(TOAST_STACK_SETTLE, displaced);
                    self.slots[index].settle.retarget(0.0);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::List,
            |_| {},
            |ctx| {
                // Newest first: a screen reader should meet the toast that just
                // arrived before the ones it pushed back.
                for index in self.visible().into_iter().rev() {
                    let slot = &self.slots[index];
                    if !slot.presence.is_visible() {
                        continue;
                    }
                    let label = match &slot.description {
                        Some(description) => {
                            format!("{}. {}", slot.title.content(), description.content())
                        }
                        None => slot.title.content().to_string(),
                    };
                    ctx.push_node(Role::ListItem, |node| node.set_label(label.as_str()));
                    if let Some(action) = &slot.action {
                        let action_label = action.content().to_string();
                        ctx.push_node(Role::Button, |node| {
                            node.set_label(action_label.as_str());
                            node.add_action(Action::Click);
                        });
                    }
                    if slot.dismissible {
                        ctx.push_node(Role::Button, |node| {
                            node.set_label("Dismiss toast");
                            node.add_action(Action::Click);
                        });
                    }
                }
            },
        );
    }
}

impl AnimatedToastStackWidget {
    /// Paint one card's status mark, text column and buttons, in absolute
    /// coordinates.
    fn paint_card_content(
        &self,
        index: usize,
        card_origin: Point,
        card_size: Size,
        colors: &ToastColors,
        now: FrameTime,
        scene: &mut dyn PaintScene,
    ) {
        let slot = &self.slots[index];
        let icon = Rect::from_origin_size(
            Point::new(
                card_origin.x + TOAST_STACK_PADDING,
                card_origin.y + TOAST_STACK_PADDING,
            ),
            Size::new(TOAST_STACK_ICON_BOX, TOAST_STACK_ICON_BOX),
        );
        scene.fill_rounded_rect(
            icon.origin(),
            icon.size(),
            TOAST_STACK_ICON_BOX / 2.0,
            colors.mark_wash,
        );
        let angle = if slot.status.spins() {
            let ms = now.as_nanos() as f64 / 1_000_000.0;
            std::f64::consts::TAU * (ms % TOAST_STACK_SPIN_MS) / TOAST_STACK_SPIN_MS
        } else {
            0.0
        };
        draw_status_mark(scene, icon.center(), slot.status, angle, colors.mark);

        let text_x = icon.x1 + TOAST_STACK_ICON_GAP;
        let close_span = if slot.dismissible {
            TOAST_STACK_CLOSE_BOX + TOAST_STACK_ICON_GAP
        } else {
            0.0
        };
        let text_width =
            (card_origin.x + card_size.width - TOAST_STACK_PADDING - close_span - text_x).max(0.0);
        // Upstream truncates; a shaped single-line run is clipped instead.
        scene.push_clip(
            Point::new(text_x, card_origin.y),
            Size::new(text_width, card_size.height),
        );
        let mut y = card_origin.y + TOAST_STACK_PADDING;
        slot.title.paint(Point::new(text_x, y), colors.ink, scene);
        y += slot.title.size().height;
        if let Some(description) = &slot.description {
            y += TOAST_STACK_TITLE_GAP;
            description.paint(Point::new(text_x, y), colors.muted, scene);
        }
        scene.pop_clip();

        if let Some(label) = &slot.action {
            let rect = Rect::from_origin_size(
                Point::new(
                    text_x,
                    card_origin.y + card_size.height
                        - TOAST_STACK_PADDING
                        - TOAST_STACK_ACTION_HEIGHT,
                ),
                Size::new(
                    label.size().width + TOAST_STACK_ACTION_PADDING_X * 2.0,
                    TOAST_STACK_ACTION_HEIGHT,
                ),
            );
            let wash = if self.hovered_button == Some((slot.id, ToastTarget::Action)) {
                TOAST_STACK_HOVER_WASH
            } else {
                TOAST_STACK_ACTION_WASH
            };
            scene.fill_rounded_rect(
                rect.origin(),
                rect.size(),
                TOAST_STACK_ACTION_HEIGHT / 2.0,
                with_alpha(colors.primary, wash),
            );
            label.paint(
                Point::new(
                    rect.x0 + TOAST_STACK_ACTION_PADDING_X,
                    rect.y0 + (TOAST_STACK_ACTION_HEIGHT - label.size().height) / 2.0,
                ),
                colors.ink,
                scene,
            );
        }

        if slot.dismissible {
            let rect = Rect::from_origin_size(
                Point::new(
                    card_origin.x + card_size.width - TOAST_STACK_PADDING - TOAST_STACK_CLOSE_BOX,
                    card_origin.y + TOAST_STACK_PADDING,
                ),
                Size::new(TOAST_STACK_CLOSE_BOX, TOAST_STACK_CLOSE_BOX),
            );
            let hovered = self.hovered_button == Some((slot.id, ToastTarget::Close));
            if hovered {
                scene.fill_rounded_rect(
                    rect.origin(),
                    rect.size(),
                    TOAST_STACK_CLOSE_BOX / 2.0,
                    with_alpha(colors.primary, TOAST_STACK_HOVER_WASH),
                );
            }
            draw_close_mark(
                scene,
                rect.center(),
                if hovered { colors.ink } else { colors.muted },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{
        BezPath, PointerButton, PointerEvent, scene::GlyphRun, text::TextContext,
    };
    use std::any::Any;

    const AREA: Size = Size::new(600.0, 600.0);

    /// Records the paint calls the stack's geometry and chrome are asserted
    /// through.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size)>,
        layers: Vec<f32>,
        transforms: usize,
        clips: usize,
        strokes: usize,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, _r: f64, _d: f64, _c: Color) {
            self.shadows.push((o, s));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, _t: Affine) {
            self.transforms += 1;
        }
        fn push_clip(&mut self, _o: Point, _s: Size) {
            self.clips += 1;
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    /// What the callbacks report.
    #[derive(Default)]
    struct Log {
        dismissed: Vec<u64>,
        actioned: Vec<u64>,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn entries(ids: &[u64]) -> Vec<AnimatedToastStackEntry> {
        ids.iter()
            .map(|id| animated_toast(*id, format!("Toast {id}")))
            .collect()
    }

    fn view(toasts: Vec<AnimatedToastStackEntry>) -> AnimatedToastStackView<Log> {
        animated_toast_stack::<Log, _>(toasts, |state: &mut Log, id: u64| {
            state.dismissed.push(id);
        })
        .on_action(|state: &mut Log, id: u64| state.actioned.push(id))
    }

    fn build(view: &AnimatedToastStackView<Log>) -> AnimatedToastStackWidget {
        let mut counter = 0u64;
        View::<Log>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(widget: &mut AnimatedToastStackWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(&mut ctx, &BoxConstraints::new(Size::ZERO, AREA))
    }

    fn laid_out(toasts: Vec<AnimatedToastStackEntry>) -> AnimatedToastStackWidget {
        let mut widget = build(&view(toasts));
        layout(&mut widget);
        widget
    }

    fn paint_at(
        widget: &mut AnimatedToastStackWidget,
        ms: f64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool, Option<TickClass>) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, AREA, ft_ms(ms));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        widget.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame(), ctx.frame_class())
    }

    fn rebuilt(
        prev: &AnimatedToastStackView<Log>,
        next: &AnimatedToastStackView<Log>,
        widget: &mut AnimatedToastStackWidget,
    ) {
        let mut counter = 0u64;
        View::<Log>::rebuild(next, prev, widget, &mut BuildCtx::new(&mut counter));
        layout(widget);
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(widget: &mut AnimatedToastStackWidget, event: &InputEvent, state: &mut Log) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, AREA);
        widget.event(&mut ctx, event);
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// Settle everything without a paint pass — the seam a geometry assertion
    /// needs, since `PaintCtx::for_test` reports no hover and paint would
    /// therefore collapse an expansion set by hand.
    fn settle(widget: &mut AnimatedToastStackWidget) {
        layout(widget);
        widget.advance(ft_ms(0.0), true);
    }

    /// The collapsed ladder is `notification-stack.tsx`'s own: 8px of peek and
    /// 12px of inset per depth step, and a scale that follows the inset rather
    /// than being a second authored number.
    #[test]
    fn the_collapsed_ladder_is_peek_and_inset_per_depth() {
        assert_eq!(toast_stack_peek(0), 0.0);
        assert_eq!(toast_stack_peek(1), TOAST_STACK_PEEK);
        assert_eq!(toast_stack_peek(3), 3.0 * TOAST_STACK_PEEK);

        assert_eq!(toast_stack_inset(0, 384.0), 0.0);
        assert_eq!(toast_stack_inset(2, 384.0), 2.0 * TOAST_STACK_INSET);
        assert_eq!(toast_stack_scale(0, 384.0), 1.0);
        assert!((toast_stack_scale(1, 384.0) - (384.0 - 24.0) / 384.0).abs() < 1e-9);
        // A deeper card is always narrower than the one in front of it, and
        // never inverts however deep the queue gets.
        let mut previous = 1.0;
        for depth in 0..40 {
            let scale = toast_stack_scale(depth, 384.0);
            assert!(scale <= previous + 1e-9, "depth {depth} widened");
            assert!((0.0..=1.0).contains(&scale));
            previous = scale;
        }
        // Degenerate widths are defined rather than dividing by zero.
        assert_eq!(toast_stack_scale(3, 0.0), 1.0);
    }

    /// Depth 0 is the **newest** toast — the card's "a new toast pushes the
    /// existing ones back", and the one place this port inverts upstream's own
    /// `zIndex: 20 - index`.
    #[test]
    fn the_newest_toast_holds_the_front_of_the_stack() {
        let widget = laid_out(entries(&[1, 2, 3]));
        assert_eq!(widget.depth(2), Some(0), "the newest card is the front one");
        assert_eq!(widget.depth(1), Some(1));
        assert_eq!(widget.depth(0), Some(2));
    }

    /// The queue keeps everything; the stack shows the last `max_visible`,
    /// exactly `toasts.slice(-maxVisible)`.
    #[test]
    fn only_the_last_max_visible_toasts_are_stacked() {
        let widget = laid_out(entries(&[1, 2, 3, 4, 5, 6]));
        assert_eq!(widget.slot_ids(), vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(
            widget.visible(),
            vec![2, 3, 4, 5],
            "four cards, newest last"
        );
        assert_eq!(widget.depth(1), None, "outside the window");
        assert_eq!(widget.depth(5), Some(0));
    }

    /// The collapsed stack's geometry against the anchored corner, checked by
    /// hand: the front card sits `bottom-6` clear of the bottom edge and
    /// `right-4` clear of the right one, and each card behind it peeks 8px
    /// further out and is inset 12px per side.
    #[test]
    fn the_collapsed_cards_land_on_the_anchored_corner() {
        let mut widget = laid_out(entries(&[1, 2]));
        settle(&mut widget);
        let front = widget.card_rect(1).expect("front card");
        let behind = widget.card_rect(0).expect("card behind");

        assert!((front.x1 - (AREA.width - TOAST_STACK_MARGIN)).abs() < 1e-9);
        assert!((front.y1 - (AREA.height - TOAST_STACK_MARGIN_BOTTOM)).abs() < 1e-9);
        assert!((front.width() - TOAST_STACK_MAX_WIDTH).abs() < 1e-9);

        assert!((behind.y1 - (front.y1 - TOAST_STACK_PEEK)).abs() < 1e-9);
        assert!((behind.x0 - (front.x0 + TOAST_STACK_INSET)).abs() < 1e-9);
        assert!((behind.width() - (front.width() - 2.0 * TOAST_STACK_INSET)).abs() < 1e-9);
        // Collapsed, every card takes the front card's box height.
        assert!((behind.height() - front.height()).abs() < 1e-9);
    }

    /// Hovering spreads the stack into upstream's own `gap-2` column: full
    /// width, no inset, each card one gap clear of the one in front.
    #[test]
    fn hover_expands_the_stack_into_a_gap_separated_list() {
        let mut widget = laid_out(entries(&[1, 2, 3]));
        settle(&mut widget);
        let collapsed = widget.card_rect(1).expect("card");

        let mut log = Log::default();
        let front = widget.card_rect(2).expect("front card");
        dispatch(
            &mut widget,
            &pointer(PointerPhase::Move, front.center()),
            &mut log,
        );
        assert!(
            widget.is_expanded(),
            "a pointer on a card expands the stack"
        );
        settle(&mut widget);

        let expanded_front = widget.card_rect(2).expect("front card");
        let expanded_next = widget.card_rect(1).expect("card behind");
        assert!(
            (expanded_next.x0 - expanded_front.x0).abs() < 1e-9,
            "no inset"
        );
        assert!((expanded_next.width() - TOAST_STACK_MAX_WIDTH).abs() < 1e-9);
        assert!(
            (expanded_next.y1 - (expanded_front.y0 - TOAST_STACK_GAP)).abs() < 1e-9,
            "one gap clear of the card in front"
        );
        assert!(
            expanded_next.y0 < collapsed.y0,
            "the spread pushes older cards further from the edge"
        );
    }

    /// The whole point of the presence driver: a toast the owner drops stays
    /// laid out for the length of its exit ramp and only then leaves the queue.
    #[test]
    fn a_dropped_toast_keeps_its_slot_until_the_exit_settles() {
        let before = view(entries(&[1, 2]));
        let mut widget = build(&before);
        layout(&mut widget);
        paint_at(&mut widget, 0.0, None);

        let after = view(entries(&[1]));
        rebuilt(&before, &after, &mut widget);
        assert_eq!(widget.slot_ids(), vec![1, 2], "still mounted");
        assert_eq!(widget.closing_ids(), vec![2]);

        // Latch the exit's start frame, then run past its 180ms ramp.
        paint_at(&mut widget, 0.0, None);
        assert_eq!(widget.slot_ids(), vec![1, 2], "mid-exit");
        paint_at(&mut widget, 500.0, None);
        assert_eq!(
            widget.slot_ids(),
            vec![1],
            "the exit finished, the slot left"
        );
    }

    /// The acceptance invariant: under interleaved pushes and dismissals the
    /// queue keeps its order, never duplicates an id, and always hands depth 0
    /// to the newest card — including while an older one is still exiting.
    #[test]
    fn rapid_pushes_and_dismissals_keep_order_and_depth() {
        let mut queue = entries(&[1, 2, 3, 4, 5, 6]);
        let mut current = view(queue.clone());
        let mut widget = build(&current);
        layout(&mut widget);
        paint_at(&mut widget, 0.0, None);
        assert_eq!(widget.depth(5), Some(0));

        // Drop one from the middle of the visible window and push two more,
        // without ever letting the exit finish.
        queue.retain(|entry| entry.id() != 5);
        queue.push(animated_toast(7, "Toast 7"));
        queue.push(animated_toast(8, "Toast 8"));
        let next = view(queue.clone());
        rebuilt(&current, &next, &mut widget);
        current = next;

        assert_eq!(widget.slot_ids(), vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(widget.closing_ids(), vec![5], "the dropped id is exiting");
        assert_eq!(widget.depth(7), Some(0), "id 8 took the front");
        // Depths are strictly increasing backwards through the queue, with no
        // gaps and no repeats.
        let depths: Vec<usize> = widget
            .visible()
            .into_iter()
            .map(|index| widget.depth(index).expect("visible"))
            .collect();
        assert_eq!(depths, vec![3, 2, 1, 0]);

        // Let the exit land, then push again: the departed id is gone and the
        // window has slid, with the invariant intact.
        paint_at(&mut widget, 0.0, None);
        paint_at(&mut widget, 500.0, None);
        assert_eq!(widget.slot_ids(), vec![1, 2, 3, 4, 6, 7, 8]);
        assert!(widget.closing_ids().is_empty());

        queue.push(animated_toast(9, "Toast 9"));
        let next = view(queue.clone());
        rebuilt(&current, &next, &mut widget);
        assert_eq!(widget.slot_ids(), vec![1, 2, 3, 4, 6, 7, 8, 9]);
        let ids: Vec<u64> = widget
            .visible()
            .into_iter()
            .map(|index| widget.slots[index].id)
            .collect();
        assert_eq!(ids, vec![6, 7, 8, 9], "the window is the queue's tail");
        assert_eq!(widget.depth(7), Some(0));
    }

    /// A toast re-queued while it is still exiting reverses back in rather than
    /// leaving and being rebuilt from scratch.
    #[test]
    fn a_requeued_toast_reverses_its_exit() {
        let before = view(entries(&[1, 2]));
        let mut widget = build(&before);
        layout(&mut widget);
        paint_at(&mut widget, 0.0, None);

        let dropped = view(entries(&[1]));
        rebuilt(&before, &dropped, &mut widget);
        assert_eq!(widget.closing_ids(), vec![2]);

        let restored = view(entries(&[1, 2]));
        rebuilt(&dropped, &restored, &mut widget);
        assert!(widget.closing_ids().is_empty());
        assert_eq!(widget.slot_ids(), vec![1, 2], "no duplicate, same slot");
    }

    /// The close button reports the toast's own id, on release inside it.
    #[test]
    fn the_close_button_reports_the_toast_id() {
        let mut widget = laid_out(entries(&[1, 2]));
        settle(&mut widget);
        let card = widget.card_rect(1).expect("front card");
        let close = widget.close_rect(card).center();

        let mut log = Log::default();
        dispatch(&mut widget, &pointer(PointerPhase::Down, close), &mut log);
        dispatch(&mut widget, &pointer(PointerPhase::Up, close), &mut log);
        assert_eq!(log.dismissed, vec![2], "the front card is the newest");
    }

    /// The action button reports through its own callback, not through dismiss.
    #[test]
    fn the_action_button_reports_separately() {
        let toasts = vec![animated_toast(1, "Saved").action("Undo")];
        let mut widget = laid_out(toasts);
        settle(&mut widget);
        let card = widget.card_rect(0).expect("card");
        let action = widget.action_rect(0, card).expect("action button").center();

        let mut log = Log::default();
        dispatch(&mut widget, &pointer(PointerPhase::Down, action), &mut log);
        dispatch(&mut widget, &pointer(PointerPhase::Up, action), &mut log);
        assert_eq!(log.actioned, vec![1]);
        assert!(log.dismissed.is_empty());
    }

    /// A swipe past 72px of travel dismisses; a shorter one springs back and
    /// reports nothing. The card itself only ever moves the elastic share of
    /// that travel.
    #[test]
    fn a_swipe_dismisses_only_past_the_threshold() {
        let mut widget = laid_out(entries(&[1]));
        settle(&mut widget);
        let card = widget.card_rect(0).expect("card");
        let start = card.center();
        let mut log = Log::default();

        let short = Point::new(start.x + 40.0, start.y);
        dispatch(&mut widget, &pointer(PointerPhase::Down, start), &mut log);
        dispatch(&mut widget, &pointer(PointerPhase::Move, short), &mut log);
        assert!(
            (widget.slots[0].slide() - 40.0 * TOAST_STACK_DRAG_ELASTIC).abs() < 1e-9,
            "the card travels the elastic share of the drag"
        );
        dispatch(&mut widget, &pointer(PointerPhase::Up, short), &mut log);
        assert!(log.dismissed.is_empty(), "under the threshold");
        assert_eq!(widget.slots[0].drag, 0.0, "the drag is released");

        let far = Point::new(start.x + TOAST_STACK_SWIPE_DISMISS + 5.0, start.y);
        dispatch(&mut widget, &pointer(PointerPhase::Down, start), &mut log);
        dispatch(&mut widget, &pointer(PointerPhase::Move, far), &mut log);
        dispatch(&mut widget, &pointer(PointerPhase::Up, far), &mut log);
        assert_eq!(log.dismissed, vec![1]);
    }

    /// A card marked `dismissible: false` shows no close button and refuses a
    /// swipe, which is upstream's `canDismiss` gate on both affordances.
    #[test]
    fn an_undismissible_toast_refuses_close_and_swipe() {
        let mut widget = laid_out(vec![animated_toast(1, "Working").dismissible(false)]);
        settle(&mut widget);
        let card = widget.card_rect(0).expect("card");
        let mut log = Log::default();

        // The close corner is now plain card surface.
        let close = widget.close_rect(card).center();
        assert_eq!(widget.hit(close), Some((0, ToastTarget::Surface)));

        let far = Point::new(card.center().x + 200.0, card.center().y);
        dispatch(
            &mut widget,
            &pointer(PointerPhase::Down, card.center()),
            &mut log,
        );
        dispatch(&mut widget, &pointer(PointerPhase::Move, far), &mut log);
        dispatch(&mut widget, &pointer(PointerPhase::Up, far), &mut log);
        assert!(log.dismissed.is_empty());
    }

    /// The host is transparent everywhere its cards are not: a press on the
    /// empty area is ignored so the screen underneath still gets it.
    #[test]
    fn a_press_away_from_every_card_passes_through() {
        let mut widget = laid_out(entries(&[1]));
        settle(&mut widget);
        let mut log = Log::default();
        let mut ctx = EventCtx::new(&mut log as &mut dyn Any, Point::ZERO, AREA);
        let result = widget.event(
            &mut ctx,
            &pointer(PointerPhase::Down, Point::new(20.0, 20.0)),
        );
        assert_eq!(result, EventResult::Ignored);
    }

    /// Every tone and every anchor is constructible and paints a card.
    #[test]
    fn every_status_and_position_paints() {
        for status in AnimatedToastStackStatus::ALL {
            for position in AnimatedToastStackPosition::ALL {
                let toasts = vec![animated_toast(1, "Ready").status(status).description("Now")];
                let mut widget = build(&view(toasts).position(position));
                layout(&mut widget);
                let (rec, _, _) = paint_at(&mut widget, 0.0, None);
                assert!(
                    !rec.rrects.is_empty(),
                    "{status:?}/{position:?} painted no surface"
                );
                assert_eq!(rec.shadows.len(), 1, "one shadow per card");
            }
        }
        assert_eq!(AnimatedToastStackStatus::ALL.len(), 5);
        assert_eq!(AnimatedToastStackPosition::ALL.len(), 6);
    }

    /// A top-anchored stack grows the other way: the front card hugs `top-4`
    /// and its neighbours peek *downward*.
    #[test]
    fn a_top_anchored_stack_peeks_downward() {
        let mut widget =
            build(&view(entries(&[1, 2])).position(AnimatedToastStackPosition::TopLeft));
        settle(&mut widget);
        let front = widget.card_rect(1).expect("front card");
        let behind = widget.card_rect(0).expect("card behind");
        assert!((front.y0 - TOAST_STACK_MARGIN).abs() < 1e-9);
        assert!((front.x0 - TOAST_STACK_MARGIN).abs() < 1e-9);
        assert!((behind.y0 - (front.y0 + TOAST_STACK_PEEK)).abs() < 1e-9);
    }

    /// Only the front card's content is drawn while the stack is collapsed —
    /// the neighbours are bare peeking surfaces, `notification-stack.tsx`'s
    /// `invisible` rule — and every card's content is drawn once spread.
    #[test]
    fn collapsed_neighbours_paint_no_content() {
        let mut widget = laid_out(entries(&[1, 2, 3]));
        settle(&mut widget);
        assert_eq!(widget.content_alpha(2), 1.0, "front card");
        assert_eq!(widget.content_alpha(1), 0.0, "collapsed neighbour");
        widget.set_expanded(true);
        settle(&mut widget);
        assert_eq!(widget.content_alpha(1), 1.0, "spread");
        assert_eq!(widget.content_alpha(0), 1.0);
    }

    /// `reduce_motion` lands every card at rest on the frame it first paints:
    /// no entrance layer, and no frame owed.
    #[test]
    fn reduce_motion_lands_the_stack_at_rest() {
        let theme = reduced();
        let mut widget = laid_out(entries(&[1, 2]));
        let (rec, needs_frame, _) = paint_at(&mut widget, 0.0, Some(&theme));
        assert!(rec.layers.is_empty(), "nothing is mid-ramp");
        assert!(!needs_frame, "a settled stack asks for no frame");

        // Against a motion-on theme the same first frame is mid-entrance.
        let mut widget = laid_out(entries(&[1, 2]));
        let (rec, needs_frame, _) = paint_at(&mut widget, 0.0, Some(&crate::theme()));
        assert!(needs_frame, "the entrance owes a frame");
        assert!(rec.layers.iter().any(|alpha| *alpha < 1.0));
    }

    /// The loading tone spins forever, so it asks for a *cosmetic* tick rather
    /// than a transition frame once the entrance has settled.
    #[test]
    fn a_loading_toast_asks_for_a_cosmetic_tick_at_rest() {
        let toasts = vec![animated_toast(1, "Uploading").status(AnimatedToastStackStatus::Loading)];
        let mut widget = laid_out(toasts);
        paint_at(&mut widget, 0.0, None);
        let (_, needs_frame, class) = paint_at(&mut widget, 2_000.0, None);
        assert!(needs_frame);
        assert_eq!(class, Some(TickClass::CosmeticLoop));

        // A settled non-spinning stack asks for nothing at all.
        let mut widget = laid_out(entries(&[1]));
        paint_at(&mut widget, 0.0, None);
        let (_, needs_frame, _) = paint_at(&mut widget, 2_000.0, None);
        assert!(!needs_frame);
    }

    /// The themed and unthemed palettes agree on the roles beUI folds them onto,
    /// and a status is genuinely tinted rather than falling back to the ink.
    #[test]
    fn the_status_palette_reads_the_themed_roles() {
        let theme = crate::theme();
        let scheme = theme.scheme();
        let neutral = resolve_colors(AnimatedToastStackStatus::Neutral, Some(&theme));
        assert_eq!(neutral.ink, scheme.on_surface);
        assert_eq!(neutral.border, scheme.outline_variant);
        assert_eq!(neutral.mark, scheme.on_surface_variant);

        let error = resolve_colors(AnimatedToastStackStatus::Error, Some(&theme));
        assert_eq!(error.mark, scheme.error);
        let success = resolve_colors(AnimatedToastStackStatus::Success, Some(&theme));
        assert_eq!(success.mark, BeuiTokens::beui().success);

        // Unthemed falls back to the vendored light table, not to black.
        let unthemed = resolve_colors(AnimatedToastStackStatus::Info, None);
        assert_eq!(unthemed.mark, crate::BEUI_LIGHT.primary);
        assert_eq!(unthemed.ink, crate::BEUI_LIGHT.foreground);
    }

    /// One accessible item per visible toast, newest first, plus a button node
    /// for each affordance the card actually shows.
    #[test]
    fn semantics_publish_the_visible_toasts() {
        use frust::authoring::SemanticsUpdate;

        let mut root = frust_core::RenderRoot::new();
        let mut state = Log::default();
        let mut tcx = TextContext::new();
        let mut logic = move |_s: &mut Log| {
            view(vec![
                animated_toast(1, "Saved").action("Undo"),
                animated_toast(2, "Deleted").dismissible(false),
            ])
        };
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(AREA, &mut tcx as &mut dyn Any);

        let update: SemanticsUpdate = root.semantics();
        let items = update
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::ListItem)
            .count();
        assert_eq!(items, 2, "one node per visible toast");
        let buttons = update
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::Button)
            .count();
        assert_eq!(buttons, 2, "toast 1's action and close, toast 2's neither");
    }
}
