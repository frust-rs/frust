//! Ports beUI's `project-folder` block — `components/motion/project-folder.tsx`
//! (beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01),
//! registry slug `project-folder`: *"An interactive project folder that opens
//! its file fan on hover or focus, expands into a focus-managed overlay, then
//! retraces the complete path when closed."*
//!
//! | upstream | here |
//! |---|---|
//! | `h-56 w-72` folder button | [`FOLDER_HEIGHT`] by [`FOLDER_WIDTH`] |
//! | `h-40 w-24` preview sheet, `-ml-12` off the centre line | [`SHEET_HEIGHT`], [`SHEET_WIDTH`] |
//! | `MAX_PREVIEWS = 5` | [`FOLDER_MAX_PREVIEWS`] |
//! | `getPreviewTransform(index, count)` | [`folder_fan`], returning a [`FolderSheet`] |
//! | its hover branch (`x * 1.4`, `y - 8`, `rotate * 1.3`, `scale * 1.02`, `opacity + 0.18`) | [`FolderSheet::opened`] |
//! | `SPRING_LAYOUT` on every fan property | [`FOLDER_FAN`] |
//! | `whileTap={{ scale: 0.98 }}` on `SPRING_PRESS` | [`FOLDER_PRESS_SCALE`] |
//! | back panel `rotateX: 15`, front flap `rotateX: -25` | [`FOLDER_BACK_TILT`] / [`FOLDER_FLAP_TILT`], as foreshortening |
//! | flap `h-16` title row over an `h-12` meta row | [`FLAP_TITLE_HEIGHT`], [`FLAP_META_HEIGHT`] |
//! | `count` plus `itemLabel` pluralisation | [`folder_count_text`] |
//!
//! # The fan is the port; the overlay is not
//!
//! Upstream is two components sharing one `LayoutGroup`: a folder card whose
//! sheets fan on hover, and a portalled, focus-trapped modal the same sheets
//! *morph into* on click, via `layoutId`. This port carries the first and
//! declines the second, deliberately:
//!
//! - A cross-tree shared-element morph needs a `layoutId` registry the framework
//!   does not publish, and the modal half is a focus-trapped dialog —
//!   [`mod@crate::overlay::modal`] already is that host, so re-implementing it
//!   inside a card would be a second copy of it.
//! - The card still **reports** the intent: [`ProjectFolderView::on_open_change`]
//!   fires for the hover/focus open, and [`ProjectFolderView::on_activate`] for
//!   the press upstream expands on. An app that wants the overlay hosts one from
//!   that callback, with the previews it already has.
//!
//! # Two ports of a 3D transform
//!
//! `rotateX` has no `PaintScene` primitive — the scene vocabulary is 2D affine —
//! so the back panel's `15deg` and the flap's `-25deg` tilts are carried as the
//! **vertical foreshortening** each rotation produces ([`tilt_scale`], `cos θ`),
//! anchored at the same `transform-origin: center bottom` upstream sets. A
//! tilted panel therefore shortens toward its own base exactly as the
//! perspective projection would; what is lost is the sheared parallax of the
//! true projection, a few pixels of skew at these angles on a 224px card.
//!
//! # Degradations against the web original
//!
//! - **No expand overlay, no focus trap, no portal** — the section above.
//! - **No backdrop blur.** `backdrop-blur-xl` has no primitive; the surfaces are
//!   the same alpha-tinted fills at the alphas upstream sets.
//! - **Sheets carry a label, not arbitrary content.** `ProjectFolderPreview
//!   .content` is an arbitrary node; a sheet here is a tinted card with a
//!   caption, for the reason [`crate::blocks::infinite_masonry`]'s module docs
//!   give about `renderItem`.
//! - **No hover-capability branch.** Upstream gates its fan on
//!   `useHoverCapable()`; here a touch device simply never delivers a hover
//!   move, so the fan opens on the press instead with no branch to maintain.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::{
    Action, Affine, BoxConstraints, BuildCtx, ChangeFlags, Color, ErasedArgCallback,
    ErasedCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerEvent, PointerPhase, Role, SemanticsCtx, Size, Vec2, View, Widget, erase_callback,
    erase_callback_arg, text::TextStyle,
};

use crate::components::popover::{paint_panel_hairline, resolve_panel};
use crate::motion::Ramp;
use crate::press::{Lane, inside, is_activation_key, press_scale, presses};
use crate::style;
use crate::text::LabelRun;
use crate::tokens::motion::{SPRING_LAYOUT, SPRING_PRESS};

// ---- Metrics ---------------------------------------------------------------

/// `w-72` — the folder card's width, in logical px.
pub const FOLDER_WIDTH: f64 = 288.0;

/// `h-56` — the folder card's height, in logical px.
pub const FOLDER_HEIGHT: f64 = 224.0;

/// `rounded-2xl` — the card's and the flap's corner radius.
pub const FOLDER_RADIUS: f64 = style::RADIUS_2XL;

/// `rounded-lg` — a preview sheet's corner radius.
pub const SHEET_RADIUS: f64 = style::RADIUS_LG;

/// `w-24` — one preview sheet's width, in logical px.
pub const SHEET_WIDTH: f64 = 96.0;

/// `h-40` — one preview sheet's height, in logical px.
pub const SHEET_HEIGHT: f64 = 160.0;

/// `MAX_PREVIEWS = 5` — how many sheets the fan ever shows.
pub const FOLDER_MAX_PREVIEWS: usize = 5;

