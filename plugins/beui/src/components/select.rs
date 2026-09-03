//! Ports beUI's **Select** — the trigger whose panel pinches off it and
//! separates, plus the *morph* variant where the trigger grows into the panel
//! as one continuous surface.
//!
//! Source: `components/motion/select.tsx` and `components/motion/select-morph.tsx`
//! (beUI v2, rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved
//! 2026-09-01), registry slug `select`: *"Composable select primitives whose
//! panel bouncily unfolds out of the trigger and separates, plus a Morph variant
//! where the trigger grows into the panel via shared layout."* The registry
//! lists the two as one slug with two example variants (`default`, `morph`), so
//! they land here as one [`SelectVariant`] rather than two modules.
//!
//! | class / prop | here |
//! |---|---|
//! | trigger `rounded-xl border border-border bg-background px-3 py-2 text-sm` | [`TRIGGER_RADIUS`], [`style::BORDER_WIDTH`], [`TRIGGER_PADDING_X`], [`style::TEXT_SM`] |
//! | trigger `hover:border-(--color-border-strong)` | `outline` (beUI's `--border-strong`) |
//! | chevron `animate={{ rotate: open ? 180 : 0 }}` + spring | [`CHEVRON_SPIN`] on [`SPRING_LAYOUT`] |
//! | gooey near-edge radius `kf = open ? [0,0,12] : [12,0,12]` | [`near_radius`] |
//! | panel `rounded-xl border border-border bg-background shadow-lg` | [`PANEL_RADIUS`], `surface` |
//! | panel `marginTop: 8` on a bouncy spring | [`PANEL_GAP`], the separation lane |
//! | list `staggerChildren: 0.035, delayChildren: 0.05` | [`ITEM_STAGGER_MS`] / [`ITEM_DELAY_MS`] |
//! | item `opacity 0→1, y −6→0, blur(3px)→0` | [`ITEM_RISE`], the blur dropped |
//! | item `rounded-lg px-2.5 py-1.5 text-sm` | [`ITEM_RADIUS`], [`ITEM_PADDING_X`], [`ITEM_PADDING_Y`] |
//! | selected item `bg-muted text-foreground` + `Check h-3.5 w-3.5` | `surface_container_highest`, [`CHECK_SIZE`] |
//! | morph `layoutId` surface on `spring duration 0.5 bounce 0.22` | the surface morph on [`SPRING_LAYOUT`] |
//! | morph `delayChildren: 0.08` | [`MORPH_ITEM_DELAY_MS`] |
//!
//! # Shape of the port: a trigger view and a panel view, both app-driven
//!
//! Upstream is five composable primitives sharing a React context (`Select`,
//! `SelectTrigger`, `SelectValue`, `SelectContent`, `SelectItem`) that owns the
//! open flag, the value, and a label registry each item writes itself into.
//! None of that plumbing has a counterpart here — a frust app already owns its
//! state — so the port is the two halves an app actually mounts, exactly like
//! the sibling catalog's select:
//!
//! * [`select_trigger`] — the closed control, capturing its own window rect into
//!   an [`OverlayAnchor`] and reporting an open toggle.
//! * [`select`] — the panel, mounted as the top child of a full-area
//!   [`frust::Stack`] (or a transparent navigator page) and anchored to that
//!   same rect through [`crate::overlay::anchored`](mod@crate::overlay::anchored).
//!
//! The label registry collapses into the option list the app hands both halves;
//! nothing registers anything at runtime.
//!
//! # Dismissal is the seam's, not this component's
//!
//! An outside press and Escape both dismiss through
//! [`crate::overlay::anchored`](mod@crate::overlay::anchored)'s host — once, and reported as
//! `on_open_change(state, false)`. This module owns no dismissal logic of its
//! own, which is why a press landing on the panel's own background is swallowed
//! by the host rather than by the list.
//!
//! # v1 boundaries, adopted from the sibling catalog
//!
//! Both are the shadcn select's own stated gaps, kept identical here so the two
//! catalogs' selects behave the same way:
//!
//! * **No type-ahead.** Upstream's `<button role="option">` list inherits the
//!   browser's typed-character jump; the pointer is the only way to move the
//!   highlight here, and there is no keyboard list navigation.
//! * **The height cap is a constant.** Upstream's panel is sized by the
//!   document; the overlay host publishes no available-height measurement, so
//!   the list caps at [`SELECT_MAX_HEIGHT`] and clips (the host's own flip still
//!   moves it above the trigger when that fits better).
//!
//! # Degradations
//!
//! - **No blur on the item entrance.** `filter: "blur(3px)"` has no counterpart
//!   in this scene's paint vocabulary; the opacity-and-rise is ported, the blur
//!   is dropped — the same call `input` made for its message row.
//! - **The morph is a surface morph, not a shared-layout morph.** Upstream's
//!   `layoutId` hands one DOM node from trigger to panel, so the *text* inside
//!   travels too. Here the panel's painted surface springs from the trigger's
//!   captured rect to its own placed rect (geometry and radius) while the rows
//!   fade in against it, and the trigger fades out under
//!   [`SelectVariant::Morph`] — one continuous surface, but its content
//!   cross-fades rather than travels.
//! - **The morph panel sits below the trigger, not over it.** Upstream's morph
//!   panel is `absolute inset-x-0 top-0`, covering the trigger it grew from.
//!   The overlay host places a panel *off* an anchor edge, so the morph variant
//!   opens flush against the trigger's bottom edge ([`MORPH_OFFSET`]) and grows
//!   there. The surface still starts on the trigger's own rect, so the growth
//!   still reads as coming out of it.
//! - **The gooey separation is one lane, not four animated corners.** Upstream
//!   animates all four corner radii of both surfaces with per-corner
//!   transitions; the port animates the *near* edge's two corners together
//!   ([`near_radius`]) and leaves the far edge at [`TRIGGER_RADIUS`], which is
//!   what the four-corner bookkeeping upstream exists to achieve.
//! - **The trigger's near edge is always its bottom one.** Upstream's context
//!   publishes the resolved placement back to the trigger so a panel that
//!   flipped upward flattens the *top* edge instead. The overlay host resolves
//!   its flip inside itself and publishes nothing, so the trigger always
//!   flattens downward.
//! - **No scroll.** A list longer than [`SELECT_MAX_HEIGHT`] is clipped rather
//!   than scrolled: a scrollable panel would have to mount inside a
//!   [`frust::scroll_view`], which the overlay seam forbids under a host.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, CornerRadii,
    CursorIcon, ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View,
    Widget, erase_callback_arg,
};
use frust::{FrameTime, Theme};

use super::checkbox::{CHECK_POINTS, ICON_VIEWBOX, mark_path};
use crate::motion::{Ramp, Stagger};
use crate::overlay::anchored::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlaySide, anchored,
};
use crate::press::{Lane, inside, is_activation_key, presses};
use crate::style;
use crate::text::{LabelRun, label_style};
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT, SPRING_PANEL};

// ---- Metrics ---------------------------------------------------------------

/// The trigger's height, in logical px — the catalog's default control height.
///
/// Upstream sizes its trigger from `py-2` plus a `text-sm` line box, which lands
/// within a pixel of [`style::HEIGHT_MD`]; the port names the catalog rung
/// instead so a select and a button of the same size line up exactly.
pub const TRIGGER_HEIGHT: f64 = style::HEIGHT_MD;

/// The trigger's corner radius, in logical px (`rounded-xl`) — deliberately not
/// [`style::RADIUS_CONTROL`], since upstream squares this control off rather
/// than making it a pill.
pub const TRIGGER_RADIUS: f64 = style::RADIUS_XL;

/// Horizontal padding inside the trigger, in logical px (`px-3`).
pub const TRIGGER_PADDING_X: f64 = 12.0;

/// Gap between the trigger's label and its chevron, in logical px (`gap-2`).
pub const TRIGGER_GAP: f64 = style::GAP_MD;

/// The chevron's box, in logical px (`h-4 w-4`).
pub const CHEVRON_BOX: f64 = style::ICON_SIZE;