/// `h-16` — the flap's title row, in logical px.
pub const FLAP_TITLE_HEIGHT: f64 = 64.0;

/// `h-12` — the flap's meta row, in logical px.
pub const FLAP_META_HEIGHT: f64 = 48.0;

/// The flap's own height: its two stacked rows.
pub const FLAP_HEIGHT: f64 = FLAP_TITLE_HEIGHT + FLAP_META_HEIGHT;

/// `px-4` — the flap's horizontal padding, in logical px.
pub const FLAP_PADDING_X: f64 = 16.0;

/// `text-xl` — the flap title's size, in logical px.
pub const FLAP_TITLE_SIZE: f64 = 20.0;

/// `bg-background/25` — the back panel's fill alpha.
pub const FOLDER_BACK_ALPHA: f32 = 0.25;

/// `bg-background/45` — a preview sheet's fill alpha.
pub const SHEET_FILL_ALPHA: f32 = 0.45;

/// `bg-background/60` — the front flap's fill alpha.
pub const FLAP_FILL_ALPHA: f32 = 0.60;

/// `border-foreground/10` — the alpha every hairline on this card is drawn at.
pub const FOLDER_HAIRLINE_ALPHA: f32 = 0.10;

/// `disabled:opacity-50` — a disabled folder's ink opacity.
pub const FOLDER_DISABLED_OPACITY: f32 = 0.5;

// ---- Motion ----------------------------------------------------------------

/// `rotateX: 15` on the back panel, in degrees — carried as the vertical
/// foreshortening it produces (see the [module docs](self)).
pub const FOLDER_BACK_TILT: f64 = 15.0;

/// `rotateX: -25` on the front flap, same treatment.
pub const FOLDER_FLAP_TILT: f64 = -25.0;

/// `whileTap={{ scale: 0.98 }}` — the card's press shrink.
pub const FOLDER_PRESS_SCALE: f64 = 0.98;

/// `SPRING_LAYOUT` — the spring every fan property travels on.
pub const FOLDER_FAN: Ramp = Ramp::spring(SPRING_LAYOUT);

// ---- The fan arithmetic ----------------------------------------------------

/// `getPreviewTransform`'s per-sheet `x` step, in logical px.
pub const SHEET_FAN_STEP: f64 = 44.0;

/// Its resting `y`, in logical px.
pub const SHEET_FAN_BASE_Y: f64 = 8.0;

/// Its centre-lift coefficient: `Math.max(0, 2 - distance) * 8`.
pub const SHEET_CENTRE_LIFT: f64 = 8.0;

/// Its per-sheet rotation step, in degrees.
pub const SHEET_FAN_ROTATION: f64 = 6.0;

/// The multiplier the hover branch applies to a sheet's `x`.
pub const SHEET_OPEN_X_FACTOR: f64 = 1.4;

/// ...the extra lift it applies to its `y`, in logical px.
pub const SHEET_OPEN_LIFT: f64 = 8.0;

/// ...the multiplier on its rotation.
pub const SHEET_OPEN_ROTATION_FACTOR: f64 = 1.3;

/// ...the multiplier on its scale.
pub const SHEET_OPEN_SCALE_FACTOR: f64 = 1.02;

/// ...and the bonus on its opacity, capped at `1.0`.
pub const SHEET_OPEN_OPACITY_BONUS: f64 = 0.18;

/// One preview sheet's placement in the fan: where it sits relative to the
/// folder's own centre line, how far it is turned, how big it is drawn, and how
/// opaque.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FolderSheet {
    /// Horizontal displacement from the centre line, in logical px.
    pub x: f64,
    /// Vertical displacement from the folder's top edge, in logical px.
    pub y: f64,
    /// Rotation in degrees — positive turns clockwise, as CSS `rotate` does.
    pub rotation: f64,
    /// Uniform scale about the sheet's own centre.
    pub scale: f64,
    /// Opacity, in `0..=1`.
    pub opacity: f64,
    /// Paint order: a higher value sits nearer the viewer. Upstream's
    /// `zIndex: 10 - distance`.
    pub depth: i32,
}

impl FolderSheet {
    /// This sheet as the fan shows it while the folder is **open** — upstream's
    /// hover branch, which is the same placement pushed further out.
    pub fn opened(self) -> Self {
        FolderSheet {
            x: self.x * SHEET_OPEN_X_FACTOR,
            y: self.y - SHEET_OPEN_LIFT,
            rotation: self.rotation * SHEET_OPEN_ROTATION_FACTOR,
            scale: self.scale * SHEET_OPEN_SCALE_FACTOR,
            opacity: (self.opacity + SHEET_OPEN_OPACITY_BONUS).min(1.0),
            depth: self.depth,
        }
    }

    /// This sheet `t` of the way from `self` to `to`, every property lerped
    /// together — the read the widget paints from, since upstream animates the
    /// whole transform under one `SPRING_LAYOUT` rather than per property.
    pub fn lerp(self, to: FolderSheet, t: f64) -> Self {
        let mix = |a: f64, b: f64| a + (b - a) * t;
        FolderSheet {
            x: mix(self.x, to.x),
            y: mix(self.y, to.y),
            rotation: mix(self.rotation, to.rotation),
            scale: mix(self.scale, to.scale),
            opacity: mix(self.opacity, to.opacity).clamp(0.0, 1.0),
            depth: to.depth,
        }
    }
}

/// `getPreviewTransform(index, count)` — where sheet `index` of `count` rests
/// while the folder is closed.
///
/// The fan is symmetric about the middle sheet: `offset = index - (count - 1) /
/// 2`, so an odd count has one sheet dead centre and an even count straddles.
/// `distance` (the absolute offset) is what the lift, the scale and the opacity
/// step down by — the neighbours at distance 1 sit a little back, everything at
/// distance 2 or more sits further back still.
///
/// An index outside `0..count`, or a zero `count`, has no place in the fan and
/// reports the centre sheet's own resting placement.
pub fn folder_fan(index: usize, count: usize) -> FolderSheet {
    if count == 0 || index >= count {
        return FolderSheet {
            x: 0.0,
            y: SHEET_FAN_BASE_Y - 2.0 * SHEET_CENTRE_LIFT,
            rotation: 0.0,
            scale: 1.04,
            opacity: 1.0,
            depth: 10,
        };
    }
    let offset = index as f64 - (count as f64 - 1.0) / 2.0;
    let distance = offset.abs();
    let centre_lift = (2.0 - distance).max(0.0) * SHEET_CENTRE_LIFT;
    FolderSheet {
        x: offset * SHEET_FAN_STEP,
        y: SHEET_FAN_BASE_Y - centre_lift,
        rotation: offset * SHEET_FAN_ROTATION,
        scale: if distance == 0.0 {
            1.04
        } else if distance == 1.0 {
            0.95
        } else {
            0.88
        },
        opacity: if distance == 0.0 {
            1.0
        } else if distance == 1.0 {
            0.78
        } else {
            0.58
        },
        depth: 10 - distance.round() as i32,
    }
}

/// The whole fan in paint order — furthest-back sheet first, so a later fill
/// covers an earlier one exactly the way upstream's `zIndex` stack does.
///
/// Ties break by index, which keeps the two sheets either side of the centre in
/// left-to-right order rather than an arbitrary one.
pub fn folder_fan_order(count: usize) -> Vec<usize> {
    let shown = count.min(FOLDER_MAX_PREVIEWS);
    let mut order: Vec<usize> = (0..shown).collect();
    order.sort_by(|&a, &b| {
        folder_fan(a, shown)
            .depth
            .cmp(&folder_fan(b, shown).depth)
            .then_with(|| a.cmp(&b))
    });
    order
}

/// `${count} ${itemLabel}${count === 1 ? "" : "s"}` — the meta row's count line.
///
/// The naive English plural is upstream's own; a caller with a word it does not
/// fit passes the already-plural label with a count of one, or supplies the
/// whole line through [`ProjectFolderView::description`].
pub fn folder_count_text(count: usize, item_label: &str) -> String {
    if count == 1 {
        format!("{count} {item_label}")
    } else {
        format!("{count} {item_label}s")
    }
}

/// The vertical foreshortening a `degrees` rotation about the horizontal axis
/// produces — `cos θ`, floored at zero so a tilt past the horizon flattens
/// rather than turning the box inside out.
pub fn tilt_scale(degrees: f64) -> f64 {
    degrees.to_radians().cos().max(0.0)
}

// ---- The component ---------------------------------------------------------

/// A view-held open callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A view-held activation callback (erased on build).
type OnActivate<State> = Rc<dyn Fn(&mut State)>;

/// One preview sheet's declared content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectFolderPreview {
    label: String,
}

/// A preview sheet showing `label`.
pub fn folder_preview(label: impl Into<String>) -> ProjectFolderPreview {
    ProjectFolderPreview {
        label: label.into(),
    }
}

impl ProjectFolderPreview {
    /// The sheet's label.
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// What the card renders from, beyond its previews.
#[derive(Clone, Debug, PartialEq)]
struct FolderConfig {
    title: String,
    description: String,
    item_label: String,
    count: Option<usize>,
    open: Option<bool>,
    disabled: bool,
    label: Option<String>,
}

/// A declarative beUI project folder. See [`project_folder`].
pub struct ProjectFolderView<State: 'static> {
    previews: Vec<ProjectFolderPreview>,
    config: FolderConfig,
    on_open_change: OnOpenChange<State>,
    on_activate: OnActivate<State>,
}

/// Build a project folder card titled `title`, fanning `previews` on hover.
pub fn project_folder<State: 'static>(
    title: impl Into<String>,
    previews: Vec<ProjectFolderPreview>,
) -> ProjectFolderView<State> {
    ProjectFolderView {
        previews,
        config: FolderConfig {
            title: title.into(),
            description: "Updated recently".to_string(),
            item_label: "file".to_string(),
            count: None,
            open: None,
            disabled: false,
            label: None,
        },
        on_open_change: Rc::new(|_, _| {}),
        on_activate: Rc::new(|_| {}),
    }
}

impl<State: 'static> ProjectFolderView<State> {
    /// Set the meta row's right-hand line (`description`).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.config.description = description.into();
        self
    }

    /// Set the noun the count line is built from (`itemLabel`).
    pub fn item_label(mut self, item_label: impl Into<String>) -> Self {
        self.config.item_label = item_label.into();
        self
    }

    /// Set the count the meta row reports, when it is not the preview count
    /// (`count`).
    pub fn count(mut self, count: usize) -> Self {
        self.config.count = Some(count);
        self
    }

    /// Take the fan over: it is open exactly when `open` says, and a hover only
    /// reports (upstream's controlled `open` prop).
    pub fn open(mut self, open: bool) -> Self {
        self.config.open = Some(open);
        self
    }

    /// Make the card unactivatable (`disabled`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.config.disabled = disabled;
        self
    }

    /// Override the card's accessible name (`ariaLabel`), which is otherwise
    /// its title.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.config.label = Some(label.into());
        self
    }

    /// Set the fan's open/close callback.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Rc::new(on_open_change);
        self
    }

    /// Set the press callback — the moment upstream expands into its overlay,
    /// which this port reports rather than hosts (see the [module docs](self)).
    pub fn on_activate<F: Fn(&mut State) + 'static>(mut self, on_activate: F) -> Self {
        self.on_activate = Rc::new(on_activate);
        self
    }
}