/// How far the chevron turns when the panel opens, in radians
/// (`rotate: open ? 180 : 0`).
pub const CHEVRON_SPIN: f64 = std::f64::consts::PI;

/// The panel's corner radius, in logical px (`rounded-xl`).
pub const PANEL_RADIUS: f64 = style::RADIUS_XL;

/// Padding inside the panel, around the list, in logical px (`p-1`).
pub const PANEL_PADDING: f64 = style::SPACING_UNIT;

/// The gap that opens between the trigger and a separated panel, in logical px
/// (`marginTop: open ? 8 : 0`).
pub const PANEL_GAP: f64 = 8.0;

/// The gap a [`SelectVariant::Morph`] panel opens at: none, so the growing
/// surface stays attached to the trigger it came out of.
pub const MORPH_OFFSET: f64 = 0.0;

/// An item's corner radius, in logical px (`rounded-lg`).
pub const ITEM_RADIUS: f64 = style::RADIUS_LG;

/// Horizontal padding inside an item, in logical px (`px-2.5`).
pub const ITEM_PADDING_X: f64 = 10.0;

/// Vertical padding inside an item, in logical px (`py-1.5`).
pub const ITEM_PADDING_Y: f64 = 6.0;

/// An item's height, in logical px — `py-1.5` around a `text-sm` line.
pub const ITEM_HEIGHT: f64 = style::TEXT_SM * 1.5 + ITEM_PADDING_Y * 2.0;

/// The selected item's check, in logical px (`h-3.5 w-3.5`).
pub const CHECK_SIZE: f64 = 14.0;

/// The check's stroke width in [`ICON_VIEWBOX`] units — lucide's own default.
pub const CHECK_STROKE_VIEWBOX: f64 = 2.0;

/// How far an entering item rises into place, in logical px (`y: -6 → 0`).
pub const ITEM_RISE: f64 = 6.0;

/// Per-item stagger delay, in milliseconds (`staggerChildren: 0.035`).
pub const ITEM_STAGGER_MS: u64 = 35;

/// Delay before the first item moves, in milliseconds (`delayChildren: 0.05`).
pub const ITEM_DELAY_MS: u64 = 50;

/// Delay before the first item moves in the morph variant, in milliseconds
/// (`delayChildren: 0.08` — the surface morph gets a head start).
pub const MORPH_ITEM_DELAY_MS: u64 = 80;

/// How long a separated panel's exit takes, in milliseconds — upstream's
/// closing `height` transition (`duration: 0.26`).
pub const PANEL_FOLD_MS: u64 = 260;

/// How long the gooey open keyframe track runs, in milliseconds
/// (`duration: 0.6`).
pub const GOOEY_OPEN_MS: f64 = 600.0;

/// How long the gooey close keyframe track runs, in milliseconds
/// (`duration: 0.42`).
pub const GOOEY_CLOSE_MS: f64 = 420.0;

/// The fraction of the open track at which the near edge starts rounding back
/// (`times: [0, 0.4, 1]`).
pub const GOOEY_OPEN_HOLD: f64 = 0.4;

/// The fraction of the close track at which the near edge reaches flat
/// (`times: [0, 0.5, 1]`).
pub const GOOEY_CLOSE_PIVOT: f64 = 0.5;

/// The list viewport's cap, in logical px — this port's stand-in for a measured
/// available height (see the [module docs](self)).
pub const SELECT_MAX_HEIGHT: f64 = 300.0;

/// Opacity of a disabled control: `disabled:opacity-50`.
const DISABLED_OPACITY: f32 = style::DISABLED_OPACITY;

/// Unthemed fallback surface — the light table's `--background`.
const FALLBACK_SURFACE: Color = crate::BEUI_LIGHT.background;
/// Unthemed fallback ink — the light table's `--foreground`.
const FALLBACK_FOREGROUND: Color = crate::BEUI_LIGHT.foreground;
/// Unthemed fallback dimmed ink — the light table's `--muted-foreground`.
const FALLBACK_MUTED: Color = crate::BEUI_LIGHT.muted_foreground;
/// Unthemed fallback hairline — the light table's `--border`.
const FALLBACK_BORDER: Color = crate::BEUI_LIGHT.border;
/// Unthemed fallback strong hairline — the light table's `--border-strong`.
const FALLBACK_BORDER_STRONG: Color = crate::BEUI_LIGHT.border_strong;
/// Unthemed fallback row wash — the light table's `--muted`.
const FALLBACK_MUTED_SURFACE: Color = crate::BEUI_LIGHT.muted;

// ---- Public data -----------------------------------------------------------

/// Which select this is: the separating panel, or the morphing surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectVariant {
    /// The registry's `default` example: the panel pinches off the trigger and
    /// separates, with staggered items.
    #[default]
    Default,
    /// The registry's `morph` example: the trigger grows into the panel as one
    /// surface and shrinks back, never detaching.
    Morph,
}

/// One option in a [`select`] list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOption {
    label: String,
    disabled: bool,
}

/// Create an option labelled `label`.
pub fn select_option(label: impl Into<String>) -> SelectOption {
    SelectOption {
        label: label.into(),
        disabled: false,
    }
}

impl SelectOption {
    /// Make the option unselectable (`disabled:pointer-events-none`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The option's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Whether the option is unselectable.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

// ---- Shared geometry and paint helpers -------------------------------------

/// The near edge's corner radius partway through the gooey keyframe track, in
/// logical px.
///
/// Upstream animates the two corners facing the panel through a three-value
/// keyframe array with explicit `times`: opening runs `[0, 0, 12]` at
/// `[0, 0.4, 1]` (flat while the panel is attached, rounding back as it pulls
/// away), closing runs `[12, 0, 12]` at `[0, 0.5, 1]` (flattening as the panel
/// arrives, rounding once it has gone). `t` is the fraction of the *current*
/// track, so a caller times it against [`GOOEY_OPEN_MS`] or [`GOOEY_CLOSE_MS`].
///
/// Unlike `crate::press::keyframes_at`, which samples an evenly-spaced array,
/// these two tracks carry their own `times` — which is exactly why they are
/// spelled out here rather than routed through it.
pub fn near_radius(opening: bool, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if opening {
        if t <= GOOEY_OPEN_HOLD {
            0.0
        } else {
            TRIGGER_RADIUS * (t - GOOEY_OPEN_HOLD) / (1.0 - GOOEY_OPEN_HOLD)
        }
    } else if t <= GOOEY_CLOSE_PIVOT {
        TRIGGER_RADIUS * (1.0 - t / GOOEY_CLOSE_PIVOT)
    } else {
        TRIGGER_RADIUS * (t - GOOEY_CLOSE_PIVOT) / (1.0 - GOOEY_CLOSE_PIVOT)
    }
}

/// Paint a lucide `ChevronDown` centred on `centre`, turned `angle` radians.
///
/// Crate-internal rather than private: the combobox and multi-select triggers
/// draw the same affordance, and a second copy would be a second set of arm
/// proportions to keep in sync.
pub(crate) fn draw_chevron(
    scene: &mut dyn PaintScene,
    centre: Point,
    angle: f64,
    color: Color,
    box_size: f64,
) {
    let arm = box_size * 0.28;
    let drop = box_size * 0.16;
    let mut path = BezPath::new();
    path.move_to(Point::new(-arm, -drop));
    path.line_to(Point::new(0.0, drop));
    path.line_to(Point::new(arm, -drop));
    scene.push_transform(Affine::translate(centre.to_vec2()) * Affine::rotate(angle));
    scene.stroke_path(
        Point::ZERO,
        &path,
        style::BORDER_WIDTH * 1.75,
        &Brush::Solid(color),
    );
    scene.pop_transform();
}

/// The stroke width the selected-option check is drawn at, in logical px.
pub(crate) fn check_stroke_width(size: f64) -> f64 {
    CHECK_STROKE_VIEWBOX * size / ICON_VIEWBOX
}

/// Paint the selected-option check at `origin`, `size` on a side.
pub(crate) fn draw_check(scene: &mut dyn PaintScene, origin: Point, size: f64, color: Color) {
    let path = mark_path(&CHECK_POINTS, size, 1.0);
    scene.stroke_path(
        origin,
        &path,
        check_stroke_width(size),
        &Brush::Solid(color),
    );
}

/// The catalog's panel palette, resolved once per paint.
pub(crate) struct PanelColors {
    /// `bg-background` — the panel's own fill.
    pub surface: Color,
    /// `border-border` — its hairline.
    pub border: Color,
    /// `hover:border-(--color-border-strong)` — a hovered control's hairline.
    pub border_strong: Color,
    /// `text-foreground` — a selected or hovered row's ink.
    pub ink: Color,
    /// `text-muted-foreground` — a resting row's ink.
    pub muted: Color,
    /// `bg-muted` — the selected/hovered row wash.
    pub wash: Color,
}

/// Resolve the panel palette, falling back to the vendored light table when no
/// theme is threaded.
pub(crate) fn panel_colors(theme: Option<&Theme>) -> PanelColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            PanelColors {
                surface: scheme.surface,
                border: scheme.outline_variant,
                border_strong: scheme.outline,
                ink: scheme.on_surface,
                muted: scheme.on_surface_variant,
                wash: scheme.surface_container_highest,
            }
        }
        None => PanelColors {
            surface: FALLBACK_SURFACE,
            border: FALLBACK_BORDER,
            border_strong: FALLBACK_BORDER_STRONG,
            ink: FALLBACK_FOREGROUND,
            muted: FALLBACK_MUTED,
            wash: FALLBACK_MUTED_SURFACE,
        },
    }
}