/// The retained widget for a [`ProjectFolderView`].
pub struct ProjectFolderWidget {
    sheets: Vec<LabelRun>,
    config: FolderConfig,
    title: LabelRun,
    description: LabelRun,
    count: LabelRun,
    /// The fan's own lane: `0.0` closed, `1.0` fanned.
    fan: Lane,
    /// The card's press shrink.
    press: Lane,
    /// Whether the fan is open, as this widget last resolved it.
    open: bool,
    /// Whether a pointer is over the card (upstream's `hoveredRef`).
    hovered: bool,
    /// Whether the card holds focus (upstream's `focusedRef`).
    focused: bool,
    /// The last open intent the app was told about — one report per change.
    reported: bool,
    /// Whether a press is armed on the card.
    armed: bool,
    on_open_change: ErasedArgCallback<bool>,
    on_activate: ErasedCallback,
}

impl ProjectFolderWidget {
    /// Whether the fan is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// How far through the fan the card is: `0.0` closed, `1.0` fanned.
    pub fn fan_progress(&self) -> f64 {
        self.fan.value().clamp(0.0, 1.0)
    }

    /// How many sheets the fan draws — the declared previews, capped at
    /// [`FOLDER_MAX_PREVIEWS`].
    pub fn sheet_count(&self) -> usize {
        self.sheets.len().min(FOLDER_MAX_PREVIEWS)
    }

    /// Sheet `index` as it is placed right now, between its resting and its
    /// fanned placement.
    pub fn sheet(&self, index: usize) -> FolderSheet {
        let count = self.sheet_count();
        let rest = folder_fan(index, count);
        rest.lerp(rest.opened(), self.fan_progress())
    }

    /// Apply an open state, retargeting the fan. Reports whether it changed.
    fn set_open(&mut self, open: bool) -> bool {
        if self.open == open {
            return false;
        }
        self.open = open;
        self.fan.retarget(if open { 1.0 } else { 0.0 });
        true
    }

    /// Upstream's own rule for what the card *wants* to be: open while it is
    /// hovered or focused, and never while it is disabled.
    fn intent(&self) -> bool {
        !self.config.disabled && (self.hovered || self.focused)
    }

    /// What the card actually shows: the app's flag when it took one over, and
    /// its own [`intent`](Self::intent) otherwise.
    fn derived_open(&self) -> bool {
        self.config.open.unwrap_or(self.intent())
    }

    /// Re-derive the open state after a hover or focus change and report it.
    ///
    /// The report is gated on the *intent*, not on what is shown: a controlled
    /// card still says what it would have done — that is what lets an app drive
    /// the same flag from the same gesture — and the latch is what keeps one
    /// observable change to one call.
    fn sync_open(&mut self, ctx: &mut EventCtx) {
        let intent = self.intent();
        if self.config.open.is_none() && self.set_open(intent) {
            ctx.request_redraw();
        }
        if self.reported != intent {
            self.reported = intent;
            (self.on_open_change)(ctx, intent);
        }
    }

    /// The count the meta row reports.
    fn reported_count(&self) -> usize {
        self.config.count.unwrap_or(self.sheets.len())
    }
}