/// Stroke a hairline just inside a `size`-shaped rounded box at `origin`.
pub(crate) fn stroke_frame(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    radius: f64,
    color: Color,
) {
    let inset = style::BORDER_WIDTH / 2.0;
    let frame = RoundedRect::from_rect(
        Rect::from_origin_size(Point::ORIGIN, size).inset(-inset),
        (radius - inset).max(0.0),
    );
    scene.stroke_path(
        origin,
        &Shape::to_path(&frame, style::PATH_TOLERANCE),
        style::BORDER_WIDTH,
        &Brush::Solid(color),
    );
}

// ---- The trigger -----------------------------------------------------------

/// A view-held, typed open-change callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative beUI select trigger. See [`select_trigger`].
pub struct SelectTriggerView<State: 'static> {
    anchor: OverlayAnchor,
    label: Option<String>,
    placeholder: String,
    variant: SelectVariant,
    open: bool,
    disabled: bool,
    on_open_change: OnOpenChange<State>,
}

/// Create the select's trigger: the closed control showing `label` (or its
/// placeholder), capturing its own window rect into `anchor`, and reporting a
/// press as an open toggle.
///
/// `label` is the *selected option's* label, which the app already holds —
/// upstream's runtime label registry has no counterpart here (see the [module
/// docs](self)).
pub fn select_trigger<State: 'static>(
    anchor: &OverlayAnchor,
    label: Option<String>,
) -> SelectTriggerView<State> {
    SelectTriggerView {
        anchor: anchor.clone(),
        label,
        placeholder: String::from("Select"),
        variant: SelectVariant::default(),
        open: false,
        disabled: false,
        on_open_change: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> SelectTriggerView<State> {
    /// Set the text shown with nothing selected (`placeholder ?? "Select"`).
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Pick the variant. Under [`SelectVariant::Morph`] the trigger fades out
    /// while the panel is open, since the panel *is* the trigger there.
    pub fn variant(mut self, variant: SelectVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Tell the trigger whether the panel is open — the chevron's spin, the
    /// gooey near edge and (under `Morph`) the fade all read this.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Disable the trigger: half opacity, inert, a not-allowed cursor.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set the open-toggle callback: a release inside reports the flipped flag.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Rc::new(on_open_change);
        self
    }
}

/// The retained widget for a [`SelectTriggerView`].
pub struct SelectTriggerWidget {
    anchor: OverlayAnchor,
    label: Option<String>,
    text: LabelRun,
    placeholder: String,
    variant: SelectVariant,
    /// The app-confirmed open flag (source of truth; adopted on `rebuild`).
    open: bool,
    disabled: bool,
    /// The chevron's spin, `0.0` closed .. `1.0` open.
    spin: Lane,
    /// The morph fade, `1.0` visible .. `0.0` gone.
    fade: Lane,
    /// When the current gooey keyframe track started, and which way it runs. A
    /// zero start means "staged by a rebuild, not yet latched by a paint".
    gooey: Option<(FrameTime, bool)>,
    hovered: bool,
    captured: bool,
    on_open_change: ErasedArgCallback<bool>,
}

impl SelectTriggerWidget {
    /// The text this trigger shows: the selected label, else the placeholder.
    fn shown(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.placeholder)
    }

    /// How far into the current gooey track `now` is, as a fraction of it.
    fn gooey_fraction(&self, now: FrameTime) -> Option<(bool, f64)> {
        let (started, opening) = self.gooey?;
        let span = if opening {
            GOOEY_OPEN_MS
        } else {
            GOOEY_CLOSE_MS
        };
        let elapsed = now.saturating_sub(started).as_secs_f64() * 1000.0;
        Some((opening, elapsed / span))
    }

    /// The near edge's radius right now, in logical px.
    fn gooey_radius(&self, now: FrameTime) -> f64 {
        self.gooey_fraction(now)
            .map_or(TRIGGER_RADIUS, |(opening, t)| near_radius(opening, t))
    }

    /// Whether the gooey track is still running at `now`.
    fn gooey_running(&self, now: FrameTime) -> bool {
        self.gooey_fraction(now).is_some_and(|(_, t)| t < 1.0)
    }

    /// The cursor this control asks for in its current state.
    fn cursor(&self) -> CursorIcon {
        if self.disabled {
            style::DISABLED_CURSOR
        } else {
            style::ACTIVE_CURSOR
        }
    }
}

impl<State: 'static> View<State> for SelectTriggerView<State> {
    type Element = SelectTriggerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SelectTriggerWidget {
        let rest = if self.open { 1.0 } else { 0.0 };
        let hidden = self.open && self.variant == SelectVariant::Morph;
        SelectTriggerWidget {
            anchor: self.anchor.clone(),
            label: self.label.clone(),
            text: LabelRun::new(
                self.label
                    .clone()
                    .unwrap_or_else(|| self.placeholder.clone()),
            ),
            placeholder: self.placeholder.clone(),
            variant: self.variant,
            open: self.open,
            disabled: self.disabled,
            spin: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), rest),
            fade: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), if hidden { 0.0 } else { 1.0 }),
            gooey: None,
            hovered: false,
            captured: false,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SelectTriggerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled every pass.
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        element.anchor = self.anchor.clone();
        let mut flags = ChangeFlags::NONE;
        if prev.label != self.label || prev.placeholder != self.placeholder {
            element.label = self.label.clone();
            element.placeholder = self.placeholder.clone();
            let shown = element.shown().to_string();
            if element.text.set_content(shown) {
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if prev.open != self.open {
            element.open = self.open;
            element.spin.retarget(if self.open { 1.0 } else { 0.0 });
            let hidden = self.open && element.variant == SelectVariant::Morph;
            element.fade.retarget(if hidden { 0.0 } else { 1.0 });
            // A literal keyframe timeline, not a retargetable lane: it restarts
            // from its own beginning, latched by the next paint.
            element.gooey = Some((FrameTime::from_nanos(0), self.open));
            flags |= ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                // A control disabled mid-press keeps no armed state behind.
                element.captured = false;
                element.hovered = false;
            }
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for SelectTriggerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let text = self.text.layout(ctx, &label_style(style::TEXT_SM));
        // `w-full` upstream: the trigger takes the width it is offered, and
        // falls back to its own content when that is unbounded.
        let natural = text.width + TRIGGER_PADDING_X * 2.0 + TRIGGER_GAP + CHEVRON_BOX;
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            natural
        };
        bc.constrain(Size::new(width, TRIGGER_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative for whether the pointer is on
        // this widget's path at all.
        if !ctx.is_hovered() || self.disabled {
            self.hovered = false;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let colors = panel_colors(theme);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let size = ctx.size();
        let origin = ctx.origin();

        // The anchored panel is placed against this rect, and `PaintCtx::origin`
        // is the one read that answers where the trigger sits on screen.
        self.anchor.set(Rect::from_origin_size(origin, size));

        let mut owes_frame = false;
        if reduce {
            self.spin.snap();
            self.fade.snap();
            self.gooey = None;
        } else {
            owes_frame |= self.spin.advance(now);
            owes_frame |= self.fade.advance(now);
            // The gooey track latches its start on the first frame after the
            // flag changed, so it is timed from the frame it first painted.
            if let Some((started, opening)) = self.gooey
                && started.as_nanos() == 0
            {
                self.gooey = Some((now, opening));
            }
            owes_frame |= self.gooey_running(now);
        }

        let alpha = self.fade.value().clamp(0.0, 1.0) as f32;
        if alpha <= 0.0 {
            // Faded out under the morph panel: nothing to paint, but the anchor
            // above still had to be written, which is what the panel morphs from.
            if owes_frame {
                ctx.request_frame();
            }
            return;
        }
        let layered = alpha < 1.0;
        if layered {
            scene.push_layer(origin, size, alpha);
        }

        let tint = |color: Color| style::disabled_tint(color, self.disabled, DISABLED_OPACITY);
        let near = if reduce {
            TRIGGER_RADIUS
        } else {
            self.gooey_radius(now)
        };
        // The panel opens below by default, so the *bottom* corners are the near
        // ones (see the module docs' placement note).
        scene.fill_rounded_rect_radii(
            origin,
            size,
            CornerRadii::new(TRIGGER_RADIUS, TRIGGER_RADIUS, near, near),
            tint(colors.surface),
        );

        let hairline = if self.hovered {
            colors.border_strong
        } else {
            colors.border
        };
        stroke_frame(scene, origin, size, TRIGGER_RADIUS, tint(hairline));

        let text = self.text.size();
        let ink = if self.label.is_some() {
            colors.ink
        } else {
            colors.muted
        };
        self.text.paint(
            Point::new(
                origin.x + TRIGGER_PADDING_X,
                origin.y + (size.height - text.height) / 2.0,
            ),
            tint(ink),
            scene,
        );

        draw_chevron(
            scene,
            Point::new(
                origin.x + size.width - TRIGGER_PADDING_X - CHEVRON_BOX / 2.0,
                origin.y + size.height / 2.0,
            ),
            self.spin.value().clamp(0.0, 1.0) * CHEVRON_SPIN,
            tint(colors.muted),
            CHEVRON_BOX,
        );

        if layered {
            scene.pop_layer();
        }
        // Paint-only animation (the trigger never resizes), so a bare frame
        // request is the right one.
        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if self.disabled || !is_activation_key(key) {
                    return EventResult::Ignored;
                }
                (self.on_open_change)(ctx, !self.open);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => {
                let size = ctx.size();
                match p.phase {
                    PointerPhase::Down => {
                        if self.disabled || !presses(p) || !inside(p.position, size) {
                            return EventResult::Ignored;
                        }
                        self.captured = true;
                        ctx.capture_pointer();
                        ctx.request_focus();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        if !self.captured {
                            let over = inside(p.position, size);
                            if over {
                                ctx.claim_hover();
                                ctx.set_cursor(self.cursor());
                            }
                            let hovered = over && !self.disabled;
                            if self.hovered != hovered {
                                self.hovered = hovered;
                                ctx.request_redraw();
                            }
                            return EventResult::Ignored;
                        }
                        ctx.set_cursor(self.cursor());
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        self.captured = false;
                        if inside(p.position, size) {
                            (self.on_open_change)(ctx, !self.open);
                        }
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        // Internal state only — never the callback.
                        self.captured = false;
                        EventResult::Handled
                    }
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::ComboBox, |node| {
            node.set_label(self.shown());
            node.set_expanded(self.open);
            if self.disabled {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
        });
    }
}

// ---- The panel -------------------------------------------------------------

/// What the panel needs from its builder chain after the content view has
/// already been wrapped in the host — the same late-binding handle idiom the
/// sibling catalog's select uses.
#[derive(Default)]
struct PanelConfig {
    anchor: Option<OverlayAnchor>,
    open: bool,
    variant: SelectVariant,
    /// The morph header's text: the selected label, else the placeholder.
    header: String,
    /// Whether `header` is a real selection rather than the placeholder.
    header_selected: bool,
}

/// A shared handle onto the panel's late-bound configuration.
type PanelHandle = Rc<RefCell<PanelConfig>>;

/// A declarative beUI select panel. See [`select`].
pub struct SelectView<State: 'static> {
    inner: AnchoredOverlayView<State>,
    config: PanelHandle,
    placement: OverlayPlacement,
    variant: SelectVariant,
}

/// Build a select panel over `options`, showing `options[selected]` as checked
/// and reporting a commit through `on_select(state, option_index)`.
///
/// Mount it as the top child of a full-area [`frust::Stack`] (or a transparent
/// navigator page) and hand it the app's open flag through [`SelectView::open`],
/// so a close plays the panel's exit — see [`crate::overlay::anchored`](mod@crate::overlay::anchored) for the
/// mounting contract.
pub fn select<State: 'static, F: Fn(&mut State, usize) + 'static>(
    options: Vec<SelectOption>,
    selected: Option<usize>,
    on_select: F,
) -> SelectView<State> {
    let config: PanelHandle = Rc::new(RefCell::new(PanelConfig {
        open: true,
        ..PanelConfig::default()
    }));
    let content = SelectPanelView {
        options,
        selected,
        config: config.clone(),
        on_select: Rc::new(on_select),
    };
    // The list lines up with the trigger's leading edge, not its centre.
    let placement = OverlayPlacement::default()
        .align(OverlayAlign::Start)
        .offset(PANEL_GAP);
    SelectView {
        inner: anchored(content).placement(placement),
        config,
        placement,
        variant: SelectVariant::Default,
    }
}

impl<State: 'static> SelectView<State> {
    /// Anchor the panel to the trigger's captured rect — which is also where its
    /// minimum width and the morph's starting geometry come from.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.config.borrow_mut().anchor = Some(anchor.clone());
        self.inner = self.inner.anchor(anchor);
        self
    }

    /// Pick the variant, which also re-times the host's own staging — see the
    /// [module docs](self) on why the morph disables it.
    pub fn variant(mut self, variant: SelectVariant) -> Self {
        self.variant = variant;
        self.config.borrow_mut().variant = variant;
        self.placement.offset = match variant {
            SelectVariant::Default => PANEL_GAP,
            SelectVariant::Morph => MORPH_OFFSET,
        };
        self.inner = self.inner.placement(self.placement);
        self.inner = match variant {
            // The host's scale-and-fade entrance is the separating panel's own
            // arrival; the gooey separation runs inside it.
            SelectVariant::Default => self
                .inner
                .enter(Ramp::spring(SPRING_PANEL))
                .exit(Ramp::eased(Duration::from_millis(PANEL_FOLD_MS), EASE_OUT)),
            // The morph *is* the animation, so the host contributes nothing on
            // the way in and only keeps the panel mounted on the way out.
            SelectVariant::Morph => self
                .inner
                .enter(Ramp::eased(Duration::ZERO, EASE_OUT))
                .exit(Ramp::spring(SPRING_LAYOUT)),
        };
        self
    }

    /// Set the morph header's text — the selected label, else the placeholder.
    ///
    /// Only [`SelectVariant::Morph`] paints it: the morph surface carries the
    /// trigger's own row at its top, which is what makes the growth continuous.
    pub fn header(self, label: Option<String>, placeholder: impl Into<String>) -> Self {
        {
            let mut config = self.config.borrow_mut();
            config.header_selected = label.is_some();
            config.header = label.unwrap_or_else(|| placeholder.into());
        }
        self
    }

    /// Set the side the panel opens on (default `bottom`).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self.inner = self.inner.placement(self.placement);
        self
    }

    /// Set the cross-axis alignment (default `start`).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self.inner = self.inner.placement(self.placement);
        self
    }

    /// Hand a **kept-mounted** panel the app's open flag, so closing it plays
    /// the exit instead of vanishing. The default is `true`.
    pub fn open(mut self, open: bool) -> Self {
        self.config.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// Set the open-change callback: an outside press or a focus-routed Escape
    /// reports `false`, through the overlay seam.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.inner = self
            .inner
            .on_dismiss(move |state| on_open_change(state, false));
        self
    }
}