impl<State: 'static> View<State> for ProjectFolderView<State> {
    type Element = ProjectFolderWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProjectFolderWidget {
        let open = self.config.open.unwrap_or(false);
        let count = self.config.count.unwrap_or(self.previews.len());
        ProjectFolderWidget {
            sheets: sheet_runs(&self.previews),
            title: LabelRun::new(self.config.title.clone()),
            description: LabelRun::new(self.config.description.clone()),
            count: LabelRun::new(folder_count_text(count, &self.config.item_label)),
            config: self.config.clone(),
            fan: Lane::at_rest(FOLDER_FAN, if open { 1.0 } else { 0.0 }),
            press: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            open,
            hovered: false,
            focused: false,
            reported: open,
            armed: false,
            on_open_change: erase_callback_arg(&self.on_open_change),
            on_activate: erase_callback(&self.on_activate),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ProjectFolderWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.previews != self.previews {
            element.sheets = sheet_runs(&self.previews);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.config != self.config {
            element.config = self.config.clone();
            if element.config.disabled {
                element.armed = false;
            }
            element.title.set_content(self.config.title.clone());
            element
                .description
                .set_content(self.config.description.clone());
            let count = element.reported_count();
            element
                .count
                .set_content(folder_count_text(count, &self.config.item_label));
            let open = element.derived_open();
            element.set_open(open);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Closures are not comparable; reinstalling the adapters is cheap.
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        element.on_activate = erase_callback(&self.on_activate);
        flags
    }

    fn teardown(&self, _element: &mut ProjectFolderWidget, _ctx: &mut BuildCtx<'_>) {}
}

/// The shaped runs for the sheets a declared preview list yields, capped.
fn sheet_runs(previews: &[ProjectFolderPreview]) -> Vec<LabelRun> {
    previews
        .iter()
        .take(FOLDER_MAX_PREVIEWS)
        .map(|preview| LabelRun::new(preview.label.clone()))
        .collect()
}

/// The label family: the theme's own scale, with the catalog's sans stack as
/// the unthemed fallback.
fn family_of(theme: Option<&Theme>) -> frust::authoring::text::FontFamily {
    theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    })
}

/// One label style at `size`.
fn folder_style(theme: Option<&Theme>, size: f64) -> TextStyle {
    TextStyle {
        family: family_of(theme),
        ..crate::text::label_style(size)
    }
}

impl Widget for ProjectFolderWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        // Every style is resolved before the first `layout` call: the shaper
        // takes `ctx` mutably, and the theme read borrows it.
        let title_style = folder_style(theme, FLAP_TITLE_SIZE);
        let meta_style = folder_style(theme, style::TEXT_SM);
        let sheet_style = folder_style(theme, style::TEXT_XS);
        self.title.layout(ctx, &title_style);
        self.count.layout(ctx, &meta_style);
        self.description.layout(ctx, &meta_style);
        for sheet in &mut self.sheets {
            sheet.layout(ctx, &sheet_style);
        }
        bc.constrain(Size::new(FOLDER_WIDTH, FOLDER_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let background = theme.map_or(crate::BEUI_LIGHT.background, |t| t.scheme().surface);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        // The framework's hover answer is authoritative; a stale latch would
        // leave a fan open under a pointer that has left.
        if self.hovered != ctx.is_hovered() {
            self.hovered = ctx.is_hovered();
            let open = self.derived_open();
            self.set_open(open);
        }

        if reduce {
            self.fan.snap();
            self.press.snap();
        } else if self.fan.advance(now) | self.press.advance(now) {
            ctx.request_frame();
        }

        let fanned = self.fan_progress();
        let ink = style::disabled_tint(chrome.ink, self.config.disabled, FOLDER_DISABLED_OPACITY);
        let dim = style::disabled_tint(
            chrome.dim_ink,
            self.config.disabled,
            FOLDER_DISABLED_OPACITY,
        );
        let hairline = style::with_alpha(chrome.ink, FOLDER_HAIRLINE_ALPHA);

        // `whileTap={{ scale: 0.98 }}`, about the card's own centre.
        let scale = press_scale(FOLDER_PRESS_SCALE, self.press.value());
        let scaled = scale != 1.0;
        if scaled {
            scene.push_transform(Affine::scale_about(
                scale,
                origin + Vec2::new(size.width / 2.0, size.height / 2.0),
            ));
        }

        // 1. The back panel, tilting away from the viewer as the folder opens.
        paint_tilted_panel(
            scene,
            origin,
            size,
            FOLDER_BACK_TILT * fanned,
            style::with_alpha(background, FOLDER_BACK_ALPHA),
            hairline,
        );

        // 2. The sheets, furthest back first.
        let centre_x = origin.x + size.width / 2.0;
        for index in folder_fan_order(self.sheet_count()) {
            let placement = self.sheet(index);
            let sheet_size = Size::new(
                SHEET_WIDTH * placement.scale,
                SHEET_HEIGHT * placement.scale,
            );
            // `-ml-12` puts a sheet's own left edge half a sheet left of the
            // centre line, so the fan pivots about that line rather than about
            // the card's left edge.
            let at = Point::new(
                centre_x + placement.x - sheet_size.width / 2.0,
                origin.y + placement.y,
            );
            scene.push_layer(at, sheet_size, placement.opacity.clamp(0.0, 1.0) as f32);
            scene.push_transform(Affine::rotate_about(
                placement.rotation.to_radians(),
                Point::new(
                    at.x + sheet_size.width / 2.0,
                    at.y + sheet_size.height / 2.0,
                ),
            ));
            scene.fill_rounded_rect(
                at,
                sheet_size,
                SHEET_RADIUS,
                style::with_alpha(background, SHEET_FILL_ALPHA),
            );
            paint_panel_hairline(scene, at, sheet_size, SHEET_RADIUS, hairline);
            if let Some(label) = self.sheets.get(index) {
                let text = label.size();
                label.paint(
                    Point::new(
                        at.x + (sheet_size.width - text.width) / 2.0,
                        at.y + sheet_size.height - text.height - style::GAP_MD,
                    ),
                    dim,
                    scene,
                );
            }
            scene.pop_transform();
            scene.pop_layer();
        }

        // 3. The front flap, tilting toward the viewer, anchored at its base.
        self.paint_flap(
            scene,
            Point::new(origin.x, origin.y + size.height - FLAP_HEIGHT),
            Size::new(size.width, FLAP_HEIGHT),
            FOLDER_FLAP_TILT * fanned,
            style::with_alpha(background, FLAP_FILL_ALPHA),
            hairline,
            ink,
            dim,
        );

        if scaled {
            scene.pop_transform();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if !ctx.has_focus() || self.config.disabled {
                    return EventResult::Ignored;
                }
                if is_activation_key(key) {
                    (self.on_activate)(ctx);
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self
            .config
            .label
            .clone()
            .unwrap_or_else(|| self.config.title.clone());
        let count = self.count.content().to_string();
        let disabled = self.config.disabled;
        let open = self.open;
        ctx.push_container(
            Role::Button,
            move |node| {
                node.set_label(label.as_str());
                node.set_expanded(open);
                if disabled {
                    node.set_disabled();
                } else {
                    node.add_action(Action::Click);
                }
            },
            |ctx| {
                ctx.push_node(Role::Label, |node| {
                    node.set_label(count.as_str());
                });
                // The sheets are decoration upstream (`aria-hidden`), but their
                // labels are the only names these previews have, so they are
                // announced as the list they stand for.
                for sheet in &self.sheets {
                    let text = sheet.content().to_string();
                    ctx.push_node(Role::ListItem, |node| {
                        node.set_label(text.as_str());
                    });
                }
            },
        );
    }
}

/// Paint one tilted panel: its foreshortened box, its fill and its hairline.
///
/// `degrees` is a rotation about the horizontal axis, carried as the vertical
/// foreshortening it produces and anchored at `transform-origin: center bottom`
/// — so a tilting panel shortens toward its own base rather than about its
/// middle.
fn paint_tilted_panel(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    degrees: f64,
    fill: Color,
    hairline: Color,
) {
    let height = size.height * tilt_scale(degrees);
    if height <= 0.0 {
        return;
    }
    let at = Point::new(origin.x, origin.y + size.height - height);
    let box_size = Size::new(size.width, height);
    scene.fill_rounded_rect(at, box_size, FOLDER_RADIUS, fill);
    paint_panel_hairline(scene, at, box_size, FOLDER_RADIUS, hairline);
}

impl ProjectFolderWidget {
    /// Paint the front flap: the tilted surface, its title row and its meta row.
    #[allow(clippy::too_many_arguments)]
    fn paint_flap(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        degrees: f64,
        fill: Color,
        hairline: Color,
        ink: Color,
        dim: Color,
    ) {
        let scale = tilt_scale(degrees);
        let height = size.height * scale;
        if height <= 0.0 {
            return;
        }
        let at = Point::new(origin.x, origin.y + size.height - height);
        let box_size = Size::new(size.width, height);
        scene.fill_rounded_rect(at, box_size, FOLDER_RADIUS, fill);
        paint_panel_hairline(scene, at, box_size, FOLDER_RADIUS, hairline);

        // The two rows keep their proportions inside the foreshortened box, so
        // the flap reads as one surface turning rather than two rows sliding.
        let title_height = FLAP_TITLE_HEIGHT * scale;
        let title = self.title.size();
        self.title.paint(
            Point::new(
                at.x + FLAP_PADDING_X,
                at.y + (title_height - title.height) / 2.0,
            ),
            ink,
            scene,
        );
        let meta_y = at.y + title_height;
        scene.fill_rect(
            Point::new(at.x, meta_y),
            Size::new(box_size.width, style::BORDER_WIDTH),
            hairline,
        );
        let meta_height = FLAP_META_HEIGHT * scale;
        let count = self.count.size();
        self.count.paint(
            Point::new(
                at.x + FLAP_PADDING_X,
                meta_y + (meta_height - count.height) / 2.0,
            ),
            ink,
            scene,
        );
        let description = self.description.size();
        self.description.paint(
            Point::new(
                at.x + box_size.width - FLAP_PADDING_X - description.width,
                meta_y + (meta_height - description.height) / 2.0,
            ),
            dim,
            scene,
        );
    }

    /// The `Widget::event` pointer arm: a hover fans the folder, a press reports
    /// the activation upstream expands on.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        let size = ctx.size();
        let over = inside(p.position, size);
        match p.phase {
            PointerPhase::Move => {
                if !over {
                    return EventResult::Ignored;
                }
                ctx.claim_hover();
                if !self.config.disabled {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if !self.hovered {
                    self.hovered = true;
                    self.sync_open(ctx);
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if !presses(p) || !over || self.config.disabled {
                    return EventResult::Ignored;
                }
                ctx.capture_pointer();
                ctx.request_focus();
                self.focused = true;
                self.hovered = true;
                self.armed = true;
                self.press.retarget(1.0);
                self.sync_open(ctx);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.armed {
                    return EventResult::Ignored;
                }
                self.armed = false;
                self.press.retarget(0.0);
                ctx.request_redraw();
                if over {
                    (self.on_activate)(ctx);
                }
                EventResult::Handled
            }
            // A `Cancel` arm never reaches app state.
            PointerPhase::Cancel => {
                if !self.armed {
                    return EventResult::Ignored;
                }
                self.armed = false;
                self.press.retarget(0.0);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, ft_ms, light, pointer, reduced};
    use frust::authoring::any;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(600.0, 400.0);

    // ---- The fan math -------------------------------------------------------

    /// The defining property: the fan is symmetric about the middle sheet.
    #[test]
    fn the_fan_is_symmetric_about_its_centre() {
        let left = folder_fan(0, 5);
        let right = folder_fan(4, 5);
        assert_eq!(left.x, -right.x);
        assert_eq!(left.rotation, -right.rotation);
        assert_eq!(left.y, right.y);
        assert_eq!(left.scale, right.scale);
        assert_eq!(left.opacity, right.opacity);

        // The middle sheet sits on the line, unturned, largest and opaque...
        let centre = folder_fan(2, 5);
        assert_eq!(centre.x, 0.0);
        assert_eq!(centre.rotation, 0.0);
        assert_eq!(centre.scale, 1.04);
        assert_eq!(centre.opacity, 1.0);
        // ...and lifted furthest, by `max(0, 2 - 0) * 8`.
        assert_eq!(centre.y, SHEET_FAN_BASE_Y - 2.0 * SHEET_CENTRE_LIFT);
    }

    #[test]
    fn the_fan_steps_out_by_a_fixed_pitch_and_steps_back_by_distance() {
        let sheets: Vec<FolderSheet> = (0..5).map(|index| folder_fan(index, 5)).collect();
        for pair in sheets.windows(2) {
            assert!((pair[1].x - pair[0].x - SHEET_FAN_STEP).abs() < 1e-9);
            assert!((pair[1].rotation - pair[0].rotation - SHEET_FAN_ROTATION).abs() < 1e-9);
        }
        // Distance 0 / 1 / 2-and-beyond take the three declared rungs.
        assert_eq!((sheets[2].scale, sheets[2].opacity), (1.04, 1.0));
        assert_eq!((sheets[1].scale, sheets[1].opacity), (0.95, 0.78));
        assert_eq!((sheets[0].scale, sheets[0].opacity), (0.88, 0.58));
        // The nearer a sheet is to the centre, the nearer the viewer it sits.
        assert!(sheets[2].depth > sheets[1].depth);
        assert!(sheets[1].depth > sheets[0].depth);
    }

    #[test]
    fn an_even_fan_straddles_the_centre_line_instead_of_sitting_on_it() {
        // Four sheets: offsets -1.5, -0.5, 0.5, 1.5 — nothing at zero.
        let sheets: Vec<FolderSheet> = (0..4).map(|index| folder_fan(index, 4)).collect();
        assert!(sheets.iter().all(|sheet| sheet.x != 0.0));
        assert_eq!(sheets[1].x, -sheets[2].x);
        // The centre lift is still strongest for the innermost pair.
        assert!(sheets[1].y < sheets[0].y);
    }

    #[test]
    fn a_single_sheet_sits_dead_centre_and_an_out_of_range_index_falls_back() {
        let only = folder_fan(0, 1);
        assert_eq!(only.x, 0.0);
        assert_eq!(only.rotation, 0.0);
        // Out of range, and a zero count, both report the centre placement.
        assert_eq!(folder_fan(5, 5), folder_fan(0, 0));
        assert_eq!(folder_fan(0, 0).x, 0.0);
    }

    /// The open branch is the resting fan pushed further out, never a different
    /// shape.
    #[test]
    fn opening_pushes_every_sheet_further_out_and_brighter() {
        for index in 0..5 {
            let rest = folder_fan(index, 5);
            let open = rest.opened();
            assert!(open.x.abs() >= rest.x.abs(), "sheet {index} came in");
            assert!(open.y < rest.y, "sheet {index} did not lift");
            assert!(
                open.rotation.abs() >= rest.rotation.abs(),
                "sheet {index} unturned"
            );
            assert!(open.scale > rest.scale);
            assert!(open.opacity >= rest.opacity);
            assert!(open.opacity <= 1.0, "opacity capped");
            assert_eq!(open.depth, rest.depth, "the stack order never changes");
        }
        // The centre sheet is already opaque, so its bonus is absorbed.
        assert_eq!(folder_fan(2, 5).opened().opacity, 1.0);
    }

    #[test]
    fn a_half_played_fan_sits_between_its_two_placements() {
        let rest = folder_fan(0, 5);
        let open = rest.opened();
        assert_eq!(rest.lerp(open, 0.0), rest);
        assert_eq!(rest.lerp(open, 1.0), open);
        let mid = rest.lerp(open, 0.5);
        assert!(mid.x < rest.x && mid.x > open.x, "x travels: {}", mid.x);
        assert!(mid.y < rest.y && mid.y > open.y);
        assert!(mid.opacity > rest.opacity && mid.opacity < open.opacity);
    }

    #[test]
    fn the_paint_order_puts_the_furthest_back_sheet_first() {
        let order = folder_fan_order(5);
        assert_eq!(order.len(), 5);
        let depths: Vec<i32> = order.iter().map(|&i| folder_fan(i, 5).depth).collect();
        for pair in depths.windows(2) {
            assert!(pair[0] <= pair[1], "out of depth order: {depths:?}");
        }
        // The centre sheet paints last, so nothing covers it.
        assert_eq!(*order.last().expect("a sheet"), 2);
        // ...and the cap holds however many previews were declared.
        assert_eq!(folder_fan_order(9).len(), FOLDER_MAX_PREVIEWS);
        assert!(folder_fan_order(0).is_empty());
    }

    // ---- The small pure helpers ---------------------------------------------

    #[test]
    fn the_count_line_pluralises_everything_but_one() {
        assert_eq!(folder_count_text(1, "file"), "1 file");
        assert_eq!(folder_count_text(0, "file"), "0 files");
        assert_eq!(folder_count_text(12, "file"), "12 files");
        assert_eq!(folder_count_text(3, "asset"), "3 assets");
    }

    #[test]
    fn a_tilt_foreshortens_and_never_inverts() {
        assert_eq!(tilt_scale(0.0), 1.0);
        // The two ported tilts shorten by the same factor either way round.
        assert!((tilt_scale(FOLDER_BACK_TILT) - tilt_scale(-FOLDER_BACK_TILT)).abs() < 1e-12);
        assert!(tilt_scale(FOLDER_FLAP_TILT) < tilt_scale(FOLDER_BACK_TILT));
        assert!(tilt_scale(FOLDER_FLAP_TILT) > 0.0);
        // Past the horizon it flattens rather than turning inside out.
        assert_eq!(tilt_scale(120.0), 0.0);
    }

    // ---- The mounted card ---------------------------------------------------

    #[derive(Default)]
    struct App {
        opens: Vec<bool>,
        activations: u32,
        controlled: Option<bool>,
        disabled: bool,
    }

    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
        clock: f64,
    }

    impl Harness {
        fn new() -> Self {
            Self::themed(light())
        }

        fn themed(theme: Theme) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App::default(),
                tcx: TextContext::new(),
                clock: 0.0,
            };
            h.root.set_theme(Box::new(theme));
            h.step(0.0);
            h
        }

        fn step(&mut self, ms: f64) -> Recorder {
            self.clock += ms;
            let mut logic = move |s: &mut App| {
                let mut card = project_folder(
                    "Brand refresh",
                    vec![
                        folder_preview("Logo"),
                        folder_preview("Type"),
                        folder_preview("Colour"),
                    ],
                )
                .disabled(s.disabled)
                .on_open_change(|s: &mut App, open| s.opens.push(open))
                .on_activate(|s: &mut App| s.activations += 1);
                if let Some(open) = s.controlled {
                    card = card.open(open);
                }
                frust::Stack(vec![any(card)])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let mut rec = Recorder::default();
            let now = self.clock;
            self.root.paint(&mut rec, ft_ms(now));
            rec
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }
    }

    #[test]
    fn a_hover_opens_the_fan_and_reports_it_once() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Move, 40.0, 40.0));
        assert_eq!(h.state.opens, vec![true]);
        // A second move inside the card is not a second open.
        h.event(pointer(PointerPhase::Move, 60.0, 60.0));
        assert_eq!(h.state.opens, vec![true]);
    }

    #[test]
    fn a_disabled_card_never_opens_and_never_activates() {
        let mut h = Harness::new();
        h.state.disabled = true;
        h.step(0.0);
        h.event(pointer(PointerPhase::Move, 40.0, 40.0));
        h.event(pointer(PointerPhase::Down, 40.0, 40.0));
        h.event(pointer(PointerPhase::Up, 40.0, 40.0));
        assert!(h.state.opens.is_empty());
        assert_eq!(h.state.activations, 0);
    }

    #[test]
    fn a_press_reports_the_activation_the_overlay_would_have_been() {
        let mut h = Harness::new();
        h.event(pointer(PointerPhase::Down, 40.0, 40.0));
        h.event(pointer(PointerPhase::Up, 40.0, 40.0));
        assert_eq!(h.state.activations, 1);
        // A press that leaves the card does not activate.
        h.event(pointer(PointerPhase::Down, 40.0, 40.0));
        h.event(pointer(PointerPhase::Up, 900.0, 900.0));
        assert_eq!(h.state.activations, 1);
        // ...and a cancelled press never reaches app state.
        h.event(pointer(PointerPhase::Down, 40.0, 40.0));
        h.event(pointer(PointerPhase::Cancel, 40.0, 40.0));
        assert_eq!(h.state.activations, 1);
    }

    #[test]
    fn a_controlled_card_shows_what_it_is_told_and_only_reports() {
        let mut h = Harness::new();
        h.state.controlled = Some(false);
        h.step(0.0);
        h.event(pointer(PointerPhase::Move, 40.0, 40.0));
        // It asked, but it did not fan itself.
        assert_eq!(h.state.opens, vec![true]);
        let closed = h.step(0.0);
        h.state.controlled = Some(true);
        h.step(0.0);
        let opening = h.step(3_000.0);
        assert_ne!(
            closed.rrects, opening.rrects,
            "the app's own flag is what fans the sheets"
        );
    }

    #[test]
    fn the_card_lays_out_at_its_declared_box_and_paints_one_sheet_per_preview() {
        let mut h = Harness::new();
        let rec = h.step(0.0);
        // The back panel, three sheets and the flap, all rounded.
        assert!(rec.rrects.len() >= 5, "{}", rec.rrects.len());
        let (back_at, back_size, _, _) = rec.rrects[0];
        assert_eq!(back_size.width, FOLDER_WIDTH);
        // Closed, the back panel is untilted and so full height.
        assert_eq!(back_size.height, FOLDER_HEIGHT);
        assert_eq!(back_at.y, 0.0);
        // One alpha layer per sheet.
        assert_eq!(rec.layers.len(), 3);
    }

    #[test]
    fn opening_tilts_the_back_panel_toward_its_own_base() {
        let mut h = Harness::new();
        let closed = h.step(0.0);
        h.state.controlled = Some(true);
        h.step(0.0);
        // Let the fan spring settle.
        let open = h.step(3_000.0);
        let (closed_at, closed_size, _, _) = closed.rrects[0];
        let (open_at, open_size, _, _) = open.rrects[0];
        assert!(
            open_size.height < closed_size.height,
            "the back panel did not foreshorten"
        );
        assert!(
            open_at.y > closed_at.y,
            "it shortened toward its base, not about its middle"
        );
    }

    #[test]
    fn reduced_motion_places_the_fan_without_asking_for_a_frame() {
        let mut h = Harness::themed(reduced());
        h.state.controlled = Some(true);
        h.step(0.0);
        let first = h.step(0.0);
        let second = h.step(16.0);
        assert_eq!(
            first.rrects, second.rrects,
            "a reduced-motion fan is already where it is going"
        );
    }
}