impl<State: 'static> View<State> for SelectView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

/// A view-held, typed commit callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// The panel's content view — the surface, the optional morph header and the
/// staggered list. Never mounted on its own; [`select`] wraps it in the host.
struct SelectPanelView<State: 'static> {
    options: Vec<SelectOption>,
    selected: Option<usize>,
    config: PanelHandle,
    on_select: OnSelect<State>,
}

/// One list row's retained state.
struct Row {
    text: LabelRun,
    disabled: bool,
}

/// The retained widget for the select panel.
pub struct SelectPanelWidget {
    rows: Vec<Row>,
    selected: Option<usize>,
    config: PanelHandle,
    /// The morph header's shaped run (painted only under `Morph`).
    header: LabelRun,
    /// When the current open/close run started, and which way it goes. A zero
    /// start means "staged by a rebuild, not yet latched by a paint".
    run: Option<(FrameTime, bool)>,
    /// The panel's own separation from the trigger, `0.0` flush .. `1.0` apart.
    separation: Lane,
    /// The morph surface's growth, `0.0` on the trigger .. `1.0` on the panel.
    morph: Lane,
    /// The panel's own measured size.
    content: Size,
    hovered: Option<usize>,
    captured: Option<usize>,
    on_select: ErasedArgCallback<usize>,
}

impl SelectPanelWidget {
    /// The variant this panel is painting.
    fn variant(&self) -> SelectVariant {
        self.config.borrow().variant
    }

    /// The header band's height, in logical px — the morph's trigger row plus
    /// its separator, or nothing for the default variant.
    fn header_height(&self) -> f64 {
        match self.variant() {
            SelectVariant::Default => 0.0,
            SelectVariant::Morph => TRIGGER_HEIGHT + style::BORDER_WIDTH,
        }
    }

    /// Row `index`'s box in the panel's own space.
    fn row_rect(&self, index: usize) -> Option<Rect> {
        if index >= self.rows.len() {
            return None;
        }
        Some(Rect::from_origin_size(
            Point::new(
                PANEL_PADDING,
                self.header_height() + PANEL_PADDING + index as f64 * ITEM_HEIGHT,
            ),
            Size::new(
                (self.content.width - PANEL_PADDING * 2.0).max(0.0),
                ITEM_HEIGHT,
            ),
        ))
    }

    /// The row under a panel-local `pos`, if it is a selectable one.
    fn hit_row(&self, pos: Point) -> Option<usize> {
        (0..self.rows.len()).find(|index| {
            !self.rows[*index].disabled && self.row_rect(*index).is_some_and(|r| r.contains(pos))
        })
    }

    /// The stagger this panel's items reveal on.
    fn stagger(&self) -> Stagger {
        Stagger::sprung(Duration::from_millis(ITEM_STAGGER_MS), SPRING_LAYOUT)
    }

    /// How long after the run's start the first item begins moving.
    fn item_delay(&self) -> Duration {
        Duration::from_millis(match self.variant() {
            SelectVariant::Default => ITEM_DELAY_MS,
            SelectVariant::Morph => MORPH_ITEM_DELAY_MS,
        })
    }

    /// How far into the current run `now` is.
    fn elapsed(&self, now: FrameTime) -> Duration {
        self.run
            .map_or(Duration::ZERO, |(started, _)| now.saturating_sub(started))
    }

    /// The surface this panel paints right now, in absolute coordinates.
    fn surface(&self, origin: Point, size: Size) -> Rect {
        match self.variant() {
            SelectVariant::Default => {
                // The separation is a translate, not a resize: the panel starts
                // flush against the trigger and slides its gap open.
                let closed = PANEL_GAP * (1.0 - self.separation.value().clamp(0.0, 1.0));
                Rect::from_origin_size(Point::new(origin.x, origin.y - closed), size)
            }
            SelectVariant::Morph => {
                let t = self.morph.value().clamp(0.0, 1.0);
                let to = Rect::from_origin_size(origin, size);
                let from = self
                    .config
                    .borrow()
                    .anchor
                    .as_ref()
                    .map_or(to, |anchor| anchor.rect());
                Rect::new(
                    from.x0 + (to.x0 - from.x0) * t,
                    from.y0 + (to.y0 - from.y0) * t,
                    from.x1 + (to.x1 - from.x1) * t,
                    from.y1 + (to.y1 - from.y1) * t,
                )
            }
        }
    }
}

impl<State: 'static> View<State> for SelectPanelView<State> {
    type Element = SelectPanelWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SelectPanelWidget {
        let open = self.config.borrow().open;
        let rest = if open { 1.0 } else { 0.0 };
        SelectPanelWidget {
            rows: rows_for(&self.options),
            selected: self.selected,
            config: self.config.clone(),
            header: LabelRun::new(self.config.borrow().header.clone()),
            run: None,
            separation: Lane::at_rest(Ramp::spring(SPRING_PANEL), rest),
            morph: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), rest),
            content: Size::ZERO,
            hovered: None,
            captured: None,
            on_select: erase_callback_arg(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SelectPanelWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = erase_callback_arg(&self.on_select);
        element.config = self.config.clone();
        let mut flags = ChangeFlags::NONE;
        if prev.options != self.options {
            element.rows = rows_for(&self.options);
            element.hovered = None;
            element.captured = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.selected != self.selected {
            element.selected = self.selected;
            flags |= ChangeFlags::PAINT;
        }
        let open = self.config.borrow().open;
        let target = if open { 1.0 } else { 0.0 };
        if element.separation.target() != target {
            element.separation.retarget(target);
            element.morph.retarget(target);
            element.run = Some((FrameTime::from_nanos(0), open));
            flags |= ChangeFlags::PAINT;
        }
        let header = self.config.borrow().header.clone();
        if element.header.set_content(header) {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

/// The retained rows for `options`.
fn rows_for(options: &[SelectOption]) -> Vec<Row> {
    options
        .iter()
        .map(|option| Row {
            text: LabelRun::new(option.label.clone()),
            disabled: option.disabled,
        })
        .collect()
}

impl Widget for SelectPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(style::TEXT_SM);
        let mut widest: f64 = 0.0;
        for row in &mut self.rows {
            widest = widest.max(row.text.layout(ctx, &style).width);
        }
        if self.variant() == SelectVariant::Morph {
            widest = widest.max(self.header.layout(ctx, &style).width);
        }
        // `min-w-[trigger width]`: a list never comes up narrower than the
        // control it dropped out of.
        let anchor_width = self
            .config
            .borrow()
            .anchor
            .as_ref()
            .map_or(0.0, |anchor| anchor.rect().width());
        let natural = widest
            + (ITEM_PADDING_X + PANEL_PADDING) * 2.0
            + TRIGGER_GAP
            + CHECK_SIZE
            + style::BORDER_WIDTH * 2.0;
        let width = natural.max(anchor_width).min(bc.max().width);
        let height =
            (self.header_height() + PANEL_PADDING * 2.0 + self.rows.len() as f64 * ITEM_HEIGHT)
                .min(SELECT_MAX_HEIGHT)
                .min(bc.max().height);
        self.content = Size::new(width, height);
        bc.constrain(self.content)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let colors = panel_colors(theme);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();
        let variant = self.variant();

        if let Some((started, opening)) = self.run
            && started.as_nanos() == 0
        {
            self.run = Some((now, opening));
        }
        let mut owes_frame = false;
        if reduce {
            self.separation.snap();
            self.morph.snap();
            self.run = None;
        } else {
            owes_frame |= self.separation.advance(now);
            owes_frame |= self.morph.advance(now);
        }

        let surface = self.surface(origin, size);
        let radius = style::resolve_radius(PANEL_RADIUS, surface.width(), surface.height());
        scene.fill_rounded_rect(surface.origin(), surface.size(), radius, colors.surface);
        stroke_frame(
            scene,
            surface.origin(),
            surface.size(),
            radius,
            colors.border,
        );

        // Everything inside is clipped to the surface, so a morph that has not
        // finished growing never spills its rows past its own edge.
        scene.push_clip_rounded(surface.origin(), surface.size(), radius);

        if variant == SelectVariant::Morph {
            let selected = self.config.borrow().header_selected;
            let text = self.header.size();
            self.header.paint(
                Point::new(
                    surface.x0 + TRIGGER_PADDING_X,
                    surface.y0 + (TRIGGER_HEIGHT - text.height) / 2.0,
                ),
                if selected { colors.ink } else { colors.muted },
                scene,
            );
            draw_chevron(
                scene,
                Point::new(
                    surface.x1 - TRIGGER_PADDING_X - CHEVRON_BOX / 2.0,
                    surface.y0 + TRIGGER_HEIGHT / 2.0,
                ),
                CHEVRON_SPIN,
                colors.muted,
                CHEVRON_BOX,
            );
            scene.fill_rect(
                Point::new(surface.x0, surface.y0 + TRIGGER_HEIGHT),
                Size::new(surface.width(), style::BORDER_WIDTH),
                colors.border,
            );
        }

        // The list rides on the resting box, not on the morphing surface: the
        // rows fade in where they will finally sit rather than being scaled with
        // the growth (the module docs' fidelity note).
        let stagger = if reduce {
            self.stagger().collapsed()
        } else {
            self.stagger()
        };
        let count = self.rows.len();
        let delay = if reduce {
            Duration::ZERO
        } else {
            self.item_delay()
        };
        let elapsed = self.elapsed(now).saturating_sub(delay);
        let entering = self.run.is_none_or(|(_, opening)| opening);
        if entering && !reduce && self.run.is_some() && !stagger.is_settled(elapsed, count) {
            owes_frame = true;
        }

        for index in 0..count {
            let Some(rect) = self.row_rect(index) else {
                continue;
            };
            let rect = rect + origin.to_vec2();
            let reveal = if entering && self.run.is_some() {
                stagger.progress_clamped(elapsed, index, count)
            } else {
                1.0
            };
            if reveal <= 0.0 {
                continue;
            }
            let rise = ITEM_RISE * (1.0 - reveal);
            let row_origin = Point::new(rect.x0, rect.y0 - rise);
            let alpha = reveal as f32;
            let layered = alpha < 1.0;
            if layered {
                scene.push_layer(row_origin, rect.size(), alpha);
            }

            let selected = self.selected == Some(index);
            let hovered = self.hovered == Some(index);
            let disabled = self.rows[index].disabled;
            if (selected || hovered) && !disabled {
                scene.fill_rounded_rect(row_origin, rect.size(), ITEM_RADIUS, colors.wash);
            }
            let ink = style::disabled_tint(
                if selected || hovered {
                    colors.ink
                } else {
                    colors.muted
                },
                disabled,
                DISABLED_OPACITY,
            );
            let text = self.rows[index].text.size();
            self.rows[index].text.paint(
                Point::new(
                    row_origin.x + ITEM_PADDING_X,
                    row_origin.y + (rect.height() - text.height) / 2.0,
                ),
                ink,
                scene,
            );
            if selected {
                draw_check(
                    scene,
                    Point::new(
                        row_origin.x + rect.width() - ITEM_PADDING_X - CHECK_SIZE,
                        row_origin.y + (rect.height() - CHECK_SIZE) / 2.0,
                    ),
                    CHECK_SIZE,
                    ink,
                );
            }

            if layered {
                scene.pop_layer();
            }
        }

        scene.pop_clip();
        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(index) = self.hit_row(p.position) else {
                    // Not a row: the host swallows the press for the panel, so
                    // the background is never this list's to consume.
                    return EventResult::Ignored;
                };
                self.captured = Some(index);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.captured.is_some() {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                    return EventResult::Handled;
                }
                let hovered = self.hit_row(p.position);
                if hovered.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if self.hovered != hovered {
                    self.hovered = hovered;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Up => {
                let Some(index) = self.captured.take() else {
                    return EventResult::Ignored;
                };
                if self.hit_row(p.position) == Some(index) {
                    (self.on_select)(ctx, index);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.captured.take().is_none() {
                    return EventResult::Ignored;
                }
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let selected = self.selected;
        ctx.push_container(
            Role::ListBox,
            |_| {},
            |ctx| {
                for (index, row) in self.rows.iter().enumerate() {
                    ctx.push_node(Role::ListBoxOption, |node| {
                        node.set_label(row.text.content());
                        node.set_selected(selected == Some(index));
                        if row.disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
                    });
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Key, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent};
    use std::any::Any;

    /// Records the geometry a pass emits.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        radii: Vec<(Point, Size, CornerRadii, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        layers: Vec<f32>,
        clips: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, radii: CornerRadii, color: Color) {
            self.radii.push((o, s, radii, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes
                .push((path.bounding_box() + origin.to_vec2(), width, color));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_clip_rounded(&mut self, _o: Point, _s: Size, _r: f64) {
            self.clips += 1;
        }
    }

    #[derive(Default)]
    struct App {
        open: bool,
        toggles: Vec<bool>,
        picked: Vec<usize>,
    }

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn options() -> Vec<SelectOption> {
        vec![
            select_option("Apple"),
            select_option("Banana"),
            select_option("Cherry").disabled(true),
        ]
    }

    fn build<S: 'static, V: View<S>>(view: &V) -> V::Element {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout<W: Widget>(w: &mut W, bc: &BoxConstraints) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, bc)
    }

    fn paint_at<W: Widget>(
        w: &mut W,
        size: Size,
        theme: Option<&Theme>,
        ms: f64,
    ) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch<W: Widget>(
        w: &mut W,
        state: &mut App,
        size: Size,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    fn trigger_view(open: bool, label: Option<&str>) -> SelectTriggerView<App> {
        let anchor = OverlayAnchor::new();
        select_trigger::<App>(&anchor, label.map(str::to_string))
            .placeholder("Pick one")
            .open(open)
            .on_open_change(|s: &mut App, next| {
                s.open = next;
                s.toggles.push(next);
            })
    }

    fn panel_view(
        open: bool,
        selected: Option<usize>,
        variant: SelectVariant,
        anchor: Option<OverlayAnchor>,
    ) -> SelectPanelView<App> {
        SelectPanelView {
            options: options(),
            selected,
            config: Rc::new(RefCell::new(PanelConfig {
                open,
                variant,
                anchor,
                header: String::from("Apple"),
                header_selected: true,
            })),
            on_select: Rc::new(|s: &mut App, i| s.picked.push(i)),
        }
    }

    // ---- Metrics ----------------------------------------------------------

    #[test]
    fn the_authored_metrics_are_the_upstream_ones() {
        assert_eq!(TRIGGER_RADIUS, 12.0, "rounded-xl");
        assert_eq!(TRIGGER_PADDING_X, 12.0, "px-3");
        assert_eq!(PANEL_PADDING, 4.0, "p-1");
        assert_eq!(ITEM_RADIUS, 8.0, "rounded-lg");
        assert_eq!(ITEM_PADDING_X, 10.0, "px-2.5");
        assert_eq!(ITEM_PADDING_Y, 6.0, "py-1.5");
        assert_eq!(PANEL_GAP, 8.0, "marginTop: 8");
        assert_eq!(ITEM_RISE, 6.0, "y: -6");
        assert_eq!(ITEM_STAGGER_MS, 35, "staggerChildren: 0.035");
        assert_eq!(ITEM_DELAY_MS, 50, "delayChildren: 0.05");
        assert_eq!(MORPH_ITEM_DELAY_MS, 80, "morph delayChildren: 0.08");
        assert_eq!(CHECK_SIZE, 14.0, "h-3.5");
    }

    /// The gooey keyframe track's two shapes, sampled at the `times` upstream
    /// authors them with.
    #[test]
    fn the_gooey_near_edge_holds_flat_on_open_and_dips_flat_on_close() {
        // Opening: flat until 40%, then rounding back to the resting radius.
        assert_eq!(near_radius(true, 0.0), 0.0);
        assert_eq!(near_radius(true, GOOEY_OPEN_HOLD), 0.0);
        assert_eq!(near_radius(true, 1.0), TRIGGER_RADIUS);
        let mid = near_radius(true, 0.7);
        assert!(mid > 0.0 && mid < TRIGGER_RADIUS, "opening mid: {mid}");

        // Closing: rounded, flat at the halfway pivot, rounded again.
        assert_eq!(near_radius(false, 0.0), TRIGGER_RADIUS);
        assert_eq!(near_radius(false, GOOEY_CLOSE_PIVOT), 0.0);
        assert_eq!(near_radius(false, 1.0), TRIGGER_RADIUS);

        // Out-of-range samples clamp rather than extrapolate.
        assert_eq!(near_radius(true, 5.0), TRIGGER_RADIUS);
        assert_eq!(near_radius(false, -1.0), TRIGGER_RADIUS);
    }

    // ---- The trigger ------------------------------------------------------

    #[test]
    fn a_closed_trigger_shows_the_placeholder_and_a_resting_chevron() {
        let mut w = build::<App, _>(&trigger_view(false, None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(220.0, 400.0)));
        assert_eq!(size, Size::new(220.0, TRIGGER_HEIGHT));
        assert_eq!(w.shown(), "Pick one");

        let (rec, _) = paint_at(&mut w, size, None, 0.0);
        let (_, _, radii, _) = rec.radii.first().copied().expect("the trigger filled");
        assert_eq!(radii.top_left, TRIGGER_RADIUS);
        assert_eq!(
            radii.bottom_left, TRIGGER_RADIUS,
            "a closed trigger rests fully rounded"
        );
        assert_eq!(w.spin.value(), 0.0);
    }

    /// The one read a panel is placed from: the trigger writes its own window
    /// rect into the shared anchor on every paint.
    #[test]
    fn the_trigger_captures_its_window_rect_into_the_anchor() {
        let anchor = OverlayAnchor::new();
        let view = select_trigger::<App>(&anchor, Some("Apple".into()));
        let mut w = build::<App, _>(&view);
        assert_eq!(anchor.rect(), Rect::ZERO, "nothing captured before a paint");
        let size = Size::new(200.0, TRIGGER_HEIGHT);
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(anchor.rect(), Rect::from_origin_size(Point::ZERO, size));
    }

    #[test]
    fn a_release_inside_the_trigger_toggles_the_flag_and_a_release_outside_does_not() {
        let mut w = build::<App, _>(&trigger_view(false, None));
        let size = Size::new(200.0, TRIGGER_HEIGHT);
        let mut app = App::default();

        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, 20.0, 20.0),
        );
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Up, 20.0, 20.0),
        );
        assert_eq!(app.toggles, vec![true], "one open request");

        // A release that wandered off the control reports nothing.
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, 20.0, 20.0),
        );
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Up, 900.0, 20.0),
        );
        assert_eq!(app.toggles, vec![true]);

        // Neither does a cancelled gesture.
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, 20.0, 20.0),
        );
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Cancel, 20.0, 20.0),
        );
        assert_eq!(app.toggles, vec![true]);
    }

    #[test]
    fn a_disabled_trigger_reports_nothing_from_pointer_or_keyboard() {
        let anchor = OverlayAnchor::new();
        let view = select_trigger::<App>(&anchor, None)
            .disabled(true)
            .on_open_change(|s: &mut App, next| s.toggles.push(next));
        let mut w = build::<App, _>(&view);
        let size = Size::new(200.0, TRIGGER_HEIGHT);
        let mut app = App::default();
        assert_eq!(
            dispatch(
                &mut w,
                &mut app,
                size,
                &pointer(PointerPhase::Down, 20.0, 20.0)
            ),
            EventResult::Ignored
        );
        let key = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        assert_eq!(dispatch(&mut w, &mut app, size, &key), EventResult::Ignored);
        assert!(app.toggles.is_empty());
    }

    #[test]
    fn enter_and_space_toggle_the_trigger() {
        let mut w = build::<App, _>(&trigger_view(true, Some("Apple")));
        let size = Size::new(200.0, TRIGGER_HEIGHT);
        let mut app = App::default();
        for key in [Key::Named(NamedKey::Enter), Key::Character(" ".into())] {
            let event = InputEvent::Key(KeyEvent {
                key,
                modifiers: Modifiers::default(),
                repeat: false,
            });
            assert_eq!(
                dispatch(&mut w, &mut app, size, &event),
                EventResult::Handled
            );
        }
        assert_eq!(
            app.toggles,
            vec![false, false],
            "an open trigger asks to close"
        );
    }

    /// The morph variant's trigger is the panel's own surface while open, so it
    /// fades out rather than sitting under it.
    #[test]
    fn a_morph_trigger_fades_out_while_open_and_the_default_one_does_not() {
        let anchor = OverlayAnchor::new();
        let mut counter = 0u64;

        let closed = select_trigger::<App>(&anchor, None).variant(SelectVariant::Morph);
        let opened = select_trigger::<App>(&anchor, None)
            .variant(SelectVariant::Morph)
            .open(true);
        let mut w = build::<App, _>(&closed);
        View::<App>::rebuild(&opened, &closed, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.fade.target(), 0.0);

        let plain = select_trigger::<App>(&anchor, None);
        let plain_open = select_trigger::<App>(&anchor, None).open(true);
        let mut d = build::<App, _>(&plain);
        View::<App>::rebuild(
            &plain_open,
            &plain,
            &mut d,
            &mut BuildCtx::new(&mut counter),
        );
        assert_eq!(d.fade.target(), 1.0, "the separating trigger stays visible");
    }

    #[test]
    fn reduce_motion_lands_the_trigger_on_its_resting_chrome_with_no_frames_owed() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut w = build::<App, _>(&trigger_view(true, Some("Apple")));
        w.spin.retarget(1.0);
        w.gooey = Some((ft_ms(0.0), true));
        let size = Size::new(200.0, TRIGGER_HEIGHT);
        let (rec, owes) = paint_at(&mut w, size, Some(&theme), 0.0);
        assert!(!owes, "a collapsed trigger owes no frame");
        assert_eq!(w.spin.value(), 1.0);
        let (_, _, radii, _) = rec.radii.first().copied().expect("the trigger filled");
        assert_eq!(radii.bottom_left, TRIGGER_RADIUS, "no gooey flattening");
    }

    // ---- The panel --------------------------------------------------------

    #[test]
    fn the_panel_sizes_itself_to_its_rows_and_caps_its_height() {
        let mut w = build::<App, _>(&panel_view(true, Some(0), SelectVariant::Default, None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(600.0, 600.0)));
        assert_eq!(size.height, PANEL_PADDING * 2.0 + 3.0 * ITEM_HEIGHT);
        assert!(size.width > 0.0);

        // A list longer than the cap stops at it rather than growing.
        let many: Vec<SelectOption> = (0..40).map(|i| select_option(format!("row {i}"))).collect();
        let view: SelectPanelView<App> = SelectPanelView {
            options: many,
            selected: None,
            config: Rc::new(RefCell::new(PanelConfig {
                open: true,
                ..PanelConfig::default()
            })),
            on_select: Rc::new(|_: &mut App, _| {}),
        };
        let mut tall = build::<App, _>(&view);
        let size = layout(&mut tall, &BoxConstraints::loose(Size::new(600.0, 900.0)));
        assert_eq!(size.height, SELECT_MAX_HEIGHT);
    }

    /// The trigger's width is a floor, never a ceiling —
    /// `min-w-[var(--trigger-width)]`.
    #[test]
    fn the_panel_is_never_narrower_than_its_trigger() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::from_origin_size(Point::ZERO, Size::new(420.0, 40.0)));
        let mut w = build::<App, _>(&panel_view(
            true,
            None,
            SelectVariant::Default,
            Some(anchor),
        ));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(600.0, 600.0)));
        assert_eq!(size.width, 420.0);
    }

    #[test]
    fn a_row_press_reports_its_index_and_a_disabled_row_reports_nothing() {
        let mut w = build::<App, _>(&panel_view(true, None, SelectVariant::Default, None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 600.0)));
        let mut app = App::default();

        let row = w.row_rect(1).expect("row 1").center();
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, row.x, row.y),
        );
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Up, row.x, row.y),
        );
        assert_eq!(app.picked, vec![1]);

        // Row 2 is `disabled` — it is not even a hit.
        let row = w.row_rect(2).expect("row 2").center();
        assert_eq!(
            dispatch(
                &mut w,
                &mut app,
                size,
                &pointer(PointerPhase::Down, row.x, row.y)
            ),
            EventResult::Ignored
        );
        assert_eq!(app.picked, vec![1]);
    }

    /// A press that starts on a row and releases on a different one commits
    /// nothing — the up-inside rule, applied per row.
    #[test]
    fn a_release_on_another_row_commits_nothing() {
        let mut w = build::<App, _>(&panel_view(true, None, SelectVariant::Default, None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 600.0)));
        let mut app = App::default();
        let from = w.row_rect(0).expect("row 0").center();
        let to = w.row_rect(1).expect("row 1").center();
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, from.x, from.y),
        );
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Up, to.x, to.y),
        );
        assert!(app.picked.is_empty());
    }

    /// A press on the panel's own background is *not* the list's: the host owns
    /// it, which is what keeps the seam the single dismissal authority.
    #[test]
    fn a_press_on_the_panel_background_is_left_to_the_host() {
        let mut w = build::<App, _>(&panel_view(true, None, SelectVariant::Default, None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 600.0)));
        let mut app = App::default();
        assert_eq!(
            dispatch(
                &mut w,
                &mut app,
                size,
                &pointer(PointerPhase::Down, 1.0, 1.0)
            ),
            EventResult::Ignored
        );
    }

    #[test]
    fn the_items_reveal_on_a_stagger_and_the_panel_stops_asking_for_frames() {
        let mut w = build::<App, _>(&panel_view(true, Some(0), SelectVariant::Default, None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 600.0)));
        w.run = Some((ft_ms(0.0), true));

        // Inside the `delayChildren` window nothing has moved yet.
        let (early, owes) = paint_at(&mut w, size, None, 10.0);
        assert!(owes, "an unsettled stagger owes a frame");
        assert!(
            early.layers.is_empty(),
            "no row has begun, so none is painted at a partial alpha"
        );

        // Partway through, the first rows are up and the last still is not.
        let (mid, owes) = paint_at(&mut w, size, None, 120.0);
        assert!(owes);
        assert!(
            !mid.layers.is_empty(),
            "rows are mid-reveal: {:?}",
            mid.layers
        );

        // Long past the whole run every row is at full opacity and the panel
        // stops driving frames.
        let (late, owes) = paint_at(&mut w, size, None, 5_000.0);
        assert!(!owes, "a settled panel owes no frame");
        assert!(late.layers.is_empty(), "every row landed at full alpha");
    }

    #[test]
    fn the_selected_row_is_washed_and_checked() {
        let mut w = build::<App, _>(&panel_view(true, Some(1), SelectVariant::Default, None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 600.0)));
        let (rec, _) = paint_at(&mut w, size, None, 5_000.0);
        let colors = panel_colors(None);
        let washes = rec
            .rrects
            .iter()
            .filter(|(_, _, radius, color)| *radius == ITEM_RADIUS && *color == colors.wash)
            .count();
        assert_eq!(washes, 1, "exactly the selected row is washed");
        let check = check_stroke_width(CHECK_SIZE);
        assert!(
            rec.strokes
                .iter()
                .any(|(_, width, _)| (*width - check).abs() < 1e-9),
            "the selected row drew its check"
        );
    }

    #[test]
    fn a_morph_panel_grows_out_of_its_anchor_and_carries_a_header() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(0.0, -TRIGGER_HEIGHT, 200.0, 0.0));
        let mut w = build::<App, _>(&panel_view(
            true,
            Some(0),
            SelectVariant::Morph,
            Some(anchor.clone()),
        ));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 600.0)));
        assert!(
            size.height > PANEL_PADDING * 2.0 + 3.0 * ITEM_HEIGHT,
            "the morph panel reserves its header row"
        );

        // Mid-morph the surface still sits on the trigger's rect...
        w.morph = Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 0.0);
        let (start, _) = paint_at(&mut w, size, None, 0.0);
        let (origin, painted, _, _) = start.rrects[0];
        assert_eq!(origin, anchor.rect().origin());
        assert_eq!(painted, anchor.rect().size());
        assert_eq!(start.clips, 1, "the growing surface clips its content");

        // ...and once the morph lands it is the panel's own box.
        w.morph = Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 1.0);
        let (done, _) = paint_at(&mut w, size, None, 0.0);
        let (origin, painted, _, _) = done.rrects[0];
        assert_eq!(origin, Point::ZERO);
        assert_eq!(painted, size);
    }

    #[test]
    fn reduce_motion_lands_the_panel_with_every_row_present_and_no_frames_owed() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut w = build::<App, _>(&panel_view(true, Some(0), SelectVariant::Default, None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 600.0)));
        w.run = Some((ft_ms(0.0), true));
        let (rec, owes) = paint_at(&mut w, size, Some(&theme), 0.0);
        assert!(!owes, "a collapsed panel owes no frame");
        assert!(
            rec.layers.is_empty(),
            "every row is fully present, so none needs a fade layer"
        );
    }

    // ---- The mounted pair -------------------------------------------------

    /// The builder chain every app writes, exercised end to end: both variants
    /// construct, and the panel view really is the anchored host.
    #[test]
    fn both_variants_construct_through_the_public_builders() {
        let anchor = OverlayAnchor::new();
        for variant in [SelectVariant::Default, SelectVariant::Morph] {
            let view = select::<App, _>(options(), Some(0), |s: &mut App, i| s.picked.push(i))
                .anchor(&anchor)
                .variant(variant)
                .header(Some("Apple".into()), "Pick one")
                .side(OverlaySide::Bottom)
                .align(OverlayAlign::Start)
                .open(true)
                .on_open_change(|s: &mut App, open| s.open = open);
            assert_eq!(view.variant, variant);
            assert_eq!(
                view.placement.offset,
                match variant {
                    SelectVariant::Default => PANEL_GAP,
                    SelectVariant::Morph => MORPH_OFFSET,
                }
            );
            let mut host = build::<App, _>(&view);
            let size = layout(&mut host, &BoxConstraints::tight(Size::new(400.0, 400.0)));
            assert_eq!(size, Size::new(400.0, 400.0), "the host fills its area");
            assert!(host.is_open());
        }
    }

    /// Dismissal is the seam's, and it fires exactly once per outside press.
    #[test]
    fn an_outside_press_dismisses_once_through_the_overlay_seam() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(20.0, 20.0, 220.0, 60.0));
        let view = select::<App, _>(options(), None, |_: &mut App, _| {})
            .anchor(&anchor)
            .open(true)
            .on_open_change(|s: &mut App, open| s.toggles.push(open));
        let mut host = build::<App, _>(&view);
        let area = Size::new(500.0, 500.0);
        layout(&mut host, &BoxConstraints::tight(area));
        let mut app = App::default();

        // Far from the panel: one dismissal, and only one.
        dispatch(
            &mut host,
            &mut app,
            area,
            &pointer(PointerPhase::Down, 480.0, 480.0),
        );
        assert_eq!(app.toggles, vec![false]);

        // Inside the panel: nothing more.
        let inside = host.content_rect().center();
        dispatch(
            &mut host,
            &mut app,
            area,
            &pointer(PointerPhase::Down, inside.x, inside.y),
        );
        assert_eq!(
            app.toggles,
            vec![false],
            "a press on the panel is not a dismissal"
        );
    }
}
